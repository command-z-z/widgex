use std::fs;

use tempfile::tempdir;
use widgex_ipc::{DaemonRequest, DaemonResponse};
use widgexd::{DaemonCommand, DaemonState, WidgetProcessTable};

const CONFIG: &str = r#"
version = 1

[[windows]]
id = "top-bar"

[windows.size]
height = 32

[[windows.widgets]]
type = "label"
text = "hello"
"#;

#[test]
fn daemon_loads_config_and_reports_status() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    fs::write(&config_path, CONFIG).unwrap();

    let mut daemon = DaemonState::new();
    daemon
        .load_config(&config_path)
        .expect("config should load");

    let status = daemon.handle_command(DaemonCommand::Status).unwrap();

    assert_eq!(
        status.loaded_config_path.as_deref(),
        Some(config_path.as_path())
    );
    assert_eq!(status.window_ids, vec!["top-bar"]);
    assert!(status.last_error.is_none());
}

#[test]
fn daemon_keeps_previous_config_when_reload_fails() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    fs::write(&config_path, CONFIG).unwrap();

    let mut daemon = DaemonState::new();
    daemon
        .load_config(&config_path)
        .expect("config should load");
    fs::write(&config_path, "version = 2").unwrap();

    let error = daemon.reload().expect_err("invalid reload should fail");
    let status = daemon.handle_command(DaemonCommand::Status).unwrap();

    assert!(error.to_string().contains("unsupported config version"));
    assert_eq!(status.window_ids, vec!["top-bar"]);
    assert!(status
        .last_error
        .unwrap()
        .contains("unsupported config version"));
}

#[test]
fn widget_process_table_toggles_window_open_and_closed() {
    let mut table = WidgetProcessTable::default();

    let first = table.apply_request(&DaemonRequest::Open {
        window_id: Some("desktop-clock".to_string()),
        toggle: true,
    });
    let second = table.apply_request(&DaemonRequest::Open {
        window_id: Some("desktop-clock".to_string()),
        toggle: true,
    });

    assert_eq!(
        first,
        DaemonResponse::ok("opened desktop-clock").with_open_windows(vec!["desktop-clock".into()])
    );
    assert_eq!(second, DaemonResponse::ok("closed desktop-clock"));
}

#[test]
fn widget_process_table_reload_preserves_open_windows() {
    let mut table = WidgetProcessTable::default();
    table.apply_request(&DaemonRequest::Open {
        window_id: Some("desktop-clock".to_string()),
        toggle: false,
    });

    let response = table.apply_request(&DaemonRequest::Reload);

    assert_eq!(
        response,
        DaemonResponse::ok("daemon reloaded").with_open_windows(vec!["desktop-clock".into()])
    );
}
