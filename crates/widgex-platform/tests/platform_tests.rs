use widgex_platform::{
    LinuxWaylandAdapter, PlatformAdapter, PlatformCapabilities, PlatformError, WindowHandle,
};

#[test]
fn linux_wayland_adapter_reports_layer_shell_capability() {
    let adapter = LinuxWaylandAdapter::new();
    let capabilities = adapter.capabilities();

    assert_eq!(
        capabilities,
        PlatformCapabilities {
            layer_shell: true,
            exclusive_zone: true,
            click_through: true,
            per_monitor_windows: true,
            desktop_widgets: true,
        }
    );
}

#[test]
fn unsupported_adapter_returns_actionable_error_for_window_creation() {
    let adapter = widgex_platform::UnsupportedAdapter::new("windows");
    let error = adapter
        .create_window(&widgex_platform::WindowRequest {
            id: "clock".to_string(),
            title: Some("Clock".to_string()),
        })
        .expect_err("unsupported adapter should reject window creation");

    assert_eq!(
        error,
        PlatformError::UnsupportedPlatform {
            platform: "windows".to_string(),
            capability: "window creation".to_string(),
        }
    );
}

#[test]
fn window_handle_exposes_id_for_daemon_tracking() {
    let handle = WindowHandle::new("top-bar");

    assert_eq!(handle.id(), "top-bar");
}
