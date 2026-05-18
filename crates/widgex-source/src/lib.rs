//! Data source engine: turns config [`DataSource`] entries into live
//! [`SourceSnapshot`] readings the renderer can bind against.
//!
//! This milestone implements the `time`, `battery`, shell command, and
//! unix-socket listen source kinds. Other system sources parse and validate but
//! produce empty snapshots until a later milestone.

use std::{collections::BTreeMap, fs, path::Path, process::Command, time::Duration};

use chrono::Local;
use widgex_core::{DataSource, SourceFormat, SourceKind, SourceMode, SourceSnapshot};

/// Where the Linux kernel exposes battery state.
const POWER_SUPPLY_BASE: &str = "/sys/class/power_supply";

/// Fallback poll cadence when no source declares an `interval_ms`.
const DEFAULT_INTERVAL_MS: u64 = 1000;

/// Poll every source once, returning one snapshot per source.
pub fn poll_all(sources: &[DataSource]) -> Vec<SourceSnapshot> {
    poll_all_with_dir(sources, Path::new("."))
}

/// Poll every non-listen source with shell commands resolved relative to
/// `cwd`.
pub fn poll_all_with_dir(sources: &[DataSource], cwd: &Path) -> Vec<SourceSnapshot> {
    sources
        .iter()
        .filter(|source| source.mode == SourceMode::Poll)
        .map(|source| poll_source_with_dir(source, cwd))
        .collect()
}

/// Poll a single source for its current reading.
pub fn poll_source(source: &DataSource) -> SourceSnapshot {
    poll_source_with_dir(source, Path::new("."))
}

/// Poll a single source with shell commands resolved relative to `cwd`.
pub fn poll_source_with_dir(source: &DataSource, cwd: &Path) -> SourceSnapshot {
    let mut snapshot = SourceSnapshot::new(&source.id);
    snapshot.fields = match source.kind {
        SourceKind::Time => time_fields(),
        SourceKind::Battery => read_battery(Path::new(POWER_SUPPLY_BASE)),
        SourceKind::Shell => poll_shell(source, cwd),
        SourceKind::Cpu | SourceKind::Memory | SourceKind::Network | SourceKind::UnixSocket => {
            BTreeMap::new()
        }
    };
    snapshot
}

/// The cadence the renderer should poll at: the smallest positive
/// `interval_ms` across all sources, or [`DEFAULT_INTERVAL_MS`].
pub fn tick_interval(sources: &[DataSource]) -> Duration {
    let millis = sources
        .iter()
        .filter(|source| source.mode == SourceMode::Poll)
        .filter_map(|source| source.interval_ms)
        .filter(|millis| *millis > 0)
        .min()
        .unwrap_or(DEFAULT_INTERVAL_MS);
    Duration::from_millis(millis)
}

/// Parse one shell command output into fields for a source snapshot.
pub fn parse_shell_output(format: SourceFormat, output: &str) -> BTreeMap<String, String> {
    parse_source_output(format, output)
}

/// Parse one source output line into snapshot fields.
pub fn parse_source_output(format: SourceFormat, output: &str) -> BTreeMap<String, String> {
    match format {
        SourceFormat::Text => BTreeMap::from([("value".to_string(), output.trim().to_string())]),
        SourceFormat::Json => parse_json_fields(output),
        SourceFormat::HyprlandEvent => parse_hyprland_event_line(output),
    }
}

/// Parse one Hyprland socket2 event line.
pub fn parse_hyprland_event_line(line: &str) -> BTreeMap<String, String> {
    let raw = line.trim();
    let mut fields = BTreeMap::from([("raw".to_string(), raw.to_string())]);
    let Some((event, payload)) = raw.split_once(">>") else {
        return fields;
    };

    fields.insert("event".to_string(), event.to_string());
    fields.insert("payload".to_string(), payload.to_string());

    match event {
        "workspacev2" | "createworkspacev2" | "destroyworkspacev2" => {
            let (id, name) = split_first_payload(payload);
            fields.insert("workspace_id".to_string(), id.to_string());
            fields.insert("workspace_name".to_string(), name.to_string());
            append_workspace_styles(&mut fields, id);
        }
        "workspace" | "createworkspace" | "destroyworkspace" | "renameworkspace" => {
            fields.insert("workspace_name".to_string(), payload.to_string());
        }
        "activewindowv2" | "windowtitlev2" | "openwindow" | "closewindow" | "movewindowv2"
        | "urgent" => {
            let (address, rest) = split_first_payload(payload);
            fields.insert("window_address".to_string(), address.to_string());
            if !rest.is_empty() {
                fields.insert("window_payload".to_string(), rest.to_string());
            }
        }
        "activewindow" => {
            let (class, title) = split_first_payload(payload);
            fields.insert("window_class".to_string(), class.to_string());
            fields.insert("window_title".to_string(), title.to_string());
        }
        _ => {}
    }

    fields
}

fn split_first_payload(payload: &str) -> (&str, &str) {
    payload
        .split_once(',')
        .map(|(first, rest)| (first.trim(), rest.trim()))
        .unwrap_or((payload.trim(), ""))
}

fn append_workspace_styles(fields: &mut BTreeMap<String, String>, active_id: &str) {
    for index in 1..=10 {
        let is_active = active_id.parse::<u8>().ok() == Some(index);
        let style = if is_active {
            active_workspace_style()
        } else {
            inactive_workspace_style()
        };
        fields.insert(format!("workspace{index}_style"), style.to_string());
    }
}

fn active_workspace_style() -> &'static str {
    "color: var(--ctp-mantle); background: var(--ctp-green); border-color: color-mix(in srgb, var(--ctp-green) 85%, transparent); box-shadow: 0 0 0 1px color-mix(in srgb, var(--ctp-green) 16%, transparent)"
}

fn inactive_workspace_style() -> &'static str {
    "color: var(--ctp-peach); background: var(--ctp-surface0); border-color: var(--ctp-surface1); box-shadow: none"
}

fn poll_shell(source: &DataSource, cwd: &Path) -> BTreeMap<String, String> {
    let Some(command) = source.command.as_deref() else {
        return BTreeMap::new();
    };

    let effective_cwd: &Path = source.working_dir.as_deref().unwrap_or(cwd);
    let Ok(output) = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(effective_cwd)
        .output()
    else {
        return BTreeMap::new();
    };

    if !output.status.success() {
        return BTreeMap::new();
    }

    parse_shell_output(
        source.format,
        String::from_utf8_lossy(&output.stdout).as_ref(),
    )
}

fn parse_json_fields(output: &str) -> BTreeMap<String, String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(output.trim()) else {
        return BTreeMap::new();
    };

    let Some(object) = value.as_object() else {
        return BTreeMap::new();
    };

    object
        .iter()
        .map(|(key, value)| {
            let value = value
                .as_str()
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| value.to_string());
            (key.clone(), value)
        })
        .collect()
}

fn time_fields() -> BTreeMap<String, String> {
    let now = Local::now();
    BTreeMap::from([
        ("now".to_string(), now.format("%H:%M:%S").to_string()),
        ("date".to_string(), now.format("%Y-%m-%d").to_string()),
    ])
}

/// Read the first battery under `base` (`<dir>/type` == `Battery`), exposing
/// `percent` (numeric `capacity`, laptop batteries), `level` (textual
/// `capacity_level`, available on peripherals too), and `status`. Returns an
/// empty map if no battery exists, so a desktop without one degrades gracefully.
fn read_battery(base: &Path) -> BTreeMap<String, String> {
    let Some(battery_dir) = first_battery_dir(base) else {
        return BTreeMap::new();
    };

    let mut fields = BTreeMap::new();
    for (field, file) in [
        ("percent", "capacity"),
        ("level", "capacity_level"),
        ("status", "status"),
    ] {
        if let Some(value) = read_sys_value(&battery_dir.join(file)) {
            fields.insert(field.to_string(), value);
        }
    }
    fields
}

fn first_battery_dir(base: &Path) -> Option<std::path::PathBuf> {
    let mut entries: Vec<_> = fs::read_dir(base)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    entries.sort();

    entries
        .into_iter()
        .find(|dir| read_sys_value(&dir.join("type")).as_deref() == Some("Battery"))
}

fn read_sys_value(path: &Path) -> Option<String> {
    let value = fs::read_to_string(path).ok()?;
    Some(value.trim().to_string())
}

/// Query `hyprctl activeworkspace -j` once and return a seed snapshot for every
/// `unix_socket` + `HyprlandEvent` listen source in `sources`.
///
/// Returns an empty `Vec` when `hyprctl` is absent or fails so callers need no
/// error handling. This seeds `workspace_name`, `workspace_id`, and all
/// `workspace{n}_style` fields before the live socket delivers its first event.
pub fn seed_listen_snapshots(sources: &[DataSource]) -> Vec<SourceSnapshot> {
    let hyprland_sources: Vec<&DataSource> = sources
        .iter()
        .filter(|s| s.mode == SourceMode::Listen && s.format == SourceFormat::HyprlandEvent)
        .collect();

    if hyprland_sources.is_empty() {
        return vec![];
    }

    let Ok(output) = Command::new("hyprctl")
        .args(["activeworkspace", "-j"])
        .output()
    else {
        return vec![];
    };

    if !output.status.success() {
        return vec![];
    }

    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return vec![];
    };

    let Some(id) = json.get("id").and_then(|v| v.as_i64()) else {
        return vec![];
    };

    let name = json
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let name = if name.is_empty() { id.to_string() } else { name };

    let fields =
        parse_source_output(SourceFormat::HyprlandEvent, &format!("workspacev2>>{id},{name}"));

    hyprland_sources
        .into_iter()
        .map(|source| {
            let mut snapshot = SourceSnapshot::new(&source.id);
            snapshot.fields = fields.clone();
            snapshot
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn reads_battery_fields_from_sysfs_layout() {
        let base = tempfile::tempdir().unwrap();
        let bat = base.path().join("BAT0");
        write(&bat.join("type"), "Battery\n");
        write(&bat.join("capacity"), "84\n");
        write(&bat.join("capacity_level"), "Normal\n");
        write(&bat.join("status"), "Discharging\n");
        // A non-battery supply that must be ignored.
        write(&base.path().join("AC/type"), "Mains\n");

        let fields = read_battery(base.path());

        assert_eq!(fields.get("percent").map(String::as_str), Some("84"));
        assert_eq!(fields.get("level").map(String::as_str), Some("Normal"));
        assert_eq!(
            fields.get("status").map(String::as_str),
            Some("Discharging")
        );
    }

    #[test]
    fn reads_peripheral_battery_without_numeric_capacity() {
        let base = tempfile::tempdir().unwrap();
        let bat = base.path().join("hidpp_battery_0");
        write(&bat.join("type"), "Battery\n");
        write(&bat.join("capacity_level"), "Normal\n");
        write(&bat.join("status"), "Discharging\n");

        let fields = read_battery(base.path());

        assert_eq!(fields.get("percent"), None);
        assert_eq!(fields.get("level").map(String::as_str), Some("Normal"));
        assert_eq!(
            fields.get("status").map(String::as_str),
            Some("Discharging")
        );
    }

    #[test]
    fn battery_read_is_empty_when_no_battery_present() {
        let base = tempfile::tempdir().unwrap();
        write(&base.path().join("AC/type"), "Mains\n");

        assert!(read_battery(base.path()).is_empty());
    }

    #[test]
    fn tick_interval_picks_smallest_positive_interval() {
        let sources = vec![
            DataSource {
                id: "clock".into(),
                kind: SourceKind::Time,
                mode: SourceMode::Poll,
                format: SourceFormat::Text,
                interval_ms: Some(1000),
                timeout_ms: None,
                command: None,
                path: None,
                working_dir: None,
            },
            DataSource {
                id: "battery".into(),
                kind: SourceKind::Battery,
                mode: SourceMode::Poll,
                format: SourceFormat::Text,
                interval_ms: Some(5000),
                timeout_ms: None,
                command: None,
                path: None,
                working_dir: None,
            },
            DataSource {
                id: "metadata".into(),
                kind: SourceKind::Shell,
                mode: SourceMode::Listen,
                format: SourceFormat::Json,
                interval_ms: Some(10),
                timeout_ms: None,
                command: Some("printf '{}'".into()),
                path: None,
                working_dir: None,
            },
        ];

        assert_eq!(tick_interval(&sources), Duration::from_millis(1000));
        assert_eq!(
            tick_interval(&[]),
            Duration::from_millis(DEFAULT_INTERVAL_MS)
        );
    }

    #[test]
    fn parses_text_shell_output_as_value_field() {
        let fields = parse_shell_output(SourceFormat::Text, "Playing\n");

        assert_eq!(fields.get("value").map(String::as_str), Some("Playing"));
    }

    #[test]
    fn parses_json_shell_output_as_named_fields() {
        let fields = parse_shell_output(
            SourceFormat::Json,
            r#"{"title":"Song","progress":42,"playing":true}"#,
        );

        assert_eq!(fields.get("title").map(String::as_str), Some("Song"));
        assert_eq!(fields.get("progress").map(String::as_str), Some("42"));
        assert_eq!(fields.get("playing").map(String::as_str), Some("true"));
    }

    #[test]
    fn polls_shell_command_relative_to_config_dir() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("value.txt"), "hello\n");
        let source = DataSource {
            id: "local".into(),
            kind: SourceKind::Shell,
            mode: SourceMode::Poll,
            format: SourceFormat::Text,
            interval_ms: None,
            timeout_ms: None,
            command: Some("cat value.txt".into()),
            path: None,
            working_dir: None,
        };

        let snapshot = poll_source_with_dir(&source, dir.path());

        assert_eq!(
            snapshot.fields.get("value").map(String::as_str),
            Some("hello")
        );
    }

    #[test]
    fn parses_hyprland_workspace_event() {
        let fields = parse_hyprland_event_line("workspacev2>>2,web\n");

        assert_eq!(
            fields.get("raw").map(String::as_str),
            Some("workspacev2>>2,web")
        );
        assert_eq!(fields.get("event").map(String::as_str), Some("workspacev2"));
        assert_eq!(fields.get("payload").map(String::as_str), Some("2,web"));
        assert_eq!(fields.get("workspace_id").map(String::as_str), Some("2"));
        assert_eq!(
            fields.get("workspace_name").map(String::as_str),
            Some("web")
        );
        assert!(
            fields
                .get("workspace2_style")
                .is_some_and(|style| style.contains("background: var(--ctp-green)"))
        );
        assert!(
            fields
                .get("workspace1_style")
                .is_some_and(|style| style.contains("background: var(--ctp-surface0)"))
        );
    }

    #[test]
    fn parses_hyprland_active_window_event() {
        let fields = parse_hyprland_event_line("activewindowv2>>0xabc123");

        assert_eq!(
            fields.get("event").map(String::as_str),
            Some("activewindowv2")
        );
        assert_eq!(
            fields.get("window_address").map(String::as_str),
            Some("0xabc123")
        );
    }

    #[test]
    fn malformed_hyprland_event_keeps_raw_line() {
        let fields = parse_hyprland_event_line("not an event");

        assert_eq!(fields.get("raw").map(String::as_str), Some("not an event"));
        assert!(!fields.contains_key("event"));
    }

    #[test]
    fn seed_listen_snapshots_is_empty_with_no_hyprland_sources() {
        let sources = vec![DataSource {
            id: "clock".into(),
            kind: SourceKind::Time,
            mode: SourceMode::Poll,
            format: SourceFormat::Text,
            interval_ms: Some(1000),
            timeout_ms: None,
            command: None,
            path: None,
            working_dir: None,
        }];
        assert!(seed_listen_snapshots(&sources).is_empty());
    }

    #[test]
    fn seed_listen_snapshots_does_not_panic_without_hyprctl() {
        let sources = vec![DataSource {
            id: "hypr_events".into(),
            kind: SourceKind::UnixSocket,
            mode: SourceMode::Listen,
            format: SourceFormat::HyprlandEvent,
            interval_ms: Some(1000),
            timeout_ms: None,
            command: None,
            path: None,
            working_dir: None,
        }];
        // Returns empty (no hyprctl in CI) or valid snapshots (live Hyprland session).
        let snapshots = seed_listen_snapshots(&sources);
        for snap in &snapshots {
            assert_eq!(snap.id, "hypr_events");
            assert!(snap.fields.contains_key("workspace_id"));
        }
    }
}
