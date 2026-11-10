//! Saved Claude quota readings, and the status-line capture that can supply
//! them.
//!
//! The account cache and the status-line cache share one format with the C#
//! app: the capture time in .NET ticks and, for each window that was valid,
//! its used percentage and reset time. Nothing else from a status line or an
//! account response is stored.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Map, Value};

use super::files::{self, TemporaryFile};
use crate::model::{LimitWindow, Usage, FIVE_HOUR_MINUTES, WEEKLY_MINUTES};
use crate::paths::{self, CLAUDE_STATUS_LINE_FILE};
use crate::timefmt::now_unix;

const MAX_INPUT_BYTES: usize = 65_536;
const MAX_CACHE_BYTES: usize = 4_096;
const FRESHNESS_SECONDS: i64 = 30 * 60;
const REPLACE_ATTEMPTS: u32 = 3;
const REPLACE_RETRY_DELAY: Duration = Duration::from_millis(25);

const TICKS_PER_SECOND: i64 = 10_000_000;
/// Seconds from the origin of .NET ticks, the start of year 1, to 1970.
const UNIX_EPOCH_SECONDS: i64 = 62_135_596_800;
/// The last instant a .NET `DateTime` can hold. The C# app rejects anything
/// later, so this app does too and the files stay readable by both.
const MAX_TICKS: i64 = 3_155_378_975_999_999_999;
const MAX_UNIX_SECONDS: i64 = MAX_TICKS / TICKS_PER_SECOND - UNIX_EPOCH_SECONDS;

const CAPTURED_KEY: &str = "captured_at_utc_ticks";
const FIVE_HOUR_KEY: &str = "five_hour";
const WEEKLY_KEY: &str = "seven_day";

/// The reading held by one cache file.
pub(super) struct Snapshot {
    /// When the reading was taken, in .NET ticks. Kept at full precision so
    /// two caches written within the same second still order correctly.
    pub(super) captured_ticks: i64,
    pub(super) usage: Usage,
    /// True once the reading is more than thirty minutes old.
    pub(super) stale: bool,
}

/// Where a cache file lives in this app's data directory. Empty when that
/// directory is unknown, which leaves the cache unread and unwritten.
pub(super) fn default_path(file_name: &str) -> PathBuf {
    paths::data_directory()
        .map(|directory| directory.join(file_name))
        .unwrap_or_default()
}

/// Runs the `--claude-statusline` mode: reads the JSON that Claude Code pipes
/// to a status-line command from standard input and saves its quota windows
/// for the tray. It prints nothing and ignores every failure, because a
/// status-line command must never interrupt Claude Code.
pub fn handle_status_line() {
    let path = default_path(CLAUDE_STATUS_LINE_FILE);
    capture_from(io::stdin().lock(), &path, now_unix());
}

fn capture_from(input: impl Read, path: &Path, now: i64) -> bool {
    let mut bytes = Vec::new();
    let read = input
        .take(MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes);
    if read.is_err() || bytes.len() > MAX_INPUT_BYTES {
        return false;
    }
    capture_status_line(&String::from_utf8_lossy(&bytes), path, now)
}

/// Saves the quota windows of a status-line payload to `path`, replacing the
/// previous reading. Returns false, leaving the file alone, when the payload
/// is unusable or the file cannot be written.
pub(super) fn capture_status_line(input: &str, path: &Path, now: i64) -> bool {
    if input.trim().is_empty() || input.len() > MAX_INPUT_BYTES || path.as_os_str().is_empty() {
        return false;
    }
    let Some(root) = parse_object(input.as_bytes()) else {
        return false;
    };
    // Startup status lines have no quota data until the first API response.
    // Keep the last actual reading and its original capture time.
    let Some(limits) = root.get("rate_limits").and_then(Value::as_object) else {
        return false;
    };
    let Some(captured_ticks) = ticks_from_unix(now) else {
        return false;
    };

    let mut cache = Map::new();
    cache.insert(CAPTURED_KEY.to_string(), captured_ticks.into());
    for key in [FIVE_HOUR_KEY, WEEKLY_KEY] {
        if let Some((used, reset)) = read_window(limits, key, "used_percentage", "resets_at", now) {
            let window = json!({ "used_percent": used, "reset_at_unix_seconds": reset });
            cache.insert(key.to_string(), window);
        }
    }
    write_cache(path, &Value::Object(cache).to_string()).is_ok()
}

/// Reads the cache at `path`. Returns `None` when the file is missing,
/// malformed, from the future or has no window that is still running.
pub(super) fn read_snapshot(path: &Path, now: i64) -> Option<Snapshot> {
    let cache = parse_object(&files::read_limited(path, MAX_CACHE_BYTES).ok()?)?;
    let captured_ticks =
        integer(cache.get(CAPTURED_KEY)?).filter(|ticks| (0..=MAX_TICKS).contains(ticks))?;
    let captured_at = captured_ticks / TICKS_PER_SECOND - UNIX_EPOCH_SECONDS;
    let age = now.checked_sub(captured_at).filter(|age| *age >= 0)?;

    let window = |key: &str, window_minutes: u32| {
        read_window(&cache, key, "used_percent", "reset_at_unix_seconds", now).map(
            |(used_percent, reset)| LimitWindow {
                used_percent,
                window_minutes,
                resets_at: Some(reset),
            },
        )
    };
    let five_hour = window(FIVE_HOUR_KEY, FIVE_HOUR_MINUTES);
    let weekly = window(WEEKLY_KEY, WEEKLY_MINUTES);
    if five_hour.is_none() && weekly.is_none() {
        return None;
    }
    Some(Snapshot {
        captured_ticks,
        usage: Usage::from_windows(captured_at, five_hour, weekly),
        stale: age > FRESHNESS_SECONDS,
    })
}

fn write_cache(path: &Path, json: &str) -> io::Result<()> {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)?;
    }
    let temporary = TemporaryFile::beside(path);
    std::fs::write(temporary.path(), json)?;
    let mut attempts_left = REPLACE_ATTEMPTS;
    loop {
        let moved = if path.exists() {
            files::replace_file(path, temporary.path())
        } else {
            std::fs::rename(temporary.path(), path)
        };
        attempts_left -= 1;
        match moved {
            // A reader may hold the file open for a moment.
            Err(_) if attempts_left > 0 => std::thread::sleep(REPLACE_RETRY_DELAY),
            result => return result,
        }
    }
}

fn parse_object(json: &[u8]) -> Option<Map<String, Value>> {
    match serde_json::from_slice(json).ok()? {
        Value::Object(object) => Some(object),
        _ => None,
    }
}

/// A window's used percentage and reset time, when both are valid and the
/// reset is still ahead. Each window is judged on its own.
fn read_window(
    source: &Map<String, Value>,
    key: &str,
    used_key: &str,
    reset_key: &str,
    now: i64,
) -> Option<(f64, i64)> {
    let window = source.get(key)?.as_object()?;
    let used = window
        .get(used_key)?
        .as_f64()
        .filter(|used| (0.0..=100.0).contains(used))?;
    let reset = integer(window.get(reset_key)?)?;
    (reset > now && reset <= MAX_UNIX_SECONDS).then_some((used, reset))
}

/// Accepts JSON integers and integer-valued floats, as the C# app does.
fn integer(value: &Value) -> Option<i64> {
    if let Some(whole) = value.as_i64() {
        return Some(whole);
    }
    let number = value.as_f64()?;
    let in_range = number >= i64::MIN as f64 && number < i64::MAX as f64;
    (number.fract() == 0.0 && in_range).then_some(number as i64)
}

fn ticks_from_unix(unix: i64) -> Option<i64> {
    unix.checked_add(UNIX_EPOCH_SECONDS)?
        .checked_mul(TICKS_PER_SECOND)
        .filter(|ticks| (0..=MAX_TICKS).contains(ticks))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use super::super::files::tests::TestDirectory;
    use crate::timefmt::parse_rfc3339;

    const MINUTE: i64 = 60;
    const HOUR: i64 = 3_600;
    const DAY: i64 = 86_400;

    fn now() -> i64 {
        parse_rfc3339("2026-09-28T12:00:00Z").unwrap()
    }

    fn status_line(windows: &[(&str, &str, i64)]) -> String {
        let windows: Vec<String> = windows
            .iter()
            .map(|(key, used, reset)| {
                format!(r#""{key}":{{"used_percentage":{used},"resets_at":{reset}}}"#)
            })
            .collect();
        format!(r#"{{"rate_limits":{{{}}}}}"#, windows.join(","))
    }

    #[test]
    fn captured_readings_age_and_expire() {
        let directory = TestDirectory::new("ClaudeUsageTests");
        let path = directory.file("usage.json");
        let now = now();
        let input = format!(
            r#"{{"session_id":"private-session","rate_limits":{{
                "five_hour":{{"used_percentage":23.5,"resets_at":{}}},
                "seven_day":{{"used_percentage":41.2,"resets_at":{}}}}}}}"#,
            now + 2 * HOUR,
            now + 3 * DAY
        );
        assert!(capture_status_line(&input, &path, now));
        let stored = fs::read_to_string(&path).unwrap();
        // The cache excludes the original status line and its identifiers.
        for private in ["private-session", "session_id", "rate_limits"] {
            assert!(!stored.contains(private), "{private}");
        }

        // Both windows survive through the freshness boundary.
        let snapshot = read_snapshot(&path, now + 30 * MINUTE).unwrap();
        let five_hour = snapshot.usage.five_hour.unwrap();
        let weekly = snapshot.usage.weekly.unwrap();
        assert_eq!((five_hour.used_percent, weekly.used_percent), (23.5, 41.2));
        assert_eq!(
            (five_hour.window_minutes, weekly.window_minutes),
            (FIVE_HOUR_MINUTES, WEEKLY_MINUTES)
        );
        // The reset deadline stays anchored to the capture time.
        assert_eq!(five_hour.resets_at, Some(now + 2 * HOUR));
        assert_eq!(snapshot.usage.updated_at, now);
        assert!(!snapshot.stale);

        // Old readings survive as stale without changing the observation time.
        let snapshot = read_snapshot(&path, now + 30 * MINUTE + 1).unwrap();
        assert!(snapshot.stale);
        assert_eq!(snapshot.usage.weekly.unwrap().used_percent, 41.2);
        assert_eq!(snapshot.usage.updated_at, now);

        // A CLI startup without quota data preserves the last actual reading.
        assert!(!capture_status_line("{}", &path, now + 40 * MINUTE));
        assert_eq!(fs::read_to_string(&path).unwrap(), stored);

        // The stale weekly quota survives the expiry of the shorter window.
        let snapshot = read_snapshot(&path, now + DAY).unwrap();
        assert!(snapshot.stale);
        assert!(snapshot.usage.five_hour.is_none() && snapshot.usage.weekly.is_some());
        // Expired windows are never shown as current allowance.
        assert!(read_snapshot(&path, now + 3 * DAY).is_none());

        // A new reading restores fresh status.
        assert!(capture_status_line(&input, &path, now + 45 * MINUTE));
        assert!(!read_snapshot(&path, now + 45 * MINUTE).unwrap().stale);
        // A capture time in the future is rejected.
        assert!(read_snapshot(&path, now - 1).is_none());
        assert!(directory.leftovers().is_empty());
    }

    #[test]
    fn windows_are_absent_or_expire_independently() {
        let directory = TestDirectory::new("ClaudeUsageTests");
        let path = directory.file("usage.json");
        let now = now();
        let week_reset = now + 3 * DAY;
        let input = status_line(&[
            ("five_hour", "5", now + 10),
            ("seven_day", "60", week_reset),
        ]);
        assert!(capture_status_line(&input, &path, now));
        // An expired five-hour window does not hide the weekly window.
        let snapshot = read_snapshot(&path, now + 11).unwrap();
        assert!(snapshot.usage.five_hour.is_none() && snapshot.usage.weekly.is_some());

        // A missing five-hour window is accepted.
        let weekly_only = status_line(&[("seven_day", "0", week_reset)]);
        assert!(capture_status_line(&weekly_only, &path, now));
        let snapshot = read_snapshot(&path, now).unwrap();
        assert!(snapshot.usage.five_hour.is_none() && snapshot.usage.weekly.is_some());

        // Valid but absent limits clear the old cache.
        assert!(capture_status_line(r#"{"rate_limits":{}}"#, &path, now));
        assert!(read_snapshot(&path, now).is_none());
    }

    #[test]
    fn malformed_and_out_of_range_input_is_ignored() {
        let directory = TestDirectory::new("ClaudeUsageTests");
        let path = directory.file("usage.json");
        let now = now();
        let future = now + HOUR;
        // One hundred percent is valid.
        let good = status_line(&[("five_hour", "100", future)]);
        assert!(capture_status_line(&good, &path, now));
        let before = fs::read_to_string(&path).unwrap();

        // Invalid input does not overwrite a valid cache.
        assert!(!capture_status_line("{invalid", &path, now));
        assert!(!capture_status_line(&"x".repeat(65_537), &path, now));
        assert!(!capture_status_line(" ", &path, now));
        assert!(!capture_status_line("[]", &path, now));
        assert!(!capture_status_line(&good, Path::new(""), now));
        assert_eq!(fs::read_to_string(&path).unwrap(), before);

        // Out-of-range percentages are captured as valid JSON but ignored.
        let out_of_bounds =
            status_line(&[("five_hour", "100.1", future), ("seven_day", "-1", future)]);
        assert!(capture_status_line(&out_of_bounds, &path, now));
        assert!(read_snapshot(&path, now).is_none());

        // A malformed five-hour window does not hide the weekly window.
        let malformed_five =
            status_line(&[("five_hour", "\"9\"", future), ("seven_day", "90", future)]);
        assert!(capture_status_line(&malformed_five, &path, now));
        let snapshot = read_snapshot(&path, now).unwrap();
        assert!(snapshot.usage.five_hour.is_none() && snapshot.usage.weekly.is_some());

        fs::write(&path, "not json").unwrap();
        assert!(read_snapshot(&path, now).is_none());
        fs::remove_file(&path).unwrap();
        assert!(read_snapshot(&path, now).is_none());
        assert!(read_snapshot(Path::new(""), now).is_none());
    }

    #[test]
    fn the_cache_format_matches_the_existing_app() {
        let directory = TestDirectory::new("ClaudeUsageTests");
        let path = directory.file("usage.json");
        let now = now();
        let ticks = 639_261_936_000_000_000_i64;
        let (five_reset, week_reset) = (now + 2 * HOUR, now + 3 * DAY);

        let input = status_line(&[
            ("five_hour", "23.5", five_reset),
            ("seven_day", "41", week_reset),
        ]);
        assert!(capture_status_line(&input, &path, now));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!(
                "{{\"captured_at_utc_ticks\":{ticks},\
                 \"five_hour\":{{\"reset_at_unix_seconds\":{five_reset},\"used_percent\":23.5}},\
                 \"seven_day\":{{\"reset_at_unix_seconds\":{week_reset},\"used_percent\":41.0}}}}"
            )
        );

        // As the C# app writes it: its own member order, whole numbers without
        // a fraction and a capture time with a fraction of a second.
        let written_by_existing_app = format!(
            "{{\"captured_at_utc_ticks\":{},\
             \"five_hour\":{{\"used_percent\":23.5,\"reset_at_unix_seconds\":{five_reset}}},\
             \"seven_day\":{{\"used_percent\":41,\"reset_at_unix_seconds\":{week_reset}}}}}",
            ticks + 1_234_567
        );
        fs::write(&path, written_by_existing_app).unwrap();
        let snapshot = read_snapshot(&path, now).unwrap();
        assert_eq!(snapshot.captured_ticks, ticks + 1_234_567);
        assert_eq!(snapshot.usage.updated_at, now);
        assert_eq!(snapshot.usage.five_hour.unwrap().used_percent, 23.5);
        assert_eq!(snapshot.usage.weekly.unwrap().resets_at, Some(week_reset));
    }

    #[test]
    fn cache_values_outside_the_format_are_rejected() {
        let directory = TestDirectory::new("ClaudeUsageTests");
        let path = directory.file("usage.json");
        let now = now();
        let cache = |ticks: &str, reset: &str| {
            format!(
                "{{\"captured_at_utc_ticks\":{ticks},\
                 \"five_hour\":{{\"used_percent\":10,\"reset_at_unix_seconds\":{reset}}}}}"
            )
        };
        let valid_ticks = ticks_from_unix(now).unwrap().to_string();
        let valid_reset = (now + HOUR).to_string();

        fs::write(&path, cache(&valid_ticks, &valid_reset)).unwrap();
        assert!(read_snapshot(&path, now).is_some());
        // An integer-valued float is still an integer.
        fs::write(&path, cache(&valid_ticks, &format!("{valid_reset}.0"))).unwrap();
        assert!(read_snapshot(&path, now).is_some());

        for (ticks, reset) in [
            ("-1", valid_reset.as_str()),
            ("3155378976000000000", valid_reset.as_str()),
            ("\"1\"", valid_reset.as_str()),
            (valid_ticks.as_str(), "1790600400.5"),
            (valid_ticks.as_str(), "253402300800"),
            (valid_ticks.as_str(), "\"1790600400\""),
        ] {
            fs::write(&path, cache(ticks, reset)).unwrap();
            assert!(read_snapshot(&path, now).is_none(), "{ticks} {reset}");
        }

        let padded = format!(
            "{}{}",
            cache(&valid_ticks, &valid_reset),
            " ".repeat(MAX_CACHE_BYTES)
        );
        fs::write(&path, padded).unwrap();
        assert!(read_snapshot(&path, now).is_none());
    }

    #[test]
    fn status_line_input_is_bounded_and_creates_the_directory() {
        let directory = TestDirectory::new("ClaudeUsageTests");
        let path = directory.file("nested").join("usage.json");
        let now = now();
        let input = status_line(&[("seven_day", "12", now + DAY)]);

        let oversized = format!("{input}{}", " ".repeat(MAX_INPUT_BYTES));
        assert!(!capture_from(oversized.as_bytes(), &path, now));
        assert!(!capture_from(&b""[..], &path, now));
        assert!(!path.exists());

        assert!(capture_from(input.as_bytes(), &path, now));
        let weekly = read_snapshot(&path, now).unwrap().usage.weekly.unwrap();
        assert_eq!(weekly.used_percent, 12.0);
    }
}
