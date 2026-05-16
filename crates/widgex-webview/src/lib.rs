//! Webview-backed widget window.
//!
//! A [`RendererPayload`] is handed to a SolidJS bundle running inside a
//! `webkit2gtk` webview. The bundle is embedded by `rust-embed` and served
//! over a `widgex://` custom protocol. The GTK window itself is anchored as a
//! desktop layer via `gtk-layer-shell` — the renderer draws the content, the
//! layer-shell window provides the desktop-widget chrome.
//!
//! Data sources are polled in-process: every tick the payload is re-resolved
//! against fresh [`SourceSnapshot`]s and pushed into the page via
//! `window.__widgexPush`.
//!
//! This milestone targets Linux/Wayland only. The cross-platform window layer
//! (tao + per-platform adapters) is a later milestone — see `PlatformAdapter`
//! in `widgex-platform`.

use std::{
    borrow::Cow,
    collections::BTreeMap,
    io::{BufRead, BufReader},
    path::Component,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    rc::Rc,
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use gtk::prelude::*;
use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use rust_embed::RustEmbed;
use serde::Deserialize;
use widgex_core::{
    Action, AnchorEdge, DataSource, RendererPayload, RendererSource, RendererWidget,
    RendererWindow, SourceKind, SourceMode, SourceSnapshot, WindowLayer, resolve_payload,
};
use wry::{
    WebViewBuilder, WebViewBuilderExtUnix,
    http::{Request, Response, header::CONTENT_TYPE},
};

/// The built SolidJS renderer bundle. In debug builds `rust-embed` reads these
/// files from disk at runtime, so `npm run build` is picked up without a
/// `cargo` rebuild; release builds embed them into the binary.
#[derive(RustEmbed)]
#[folder = "../../apps/renderer/dist"]
struct RendererAsset;

/// A non-GUI summary of a window, used by `widgex open --dry-run`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowPreview {
    pub id: String,
    pub title: Option<String>,
    pub width: u32,
    pub height: u32,
    pub text_preview: String,
}

const DEFAULT_WIDTH: u32 = 320;
const DEFAULT_HEIGHT: u32 = 120;

/// Build a non-GUI preview of the selected window. Binding templates are
/// resolved against deterministic example snapshots so the output is stable.
pub fn build_window_preview(
    payload: &RendererPayload,
    window_id: Option<&str>,
) -> Result<WindowPreview> {
    let resolved = resolve_payload(payload, &example_snapshots(&payload.sources));
    let window = select_window(&resolved, window_id)?;

    Ok(WindowPreview {
        id: window.id.clone(),
        title: window.title.clone(),
        width: window.size.width.unwrap_or(DEFAULT_WIDTH),
        height: window.size.height.unwrap_or(DEFAULT_HEIGHT),
        text_preview: first_text_preview(window).unwrap_or_default(),
    })
}

/// Open the selected window as a layer-shell-anchored webview and run the GTK
/// event loop until the window is closed. `sources` drives the live poll loop.
pub fn run_widget_window(
    payload: &RendererPayload,
    config_dir: impl AsRef<Path>,
    window_id: Option<&str>,
    sources: &[DataSource],
    allow_shell: bool,
) -> Result<()> {
    // webkit2gtk's DMABUF renderer triggers "Protocol error" on many Wayland
    // compositors (and inside VMs/containers), producing a blank window.
    // Disable it before GTK/webkit start unless the user already chose a value.
    // SAFETY: runs before GTK spawns any thread — the process is single-threaded.
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }

    gtk::init().context("failed to initialize GTK")?;

    // `base` carries the unresolved templates; it is re-resolved every tick.
    // Theme CSS is inlined here so the renderer receives content, not a path.
    let config_dir = config_dir.as_ref().to_path_buf();
    let mut base = payload.clone();
    inline_theme_css(&mut base, &config_dir);

    let window_spec = select_window(&base, window_id)?.clone();
    base.windows = vec![window_spec.clone()];

    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title(window_spec.title.as_deref().unwrap_or(&window_spec.id));
    window.set_decorated(false);
    window.set_app_paintable(true);

    // A layer-shell surface anchored to non-opposite edges takes its size from
    // the child's natural size — and a webview reports 0x0. Force the configured
    // size so the surface does not collapse. `-1` lets an axis stretch when the
    // window is anchored to both of that axis's edges (e.g. a full-width bar).
    let width = window_spec.size.width.map_or(-1, |value| value as i32);
    let height = window_spec.size.height.map_or(-1, |value| value as i32);
    window.set_size_request(width, height);
    window.set_default_size(
        window_spec.size.width.unwrap_or(DEFAULT_WIDTH) as i32,
        window_spec.size.height.unwrap_or(DEFAULT_HEIGHT) as i32,
    );

    window.init_layer_shell();
    window.set_namespace("widgex");
    window.set_layer(map_layer(window_spec.layer));
    window.set_keyboard_mode(KeyboardMode::None);

    for edge in [
        AnchorEdge::Top,
        AnchorEdge::Right,
        AnchorEdge::Bottom,
        AnchorEdge::Left,
    ] {
        window.set_anchor(map_edge(edge), window_spec.anchor.contains(&edge));
    }
    window.set_layer_shell_margin(Edge::Top, window_spec.margin.top as i32);
    window.set_layer_shell_margin(Edge::Right, window_spec.margin.right as i32);
    window.set_layer_shell_margin(Edge::Bottom, window_spec.margin.bottom as i32);
    window.set_layer_shell_margin(Edge::Left, window_spec.margin.left as i32);
    window.set_exclusive_zone(window_spec.exclusive_zone.unwrap_or(0));

    let listener_rx = start_listeners(sources, config_dir.clone());
    let mut latest_listen_snapshots = BTreeMap::<String, SourceSnapshot>::new();
    // Empty snapshots for the initial render — window opens immediately without
    // blocking on shell commands. Real data arrives on the first poll tick (~500 ms).
    let initial = resolve_payload(&base, &[]);
    let init_script = format!(
        "window.__WIDGEX_PAYLOAD__ = {};",
        serde_json::to_string(&initial)?
    );

    let webview = WebViewBuilder::new()
        .with_url("widgex://localhost/index.html")
        .with_transparent(true)
        .with_initialization_script(init_script)
        .with_custom_protocol("widgex".to_string(), {
            let config_dir = config_dir.clone();
            move |id, request| serve_asset(id, request, &config_dir)
        })
        .with_ipc_handler({
            let config_dir = config_dir.clone();
            move |request: Request<String>| {
                if let Err(error) = handle_widget_event(request.body(), &config_dir, allow_shell) {
                    eprintln!("widgex ipc error: {error}");
                }
            }
        })
        .build_gtk(&window)
        .map_err(|error| anyhow!("failed to create webview: {error}"))?;
    let webview = Rc::new(webview);

    window.show_all();
    if window_spec.click_through {
        let empty_input = gtk::cairo::Region::create();
        window.input_shape_combine_region(Some(&empty_input));
    }
    window.connect_destroy(|_| gtk::main_quit());

    // Per-source poll threads (对齐 Eww defpoll 架构): each source runs on its
    // own timer so a slow source (e.g. lyrics.py doing an HTTP fetch on song
    // switch) only blocks itself — progress, icons, etc. keep updating.
    let poll_rx = start_pollers(sources, config_dir.clone());
    let mut latest_poll_snapshots = BTreeMap::<String, SourceSnapshot>::new();
    let push_webview = Rc::clone(&webview);
    gtk::glib::timeout_add_local(Duration::from_millis(100), move || {
        while let Ok(snapshot) = poll_rx.try_recv() {
            latest_poll_snapshots.insert(snapshot.id.clone(), snapshot);
        }
        drain_listener_snapshots(&listener_rx, &mut latest_listen_snapshots);
        let mut snapshots: Vec<SourceSnapshot> = latest_poll_snapshots.values().cloned().collect();
        snapshots.extend(latest_listen_snapshots.values().cloned());
        let resolved = resolve_payload(&base, &snapshots);
        if let Ok(json) = serde_json::to_string(&resolved) {
            let _ = push_webview.evaluate_script(&format!(
                "window.__widgexPush && window.__widgexPush({json})"
            ));
        }
        gtk::glib::ControlFlow::Continue
    });

    gtk::main();
    Ok(())
}

#[derive(Debug, Deserialize)]
struct WidgetEvent {
    action: Action,
    #[serde(default)]
    value: Option<String>,
}

pub fn handle_widget_event(body: &str, config_dir: &Path, allow_shell: bool) -> Result<()> {
    let event: WidgetEvent = serde_json::from_str(body).context("invalid widget event")?;
    execute_action(
        &event.action,
        event.value.as_deref(),
        config_dir,
        allow_shell,
    )
}

pub fn execute_action(
    action: &Action,
    value: Option<&str>,
    config_dir: &Path,
    allow_shell: bool,
) -> Result<()> {
    match action {
        Action::Command { command } => {
            if !allow_shell {
                return Err(anyhow!(
                    "command action requires permissions.allow_shell = true"
                ));
            }
            let command = value
                .map(|value| command.replace("{}", value).replace("{{ value }}", value))
                .unwrap_or_else(|| command.clone());
            Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(config_dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .context("failed to execute command action")?;
        }
        Action::Emit { event } => {
            eprintln!("widgex event emitted: {event}");
        }
    }
    Ok(())
}

fn start_listeners(sources: &[DataSource], cwd: PathBuf) -> Receiver<SourceSnapshot> {
    let (tx, rx) = mpsc::channel();
    for source in sources
        .iter()
        .filter(|source| source.kind == SourceKind::Shell && source.mode == SourceMode::Listen)
        .cloned()
    {
        let tx = tx.clone();
        let cwd = cwd.clone();
        thread::spawn(move || {
            loop {
                let Some(command) = source.command.as_deref() else {
                    return;
                };
                let mut child = match Command::new("sh")
                    .arg("-c")
                    .arg(command)
                    .current_dir(&cwd)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                {
                    Ok(child) => child,
                    Err(_) => return,
                };

                let Some(stdout) = child.stdout.take() else {
                    let _ = child.wait();
                    return;
                };

                for line in BufReader::new(stdout)
                    .lines()
                    .map_while(std::result::Result::ok)
                {
                    let mut snapshot = SourceSnapshot::new(&source.id);
                    snapshot.fields = widgex_source::parse_shell_output(source.format, &line);
                    if tx.send(snapshot).is_err() {
                        let _ = child.kill();
                        let _ = child.wait();
                        return;
                    }
                }

                let _ = child.wait();
                thread::sleep(std::time::Duration::from_millis(
                    source.interval_ms.unwrap_or(1000).max(100),
                ));
            }
        });
    }
    rx
}

/// Each poll-mode source gets its own background thread with its own interval,
/// mirroring Eww's `defpoll` design: a slow source only blocks itself.
fn start_pollers(sources: &[DataSource], cwd: PathBuf) -> Receiver<SourceSnapshot> {
    let (tx, rx) = mpsc::channel();
    for source in sources
        .iter()
        .filter(|s| s.mode == SourceMode::Poll)
        .cloned()
    {
        let tx = tx.clone();
        let cwd = cwd.clone();
        let interval = Duration::from_millis(source.interval_ms.unwrap_or(1000).max(100));
        thread::spawn(move || {
            loop {
                let snapshot = widgex_source::poll_source_with_dir(&source, &cwd);
                if tx.send(snapshot).is_err() {
                    break;
                }
                thread::sleep(interval);
            }
        });
    }
    rx
}

fn drain_listener_snapshots(
    rx: &Receiver<SourceSnapshot>,
    latest: &mut BTreeMap<String, SourceSnapshot>,
) {
    while let Ok(snapshot) = rx.try_recv() {
        latest.insert(snapshot.id.clone(), snapshot);
    }
}

/// Custom-protocol handler: serve the embedded renderer bundle.
fn serve_asset(
    _id: wry::WebViewId<'_>,
    request: Request<Vec<u8>>,
    config_dir: &Path,
) -> Response<Cow<'static, [u8]>> {
    let raw = request.uri().path().trim_start_matches('/');
    let path = if raw.is_empty() { "index.html" } else { raw };

    match RendererAsset::get(path) {
        Some(file) => Response::builder()
            .header(CONTENT_TYPE, mime_for(path))
            .body(file.data)
            .unwrap(),
        None => serve_config_file(config_dir, path),
    }
}

fn serve_config_file(config_dir: &Path, path: &str) -> Response<Cow<'static, [u8]>> {
    if !is_safe_relative_path(path) {
        return Response::builder()
            .status(403)
            .body(Cow::Owned(Vec::new()))
            .unwrap();
    }

    let file_path = config_dir.join(path);
    match std::fs::read(&file_path) {
        Ok(bytes) => Response::builder()
            .header(CONTENT_TYPE, mime_for(path))
            .body(Cow::Owned(bytes))
            .unwrap(),
        Err(_) => Response::builder()
            .status(404)
            .body(Cow::Owned(Vec::new()))
            .unwrap(),
    }
}

fn is_safe_relative_path(path: &str) -> bool {
    Path::new(path)
        .components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

fn mime_for(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, ext)| ext) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "application/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// Replace theme CSS path references with concatenated CSS contents so the
/// renderer can inject one style tag.
pub fn inline_theme_css(payload: &mut RendererPayload, config_dir: &Path) {
    let css_refs = if payload.theme_css_files.is_empty() {
        payload
            .theme_css
            .take()
            .into_iter()
            .collect::<Vec<String>>()
    } else {
        payload.theme_css.take();
        std::mem::take(&mut payload.theme_css_files)
    };

    if css_refs.is_empty() {
        return;
    }

    let css = css_refs
        .into_iter()
        .filter_map(|css_ref| {
            let path = PathBuf::from(&css_ref);
            let css_path = if path.is_absolute() {
                path
            } else {
                config_dir.join(path)
            };
            std::fs::read_to_string(&css_path).ok()
        })
        .collect::<Vec<_>>()
        .join("\n");

    payload.theme_css = Some(css);
}

fn select_window<'a>(
    payload: &'a RendererPayload,
    window_id: Option<&str>,
) -> Result<&'a RendererWindow> {
    match window_id {
        Some(window_id) => payload
            .windows
            .iter()
            .find(|window| window.id == window_id)
            .ok_or_else(|| anyhow!("window {window_id:?} not found in config")),
        None => payload
            .windows
            .first()
            .ok_or_else(|| anyhow!("config does not define any windows")),
    }
}

fn first_text_preview(window: &RendererWindow) -> Option<String> {
    window.widgets.iter().find_map(first_widget_text)
}

fn first_widget_text(widget: &RendererWidget) -> Option<String> {
    widget
        .text
        .clone()
        .or_else(|| widget.children.iter().find_map(first_widget_text))
}

/// Deterministic per-kind example values, used only for `--dry-run` previews.
fn example_snapshots(sources: &[RendererSource]) -> Vec<SourceSnapshot> {
    sources
        .iter()
        .map(|source| {
            let snapshot = SourceSnapshot::new(&source.id);
            match source.kind {
                SourceKind::Time => snapshot
                    .with_field("now", "12:34:56")
                    .with_field("date", "2026-01-01"),
                SourceKind::Battery => snapshot
                    .with_field("percent", "84")
                    .with_field("level", "Normal")
                    .with_field("status", "Discharging"),
                SourceKind::Cpu | SourceKind::Memory | SourceKind::Network | SourceKind::Shell => {
                    snapshot
                }
            }
        })
        .collect()
}

fn map_layer(layer: WindowLayer) -> Layer {
    match layer {
        WindowLayer::Background => Layer::Background,
        WindowLayer::Bottom => Layer::Bottom,
        WindowLayer::Top => Layer::Top,
        WindowLayer::Overlay => Layer::Overlay,
    }
}

fn map_edge(edge: AnchorEdge) -> Edge {
    match edge {
        AnchorEdge::Top => Edge::Top,
        AnchorEdge::Bottom => Edge::Bottom,
        AnchorEdge::Left => Edge::Left,
        AnchorEdge::Right => Edge::Right,
    }
}

#[cfg(test)]
mod tests {
    use super::is_safe_relative_path;

    #[test]
    fn config_asset_paths_reject_traversal_and_absolute_paths() {
        assert!(is_safe_relative_path("spotify_cache/album_art.png"));
        assert!(is_safe_relative_path("./spotify_cache/default.png"));
        assert!(!is_safe_relative_path("../secret"));
        assert!(!is_safe_relative_path("/etc/passwd"));
    }
}
