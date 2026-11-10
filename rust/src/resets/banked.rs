//! Remembers the banked resets seen so far, across refreshes and restarts, so
//! that a newly added one is announced exactly once. The state file has the
//! format of the C# app's `banked-reset-state.json`.

use std::collections::BTreeSet;
use std::io;
use std::path::Path;

use serde_json::Value;

use super::storage::{read_capped, write_atomically};
use crate::model::Usage;

const STATE_VERSION: i64 = 1;
/// The C# app's JSON reader refuses documents beyond roughly this size.
const MAX_STATE_BYTES: u64 = 2 * 1024 * 1024;

/// The banked resets known from earlier readings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct BankedResets {
    /// False until a reading with reset information establishes the baseline.
    observed: bool,
    available_count: usize,
    /// Whether the last reading itemized every reset it counted. Credit IDs
    /// are only compared between two complete lists.
    itemized_list_was_complete: bool,
    observed_credit_ids: BTreeSet<String>,
}

impl BankedResets {
    /// Loads the saved state. A missing, damaged or unsupported file gives an
    /// empty state, whose first reading sets the baseline without announcing
    /// anything.
    pub fn load(path: &Path) -> Self {
        read_capped(path, MAX_STATE_BYTES)
            .and_then(|json| Self::from_json(&json))
            .unwrap_or_default()
    }

    /// Replaces the saved state with this one.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        write_atomically(path, &self.to_json())
    }

    /// The number of banked resets in the latest reading that reported any.
    pub fn available_count(&self) -> usize {
        self.available_count
    }

    /// Records a successful reading and returns how many resets it added. A
    /// reading without reset information changes nothing.
    pub fn observe(&mut self, usage: &Usage) -> usize {
        if usage.reset_count.is_none() && usage.resets.is_empty() {
            return 0;
        }
        let itemized_count = usage.resets.len();
        let available_count = itemized_count.max(usage.reset_count.unwrap_or(0));
        let itemized_list_is_complete = itemized_count >= available_count;
        // Every ID is recorded, whether or not this reading can be compared.
        let new_itemized_count = usage
            .resets
            .iter()
            .filter_map(|credit| credit.id.as_deref())
            .filter(|id| !id.trim().is_empty())
            .filter(|id| self.observed_credit_ids.insert((*id).to_string()))
            .count();

        let count_increase = available_count.saturating_sub(self.available_count);
        let added = if !self.observed {
            0
        } else if self.itemized_list_was_complete && itemized_list_is_complete {
            // A reset that replaces a used one leaves the total unchanged.
            count_increase.max(new_itemized_count)
        } else {
            count_increase
        };
        self.observed = true;
        self.available_count = available_count;
        self.itemized_list_was_complete = itemized_list_is_complete;
        added
    }

    fn from_json(json: &str) -> Option<Self> {
        let root: Value = serde_json::from_str(json).ok()?;
        let root = root.as_object()?;
        // The C# class supplies a default for a missing member, so only a
        // member that is present can be wrong.
        let flag = |name: &str| root.get(name).map_or(Some(false), Value::as_bool);
        let version = root
            .get("Version")
            .map_or(Some(STATE_VERSION), Value::as_i64)?;
        if version != STATE_VERSION {
            return None;
        }
        // The C# member is a 32-bit integer; a negative count is invalid.
        let available_count = match root.get("AvailableCount") {
            None => 0,
            Some(count) => usize::try_from(i32::try_from(count.as_i64()?).ok()?).ok()?,
        };
        let observed_credit_ids = match root.get("ObservedCreditIds") {
            None | Some(Value::Null) => BTreeSet::new(),
            Some(ids) => ids
                .as_array()?
                .iter()
                .filter_map(Value::as_str)
                .filter(|id| !id.trim().is_empty())
                .map(str::to_string)
                .collect(),
        };
        let itemized_list_was_complete = flag("ItemizedListWasComplete")?;
        if !flag("HasObservation")? {
            return Some(Self::default());
        }
        Some(Self {
            observed: true,
            available_count,
            itemized_list_was_complete,
            observed_credit_ids,
        })
    }

    /// Members appear in the order the C# serializer writes them.
    fn to_json(&self) -> String {
        let ids: Value = self.observed_credit_ids.iter().cloned().collect();
        format!(
            "{{\"Version\":{STATE_VERSION},\"HasObservation\":{},\"AvailableCount\":{},\
             \"ItemizedListWasComplete\":{},\"ObservedCreditIds\":{ids}}}",
            self.observed, self.available_count, self.itemized_list_was_complete
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ResetCredit;
    use crate::resets::storage::tests::TempDir;

    fn usage(reset_count: Option<usize>, credit_ids: &[&str]) -> Usage {
        let mut usage = Usage::from_windows(0, None, None);
        usage.reset_count = reset_count;
        usage.resets = credit_ids
            .iter()
            .map(|id| ResetCredit {
                id: Some((*id).to_string()),
                reset_type: Some("codexRateLimits".to_string()),
                title: None,
                expires_at: Some(1_783_820_680),
            })
            .collect();
        usage
    }

    /// The detector as it is after the app restarts with its saved state.
    fn restarted(banked: &BankedResets) -> BankedResets {
        BankedResets::from_json(&banked.to_json()).unwrap()
    }

    #[test]
    fn added_resets_are_counted_once() {
        let mut banked = BankedResets::default();
        // The first reading establishes a baseline.
        assert_eq!(
            banked.observe(&usage(Some(2), &["credit-1", "credit-2"])),
            0
        );

        let added = usage(Some(3), &["credit-1", "credit-2", "credit-3"]);
        assert_eq!(banked.observe(&added), 1);
        assert_eq!(banked.available_count(), 3);
        assert_eq!(banked.observe(&added), 0);

        // A new itemized reset counts even when the total is unchanged.
        let replacement = usage(Some(3), &["credit-2", "credit-3", "credit-4"]);
        assert_eq!(banked.observe(&replacement), 1);
    }

    #[test]
    fn readings_without_reset_data_keep_the_state() {
        let mut banked = BankedResets::default();
        banked.observe(&usage(Some(3), &["credit-1", "credit-2", "credit-3"]));
        assert_eq!(banked.observe(&usage(None, &[])), 0);
        assert_eq!(banked.available_count(), 3);

        // Using resets is not announced, and a later addition still is.
        assert_eq!(banked.observe(&usage(Some(0), &[])), 0);
        assert_eq!(banked.available_count(), 0);
        assert_eq!(banked.observe(&usage(Some(1), &[])), 1);
    }

    #[test]
    fn an_incomplete_list_is_judged_by_its_count_alone() {
        let mut banked = BankedResets::default();
        banked.observe(&usage(Some(3), &["credit-1"]));
        // Another ID from the same three resets is not a new reset.
        assert_eq!(
            banked.observe(&usage(Some(3), &["credit-1", "credit-2"])),
            0
        );
        // The list completes without the count changing.
        assert_eq!(
            banked.observe(&usage(Some(3), &["credit-1", "credit-2", "credit-3"])),
            0
        );
        // A list longer than the reported count is itself the count.
        let longer = ["credit-1", "credit-2", "credit-3", "credit-4"];
        assert_eq!(banked.observe(&usage(Some(1), &longer)), 1);
        assert_eq!(banked.available_count(), 4);
    }

    #[test]
    fn resets_added_while_the_app_was_closed_are_detected() {
        let mut banked = BankedResets::default();
        banked.observe(&usage(Some(1), &[]));

        let mut after_restart = restarted(&banked);
        assert_eq!(after_restart.observe(&usage(Some(2), &[])), 1);

        // Saved state prevents the same notification after another restart.
        let directory = TempDir::new("banked");
        let path = directory.file("banked-reset-state.json");
        after_restart.save(&path).unwrap();
        let mut loaded = BankedResets::load(&path);
        assert_eq!(loaded, after_restart);
        assert_eq!(loaded.observe(&usage(Some(2), &[])), 0);
    }

    #[test]
    fn state_is_written_in_the_existing_format() {
        let mut banked = BankedResets::default();
        assert_eq!(
            banked.to_json(),
            r#"{"Version":1,"HasObservation":false,"AvailableCount":0,"ItemizedListWasComplete":false,"ObservedCreditIds":[]}"#
        );
        banked.observe(&usage(Some(2), &["credit-b", "credit-\"a\""]));
        assert_eq!(
            banked.to_json(),
            r#"{"Version":1,"HasObservation":true,"AvailableCount":2,"ItemizedListWasComplete":true,"ObservedCreditIds":["credit-\"a\"","credit-b"]}"#
        );
        assert_eq!(restarted(&banked), banked);
    }

    #[test]
    fn existing_state_files_are_read() {
        let saved = r#"{"Version":1,"HasObservation":true,"AvailableCount":2,
            "ItemizedListWasComplete":true,
            "ObservedCreditIds":["RateLimitResetCredit_21c3","RateLimitResetCredit_3a42"," ",null]}"#;
        let mut banked = BankedResets::from_json(saved).unwrap();
        assert_eq!(banked.available_count(), 2);
        let known = ["RateLimitResetCredit_21c3", "RateLimitResetCredit_3a42"];
        assert_eq!(banked.observe(&usage(Some(2), &known)), 0);
        let replaced = ["RateLimitResetCredit_3a42", "RateLimitResetCredit_646"];
        assert_eq!(banked.observe(&usage(Some(2), &replaced)), 1);

        // Members the C# class would default may be absent.
        let minimal = BankedResets::from_json(r#"{"HasObservation":true}"#).unwrap();
        assert!(minimal.observed && minimal.available_count == 0);
        // Without an observation nothing else in the file counts.
        let unobserved =
            r#"{"Version":1,"HasObservation":false,"AvailableCount":5,"ObservedCreditIds":["a"]}"#;
        assert_eq!(
            BankedResets::from_json(unobserved),
            Some(BankedResets::default())
        );
    }

    #[test]
    fn unusable_state_starts_from_a_quiet_baseline() {
        for invalid in [
            "invalid",
            "null",
            "[]",
            r#"{"Version":2,"HasObservation":true,"AvailableCount":1}"#,
            r#"{"Version":1,"HasObservation":true,"AvailableCount":-1}"#,
            r#"{"Version":1,"HasObservation":true,"AvailableCount":3000000000}"#,
            r#"{"Version":1,"HasObservation":"yes","AvailableCount":1}"#,
            r#"{"Version":1,"HasObservation":true,"ObservedCreditIds":"credit-1"}"#,
        ] {
            assert_eq!(BankedResets::from_json(invalid), None, "{invalid}");
        }

        let directory = TempDir::new("banked-invalid");
        let path = directory.file("banked-reset-state.json");
        assert_eq!(BankedResets::load(&path), BankedResets::default());
        std::fs::write(&path, "invalid").unwrap();
        let mut banked = BankedResets::load(&path);
        assert_eq!(banked.observe(&usage(Some(4), &[])), 0);
    }
}
