# Widgex Architecture

Widgex is a desktop widget runtime that separates concerns cleanly across three
roles: a CLI front-end, a long-running daemon, and a renderer subprocess. TOML
config defines windows and data sources; a SolidJS bundle handles presentation
inside a WebKit webview (or a GTK widget tree for the native-render path on
Linux). The platform abstraction layer makes the renderer portable across Linux
(Wayland + X11), Windows, and macOS without touching the core config, IPC, or
front-end code.

---

## Workspace Layout

```
widgex/
├── Cargo.toml                   # workspace root
├── crates/
│   ├── widgex-core/             # config structs, validation, template engine (portable)
│   ├── widgex-ipc/              # IPC message types + platform-gated transport helpers
│   ├── widgex-source/           # data source polling/listening (mostly portable)
│   ├── widgex-webview/          # platform dispatcher + per-OS renderer implementations
│   │   └── src/
│   │       ├── lib.rs           # thin #[cfg] dispatcher (WindowPreview + re-exports)
│   │       ├── linux.rs         # Linux renderer (Wayland + X11, GTK + webkit2gtk)
│   │       ├── x11_window.rs    # X11 EWMH hints + GDK monitor positioning
│   │       ├── native_renderer.rs  # GTK widget-tree renderer (Linux ghosting workaround)
│   │       ├── windows.rs       # Windows stubs (wry/WebView2 + Win32, todo)
│   │       ├── macos.rs         # macOS stubs (wry/WKWebView + AppKit, todo)
│   │       └── stub.rs          # other platforms (bail!)
│   ├── widgex-cli/              # widgex binary (clap CLI, templates)
│   └── widgexd/                 # widgexd binary (daemon, process manager)
└── apps/
    └── renderer/                # SolidJS front-end (Vite, TypeScript)
        └── src/
            ├── App.tsx          # root component, payload store, widget dispatch
            └── widgetTree.ts    # raw → normalized widget type mapping
```

### Dependency graph

```
widgex-cli ──► widgex-core
           ──► widgex-ipc
           ──► widgex-webview ──► widgex-core
                               ──► widgex-ipc
                               ──► widgex-source ──► widgex-core
widgexd    ──► widgex-core
           ──► widgex-ipc
```

### What is portable across all platforms

| Layer | Change needed |
|-------|--------------|
| `widgex-core` | none — pure Rust types, template engine, no platform deps |
| `widgex-cli` | none — clap parsing + routing, fully portable |
| SolidJS front-end | none — runs inside WebView, no host platform coupling |
| TOML config format | none — `anchor`/`layer` fields mapped per-platform in the renderer |
| `widgex-source` (most) | none — `battery` sysfs reading is `#[cfg(target_os="linux")]` |
| `widgex-ipc` (protocol) | none — JSON line protocol unchanged; transport is `#[cfg]`-gated |

---

## Process Model

```
user terminal
    │
    └── widgex open <id>           (widgex-cli)
            │
            │  DaemonRequest::Open  (Unix socket / Named Pipe)
            ▼
        widgexd daemon              (widgexd, long-running)
            │
            │  if renderer not running:
            │      spawn widgex renderer --foreground --config … --socket …
            │
            │  RendererRequest::Open  (Unix socket / Named Pipe)
            ▼
        widgex renderer             (widgex-cli renderer subcommand)
            │
            ├── platform event loop  (GTK main loop on Linux; Win32/NSApp on Windows/macOS)
            ├── one window per open widget window
            ├── per-source poll threads   (one per poll DataSource)
            └── per-source listen threads (one per listen DataSource)
```

Key points:
- `widgexd` never touches GUI code; it is a pure socket-loop process that manages
  the renderer child process.
- On Linux the renderer is spawned with `process_group(0)` (`#[cfg(unix)]`) so all
  listener shell processes share the renderer's PGID and are killed together on exit.
  On Windows this is replaced by a Job Object (todo).
- On the last window close the renderer exits; `widgexd` detects this via
  `child.try_wait()` on the next reap cycle.

### Socket paths

| Socket | Linux/macOS | Windows (planned) |
|--------|-------------|-------------------|
| Daemon socket | `$XDG_RUNTIME_DIR/widgex.sock` | `\\.\pipe\widgex` |
| Renderer socket | `$XDG_RUNTIME_DIR/widgex-renderer.sock` | `\\.\pipe\widgex-renderer` |

---

## Platform Abstraction Layer (`widgex-webview`)

### Dispatcher (`lib.rs`)

`lib.rs` is a thin `#[cfg]` router. It defines `WindowPreview` (the only
platform-independent type) and re-exports the correct implementation module:

```rust
// lib.rs (simplified)
#[cfg(target_os = "linux")]   pub use linux::{ run_renderer, run_widget_window, … };
#[cfg(target_os = "windows")] pub use windows::{ … };
#[cfg(target_os = "macos")]   pub use macos::{ … };
#[cfg(other)]                 pub use stub::{ … };  // bail!() stubs
```

The six public functions are identical in signature across all platforms:

| Function | Purpose |
|----------|---------|
| `run_widget_window` | Open one window, run event loop until closed |
| `run_renderer` | Multi-window renderer, control socket listener |
| `build_window_preview` | Dry-run text preview, no GUI |
| `handle_widget_event` | Deserialise JS IPC event, call `execute_action` |
| `execute_action` | Run shell command / emit event |
| `inline_theme_css` | Read CSS file(s) from disk, inline into payload |

### Linux backend (`linux.rs`)

Runtime detection at startup (cached via `OnceLock`):

```rust
pub(crate) fn display_backend() -> DisplayBackend {
    *BACKEND.get_or_init(|| {
        if gtk_layer_shell::is_supported() { Wayland } else { X11 }
    })
}
```

The two dispatch helpers called by both `run_widget_window` and `add_window`:

```rust
fn apply_desktop_hints(window: &gtk::Window, spec: &RendererWindow) {
    match display_backend() {
        Wayland => { window.init_layer_shell(); window.set_layer(…); … }
        X11     => x11_window::apply_hints_before_show(window, spec.layer),
    }
}

fn apply_desktop_position(window: &gtk::Window, spec: &RendererWindow) {
    if display_backend() == X11 {
        x11_window::apply_position_after_show(window, &spec.anchor, …);
    }
    // Wayland: layer-shell handles positioning; no move_() needed
}
```

### X11 backend (`x11_window.rs`)

Implements the Wayland layer-shell contract using EWMH/ICCCM hints and manual
`gdk::Window::move_()` after `show_all()`.

**Layer → hint mapping:**

| layer | `_NET_WM_WINDOW_TYPE` | z-order |
|-------|-----------------------|---------|
| `background` | `Desktop` | `keep_below` |
| `bottom` | `Dock` | WM-managed |
| `top` | `Notification` | `keep_above` |
| `overlay` | `Splashscreen` | `keep_above` |

**Positioning flow:**
1. `apply_hints_before_show` — sets WM type hint + `skip_taskbar/pager` + RGBA
   visual + `connect_realize(|w| w.stick())` — all before `show_all()`.
2. `apply_position_after_show` — resolves GDK monitor by name (or falls back
   to primary), reads `monitor.geometry()`, computes `(x, y)` from `anchor` +
   `margin`, calls `window.move_(x, y)`.

`exclusive_zone` has no X11 equivalent (`_NET_WM_STRUT_PARTIAL` requires xlib);
non-zero values are silently ignored on X11.

### Windows backend (`windows.rs`) — stub

- **Window**: `wry::WebViewBuilder::build()` creates an independent HWND (no
  parent GTK window needed).
- **Desktop anchoring**: `SetWindowPos` for position + `HWND_BOTTOM`/`HWND_TOPMOST`
  for layer; `SetWindowLongW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE)` to hide from
  taskbar.
- **Click-through**: `SetWindowLongW(… | WS_EX_TRANSPARENT | WS_EX_LAYERED)`.
- **Monitor geometry**: `GetMonitorInfoW` + `EnumDisplayMonitors`.
- **Event loop**: Win32 `GetMessageW` / `DispatchMessageW`; 16 ms `SetTimer`
  for payload ticks.
- **Control socket**: Named Pipe (`CreateNamedPipeW`) replaces `UnixListener`.
- **New deps**: `windows = "0.58"` (Win32 features) + `raw-window-handle = "0.6"`.

### macOS backend (`macos.rs`) — stub

- **Window**: `wry::WebViewBuilder::build()` creates an independent NSWindow +
  WKWebView (no GTK needed).
- **Desktop anchoring**: `[NSWindow setLevel: kCGDesktopWindowLevel]` for layer;
  `[NSWindow setCollectionBehavior: CanJoinAllSpaces | Stationary]` for all-desktop
  presence.
- **Click-through**: `[NSWindow setIgnoresMouseEvents: YES]`.
- **Monitor geometry**: `NSScreen.screens` + y-axis flip (Quartz origin = bottom-left).
- **Event loop**: tao `EventLoop` or `NSApp.run()`.
- **Control socket**: macOS is Unix — reuses `UnixListener` / `UnixStream` unchanged.
- **New deps**: `objc2 = "0.5"` + `objc2-app-kit` + `raw-window-handle = "0.6"`.

---

## IPC Protocol (`widgex-ipc`)

All messages are newline-delimited JSON (`serde_json`), one request per
connection (the client writes one line; the server replies with one line then
closes).

### CLI ↔ Daemon (`DaemonRequest` / `DaemonResponse`)

```rust
enum DaemonRequest {
    Status,
    Reload,
    Stop,
    Open { window_id: Option<String>, toggle: bool },
    Close { window_id: Option<String> },
}

struct DaemonResponse {
    ok: bool,
    message: String,
    open_windows: Vec<String>,
}
```

### Daemon ↔ Renderer (`RendererRequest` / `RendererResponse`)

```rust
enum RendererRequest {
    Open { window_id: String },
    Close { window_id: String },
    Stop,
    Status,
    Reload,   // restarts WebKitWebProcess in-place (webview.reload())
}
```

### Transport gating

```
#[cfg(unix)]    → std::os::unix::net::UnixStream   (Linux + macOS)
#[cfg(windows)] → Named Pipe via Win32 (todo)
#[cfg(other)]   → bail!()
```

The renderer socket is non-blocking on Linux/macOS; it is polled inside the
16 ms GTK `timeout_add_local` tick so IPC does not block the event loop.

---

## Config System (`widgex-core`)

### File format

Config is TOML v1. The root file may list module paths:

```toml
version = 1
modules = ["widgets/clock/clock.toml", "widgets/bar/bar.toml"]

[theme]
css = "style.css"

[permissions]
allow_shell = true
```

Each module is an independently-parsed TOML fragment that may define its own
`[[sources]]`, `[[windows]]`, and one `css` path. Modules let each widget live
in its own directory.

### Config structs

```
Config
├── version: u16
├── modules: Vec<String>         # module paths (resolved at load time)
├── theme: Option<Theme>         # css / css_files
├── permissions: Permissions     # allow_shell
├── sources: Vec<DataSource>
│   ├── id, kind, mode, format
│   ├── interval_ms, timeout_ms
│   ├── command (shell), path (unix_socket)
│   └── working_dir (set from module dir, not serialised)
└── windows: Vec<WindowSpec>
    ├── id, title, layer, anchor, margin, size
    ├── exclusive_zone, click_through, monitor
    ├── native_render (Linux-only GTK workaround flag)
    └── widgets: Vec<WidgetNode>
```

### Validation pipeline

```
parse_config_file(path)
    → TOML deserialize
    → expand_config_modules()   # load each module, merge sources/windows
    → validate_config()         # duplicate ids, missing fields, permission checks,
                                #   binding syntax, source reference checks
    → renderer_payload_from_config()  # flatten to RendererPayload (no Option<…>)
```

`RendererPayload` is the wire format handed to the renderer subprocess (JSON-serialised
in the `__WIDGEX_PAYLOAD__` script injection and via `__widgexPush`).

### Template binding engine

Widget fields (`text`, `value`, `src`, `style`, `frame_row`, …) support
`{{ source_id.field }}` placeholders. Resolution happens entirely in Rust on
the daemon/renderer side via `resolve_payload()`:

```
resolve_payload(base_payload, snapshots) → RendererPayload
```

The front-end receives already-resolved values on every tick; it never
evaluates bindings itself.

---

## Data Source System (`widgex-source`)

### Source kinds

| kind | mode | platform | notes |
|------|------|----------|-------|
| `time` | poll | all | `now` (HH:MM:SS), `date` (YYYY-MM-DD) from `chrono::Local` |
| `battery` | poll | Linux only | reads `/sys/class/power_supply/BAT*/capacity*`; returns empty map elsewhere (`#[cfg(target_os="linux")]`) |
| `shell` | poll + listen | all (Unix: `sh -c`; Windows: needs `cmd /c`) | spawns subprocess per interval; timeout kill via SIGKILL (`#[cfg(unix)]`) |
| `unix_socket` | listen | Linux + macOS | reads lines from a Unix domain socket, reconnects on disconnect |
| `cpu` / `memory` / `network` | declared | — | not yet implemented; return empty snapshots |

### Output formats

| format | parsing |
|--------|---------|
| `text` | single field `value = trimmed stdout` |
| `json` | top-level JSON object fields mapped 1:1 |
| `hyprland_event` | `event>>payload` wire format; produces `event`, `payload`, `workspace_id`, `workspace_name`, `workspace{n}_style` fields |

### Poll architecture

Each poll-mode source runs in its own `thread::spawn` loop with its own interval
(mirroring Eww's `defpoll` design). A slow source (e.g. HTTP fetch) only delays
itself; it does not block other sources. All threads write `SourceSnapshot` into
a single `mpsc::channel`; the event-loop tick closure drains it via `try_recv()`.

### Listen architecture

Listen-mode sources also run in their own threads:
- `shell` listen: spawns a long-running `sh -c` subprocess and reads stdout
  line by line; restarts the subprocess after it exits.
- `unix_socket` listen: connects to a socket path (with `$VAR` expansion) and
  reads lines; reconnects on disconnect.

For `HyprlandEvent` format, the listener maintains a `fields_cache` so that
fields from prior events (e.g. `workspace_name` from `workspacev2`) persist into
the next event's snapshot (e.g. `activewindowv2`). This lets workspace indicators
stay correct when a non-workspace event fires.

`seed_listen_snapshots()` runs `hyprctl activeworkspace -j` once at startup
to seed the initial workspace fields before the socket delivers its first event.

---

## Rendering Pipeline

### Overview

```
RendererPayload (base, unresolved)
        │
        │  resolve_payload(base, snapshots)  [every 16 ms tick]
        ▼
RendererPayload (resolved)
        │
        ├─── WebKit path ──► evaluate_script("window.__widgexPush(json)")
        │                          │
        │                          ▼
        │                    SolidJS (App.tsx)
        │                    createStore + reconcile
        │                    DOM widget tree
        │
        └─── Native path ──► NativeRenderer::update(win_data)   [Linux only]
                                   │
                                   ▼
                             GtkLabel::set_text()
                             SpriteHandle::update_state() + queue_draw()
```

### Event loop integration

On Linux the 16 ms tick is `gtk::glib::timeout_add_local`. On Windows/macOS
(planned) the equivalent is a `SetTimer`/tao timer callback. Each tick:

1. Drain `destroyed_ids` (windows closed by user) → remove from `state.windows`,
   quit event loop if empty.
2. Drain `poll_rx` channel → update `global_poll_snapshots`.
3. Drain `listener_rx` channel → update `global_listen_snapshots`.
4. If any snapshot changed (`dirty = true`), resolve and push to each window.
5. Poll the non-blocking control socket for `RendererRequest` messages.

### WebKit path (Linux)

The WebKit path is used when `native_render = false` (the default on Linux).

- `gtk::Window` (Toplevel, undecorated, `app_paintable = true`)
- `wry::WebView` using `build_gtk(&window)` — GTK/webkit2gtk backend (wry 0.55)
  - Custom protocol `widgex://` serves the embedded SolidJS bundle via `rust-embed`.
  - Config-dir files (images, CSS, spritesheets) served via path-safe relative lookup.
  - `WEBKIT_DISABLE_DMABUF_RENDERER=1` set before GTK init (workaround for DMA-BUF
    protocol errors in VMs).
- Initialization script injects `window.__WIDGEX_PAYLOAD__` and defines a
  `__widgex_queue` buffer so payloads sent before SolidJS mounts are not dropped.
- Each open window gets its own `WebContext` → its own `WebKitWebProcess`.

**Per-window payload push optimization**: the renderer builds a single-window
`RendererPayload` slice before serialising to JSON and calling `evaluate_script`.
This prevents a fast source (e.g. 100 ms pet-brain) from flooding all WebKit
processes with the full multi-window payload on every tick.

### wry standalone path (Windows / macOS)

On Windows and macOS, `wry::WebViewBuilder::build()` creates an independent
native window (HWND / NSWindow) with the WebView embedded. No GTK parent window
is needed. The `widgex://` custom protocol, IPC handler, and initialization script
injection are identical to the Linux path; only the window creation and event loop
differ.

### Widget-action IPC (WebKit → Rust)

User interactions (clicks, scroll, change) in the SolidJS layer call
`window.ipc.postMessage(JSON.stringify({action, value}))`. The wry IPC handler
deserialises this and calls `execute_action()`, which either:
- Runs `sh -c <command>` fire-and-forget on Unix (requires `allow_shell = true`).
- Runs `cmd /c <command>` on Windows (todo).
- Emits a named event to stderr (placeholder).

### Native render path (Linux only)

Used when `native_render = true` on a window. Bypasses webkit entirely.
Only available on Linux (`#[cfg(target_os = "linux")]`).

`NativeRenderer` builds a GTK widget tree that mirrors the `RendererWidget` tree:

| WidgetKind | GTK widget |
|------------|-----------|
| `box` | `gtk::Box` (horizontal or vertical) |
| `label` | `gtk::Label` |
| `image` | `gtk::Image` (loaded via `gdk_pixbuf`) |
| `spacer` | `gtk::Box` (expand fill) |
| `animation` | `gtk::DrawingArea` + Cairo painter (SpriteHandle) |
| `button`, `progress`, `canvas` | not yet supported in native renderer |

CSS is applied via `gtk::CssProvider` with two pre-processing steps:
1. `resolve_css_vars()`: inlines `var(--name)` values from the `:root` block
   (GTK3's CSS engine does not understand custom properties).
2. `strip_unsupported_props()`: removes CSS declarations GTK3 rejects for the
   entire rule block (e.g. `display`, `flex`, `line-height`).

**Sprite animation** (`SpriteHandle`):
- The spritesheet (WebP/PNG) is loaded once via `gdk_pixbuf` and converted
  to a Cairo `ARgb32` `ImageSurface` (premultiplied BGRA).
- A `glib::timeout_add_local(16 ms)` timer advances frames by elapsed time
  and calls `queue_draw()`.
- The `connect_draw` callback erases the surface with `Source` operator (full
  clear, no ghosting) then blits the current frame tile.
- Per-tick `update()` pushes new `x`, `y`, `row`, `frame_count` values from
  the resolved payload; the next `queue_draw()` picks them up.

Both `apply_desktop_hints` and `apply_desktop_position` are called in
`NativeRenderer::new` so the native path also supports X11 correctly.

---

## Window System

### Linux / Wayland

All windows are `gtk::Window` (Toplevel) anchored to the compositor via
`gtk-layer-shell` / `zwlr_layer_shell_v1`.

```toml
layer = "top"                           # background | bottom | top | overlay
anchor = ["top", "left", "right"]       # edges to anchor to

[windows.margin]
top = 32

[windows.size]
height = 32

exclusive_zone = 32                     # reserve space from other layers
```

When anchored to two opposite edges on one axis the layer-shell stretches the
window to fill that axis. `size_request` is set to `-1` on stretched axes.

### Linux / X11

EWMH/ICCCM hints replace layer-shell. The mapping is:

| layer | `_NET_WM_WINDOW_TYPE` | z-order |
|-------|-----------------------|---------|
| `background` | `Desktop` | `keep_below` |
| `bottom` | `Dock` | WM-managed |
| `top` | `Notification` | `keep_above` |
| `overlay` | `Splashscreen` | `keep_above` |

Window positioning uses GDK monitor geometry:
1. Resolve monitor by `monitor` name, or fall back to primary.
2. Read `monitor.geometry()` rectangle.
3. Compute `(x, y)` from `anchor` + `margin`.
4. Call `window.move_(x, y)` after `show_all()`.

`exclusive_zone` is not implemented on X11 (no X11 equivalent without
`_NET_WM_STRUT_PARTIAL`; silently ignored).

### Windows (planned)

Win32 API replaces both GTK and layer-shell:
- Position: `SetWindowPos(hwnd, z_order, x, y, w, h, flags)`
- Layer: `HWND_BOTTOM` / `HWND_TOPMOST` z-order constants
- Taskbar skip: `SetWindowLongW(GWL_EXSTYLE, WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE)`
- Click-through: `… | WS_EX_TRANSPARENT | WS_EX_LAYERED`
- Monitor geometry: `GetMonitorInfoW(MonitorFromWindow(hwnd, …))`

### macOS (planned)

AppKit replaces both GTK and layer-shell:
- Layer: `[NSWindow setLevel: kCGDesktopWindowLevel / NSFloatingWindowLevel / …]`
- All-desktop: `[NSWindow setCollectionBehavior: CanJoinAllSpaces | Stationary]`
- Click-through: `[NSWindow setIgnoresMouseEvents: YES]`
- Monitor geometry: `NSScreen.screens` (y-axis origin = bottom-left; flip needed)

### Click-through (all platforms)

`click_through = true` in config makes the widget window transparent to pointer
events. On Linux an empty `cairo::Region` is set as the input shape after
`show_all()`. On Windows `WS_EX_TRANSPARENT` is applied. On macOS
`setIgnoresMouseEvents: YES` is used.

---

## SolidJS Front-end (`apps/renderer`)

The renderer bundle is a SolidJS application built by Vite. The host (Rust) and
the page share a simple data contract:

| Name | Direction | Purpose |
|------|-----------|---------|
| `window.__WIDGEX_PAYLOAD__` | Rust → JS | initial payload before page scripts run |
| `window.__WIDGEX_WINDOW_ID__` | Rust → JS | which window in the payload to render |
| `window.__widgexPush(payload)` | Rust → JS | live update on each tick |
| `window.__widgex_queue` | JS | buffer for payloads arriving before mount |
| `window.ipc.postMessage(json)` | JS → Rust | widget user-action events |

### State management

`createStore` + `reconcile` (SolidJS) is used for the widget tree:
- `reconcile` diffs the incoming widget array against the store, updating only
  changed fields on the same Proxy references.
- `<For>` over the stable references never unmounts nodes, so click handlers
  are always attached (no dropped clicks during rapid payload updates).

### Widget types

| type | HTML element | notes |
|------|-------------|-------|
| `box` | `<div>` | flex row or column, supports `on_click`, `on_right_click`, `on_scroll_*` |
| `label` | `<span>` | static or bound text |
| `button` | `<button>` | `on_click`, `on_right_click`, `on_scroll_*` |
| `image` | `<img>` | `src` from config directory via `widgex://` protocol |
| `progress` | `<input type="range">` | 0–100, fires `on_change` with current value |
| `spacer` | `<div>` | flex spacer |
| `animation` | `<canvas>` | spritesheet animation; if `draw_x`/`draw_y` set, canvas fills parent |
| `canvas` | `<canvas>` | particle engine (snow / leaves / stars) driven by `requestAnimationFrame` |

---

## CLI Commands (`widgex-cli`)

| command | action |
|---------|--------|
| `widgex init [--template]` | write starter config + style.css |
| `widgex check [--config]` | parse + validate config, print diagnostics |
| `widgex render [--config]` | print resolved `RendererPayload` JSON |
| `widgex open [id] [--toggle]` | send `Open` to daemon (or run renderer directly with `--foreground`) |
| `widgex daemon start/stop/reload/status` | manage widgexd lifecycle |
| `widgex renderer --foreground …` | internal; spawned by daemon; not for direct use |
| `widgex schema` | print JSON Schema for the config format |
| `widgex doctor` | print session type, display info, default config path |
| `widgex ai generate/fix/explain` | scaffolded; not yet implemented |

Config defaults to `$XDG_CONFIG_DIRS/widgex/widgex/config.toml`
(from `directories::ProjectDirs`).

---

## Known Issues and Workarounds

### webkit2gtk transparent-window ghosting (wry#1524)

**Symptom**: On transparent `zwlr_layer_shell_v1` windows, webkit2gtk's DMA-BUF
incremental texture update only marks newly-drawn pixels as damaged. Areas
cleared to alpha=0 are not included in `damageRects`, so GDK copies old pixels
from the previous buffer — leaving ghost trails on the compositor.

**Root cause**: `BufferDMABuf::didUpdateContents` and related functions in
WebKit's UIProcess skip the full-surface copy and rely on damage regions. For
ARGB surfaces, clearing to alpha=0 does not register as damage.

**Workaround**: `native_render = true` on windows that need pixel-perfect
transparency. The GTK/Cairo path (`NativeRenderer`) uses `wl_shm` software
rendering and clears the full surface with the `Source` Cairo operator on every
frame.

**Real fix**: Patch `AcceleratedBackingStore.cpp` to skip incremental update
for ARGB surfaces. Requires building WebKit from source.

### Memory isolation

Each window's `WebViewBuilder` creates its own `WebContext`, giving each window
its own `WebKitWebProcess`. Closing a window calls `terminate_web_process()`
explicitly before `gtk::Window::destroy()` to ensure the process exits
immediately rather than waiting for GObject finalisation.
