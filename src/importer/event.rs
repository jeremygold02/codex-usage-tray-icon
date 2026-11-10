//! Which log lines count as account-quota readings.
//!
//! Codex appends a `token_count` event to a session's log whenever the server
//! reports usage, and the event carries the account's rate limits as they
//! stood at that moment. Only those percentages are read. The token totals in
//! the same event are never turned into a percentage: the allowance they
//! would have to be measured against is not known.
//!
//! An event is judged against the live reading the import started from. That
//! reading says which windows the signed-in account has and when each one
//! resets, which is what tells this account's earlier cycles apart from
//! limits of another plan or of a cycle that has not begun.

use serde_json::{Map, Value};

use crate::history::reset_after_seconds;
use crate::model::{LimitWindow, Usage, FIVE_HOUR_MINUTES, WEEKLY_MINUTES};
use crate::timefmt::parse_rfc3339;

/// Rate-limit events are far shorter than this. Longer lines hold tool output
/// or attachments and are skipped without being parsed.
pub(super) const MAX_LINE_BYTES: usize = 65_536;
/// The limit id of the account's own quota; other ids belong to extra models.
const ACCOUNT_LIMIT_ID: &str = "codex";
/// Two statements of the same deadline can differ by this much.
const RESET_TOLERANCE_SECONDS: i64 = 2;
const SECONDS_PER_MINUTE: i64 = 60;

/// What an event has to agree with to be imported, fixed for one scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Criteria {
    /// Events before this are older than the history keeps. Unix seconds,
    /// like every other time here.
    earliest: i64,
    now: i64,
    /// When the live reading was taken. A later event may describe a cycle
    /// that reading knows nothing about.
    live_at: i64,
    /// The live deadline of each window, for the windows the account has.
    five_hour_reset: Option<i64>,
    weekly_reset: Option<i64>,
}

impl Criteria {
    /// The criteria set by the live reading `current`, for events from the
    /// `horizon_seconds` before `now`. `None` when the reading has no window
    /// whose deadline is still ahead: without a current cycle to compare
    /// with, no event can be attributed to the account.
    pub(super) fn new(current: &Usage, now: i64, horizon_seconds: i64) -> Option<Self> {
        let live_reset = |window: &Option<LimitWindow>, minutes: u32| {
            let window = window
                .as_ref()
                .filter(|window| window.window_minutes == minutes)?;
            let remaining = reset_after_seconds(window, current.updated_at)?;
            let reset = current.updated_at.checked_add(remaining)?;
            (reset > now).then_some(reset)
        };
        let five_hour_reset = live_reset(&current.five_hour, FIVE_HOUR_MINUTES);
        let weekly_reset = live_reset(&current.weekly, WEEKLY_MINUTES);
        if five_hour_reset.is_none() && weekly_reset.is_none() {
            return None;
        }
        Some(Self {
            earliest: now.saturating_sub(horizon_seconds),
            now,
            live_at: current.updated_at,
            five_hour_reset,
            weekly_reset,
        })
    }

    /// The oldest event time that is still imported, in Unix seconds.
    pub(super) fn earliest(&self) -> i64 {
        self.earliest
    }

    /// One window of an event from `event_at`, if it is a reading of a
    /// window the account has now.
    fn window(&self, source: &Map<String, Value>, event_at: i64) -> Option<LimitWindow> {
        let minutes = whole_number(source.get("window_minutes")?)?;
        let used_percent = source.get("used_percent")?.as_f64()?;
        let resets_at = whole_number(source.get("resets_at")?)?;
        if !(0.0..=100.0).contains(&used_percent) {
            return None;
        }
        let (window_minutes, live_reset) = match u32::try_from(minutes) {
            Ok(FIVE_HOUR_MINUTES) => (FIVE_HOUR_MINUTES, self.five_hour_reset),
            Ok(WEEKLY_MINUTES) => (WEEKLY_MINUTES, self.weekly_reset),
            _ => return None,
        };
        // Earlier cycles are kept for learning. A window the signed-in
        // account lacks, or a cycle after the live one, is not its history.
        let live_reset = live_reset?;
        let remaining = resets_at.checked_sub(event_at)?;
        if remaining <= 0 || resets_at > live_reset.saturating_add(RESET_TOLERANCE_SECONDS) {
            return None;
        }
        // No window promises a reset further away than its own length.
        let longest = i64::from(window_minutes) * SECONDS_PER_MINUTE + RESET_TOLERANCE_SECONDS;
        if remaining > longest {
            return None;
        }
        Some(LimitWindow {
            used_percent,
            window_minutes,
            resets_at: Some(resets_at),
        })
    }
}

/// The reading in one log line, or `None` when the line is not an account
/// rate-limit event that meets `criteria`. Each window is judged on its own,
/// so an event can supply one window and not the other.
///
/// The reading's time is the event's, cut to whole seconds, the precision of
/// [`Usage::updated_at`].
pub(super) fn parse_line(line: &str, criteria: &Criteria) -> Option<Usage> {
    if line.len() > MAX_LINE_BYTES {
        return None;
    }
    let root: Value = serde_json::from_str(line.trim_start_matches('\u{feff}')).ok()?;
    let root = root.as_object()?;
    if !has_text(root, "type", "event_msg") {
        return None;
    }
    let payload = root.get("payload")?.as_object()?;
    if !has_text(payload, "type", "token_count") {
        return None;
    }
    let limits = payload.get("rate_limits")?.as_object()?;
    match limits.get("limit_id") {
        // Older events carry no id and only ever described the account.
        None => {}
        Some(Value::String(id)) if id.eq_ignore_ascii_case(ACCOUNT_LIMIT_ID) => {}
        Some(_) => return None,
    }

    let timestamp = root.get("timestamp")?.as_str()?;
    // Codex writes UTC. A local time would need an offset this app cannot verify.
    if !timestamp.ends_with(['Z', 'z']) && !timestamp.ends_with("+00:00") {
        return None;
    }
    let event_at = parse_rfc3339(timestamp)?;
    if event_at < criteria.earliest || event_at > criteria.now || event_at > criteria.live_at {
        return None;
    }

    let mut five_hour = None;
    let mut weekly = None;
    for name in ["primary", "secondary"] {
        let window = limits
            .get(name)
            .and_then(Value::as_object)
            .and_then(|source| criteria.window(source, event_at));
        match window {
            Some(window) if window.window_minutes == FIVE_HOUR_MINUTES => {
                five_hour = five_hour.or(Some(window));
            }
            Some(window) => weekly = weekly.or(Some(window)),
            None => {}
        }
    }
    if five_hour.is_none() && weekly.is_none() {
        return None;
    }
    Some(Usage::from_windows(event_at, five_hour, weekly))
}

fn has_text(source: &Map<String, Value>, key: &str, expected: &str) -> bool {
    source.get(key).and_then(Value::as_str) == Some(expected)
}

/// A JSON number without a fractional part, however it was written.
fn whole_number(value: &Value) -> Option<i64> {
    if let Some(whole) = value.as_i64() {
        return Some(whole);
    }
    let number = value.as_f64()?;
    let in_range = number >= i64::MIN as f64 && number < i64::MAX as f64;
    (number.fract() == 0.0 && in_range).then_some(number as i64)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::history::tests::{DAY, HOUR, MINUTE, NOW};
    use crate::history::RETENTION_DAYS;
    use crate::localtime::LocalTime;

    pub const HORIZON: i64 = RETENTION_DAYS * DAY;

    /// A live reading with both windows: half of the five hours and 60
    /// percent of the week used.
    pub fn current(updated_at: i64, five_hour_reset: i64, weekly_reset: i64) -> Usage {
        let window = |used_percent, window_minutes, reset| LimitWindow {
            used_percent,
            window_minutes,
            resets_at: Some(reset),
        };
        Usage::from_windows(
            updated_at,
            Some(window(50.0, FIVE_HOUR_MINUTES, five_hour_reset)),
            Some(window(60.0, WEEKLY_MINUTES, weekly_reset)),
        )
    }

    /// A timestamp as Codex writes it, with `fraction` as the digits after
    /// the seconds.
    pub fn timestamp(unix: i64, fraction: &str) -> String {
        let time = LocalTime::from_wall_ms(unix * 1_000);
        let date = time.date();
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{fraction}Z",
            date.year(),
            date.month(),
            date.day(),
            time.hour(),
            time.minute(),
            time.second()
        )
    }

    /// One rate-limit window as it appears in an event.
    pub fn window(percent: f64, minutes: u32, reset: i64) -> String {
        format!(r#"{{"used_percent":{percent},"window_minutes":{minutes},"resets_at":{reset}}}"#)
    }

    /// A `token_count` event line with the given text for its timestamp.
    pub fn event_at(
        timestamp: &str,
        primary: Option<&str>,
        secondary: Option<&str>,
        limit_id: Option<&str>,
    ) -> String {
        let limit_id = limit_id.map_or(String::new(), |id| format!(r#""limit_id":"{id}","#));
        format!(
            concat!(
                r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"token_count","#,
                r#""rate_limits":{{{}"primary":{},"secondary":{}}}}}}}"#
            ),
            timestamp,
            limit_id,
            primary.unwrap_or("null"),
            secondary.unwrap_or("null")
        )
    }

    /// A `token_count` event line from the given Unix second.
    pub fn event(
        unix: i64,
        primary: Option<&str>,
        secondary: Option<&str>,
        limit_id: Option<&str>,
    ) -> String {
        event_at(&timestamp(unix, "0000000"), primary, secondary, limit_id)
    }

    fn parse(line: &str, current: &Usage, now: i64) -> Option<Usage> {
        parse_line(line, &Criteria::new(current, now, HORIZON)?)
    }

    #[test]
    fn account_events_yield_the_windows_they_report() {
        let five_reset = NOW + 2 * HOUR;
        let week_reset = NOW + 3 * DAY;
        let live = current(NOW, five_reset, week_reset);
        let five = window(12.5, 300, five_reset);
        let week = window(37.5, 10_080, week_reset);
        let codex = Some("codex");
        let earlier = NOW - 20 * MINUTE;

        let parsed = parse(&event(earlier, Some(&five), Some(&week), codex), &live, NOW)
            .expect("Codex token count event provides both primary limits");
        assert_eq!(parsed.updated_at, earlier);
        let (five_hour, weekly) = (
            parsed.five_hour.as_ref().unwrap(),
            parsed.weekly.as_ref().unwrap(),
        );
        assert_eq!(
            (five_hour.used_percent, weekly.used_percent),
            (12.5, 37.5),
            "historical percentages are retained"
        );
        assert_eq!(
            (five_hour.window_minutes, weekly.window_minutes),
            (FIVE_HOUR_MINUTES, WEEKLY_MINUTES)
        );
        assert_eq!(
            (five_hour.resets_at, weekly.resets_at),
            (Some(five_reset), Some(week_reset)),
            "reset deadline is preserved"
        );
        assert!(parsed.additional_limits.is_empty() && parsed.resets.is_empty());

        assert_eq!(
            parse(
                &event(earlier, Some(&five), Some(&week), Some("other")),
                &live,
                NOW
            ),
            None,
            "additional limit IDs are excluded"
        );
        assert!(
            parse(&event(earlier, Some(&five), Some(&week), None), &live, NOW).is_some(),
            "older events without a limit ID are accepted"
        );
        assert!(
            parse(
                &event(earlier, Some(&five), Some(&week), Some("CODEX")),
                &live,
                NOW
            )
            .is_some(),
            "the account's limit ID is matched without regard to case"
        );
        let null_id = event(earlier, Some(&five), Some(&week), None)
            .replace(r#""rate_limits":{"#, r#""rate_limits":{"limit_id":null,"#);
        assert_eq!(
            parse(&null_id, &live, NOW),
            None,
            "a limit ID that is present has to name the account"
        );
        // The windows are told apart by their length, not by their position.
        let swapped = parse(&event(earlier, Some(&week), Some(&five), codex), &live, NOW).unwrap();
        assert_eq!(swapped, parsed);
    }

    #[test]
    fn each_window_is_judged_on_its_own() {
        let five_reset = NOW + 2 * HOUR;
        let week_reset = NOW + 3 * DAY;
        let live = current(NOW, five_reset, week_reset);
        let week = window(37.5, 10_080, week_reset);
        let codex = Some("codex");
        let earlier = NOW - 20 * MINUTE;
        let with_five = |five: &str| {
            parse(&event(earlier, Some(five), Some(&week), codex), &live, NOW)
                .expect("the weekly window is still read")
        };

        assert_eq!(
            with_five(&window(20.0, 300, five_reset + MINUTE)).five_hour,
            None,
            "different reset cycle is excluded independently"
        );
        assert!(
            with_five(&window(20.0, 300, five_reset + 2))
                .five_hour
                .is_some(),
            "a deadline restated two seconds later is the same cycle"
        );
        assert_eq!(
            with_five(&window(20.0, 300, five_reset + 3)).five_hour,
            None
        );
        for percent in [-1.0, 100.5] {
            assert_eq!(
                with_five(&window(percent, 300, five_reset)).five_hour,
                None,
                "invalid percentage is excluded independently"
            );
        }
        for percent in [0.0, 100.0] {
            assert!(with_five(&window(percent, 300, five_reset))
                .five_hour
                .is_some());
        }
        assert_eq!(five_reset, 1_791_554_417);
        for malformed in [
            r#"{"used_percent":"20","window_minutes":300,"resets_at":1791554417}"#,
            r#"{"used_percent":20,"window_minutes":300.5,"resets_at":1791554417}"#,
            r#"{"used_percent":20,"window_minutes":300,"resets_at":1791554417.5}"#,
            r#"{"used_percent":20,"window_minutes":300,"resets_at":"1791554417"}"#,
            r#"{"used_percent":20,"window_minutes":300,"resets_at":1e300}"#,
            r#"{"used_percent":20,"window_minutes":300}"#,
            r#"{"used_percent":20,"resets_at":1791554417}"#,
            r#"{"window_minutes":300,"resets_at":1791554417}"#,
            r#"{"used_tokens":20000,"window_minutes":300,"resets_at":1791554417}"#,
            "[]",
            "5",
        ] {
            assert_eq!(with_five(malformed).five_hour, None, "{malformed}");
        }
        // Numbers may be written as floats as long as they are whole.
        let floats = r#"{"used_percent":20,"window_minutes":300.0,"resets_at":1.791554417e9}"#;
        assert_eq!(with_five(floats).five_hour.unwrap().used_percent, 20.0);

        assert_eq!(
            parse(
                &event(earlier, Some(&window(10.0, 60, five_reset)), None, codex),
                &live,
                NOW
            ),
            None,
            "wrong window duration is excluded"
        );
        // A second window of the same length never replaces the first.
        let twice = parse(
            &event(
                earlier,
                Some(&week),
                Some(&window(99.0, 10_080, week_reset)),
                codex,
            ),
            &live,
            NOW,
        )
        .unwrap();
        assert_eq!(twice.weekly.unwrap().used_percent, 37.5);
    }

    #[test]
    fn events_outside_the_live_reading_s_reach_are_excluded() {
        let five_reset = NOW + 2 * HOUR;
        let week_reset = NOW + 3 * DAY;
        let live = current(NOW, five_reset, week_reset);
        let five = window(12.5, 300, five_reset);
        let week = window(37.5, 10_080, week_reset);
        let codex = Some("codex");

        assert_eq!(
            parse(
                &event(NOW - 29 * DAY, Some(&five), Some(&week), codex),
                &live,
                NOW
            ),
            None,
            "samples older than four weeks are excluded"
        );
        assert_eq!(
            parse(&event(NOW + 1, Some(&five), Some(&week), codex), &live, NOW),
            None,
            "future samples are excluded"
        );
        assert_eq!(
            parse(
                &event(NOW - 5 * MINUTE, Some(&five), None, codex),
                &current(NOW - 10 * MINUTE, five_reset, week_reset),
                NOW
            ),
            None,
            "events newer than the live reading are excluded"
        );
        assert_eq!(
            parse(&event(NOW - 4 * HOUR, Some(&five), None, codex), &live, NOW),
            None,
            "a reset deadline beyond the five-hour cycle is excluded"
        );
        assert_eq!(
            parse(
                &event(
                    NOW - 20 * MINUTE,
                    Some(&window(12.5, 300, NOW - 20 * MINUTE)),
                    None,
                    codex
                ),
                &live,
                NOW
            ),
            None,
            "a window that had already reset says nothing about its cycle"
        );

        let prior_cycle = event(
            NOW - 6 * DAY,
            None,
            Some(&window(80.0, 10_080, week_reset - 7 * DAY)),
            codex,
        );
        let parsed = parse(&prior_cycle, &live, NOW).unwrap();
        assert_eq!(
            parsed.weekly.unwrap().used_percent,
            80.0,
            "prior reset cycles within 14 days are available for learning"
        );
        // Four weeks back is the limit, and an event exactly there still counts.
        let oldest = event(
            NOW - 28 * DAY,
            None,
            Some(&window(70.0, 10_080, week_reset - 28 * DAY)),
            codex,
        );
        assert!(parse(&oldest, &live, NOW).is_some());
        let five_hour_only = Usage::from_windows(NOW, live.five_hour.clone(), None);
        assert_eq!(
            parse(&prior_cycle, &five_hour_only, NOW),
            None,
            "history cannot add a window missing from the current account"
        );
    }

    #[test]
    fn a_live_reading_without_a_running_cycle_accepts_nothing() {
        let five_reset = NOW + 2 * HOUR;
        let week_reset = NOW + 3 * DAY;
        let line = event(
            NOW - 20 * MINUTE,
            Some(&window(12.5, 300, five_reset)),
            Some(&window(37.5, 10_080, week_reset)),
            Some("codex"),
        );
        assert_eq!(
            parse(&line, &Usage::from_windows(0, None, None), NOW),
            None,
            "missing current cycle is rejected"
        );
        let mut unknown_deadline = current(NOW, five_reset, week_reset);
        unknown_deadline.five_hour.as_mut().unwrap().resets_at = None;
        unknown_deadline.weekly.as_mut().unwrap().resets_at = None;
        assert_eq!(Criteria::new(&unknown_deadline, NOW, HORIZON), None);
        assert_eq!(
            Criteria::new(&current(NOW - DAY, NOW - HOUR, NOW), NOW, HORIZON),
            None,
            "deadlines that have passed describe no current cycle"
        );
        // One running window is enough, and only that window is imported.
        let weekly_running = current(NOW, NOW - HOUR, week_reset);
        let parsed = parse(&line, &weekly_running, NOW).unwrap();
        assert!(parsed.five_hour.is_none() && parsed.weekly.is_some());
    }

    #[test]
    fn lines_that_are_not_rate_limit_events_are_ignored() {
        let five_reset = NOW + 2 * HOUR;
        let week_reset = NOW + 3 * DAY;
        let live = current(NOW, five_reset, week_reset);
        let five = window(12.5, 300, five_reset);
        let earlier = NOW - 20 * MINUTE;
        let valid = event(earlier, Some(&five), None, Some("codex"));
        assert!(parse(&valid, &live, NOW).is_some());

        assert_eq!(
            parse("{invalid", &live, NOW),
            None,
            "malformed events are ignored"
        );
        for line in [
            "",
            "   ",
            "null",
            "[]",
            "\"token_count rate_limits\"",
            &valid[..valid.len() - 1],
            &format!("{valid}{valid}"),
            &valid.replace("event_msg", "response_item"),
            &valid.replace("token_count", "agent_message"),
            &valid.replace(r#""type":"event_msg""#, r#""type":7"#),
            &valid.replace(r#""rate_limits":{"#, r#""rate_limits":null,"other":{"#),
            &valid.replace(r#""payload":{"#, r#""payload":null,"other":{"#),
        ] {
            assert_eq!(parse(line, &live, NOW), None, "{line}");
        }
        // Token totals are present in real events and are never read.
        let tokens_only = format!(
            concat!(
                r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"token_count","#,
                r#""info":{{"total_token_usage":{{"total_tokens":150000}},"#,
                r#""model_context_window":200000}},"rate_limits":null}}}}"#
            ),
            timestamp(earlier, "000")
        );
        assert_eq!(
            parse(&tokens_only, &live, NOW),
            None,
            "token counts are never converted into quota percentages"
        );
        let with_tokens = valid.replace(
            r#""rate_limits""#,
            r#""info":{"total_token_usage":{"total_tokens":150000}},"rate_limits""#,
        );
        assert_eq!(parse(&with_tokens, &live, NOW), parse(&valid, &live, NOW));

        // A byte-order mark and surrounding whitespace do not hide an event.
        assert!(parse(&format!("\u{feff}{valid}\r"), &live, NOW).is_some());
        let padded = format!("{valid}{}", " ".repeat(MAX_LINE_BYTES - valid.len()));
        assert!(parse(&padded, &live, NOW).is_some());
        assert_eq!(
            parse(&format!("{padded} "), &live, NOW),
            None,
            "overlong lines are not parsed"
        );
        let nested = format!("{}{}", "[".repeat(5_000), "]".repeat(5_000));
        assert_eq!(parse(&nested, &live, NOW), None);
    }

    #[test]
    fn timestamps_have_to_be_utc() {
        let five_reset = NOW + 2 * HOUR;
        let live = current(NOW, five_reset, NOW + 3 * DAY);
        let five = window(12.5, 300, five_reset);
        let earlier = NOW - 20 * MINUTE;
        let at = |text: &str| {
            parse(
                &event_at(text, Some(&five), None, Some("codex")),
                &live,
                NOW,
            )
            .map(|usage| usage.updated_at)
        };

        // Codex writes milliseconds; they are cut to the whole second.
        assert_eq!(at(&timestamp(earlier, "987")), Some(earlier));
        assert_eq!(at(&timestamp(earlier, "0000000")), Some(earlier));
        let whole = timestamp(earlier, "0").replace(".0Z", "");
        assert_eq!(whole, "2026-10-09T11:40:17");
        assert_eq!(at(&format!("{whole}Z")), Some(earlier));
        assert_eq!(at(&format!("{whole}z")), Some(earlier));
        assert_eq!(at(&format!("{whole}+00:00")), Some(earlier));
        for text in [
            whole.clone(),
            format!("{whole}+02:00"),
            format!("{whole}-00:00"),
            format!("{whole}Z "),
            "yesterday at noon Z".to_string(),
            String::new(),
        ] {
            assert_eq!(at(&text), None, "{text}");
        }
        let numeric = event(earlier, Some(&five), None, Some("codex")).replace(
            &format!(r#""{}""#, timestamp(earlier, "0000000")),
            &earlier.to_string(),
        );
        assert_eq!(
            parse(&numeric, &live, NOW),
            None,
            "a numeric timestamp is not read"
        );
    }
}
