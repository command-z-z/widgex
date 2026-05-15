use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlatformCapabilities {
    pub layer_shell: bool,
    pub exclusive_zone: bool,
    pub click_through: bool,
    pub per_monitor_windows: bool,
    pub desktop_widgets: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRequest {
    pub id: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowHandle {
    id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PlatformError {
    #[error("{platform} does not support {capability}")]
    UnsupportedPlatform {
        platform: String,
        capability: String,
    },
}

pub trait PlatformAdapter {
    fn capabilities(&self) -> PlatformCapabilities;
    fn create_window(&self, request: &WindowRequest) -> Result<WindowHandle, PlatformError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LinuxWaylandAdapter;

#[derive(Debug, Clone)]
pub struct UnsupportedAdapter {
    platform: String,
}

impl WindowHandle {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

impl LinuxWaylandAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl PlatformAdapter for LinuxWaylandAdapter {
    fn capabilities(&self) -> PlatformCapabilities {
        PlatformCapabilities {
            layer_shell: true,
            exclusive_zone: true,
            click_through: true,
            per_monitor_windows: true,
            desktop_widgets: true,
        }
    }

    fn create_window(&self, request: &WindowRequest) -> Result<WindowHandle, PlatformError> {
        Ok(WindowHandle::new(&request.id))
    }
}

impl UnsupportedAdapter {
    pub fn new(platform: impl Into<String>) -> Self {
        Self {
            platform: platform.into(),
        }
    }
}

impl PlatformAdapter for UnsupportedAdapter {
    fn capabilities(&self) -> PlatformCapabilities {
        PlatformCapabilities {
            layer_shell: false,
            exclusive_zone: false,
            click_through: false,
            per_monitor_windows: false,
            desktop_widgets: false,
        }
    }

    fn create_window(&self, _request: &WindowRequest) -> Result<WindowHandle, PlatformError> {
        Err(PlatformError::UnsupportedPlatform {
            platform: self.platform.clone(),
            capability: "window creation".to_string(),
        })
    }
}
