use crate::settings::IconMetric;

pub const FIVE_HOUR_MINUTES: u32 = 300;
pub const WEEKLY_MINUTES: u32 = 10_080;

#[derive(Clone, Debug, PartialEq)]
pub struct LimitWindow {
    pub used_percent: f64,
    pub window_minutes: u32,
    /// Unix seconds at which the window resets, when the provider reports it.
    pub resets_at: Option<i64>,
}

impl LimitWindow {
    pub fn remaining_percent(&self) -> i32 {
        (100.0 - self.used_percent)
            .clamp(0.0, 100.0)
            .round_ties_even() as i32
    }
}

/// A banked reset the account can still use.
#[derive(Clone, Debug, PartialEq)]
pub struct ResetCredit {
    pub id: Option<String>,
    pub reset_type: Option<String>,
    pub title: Option<String>,
    /// Unix seconds at which the credit expires, when known.
    pub expires_at: Option<i64>,
}

/// Limits for an auxiliary model, reported alongside the account's own.
#[derive(Clone, Debug, PartialEq)]
pub struct LimitSet {
    pub limit_id: String,
    pub display_name: String,
    pub five_hour: Option<LimitWindow>,
    pub weekly: Option<LimitWindow>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Usage {
    /// Unix seconds of the successful reading.
    pub updated_at: i64,
    pub five_hour: Option<LimitWindow>,
    pub weekly: Option<LimitWindow>,
    /// Auxiliary limits, ordered by display name.
    pub additional_limits: Vec<LimitSet>,
    /// Available banked resets, soonest expiry first.
    pub resets: Vec<ResetCredit>,
    pub reset_count: Option<usize>,
    pub plan_type: Option<String>,
}

/// The result of asking Codex to use a banked reset.
#[derive(Clone, Debug, PartialEq)]
pub enum RedemptionOutcome {
    Reset,
    AlreadyRedeemed,
    NothingToReset,
    NoCredit,
    /// The request failed; the message is suitable for display.
    Failed(String),
}

impl Usage {
    /// A reading that carries only the two primary windows.
    pub fn from_windows(
        updated_at: i64,
        five_hour: Option<LimitWindow>,
        weekly: Option<LimitWindow>,
    ) -> Self {
        Self {
            updated_at,
            five_hour,
            weekly,
            additional_limits: Vec::new(),
            resets: Vec::new(),
            reset_count: None,
            plan_type: None,
        }
    }

    /// The window shown on the tray icon. `Auto` picks whichever has less
    /// remaining, preferring weekly on a tie; an explicit choice falls back to
    /// the other window when its own is unavailable. The flag is true for the
    /// weekly window.
    pub fn icon_window(&self, metric: IconMetric) -> Option<(&LimitWindow, bool)> {
        let weekly = self.weekly.as_ref().map(|window| (window, true));
        let five_hour = self.five_hour.as_ref().map(|window| (window, false));
        match metric {
            IconMetric::Weekly => weekly.or(five_hour),
            IconMetric::FiveHour => five_hour.or(weekly),
            IconMetric::Auto => match (weekly, five_hour) {
                (Some(weekly), Some(five_hour))
                    if five_hour.0.used_percent > weekly.0.used_percent =>
                {
                    Some(five_hour)
                }
                (weekly, five_hour) => weekly.or(five_hour),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(used_percent: f64, window_minutes: u32, resets_at: Option<i64>) -> LimitWindow {
        LimitWindow {
            used_percent,
            window_minutes,
            resets_at,
        }
    }

    fn usage(weekly: Option<f64>, five_hour: Option<f64>) -> Usage {
        Usage::from_windows(
            0,
            five_hour.map(|used| window(used, FIVE_HOUR_MINUTES, None)),
            weekly.map(|used| window(used, WEEKLY_MINUTES, None)),
        )
    }

    #[test]
    fn remaining_percent_is_clamped_and_rounded() {
        assert_eq!(window(28.0, WEEKLY_MINUTES, None).remaining_percent(), 72);
        assert_eq!(window(27.5, WEEKLY_MINUTES, None).remaining_percent(), 72);
        assert_eq!(window(140.0, WEEKLY_MINUTES, None).remaining_percent(), 0);
        assert_eq!(window(-3.0, WEEKLY_MINUTES, None).remaining_percent(), 100);
    }

    #[test]
    fn icon_window_follows_the_configured_metric() {
        let is_weekly = |usage: Usage, metric| usage.icon_window(metric).map(|(_, weekly)| weekly);
        // Auto shows whichever limit has less remaining, preferring weekly on a tie.
        assert_eq!(
            is_weekly(usage(Some(20.0), Some(60.0)), IconMetric::Auto),
            Some(false)
        );
        assert_eq!(
            is_weekly(usage(Some(90.0), Some(60.0)), IconMetric::Auto),
            Some(true)
        );
        assert_eq!(
            is_weekly(usage(Some(90.0), Some(90.0)), IconMetric::Auto),
            Some(true)
        );
        assert_eq!(
            is_weekly(usage(None, Some(90.0)), IconMetric::Auto),
            Some(false)
        );
        // Explicit choices are kept, falling back when their window is missing.
        assert_eq!(
            is_weekly(usage(Some(20.0), Some(60.0)), IconMetric::Weekly),
            Some(true)
        );
        assert_eq!(
            is_weekly(usage(Some(90.0), Some(60.0)), IconMetric::FiveHour),
            Some(false)
        );
        assert_eq!(
            is_weekly(usage(None, Some(90.0)), IconMetric::Weekly),
            Some(false)
        );
        assert_eq!(
            is_weekly(usage(Some(10.0), None), IconMetric::FiveHour),
            Some(true)
        );
        assert_eq!(is_weekly(usage(None, None), IconMetric::Auto), None);
    }
}
