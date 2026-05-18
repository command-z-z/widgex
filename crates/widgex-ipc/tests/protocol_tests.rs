use widgex_ipc::{default_socket_path, DaemonRequest, DaemonResponse};

#[test]
fn open_toggle_request_round_trips_as_json_line() {
    let request = DaemonRequest::Open {
        window_id: Some("desktop-clock".to_string()),
        toggle: true,
    };

    let line = request.to_json_line().unwrap();
    let parsed = DaemonRequest::from_json_line(&line).unwrap();

    assert_eq!(parsed, request);
    assert!(line.ends_with('\n'));
}

#[test]
fn reload_request_round_trips_as_json_line() {
    let request = DaemonRequest::Reload;

    let line = request.to_json_line().unwrap();
    let parsed = DaemonRequest::from_json_line(&line).unwrap();

    assert_eq!(parsed, request);
}

#[test]
fn status_response_round_trips_with_open_windows() {
    let response = DaemonResponse::ok("running").with_open_windows(vec!["desktop-clock".into()]);

    let line = response.to_json_line().unwrap();
    let parsed = DaemonResponse::from_json_line(&line).unwrap();

    assert!(parsed.ok);
    assert_eq!(parsed.message, "running");
    assert_eq!(parsed.open_windows, vec!["desktop-clock"]);
}

#[test]
fn default_socket_path_lives_under_runtime_or_tmp() {
    let display = default_socket_path().display().to_string();

    assert!(display.contains("widgex.sock"));
}
