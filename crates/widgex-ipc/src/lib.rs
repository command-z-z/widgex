use std::{
    env,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonRequest {
    Status,
    Reload,
    Stop,
    Open {
        window_id: Option<String>,
        toggle: bool,
    },
    Close {
        window_id: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonResponse {
    pub ok: bool,
    pub message: String,
    #[serde(default)]
    pub open_windows: Vec<String>,
}

pub fn default_socket_path() -> PathBuf {
    env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join("widgex.sock")
}

pub fn send_request(
    socket_path: impl AsRef<Path>,
    request: &DaemonRequest,
) -> Result<DaemonResponse> {
    let socket_path = socket_path.as_ref();
    let mut stream = UnixStream::connect(socket_path).with_context(|| {
        format!(
            "failed to connect to daemon socket {}",
            socket_path.display()
        )
    })?;
    stream
        .write_all(request.to_json_line()?.as_bytes())
        .context("failed to write daemon request")?;

    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .context("failed to read daemon response")?;
    DaemonResponse::from_json_line(&line)
}

impl DaemonRequest {
    pub fn to_json_line(&self) -> Result<String> {
        Ok(format!("{}\n", serde_json::to_string(self)?))
    }

    pub fn from_json_line(line: &str) -> Result<Self> {
        Ok(serde_json::from_str(line.trim_end())?)
    }
}

impl DaemonResponse {
    pub fn ok(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: message.into(),
            open_windows: Vec::new(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: message.into(),
            open_windows: Vec::new(),
        }
    }

    pub fn with_open_windows(mut self, mut open_windows: Vec<String>) -> Self {
        open_windows.sort();
        self.open_windows = open_windows;
        self
    }

    pub fn to_json_line(&self) -> Result<String> {
        Ok(format!("{}\n", serde_json::to_string(self)?))
    }

    pub fn from_json_line(line: &str) -> Result<Self> {
        Ok(serde_json::from_str(line.trim_end())?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RendererRequest {
    Open { window_id: String },
    Close { window_id: String },
    Stop,
    Status,
    /// Reload all webviews in-process. GTK windows stay open; only WebKitWebProcess restarts.
    Reload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RendererResponse {
    pub ok: bool,
    pub message: String,
    #[serde(default)]
    pub open_windows: Vec<String>,
}

impl RendererRequest {
    pub fn to_json_line(&self) -> Result<String> {
        Ok(format!("{}\n", serde_json::to_string(self)?))
    }

    pub fn from_json_line(line: &str) -> Result<Self> {
        Ok(serde_json::from_str(line.trim_end())?)
    }
}

impl RendererResponse {
    pub fn ok(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: message.into(),
            open_windows: Vec::new(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: message.into(),
            open_windows: Vec::new(),
        }
    }

    pub fn with_open_windows(mut self, mut open_windows: Vec<String>) -> Self {
        open_windows.sort();
        self.open_windows = open_windows;
        self
    }

    pub fn to_json_line(&self) -> Result<String> {
        Ok(format!("{}\n", serde_json::to_string(self)?))
    }

    pub fn from_json_line(line: &str) -> Result<Self> {
        Ok(serde_json::from_str(line.trim_end())?)
    }
}

pub fn send_renderer_request(
    socket_path: impl AsRef<Path>,
    request: &RendererRequest,
) -> Result<RendererResponse> {
    let socket_path = socket_path.as_ref();
    let mut stream = UnixStream::connect(socket_path).with_context(|| {
        format!(
            "failed to connect to renderer socket {}",
            socket_path.display()
        )
    })?;
    stream
        .write_all(request.to_json_line()?.as_bytes())
        .context("failed to write renderer request")?;

    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .context("failed to read renderer response")?;
    RendererResponse::from_json_line(&line)
}
