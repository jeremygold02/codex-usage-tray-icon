//! Automatic use of a banked reset that is about to expire: which credit
//! qualifies, and the attempt record and idempotency key that keep a retry
//! from spending a second credit.

use std::fmt::Write;
use std::ptr::null_mut;

use windows_sys::Win32::Security::Cryptography::{
    BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
};

use super::known_expiry;
use crate::model::{RedemptionOutcome, ResetCredit, Usage};

/// How long to wait before repeating a check or an attempt that did not settle.
pub(super) const RETRY_SECONDS: i64 = 60;
const MIN_LEAD_MINUTES: i32 = 1;
const MAX_LEAD_MINUTES: i32 = 120;
/// The reset type Codex gives credits for the account's own limits.
const SUPPORTED_RESET_TYPE: &str = "codexRateLimits";

/// How far the last redemption attempt got. Saved in settings as
/// `AutoRedeemAttemptStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptStatus {
    /// The request was sent, or was about to be, and its result is unknown.
    Pending,
    /// Codex found no usage to reset; the credit is still available.
    NothingToReset,
    /// The credit is spent or gone, so it must not be tried again.
    Completed,
}

impl AttemptStatus {
    /// The text the C# app saves for this status.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::NothingToReset => "nothingToReset",
            Self::Completed => "completed",
        }
    }

    /// Reads a saved status. Anything unrecognized means no status.
    pub fn parse(text: &str) -> Option<Self> {
        [Self::Pending, Self::NothingToReset, Self::Completed]
            .into_iter()
            .find(|status| status.as_str() == text)
    }
}

/// The last redemption attempt, as saved in settings so that a restart
/// neither repeats a finished redemption nor retries one under a new key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AutoRedeemAttempt {
    /// `AutoRedeemCreditId`.
    pub credit_id: Option<String>,
    /// `AutoRedeemIdempotencyKey`.
    pub idempotency_key: Option<String>,
    /// `AutoRedeemAttemptStatus`.
    pub status: Option<AttemptStatus>,
    /// `AutoRedeemLastAttemptUtc`, in Unix seconds.
    pub last_attempt_utc: Option<i64>,
}

impl AutoRedeemAttempt {
    /// The credit that needs no further attempt.
    pub(super) fn completed_credit_id(&self) -> Option<&str> {
        self.credit_id
            .as_deref()
            .filter(|_| self.status == Some(AttemptStatus::Completed))
    }
}

/// A redemption to carry out with `codex::consume_reset_credit`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RedemptionPlan {
    /// The credit to redeem.
    pub credit_id: String,
    /// Sent with the request; a repeat under the same key spends nothing more.
    pub idempotency_key: String,
    /// The attempt record to save before the request is sent, so that a crash
    /// or restart in between retries under the same key.
    pub attempt: AutoRedeemAttempt,
}

impl RedemptionPlan {
    /// The attempt record once the request has returned.
    pub(super) fn record(&self, outcome: &RedemptionOutcome, now: i64) -> AutoRedeemAttempt {
        let status = match outcome {
            RedemptionOutcome::Reset
            | RedemptionOutcome::AlreadyRedeemed
            | RedemptionOutcome::NoCredit => AttemptStatus::Completed,
            RedemptionOutcome::NothingToReset => AttemptStatus::NothingToReset,
            // The request may still have reached Codex, so a retry keeps the key.
            RedemptionOutcome::Failed(_) => AttemptStatus::Pending,
        };
        AutoRedeemAttempt {
            status: Some(status),
            last_attempt_utc: Some(now),
            ..self.attempt.clone()
        }
    }
}

/// The result of preparing a redemption for an eligible credit.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Preparation {
    Ready(RedemptionPlan),
    /// The same credit was attempted too recently; try again at this time.
    RetryAt(i64),
    /// No idempotency key could be generated.
    KeyUnavailable,
}

/// Decides how to redeem `credit_id` given the last saved attempt.
pub(super) fn prepare(
    credit_id: &str,
    attempt: &AutoRedeemAttempt,
    now: i64,
    new_key: &mut dyn FnMut() -> Option<String>,
) -> Preparation {
    let same_credit = attempt.credit_id.as_deref() == Some(credit_id);
    if same_credit {
        let retry_at = attempt
            .last_attempt_utc
            .map(|last| last.saturating_add(RETRY_SECONDS));
        if let Some(retry_at) = retry_at.filter(|retry_at| now < *retry_at) {
            return Preparation::RetryAt(retry_at);
        }
    }

    // Only an attempt whose result is unknown keeps its key: repeating that
    // request cannot spend a second credit.
    let pending_key = attempt
        .idempotency_key
        .as_deref()
        .filter(|_| same_credit && attempt.status == Some(AttemptStatus::Pending))
        .filter(|key| !key.trim().is_empty());
    let Some(idempotency_key) = pending_key.map(str::to_string).or_else(new_key) else {
        return Preparation::KeyUnavailable;
    };
    Preparation::Ready(RedemptionPlan {
        credit_id: credit_id.to_string(),
        idempotency_key: idempotency_key.clone(),
        attempt: AutoRedeemAttempt {
            credit_id: Some(credit_id.to_string()),
            idempotency_key: Some(idempotency_key),
            status: Some(AttemptStatus::Pending),
            last_attempt_utc: Some(now),
        },
    })
}

/// The ID of the credit to redeem now: the soonest to expire within
/// `lead_minutes`, and only while there is usage for it to reset.
pub(super) fn find_eligible_credit<'a>(
    usage: &'a Usage,
    now: i64,
    lead_minutes: i32,
    excluded_credit_id: Option<&str>,
) -> Option<&'a str> {
    if !has_usage_to_reset(usage) {
        return None;
    }
    let deadline = now.saturating_add(lead_seconds(lead_minutes));
    usage
        .resets
        .iter()
        .filter_map(|credit| usable_expiry(credit, now, excluded_credit_id))
        .filter(|(_, expiry)| *expiry <= deadline)
        // The first of several credits expiring together is the one chosen.
        .min_by_key(|(_, expiry)| *expiry)
        .map(|(id, _)| id)
}

/// When the next redeemable credit comes within `lead_minutes` of expiring,
/// or `now` if one already has.
pub(super) fn next_check(
    usage: &Usage,
    now: i64,
    lead_minutes: i32,
    excluded_credit_id: Option<&str>,
) -> Option<i64> {
    let lead = lead_seconds(lead_minutes);
    usage
        .resets
        .iter()
        .filter_map(|credit| usable_expiry(credit, now, excluded_credit_id))
        .map(|(_, expiry)| (expiry - lead).max(now))
        .min()
}

fn has_usage_to_reset(usage: &Usage) -> bool {
    [&usage.weekly, &usage.five_hour]
        .into_iter()
        .flatten()
        .any(|window| window.used_percent > 0.0)
}

/// The ID and expiry of a credit this app may redeem by itself.
fn usable_expiry<'a>(
    credit: &'a ResetCredit,
    now: i64,
    excluded_credit_id: Option<&str>,
) -> Option<(&'a str, i64)> {
    let id = credit.id.as_deref().filter(|id| !id.trim().is_empty())?;
    let expiry = known_expiry(credit)?;
    let supported = credit
        .reset_type
        .as_deref()
        .is_some_and(is_supported_reset_type);
    (supported && excluded_credit_id != Some(id) && expiry > now).then_some((id, expiry))
}

/// Accepts the type in camel, snake or kebab case, as the C# app does.
fn is_supported_reset_type(reset_type: &str) -> bool {
    let normalized: String = reset_type
        .chars()
        .filter(|character| !matches!(character, '_' | '-'))
        .collect();
    normalized.eq_ignore_ascii_case(SUPPORTED_RESET_TYPE)
}

fn lead_seconds(lead_minutes: i32) -> i64 {
    i64::from(lead_minutes.clamp(MIN_LEAD_MINUTES, MAX_LEAD_MINUTES)) * 60
}

/// A random version 4 UUID in the hyphenated lowercase form the C# app uses
/// for idempotency keys, or `None` if the system has no random numbers to give.
pub fn new_idempotency_key() -> Option<String> {
    let mut bytes = [0u8; 16];
    // SAFETY: the pointer and length describe `bytes`, and the flag requires
    // the null algorithm handle.
    let status = unsafe {
        BCryptGenRandom(
            null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    (status >= 0).then(|| format_uuid_v4(bytes))
}

fn format_uuid_v4(mut bytes: [u8; 16]) -> String {
    bytes[6] = (bytes[6] & 0x0F) | 0x40;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    let mut text = String::with_capacity(36);
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            text.push('-');
        }
        // Writing to a String cannot fail.
        let _ = write!(text, "{byte:02x}");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{LimitWindow, FIVE_HOUR_MINUTES, WEEKLY_MINUTES};

    const NOW: i64 = 1_785_081_600;
    const MINUTE: i64 = 60;

    fn credit(id: &str, reset_type: &str, expires_at: i64) -> ResetCredit {
        ResetCredit {
            id: Some(id.to_string()),
            reset_type: Some(reset_type.to_string()),
            title: None,
            expires_at: Some(expires_at),
        }
    }

    fn usage(weekly_remaining: i32, five_hour_remaining: i32, resets: Vec<ResetCredit>) -> Usage {
        let window = |remaining: i32, window_minutes| {
            Some(LimitWindow {
                used_percent: f64::from(100 - remaining),
                window_minutes,
                resets_at: None,
            })
        };
        let mut usage = Usage::from_windows(
            NOW,
            window(five_hour_remaining, FIVE_HOUR_MINUTES),
            window(weekly_remaining, WEEKLY_MINUTES),
        );
        usage.resets = resets;
        usage
    }

    fn attempt(credit_id: &str, key: &str, status: AttemptStatus, last: i64) -> AutoRedeemAttempt {
        AutoRedeemAttempt {
            credit_id: Some(credit_id.to_string()),
            idempotency_key: Some(key.to_string()),
            status: Some(status),
            last_attempt_utc: Some(last),
        }
    }

    fn new_key() -> Option<String> {
        Some("new-key".to_string())
    }

    #[test]
    fn the_soonest_supported_credit_inside_the_lead_time_is_eligible() {
        let usage = usage(
            75,
            100,
            vec![
                credit("unsupported", "otherReset", NOW + MINUTE),
                credit("outside-window", "codexRateLimits", NOW + 6 * MINUTE),
                credit("eligible", "codex_rate_limits", NOW + 4 * MINUTE),
            ],
        );
        assert_eq!(find_eligible_credit(&usage, NOW, 5, None), Some("eligible"));
        // A credit whose redemption completed is not selected again.
        assert_eq!(find_eligible_credit(&usage, NOW, 5, Some("eligible")), None);
        // The next check uses the configured lead time.
        assert_eq!(
            next_check(&usage, NOW, 5, Some("eligible")),
            Some(NOW + MINUTE)
        );
        assert_eq!(next_check(&usage, NOW, 5, None), Some(NOW));
    }

    #[test]
    fn full_usage_does_not_consume_a_reset() {
        let resets = vec![credit("unused", "codexRateLimits", NOW + MINUTE)];
        let full = usage(100, 100, resets.clone());
        assert_eq!(find_eligible_credit(&full, NOW, 5, None), None);
        // The credit is still watched in case usage appears before it expires.
        assert_eq!(next_check(&full, NOW, 5, None), Some(NOW));
        let five_hour_used = usage(100, 99, resets);
        assert_eq!(
            find_eligible_credit(&five_hour_used, NOW, 5, None),
            Some("unused")
        );
    }

    #[test]
    fn unusable_credits_are_never_eligible() {
        let nameless = ResetCredit {
            id: None,
            ..credit("", "codexRateLimits", NOW + MINUTE)
        };
        let undated = ResetCredit {
            expires_at: None,
            ..credit("undated", "codexRateLimits", 0)
        };
        let untyped = ResetCredit {
            reset_type: None,
            ..credit("untyped", "", NOW + MINUTE)
        };
        let usage = usage(
            50,
            50,
            vec![
                nameless,
                undated,
                untyped,
                credit(" ", "codexRateLimits", NOW + MINUTE),
                credit("expired", "codexRateLimits", NOW),
                // Beyond the last date the C# app can represent.
                credit("endless", "codexRateLimits", 253_402_300_800),
            ],
        );
        assert_eq!(find_eligible_credit(&usage, NOW, 120, None), None);
        assert_eq!(next_check(&usage, NOW, 120, None), None);
    }

    #[test]
    fn ties_and_lead_limits_follow_the_list_order() {
        let usage = usage(
            50,
            100,
            vec![
                credit("first", "CODEX-RATE-LIMITS", NOW + 90),
                credit("second", "codexRateLimits", NOW + 90),
                credit("far", "codexRateLimits", NOW + 121 * MINUTE),
            ],
        );
        // A lead below one minute counts as one minute, above 120 as 120.
        assert_eq!(find_eligible_credit(&usage, NOW, 0, None), None);
        assert_eq!(find_eligible_credit(&usage, NOW, 2, None), Some("first"));
        assert_eq!(
            find_eligible_credit(&usage, NOW, 2, Some("first")),
            Some("second")
        );
        let excluded = Some("first");
        assert_eq!(next_check(&usage, NOW, 0, excluded), Some(NOW + 30));
        let only_far = Usage {
            resets: usage.resets[2..].to_vec(),
            ..usage
        };
        assert_eq!(find_eligible_credit(&only_far, NOW, 500, None), None);
        assert_eq!(next_check(&only_far, NOW, 500, None), Some(NOW + MINUTE));
    }

    #[test]
    fn a_new_credit_gets_a_new_key_and_a_pending_record() {
        let earlier = attempt("other", "old-key", AttemptStatus::Pending, NOW - 5);
        let plan = RedemptionPlan {
            credit_id: "credit-1".to_string(),
            idempotency_key: "new-key".to_string(),
            attempt: attempt("credit-1", "new-key", AttemptStatus::Pending, NOW),
        };
        assert_eq!(
            prepare("credit-1", &earlier, NOW, &mut new_key),
            Preparation::Ready(plan.clone())
        );
        let none = AutoRedeemAttempt::default();
        assert_eq!(
            prepare("credit-1", &none, NOW, &mut new_key),
            Preparation::Ready(plan)
        );
        assert_eq!(
            prepare("credit-1", &none, NOW, &mut || None),
            Preparation::KeyUnavailable
        );
    }

    #[test]
    fn a_repeated_attempt_waits_and_keeps_only_a_pending_key() {
        let pending = attempt("credit-1", "old-key", AttemptStatus::Pending, NOW);
        assert_eq!(
            prepare("credit-1", &pending, NOW + 59, &mut new_key),
            Preparation::RetryAt(NOW + 60)
        );
        let key_after = |earlier: &AutoRedeemAttempt| match prepare(
            "credit-1",
            earlier,
            NOW + 60,
            &mut new_key,
        ) {
            Preparation::Ready(plan) => plan.idempotency_key,
            other => panic!("expected a plan, got {other:?}"),
        };
        assert_eq!(key_after(&pending), "old-key");

        // Codex answered the last request, so the next one is a new request.
        let answered = attempt("credit-1", "old-key", AttemptStatus::NothingToReset, NOW);
        assert_eq!(key_after(&answered), "new-key");
        let unknown_status = AutoRedeemAttempt {
            status: None,
            ..pending.clone()
        };
        assert_eq!(key_after(&unknown_status), "new-key");
        let blank_key = AutoRedeemAttempt {
            idempotency_key: Some(" ".to_string()),
            ..pending.clone()
        };
        assert_eq!(key_after(&blank_key), "new-key");

        // Without a recorded time there is nothing to wait for.
        let untimed = AutoRedeemAttempt {
            last_attempt_utc: None,
            ..pending
        };
        assert!(matches!(
            prepare("credit-1", &untimed, NOW, &mut new_key),
            Preparation::Ready(plan) if plan.idempotency_key == "old-key"
        ));
    }

    #[test]
    fn outcomes_are_recorded_against_the_attempted_credit() {
        let plan = RedemptionPlan {
            credit_id: "credit-1".to_string(),
            idempotency_key: "key-1".to_string(),
            attempt: attempt("credit-1", "key-1", AttemptStatus::Pending, NOW),
        };
        let recorded = |outcome: RedemptionOutcome| {
            let record = plan.record(&outcome, NOW + 3);
            assert_eq!(
                record,
                AutoRedeemAttempt {
                    status: record.status,
                    last_attempt_utc: Some(NOW + 3),
                    ..plan.attempt.clone()
                }
            );
            record.status
        };
        let completed = Some(AttemptStatus::Completed);
        assert_eq!(recorded(RedemptionOutcome::Reset), completed);
        assert_eq!(recorded(RedemptionOutcome::AlreadyRedeemed), completed);
        assert_eq!(recorded(RedemptionOutcome::NoCredit), completed);
        assert_eq!(
            recorded(RedemptionOutcome::NothingToReset),
            Some(AttemptStatus::NothingToReset)
        );
        assert_eq!(
            recorded(RedemptionOutcome::Failed("timed out".to_string())),
            Some(AttemptStatus::Pending)
        );
    }

    #[test]
    fn only_a_completed_attempt_excludes_its_credit() {
        let completed = attempt("credit-1", "key-1", AttemptStatus::Completed, NOW);
        assert_eq!(completed.completed_credit_id(), Some("credit-1"));
        let pending = attempt("credit-1", "key-1", AttemptStatus::Pending, NOW);
        assert_eq!(pending.completed_credit_id(), None);
        assert_eq!(AutoRedeemAttempt::default().completed_credit_id(), None);
    }

    #[test]
    fn statuses_use_the_saved_spellings() {
        for (status, text) in [
            (AttemptStatus::Pending, "pending"),
            (AttemptStatus::NothingToReset, "nothingToReset"),
            (AttemptStatus::Completed, "completed"),
        ] {
            assert_eq!(status.as_str(), text);
            assert_eq!(AttemptStatus::parse(text), Some(status));
        }
        // The C# app compares the saved text exactly.
        assert_eq!(AttemptStatus::parse("Completed"), None);
        assert_eq!(AttemptStatus::parse(""), None);
    }

    #[test]
    fn idempotency_keys_are_version_4_uuids() {
        assert_eq!(
            format_uuid_v4([0xFF; 16]),
            "ffffffff-ffff-4fff-bfff-ffffffffffff"
        );
        assert_eq!(
            format_uuid_v4([0x00; 16]),
            "00000000-0000-4000-8000-000000000000"
        );

        let key = new_idempotency_key().unwrap();
        let groups: Vec<&str> = key.split('-').collect();
        assert_eq!(
            groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
            [8, 4, 4, 4, 12]
        );
        assert!(key
            .chars()
            .all(|character| matches!(character, '0'..='9' | 'a'..='f' | '-')));
        assert!(groups[2].starts_with('4'));
        assert!(matches!(groups[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'));
        assert_ne!(new_idempotency_key().unwrap(), key);
    }
}
