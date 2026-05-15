//! Data source engine: turns config [`DataSource`] entries into live
//! [`SourceSnapshot`] readings the renderer can bind against.
//!
//! This milestone implements the `time` and `battery` source kinds. The other
//! kinds (`cpu`, `memory`, `network`, `shell`) parse and validate but produce
//! empty snapshots until a later milestone.

use std::{collections::BTreeMap, fs, path::Path, time::Duration};

use chrono::Local;
use widgex_core::{DataSource, SourceKind, SourceSnapshot};

/// Where the Linux kernel exposes battery state.
const POWER_SUPPLY_BASE: &str = "/sys/class/power_supply";

/// Fallback poll cadence when no source declares an `interval_ms`.
const DEFAULT_INTERVAL_MS: u64 = 1000;

/// Poll every source once, returning one snapshot per source.
pub fn poll_all(sources: &[DataSource]) -> Vec<SourceSnapshot> {
    sources.iter().map(poll_source).collect()
}

/// Poll a single source for its current reading.
pub fn poll_source(source: &DataSource) -> SourceSnapshot {
    let mut snapshot = SourceSnapshot::new(&source.id);
    snapshot.fields = match source.kind {
        SourceKind::Time => time_fields(),
        SourceKind::Battery => read_battery(Path::new(POWER_SUPPLY_BASE)),
        SourceKind::Cpu | SourceKind::Memory | SourceKind::Network | SourceKind::Shell => {
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
        .filter_map(|source| source.interval_ms)
        .filter(|millis| *millis > 0)
        .min()
        .unwrap_or(DEFAULT_INTERVAL_MS);
    Duration::from_millis(millis)
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
                interval_ms: Some(1000),
                timeout_ms: None,
                command: None,
            },
            DataSource {
                id: "battery".into(),
                kind: SourceKind::Battery,
                interval_ms: Some(5000),
                timeout_ms: None,
                command: None,
            },
        ];

        assert_eq!(tick_interval(&sources), Duration::from_millis(1000));
        assert_eq!(
            tick_interval(&[]),
            Duration::from_millis(DEFAULT_INTERVAL_MS)
        );
    }
}
