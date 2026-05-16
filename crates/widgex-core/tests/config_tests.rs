use std::fs;

use widgex_core::{
    Action, Binding, Config, ConfigDiagnostic, SourceFormat, SourceKind, SourceMode,
    SourceSnapshot, WidgetKind, load_validated_config, parse_config_str,
    renderer_payload_from_config, resolve_payload, resolve_template, schema_json_pretty,
    validate_config,
};

const MINIMAL_CONFIG: &str = r##"
version = 1

[theme]
css = "style.css"

[[sources]]
id = "clock"
kind = "time"
interval_ms = 1000

[[windows]]
id = "top-bar"
layer = "top"
anchor = ["top", "left", "right"]
exclusive_zone = 32

[windows.size]
height = 32

[[windows.widgets]]
type = "box"
direction = "row"

[[windows.widgets.children]]
type = "label"
text = "{{ clock.now }}"
"##;

#[test]
fn parses_minimal_top_bar_config() {
    let config = parse_config_str(MINIMAL_CONFIG).expect("config should parse");

    assert_eq!(config.version, 1);
    assert_eq!(
        config.theme.as_ref().unwrap().css.as_deref(),
        Some("style.css")
    );
    assert_eq!(config.sources[0].kind, SourceKind::Time);
    assert_eq!(config.windows[0].id, "top-bar");
    assert_eq!(config.windows[0].anchor.len(), 3);
    assert_eq!(config.windows[0].widgets[0].kind, WidgetKind::Box);
    assert_eq!(
        config.windows[0].widgets[0].children[0].text.as_deref(),
        Some("{{ clock.now }}")
    );
}

#[test]
fn reports_duplicate_window_ids_with_actionable_help() {
    let config = parse_config_str(
        r#"
version = 1

[[windows]]
id = "bar"

[windows.size]
height = 24

[[windows.widgets]]
type = "label"
text = "one"

[[windows]]
id = "bar"

[windows.size]
height = 24

[[windows.widgets]]
type = "label"
text = "two"
"#,
    )
    .expect("config syntax should parse");

    let diagnostics = validate_config(&config).expect_err("duplicate ids should fail");

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.path == "windows[1].id"
            && diagnostic.message.contains("duplicate window id")
            && diagnostic.help.contains("unique")
    }));
}

#[test]
fn rejects_shell_sources_unless_permission_is_enabled() {
    let config = parse_config_str(
        r#"
version = 1

[[sources]]
id = "uptime"
kind = "shell"
command = "uptime"

[[windows]]
id = "clock"

[windows.size]
height = 24

[[windows.widgets]]
type = "label"
text = "{{ uptime.stdout }}"
"#,
    )
    .expect("config syntax should parse");

    let diagnostics = validate_config(&config).expect_err("shell source should be rejected");

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.path == "sources[0]"
            && diagnostic
                .message
                .contains("shell source requires permissions.allow_shell")
    }));
}

#[test]
fn rejects_command_actions_unless_permission_is_enabled() {
    let config = parse_config_str(
        r#"
version = 1

[[windows]]
id = "controls"

[[windows.widgets]]
type = "button"
text = "Play"

[windows.widgets.on_click]
type = "command"
command = "playerctl play-pause"
"#,
    )
    .expect("config syntax should parse");

    let diagnostics = validate_config(&config).expect_err("command action should be rejected");

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.path == "windows[0].widgets[0].on_click"
            && diagnostic
                .message
                .contains("command action requires permissions.allow_shell")
    }));
}

#[test]
fn extracts_binding_references_from_template_text() {
    let binding = Binding::parse("CPU {{ cpu.percent }} MEM {{ memory.used }}").unwrap();

    assert_eq!(binding.references, vec!["cpu.percent", "memory.used"]);
}

#[test]
fn schema_contains_public_config_sections() {
    let schema = schema_json_pretty::<Config>().expect("schema should serialize");

    assert!(schema.contains("\"windows\""));
    assert!(schema.contains("\"sources\""));
    assert!(schema.contains("\"permissions\""));
    assert!(schema.contains("\"modules\""));
}

#[test]
fn diagnostic_display_includes_path_message_and_help() {
    let diagnostic = ConfigDiagnostic::new(
        "windows[0].widgets",
        "window must define at least one widget",
        "add a [[windows.widgets]] entry",
    );

    assert_eq!(
        diagnostic.to_string(),
        "windows[0].widgets: window must define at least one widget (help: add a [[windows.widgets]] entry)"
    );
}

#[test]
fn resolve_template_substitutes_known_references_and_blanks_unknown() {
    let snapshots = vec![
        SourceSnapshot::new("clock").with_field("now", "12:34:56"),
        SourceSnapshot::new("battery").with_field("percent", "84"),
    ];

    assert_eq!(
        resolve_template("{{ clock.now }} · {{ battery.percent }}%", &snapshots),
        "12:34:56 · 84%"
    );
    assert_eq!(resolve_template("{{ battery.missing }}", &snapshots), "");
    assert_eq!(
        resolve_template("no bindings here", &snapshots),
        "no bindings here"
    );
}

#[test]
fn resolve_payload_rewrites_widget_text_in_place() {
    let config = parse_config_str(MINIMAL_CONFIG).expect("config should parse");
    let payload = renderer_payload_from_config(&config).expect("config should validate");
    let snapshots = vec![SourceSnapshot::new("clock").with_field("now", "09:00:00")];

    let resolved = resolve_payload(&payload, &snapshots);

    assert_eq!(
        resolved.windows[0].widgets[0].children[0].text.as_deref(),
        Some("09:00:00")
    );
}

#[test]
fn resolves_src_and_style_bindings_in_payload() {
    let config = parse_config_str(
        r##"
version = 1

[[sources]]
id = "metadata"
kind = "shell"
mode = "listen"
format = "json"
command = "printf '{}'"

[permissions]
allow_shell = true

[[windows]]
id = "music"

[[windows.widgets]]
type = "box"
style = "background-image:url('{{ metadata.image }}')"

[[windows.widgets.children]]
type = "image"
src = "{{ metadata.image }}"
"##,
    )
    .expect("config syntax should parse");

    let payload = renderer_payload_from_config(&config).expect("config should validate");
    let snapshot = SourceSnapshot::new("metadata").with_field("image", "./cover.png");
    let resolved = resolve_payload(&payload, &[snapshot]);

    assert_eq!(payload.sources[0].mode, SourceMode::Listen);
    assert_eq!(payload.sources[0].format, SourceFormat::Json);
    assert_eq!(
        resolved.windows[0].widgets[0].style.as_deref(),
        Some("background-image:url('./cover.png')")
    );
    assert_eq!(
        resolved.windows[0].widgets[0].children[0].src.as_deref(),
        Some("./cover.png")
    );
}

#[test]
fn renderer_payload_carries_widget_actions() {
    let config = parse_config_str(
        r#"
version = 1

[permissions]
allow_shell = true

[[windows]]
id = "controls"

[[windows.widgets]]
type = "progress"
value = "20"

[windows.widgets.on_change]
type = "command"
command = "./seek.sh {}"
"#,
    )
    .expect("config syntax should parse");

    let payload = renderer_payload_from_config(&config).expect("config should validate");

    assert_eq!(
        payload.windows[0].widgets[0].on_change,
        Some(Action::Command {
            command: "./seek.sh {}".to_string()
        })
    );
}

#[test]
fn reports_binding_reference_to_unknown_source() {
    let config = parse_config_str(
        r#"
version = 1

[[windows]]
id = "bar"

[windows.size]
height = 24

[[windows.widgets]]
type = "label"
text = "{{ ghost.value }}"
"#,
    )
    .expect("config syntax should parse");

    let diagnostics = validate_config(&config).expect_err("unknown source should fail");

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.message.contains("unknown source \"ghost\"") })
    );
}

#[test]
fn builds_renderer_payload_from_desktop_clock_config() {
    let config = parse_config_str(
        r##"
version = 1

[theme]
css = "style.css"

[[sources]]
id = "clock"
kind = "time"
interval_ms = 1000

[[windows]]
id = "desktop-clock"
title = "Desktop Clock"
layer = "top"
anchor = ["top", "right"]
click_through = true

[windows.margin]
top = 24
right = 24

[windows.size]
width = 220
height = 72

[[windows.widgets]]
type = "box"
direction = "column"

[[windows.widgets.children]]
type = "label"
class = ["clock-time"]
text = "{{ clock.now }}"
"##,
    )
    .expect("config syntax should parse");

    let payload = renderer_payload_from_config(&config).expect("config should validate");

    assert_eq!(payload.version, 1);
    assert_eq!(payload.theme_css.as_deref(), Some("style.css"));
    assert_eq!(payload.windows[0].id, "desktop-clock");
    assert!(payload.windows[0].click_through);
    assert_eq!(
        payload.windows[0].widgets[0].children[0]
            .bindings
            .as_ref()
            .unwrap()
            .text,
        vec!["clock.now"]
    );
}

#[test]
fn load_validated_config_merges_explicit_modules_and_css_files() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("widgets/dashboard")).unwrap();
    fs::create_dir_all(dir.path().join("shared")).unwrap();
    fs::write(
        dir.path().join("config.toml"),
        r#"
version = 1
modules = [
  "shared/lyrics.toml",
  "widgets/dashboard/dashboard.toml",
]

[theme]
css = "styles/base.css"
"#,
    )
    .unwrap();
    fs::write(
        dir.path().join("shared/lyrics.toml"),
        r#"
[[sources]]
id = "lyrics"
kind = "time"
"#,
    )
    .unwrap();
    fs::write(
        dir.path().join("widgets/dashboard/dashboard.toml"),
        r#"
css = "dashboard.css"

[[windows]]
id = "dashboard"

[[windows.widgets]]
type = "label"
text = "{{ lyrics.now }}"
"#,
    )
    .unwrap();

    let config = load_validated_config(dir.path().join("config.toml")).unwrap();
    let payload = renderer_payload_from_config(&config).unwrap();

    assert_eq!(config.sources[0].id, "lyrics");
    assert_eq!(config.windows[0].id, "dashboard");
    assert_eq!(
        payload.theme_css_files,
        vec!["styles/base.css", "widgets/dashboard/dashboard.css"]
    );
}

#[test]
fn load_validated_config_reports_missing_module_path() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("config.toml"),
        r#"
version = 1
modules = ["widgets/missing.toml"]
"#,
    )
    .unwrap();

    let diagnostics = load_validated_config(dir.path().join("config.toml")).unwrap_err();

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.path == "widgets/missing.toml"
            && diagnostic.message.contains("failed to read module")
    }));
}
