# Widgex Config v1

Widgex starts with TOML because it is easy to validate, generate, and edit with schema support.

## Required Fields

- `version = 1`
- At least one `[[windows]]` entry.
- Every window needs a stable `id`.
- Every window needs at least one `[[windows.widgets]]` entry.

## Security

Shell sources are disabled by default. A config that uses `kind = "shell"` must opt in explicitly:

```toml
[permissions]
allow_shell = true
```

The daemon validates this before applying a config so reload failures do not break an already-running widget layout.

## First Desktop Widget

Create a floating desktop clock:

```bash
cargo run -p widgex -- init --template desktop-clock --config ~/.config/widgex/config.toml
cargo run -p widgex -- check --config ~/.config/widgex/config.toml
cargo run -p widgex -- render --config ~/.config/widgex/config.toml
cargo run -p widgex -- daemon start --config ~/.config/widgex/config.toml
cargo run -p widgex -- open --toggle desktop-clock
```

The `render` command is not a web server. It is the parser boundary that turns user config into the JSON payload consumed by the desktop runtime.
