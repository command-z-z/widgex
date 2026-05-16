use widgex_core::{Action, load_validated_config, renderer_payload_from_config};
use widgex_webview::{
    WindowPreview, build_window_preview, execute_action, handle_widget_event, inline_theme_css,
};

#[test]
fn builds_preview_for_selected_desktop_clock_window() {
    let config = load_validated_config("../../examples/desktop-clock/config.toml").unwrap();
    let payload = renderer_payload_from_config(&config).unwrap();

    let preview = build_window_preview(&payload, Some("desktop-clock")).unwrap();

    assert_eq!(
        preview,
        WindowPreview {
            id: "desktop-clock".to_string(),
            title: Some("Desktop Clock".to_string()),
            width: 220,
            height: 72,
            text_preview: "12:34:56".to_string(),
        }
    );
}

#[test]
fn preview_errors_for_unknown_window_id() {
    let config = load_validated_config("../../examples/desktop-clock/config.toml").unwrap();
    let payload = renderer_payload_from_config(&config).unwrap();

    assert!(build_window_preview(&payload, Some("does-not-exist")).is_err());
}

#[test]
fn command_actions_require_shell_permission() {
    let action = Action::Command {
        command: "true".to_string(),
    };

    let error = execute_action(&action, None, std::path::Path::new("."), false)
        .expect_err("shell command should be rejected without permission");

    assert!(error.to_string().contains("permissions.allow_shell"));
}

#[test]
fn widget_event_substitutes_change_value() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("value.txt");
    let body = format!(
        r#"{{"action":{{"type":"command","command":"printf {{}} > {}"}},"value":"42"}}"#,
        output.display()
    );

    handle_widget_event(&body, dir.path(), true).expect("event should execute");

    for _ in 0..20 {
        if output.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    assert_eq!(std::fs::read_to_string(output).unwrap(), "42");
}

#[test]
fn inline_theme_css_concatenates_multiple_css_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("widgets/dashboard")).unwrap();
    std::fs::write(dir.path().join("base.css"), "body { color: white; }").unwrap();
    std::fs::write(
        dir.path().join("widgets/dashboard/dashboard.css"),
        ".dashboard { color: green; }",
    )
    .unwrap();
    let mut payload = widgex_core::RendererPayload {
        version: 1,
        theme_css: Some("base.css".to_string()),
        theme_css_files: vec![
            "base.css".to_string(),
            "widgets/dashboard/dashboard.css".to_string(),
        ],
        windows: Vec::new(),
        sources: Vec::new(),
    };

    inline_theme_css(&mut payload, dir.path());

    assert_eq!(
        payload.theme_css.as_deref(),
        Some("body { color: white; }\n.dashboard { color: green; }")
    );
    assert!(payload.theme_css_files.is_empty());
}
