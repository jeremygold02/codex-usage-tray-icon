//! Notices when a weekly or 5-hour limit returns to 100% remaining.

use crate::model::{LimitWindow, Usage};

const FULL_REMAINING_PERCENT: i32 = 100;
/// At or below this, usage has clearly resumed and one reading re-arms detection.
const IMMEDIATE_REARM_PERCENT: i32 = 98;
/// A 99% reading alternates with 100% right after a reset, so it has to
/// repeat this often before it counts as usage.
const STABLE_BELOW_FULL_OBSERVATIONS: u32 = 2;

/// The limits seen returning to 100% in one reading.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct LimitResets {
    pub weekly: bool,
    pub five_hour: bool,
}

impl LimitResets {
    pub fn any(self) -> bool {
        self.weekly || self.five_hour
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct WindowState {
    observed: bool,
    /// Usage has been seen below 100% since the last reset was reported.
    armed: bool,
    below_full_observations: u32,
}

impl WindowState {
    /// True when the window is back at 100% after usage was seen below it.
    fn observe(&mut self, window: Option<&LimitWindow>) -> bool {
        // A window missing from one reading keeps what was learned before it.
        let Some(window) = window else {
            return false;
        };
        let remaining = window.remaining_percent();
        let below_full = remaining < FULL_REMAINING_PERCENT;
        if !self.observed {
            // Starting at 100% says nothing about a reset.
            self.observed = true;
            self.armed = below_full;
            self.below_full_observations = u32::from(below_full);
            return false;
        }
        if below_full {
            self.below_full_observations = self.below_full_observations.saturating_add(1);
            if remaining <= IMMEDIATE_REARM_PERCENT
                || self.below_full_observations >= STABLE_BELOW_FULL_OBSERVATIONS
            {
                self.armed = true;
            }
            return false;
        }
        self.below_full_observations = 0;
        std::mem::take(&mut self.armed)
    }
}

/// Detects limit resets from successive successful readings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct LimitResetDetector {
    weekly: WindowState,
    five_hour: WindowState,
}

impl LimitResetDetector {
    pub fn observe(&mut self, usage: &Usage) -> LimitResets {
        LimitResets {
            weekly: self.weekly.observe(usage.weekly.as_ref()),
            five_hour: self.five_hour.observe(usage.five_hour.as_ref()),
        }
    }
}

/// Withholds limit-reset notifications for a while after the app caused the
/// reset itself by redeeming a credit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ResetSuppression {
    until: Option<i64>,
}

impl ResetSuppression {
    /// Suppresses notifications up to and including `until`, never shortening
    /// a suppression already in effect.
    pub fn suppress_until(&mut self, until: i64) {
        self.until = Some(self.until.map_or(until, |current| current.max(until)));
    }

    pub fn filter(&mut self, detected: LimitResets, now: i64) -> LimitResets {
        match self.until {
            Some(until) if now <= until => LimitResets::default(),
            _ => {
                self.until = None;
                detected
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FIVE_HOUR_MINUTES, WEEKLY_MINUTES};

    const WEEKLY: LimitResets = LimitResets {
        weekly: true,
        five_hour: false,
    };
    const FIVE_HOUR: LimitResets = LimitResets {
        weekly: false,
        five_hour: true,
    };
    const BOTH: LimitResets = LimitResets {
        weekly: true,
        five_hour: true,
    };
    const NONE: LimitResets = LimitResets {
        weekly: false,
        five_hour: false,
    };

    fn usage(weekly_remaining: Option<i32>, five_hour_remaining: Option<i32>) -> Usage {
        let window = |remaining: Option<i32>, window_minutes| {
            remaining.map(|remaining| LimitWindow {
                used_percent: f64::from(100 - remaining),
                window_minutes,
                resets_at: None,
            })
        };
        Usage::from_windows(
            0,
            window(five_hour_remaining, FIVE_HOUR_MINUTES),
            window(weekly_remaining, WEEKLY_MINUTES),
        )
    }

    #[test]
    fn starting_at_full_usage_is_silent() {
        let mut detector = LimitResetDetector::default();
        assert_eq!(detector.observe(&usage(Some(100), Some(100))), NONE);
        assert_eq!(detector.observe(&usage(Some(100), Some(100))), NONE);
        // One 99% sample in the startup cycle is treated as jitter.
        detector.observe(&usage(Some(99), Some(100)));
        assert_eq!(detector.observe(&usage(Some(100), Some(100))), NONE);
    }

    #[test]
    fn windows_reset_independently_and_only_once() {
        let mut detector = LimitResetDetector::default();
        // The first reading below 100% only establishes a baseline.
        assert_eq!(detector.observe(&usage(Some(72), Some(45))), NONE);
        assert_eq!(detector.observe(&usage(Some(100), Some(40))), WEEKLY);
        assert_eq!(detector.observe(&usage(Some(100), Some(40))), NONE);
        assert_eq!(detector.observe(&usage(Some(100), Some(100))), FIVE_HOUR);
    }

    #[test]
    fn simultaneous_resets_are_combined() {
        let mut detector = LimitResetDetector::default();
        detector.observe(&usage(Some(60), Some(70)));
        assert_eq!(detector.observe(&usage(Some(100), Some(100))), BOTH);
        assert!(BOTH.any() && WEEKLY.any() && FIVE_HOUR.any() && !NONE.any());
    }

    #[test]
    fn a_missing_window_keeps_its_state() {
        let mut detector = LimitResetDetector::default();
        detector.observe(&usage(Some(80), None));
        detector.observe(&usage(None, None));
        assert_eq!(detector.observe(&usage(Some(100), None)), WEEKLY);
    }

    #[test]
    fn jitter_between_99_and_100_does_not_repeat_the_reset() {
        let mut detector = LimitResetDetector::default();
        detector.observe(&usage(Some(50), None));
        assert_eq!(detector.observe(&usage(Some(100), None)), WEEKLY);
        // One 99% sample does not re-arm detection.
        assert_eq!(detector.observe(&usage(Some(99), None)), NONE);
        assert_eq!(detector.observe(&usage(Some(100), None)), NONE);
        // Staying below 100% does.
        detector.observe(&usage(Some(99), None));
        detector.observe(&usage(Some(99), None));
        assert_eq!(detector.observe(&usage(Some(100), None)), WEEKLY);
    }

    #[test]
    fn usage_at_98_percent_rearms_immediately() {
        let mut detector = LimitResetDetector::default();
        detector.observe(&usage(Some(50), None));
        detector.observe(&usage(Some(100), None));
        detector.observe(&usage(Some(98), None));
        assert_eq!(detector.observe(&usage(Some(100), None)), WEEKLY);
    }

    #[test]
    fn suppression_hides_resets_until_it_expires() {
        let started = 1_785_109_337;
        let minutes = |count: i64| started + count * 60;
        let mut suppression = ResetSuppression::default();
        suppression.suppress_until(minutes(30));

        assert_eq!(suppression.filter(NONE, minutes(1)), NONE);
        assert_eq!(suppression.filter(WEEKLY, minutes(2)), NONE);
        assert_eq!(suppression.filter(FIVE_HOUR, minutes(3)), NONE);
        assert_eq!(suppression.filter(WEEKLY, minutes(30)), NONE);
        assert_eq!(suppression.filter(WEEKLY, minutes(31)), WEEKLY);
        // Expiry is remembered, so an earlier clock reading cannot revive it.
        assert_eq!(suppression.filter(WEEKLY, minutes(2)), WEEKLY);
    }

    #[test]
    fn a_later_suppression_never_shortens_an_earlier_one() {
        let mut suppression = ResetSuppression::default();
        suppression.suppress_until(1_000);
        suppression.suppress_until(400);
        assert_eq!(suppression.filter(BOTH, 900), NONE);
        assert_eq!(suppression.filter(BOTH, 1_001), BOTH);
    }

    #[test]
    fn unsuppressed_resets_pass_through() {
        let mut suppression = ResetSuppression::default();
        assert_eq!(suppression.filter(BOTH, 0), BOTH);
    }
}
