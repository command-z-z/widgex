use std::fs;

use tempfile::tempdir;
use widgex::{run, CliOutput};

#[test]
fn init_writes_a_valid_top_bar_template() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.toml");

    let output = run(["widgex", "init", "--config", config_path.to_str().unwrap()]).unwrap();

    assert!(matches!(output, CliOutput::Message(message) if message.contains("created")));
    let written = fs::read_to_string(config_path).unwrap();
    widgex_core::parse_config_str(&written).unwrap();
    assert!(dir.path().join("style.css").exists());
}

#[test]
fn init_writes_a_valid_desktop_clock_template() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.toml");

    let output = run([
        "widgex",
        "init",
        "--template",
        "desktop-clock",
        "--config",
        config_path.to_str().unwrap(),
    ])
    .unwrap();

    assert!(matches!(output, CliOutput::Message(message) if message.contains("desktop-clock")));
    let written = fs::read_to_string(config_path).unwrap();
    let config = widgex_core::parse_config_str(&written).unwrap();
    assert_eq!(config.windows[0].id, "desktop-clock");
    assert_eq!(
        config.windows[0].widgets[0].children[0].text.as_deref(),
        Some("{{ clock.now }}")
    );
    let style = fs::read_to_string(dir.path().join("style.css")).unwrap();
    assert!(style.contains(".clock-time"));
    assert!(!style.contains(":root"));
    assert!(!style.contains("display:"));
    assert!(!style.contains("width:"));
    assert!(style.contains(".widgex-window"));
}

#[test]
fn check_reports_valid_config() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    fs::write(
        &config_path,
        r#"
version = 1

[[windows]]
id = "clock"

[windows.size]
height = 24

[[windows.widgets]]
type = "label"
text = "hello"
"#,
    )
    .unwrap();

    let output = run(["widgex", "check", "--config", config_path.to_str().unwrap()]).unwrap();

    assert_eq!(output, CliOutput::Message("config ok".to_string()));
}

#[test]
fn schema_outputs_json_schema() {
    let output = run(["widgex", "schema"]).unwrap();

    match output {
        CliOutput::Json(json) => {
            assert!(json.contains("\"windows\""));
            assert!(json.contains("\"sources\""));
        }
        other => panic!("expected schema json, got {other:?}"),
    }
}

#[test]
fn render_outputs_renderer_payload_for_config_file() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    fs::write(
        &config_path,
        r#"
version = 1

[[windows]]
id = "desktop-clock"
layer = "top"
anchor = ["top", "right"]
click_through = true

[windows.size]
width = 220
height = 72

[[windows.widgets]]
type = "label"
text = "Hello desktop"
"#,
    )
    .unwrap();

    let output = run([
        "widgex",
        "render",
        "--config",
        config_path.to_str().unwrap(),
    ])
    .unwrap();

    match output {
        CliOutput::Json(json) => {
            assert!(json.contains("\"desktop-clock\""));
            assert!(json.contains("\"Hello desktop\""));
            assert!(json.contains("\"click_through\": true"));
        }
        other => panic!("expected renderer payload json, got {other:?}"),
    }
}

#[test]
fn open_dry_run_reports_selected_desktop_window_without_starting_gui() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    run([
        "widgex",
        "init",
        "--template",
        "desktop-clock",
        "--config",
        config_path.to_str().unwrap(),
    ])
    .unwrap();

    let output = run([
        "widgex",
        "open",
        "--dry-run",
        "--config",
        config_path.to_str().unwrap(),
    ])
    .unwrap();

    assert_eq!(
        output,
        CliOutput::Message(
            "would open desktop window desktop-clock (220x72): 12:34:56".to_string()
        )
    );
}

#[test]
fn open_toggle_dry_run_reports_daemon_request_shape() {
    let output = run(["widgex", "open", "--toggle", "--dry-run", "desktop-clock"]).unwrap();

    assert_eq!(
        output,
        CliOutput::Message("would send toggle desktop-clock to daemon".to_string())
    );
}

#[test]
fn daemon_start_dry_run_reports_binary_and_socket() {
    let output = run(["widgex", "daemon", "start", "--dry-run"]).unwrap();

    match output {
        CliOutput::Message(message) => {
            assert!(message.contains("would start widgexd"));
            assert!(message.contains("widgex.sock"));
        }
        other => panic!("expected daemon dry-run message, got {other:?}"),
    }
}

#[test]
fn daemon_reload_dry_run_reports_socket() {
    let output = run(["widgex", "daemon", "reload", "--dry-run"]).unwrap();

    match output {
        CliOutput::Message(message) => {
            assert!(message.contains("would reload daemon"));
            assert!(message.contains("widgex.sock"));
        }
        other => panic!("expected daemon reload dry-run message, got {other:?}"),
    }
}

#[test]
fn doctor_reports_arch_wayland_facts_without_requiring_a_display() {
    let output = run(["widgex", "doctor"]).unwrap();

    match output {
        CliOutput::Message(message) => {
            assert!(message.contains("Widgex doctor"));
            assert!(message.contains("session"));
        }
        other => panic!("expected doctor message, got {other:?}"),
    }
}
