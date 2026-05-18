# Widgex

Widgex is an early-stage modern widget runtime inspired by Eww, AGS, Quickshell, Rainmeter, and Uebersicht.

The first implementation target is Arch Linux on Wayland. The architecture keeps platform adapters separate so Windows and macOS support can be added without replacing the configuration model or renderer contract.

## Current MVP

- Rust workspace: `widgex-core`, `widgex-source`, `widgex-webview`, `widgex-ipc`, `widgex-platform`, `widgexd`, and the `widgex` CLI.
- TOML config parsing, validation, and JSON Schema generation for editor completion.
- Config-to-renderer JSON payload generation with `{{ source.field }}` binding resolution.
- Live data sources (`time`, `battery`, shell command) polled in-process and pushed to the renderer.
- SolidJS renderer running inside a `webkit2gtk` webview, anchored as a desktop layer via `gtk-layer-shell`.
- Daemon that spawns/toggles widget windows over a Unix socket.
- Platform capability abstraction with a Linux Wayland adapter.
- Single-process multi-window renderer: all WebViews share one `WebKitWebProcess` via a patched `wry` (see `vendor/wry`), reducing memory from ~350 MB × N to ~350 MB flat.

The reactive loop is end-to-end: a source is polled on its `interval_ms`, bindings are
re-resolved, and the webview updates live.

## Recent Updates

### Animation widget (`kind = "animation"`)

Spritesheet-based frame animation rendered in a `<canvas>`. Config fields:

```toml
[[windows.widgets]]
kind = "animation"
src = "sprite.png"
frame_width = 192
frame_height = 208
cols = 4
frame_count = "12"
frame_durations = [100, 100, 150]   # ms per frame, cycled
draw_x = "0"   # optional: draw at fixed (x, y) in a full-screen canvas
draw_y = "0"
```

Also supported by the native GTK renderer for flicker-free transparent surfaces.

### Hyprland event source (`kind = "unix_socket"`, `format = "hyprland_event"`)

Subscribe to Hyprland's socket2 event stream and bind workspace/window fields directly in widget templates:

```toml
[[sources]]
id = "hypr"
kind = "unix_socket"
path = "/tmp/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket2.sock"
format = "hyprland_event"
```

Parsed fields include `event`, `payload`, `workspace_id`, `workspace_name`, `window_class`, `window_title`, and per-workspace style helpers (`workspace1_style` … `workspace10_style`).

### Native GTK renderer (`native_render = true`)

A GTK widget-tree renderer that replaces the WebView for specific windows, eliminating the ghost-pixel artifact caused by webkit2gtk's DMA-BUF incremental repaint on transparent layer-shell surfaces ([wry#1524](https://github.com/tauri-apps/wry/issues/1524)).

```toml
[[windows]]
id = "clock"
native_render = true   # use GTK widget tree instead of WebView
```

Remove this flag once the upstream issue is resolved.

### Daemon hot-reload (`widgex daemon reload`)

Reload config and restart all open windows without stopping the daemon:

```bash
widgex daemon reload
```

### Extended mouse events

Widgets now support right-click and scroll-wheel actions alongside `on_click`:

```toml
[[windows.widgets]]
kind = "box"
on_click      = { run = "pactl set-sink-volume @DEFAULT_SINK@ +5%" }
on_right_click = { run = "pavucontrol" }
on_scroll_up   = { run = "pactl set-sink-volume @DEFAULT_SINK@ +2%" }
on_scroll_down = { run = "pactl set-sink-volume @DEFAULT_SINK@ -2%" }
```

### Multiple theme CSS files

```toml
[theme]
css_files = ["variables.css", "reset.css"]
```

Files are loaded in order after the inline `css` field, making it easy to split theme variables from global styles.

### Module-relative working directories

Shell commands and CSS file paths inside a widget module directory now resolve relative to that module's directory, not the root config file.

## System Dependencies

Building the webview renderer requires (Arch package names):

- `webkit2gtk-4.1`
- `gtk3`
- `gtk-layer-shell`

The renderer bundle must be built before running a widget window:

```bash
cd apps/renderer && npm install && npm run build
```

## Try It

```bash
cargo run -p widgex -- init --template desktop-clock --config /tmp/widgex/config.toml
cargo run -p widgex -- check --config /tmp/widgex/config.toml
cargo run -p widgex -- render --config /tmp/widgex/config.toml
cargo run -p widgex -- daemon start --config /tmp/widgex/config.toml
cargo run -p widgex -- open --toggle desktop-clock
cargo run -p widgex -- daemon status
cargo run -p widgex -- daemon reload
cargo run -p widgex -- schema
cargo run -p widgex -- doctor
```

`render` is the important desktop-runtime boundary: it parses the TOML config and emits the normalized JSON payload that the daemon/renderer will consume. The Vite dev server is only for frontend development; users should create widgets through config files.

The normal desktop workflow mirrors Eww: start the daemon once, then toggle windows by id:

```bash
widgex daemon start --config ~/.config/widgex/config.toml
widgex open --toggle desktop-clock
```

## Development

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The renderer lives in `apps/renderer`. This environment currently has `npm` but not `pnpm`, so use npm unless you install pnpm separately.
