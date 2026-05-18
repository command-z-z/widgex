use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    thread,
    time::Duration,
};

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use directories::ProjectDirs;
use widgex_core::{
    diagnostics_to_string, load_validated_config, renderer_payload_from_config, schema_json_pretty,
    Config,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliOutput {
    Message(String),
    Json(String),
}

#[derive(Debug, Parser)]
#[command(
    name = "widgex",
    version,
    about = "Modern cross-platform widget runtime"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Init {
        #[arg(long, default_value = "desktop-clock")]
        template: TemplateName,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    Check {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    Render {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    Open {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long = "window")]
        window_option: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        foreground: bool,
        #[arg(long)]
        toggle: bool,
        window: Option<String>,
    },
    Renderer {
        #[arg(long)]
        foreground: bool,
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        socket: PathBuf,
        #[arg(long = "window")]
        window: Vec<String>,
    },
    Schema,
    Doctor,
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    Ai {
        #[command(subcommand)]
        command: AiCommand,
    },
}

#[derive(Debug, Subcommand)]
enum DaemonCommand {
    Status {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
    },
    Start {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
    },
    Stop {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
    },
    Reload {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum TemplateName {
    DesktopClock,
    TopBar,
}

#[derive(Debug, Subcommand)]
enum AiCommand {
    Generate {
        prompt: Vec<String>,
    },
    Fix {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    Explain {
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

pub fn run<I, T>(args: I) -> Result<CliOutput>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = Cli::parse_from(args);
    run_cli(cli)
}

fn run_cli(cli: Cli) -> Result<CliOutput> {
    match cli.command {
        Command::Init { template, config } => {
            let path = config.unwrap_or_else(default_config_path);
            write_template(&path, template)?;
            Ok(CliOutput::Message(format!(
                "created {} at {}",
                template.as_str(),
                path.to_string_lossy()
            )))
        }
        Command::Check { config } => {
            let path = config.unwrap_or_else(default_config_path);
            load_validated_config(&path)
                .map_err(|diagnostics| anyhow!(diagnostics_to_string(&diagnostics)))?;
            Ok(CliOutput::Message("config ok".to_string()))
        }
        Command::Render { config } => {
            let path = config.unwrap_or_else(default_config_path);
            let config = load_validated_config(&path)
                .map_err(|diagnostics| anyhow!(diagnostics_to_string(&diagnostics)))?;
            let payload = renderer_payload_from_config(&config)
                .map_err(|diagnostics| anyhow!(diagnostics_to_string(&diagnostics)))?;
            Ok(CliOutput::Json(serde_json::to_string_pretty(&payload)?))
        }
        Command::Open {
            config,
            window_option,
            dry_run,
            foreground,
            toggle,
            window,
        } => {
            let path = config.unwrap_or_else(default_config_path);
            let window_id = window_option.or(window);

            if dry_run && toggle {
                let label = window_id.as_deref().unwrap_or("<first-window>");
                return Ok(CliOutput::Message(format!(
                    "would send toggle {label} to daemon"
                )));
            }

            if !foreground && !dry_run {
                let response = widgex_ipc::send_request(
                    widgex_ipc::default_socket_path(),
                    &widgex_ipc::DaemonRequest::Open { window_id, toggle },
                )?;
                if response.ok {
                    return Ok(CliOutput::Message(response.message));
                }
                return Err(anyhow!(response.message));
            }

            let config_model = load_validated_config(&path)
                .map_err(|diagnostics| anyhow!(diagnostics_to_string(&diagnostics)))?;
            let payload = renderer_payload_from_config(&config_model)
                .map_err(|diagnostics| anyhow!(diagnostics_to_string(&diagnostics)))?;
            let window_id = window_id.as_deref();

            if dry_run {
                let preview = widgex_webview::build_window_preview(&payload, window_id)?;
                Ok(CliOutput::Message(format!(
                    "would open desktop window {} ({}x{}): {}",
                    preview.id, preview.width, preview.height, preview.text_preview
                )))
            } else {
                let config_dir = path.parent().unwrap_or_else(|| Path::new("."));
                widgex_webview::run_widget_window(
                    &payload,
                    config_dir,
                    window_id,
                    &config_model.sources,
                    config_model.permissions.allow_shell,
                )?;
                Ok(CliOutput::Message("widget window closed".to_string()))
            }
        }
        Command::Renderer {
            foreground,
            config,
            socket,
            window,
        } => {
            if !foreground {
                return Err(anyhow!("renderer subcommand must be run with --foreground"));
            }
            let config_path = config.unwrap_or_else(default_config_path);
            let config = load_validated_config(&config_path)
                .map_err(|diags| anyhow!(diagnostics_to_string(&diags)))?;
            let payload = renderer_payload_from_config(&config)
                .map_err(|diags| anyhow!(diagnostics_to_string(&diags)))?;
            let config_dir = config_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .to_path_buf();
            let allow_shell = config.permissions.allow_shell;
            let window_ids: Vec<&str> = if window.is_empty() {
                payload.windows.iter().map(|w| w.id.as_str()).collect()
            } else {
                window.iter().map(String::as_str).collect()
            };
            widgex_webview::run_renderer(
                &payload,
                &config_dir,
                &config.sources,
                allow_shell,
                &socket,
                &window_ids,
            )?;
            Ok(CliOutput::Message(String::new()))
        }
        Command::Schema => Ok(CliOutput::Json(schema_json_pretty::<Config>()?)),
        Command::Doctor => Ok(CliOutput::Message(doctor_report())),
        Command::Daemon { command } => Ok(CliOutput::Message(match command {
            DaemonCommand::Status { socket, dry_run } => {
                let socket = socket.unwrap_or_else(widgex_ipc::default_socket_path);
                if dry_run {
                    format!("would query daemon at {}", socket.display())
                } else {
                    let response =
                        widgex_ipc::send_request(&socket, &widgex_ipc::DaemonRequest::Status)?;
                    format_daemon_response(response)
                }
            }
            DaemonCommand::Start {
                config,
                socket,
                dry_run,
            } => {
                let config = config.unwrap_or_else(default_config_path);
                let socket = socket.unwrap_or_else(widgex_ipc::default_socket_path);
                let daemon = daemon_binary_path();
                if dry_run {
                    format!(
                        "would start widgexd {} --config {} --socket {}",
                        daemon.display(),
                        config.display(),
                        socket.display()
                    )
                } else {
                    start_daemon(&daemon, &config, &socket)?;
                    format!("daemon started at {}", socket.display())
                }
            }
            DaemonCommand::Stop { socket, dry_run } => {
                let socket = socket.unwrap_or_else(widgex_ipc::default_socket_path);
                if dry_run {
                    format!("would stop daemon at {}", socket.display())
                } else {
                    let response =
                        widgex_ipc::send_request(&socket, &widgex_ipc::DaemonRequest::Stop)?;
                    response.message
                }
            }
            DaemonCommand::Reload { socket, dry_run } => {
                let socket = socket.unwrap_or_else(widgex_ipc::default_socket_path);
                if dry_run {
                    format!("would reload daemon at {}", socket.display())
                } else {
                    let response =
                        widgex_ipc::send_request(&socket, &widgex_ipc::DaemonRequest::Reload)?;
                    if response.ok {
                        format_daemon_response(response)
                    } else {
                        return Err(anyhow!(response.message));
                    }
                }
            }
        })),
        Command::Ai { command } => Ok(CliOutput::Message(match command {
            AiCommand::Generate { prompt } => format!(
                "AI generate is scaffolded; prompt received: {}",
                prompt.join(" ")
            ),
            AiCommand::Fix { config } => format!(
                "AI fix is scaffolded; config: {}",
                display_optional_path(config)
            ),
            AiCommand::Explain { config } => format!(
                "AI explain is scaffolded; config: {}",
                display_optional_path(config)
            ),
        })),
    }
}

fn format_daemon_response(response: widgex_ipc::DaemonResponse) -> String {
    if response.open_windows.is_empty() {
        response.message
    } else {
        format!(
            "{}\nopen windows: {}",
            response.message,
            response.open_windows.join(", ")
        )
    }
}

impl CliOutput {
    pub fn print(self) {
        match self {
            Self::Message(message) | Self::Json(message) => println!("{message}"),
        }
    }
}

impl TemplateName {
    fn as_str(self) -> &'static str {
        match self {
            Self::DesktopClock => "desktop-clock",
            Self::TopBar => "top-bar",
        }
    }

    fn content(self) -> &'static str {
        match self {
            Self::DesktopClock => DESKTOP_CLOCK_TEMPLATE,
            Self::TopBar => TOP_BAR_TEMPLATE,
        }
    }

    fn style(self) -> &'static str {
        match self {
            Self::DesktopClock => DESKTOP_CLOCK_STYLE,
            Self::TopBar => TOP_BAR_STYLE,
        }
    }
}

fn write_template(path: &Path, template: TemplateName) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
        let style_path = parent.join("style.css");
        fs::write(&style_path, template.style())
            .with_context(|| format!("failed to write {}", style_path.display()))?;
    }

    fs::write(path, template.content())
        .with_context(|| format!("failed to write {}", path.display()))
}

fn default_config_path() -> PathBuf {
    ProjectDirs::from("dev", "widgex", "widgex")
        .map(|dirs| dirs.config_dir().join("config.toml"))
        .unwrap_or_else(|| PathBuf::from(".widgex/config.toml"))
}

fn daemon_binary_path() -> PathBuf {
    env::current_exe()
        .map(|path| path.with_file_name("widgexd"))
        .unwrap_or_else(|_| PathBuf::from("widgexd"))
}

fn start_daemon(daemon: &Path, config: &Path, socket: &Path) -> Result<()> {
    if widgex_ipc::send_request(socket, &widgex_ipc::DaemonRequest::Status).is_ok() {
        return Ok(());
    }

    ProcessCommand::new("setsid")
        .arg("-f")
        .arg(daemon)
        .arg("--config")
        .arg(config)
        .arg("--socket")
        .arg(socket)
        .arg("--cli-path")
        .arg(env::current_exe().context("failed to locate current widgex binary")?)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to start {}", daemon.display()))?;

    for _ in 0..40 {
        thread::sleep(Duration::from_millis(50));
        if widgex_ipc::send_request(socket, &widgex_ipc::DaemonRequest::Status).is_ok() {
            return Ok(());
        }
    }

    Err(anyhow!(
        "daemon did not become ready at {}",
        socket.display()
    ))
}

fn doctor_report() -> String {
    let session = env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_string());
    let wayland_display = env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "not set".to_string());
    let desktop = env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "unknown".to_string());

    format!(
        "Widgex doctor\nsession: {session}\nwayland_display: {wayland_display}\ndesktop: {desktop}\nconfig: {}",
        default_config_path().display()
    )
}

fn display_optional_path(path: Option<PathBuf>) -> String {
    path.map(|path| path.display().to_string())
        .unwrap_or_else(|| default_config_path().display().to_string())
}

const TOP_BAR_TEMPLATE: &str = r#"version = 1

[theme]
css = "style.css"

[[sources]]
id = "clock"
kind = "time"
interval_ms = 1000

[[sources]]
id = "battery"
kind = "battery"
interval_ms = 5000

[[windows]]
id = "top-bar"
layer = "top"
anchor = ["top", "left", "right"]
exclusive_zone = 32

[windows.size]
height = 32

[[windows.widgets]]
type = "box"
direction = "row"

[[windows.widgets.children]]
type = "label"
text = "{{ clock.now }}"

[[windows.widgets.children]]
type = "label"
class = ["battery"]
text = "BAT {{ battery.level }} · {{ battery.status }}"
"#;

const TOP_BAR_STYLE: &str = r#".widgex-window {
  color: #f5f7fb;
  background: transparent;
  font-family: Inter, ui-sans-serif, system-ui, sans-serif;
  background: rgba(18, 22, 31, 0.92);
  border-bottom: 1px solid rgba(255, 255, 255, 0.1);
}
"#;

const DESKTOP_CLOCK_TEMPLATE: &str = r#"version = 1

[theme]
css = "style.css"

[[sources]]
id = "clock"
kind = "time"
interval_ms = 1000

[[windows]]
id = "desktop-clock"
title = "Desktop Clock"
layer = "top"
anchor = ["top", "right"]
click_through = true

[windows.margin]
top = 24
right = 24

[windows.size]
width = 220
height = 72

[[windows.widgets]]
type = "box"
direction = "column"

[[windows.widgets.children]]
type = "label"
class = ["clock-time"]
text = "{{ clock.now }}"
"#;

const DESKTOP_CLOCK_STYLE: &str = r#".widgex-window {
  color: #f5f7fb;
  background: transparent;
  font-family: Inter, ui-sans-serif, system-ui, sans-serif;
  background: rgba(18, 22, 31, 0.9);
  border: 1px solid rgba(255, 255, 255, 0.12);
  border-radius: 8px;
}

.clock-time {
  font-size: 28px;
  font-weight: 600;
}
"#;
