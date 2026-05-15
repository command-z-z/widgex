use std::{
    collections::BTreeSet,
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command as ProcessCommand, Stdio},
    time::SystemTime,
};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use widgex_core::{Config, diagnostics_to_string, load_validated_config};
use widgex_ipc::{DaemonRequest, DaemonResponse};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonCommand {
    Status,
    Reload,
    Open(String),
    Close(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub loaded_config_path: Option<PathBuf>,
    pub window_ids: Vec<String>,
    pub last_error: Option<String>,
    pub started_at: SystemTime,
}

#[derive(Debug)]
pub struct DaemonState {
    config: Option<Config>,
    config_path: Option<PathBuf>,
    last_error: Option<String>,
    started_at: SystemTime,
}

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error("no config has been loaded")]
    NoConfigLoaded,
    #[error("{0}")]
    Config(String),
}

#[derive(Debug, Default)]
pub struct WidgetProcessTable {
    open_windows: BTreeSet<String>,
}

#[derive(Debug)]
pub struct WidgetProcessManager {
    config_path: PathBuf,
    cli_path: PathBuf,
    children: std::collections::BTreeMap<String, Child>,
}

impl Default for DaemonState {
    fn default() -> Self {
        Self::new()
    }
}

impl DaemonState {
    pub fn new() -> Self {
        Self {
            config: None,
            config_path: None,
            last_error: None,
            started_at: SystemTime::now(),
        }
    }

    pub fn load_config(&mut self, path: impl AsRef<Path>) -> Result<(), DaemonError> {
        let path = path.as_ref();
        let config = read_validated(path)?;

        self.config = Some(config);
        self.config_path = Some(path.to_path_buf());
        self.last_error = None;
        Ok(())
    }

    pub fn reload(&mut self) -> Result<(), DaemonError> {
        let Some(path) = self.config_path.clone() else {
            return Err(DaemonError::NoConfigLoaded);
        };

        match read_validated(&path) {
            Ok(config) => {
                self.config = Some(config);
                self.last_error = None;
                Ok(())
            }
            Err(error) => {
                self.last_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    pub fn handle_command(&mut self, command: DaemonCommand) -> Result<DaemonStatus, DaemonError> {
        match command {
            DaemonCommand::Status => Ok(self.status()),
            DaemonCommand::Reload => {
                self.reload()?;
                Ok(self.status())
            }
            DaemonCommand::Open(_) | DaemonCommand::Close(_) => Ok(self.status()),
        }
    }

    pub fn status(&self) -> DaemonStatus {
        DaemonStatus {
            loaded_config_path: self.config_path.clone(),
            window_ids: self
                .config
                .as_ref()
                .map(|config| {
                    config
                        .windows
                        .iter()
                        .map(|window| window.id.clone())
                        .collect()
                })
                .unwrap_or_default(),
            last_error: self.last_error.clone(),
            started_at: self.started_at,
        }
    }
}

impl WidgetProcessTable {
    pub fn apply_request(&mut self, request: &DaemonRequest) -> DaemonResponse {
        match request {
            DaemonRequest::Status => DaemonResponse::ok("daemon running")
                .with_open_windows(self.open_windows.iter().cloned().collect()),
            DaemonRequest::Stop => DaemonResponse::ok("daemon stopping")
                .with_open_windows(self.open_windows.iter().cloned().collect()),
            DaemonRequest::Open { window_id, toggle } => {
                let Some(window_id) = window_id.as_deref() else {
                    return DaemonResponse::error("open requires a window id");
                };

                if *toggle && self.open_windows.remove(window_id) {
                    return DaemonResponse::ok(format!("closed {window_id}"))
                        .with_open_windows(self.open_windows.iter().cloned().collect());
                }

                self.open_windows.insert(window_id.to_string());
                DaemonResponse::ok(format!("opened {window_id}"))
                    .with_open_windows(self.open_windows.iter().cloned().collect())
            }
            DaemonRequest::Close { window_id } => {
                let Some(window_id) = window_id.as_deref() else {
                    return DaemonResponse::error("close requires a window id");
                };

                if self.open_windows.remove(window_id) {
                    DaemonResponse::ok(format!("closed {window_id}"))
                        .with_open_windows(self.open_windows.iter().cloned().collect())
                } else {
                    DaemonResponse::ok(format!("{window_id} was not open"))
                        .with_open_windows(self.open_windows.iter().cloned().collect())
                }
            }
        }
    }
}

impl WidgetProcessManager {
    pub fn new(config_path: impl Into<PathBuf>, cli_path: impl Into<PathBuf>) -> Self {
        Self {
            config_path: config_path.into(),
            cli_path: cli_path.into(),
            children: std::collections::BTreeMap::new(),
        }
    }

    pub fn handle_request(&mut self, request: DaemonRequest) -> DaemonResponse {
        self.reap_finished();

        self.handle_request_result(request)
            .unwrap_or_else(|error| DaemonResponse::error(error.to_string()))
    }

    fn handle_request_result(&mut self, request: DaemonRequest) -> Result<DaemonResponse> {
        match request {
            DaemonRequest::Status => {
                Ok(DaemonResponse::ok("daemon running").with_open_windows(self.open_window_ids()))
            }
            DaemonRequest::Stop => {
                self.stop_all();
                Ok(DaemonResponse::ok("daemon stopping"))
            }
            DaemonRequest::Open { window_id, toggle } => {
                let window_id = self.resolve_window_id(window_id.as_deref())?;

                if toggle && self.children.contains_key(&window_id) {
                    self.stop_window(&window_id)?;
                    Ok(DaemonResponse::ok(format!("closed {window_id}"))
                        .with_open_windows(self.open_window_ids()))
                } else if self.children.contains_key(&window_id) {
                    Ok(DaemonResponse::ok(format!("{window_id} already open"))
                        .with_open_windows(self.open_window_ids()))
                } else {
                    self.spawn_window(&window_id)?;
                    Ok(DaemonResponse::ok(format!("opened {window_id}"))
                        .with_open_windows(self.open_window_ids()))
                }
            }
            DaemonRequest::Close { window_id } => {
                let window_id = self.resolve_window_id(window_id.as_deref())?;
                if self.children.contains_key(&window_id) {
                    self.stop_window(&window_id)?;
                    Ok(DaemonResponse::ok(format!("closed {window_id}"))
                        .with_open_windows(self.open_window_ids()))
                } else {
                    Ok(DaemonResponse::ok(format!("{window_id} was not open"))
                        .with_open_windows(self.open_window_ids()))
                }
            }
        }
    }

    fn resolve_window_id(&self, requested: Option<&str>) -> Result<String> {
        let config = load_validated_config(&self.config_path)
            .map_err(|diagnostics| anyhow!(diagnostics_to_string(&diagnostics)))?;

        match requested {
            Some(window_id) => {
                if config.windows.iter().any(|window| window.id == window_id) {
                    Ok(window_id.to_string())
                } else {
                    Err(anyhow!("window {window_id:?} not found in config"))
                }
            }
            None => config
                .windows
                .first()
                .map(|window| window.id.clone())
                .ok_or_else(|| anyhow!("config does not define any windows")),
        }
    }

    fn spawn_window(&mut self, window_id: &str) -> Result<()> {
        let child = ProcessCommand::new(&self.cli_path)
            .arg("open")
            .arg("--foreground")
            .arg("--config")
            .arg(&self.config_path)
            .arg(window_id)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("failed to spawn {}", self.cli_path.display()))?;

        self.children.insert(window_id.to_string(), child);
        Ok(())
    }

    fn stop_window(&mut self, window_id: &str) -> Result<()> {
        if let Some(mut child) = self.children.remove(window_id) {
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(())
    }

    fn stop_all(&mut self) {
        let window_ids = self.children.keys().cloned().collect::<Vec<_>>();
        for window_id in window_ids {
            let _ = self.stop_window(&window_id);
        }
    }

    fn reap_finished(&mut self) {
        self.children.retain(|_, child| match child.try_wait() {
            Ok(Some(_)) => false,
            Ok(None) => true,
            Err(_) => false,
        });
    }

    fn open_window_ids(&self) -> Vec<String> {
        self.children.keys().cloned().collect()
    }
}

pub fn run_socket_daemon(
    config_path: impl AsRef<Path>,
    socket_path: impl AsRef<Path>,
    cli_path: impl AsRef<Path>,
) -> Result<()> {
    let config_path = config_path.as_ref().to_path_buf();
    let socket_path = socket_path.as_ref().to_path_buf();
    let cli_path = cli_path.as_ref().to_path_buf();

    load_validated_config(&config_path)
        .map_err(|diagnostics| anyhow!(diagnostics_to_string(&diagnostics)))?;

    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    remove_stale_socket(&socket_path)?;
    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("failed to bind {}", socket_path.display()))?;
    let mut manager = WidgetProcessManager::new(config_path, cli_path);

    for stream in listener.incoming() {
        let mut stream = stream.context("failed to accept daemon connection")?;
        let request = read_request(&stream)?;
        let should_stop = matches!(request, DaemonRequest::Stop);
        let response = manager.handle_request(request);
        stream
            .write_all(response.to_json_line()?.as_bytes())
            .context("failed to write daemon response")?;

        if should_stop {
            break;
        }
    }

    manager.stop_all();
    let _ = fs::remove_file(&socket_path);
    Ok(())
}

fn read_request(stream: &UnixStream) -> Result<DaemonRequest> {
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .context("failed to read daemon request")?;
    DaemonRequest::from_json_line(&line)
}

fn remove_stale_socket(socket_path: &Path) -> Result<()> {
    if !socket_path.exists() {
        return Ok(());
    }

    if UnixStream::connect(socket_path).is_ok() {
        return Err(anyhow!(
            "daemon socket {} is already in use",
            socket_path.display()
        ));
    }

    fs::remove_file(socket_path)
        .with_context(|| format!("failed to remove stale socket {}", socket_path.display()))
}

fn read_validated(path: &Path) -> Result<Config, DaemonError> {
    load_validated_config(path)
        .map_err(|diagnostics| DaemonError::Config(diagnostics_to_string(&diagnostics)))
}
