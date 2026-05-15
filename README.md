# Widgex

Widgex is an early-stage modern widget runtime inspired by Eww, AGS, Quickshell, Rainmeter, and Uebersicht.

The first implementation target is Arch Linux on Wayland. The architecture keeps platform adapters separate so Windows and macOS support can be added without replacing the configuration model or renderer contract.

## Current MVP

- Rust workspace: `widgex-core`, `widgex-source`, `widgex-webview`, `widgex-ipc`, `widgex-platform`, `widgexd`, and the `widgex` CLI.
- TOML config parsing, validation, and JSON Schema generation for editor completion.
- Config-to-renderer JSON payload generation with `{{ source.field }}` binding resolution.
- Live data sources (`time`, `battery`) polled in-process and pushed to the renderer.
- SolidJS renderer running inside a `webkit2gtk` webview, anchored as a desktop layer via `gtk-layer-shell`.
- Daemon that spawns/toggles widget windows over a Unix socket.
- Platform capability abstraction with a Linux Wayland adapter.

The reactive loop is end-to-end: a source is polled on its `interval_ms`, bindings are
re-resolved, and the webview updates live.

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
