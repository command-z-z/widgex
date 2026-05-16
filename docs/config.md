# Widgex Config v1

Widgex starts with TOML because it is easy to validate, generate, and edit with schema support.

## Required Fields

- `version = 1`
- At least one `[[windows]]` entry.
- Every window needs a stable `id`.
- Every window needs at least one `[[windows.widgets]]` entry.

## Modules

Large setups can keep the root config small and load window modules explicitly:

```toml
version = 1
modules = [
  "shared/lyrics.toml",
  "widgets/dashboard/dashboard.toml",
]

[theme]
css = "styles/base.css"
```

A module may define data sources, windows, and one CSS file:

```toml
css = "dashboard.css"

[[sources]]
id = "metadata"
kind = "shell"
mode = "listen"
format = "json"
command = "/usr/bin/python ./src/metadata.py"

[[windows]]
id = "dashboard"

[[windows.widgets]]
type = "label"
text = "{{ metadata.title }}"
```

Module paths are resolved from the root config. A module's `css` path is
resolved from that module file, then concatenated after the root theme CSS.

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
