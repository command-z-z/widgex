use std::path::Path;

use anyhow::{Result, bail};
use widgex_core::{Action, DataSource, RendererPayload};

pub fn build_window_preview(
    _payload: &RendererPayload,
    _window_id: Option<&str>,
) -> Result<crate::WindowPreview> {
    bail!("widgex renderer is not supported on this platform")
}

pub fn run_widget_window(
    _payload: &RendererPayload,
    _config_dir: impl AsRef<Path>,
    _window_id: Option<&str>,
    _sources: &[DataSource],
    _allow_shell: bool,
) -> Result<()> {
    bail!("widgex renderer is not supported on this platform")
}

pub fn run_renderer(
    _payload: &RendererPayload,
    _config_dir: impl AsRef<Path>,
    _config_path: impl AsRef<Path>,
    _sources: &[DataSource],
    _allow_shell: bool,
    _control_socket_path: &Path,
    _initial_window_ids: &[&str],
) -> Result<()> {
    bail!("widgex renderer is not supported on this platform")
}

pub fn handle_widget_event(_body: &str, _config_dir: &Path, _allow_shell: bool) -> Result<()> {
    bail!("widgex renderer is not supported on this platform")
}

pub fn execute_action(
    _action: &Action,
    _value: Option<&str>,
    _config_dir: &Path,
    _allow_shell: bool,
) -> Result<()> {
    bail!("widgex renderer is not supported on this platform")
}

pub fn inline_theme_css(_payload: &mut RendererPayload, _config_dir: &Path) {}
