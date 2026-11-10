//! The on-disk formats of the usage history, shared with the C# app.
//!
//! Both files hold what .NET's `JavaScriptSerializer` writes for the C# app's
//! `List<UsageHistorySample>` and `ImportCompletion`, so either app can read
//! what the other saved. Dates are `"\/Date(ms)\/"` strings. The escaped
//! slashes matter: the C# deserializer only recognises a date when they are
//! present, which is why the files are written by hand instead of by
//! `serde_json`.
//!
//! Members are written in the C# declaration order. The C# app usually does
//! the same but can write them in another order, so reading never depends on
//! it.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use serde_json::{Map, Value};

use crate::history::{HistorySample, HistoryWindow};
use crate::localtime::{MAX_TIMESTAMP_MS, MIN_TIMESTAMP_MS, TICKS_PER_MS};

/// The most readings a history holds; older ones make way for new ones.
pub const MAX_SAMPLES: usize = 45_000;

const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const IMPORT_VERSION: &str = "3";
/// .NET ticks, counted from 0001-01-01, at the Unix epoch.
const UNIX_EPOCH_TICKS: i64 = 621_355_968_000_000_000;
const REPLACE_ATTEMPTS: u32 = 3;
const REPLACE_RETRY_DELAY: Duration = Duration::from_millis(25);

static TEMP_FILE_COUNTER: AtomicU32 = AtomicU32::new(0);

/// A day of imported readings that the completion marker expects the history to still hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoverageAnchor {
    /// The imported reading's timestamp in .NET ticks.
    pub timestamp_ticks: i64,
    pub weekly: bool,
}

/// Unix milliseconds as .NET ticks.
pub fn ticks_from_ms(ms: i64) -> i64 {
    ms.saturating_mul(TICKS_PER_MS)
        .saturating_add(UNIX_EPOCH_TICKS)
}

/// `path` with `suffix` appended to its file name.
pub fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Reads the history file. `None` means the file is missing, oversized or not
/// a history the C# app could load. Entries the C# app would skip (no
/// timestamp, no window, or an impossible window) are left out.
pub fn read_samples(path: &Path) -> Option<Vec<HistorySample>> {
    parse_samples(&read_text(path)?)
}

/// Saves the history through a temporary file so a crash cannot leave a
/// truncated history behind. Returns false when it could not be written.
pub fn write_samples(path: &Path, samples: &[HistorySample]) -> bool {
    let json = serialize_samples(samples);
    if json.len() as u64 > MAX_FILE_BYTES || !create_parent(path) {
        return false;
    }
    let unique = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = with_suffix(path, &format!(".{:x}-{unique:x}.tmp", std::process::id()));
    let saved = fs::write(&temporary, json).is_ok() && replace(&temporary, path);
    if !saved {
        // Nothing useful can be done if the leftover cannot be removed either.
        let _ = fs::remove_file(&temporary);
    }
    saved
}

/// Reads the import-completion marker. `None` means it is missing, damaged or
/// from an older version that did not record coverage.
pub fn read_completion(path: &Path) -> Option<Vec<CoverageAnchor>> {
    parse_completion(&read_text(path)?)
}

/// Writes the import-completion marker. A failed write is not reported: the
/// next launch then finds no marker and repeats the import.
pub fn write_completion(path: &Path, coverage: &[CoverageAnchor]) {
    if create_parent(path) {
        let _ = fs::write(path, serialize_completion(coverage));
    }
}

fn create_parent(path: &Path) -> bool {
    path.parent()
        .is_none_or(|parent| fs::create_dir_all(parent).is_ok())
}

fn replace(temporary: &Path, path: &Path) -> bool {
    for attempt in 1..=REPLACE_ATTEMPTS {
        if fs::rename(temporary, path).is_ok() {
            return true;
        }
        // Windows can briefly lock the file being replaced. Keep the write
        // atomic and retry shortly before giving up on this save.
        if attempt < REPLACE_ATTEMPTS {
            std::thread::sleep(REPLACE_RETRY_DELAY);
        }
    }
    false
}

fn read_text(path: &Path) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    Some(text.strip_prefix('\u{feff}').unwrap_or(&text).to_string())
}

enum WindowField {
    Absent,
    /// Present with values no reading can have; the C# app skips such samples.
    Invalid,
    Present(HistoryWindow),
}

fn parse_samples(json: &str) -> Option<Vec<HistorySample>> {
    let root: Value = serde_json::from_str(json).ok()?;
    let entries = root.as_array()?;
    if entries.len() > MAX_SAMPLES {
        return None;
    }
    let mut samples = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.is_null() {
            continue;
        }
        let entry = entry.as_object()?;
        let timestamp_ms = date_field(entry, "TimestampUtc")?;
        let weekly = window_field(entry, "Weekly")?;
        let five_hour = window_field(entry, "FiveHour")?;
        let (Some(timestamp_ms), Some(weekly), Some(five_hour)) =
            (timestamp_ms, usable(weekly), usable(five_hour))
        else {
            continue;
        };
        if weekly.is_some() || five_hour.is_some() {
            samples.push(HistorySample {
                timestamp_ms,
                weekly,
                five_hour,
            });
        }
    }
    Some(samples)
}

/// The window to keep, or `None` when its sample has to be skipped.
fn usable(field: WindowField) -> Option<Option<HistoryWindow>> {
    match field {
        WindowField::Absent => Some(None),
        WindowField::Invalid => None,
        WindowField::Present(window) => Some(Some(window)),
    }
}

/// The outer `None` means the value has the wrong type, which makes the C#
/// deserializer reject the whole file.
fn window_field(object: &Map<String, Value>, name: &str) -> Option<WindowField> {
    let window = match object.get(name) {
        None | Some(Value::Null) => return Some(WindowField::Absent),
        Some(value) => value.as_object()?,
    };
    let used_percent = match window.get("UsedPercent") {
        None => 0.0,
        Some(value) => value.as_f64()?,
    };
    let window_minutes = match window.get("WindowMinutes") {
        None | Some(Value::Null) => None,
        Some(value) => Some(i32::try_from(value.as_i64()?).ok()?),
    };
    let reset_at_ms = date_field(window, "ResetAtUtc")?;
    let flag = |name: &str| match window.get(name) {
        None | Some(Value::Null) => Some(false),
        Some(value) => value.as_bool(),
    };
    let cycle_average_unavailable = flag("CycleAverageUnavailable")?;
    let is_imported = flag("IsImported")?;
    if !(0.0..=100.0).contains(&used_percent) || window_minutes.is_some_and(|minutes| minutes <= 0)
    {
        return Some(WindowField::Invalid);
    }
    Some(WindowField::Present(HistoryWindow {
        used_percent,
        reset_at_ms,
        window_minutes: window_minutes.map(i32::unsigned_abs),
        cycle_average_unavailable,
        is_imported,
    }))
}

fn date_field(object: &Map<String, Value>, name: &str) -> Option<Option<i64>> {
    match object.get(name) {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(text)) => parse_date(text).map(Some),
        Some(_) => None,
    }
}

fn parse_date(text: &str) -> Option<i64> {
    let ms: i64 = text
        .strip_prefix("/Date(")?
        .strip_suffix(")/")?
        .parse()
        .ok()?;
    (MIN_TIMESTAMP_MS..=MAX_TIMESTAMP_MS)
        .contains(&ms)
        .then_some(ms)
}

fn serialize_samples(samples: &[HistorySample]) -> String {
    let mut json = String::with_capacity(samples.len() * 200 + 2);
    json.push('[');
    for (index, sample) in samples.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str("{\"TimestampUtc\":");
        push_date(&mut json, sample.timestamp_ms);
        json.push_str(",\"Weekly\":");
        push_window(&mut json, sample.weekly.as_ref());
        json.push_str(",\"FiveHour\":");
        push_window(&mut json, sample.five_hour.as_ref());
        json.push('}');
    }
    json.push(']');
    json
}

fn push_window(json: &mut String, window: Option<&HistoryWindow>) {
    let Some(window) = window else {
        json.push_str("null");
        return;
    };
    // Formatting into a String cannot fail.
    let _ = write!(
        json,
        "{{\"UsedPercent\":{},\"ResetAtUtc\":",
        window.used_percent
    );
    match window.reset_at_ms {
        Some(reset_at_ms) => push_date(json, reset_at_ms),
        None => json.push_str("null"),
    }
    json.push_str(",\"WindowMinutes\":");
    match window.window_minutes {
        Some(minutes) => {
            let _ = write!(json, "{minutes}");
        }
        None => json.push_str("null"),
    }
    let _ = write!(
        json,
        ",\"CycleAverageUnavailable\":{},\"IsImported\":{}}}",
        window.cycle_average_unavailable, window.is_imported
    );
}

fn push_date(json: &mut String, ms: i64) {
    let _ = write!(json, "\"\\/Date({ms})\\/\"");
}

fn parse_completion(json: &str) -> Option<Vec<CoverageAnchor>> {
    let root: Value = serde_json::from_str(json).ok()?;
    if root.get("Version")?.as_str()? != IMPORT_VERSION {
        return None;
    }
    root.get("Coverage")?
        .as_array()?
        .iter()
        .map(|anchor| {
            let anchor = anchor.as_object()?;
            Some(CoverageAnchor {
                timestamp_ticks: match anchor.get("TimestampUtcTicks") {
                    None => 0,
                    Some(value) => value.as_i64()?,
                },
                weekly: match anchor.get("Weekly") {
                    None => false,
                    Some(value) => value.as_bool()?,
                },
            })
        })
        .collect()
}

fn serialize_completion(coverage: &[CoverageAnchor]) -> String {
    let mut json = format!("{{\"Version\":\"{IMPORT_VERSION}\",\"Coverage\":[");
    for (index, anchor) in coverage.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        // Formatting into a String cannot fail.
        let _ = write!(
            json,
            "{{\"TimestampUtcTicks\":{},\"Weekly\":{}}}",
            anchor.timestamp_ticks, anchor.weekly
        );
    }
    json.push_str("]}");
    json
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(used_percent: f64, reset_at_ms: Option<i64>, minutes: Option<u32>) -> HistoryWindow {
        HistoryWindow {
            used_percent,
            reset_at_ms,
            window_minutes: minutes,
            cycle_average_unavailable: false,
            is_imported: false,
        }
    }

    #[test]
    fn samples_are_written_exactly_as_the_existing_app_writes_them() {
        // The first line is copied from a history file saved by the C# app.
        let saved = concat!(
            r#"[{"TimestampUtc":"\/Date(1791918125546)\/","Weekly":{"UsedPercent":21,"#,
            r#""ResetAtUtc":"\/Date(1792472317546)\/","WindowMinutes":10080,"#,
            r#""CycleAverageUnavailable":false,"IsImported":false},"FiveHour":null},"#,
            r#"{"TimestampUtc":"\/Date(1791918185569)\/","Weekly":null,"#,
            r#""FiveHour":{"UsedPercent":37.5,"ResetAtUtc":null,"WindowMinutes":null,"#,
            r#""CycleAverageUnavailable":true,"IsImported":true}}]"#
        );
        let samples = vec![
            HistorySample {
                timestamp_ms: 1_791_918_125_546,
                weekly: Some(window(21.0, Some(1_792_472_317_546), Some(10_080))),
                five_hour: None,
            },
            HistorySample {
                timestamp_ms: 1_791_918_185_569,
                weekly: None,
                five_hour: Some(HistoryWindow {
                    cycle_average_unavailable: true,
                    is_imported: true,
                    ..window(37.5, None, None)
                }),
            },
        ];
        assert_eq!(serialize_samples(&samples), saved);
        assert_eq!(parse_samples(saved), Some(samples));
        assert_eq!(serialize_samples(&[]), "[]");
        assert_eq!(parse_samples("[]"), Some(Vec::new()));
    }

    #[test]
    fn percentages_load_as_exactly_the_saved_value() {
        // The C# app writes some values with 17 digits. serde_json reads those
        // one bit off unless its `float_roundtrip` feature is enabled.
        for text in [
            "37.230000000000004",
            "71.490000000000009",
            "37.769999999999996",
            "0.30000000000000004",
        ] {
            let json = format!(
                r#"[{{"TimestampUtc":"\/Date(1000)\/","Weekly":{{"UsedPercent":{text}}}}}]"#
            );
            let samples = parse_samples(&json).unwrap();
            let loaded = samples[0].weekly.unwrap().used_percent;
            let exact: f64 = text.parse().unwrap();
            assert_eq!(loaded.to_bits(), exact.to_bits(), "{text}");
            assert_eq!(parse_samples(&serialize_samples(&samples)), Some(samples));
        }
        assert_eq!("37.230000000000004".parse(), Ok(7.23_f64 + 30.0));
    }

    #[test]
    fn members_are_read_in_any_order() {
        // Copied from a file saved by the C# app, whose serializer does not
        // always write members in declaration order.
        let json = concat!(
            r#"[{"Weekly":{"WindowMinutes":10080,"CycleAverageUnavailable":false,"#,
            r#""IsImported":true,"UsedPercent":32,"ResetAtUtc":null},"FiveHour":null,"#,
            r#""TimestampUtc":"\/Date(1793713771000)\/"}]"#
        );
        assert_eq!(
            parse_samples(json),
            Some(vec![HistorySample {
                timestamp_ms: 1_793_713_771_000,
                weekly: Some(HistoryWindow {
                    is_imported: true,
                    ..window(32.0, None, Some(10_080))
                }),
                five_hour: None,
            }])
        );
    }

    #[test]
    fn entries_the_existing_app_skips_are_left_out() {
        let json = r#"[
            null,
            {"Weekly":{"UsedPercent":10}},
            {"TimestampUtc":"\/Date(1000)\/","Weekly":null,"FiveHour":null},
            {"TimestampUtc":"\/Date(2000)\/","Weekly":{"UsedPercent":101}},
            {"TimestampUtc":"\/Date(3000)\/","Weekly":{"UsedPercent":-1}},
            {"TimestampUtc":"\/Date(4000)\/","Weekly":{"UsedPercent":10,"WindowMinutes":0}},
            {"TimestampUtc":"\/Date(5000)\/","Weekly":{"UsedPercent":10},
             "FiveHour":{"UsedPercent":20,"WindowMinutes":-5}},
            {"TimestampUtc":"\/Date(6000)\/","Weekly":{"WindowMinutes":10080},"Extra":1}
        ]"#;
        // Missing members take the C# defaults: zero percent, no reset, live reading.
        assert_eq!(
            parse_samples(json),
            Some(vec![HistorySample {
                timestamp_ms: 6_000,
                weekly: Some(window(0.0, None, Some(10_080))),
                five_hour: None,
            }])
        );
    }

    #[test]
    fn files_the_existing_app_cannot_load_are_rejected_whole() {
        for json in [
            "{invalid history",
            "null",
            r#"{"TimestampUtc":"\/Date(1000)\/"}"#,
            "[1]",
            r#"[{"TimestampUtc":1000,"Weekly":{"UsedPercent":10}}]"#,
            r#"[{"TimestampUtc":"2026-10-09T12:00:00Z","Weekly":{"UsedPercent":10}}]"#,
            r#"[{"TimestampUtc":"\/Date(999999999999999999)\/","Weekly":{"UsedPercent":10}}]"#,
            r#"[{"TimestampUtc":"\/Date(1000)\/","Weekly":{"UsedPercent":"10"}}]"#,
            r#"[{"TimestampUtc":"\/Date(1000)\/","Weekly":{"WindowMinutes":99999999999}}]"#,
            r#"[{"TimestampUtc":"\/Date(1000)\/","Weekly":{"IsImported":"yes"}}]"#,
            r#"[{"TimestampUtc":"\/Date(1000)\/","Weekly":[]}]"#,
        ] {
            assert_eq!(parse_samples(json), None, "{json}");
        }
        let oversized = format!("[{}null]", "null,".repeat(MAX_SAMPLES));
        assert_eq!(parse_samples(&oversized), None);
    }

    #[test]
    fn completion_markers_match_the_existing_format() {
        // Copied from a marker saved by the C# app.
        let saved = concat!(
            r#"{"Version":"3","Coverage":[{"TimestampUtcTicks":639244810575790000,"Weekly":true},"#,
            r#"{"TimestampUtcTicks":639245109554860000,"Weekly":false}]}"#
        );
        let coverage = vec![
            CoverageAnchor {
                timestamp_ticks: 639_244_810_575_790_000,
                weekly: true,
            },
            CoverageAnchor {
                timestamp_ticks: 639_245_109_554_860_000,
                weekly: false,
            },
        ];
        assert_eq!(serialize_completion(&coverage), saved);
        assert_eq!(parse_completion(saved), Some(coverage));
        assert_eq!(
            parse_completion(r#"{"Version":"3","Coverage":[]}"#),
            Some(Vec::new())
        );
        // Older markers were a bare version number without coverage.
        for json in [
            "1",
            "2",
            "{invalid completion",
            r#"{"Version":"2","Coverage":[]}"#,
            r#"{"Version":"3"}"#,
            r#"{"Version":"3","Coverage":[null]}"#,
            r#"{"Version":"3","Coverage":[{"TimestampUtcTicks":"1","Weekly":true}]}"#,
        ] {
            assert_eq!(parse_completion(json), None, "{json}");
        }
    }

    #[test]
    fn ticks_count_from_the_dotnet_epoch() {
        assert_eq!(ticks_from_ms(0), 621_355_968_000_000_000);
        assert_eq!(ticks_from_ms(MIN_TIMESTAMP_MS), 0);
        // 2026-09-08T16:17:37.579Z.
        assert_eq!(ticks_from_ms(1_788_884_257_579), 639_244_810_575_790_000);
    }

    #[test]
    fn paths_gain_a_suffix_after_the_extension() {
        assert_eq!(
            with_suffix(Path::new(r"C:\data\usage-history.json"), ".imported"),
            PathBuf::from(r"C:\data\usage-history.json.imported")
        );
    }
}
