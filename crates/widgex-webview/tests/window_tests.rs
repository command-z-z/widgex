use widgex_core::{load_validated_config, renderer_payload_from_config};
use widgex_webview::{WindowPreview, build_window_preview};

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
