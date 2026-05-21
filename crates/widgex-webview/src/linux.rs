//! Linux implementation of the widget renderer.
//!
//! A [`RendererPayload`] is handed to a SolidJS bundle running inside a
//! `webkit2gtk` webview. The bundle is embedded by `rust-embed` and served
//! over a `widgex://` custom protocol. The GTK window is anchored as a desktop
//! layer via `gtk-layer-shell` on Wayland, or positioned via EWMH hints on X11.
//!
//! Data sources are polled in-process: every tick the payload is re-resolved
//! against fresh [`SourceSnapshot`]s and pushed into the page via
//! `window.__widgexPush`.

use std::{
    borrow::Cow,
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashSet},
    io::{self, BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    os::unix::process::CommandExt,
    path::Component,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
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
    RendererWindow, SourceKind, SourceMode, SourceSnapshot, WindowLayer, load_validated_config,
    renderer_payload_from_config, resolve_payload,
};

use super::native_renderer::NativeRenderer;
use super::x11_window;
use webkit2gtk::WebViewExt;
use widgex_ipc::{RendererRequest, RendererResponse};
use wry::{
    WebContext, WebViewBuilder, WebViewBuilderExtUnix, WebViewExtUnix,
    http::{Request, Response, header::CONTENT_TYPE},
};

/// The built SolidJS renderer bundle. In debug builds `rust-embed` reads these
/// files from disk at runtime; release builds embed them into the binary.
#[derive(RustEmbed)]
#[folder = "../../apps/renderer/dist"]
struct RendererAsset;

const DEFAULT_WIDTH: u32 = 320;
const DEFAULT_HEIGHT: u32 = 120;
const RENDERER_TICK_MS: u64 = 100;

// ── Display backend detection ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisplayBackend {
    Wayland,
    X11,
}

static BACKEND: std::sync::OnceLock<DisplayBackend> = std::sync::OnceLock::new();

pub(crate) fn display_backend() -> DisplayBackend {
    *BACKEND.get_or_init(|| {
        if gtk_layer_shell::is_supported() {
            DisplayBackend::Wayland
        } else {
            DisplayBackend::X11
        }
    })
}

/// Apply layer-shell (Wayland) or EWMH hints (X11) before the window is shown.
pub(crate) fn apply_desktop_hints(window: &gtk::Window, spec: &RendererWindow) {
    match display_backend() {
        DisplayBackend::Wayland => {
            window.init_layer_shell();
            window.set_namespace("widgex");
            window.set_layer(map_layer(spec.layer));
            window.set_keyboard_mode(KeyboardMode::None);
            for edge in [
                AnchorEdge::Top,
                AnchorEdge::Right,
                AnchorEdge::Bottom,
                AnchorEdge::Left,
            ] {
                window.set_anchor(map_edge(edge), spec.anchor.contains(&edge));
            }
            window.set_layer_shell_margin(Edge::Top, spec.margin.top as i32);
            window.set_layer_shell_margin(Edge::Right, spec.margin.right as i32);
            window.set_layer_shell_margin(Edge::Bottom, spec.margin.bottom as i32);
            window.set_layer_shell_margin(Edge::Left, spec.margin.left as i32);
            window.set_exclusive_zone(spec.exclusive_zone.unwrap_or(0));
        }
        DisplayBackend::X11 => {
            x11_window::apply_hints_before_show(window, spec.layer);
        }
    }
}

/// After `show_all()`: on X11 move the window to the anchor-computed position.
pub(crate) fn apply_desktop_position(window: &gtk::Window, spec: &RendererWindow) {
    if display_backend() == DisplayBackend::X11 {
        x11_window::apply_position_after_show(
            window,
            &spec.anchor,
            spec.margin,
            spec.size,
            spec.monitor.as_deref(),
        );
    }
}

// ── Build window preview ────────────────────────────────────────────────────

/// Build a non-GUI preview of the selected window. Binding templates are
/// resolved against deterministic example snapshots so the output is stable.
pub fn build_window_preview(
    payload: &RendererPayload,
    window_id: Option<&str>,
) -> Result<crate::WindowPreview> {
    let resolved = resolve_payload(payload, &example_snapshots(&payload.sources));
    let window = select_window(&resolved, window_id)?;

    Ok(crate::WindowPreview {
        id: window.id.clone(),
        title: window.title.clone(),
        width: window.size.width.unwrap_or(DEFAULT_WIDTH),
        height: window.size.height.unwrap_or(DEFAULT_HEIGHT),
        text_preview: first_text_preview(window).unwrap_or_default(),
    })
}

// ── Single-window renderer (--foreground / standalone) ──────────────────────

/// Open the selected window as a desktop-anchored webview and run the GTK
/// event loop until the window is closed.
pub fn run_widget_window(
    payload: &RendererPayload,
    config_dir: impl AsRef<Path>,
    window_id: Option<&str>,
    sources: &[DataSource],
    allow_shell: bool,
) -> Result<()> {
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        // SAFETY: single-threaded before GTK init.
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }

    gtk::init().context("failed to initialize GTK")?;

    let config_dir = config_dir.as_ref().to_path_buf();
    let mut base = payload.clone();
    inline_theme_css(&mut base, &config_dir);

    let window_spec = select_window(&base, window_id)?.clone();
    base.windows = vec![window_spec.clone()];

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

    apply_desktop_hints(&window, &window_spec);

    let mut active_ids = HashSet::new();
    collect_source_ids_from_widgets(&window_spec.widgets, &mut active_ids);
    let active_sources: Vec<DataSource> = sources
        .iter()
        .filter(|s| active_ids.contains(&s.id))
        .cloned()
        .collect();

    let (listener_tx, listener_rx) = mpsc::channel::<SourceSnapshot>();
    start_listeners_into(&active_sources, config_dir.clone(), listener_tx);
    let mut latest_listen_snapshots: BTreeMap<String, SourceSnapshot> =
        widgex_source::seed_listen_snapshots(&active_sources)
            .into_iter()
            .map(|s| (s.id.clone(), s))
            .collect();
    let initial = resolve_payload(&base, &[]);
    let init_script = format!(
        "window.__WIDGEX_PAYLOAD__ = {};",
        serde_json::to_string(&initial)?
    );
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
            let action_dir = window_spec
                .working_dir
                .clone()
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
    apply_desktop_position(&window, &window_spec);
    if window_spec.click_through {
        let empty_input = gtk::cairo::Region::create();
        window.input_shape_combine_region(Some(&empty_input));
    }
    window.connect_destroy(|_| gtk::main_quit());

    let (poll_tx, poll_rx) = mpsc::channel::<SourceSnapshot>();
    start_pollers_into(&active_sources, config_dir.clone(), poll_tx);
    let mut latest_poll_snapshots = BTreeMap::<String, SourceSnapshot>::new();
    let push_webview = Rc::clone(&webview);
    let mut last_pushed_json = String::new();
    let mut last_push = std::time::Instant::now();
    gtk::glib::timeout_add_local(Duration::from_millis(RENDERER_TICK_MS), move || {
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
        if last_push.elapsed() < Duration::from_millis(200) {
            return gtk::glib::ControlFlow::Continue;
        }
        let mut snapshots: Vec<SourceSnapshot> = latest_poll_snapshots.values().cloned().collect();
        snapshots.extend(latest_listen_snapshots.values().cloned());
        let resolved = resolve_payload(&base, &snapshots);
        if let Ok(json) = serde_json::to_string(&resolved)
            && json != last_pushed_json
        {
            let _ = push_webview.evaluate_script(&format!(
                "window.__widgexPush && window.__widgexPush({json})"
            ));
            last_pushed_json = json;
            last_push = std::time::Instant::now();
        }
        gtk::glib::ControlFlow::Continue
    });

    gtk::main();
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

struct RendererState {
    windows: BTreeMap<String, ManagedWindow>,
    global_poll_snapshots: BTreeMap<String, SourceSnapshot>,
    global_listen_snapshots: BTreeMap<String, SourceSnapshot>,
    base_payload: RendererPayload,
    config_dir: PathBuf,
    config_path: PathBuf,
    allow_shell: bool,
    web_context: WebContext,
    all_sources: Vec<DataSource>,
    source_workers: SourceWorkers,
}

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

fn add_window(
    window_id: &str,
    state: &mut RendererState,
    destroyed_ids: Rc<RefCell<BTreeSet<String>>>,
) -> Result<ManagedWindow> {
    let window_spec = select_window(&state.base_payload, Some(window_id))?.clone();

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

    apply_desktop_hints(&window, &window_spec);

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

    // Use a shared WebContext so all windows share one WebKitNetworkProcess.
    // Only register the widgex:// protocol once; subsequent windows reuse it.
    let needs_protocol = !state.web_context.is_custom_protocol_registered("widgex");
    let mut builder = WebViewBuilder::new_with_web_context(&mut state.web_context)
        .with_url("widgex://localhost/index.html")
        .with_transparent(true)
        .with_initialization_script(init_script)
        .with_ipc_handler({
            move |request: Request<String>| {
                if let Err(e) = handle_widget_event(request.body(), &action_dir, allow_shell) {
                    eprintln!("widgex ipc error: {e}");
                }
            }
        });
    if needs_protocol {
        builder = builder.with_custom_protocol("widgex".to_string(), {
            move |id, request| serve_asset(id, request, &config_dir)
        });
    }
    let webview = builder
        .build_gtk(&window)
        .map_err(|e| anyhow!("failed to create webview: {e}"))?;
    let webview = Rc::new(webview);

    window.show_all();
    apply_desktop_position(&window, &window_spec);
    if window_spec.click_through {
        let empty_input = gtk::cairo::Region::create();
        window.input_shape_combine_region(Some(&empty_input));
    }

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

pub fn run_renderer(
    payload: &RendererPayload,
    config_dir: impl AsRef<Path>,
    config_path: impl AsRef<Path>,
    sources: &[DataSource],
    allow_shell: bool,
    control_socket_path: &Path,
    initial_window_ids: &[&str],
) -> Result<()> {
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        // SAFETY: single-threaded before GTK init.
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }

    gtk::init().context("failed to initialize GTK")?;

    let config_dir = config_dir.as_ref().to_path_buf();
    let config_path = config_path.as_ref().to_path_buf();
    let mut base = payload.clone();
    inline_theme_css(&mut base, &config_dir);

    let active_ids = source_ids_for_windows(&base, initial_window_ids);
    let active_sources: Vec<DataSource> = sources
        .iter()
        .filter(|s| active_ids.contains(&s.id))
        .cloned()
        .collect();

    let (poll_tx, poll_rx) = mpsc::channel::<SourceSnapshot>();
    let (listener_tx, listener_rx) = mpsc::channel::<SourceSnapshot>();
    let mut source_workers =
        SourceWorkers::new(config_dir.clone(), poll_tx.clone(), listener_tx.clone());
    source_workers.reconcile(sources, active_ids.clone());

    let initial_listen: BTreeMap<String, SourceSnapshot> =
        widgex_source::seed_listen_snapshots(&active_sources)
            .into_iter()
            .map(|s| (s.id.clone(), s))
            .collect();

    let state = Rc::new(RefCell::new(RendererState {
        windows: BTreeMap::new(),
        global_poll_snapshots: BTreeMap::new(),
        global_listen_snapshots: initial_listen,
        base_payload: base,
        config_dir: config_dir.clone(),
        config_path,
        allow_shell,
        web_context: WebContext::default(),
        all_sources: sources.to_vec(),
        source_workers,
    }));

    let destroyed_ids: Rc<RefCell<BTreeSet<String>>> = Rc::new(RefCell::new(BTreeSet::new()));

    for &id in initial_window_ids {
        let result = {
            let mut st = state.borrow_mut();
            add_window(id, &mut st, Rc::clone(&destroyed_ids))
        };
        match result {
            Ok(managed) => {
                state.borrow_mut().windows.insert(id.to_string(), managed);
            }
            Err(e) => eprintln!("widgex renderer: failed to open window {id:?}: {e}"),
        }
    }

    if state.borrow().windows.is_empty() {
        return Err(anyhow!(
            "no windows were opened; check window IDs and config"
        ));
    }

    let _ = std::fs::remove_file(control_socket_path);
    let control_listener = UnixListener::bind(control_socket_path)
        .context("failed to bind renderer control socket")?;
    control_listener
        .set_nonblocking(true)
        .context("failed to set control socket non-blocking")?;

    let state_tick = Rc::clone(&state);
    let destroyed_tick = Rc::clone(&destroyed_ids);
    let mut last_push = std::time::Instant::now();
    gtk::glib::timeout_add_local(Duration::from_millis(RENDERER_TICK_MS), move || {
        // ── Drain destroyed-window notifications ──────────────────────────
        {
            let killed = std::mem::take(&mut *destroyed_tick.borrow_mut());
            for id in killed {
                let (managed, is_empty) = {
                    let mut st = state_tick.borrow_mut();
                    let managed = st.windows.remove(&id);
                    let is_empty = st.windows.is_empty();
                    (managed, is_empty)
                };
                // GTK-triggered close doesn't go through RendererRequest::Close,
                // so we must terminate the web process explicitly here.
                #[cfg(target_os = "linux")]
                if let Some(ManagedWindow::Webkit { ref webview, .. }) = managed {
                    webview.webview().terminate_web_process();
                }
                drop(managed);
                {
                    let mut st = state_tick.borrow_mut();
                    reconcile_sources_for_open_windows(&mut st);
                }
                if is_empty {
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
                st.global_poll_snapshots
                    .insert(snapshot.id.clone(), snapshot);
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
                    let response =
                        handle_control_request(&mut stream, &state_tick, &destroyed_tick);
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
        if dirty && last_push.elapsed() >= Duration::from_millis(200) {
            let mut st = state_tick.borrow_mut();
            let mut snapshots: Vec<SourceSnapshot> =
                st.global_poll_snapshots.values().cloned().collect();
            snapshots.extend(st.global_listen_snapshots.values().cloned());

            let base_payload = st.base_payload.clone();
            let resolved = resolve_payload(&base_payload, &snapshots);
            let mut pushed = false;
            for (window_id, managed) in st.windows.iter_mut() {
                match managed {
                    ManagedWindow::Webkit {
                        webview,
                        last_pushed_json,
                        ..
                    } => {
                        let win_payload = RendererPayload {
                            windows: resolved
                                .windows
                                .iter()
                                .filter(|w| w.id == *window_id)
                                .cloned()
                                .collect(),
                            ..resolved.clone()
                        };
                        if let Ok(json) = serde_json::to_string(&win_payload)
                            && json != *last_pushed_json
                        {
                            let _ = webview.evaluate_script(&format!(
                                "window.__widgexPush && window.__widgexPush({json})"
                            ));
                            *last_pushed_json = json;
                            pushed = true;
                        }
                    }
                    ManagedWindow::Native { renderer } => {
                        if let Some(win_data) = resolved.windows.iter().find(|w| w.id == *window_id)
                        {
                            renderer.update(win_data);
                        }
                    }
                }
            }
            if pushed {
                last_push = std::time::Instant::now();
            }
        }

        gtk::glib::ControlFlow::Continue
    });

    gtk::main();

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
            Some((RendererResponse::ok("ok").with_open_windows(open), false))
        }
        RendererRequest::Stop => Some((RendererResponse::ok("stopping"), true)),
        RendererRequest::Open { window_id } => {
            let already_open = state.borrow().windows.contains_key(&window_id);
            if already_open {
                return Some((
                    RendererResponse::ok(format!("window {window_id:?} already open")),
                    false,
                ));
            }

            {
                let config_path = state.borrow().config_path.clone();
                if let Some(fresh_payload) = load_validated_config(&config_path)
                    .ok()
                    .and_then(|c| renderer_payload_from_config(&c).ok())
                {
                    let mut st = state.borrow_mut();
                    if let Some(new_spec) = fresh_payload
                        .windows
                        .into_iter()
                        .find(|w| w.id == window_id)
                    {
                        if let Some(pos) = st
                            .base_payload
                            .windows
                            .iter()
                            .position(|w| w.id == window_id)
                        {
                            st.base_payload.windows[pos] = new_spec;
                        }
                    }
                }
            }

            let result = {
                let mut st = state.borrow_mut();
                add_window(&window_id, &mut st, Rc::clone(destroyed_ids))
            };
            match result {
                Ok(managed) => {
                    state
                        .borrow_mut()
                        .windows
                        .insert(window_id.clone(), managed);

                    // Start any sources needed by this window that aren't running yet.
                    let missing = {
                        let mut st = state.borrow_mut();
                        let needed = source_ids_for_open_windows(&st);
                        let missing: Vec<DataSource> = st
                            .all_sources
                            .iter()
                            .filter(|s| {
                                needed.contains(&s.id) && !st.source_workers.is_running(&s.id)
                            })
                            .cloned()
                            .collect();
                        reconcile_sources_for_open_windows(&mut st);
                        missing
                    };
                    if !missing.is_empty() {
                        for snap in widgex_source::seed_listen_snapshots(&missing) {
                            state
                                .borrow_mut()
                                .global_listen_snapshots
                                .entry(snap.id.clone())
                                .or_insert(snap);
                        }
                    }

                    Some((RendererResponse::ok(format!("opened {window_id:?}")), false))
                }
                Err(e) => Some((
                    RendererResponse::error(format!("failed to open {window_id:?}: {e}")),
                    false,
                )),
            }
        }
        RendererRequest::Reload => {
            let mut st = state.borrow_mut();
            let count = st.windows.len();
            for managed in st.windows.values_mut() {
                if let ManagedWindow::Webkit {
                    webview,
                    last_pushed_json,
                    ..
                } = managed
                {
                    webview.webview().reload();
                    last_pushed_json.clear();
                }
            }
            Some((
                RendererResponse::ok(format!("reloaded {count} windows")),
                false,
            ))
        }
        RendererRequest::Close { window_id } => {
            let extracted = state.borrow_mut().windows.remove(&window_id);
            if let Some(managed) = extracted {
                let will_be_empty = state.borrow().windows.is_empty();

                #[cfg(target_os = "linux")]
                if let ManagedWindow::Webkit { ref webview, .. } = managed {
                    webview.webview().terminate_web_process();
                }

                unsafe { managed.gtk_window().destroy() };
                drop(managed);

                destroyed_ids.borrow_mut().remove(&window_id);
                reconcile_sources_for_open_windows(&mut state.borrow_mut());
                Some((
                    RendererResponse::ok(format!("closed {window_id:?}")),
                    will_be_empty,
                ))
            } else {
                Some((
                    RendererResponse::error(format!("window {window_id:?} not found")),
                    false,
                ))
            }
        }
    }
}

// ── Widget events ────────────────────────────────────────────────────────────

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

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
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
                .map(|v| {
                    let quoted = shell_quote(v);
                    command
                        .replace("{}", &quoted)
                        .replace("{{ value }}", &quoted)
                })
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
            thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Action::Emit { event } => {
            eprintln!("widgex event emitted: {event}");
        }
    }
    Ok(())
}

// ── Source dependency helpers ─────────────────────────────────────────────────

fn source_ids_for_windows(payload: &RendererPayload, window_ids: &[&str]) -> HashSet<String> {
    let id_set: HashSet<&str> = window_ids.iter().copied().collect();
    let mut out = HashSet::new();
    for window in &payload.windows {
        if id_set.contains(window.id.as_str()) {
            collect_source_ids_from_widgets(&window.widgets, &mut out);
        }
    }
    out
}

fn collect_source_ids_from_widgets(widgets: &[RendererWidget], out: &mut HashSet<String>) {
    for widget in widgets {
        if let Some(b) = &widget.bindings {
            for refs in [
                &b.text,
                &b.value,
                &b.src,
                &b.frame_row,
                &b.frame_count,
                &b.draw_x,
                &b.draw_y,
                &b.style,
            ] {
                for r in refs.iter() {
                    if let Some(id) = r.split('.').next() {
                        out.insert(id.to_string());
                    }
                }
            }
        }
        collect_source_ids_from_widgets(&widget.children, out);
    }
}

fn source_ids_for_open_windows(state: &RendererState) -> HashSet<String> {
    let window_ids: Vec<&str> = state.windows.keys().map(String::as_str).collect();
    source_ids_for_windows(&state.base_payload, &window_ids)
}

fn reconcile_sources_for_open_windows(state: &mut RendererState) {
    let needed = source_ids_for_open_windows(state);
    state
        .source_workers
        .reconcile(&state.all_sources, needed.clone());
    state
        .global_poll_snapshots
        .retain(|source_id, _| needed.contains(source_id));
    state
        .global_listen_snapshots
        .retain(|source_id, _| needed.contains(source_id));
}

// ── Source listeners ─────────────────────────────────────────────────────────

struct SourceWorker {
    stop: Arc<AtomicBool>,
}

impl SourceWorker {
    fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

struct SourceWorkers {
    cwd: PathBuf,
    poll_tx: Sender<SourceSnapshot>,
    listener_tx: Sender<SourceSnapshot>,
    workers: BTreeMap<String, SourceWorker>,
}

impl SourceWorkers {
    fn new(
        cwd: PathBuf,
        poll_tx: Sender<SourceSnapshot>,
        listener_tx: Sender<SourceSnapshot>,
    ) -> Self {
        Self {
            cwd,
            poll_tx,
            listener_tx,
            workers: BTreeMap::new(),
        }
    }

    fn reconcile(&mut self, sources: &[DataSource], needed: HashSet<String>) {
        let stale: Vec<String> = self
            .workers
            .keys()
            .filter(|id| !needed.contains(*id))
            .cloned()
            .collect();
        for id in stale {
            if let Some(worker) = self.workers.remove(&id) {
                worker.stop();
            }
        }

        for source in sources.iter().filter(|source| needed.contains(&source.id)) {
            if self.workers.contains_key(&source.id) {
                continue;
            }
            let stop = Arc::new(AtomicBool::new(false));
            spawn_source_worker(
                source.clone(),
                self.cwd.clone(),
                self.poll_tx.clone(),
                self.listener_tx.clone(),
                Arc::clone(&stop),
            );
            self.workers
                .insert(source.id.clone(), SourceWorker { stop });
        }
    }

    fn is_running(&self, source_id: &str) -> bool {
        self.workers.contains_key(source_id)
    }
}

impl Drop for SourceWorkers {
    fn drop(&mut self) {
        for (_, worker) in std::mem::take(&mut self.workers) {
            worker.stop();
        }
    }
}

fn start_listeners_into(sources: &[DataSource], cwd: PathBuf, tx: Sender<SourceSnapshot>) {
    for source in sources
        .iter()
        .filter(|s| s.mode == SourceMode::Listen)
        .cloned()
    {
        match source.kind {
            SourceKind::Shell => {
                spawn_listen_shell_source(
                    source,
                    cwd.clone(),
                    tx.clone(),
                    Arc::new(AtomicBool::new(false)),
                );
            }
            SourceKind::UnixSocket => {
                spawn_listen_unix_socket_source(
                    source,
                    cwd.clone(),
                    tx.clone(),
                    Arc::new(AtomicBool::new(false)),
                );
            }
            _ => {}
        }
    }
}

fn spawn_source_worker(
    source: DataSource,
    cwd: PathBuf,
    poll_tx: Sender<SourceSnapshot>,
    listener_tx: Sender<SourceSnapshot>,
    stop: Arc<AtomicBool>,
) {
    match (source.mode, source.kind) {
        (SourceMode::Poll, _) => spawn_poll_source(source, cwd, poll_tx, stop),
        (SourceMode::Listen, SourceKind::Shell) => {
            spawn_listen_shell_source(source, cwd, listener_tx, stop);
        }
        (SourceMode::Listen, SourceKind::UnixSocket) => {
            spawn_listen_unix_socket_source(source, cwd, listener_tx, stop);
        }
        _ => {}
    }
}

fn spawn_poll_source(
    source: DataSource,
    cwd: PathBuf,
    tx: mpsc::Sender<SourceSnapshot>,
    stop: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let interval = Duration::from_millis(source.interval_ms.unwrap_or(1000).max(100));
        while !stop.load(Ordering::Relaxed) {
            let snapshot = widgex_source::poll_source_with_dir(&source, &cwd);
            if tx.send(snapshot).is_err() {
                break;
            }
            if sleep_until_stopped(&stop, interval) {
                break;
            }
        }
    });
}

fn spawn_listen_shell_source(
    source: DataSource,
    cwd: PathBuf,
    tx: mpsc::Sender<SourceSnapshot>,
    stop: Arc<AtomicBool>,
) {
    thread::spawn(move || listen_shell_source(source, cwd, tx, stop));
}

fn listen_shell_source(
    source: DataSource,
    cwd: PathBuf,
    tx: mpsc::Sender<SourceSnapshot>,
    stop: Arc<AtomicBool>,
) {
    let effective_cwd: PathBuf = source.working_dir.clone().unwrap_or(cwd);
    let mut fields_cache = BTreeMap::new();
    while !stop.load(Ordering::Relaxed) {
        let Some(command) = source.command.as_deref() else {
            return;
        };
        let mut child = match Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&effective_cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                eprintln!("widgex listen shell failed for {}: {error}", source.id);
                return;
            }
        };
        let pid = child.id() as libc::pid_t;
        let child_done = Arc::new(AtomicBool::new(false));
        let stop_child = Arc::clone(&stop);
        let child_done_for_killer = Arc::clone(&child_done);
        thread::spawn(move || {
            while !stop_child.load(Ordering::Relaxed)
                && !child_done_for_killer.load(Ordering::Relaxed)
            {
                thread::sleep(Duration::from_millis(50));
            }
            if stop_child.load(Ordering::Relaxed) && !child_done_for_killer.load(Ordering::Relaxed)
            {
                unsafe {
                    libc::killpg(pid, libc::SIGTERM);
                    libc::kill(pid, libc::SIGTERM);
                }
            }
        });

        let Some(stdout) = child.stdout.take() else {
            let _ = child.wait();
            child_done.store(true, Ordering::Relaxed);
            return;
        };

        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        while !stop.load(Ordering::Relaxed) {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {}
                Err(_) => break,
            }
            let line = line.trim_end_matches(&['\r', '\n'][..]);
            if send_source_line(&source, &line, &mut fields_cache, &tx).is_err() {
                let _ = child.kill();
                let _ = child.wait();
                child_done.store(true, Ordering::Relaxed);
                return;
            }
        }

        let _ = child.wait();
        child_done.store(true, Ordering::Relaxed);
        if sleep_until_stopped(&stop, reconnect_interval(&source)) {
            break;
        }
    }
}

fn spawn_listen_unix_socket_source(
    source: DataSource,
    cwd: PathBuf,
    tx: mpsc::Sender<SourceSnapshot>,
    stop: Arc<AtomicBool>,
) {
    thread::spawn(move || listen_unix_socket_source(source, cwd, tx, stop));
}

fn listen_unix_socket_source(
    source: DataSource,
    cwd: PathBuf,
    tx: mpsc::Sender<SourceSnapshot>,
    stop: Arc<AtomicBool>,
) {
    let mut fields_cache = BTreeMap::new();
    while !stop.load(Ordering::Relaxed) {
        let Some(path) = source.path.as_deref() else {
            return;
        };
        let path = source_path(path, &cwd);
        match UnixStream::connect(&path) {
            Ok(stream) => {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                while !stop.load(Ordering::Relaxed) {
                    line.clear();
                    match reader.read_line(&mut line) {
                        Ok(0) => break,
                        Ok(_) => {}
                        Err(error)
                            if matches!(
                                error.kind(),
                                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                            ) =>
                        {
                            continue;
                        }
                        Err(_) => break,
                    }
                    let line = line.trim_end_matches(&['\r', '\n'][..]);
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
        if sleep_until_stopped(&stop, reconnect_interval(&source)) {
            break;
        }
    }
}

fn sleep_until_stopped(stop: &AtomicBool, duration: Duration) -> bool {
    let step = Duration::from_millis(50);
    let mut slept = Duration::ZERO;
    while slept < duration {
        if stop.load(Ordering::Relaxed) {
            return true;
        }
        let remaining = duration.saturating_sub(slept);
        let nap = remaining.min(step);
        thread::sleep(nap);
        slept += nap;
    }
    stop.load(Ordering::Relaxed)
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

// ── Source pollers ────────────────────────────────────────────────────────────

fn start_pollers_into(sources: &[DataSource], cwd: PathBuf, tx: Sender<SourceSnapshot>) {
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

// ── Asset serving ─────────────────────────────────────────────────────────────

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
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
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

// ── CSS inlining ──────────────────────────────────────────────────────────────

pub fn inline_theme_css(payload: &mut RendererPayload, config_dir: &Path) {
    let css_refs = if payload.theme_css_files.is_empty() {
        payload.theme_css.take().into_iter().collect::<Vec<_>>()
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

// ── Helpers ───────────────────────────────────────────────────────────────────

fn select_window<'a>(
    payload: &'a RendererPayload,
    window_id: Option<&str>,
) -> Result<&'a RendererWindow> {
    match window_id {
        Some(id) => payload
            .windows
            .iter()
            .find(|w| w.id == id)
            .ok_or_else(|| anyhow!("window {id:?} not found in config")),
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
                _ => snapshot,
            }
        })
        .collect()
}

pub(crate) fn map_layer(layer: WindowLayer) -> Layer {
    match layer {
        WindowLayer::Background => Layer::Background,
        WindowLayer::Bottom => Layer::Bottom,
        WindowLayer::Top => Layer::Top,
        WindowLayer::Overlay => Layer::Overlay,
    }
}

pub(crate) fn map_edge(edge: AnchorEdge) -> Edge {
    match edge {
        AnchorEdge::Top => Edge::Top,
        AnchorEdge::Bottom => Edge::Bottom,
        AnchorEdge::Left => Edge::Left,
        AnchorEdge::Right => Edge::Right,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{
        SourceWorkers, is_safe_relative_path, source_ids_for_windows, start_listeners_into,
    };
    use std::{io::Write, os::unix::net::UnixListener, thread, time::Duration};
    use widgex_core::{
        DataSource, RendererPayload, RendererWidget, RendererWidgetBindings, RendererWindow,
        SourceFormat, SourceKind, SourceMode, WidgetKind,
    };

    fn test_source(id: &str, mode: SourceMode) -> DataSource {
        DataSource {
            id: id.into(),
            kind: SourceKind::Shell,
            mode,
            format: SourceFormat::Text,
            interval_ms: Some(100),
            timeout_ms: None,
            command: Some("printf ready".into()),
            path: None,
            working_dir: None,
        }
    }

    fn label_window(id: &str, binding: &str) -> RendererWindow {
        RendererWindow {
            id: id.into(),
            title: None,
            layer: widgex_core::WindowLayer::Top,
            anchor: Vec::new(),
            margin: Default::default(),
            size: Default::default(),
            exclusive_zone: None,
            click_through: false,
            monitor: None,
            native_render: false,
            working_dir: None,
            widgets: vec![RendererWidget {
                kind: WidgetKind::Label,
                id: None,
                class: Vec::new(),
                text: Some(binding.into()),
                value: None,
                src: None,
                frame_width: None,
                frame_height: None,
                cols: None,
                frame_row: None,
                frame_count: None,
                draw_x: None,
                draw_y: None,
                frame_durations: Vec::new(),
                style: None,
                direction: None,
                on_click: None,
                on_change: None,
                on_right_click: None,
                on_scroll_up: None,
                on_scroll_down: None,
                bindings: Some(RendererWidgetBindings {
                    text: vec![binding.trim_matches(&['{', '}', ' '][..]).into()],
                    ..Default::default()
                }),
                children: Vec::new(),
            }],
        }
    }

    fn payload_with_windows(windows: Vec<RendererWindow>) -> RendererPayload {
        RendererPayload {
            version: 1,
            theme_css: None,
            theme_css_files: Vec::new(),
            windows,
            sources: Vec::new(),
        }
    }

    #[test]
    fn config_asset_paths_reject_traversal_and_absolute_paths() {
        assert!(is_safe_relative_path("spotify_cache/album_art.png"));
        assert!(is_safe_relative_path("./spotify_cache/default.png"));
        assert!(!is_safe_relative_path("../secret"));
        assert!(!is_safe_relative_path("/etc/passwd"));
    }

    #[test]
    fn source_ids_for_windows_unions_multiple_open_windows() {
        let payload = payload_with_windows(vec![
            label_window("top-bar", "{{ top_bar.cpu }}"),
            label_window("swaync-panel", "{{ swaync.dnd_label }}"),
            label_window("weather", "{{ weather.temp }}"),
        ]);

        let ids = source_ids_for_windows(&payload, &["top-bar", "swaync-panel"]);

        assert!(ids.contains("top_bar"));
        assert!(ids.contains("swaync"));
        assert!(!ids.contains("weather"));
    }

    #[test]
    fn source_workers_reconcile_stops_sources_not_needed_by_open_windows() {
        let dir = tempfile::tempdir().unwrap();
        let (poll_tx, _poll_rx) = std::sync::mpsc::channel();
        let (listener_tx, _listener_rx) = std::sync::mpsc::channel();
        let sources = vec![
            test_source("top_bar", SourceMode::Poll),
            test_source("swaync", SourceMode::Listen),
        ];
        let mut workers = SourceWorkers::new(dir.path().to_path_buf(), poll_tx, listener_tx);

        workers.reconcile(
            &sources,
            ["top_bar".to_string(), "swaync".to_string()]
                .into_iter()
                .collect(),
        );
        assert!(workers.is_running("top_bar"));
        assert!(workers.is_running("swaync"));

        workers.reconcile(&sources, ["top_bar".to_string()].into_iter().collect());

        assert!(workers.is_running("top_bar"));
        assert!(!workers.is_running("swaync"));
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

        let (tx, rx) = std::sync::mpsc::channel();
        start_listeners_into(&sources, dir.path().to_path_buf(), tx);
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
        let (tx, rx) = std::sync::mpsc::channel();
        start_listeners_into(&sources, dir.path().to_path_buf(), tx);
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

        let (tx, rx) = std::sync::mpsc::channel();
        start_listeners_into(&sources, dir.path().to_path_buf(), tx);
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
