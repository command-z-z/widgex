//! Data source engine: turns config [`DataSource`] entries into live
//! [`SourceSnapshot`] readings the renderer can bind against.
//!
//! This milestone implements the `time`, `battery`, and shell command source
//! kinds. Other system sources parse and validate but produce empty snapshots
//! until a later milestone.

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
        SourceKind::Cpu | SourceKind::Memory | SourceKind::Network => BTreeMap::new(),
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
    match format {
        SourceFormat::Text => BTreeMap::from([("value".to_string(), output.trim().to_string())]),
        SourceFormat::Json => parse_json_fields(output),
    }
}

fn poll_shell(source: &DataSource, cwd: &Path) -> BTreeMap<String, String> {
    let Some(command) = source.command.as_deref() else {
        return BTreeMap::new();
    };

    let Ok(output) = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
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
            },
            DataSource {
                id: "battery".into(),
                kind: SourceKind::Battery,
                mode: SourceMode::Poll,
                format: SourceFormat::Text,
                interval_ms: Some(5000),
                timeout_ms: None,
                command: None,
            },
            DataSource {
                id: "metadata".into(),
                kind: SourceKind::Shell,
                mode: SourceMode::Listen,
                format: SourceFormat::Json,
                interval_ms: Some(10),
                timeout_ms: None,
                command: Some("printf '{}'".into()),
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
        };

        let snapshot = poll_source_with_dir(&source, dir.path());

        assert_eq!(
            snapshot.fields.get("value").map(String::as_str),
            Some("hello")
        );
    }
}
