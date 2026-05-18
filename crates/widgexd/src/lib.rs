use std::{
    collections::BTreeSet,
    fs,
    io::{self, BufRead, BufReader, Write},
    os::unix::io::AsRawFd,
    os::unix::net::{UnixListener, UnixStream},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command as ProcessCommand, Stdio},
    thread,
    time::{Duration, SystemTime},
};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use widgex_core::{diagnostics_to_string, load_validated_config, Config};
use widgex_ipc::{send_renderer_request, DaemonRequest, DaemonResponse, RendererRequest};

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
    renderer_child: Option<Child>,
    renderer_socket: PathBuf,
    open_windows: BTreeSet<String>,
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
            DaemonRequest::Reload => DaemonResponse::ok("daemon reloaded")
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
    pub fn new(
        config_path: impl Into<PathBuf>,
        cli_path: impl Into<PathBuf>,
        renderer_socket: impl Into<PathBuf>,
    ) -> Self {
        Self {
            config_path: config_path.into(),
            cli_path: cli_path.into(),
            renderer_child: None,
            renderer_socket: renderer_socket.into(),
            open_windows: BTreeSet::new(),
        }
    }

    pub fn handle_request(&mut self, request: DaemonRequest) -> DaemonResponse {
        self.handle_request_result(request)
            .unwrap_or_else(|error| DaemonResponse::error(error.to_string()))
    }

    fn handle_request_result(&mut self, request: DaemonRequest) -> Result<DaemonResponse> {
        match request {
            DaemonRequest::Status => {
                self.reap_finished();
                Ok(DaemonResponse::ok("daemon running").with_open_windows(self.open_window_ids()))
            }
            DaemonRequest::Reload => self.reload_renderer(),
            DaemonRequest::Stop => {
                self.stop_all();
                Ok(DaemonResponse::ok("daemon stopping"))
            }
            DaemonRequest::Open { window_id, toggle } => {
                let window_id = self.resolve_window_id(window_id.as_deref())?;

                if toggle && self.open_windows.contains(&window_id) {
                    // Toggle close: send Close to renderer, remove from open set
                    let _ = send_renderer_request(
                        &self.renderer_socket,
                        &RendererRequest::Close {
                            window_id: window_id.clone(),
                        },
                    );
                    self.open_windows.remove(&window_id);
                    if self.open_windows.is_empty() {
                        self.wait_for_renderer_exit();
                    }
                    Ok(DaemonResponse::ok(format!("closed {window_id}"))
                        .with_open_windows(self.open_window_ids()))
                } else if self.open_windows.contains(&window_id) {
                    Ok(DaemonResponse::ok(format!("{window_id} already open"))
                        .with_open_windows(self.open_window_ids()))
                } else if !self.renderer_running() {
                    // No renderer yet — spawn one with this window as initial window
                    self.spawn_renderer(&[window_id.clone()])?;
                    self.open_windows.insert(window_id.clone());
                    Ok(DaemonResponse::ok(format!("opened {window_id}"))
                        .with_open_windows(self.open_window_ids()))
                } else {
                    // Renderer already running — ask it to open another window
                    send_renderer_request(
                        &self.renderer_socket,
                        &RendererRequest::Open {
                            window_id: window_id.clone(),
                        },
                    )
                    .with_context(|| format!("failed to open window {window_id} in renderer"))?;
                    self.open_windows.insert(window_id.clone());
                    Ok(DaemonResponse::ok(format!("opened {window_id}"))
                        .with_open_windows(self.open_window_ids()))
                }
            }
            DaemonRequest::Close { window_id } => {
                let window_id = self.resolve_window_id(window_id.as_deref())?;
                if self.open_windows.contains(&window_id) {
                    let _ = send_renderer_request(
                        &self.renderer_socket,
                        &RendererRequest::Close {
                            window_id: window_id.clone(),
                        },
                    );
                    self.open_windows.remove(&window_id);
                    if self.open_windows.is_empty() {
                        self.wait_for_renderer_exit();
                    }
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

    fn spawn_renderer(&mut self, initial_window_ids: &[String]) -> Result<()> {
        // Remove stale renderer socket file if it exists (same pattern as
        // remove_stale_socket in run_socket_daemon, but best-effort here since
        // the renderer creates its own socket).
        if self.renderer_socket.exists() {
            let _ = fs::remove_file(&self.renderer_socket);
        }

        let mut cmd = ProcessCommand::new(&self.cli_path);
        cmd.arg("renderer")
            .arg("--foreground")
            .arg("--config")
            .arg(&self.config_path)
            .arg("--socket")
            .arg(&self.renderer_socket);

        for id in initial_window_ids {
            cmd.arg("--window").arg(id);
        }

        let child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // Put the renderer and all its descendant processes in their own
            // process group so that killing the group closes everything cleanly.
            .process_group(0)
            .spawn()
            .with_context(|| format!("failed to spawn renderer {}", self.cli_path.display()))?;

        self.renderer_child = Some(child);

        // Poll for socket readiness: try UnixStream::connect every 50 ms up to
        // 40 times (2 seconds total).
        for _ in 0..40 {
            thread::sleep(Duration::from_millis(50));
            if UnixStream::connect(&self.renderer_socket).is_ok() {
                return Ok(());
            }
        }

        // Timed out — kill the orphaned renderer process group before returning.
        if let Some(mut child) = self.renderer_child.take() {
            let pid = child.id() as i32;
            unsafe { libc::killpg(pid, libc::SIGTERM) };
            let _ = child.wait();
        }
        let _ = std::fs::remove_file(&self.renderer_socket);

        Err(anyhow!(
            "renderer socket {} did not appear within 2 seconds",
            self.renderer_socket.display()
        ))
    }

    fn reload_renderer(&mut self) -> Result<DaemonResponse> {
        load_validated_config(&self.config_path)
            .map_err(|diagnostics| anyhow!(diagnostics_to_string(&diagnostics)))?;

        self.reap_finished();

        // Prefer in-process reload: WebKitWebProcess restarts but GTK windows stay open.
        // Only fall back to kill+respawn if the renderer is dead or the request fails.
        if self.renderer_running() {
            if send_renderer_request(&self.renderer_socket, &RendererRequest::Reload).is_ok() {
                return Ok(DaemonResponse::ok("renderer reloaded")
                    .with_open_windows(self.open_window_ids()));
            }
        }

        let reopen: Vec<String> = self.open_window_ids();
        self.stop_all();

        if !reopen.is_empty() {
            self.spawn_renderer(&reopen)?;
            self.open_windows = reopen.iter().cloned().collect();
        }

        Ok(DaemonResponse::ok("daemon reloaded").with_open_windows(self.open_window_ids()))
    }

    pub fn stop_all(&mut self) {
        // Ask the renderer to stop gracefully (best-effort).
        let _ = send_renderer_request(&self.renderer_socket, &RendererRequest::Stop);

        // Kill the renderer process group unconditionally.
        if let Some(ref child) = self.renderer_child {
            let pid = child.id() as i32;
            // SAFETY: pid is the renderer's process group id because
            // spawn_renderer uses process_group(0).
            unsafe { libc::killpg(pid, libc::SIGTERM) };
        }

        if let Some(mut child) = self.renderer_child.take() {
            let _ = child.wait();
        }

        self.open_windows.clear();
    }

    pub fn reap_finished(&mut self) {
        let gone = match self.renderer_child.as_mut().map(|c| c.try_wait()) {
            None => false,
            Some(Ok(None)) => false,                  // still running
            Some(Ok(Some(_))) | Some(Err(_)) => true, // exited or error → treat as gone
        };

        if gone {
            self.renderer_child = None;
            self.open_windows.clear();
        }
    }

    fn renderer_running(&mut self) -> bool {
        self.reap_finished();
        self.renderer_child.is_some()
    }

    /// Wait for the renderer child to exit (called when open_windows becomes empty).
    /// Sends SIGTERM, polls for up to 500 ms, then force-kills with SIGKILL.
    fn wait_for_renderer_exit(&mut self) {
        let Some(ref mut child) = self.renderer_child else {
            return;
        };
        let pid = child.id() as i32;
        unsafe { libc::killpg(pid, libc::SIGTERM) };
        for _ in 0..5 {
            thread::sleep(Duration::from_millis(100));
            match child.try_wait() {
                Ok(Some(_)) | Err(_) => {
                    self.renderer_child = None;
                    self.open_windows.clear();
                    return;
                }
                Ok(None) => {}
            }
        }
        // Force kill if still alive after 500 ms.
        unsafe { libc::killpg(pid, libc::SIGKILL) };
        let _ = self.renderer_child.as_mut().map(|c| c.wait());
        self.renderer_child = None;
        self.open_windows.clear();
    }

    fn open_window_ids(&self) -> Vec<String> {
        self.open_windows.iter().cloned().collect()
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
    let renderer_socket = socket_path.with_file_name("widgex-renderer.sock");
    let mut manager = WidgetProcessManager::new(config_path, cli_path, renderer_socket);

    loop {
        match accept_timeout(&listener, Duration::from_secs(1)) {
            Ok(None) => {
                // Timeout: reap any finished widget processes so they don't sit as
                // zombies between IPC requests.
                manager.reap_finished();
            }
            Ok(Some(mut stream)) => {
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
            Err(e) => return Err(e.into()),
        }
    }

    manager.stop_all();
    let _ = fs::remove_file(&socket_path);
    Ok(())
}

/// Block until the listener has an incoming connection or `timeout` elapses.
/// Returns `Ok(None)` on timeout, `Ok(Some(stream))` on connection, `Err` on error.
fn accept_timeout(listener: &UnixListener, timeout: Duration) -> io::Result<Option<UnixStream>> {
    let mut pfd = libc::pollfd {
        fd: listener.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as libc::c_int;
    let n = unsafe { libc::poll(std::ptr::addr_of_mut!(pfd), 1, timeout_ms) };
    match n {
        -1 => {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                return Ok(None);
            }
            Err(err)
        }
        0 => Ok(None),
        _ => listener.accept().map(|(stream, _)| Some(stream)),
    }
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
