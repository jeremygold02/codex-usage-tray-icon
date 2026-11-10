//! One reminder per banked reset shortly before it expires. The reminders
//! already given are kept in a file with the format of the C# app's
//! `reset-reminders.json`, so a restart does not repeat them.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use serde_json::Value;

use super::storage::{read_capped, write_atomically};
use super::{known_expiry, representable, DOTNET_EPOCH_SECONDS};
use crate::model::{ResetCredit, Usage};

const MAX_FILE_BYTES: u64 = 1_048_576;
const MAX_ENTRIES: usize = 2_048;
const MIN_LEAD_HOURS: i32 = 1;
const MAX_LEAD_HOURS: i32 = 72;
/// .NET measures time in 100-nanosecond ticks.
const TICKS_PER_SECOND: i64 = 10_000_000;

/// The reminders already given, as reminder key to credit expiry in Unix
/// seconds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ExpiryReminders {
    notified: BTreeMap<String, i64>,
}

/// What one reading changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Observation {
    /// Expiry times of the credits that need a reminder now.
    pub due: Vec<i64>,
    /// Whether the saved state is out of date.
    pub changed: bool,
}

impl ExpiryReminders {
    /// Loads the saved reminders. A missing or damaged file gives an empty
    /// state: a reminder may then repeat, but monitoring carries on.
    pub fn load(path: &Path) -> Self {
        read_capped(path, MAX_FILE_BYTES)
            .and_then(|json| Self::from_json(&json))
            .unwrap_or_default()
    }

    /// Replaces the saved reminders with these.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        write_atomically(path, &self.to_json())
    }

    /// Records a successful reading and returns the credits that have come
    /// within `lead_hours` of expiring since the last one.
    pub fn observe(&mut self, usage: &Usage, now: i64, lead_hours: i32) -> Observation {
        let remembered = self.notified.len();
        self.notified.retain(|_, expiry| *expiry > now);
        let forgotten = self.notified.len() != remembered;

        let deadline = now.saturating_add(lead_seconds(lead_hours));
        let mut due = Vec::new();
        for credit in &usage.resets {
            let Some(expiry) = known_expiry(credit) else {
                continue;
            };
            let key = reminder_key(credit, expiry);
            if expiry <= now || expiry > deadline || self.notified.contains_key(&key) {
                continue;
            }
            if self.notified.len() >= MAX_ENTRIES {
                break;
            }
            self.notified.insert(key, expiry);
            due.push(expiry);
        }
        Observation {
            changed: forgotten || !due.is_empty(),
            due,
        }
    }

    /// When the next credit without a reminder comes due, or `now` if one
    /// already has.
    pub fn next_check(&self, usage: &Usage, now: i64, lead_hours: i32) -> Option<i64> {
        let lead = lead_seconds(lead_hours);
        usage
            .resets
            .iter()
            .filter_map(|credit| {
                let expiry = known_expiry(credit)?;
                let pending =
                    expiry > now && !self.notified.contains_key(&reminder_key(credit, expiry));
                pending.then(|| (expiry - lead).max(now))
            })
            .min()
    }

    fn from_json(json: &str) -> Option<Self> {
        let root: Value = serde_json::from_str(json).ok()?;
        let entries = root.as_object()?;
        if entries.len() > MAX_ENTRIES {
            return None;
        }
        let notified = entries
            .iter()
            .map(|(key, date)| Some((key.clone(), parse_date(date.as_str()?)?)))
            .collect::<Option<_>>()?;
        Some(Self { notified })
    }

    fn to_json(&self) -> String {
        let entries: Vec<String> = self
            .notified
            .iter()
            .map(|(key, expiry)| {
                // The C# reader only takes a string for a date when its
                // slashes are escaped, which a JSON library would not do.
                let milliseconds = expiry.saturating_mul(1_000);
                format!(
                    "{}:\"\\/Date({milliseconds})\\/\"",
                    Value::from(key.as_str())
                )
            })
            .collect();
        format!("{{{}}}", entries.join(","))
    }
}

fn lead_seconds(lead_hours: i32) -> i64 {
    i64::from(lead_hours.clamp(MIN_LEAD_HOURS, MAX_LEAD_HOURS)) * 3_600
}

/// Identifies one credit at one expiry, so a changed expiry is reminded again.
/// The time is in .NET ticks to match keys the C# app saved.
fn reminder_key(credit: &ResetCredit, expiry: i64) -> String {
    let ticks = (expiry + DOTNET_EPOCH_SECONDS) * TICKS_PER_SECOND;
    match &credit.id {
        Some(id) => format!("{id}:{ticks}"),
        None => format!(
            "{}:{}:{ticks}",
            credit.reset_type.as_deref().unwrap_or_default(),
            credit.title.as_deref().unwrap_or_default()
        ),
    }
}

/// Reads the C# serializer's date form, `/Date(<Unix milliseconds>)/`.
fn parse_date(text: &str) -> Option<i64> {
    let milliseconds: i64 = text
        .strip_prefix("/Date(")?
        .strip_suffix(")/")?
        .parse()
        .ok()?;
    representable(milliseconds.div_euclid(1_000))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resets::storage::tests::TempDir;

    const NOW: i64 = 1_783_700_000;
    const HOUR: i64 = 3_600;

    fn credit(id: &str, expires_at: Option<i64>) -> ResetCredit {
        ResetCredit {
            id: Some(id.to_string()),
            reset_type: Some("codexRateLimits".to_string()),
            title: Some("Full reset".to_string()),
            expires_at,
        }
    }

    fn usage(resets: Vec<ResetCredit>) -> Usage {
        let mut usage = Usage::from_windows(NOW, None, None);
        usage.resets = resets;
        usage
    }

    fn due_count(reminders: &mut ExpiryReminders, usage: &Usage) -> usize {
        reminders.observe(usage, NOW, 24).due.len()
    }

    #[test]
    fn each_expiring_credit_is_reminded_once() {
        let directory = TempDir::new("reminders");
        let path = directory.file("reset-reminders.json");
        let mut usage = usage(vec![
            credit("due", Some(NOW + 24 * HOUR)),
            credit("later", Some(NOW + 30 * HOUR)),
            credit("expired", Some(NOW)),
            credit("unknown", None),
        ]);
        let mut reminders = ExpiryReminders::load(&path);
        // A credit exactly at the lead time is due immediately.
        assert_eq!(reminders.next_check(&usage, NOW, 24), Some(NOW));

        // Only a known, unexpired credit inside the lead time is due.
        let first = reminders.observe(&usage, NOW, 24);
        assert_eq!(first.due, [NOW + 24 * HOUR]);
        assert!(first.changed);
        reminders.save(&path).unwrap();
        // Repeat polls do not remind twice, nor does a restart.
        let repeat = reminders.observe(&usage, NOW, 24);
        assert!(repeat.due.is_empty() && !repeat.changed);
        assert_eq!(due_count(&mut ExpiryReminders::load(&path), &usage), 0);

        // The next check is scheduled ahead of the later expiry.
        assert_eq!(reminders.next_check(&usage, NOW, 24), Some(NOW + 6 * HOUR));
        // A redeemed credit does not keep a scheduled reminder.
        usage.resets.remove(1);
        assert_eq!(reminders.next_check(&usage, NOW, 24), None);

        // A changed expiry permits an updated reminder.
        usage.resets[0].expires_at = Some(NOW + 23 * HOUR);
        assert_eq!(due_count(&mut reminders, &usage), 1);
        // A damaged cache does not break reminders.
        std::fs::write(&path, "invalid").unwrap();
        assert_eq!(due_count(&mut ExpiryReminders::load(&path), &usage), 1);
    }

    #[test]
    fn expired_reminders_are_forgotten() {
        let mut reminders = ExpiryReminders::default();
        let usage = usage(vec![credit("due", Some(NOW + HOUR))]);
        reminders.observe(&usage, NOW, 24);
        assert_eq!(reminders.notified.len(), 1);

        // Forgetting an entry is a change worth saving, even with nothing due.
        let later = reminders.observe(&usage, NOW + HOUR, 24);
        assert!(later.due.is_empty() && later.changed);
        assert!(reminders.notified.is_empty());
    }

    #[test]
    fn lead_time_is_limited_to_the_supported_range() {
        let usage = usage(vec![
            credit("soon", Some(NOW + 2 * HOUR)),
            credit("far", Some(NOW + 73 * HOUR)),
        ]);
        let due = |now: i64, lead_hours: i32| {
            ExpiryReminders::default()
                .observe(&usage, now, lead_hours)
                .due
        };
        // Less than one hour counts as one hour.
        let reminders = ExpiryReminders::default();
        assert_eq!(reminders.next_check(&usage, NOW, 0), Some(NOW + HOUR));
        assert!(due(NOW, 0).is_empty());
        assert_eq!(due(NOW + HOUR, 0), [NOW + 2 * HOUR]);
        // More than 72 hours counts as 72 hours.
        assert_eq!(due(NOW, 500), [NOW + 2 * HOUR]);
        assert_eq!(due(NOW + HOUR, 500), [NOW + 2 * HOUR, NOW + 73 * HOUR]);
    }

    #[test]
    fn the_cache_stops_growing_at_its_limit() {
        let mut reminders = ExpiryReminders::default();
        let many = usage(
            (0..MAX_ENTRIES + 5)
                .map(|index| credit(&format!("credit-{index}"), Some(NOW + HOUR)))
                .collect(),
        );
        assert_eq!(due_count(&mut reminders, &many), MAX_ENTRIES);
        assert_eq!(due_count(&mut reminders, &many), 0);
        // The credits left out stay scheduled.
        assert_eq!(reminders.next_check(&many, NOW, 24), Some(NOW));
    }

    #[test]
    fn keys_and_dates_match_the_existing_format() {
        // .NET counts this second as tick 639194174800000000.
        let expiry = 1_783_820_680;
        assert_eq!(
            reminder_key(&credit("credit-1", None), expiry),
            "credit-1:639194174800000000"
        );
        let unnamed = ResetCredit {
            id: None,
            reset_type: Some("codexRateLimits".to_string()),
            title: None,
            expires_at: None,
        };
        assert_eq!(
            reminder_key(&unnamed, expiry),
            "codexRateLimits::639194174800000000"
        );

        let mut reminders = ExpiryReminders::default();
        assert_eq!(reminders.to_json(), "{}");
        let quoted = usage(vec![credit("credit-\"1\"", Some(expiry))]);
        reminders.observe(&quoted, expiry - HOUR, 24);
        let json = reminders.to_json();
        assert_eq!(
            json,
            r#"{"credit-\"1\":639194174800000000":"\/Date(1783820680000)\/"}"#
        );
        assert_eq!(ExpiryReminders::from_json(&json), Some(reminders));
    }

    #[test]
    fn unusable_caches_are_discarded_whole() {
        assert_eq!(
            ExpiryReminders::from_json("{}"),
            Some(ExpiryReminders::default())
        );
        for invalid in [
            "invalid",
            "null",
            "[]",
            r#"{"credit-1:1":"2026-07-11T21:44:40Z"}"#,
            r#"{"credit-1:1":1783820680000}"#,
            r#"{"credit-1:1":"\/Date(soon)\/"}"#,
            // Beyond the last date the C# app can represent.
            r#"{"credit-1:1":"\/Date(253402300800000)\/"}"#,
            r#"{"credit-1:1":"\/Date(1783820680000)\/","credit-2:1":null}"#,
        ] {
            assert_eq!(ExpiryReminders::from_json(invalid), None, "{invalid}");
        }

        let oversized: Vec<String> = (0..=MAX_ENTRIES)
            .map(|index| format!(r#""credit-{index}:1":"\/Date(1783820680000)\/""#))
            .collect();
        let oversized = format!("{{{}}}", oversized.join(","));
        assert_eq!(ExpiryReminders::from_json(&oversized), None);
    }
}
