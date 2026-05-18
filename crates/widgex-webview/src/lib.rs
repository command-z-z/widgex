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
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    io::{self, BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::Component,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    rc::Rc,
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

#[allow(unused_imports)]
use libc;

use anyhow::{Context, Result, anyhow};
use gtk::prelude::*;
use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use rust_embed::RustEmbed;
use serde::Deserialize;
use widgex_core::{
    Action, AnchorEdge, DataSource, RendererPayload, RendererSource, RendererWidget,
    RendererWindow, SourceKind, SourceMode, SourceSnapshot, WindowLayer, resolve_payload,
};

mod native_renderer;
use native_renderer::NativeRenderer;
use widgex_ipc::{RendererRequest, RendererResponse};
use webkit2gtk::WebView as GtkWebView;
use wry::{
    WebContext, WebViewBuilder, WebViewBuilderExtUnix, WebViewExtUnix,
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
    // Seed HyprlandEvent listen sources with the active workspace so workspace
    // indicators are correct on first render, before the socket delivers any event.
    let mut latest_listen_snapshots: BTreeMap<String, SourceSnapshot> =
        widgex_source::seed_listen_snapshots(sources)
            .into_iter()
            .map(|s| (s.id.clone(), s))
            .collect();
    // Empty snapshots for the initial render — window opens immediately without
    // blocking on shell commands. Real data arrives on the first poll tick (~500 ms).
    let initial = resolve_payload(&base, &[]);
    let init_script = format!(
        "window.__WIDGEX_PAYLOAD__ = {};",
        serde_json::to_string(&initial)?
    );

    // Pre-define __widgexPush as a queue so payloads sent before SolidJS
    // finishes mounting are buffered rather than dropped. App.tsx drains this
    // queue on mount and replaces the function with the real implementation.
    let init_script = format!(
        "{init_script}\
        window.__widgex_queue=[];\
        window.__widgexPush=function(p){{window.__widgex_queue.push(p);}};",
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
            let action_dir = window_spec.working_dir.clone()
                .unwrap_or_else(|| config_dir.clone());
            move |request: Request<String>| {
                if let Err(error) = handle_widget_event(request.body(), &action_dir, allow_shell) {
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
    let mut last_pushed_json = String::new();
    // 16 ms ≈ one display frame: listener events (e.g. Hyprland workspace switches)
    // are drained within one frame instead of waiting up to 100 ms. We only call
    // evaluate_script when the resolved JSON actually changed, so there is no
    // extra IPC cost on ticks where nothing moved.
    gtk::glib::timeout_add_local(Duration::from_millis(16), move || {
        let mut dirty = false;
        while let Ok(snapshot) = poll_rx.try_recv() {
            latest_poll_snapshots.insert(snapshot.id.clone(), snapshot);
            dirty = true;
        }
        if drain_listener_snapshots(&listener_rx, &mut latest_listen_snapshots) {
            dirty = true;
        }
        if !dirty {
            return gtk::glib::ControlFlow::Continue;
        }
        let mut snapshots: Vec<SourceSnapshot> = latest_poll_snapshots.values().cloned().collect();
        snapshots.extend(latest_listen_snapshots.values().cloned());
        let resolved = resolve_payload(&base, &snapshots);
        if let Ok(json) = serde_json::to_string(&resolved) {
            if json != last_pushed_json {
                let _ = push_webview.evaluate_script(&format!(
                    "window.__widgexPush && window.__widgexPush({json})"
                ));
                last_pushed_json = json;
            }
        }
        gtk::glib::ControlFlow::Continue
    });

    gtk::main();
    // Kill any shell-listen child processes that were spawned by listener threads.
    // When the daemon launches this process it calls process_group(0), making our
    // PID equal to our PGID. All listener-thread children inherit that group. If we
    // are the group leader we send SIGTERM to the group (temporarily ignoring it
    // ourselves) so orphaned children do not outlive the window close. When the user
    // runs widgex --foreground directly in a terminal the PGID != PID check keeps us
    // from killing unrelated processes in the terminal's process group.
    #[cfg(unix)]
    unsafe {
        let my_pid = std::process::id() as libc::pid_t;
        if libc::getpgid(0) == my_pid {
            let saved = libc::signal(libc::SIGTERM, libc::SIG_IGN);
            libc::killpg(my_pid, libc::SIGTERM);
            libc::signal(libc::SIGTERM, saved);
        }
    }
    Ok(())
}

// ── Multi-window renderer ────────────────────────────────────────────────────

/// Internal state shared between the GTK tick closure and open/close helpers.
/// GTK is single-threaded, so `Rc<RefCell<…>>` is sufficient — no Arc/Mutex.
struct RendererState {
    windows: BTreeMap<String, ManagedWindow>,
    global_poll_snapshots: BTreeMap<String, SourceSnapshot>,
    global_listen_snapshots: BTreeMap<String, SourceSnapshot>,
    base_payload: RendererPayload,
    config_dir: PathBuf,
    allow_shell: bool,
    /// Must outlive all webviews — kept here so it lives as long as RendererState.
    _web_context: WebContext,
    /// Invisible 1×1 window hosting the anchor WebView; must stay alive.
    _anchor_gtk_window: gtk::OffscreenWindow,
    /// Wry handle for the anchor WebView; keeps the GObject ref alive.
    _anchor_webview: wry::WebView,
    /// webkit2gtk handle cloned and passed as `related_view` to each new window,
    /// so all widget WebViews share one WebKitWebProcess with the anchor.
    anchor_wkwebview: GtkWebView,
}

/// One GTK window tracked by the multi-window runner.
/// Either a webkit WebView window or a native GTK renderer window.
enum ManagedWindow {
    Webkit {
        gtk_window: gtk::Window,
        webview: Rc<wry::WebView>,
        last_pushed_json: String,
    },
    Native {
        renderer: NativeRenderer,
    },
}

impl ManagedWindow {
    fn gtk_window(&self) -> &gtk::Window {
        match self {
            ManagedWindow::Webkit { gtk_window, .. } => gtk_window,
            ManagedWindow::Native { renderer } => &renderer.window,
        }
    }
}

/// Open the selected widget window as a layer-shell-anchored webview.
///
/// `anchor` is the always-alive WebView that all windows pass as `related_view`
/// to webkit2gtk, placing them in the same WebKitWebProcess.
///
/// Returns the `ManagedWindow` on success. The caller must insert it into
/// `state.windows`.
fn add_window(
    window_id: &str,
    state: &RendererState,
    destroyed_ids: Rc<RefCell<BTreeSet<String>>>,
    anchor: &GtkWebView,
) -> Result<ManagedWindow> {
    let window_spec = select_window(&state.base_payload, Some(window_id))?.clone();

    // Linux-only workaround for webkit2gtk transparent-window ghost pixels (wry#1524).
    // When native_render = true, use a GTK widget tree instead of a WebView.
    #[cfg(target_os = "linux")]
    if window_spec.native_render {
        let all_css = state.base_payload.theme_css.as_deref();
        let renderer = NativeRenderer::new(&window_spec, &state.config_dir, all_css)
            .ok_or_else(|| anyhow!("failed to create native renderer for {window_id:?}"))?;
        let id_for_destroy = window_id.to_string();
        renderer.window.connect_destroy(move |_| {
            destroyed_ids.borrow_mut().insert(id_for_destroy.clone());
        });
        return Ok(ManagedWindow::Native { renderer });
    }

    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title(window_spec.title.as_deref().unwrap_or(&window_spec.id));
    window.set_decorated(false);
    window.set_app_paintable(true);

    let width = window_spec.size.width.map_or(-1, |v| v as i32);
    let height = window_spec.size.height.map_or(-1, |v| v as i32);
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

    // Seed the initial payload (empty snapshots; real data arrives shortly).
    let initial = resolve_payload(&state.base_payload, &[]);
    let init_script = format!(
        "window.__WIDGEX_PAYLOAD__ = {};\
         window.__WIDGEX_WINDOW_ID__ = {};\
         window.__widgex_queue=[];\
         window.__widgexPush=function(p){{window.__widgex_queue.push(p);}};",
        serde_json::to_string(&initial)?,
        serde_json::to_string(window_id)?
    );

    let config_dir = state.config_dir.clone();
    let allow_shell = state.allow_shell;
    let action_dir = window_spec
        .working_dir
        .clone()
        .unwrap_or_else(|| config_dir.clone());

    // with_related_view places this WebView in the same WebKitWebProcess as the
    // anchor and inherits its WebContext (including the widgex:// protocol).
    let webview = WebViewBuilder::new()
        .with_related_view(anchor.clone())
        .with_url("widgex://localhost/index.html")
        .with_transparent(true)
        .with_initialization_script(init_script)
        .with_ipc_handler({
            move |request: Request<String>| {
                if let Err(e) = handle_widget_event(request.body(), &action_dir, allow_shell) {
                    eprintln!("widgex ipc error: {e}");
                }
            }
        })
        .build_gtk(&window)
        .map_err(|e| anyhow!("failed to create webview: {e}"))?;
    let webview = Rc::new(webview);

    window.show_all();
    if window_spec.click_through {
        let empty_input = gtk::cairo::Region::create();
        window.input_shape_combine_region(Some(&empty_input));
    }

    // Notify the tick loop that this window was destroyed (e.g. user closed it).
    let id_for_destroy = window_id.to_string();
    window.connect_destroy(move |_| {
        destroyed_ids.borrow_mut().insert(id_for_destroy.clone());
    });

    Ok(ManagedWindow::Webkit {
        gtk_window: window,
        webview,
        last_pushed_json: String::new(),
    })
}


/// Open all `initial_window_ids` windows and run the GTK main loop.
///
/// A single shared [`WebContext`] is used for all webviews — this gives one
/// WebKit network process for all windows and one `widgex://` protocol
/// registration. A non-blocking [`UnixListener`] is polled every 16 ms so
/// the daemon can open/close windows while the loop is running.
pub fn run_renderer(
    payload: &RendererPayload,
    config_dir: impl AsRef<Path>,
    sources: &[DataSource],
    allow_shell: bool,
    control_socket_path: &Path,
    initial_window_ids: &[&str],
) -> Result<()> {
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        // SAFETY: runs before GTK spawns any threads.
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }

    gtk::init().context("failed to initialize GTK")?;

    let config_dir = config_dir.as_ref().to_path_buf();
    let mut base = payload.clone();
    inline_theme_css(&mut base, &config_dir);

    // Start poll + listen threads.
    let poll_rx = start_pollers(sources, config_dir.clone());
    let listener_rx = start_listeners(sources, config_dir.clone());

    // Seed listen snapshots (e.g. active Hyprland workspace).
    let initial_listen: BTreeMap<String, SourceSnapshot> =
        widgex_source::seed_listen_snapshots(sources)
            .into_iter()
            .map(|s| (s.id.clone(), s))
            .collect();

    // One shared WebContext → one WebKit network process for all windows.
    // The widgex:// protocol is registered on this context via the anchor WebView.
    let mut web_context = WebContext::new(None);

    // Anchor WebView: invisible 1×1 offscreen window that is the stable
    // `related_view` reference for every widget WebView. All widget WebViews
    // share the anchor's renderer process instead of spawning one each.
    let anchor_gtk_window = gtk::OffscreenWindow::new();
    anchor_gtk_window.set_size_request(1, 1);
    anchor_gtk_window.show_all();
    let anchor_webview = WebViewBuilder::new_with_web_context(&mut web_context)
        .with_custom_protocol("widgex".to_string(), {
            let config_dir = config_dir.clone();
            move |id, request| serve_asset(id, request, &config_dir)
        })
        .build_gtk(&anchor_gtk_window)
        .map_err(|e| anyhow!("failed to create anchor webview: {e}"))?;
    let anchor_wkwebview = anchor_webview.webview();

    let state = Rc::new(RefCell::new(RendererState {
        windows: BTreeMap::new(),
        global_poll_snapshots: BTreeMap::new(),
        global_listen_snapshots: initial_listen,
        base_payload: base,
        config_dir: config_dir.clone(),
        allow_shell,
        _web_context: web_context,
        _anchor_gtk_window: anchor_gtk_window,
        _anchor_webview: anchor_webview,
        anchor_wkwebview,
    }));

    // IDs of windows whose GTK windows were destroyed by the user (not by us).
    let destroyed_ids: Rc<RefCell<BTreeSet<String>>> = Rc::new(RefCell::new(BTreeSet::new()));

    // Open initial windows.
    for &id in initial_window_ids {
        // Clone anchor handle before borrowing state to avoid holding two borrows.
        let anchor = state.borrow().anchor_wkwebview.clone();
        // Evaluate add_window in its own block so the Ref guard from
        // state.borrow() is dropped before the match arms run.  If the borrow
        // lived into the Ok arm (Rust extends scrutinee temporaries to the end
        // of the match block), the subsequent state.borrow_mut() would panic.
        let result = {
            let st = state.borrow();
            add_window(id, &st, Rc::clone(&destroyed_ids), &anchor)
        };
        match result {
            Ok(managed) => {
                state.borrow_mut().windows.insert(id.to_string(), managed);
            }
            Err(e) => eprintln!("widgex renderer: failed to open window {id:?}: {e}"),
        }
    }

    if state.borrow().windows.is_empty() {
        return Err(anyhow!("no windows were opened; check window IDs and config"));
    }

    // Non-blocking control socket.
    // Remove a stale socket file if it exists so bind() succeeds.
    let _ = std::fs::remove_file(control_socket_path);
    let control_listener =
        UnixListener::bind(control_socket_path).context("failed to bind renderer control socket")?;
    control_listener
        .set_nonblocking(true)
        .context("failed to set control socket non-blocking")?;

    // GTK tick: drain channels, poll control socket, push changed payloads.
    let state_tick = Rc::clone(&state);
    let destroyed_tick = Rc::clone(&destroyed_ids);
    gtk::glib::timeout_add_local(Duration::from_millis(16), move || {
        // ── Drain destroyed-window notifications ──────────────────────────
        {
            let killed = std::mem::take(&mut *destroyed_tick.borrow_mut());
            for id in killed {
                let mut st = state_tick.borrow_mut();
                st.windows.remove(&id);
                if st.windows.is_empty() {
                    drop(st);
                    gtk::main_quit();
                    return gtk::glib::ControlFlow::Break;
                }
            }
        }

        // ── Drain poll + listen channels ──────────────────────────────────
        let mut dirty = false;
        {
            let mut st = state_tick.borrow_mut();
            while let Ok(snapshot) = poll_rx.try_recv() {
                st.global_poll_snapshots.insert(snapshot.id.clone(), snapshot);
                dirty = true;
            }
            if drain_listener_snapshots(&listener_rx, &mut st.global_listen_snapshots) {
                dirty = true;
            }
        }

        // ── Poll control socket ───────────────────────────────────────────
        loop {
            match control_listener.accept() {
                Ok((mut stream, _)) => {
                    let response = handle_control_request(
                        &mut stream,
                        &state_tick,
                        &destroyed_tick,
                    );
                    if let Some((resp, stop)) = response {
                        if let Ok(line) = resp.to_json_line() {
                            let _ = stream.write_all(line.as_bytes());
                        }
                        if stop {
                            gtk::main_quit();
                            return gtk::glib::ControlFlow::Break;
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => {
                    eprintln!("widgex renderer control error: {e}");
                    break;
                }
            }
        }

        // ── Push changed payloads to each window ─────────────────────────
        if dirty {
            let mut st = state_tick.borrow_mut();
            let mut snapshots: Vec<SourceSnapshot> =
                st.global_poll_snapshots.values().cloned().collect();
            snapshots.extend(st.global_listen_snapshots.values().cloned());

            // Clone base_payload to release the immutable borrow before the
            // mutable windows iteration.
            let base_payload = st.base_payload.clone();
            let resolved = resolve_payload(&base_payload, &snapshots);
            for (window_id, managed) in st.windows.iter_mut() {
                match managed {
                    ManagedWindow::Webkit { webview, last_pushed_json, .. } => {
                        // Build a single-window payload so evaluate_script is
                        // only called when THIS window's data actually changed.
                        // Pushing the full multi-window payload would fire on
                        // every source tick (e.g. the pet 100 ms brain source),
                        // flooding WebKit with 300+ KB/call and causing runaway
                        // memory growth in WebKitWebProcess.
                        let win_payload = RendererPayload {
                            windows: resolved
                                .windows
                                .iter()
                                .filter(|w| w.id == *window_id)
                                .cloned()
                                .collect(),
                            ..resolved.clone()
                        };
                        if let Ok(json) = serde_json::to_string(&win_payload) {
                            if json != *last_pushed_json {
                                let _ = webview.evaluate_script(&format!(
                                    "window.__widgexPush && window.__widgexPush({json})"
                                ));
                                *last_pushed_json = json;
                            }
                        }
                    }
                    ManagedWindow::Native { renderer } => {
                        if let Some(win_data) = resolved.windows.iter().find(|w| w.id == *window_id) {
                            renderer.update(win_data);
                        }
                    }
                }
            }
        }

        gtk::glib::ControlFlow::Continue
    });

    gtk::main();

    // Kill listener-thread children (same as run_widget_window).
    #[cfg(unix)]
    unsafe {
        let my_pid = std::process::id() as libc::pid_t;
        if libc::getpgid(0) == my_pid {
            let saved = libc::signal(libc::SIGTERM, libc::SIG_IGN);
            libc::killpg(my_pid, libc::SIGTERM);
            libc::signal(libc::SIGTERM, saved);
        }
    }

    Ok(())
}

/// Read one JSON line from `stream`, parse it as a [`RendererRequest`], and
/// return the [`RendererResponse`] plus a `bool` indicating whether the caller
/// should call `gtk::main_quit()`. Returns `None` on I/O or parse errors (the
/// error is printed to stderr).
fn handle_control_request(
    stream: &mut std::os::unix::net::UnixStream,
    state: &Rc<RefCell<RendererState>>,
    destroyed_ids: &Rc<RefCell<BTreeSet<String>>>,
) -> Option<(RendererResponse, bool)> {
    let mut line = String::new();
    if BufReader::new(&*stream).read_line(&mut line).is_err() || line.is_empty() {
        return None;
    }
    let request = match RendererRequest::from_json_line(&line) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("widgex renderer: bad control request: {e}");
            return Some((RendererResponse::error(format!("parse error: {e}")), false));
        }
    };

    match request {
        RendererRequest::Status => {
            let st = state.borrow();
            let open: Vec<String> = st.windows.keys().cloned().collect();
            Some((
                RendererResponse::ok("ok").with_open_windows(open),
                false,
            ))
        }
        RendererRequest::Stop => {
            // Signal the tick loop to call gtk::main_quit via the bool flag.
            Some((RendererResponse::ok("stopping"), true))
        }
        RendererRequest::Open { window_id } => {
            let already_open = state.borrow().windows.contains_key(&window_id);
            if already_open {
                return Some((
                    RendererResponse::ok(format!("window {window_id:?} already open")),
                    false,
                ));
            }
            let anchor = state.borrow().anchor_wkwebview.clone();
            let result = {
                let st = state.borrow();
                add_window(&window_id, &st, Rc::clone(destroyed_ids), &anchor)
            };
            match result {
                Ok(managed) => {
                    state.borrow_mut().windows.insert(window_id.clone(), managed);
                    Some((RendererResponse::ok(format!("opened {window_id:?}")), false))
                }
                Err(e) => Some((
                    RendererResponse::error(format!("failed to open {window_id:?}: {e}")),
                    false,
                )),
            }
        }
        RendererRequest::Close { window_id } => {
            // Borrow safety: we extract the ManagedWindow from state while
            // holding borrow_mut, then DROP the guard before calling
            // gtk_window.destroy(). This ensures that if the connect_destroy
            // callback (which borrows destroyed_ids) somehow also touched
            // state, there would be no active mutable borrow of state at that
            // point. destroyed_ids is a separate RefCell so is not affected.
            let extracted = state.borrow_mut().windows.remove(&window_id);
            // `state` borrow_mut guard is dropped here — before destroy().
            if let Some(managed) = extracted {
                let will_be_empty = state.borrow().windows.is_empty();
                // destroy() fires connect_destroy callbacks synchronously.
                // No borrow_mut on state or destroyed_ids is held at this point.
                // SAFETY: we have exclusive ownership of this window; it is no
                // longer referenced by any other part of the program after
                // removal from state.windows above.
                unsafe { managed.gtk_window().destroy() };
                // The connect_destroy callback may have inserted window_id into
                // destroyed_ids. Remove it so the tick loop skips it.
                destroyed_ids.borrow_mut().remove(&window_id);
                Some((RendererResponse::ok(format!("closed {window_id:?}")), will_be_empty))
            } else {
                Some((
                    RendererResponse::error(format!("window {window_id:?} not found")),
                    false,
                ))
            }
        }
    }
}

// ── End multi-window renderer ────────────────────────────────────────────────

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
            let mut child = Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(config_dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .context("failed to execute command action")?;
            // Reap the child so it never becomes a zombie.  We do not care
            // about the exit status — fire-and-forget — but we must call
            // wait() or the kernel holds the exit record until the parent dies.
            thread::spawn(move || { let _ = child.wait(); });
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
        .filter(|source| source.mode == SourceMode::Listen)
        .cloned()
    {
        let tx = tx.clone();
        let cwd = cwd.clone();
        match source.kind {
            SourceKind::Shell => {
                thread::spawn(move || listen_shell_source(source, cwd, tx));
            }
            SourceKind::UnixSocket => {
                thread::spawn(move || listen_unix_socket_source(source, cwd, tx));
            }
            _ => {}
        }
    }
    rx
}

fn listen_shell_source(source: DataSource, cwd: PathBuf, tx: mpsc::Sender<SourceSnapshot>) {
    let effective_cwd: PathBuf = source.working_dir.clone().unwrap_or(cwd);
    let mut fields_cache = BTreeMap::new();
    loop {
        let Some(command) = source.command.as_deref() else {
            return;
        };
        let mut child = match Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&effective_cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                eprintln!("widgex listen shell failed for {}: {error}", source.id);
                return;
            }
        };

        let Some(stdout) = child.stdout.take() else {
            let _ = child.wait();
            return;
        };

        for line in BufReader::new(stdout)
            .lines()
            .map_while(std::result::Result::ok)
        {
            if send_source_line(&source, &line, &mut fields_cache, &tx).is_err() {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
        }

        let _ = child.wait();
        thread::sleep(reconnect_interval(&source));
    }
}

fn listen_unix_socket_source(source: DataSource, cwd: PathBuf, tx: mpsc::Sender<SourceSnapshot>) {
    let mut fields_cache = BTreeMap::new();
    loop {
        let Some(path) = source.path.as_deref() else {
            return;
        };
        let path = source_path(path, &cwd);
        match UnixStream::connect(&path) {
            Ok(stream) => {
                let reader = BufReader::new(stream);
                for line in reader.lines().map_while(std::result::Result::ok) {
                    if send_source_line(&source, &line, &mut fields_cache, &tx).is_err() {
                        return;
                    }
                }
            }
            Err(error) => {
                eprintln!(
                    "widgex unix_socket listen failed for {} at {}: {error}",
                    source.id,
                    path.display()
                );
            }
        }
        thread::sleep(reconnect_interval(&source));
    }
}

fn send_source_line(
    source: &DataSource,
    line: &str,
    fields_cache: &mut BTreeMap<String, String>,
    tx: &mpsc::Sender<SourceSnapshot>,
) -> std::result::Result<(), mpsc::SendError<SourceSnapshot>> {
    let fields = widgex_source::parse_source_output(source.format, line);
    let mut snapshot = SourceSnapshot::new(&source.id);
    if source.format == widgex_core::SourceFormat::HyprlandEvent {
        fields_cache.extend(fields);
        snapshot.fields = fields_cache.clone();
    } else {
        snapshot.fields = fields;
    }
    tx.send(snapshot)
}

fn reconnect_interval(source: &DataSource) -> Duration {
    Duration::from_millis(source.interval_ms.unwrap_or(1000).max(100))
}

fn source_path(path: &str, cwd: &Path) -> PathBuf {
    let expanded = expand_env_vars(path);
    let path = PathBuf::from(expanded);
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

fn expand_env_vars(input: &str) -> String {
    let mut output = String::new();
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '$' {
            output.push(ch);
            continue;
        }

        if matches!(chars.peek(), Some('{')) {
            chars.next();
            let mut name = String::new();
            for next in chars.by_ref() {
                if next == '}' {
                    break;
                }
                name.push(next);
            }
            output.push_str(&std::env::var(name).unwrap_or_default());
            continue;
        }

        let mut name = String::new();
        while let Some(next) = chars.peek().copied() {
            if next == '_' || next.is_ascii_alphanumeric() {
                chars.next();
                name.push(next);
            } else {
                break;
            }
        }

        if name.is_empty() {
            output.push('$');
        } else {
            output.push_str(&std::env::var(name).unwrap_or_default());
        }
    }
    output
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
) -> bool {
    let mut received = false;
    while let Ok(snapshot) = rx.try_recv() {
        latest.insert(snapshot.id.clone(), snapshot);
        received = true;
    }
    received
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
                SourceKind::Cpu
                | SourceKind::Memory
                | SourceKind::Network
                | SourceKind::Shell
                | SourceKind::UnixSocket => snapshot,
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
    use super::{is_safe_relative_path, start_listeners};
    use std::{io::Write, os::unix::net::UnixListener, thread, time::Duration};
    use widgex_core::{DataSource, SourceFormat, SourceKind, SourceMode};

    #[test]
    fn config_asset_paths_reject_traversal_and_absolute_paths() {
        assert!(is_safe_relative_path("spotify_cache/album_art.png"));
        assert!(is_safe_relative_path("./spotify_cache/default.png"));
        assert!(!is_safe_relative_path("../secret"));
        assert!(!is_safe_relative_path("/etc/passwd"));
    }

    #[test]
    fn unix_socket_listener_reads_hyprland_event_lines() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("hypr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            writeln!(stream, "workspacev2>>3,chat").unwrap();
        });
        let sources = vec![DataSource {
            id: "hypr_events".into(),
            kind: SourceKind::UnixSocket,
            mode: SourceMode::Listen,
            format: SourceFormat::HyprlandEvent,
            interval_ms: Some(100),
            timeout_ms: None,
            command: None,
            path: Some(socket_path.to_string_lossy().into_owned()),
            working_dir: None,
        }];

        let rx = start_listeners(&sources, dir.path().to_path_buf());
        let snapshot = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        server.join().unwrap();

        assert_eq!(snapshot.id, "hypr_events");
        assert_eq!(
            snapshot.fields.get("event").map(String::as_str),
            Some("workspacev2")
        );
        assert_eq!(
            snapshot.fields.get("workspace_id").map(String::as_str),
            Some("3")
        );
        assert_eq!(
            snapshot.fields.get("workspace_name").map(String::as_str),
            Some("chat")
        );
    }

    #[test]
    fn unix_socket_listener_reconnects_after_disconnect() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("events.sock");
        let sources = vec![DataSource {
            id: "hypr_events".into(),
            kind: SourceKind::UnixSocket,
            mode: SourceMode::Listen,
            format: SourceFormat::HyprlandEvent,
            interval_ms: Some(100),
            timeout_ms: None,
            command: None,
            path: Some(socket_path.to_string_lossy().into_owned()),
            working_dir: None,
        }];

        let first_listener = UnixListener::bind(&socket_path).unwrap();
        let first_server = thread::spawn(move || {
            let (mut stream, _) = first_listener.accept().unwrap();
            writeln!(stream, "workspacev2>>1,main").unwrap();
        });
        let rx = start_listeners(&sources, dir.path().to_path_buf());
        let first = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        first_server.join().unwrap();
        std::fs::remove_file(&socket_path).unwrap();

        let second_listener = UnixListener::bind(&socket_path).unwrap();
        let second_server = thread::spawn(move || {
            let (mut stream, _) = second_listener.accept().unwrap();
            writeln!(stream, "workspacev2>>2,web").unwrap();
        });
        let second = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        second_server.join().unwrap();

        assert_eq!(
            first.fields.get("workspace_name").map(String::as_str),
            Some("main")
        );
        assert_eq!(
            second.fields.get("workspace_name").map(String::as_str),
            Some("web")
        );
    }

    #[test]
    fn hyprland_event_listener_keeps_workspace_state_across_other_events() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("hypr.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            writeln!(stream, "workspacev2>>4,music").unwrap();
            writeln!(stream, "activewindowv2>>0xabc").unwrap();
        });
        let sources = vec![DataSource {
            id: "hypr_events".into(),
            kind: SourceKind::UnixSocket,
            mode: SourceMode::Listen,
            format: SourceFormat::HyprlandEvent,
            interval_ms: Some(100),
            timeout_ms: None,
            command: None,
            path: Some(socket_path.to_string_lossy().into_owned()),
            working_dir: None,
        }];

        let rx = start_listeners(&sources, dir.path().to_path_buf());
        let first = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let second = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        server.join().unwrap();

        assert_eq!(
            first.fields.get("workspace4_style").map(String::as_str),
            second.fields.get("workspace4_style").map(String::as_str)
        );
        assert!(
            second
                .fields
                .get("workspace4_style")
                .is_some_and(|style| style.contains("background: var(--ctp-green)"))
        );
        assert_eq!(
            second.fields.get("window_address").map(String::as_str),
            Some("0xabc")
        );
    }
}
