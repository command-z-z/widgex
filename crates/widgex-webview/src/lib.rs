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
    path::{Path, PathBuf},
    rc::Rc,
};

use anyhow::{Context, Result, anyhow};
use gtk::prelude::*;
use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use rust_embed::RustEmbed;
use widgex_core::{
    AnchorEdge, DataSource, RendererPayload, RendererSource, RendererWidget, RendererWindow,
    SourceKind, SourceSnapshot, WindowLayer, resolve_payload,
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
    let mut base = payload.clone();
    inline_theme_css(&mut base, config_dir.as_ref());

    let window_spec = select_window(&base, window_id)?.clone();

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

    // Initial snapshot so the page has data before the first poll tick.
    let initial = resolve_payload(&base, &widgex_source::poll_all(sources));
    let init_script = format!(
        "window.__WIDGEX_PAYLOAD__ = {};",
        serde_json::to_string(&initial)?
    );

    let webview = WebViewBuilder::new()
        .with_url("widgex://localhost/index.html")
        .with_transparent(true)
        .with_initialization_script(init_script)
        .with_custom_protocol("widgex".to_string(), serve_asset)
        .with_ipc_handler(|request: Request<String>| {
            // on_click actions are wired in a later milestone; log for now.
            eprintln!("widgex ipc: {}", request.body());
        })
        .build_gtk(&window)
        .map_err(|error| anyhow!("failed to create webview: {error}"))?;
    let webview = Rc::new(webview);

    window.show_all();
    window.connect_destroy(|_| gtk::main_quit());

    // Poll loop: re-resolve the payload and push it into the page.
    let tick = widgex_source::tick_interval(sources);
    let sources = sources.to_vec();
    let push_webview = Rc::clone(&webview);
    gtk::glib::timeout_add_local(tick, move || {
        let resolved = resolve_payload(&base, &widgex_source::poll_all(&sources));
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

/// Custom-protocol handler: serve the embedded renderer bundle.
fn serve_asset(_id: wry::WebViewId<'_>, request: Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let raw = request.uri().path().trim_start_matches('/');
    let path = if raw.is_empty() { "index.html" } else { raw };

    match RendererAsset::get(path) {
        Some(file) => Response::builder()
            .header(CONTENT_TYPE, mime_for(path))
            .body(file.data)
            .unwrap(),
        None => Response::builder()
            .status(404)
            .body(Cow::Owned(Vec::new()))
            .unwrap(),
    }
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

/// Replace `payload.theme_css` (a path relative to the config directory) with
/// the CSS file's contents, so the renderer can inject it directly.
fn inline_theme_css(payload: &mut RendererPayload, config_dir: &Path) {
    let Some(css_ref) = payload.theme_css.take() else {
        return;
    };

    let css_path = {
        let path = PathBuf::from(&css_ref);
        if path.is_absolute() {
            path
        } else {
            config_dir.join(path)
        }
    };

    payload.theme_css = std::fs::read_to_string(&css_path).ok();
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
