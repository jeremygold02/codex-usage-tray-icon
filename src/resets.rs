//! Usage notifications and banked-reset handling as a decision engine.
//!
//! The tray tells a [`ResetEngine`] what happened: a refresh finished or
//! failed, a timer fired, a redemption returned. The engine answers with a
//! [`Step`] listing the notifications to show, the timers to arm, the state
//! files to write and, when an expiring reset should be used, the request to
//! make. The decisions never read the clock, the settings file or the tray:
//! times are Unix seconds supplied by the caller.

mod banked;
mod limit_resets;
pub mod redemption;
mod reminders;
mod storage;

use std::path::Path;

use crate::model::{LimitWindow, RedemptionOutcome, ResetCredit, Usage};
use crate::timefmt::{format_date_time, format_duration};

use banked::BankedResets;
use limit_resets::{LimitResetDetector, LimitResets, ResetSuppression};
use redemption::{AutoRedeemAttempt, Preparation, RedemptionPlan, RETRY_SECONDS};
use reminders::ExpiryReminders;

/// Seconds from 0001-01-01, where .NET's `DateTime` begins, to the Unix epoch.
const DOTNET_EPOCH_SECONDS: i64 = 62_135_596_800;
/// 9999-12-31T23:59:59Z, the last second a .NET `DateTime` can hold.
const DOTNET_LAST_SECOND: i64 = 253_402_300_799;

/// How soon to look again when a timer fires while a refresh or redemption
/// is still running.
const BUSY_RETRY_SECONDS: i64 = 5;
/// After a failed refresh a due reminder is retried no sooner than this.
const REMINDER_RETRY_SECONDS: i64 = 300;
/// A redemption returns the limits to 100%, which is then not worth a
/// notification of its own.
const REDEMPTION_QUIET_SECONDS: i64 = 30 * 60;
const MIN_TIMER_MILLISECONDS: i64 = 1_000;
/// The longest delay a Windows timer accepts.
const MAX_TIMER_MILLISECONDS: i64 = i32::MAX as i64;

const REFRESH_FAILED_MESSAGE: &str = "Unable to refresh usage. Try again.";
const REDEMPTION_FAILED_MESSAGE: &str = "The automatic reset request failed.";
const KEY_UNAVAILABLE_MESSAGE: &str = "Could not create a redemption request key.";

/// The icon shown with a notification, as the C# app's `ToolTipIcon`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationKind {
    /// Something happened that needs no action.
    Info,
    /// Something needs attention or went wrong.
    Warning,
}

/// A tray notification to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    /// The heading, which names the kind of event.
    pub title: String,
    /// The message. Threshold alerts for both limits span two lines.
    pub text: String,
    /// The icon to show beside it.
    pub kind: NotificationKind,
}

impl Notification {
    fn info(title: &str, text: impl Into<String>) -> Self {
        Self {
            title: title.to_string(),
            text: text.into(),
            kind: NotificationKind::Info,
        }
    }

    fn warning(title: &str, text: impl Into<String>) -> Self {
        Self {
            title: title.to_string(),
            text: text.into(),
            kind: NotificationKind::Warning,
        }
    }
}

/// The settings the engine reads, named after the members of the C# app's
/// `settings.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResetSettings {
    /// `ThresholdNotifications`.
    pub threshold_notifications: bool,
    /// `CriticalThreshold`: remaining percent at or below which usage is critical.
    pub critical_threshold: i32,
    /// `LowThreshold`: remaining percent at or below which usage is low.
    pub low_threshold: i32,
    /// `ResetExpiryReminders`.
    pub reset_expiry_reminders: bool,
    /// `ResetExpiryLeadHours`; values outside 1 to 72 are brought into range.
    pub reset_expiry_lead_hours: i32,
    /// `AutoRedeemResetCredits`.
    pub auto_redeem_reset_credits: bool,
    /// `AutoRedeemLeadMinutes`; values outside 1 to 120 are brought into range.
    pub auto_redeem_lead_minutes: i32,
    /// The four saved `AutoRedeem...` attempt members.
    pub auto_redeem_attempt: AutoRedeemAttempt,
}

impl Default for ResetSettings {
    /// The C# app's defaults.
    fn default() -> Self {
        Self {
            threshold_notifications: false,
            critical_threshold: 15,
            low_threshold: 25,
            reset_expiry_reminders: true,
            reset_expiry_lead_hours: 24,
            auto_redeem_reset_credits: false,
            auto_redeem_lead_minutes: 5,
            auto_redeem_attempt: AutoRedeemAttempt::default(),
        }
    }
}

impl ResetSettings {
    /// True when one refresh is needed at startup even though automatic
    /// checks are paused, so that expiring credits are noticed.
    pub fn needs_startup_refresh(&self) -> bool {
        self.auto_redeem_reset_credits || self.reset_expiry_reminders
    }
}

/// What to do with one of the tray's one-shot timers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TimerChange {
    /// Leave the timer as it is.
    #[default]
    Keep,
    /// Stop the timer.
    Cancel,
    /// Stop the timer and start it again to fire at this Unix time.
    FireAt(i64),
}

/// What the tray has to do after an event, in this order: show the
/// notifications, write the state files, apply the timer changes, start the
/// redemption, start the refresh.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Step {
    /// Notifications in the order the C# app raises them; where only one can
    /// be visible, the last one wins.
    pub notifications: Vec<Notification>,
    /// The timer that revalidates credits when a reminder comes due.
    pub reminder_timer: TimerChange,
    /// The timer that looks for a credit to redeem.
    pub redemption_timer: TimerChange,
    /// A redemption to carry out. Its attempt record has to be saved first;
    /// see [`ResetEngine::redemption_not_saved`] for when that fails.
    pub redemption: Option<RedemptionPlan>,
    /// Start a refresh now, without manual-refresh feedback.
    pub refresh: bool,
    /// The banked-reset state changed; see [`ResetEngine::save`].
    pub save_banked_resets: bool,
    /// The reminder state changed; see [`ResetEngine::save`].
    pub save_reminders: bool,
}

/// Converts a time from a [`TimerChange`] into a delay a Windows timer accepts.
pub fn timer_delay_milliseconds(fire_at: i64, now: i64) -> u32 {
    fire_at
        .saturating_sub(now)
        .saturating_mul(1_000)
        .clamp(MIN_TIMER_MILLISECONDS, MAX_TIMER_MILLISECONDS) as u32
}

/// How close a limit is to running out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ThresholdLevel {
    Normal,
    Low,
    Critical,
}

impl ThresholdLevel {
    fn of(window: Option<&LimitWindow>, settings: &ResetSettings) -> Self {
        match window.map(LimitWindow::remaining_percent) {
            Some(remaining) if remaining <= settings.critical_threshold => Self::Critical,
            Some(remaining) if remaining <= settings.low_threshold => Self::Low,
            _ => Self::Normal,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ThresholdLevels {
    weekly: ThresholdLevel,
    five_hour: ThresholdLevel,
}

impl ThresholdLevels {
    fn of(usage: &Usage, settings: &ResetSettings) -> Self {
        Self {
            weekly: ThresholdLevel::of(usage.weekly.as_ref(), settings),
            five_hour: ThresholdLevel::of(usage.five_hour.as_ref(), settings),
        }
    }
}

/// Everything the engine remembers between events. Only the banked resets
/// and the reminders given outlive the process.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResetEngine {
    banked: BankedResets,
    reminders: ExpiryReminders,
    limit_resets: LimitResetDetector,
    suppression: ResetSuppression,
    /// `None` until a reading sets the baseline that later ones are compared with.
    threshold_levels: Option<ThresholdLevels>,
    /// The credit and message of the last redemption error shown, so that a
    /// retry failing the same way stays quiet.
    last_redemption_error: Option<String>,
    /// The redemption handed to the tray that has not reported back yet.
    in_flight: Option<RedemptionPlan>,
    refresh_after_redemption: bool,
}

impl ResetEngine {
    /// Restores the state saved in `banked-reset-state.json` and
    /// `reset-reminders.json`. A missing or damaged file starts that part
    /// afresh, which for banked resets means a quiet baseline.
    pub fn load(banked_path: &Path, reminders_path: &Path) -> Self {
        Self {
            banked: BankedResets::load(banked_path),
            reminders: ExpiryReminders::load(reminders_path),
            ..Self::default()
        }
    }

    /// Writes the state files `step` marked as changed.
    pub fn save(&self, step: &Step, banked_path: &Path, reminders_path: &Path) {
        // A failed write is not reported: the state in memory still prevents
        // repeats, and the worst outcome is one repeated notification after
        // a restart.
        if step.save_banked_resets {
            let _ = self.banked.save(banked_path);
        }
        if step.save_reminders {
            let _ = self.reminders.save(reminders_path);
        }
    }

    /// Call before starting any refresh. Returns false while a redemption is
    /// running: the refresh must wait, and [`Step::refresh`] asks for it once
    /// the redemption has finished.
    pub fn begin_refresh(&mut self) -> bool {
        self.refresh_after_redemption |= self.in_flight.is_some();
        self.in_flight.is_none()
    }

    /// Handles a successful reading. `manual` is true when the user asked for
    /// this refresh from the tray menu and expects a notification in reply.
    pub fn refresh_succeeded(
        &mut self,
        usage: &Usage,
        settings: &ResetSettings,
        now: i64,
        manual: bool,
        new_key: &mut dyn FnMut() -> Option<String>,
    ) -> Step {
        let mut step = Step {
            save_banked_resets: true,
            ..Step::default()
        };
        let limit_resets = self
            .suppression
            .filter(self.limit_resets.observe(usage), now);
        let banked_added = self.banked.observe(usage);

        let threshold_alert = self.observe_thresholds(usage, settings);
        if threshold_alert.is_none() && manual && !limit_resets.any() && banked_added == 0 {
            // With no other news, a manual refresh is answered with the readings.
            let summary = Notification::info("Codex Usage", usage_summary(usage));
            step.notifications.push(summary);
        }
        step.notifications.extend(threshold_alert);
        step.notifications
            .extend(limit_reset_notification(limit_resets));
        step.notifications.extend(banked_notification(
            banked_added,
            self.banked.available_count(),
        ));

        if settings.reset_expiry_reminders {
            let observation = self
                .reminders
                .observe(usage, now, settings.reset_expiry_lead_hours);
            step.save_reminders = observation.changed;
            step.notifications
                .extend(expiry_notification(&observation.due, now));
        }
        step.reminder_timer = self.reminder_check(Some(usage), settings, now, false);

        let attempt = &settings.auto_redeem_attempt;
        self.evaluate_redemption(Some(usage), settings, attempt, now, new_key, &mut step);
        step
    }

    /// Handles a failed refresh. `message` is the failure as shown to the
    /// user and `last_usage` the last successful reading, if there is one.
    pub fn refresh_failed(
        &self,
        message: &str,
        last_usage: Option<&Usage>,
        settings: &ResetSettings,
        now: i64,
        manual: bool,
    ) -> Step {
        let mut step = Step::default();
        if manual {
            let text = match message.trim() {
                "" => REFRESH_FAILED_MESSAGE,
                text => text,
            };
            step.notifications
                .push(Notification::warning("Codex Usage", text));
        }
        let attempt = &settings.auto_redeem_attempt;
        step.redemption_timer = self.redemption_check(last_usage, settings, attempt, now);
        step.reminder_timer = self.reminder_check(last_usage, settings, now, true);
        step
    }

    /// Handles the reminder timer firing.
    pub fn reminder_timer_fired(
        &self,
        settings: &ResetSettings,
        refresh_in_progress: bool,
        now: i64,
    ) -> Step {
        let mut step = Step {
            reminder_timer: TimerChange::Cancel,
            ..Step::default()
        };
        if settings.reset_expiry_reminders {
            if refresh_in_progress || self.in_flight.is_some() {
                step.reminder_timer = TimerChange::FireAt(now.saturating_add(BUSY_RETRY_SECONDS));
            } else {
                // The credit is revalidated before any reminder is shown,
                // even while automatic checks are paused.
                step.refresh = true;
            }
        }
        step
    }

    /// Handles the redemption timer firing.
    pub fn redemption_timer_fired(
        &self,
        settings: &ResetSettings,
        refresh_in_progress: bool,
        now: i64,
    ) -> Step {
        let mut step = Step {
            redemption_timer: TimerChange::Cancel,
            ..Step::default()
        };
        // A running redemption looks again by itself when it finishes.
        if settings.auto_redeem_reset_credits && self.in_flight.is_none() {
            if refresh_in_progress {
                step.redemption_timer = TimerChange::FireAt(now.saturating_add(BUSY_RETRY_SECONDS));
            } else {
                step.refresh = true;
            }
        }
        step
    }

    /// Handles a failure to save the attempt record of the plan just issued.
    /// The redemption must not be sent: without the saved key, a restart
    /// could spend a second credit.
    pub fn redemption_not_saved(&mut self, error: &str, now: i64) -> Step {
        let mut step = Step::default();
        let Some(plan) = self.in_flight.take() else {
            return step;
        };
        let message = format!("Could not save redemption state: {error}");
        step.notifications
            .extend(self.redemption_error(&plan.credit_id, &message));
        step.redemption_timer = TimerChange::FireAt(now.saturating_add(RETRY_SECONDS));
        step
    }

    /// The attempt record for the running redemption given its outcome, or
    /// `None` when no redemption is running. The tray saves it, keeps it as
    /// the current attempt whether or not the save worked, and then calls
    /// [`Self::redemption_finished`].
    pub fn redemption_record(
        &self,
        outcome: &RedemptionOutcome,
        now: i64,
    ) -> Option<AutoRedeemAttempt> {
        self.in_flight
            .as_ref()
            .map(|plan| plan.record(outcome, now))
    }

    /// Folds the outcome of the running redemption into the state.
    /// `record_error` is why saving [`Self::redemption_record`] failed, if it
    /// did, and `last_usage` the last successful reading.
    pub fn redemption_finished(
        &mut self,
        outcome: &RedemptionOutcome,
        record_error: Option<&str>,
        last_usage: Option<&Usage>,
        settings: &ResetSettings,
        now: i64,
        new_key: &mut dyn FnMut() -> Option<String>,
    ) -> Step {
        let mut step = Step::default();
        let Some(plan) = self.in_flight.take() else {
            return step;
        };
        if let Some(error) = record_error {
            let message = format!("Could not update redemption state: {error}");
            step.notifications
                .extend(self.redemption_error(&plan.credit_id, &message));
        }

        let mut refresh = std::mem::take(&mut self.refresh_after_redemption);
        match outcome {
            RedemptionOutcome::Reset => {
                self.redemption_succeeded(now);
                step.notifications.push(Notification::info(
                    "Codex Limit Reset Used",
                    "An expiring limit reset was used automatically.",
                ));
                refresh = true;
            }
            RedemptionOutcome::AlreadyRedeemed => {
                self.redemption_succeeded(now);
                step.notifications.push(Notification::info(
                    "Codex Limit Reset",
                    "The expiring reset had already been redeemed.",
                ));
                refresh = true;
            }
            RedemptionOutcome::NoCredit => {
                self.last_redemption_error = None;
                refresh = true;
            }
            RedemptionOutcome::NothingToReset => {}
            RedemptionOutcome::Failed(message) => {
                step.notifications
                    .extend(self.redemption_error(&plan.credit_id, message));
            }
        }
        if refresh {
            // The reading that follows decides what happens next.
            step.refresh = true;
            return step;
        }

        // The settings passed in may still hold the record from before the
        // request, so the outcome just recorded is used instead.
        let attempt = plan.record(outcome, now);
        self.evaluate_redemption(last_usage, settings, &attempt, now, new_key, &mut step);
        step
    }

    /// Handles saved settings. Call it after the tray has applied its own
    /// refresh schedule, with `refresh_in_progress` as it stands then.
    pub fn settings_changed(
        &mut self,
        last_usage: Option<&Usage>,
        settings: &ResetSettings,
        refresh_in_progress: bool,
        now: i64,
    ) -> Step {
        // New thresholds must not raise an alert for a level already reached.
        self.threshold_levels = last_usage.map(|usage| ThresholdLevels::of(usage, settings));

        let mut step = Step {
            reminder_timer: self.reminder_check(last_usage, settings, now, false),
            ..Step::default()
        };
        let idle = !refresh_in_progress && self.in_flight.is_none();
        if !settings.auto_redeem_reset_credits {
            step.redemption_timer = TimerChange::Cancel;
        } else if refresh_in_progress && self.in_flight.is_none() {
            step.redemption_timer = TimerChange::FireAt(now.saturating_add(BUSY_RETRY_SECONDS));
        }
        // Either feature may just have been turned on, so credits are checked now.
        step.refresh = idle && settings.needs_startup_refresh();
        step
    }

    /// Updates the threshold baseline and returns an alert when a limit has
    /// dropped to a worse level than in the previous reading.
    fn observe_thresholds(
        &mut self,
        usage: &Usage,
        settings: &ResetSettings,
    ) -> Option<Notification> {
        let current = ThresholdLevels::of(usage, settings);
        // The levels are tracked while alerts are off, so turning them on
        // does not announce a level reached earlier.
        let previous = self.threshold_levels.replace(current)?;
        if !settings.threshold_notifications {
            return None;
        }
        let windows = [
            ("Weekly", &usage.weekly, previous.weekly, current.weekly),
            (
                "5h",
                &usage.five_hour,
                previous.five_hour,
                current.five_hour,
            ),
        ];
        let worsened: Vec<(&str, &LimitWindow, ThresholdLevel)> = windows
            .into_iter()
            .filter(|(_, _, before, after)| after > before)
            .filter_map(|(label, window, _, level)| Some((label, window.as_ref()?, level)))
            .collect();
        let highest = worsened.iter().map(|(_, _, level)| *level).max()?;

        let lines: Vec<String> = worsened
            .iter()
            .map(|(label, window, level)| {
                let severity = match level {
                    ThresholdLevel::Critical => "critical",
                    _ => "low",
                };
                let remaining = window.remaining_percent();
                format!("{label} usage is {severity}: {remaining}% remaining.")
            })
            .collect();
        let title = match highest {
            ThresholdLevel::Critical => "Codex Usage Critical",
            _ => "Codex Usage Low",
        };
        Some(Notification::warning(title, lines.join("\r\n")))
    }

    /// When the reminder timer should fire next. After a failed refresh
    /// (`retry`) a reminder that is already due is not retried at once.
    fn reminder_check(
        &self,
        usage: Option<&Usage>,
        settings: &ResetSettings,
        now: i64,
        retry: bool,
    ) -> TimerChange {
        if !settings.reset_expiry_reminders {
            return TimerChange::Cancel;
        }
        let lead_hours = settings.reset_expiry_lead_hours;
        let soonest = now.saturating_add(if retry { REMINDER_RETRY_SECONDS } else { 1 });
        usage
            .and_then(|usage| self.reminders.next_check(usage, now, lead_hours))
            .map_or(TimerChange::Cancel, |check| {
                TimerChange::FireAt(check.max(soonest))
            })
    }

    /// When the redemption timer should fire next.
    fn redemption_check(
        &self,
        usage: Option<&Usage>,
        settings: &ResetSettings,
        attempt: &AutoRedeemAttempt,
        now: i64,
    ) -> TimerChange {
        if !settings.auto_redeem_reset_credits || self.in_flight.is_some() {
            return TimerChange::Cancel;
        }
        let retry = TimerChange::FireAt(now.saturating_add(RETRY_SECONDS));
        let Some(usage) = usage else {
            // Nothing has been read yet, so the credits are still unknown.
            return retry;
        };
        let lead_minutes = settings.auto_redeem_lead_minutes;
        match redemption::next_check(usage, now, lead_minutes, attempt.completed_credit_id()) {
            // A credit inside its lead time that was not redeemed, for want
            // of usage to reset, is looked at again until it expires.
            Some(check) if check <= now => retry,
            Some(check) => TimerChange::FireAt(check),
            None => TimerChange::Cancel,
        }
    }

    /// Plans the redemption of an expiring credit, or sets the redemption
    /// timer for the next time one could be due.
    fn evaluate_redemption(
        &mut self,
        usage: Option<&Usage>,
        settings: &ResetSettings,
        attempt: &AutoRedeemAttempt,
        now: i64,
        new_key: &mut dyn FnMut() -> Option<String>,
        step: &mut Step,
    ) {
        step.redemption_timer = TimerChange::Cancel;
        let Some(usage) = usage.filter(|_| settings.auto_redeem_reset_credits) else {
            return;
        };
        if self.in_flight.is_some() {
            return;
        }
        let lead_minutes = settings.auto_redeem_lead_minutes;
        let completed = attempt.completed_credit_id();
        let Some(credit_id) = redemption::find_eligible_credit(usage, now, lead_minutes, completed)
        else {
            step.redemption_timer = self.redemption_check(Some(usage), settings, attempt, now);
            return;
        };
        match redemption::prepare(credit_id, attempt, now, new_key) {
            Preparation::Ready(plan) => {
                self.in_flight = Some(plan.clone());
                step.redemption = Some(plan);
            }
            Preparation::RetryAt(retry_at) => {
                step.redemption_timer = TimerChange::FireAt(retry_at);
            }
            Preparation::KeyUnavailable => {
                step.notifications
                    .extend(self.redemption_error(credit_id, KEY_UNAVAILABLE_MESSAGE));
                step.redemption_timer = TimerChange::FireAt(now.saturating_add(RETRY_SECONDS));
            }
        }
    }

    fn redemption_succeeded(&mut self, now: i64) {
        self.last_redemption_error = None;
        self.suppression
            .suppress_until(now.saturating_add(REDEMPTION_QUIET_SECONDS));
    }

    /// The notification for a redemption error, unless the same error for
    /// the same credit was the last one shown.
    fn redemption_error(&mut self, credit_id: &str, message: &str) -> Option<Notification> {
        let message = match message.trim() {
            "" => REDEMPTION_FAILED_MESSAGE,
            message => message,
        };
        let error = format!("{credit_id}|{message}");
        if self.last_redemption_error.as_deref() == Some(error.as_str()) {
            return None;
        }
        self.last_redemption_error = Some(error);
        Some(Notification::warning("Codex Automatic Reset", message))
    }
}

/// Returns the time if the C# app could represent it. That app treats a time
/// outside this range as missing; staying inside it also keeps the arithmetic
/// on times read from Codex from overflowing.
fn representable(unix: i64) -> Option<i64> {
    (-DOTNET_EPOCH_SECONDS..=DOTNET_LAST_SECOND)
        .contains(&unix)
        .then_some(unix)
}

fn known_expiry(credit: &ResetCredit) -> Option<i64> {
    credit.expires_at.and_then(representable)
}

/// The readings in one line, as a manual refresh reports them.
fn usage_summary(usage: &Usage) -> String {
    let describe = |label: &str, window: &LimitWindow| {
        let reset = window
            .resets_at
            .and_then(representable)
            .map(|resets_at| {
                // Counted from the reading, like the C# app's reset countdown.
                let seconds = resets_at
                    .saturating_sub(usage.updated_at)
                    .clamp(0, i64::from(i32::MAX));
                format!(", resets {}", format_duration(seconds))
            })
            .unwrap_or_default();
        format!("{label} {}% left{reset}", window.remaining_percent())
    };
    let windows: Vec<String> = [("Weekly", &usage.weekly), ("5h", &usage.five_hour)]
        .into_iter()
        .filter_map(|(label, window)| Some(describe(label, window.as_ref()?)))
        .collect();
    if windows.is_empty() {
        "Usage unavailable".to_string()
    } else {
        windows.join(" | ")
    }
}

fn limit_reset_notification(resets: LimitResets) -> Option<Notification> {
    let text = match (resets.weekly, resets.five_hour) {
        (true, true) => "Weekly and 5-hour usage are back to 100%.",
        (true, false) => "Weekly usage is back to 100%.",
        (false, true) => "5-hour usage is back to 100%.",
        (false, false) => return None,
    };
    Some(Notification::info("Codex Usage Reset", text))
}

fn banked_notification(added: usize, available: usize) -> Option<Notification> {
    let added = match added {
        0 => return None,
        1 => "A new limit reset was added.".to_string(),
        count => format!("{count} new limit resets were added."),
    };
    let noun = if available == 1 {
        "banked reset is"
    } else {
        "banked resets are"
    };
    Some(Notification::info(
        "Codex Limit Reset Banked",
        format!("{added} {available} {noun} now available."),
    ))
}

/// One reminder for all the credits that came due in the same reading.
fn expiry_notification(due: &[i64], now: i64) -> Option<Notification> {
    let earliest = format_date_time(*due.iter().min()?, now);
    let text = match due.len() {
        1 => format!("A banked reset expires {earliest}."),
        count => format!("{count} banked resets expire soon. Earliest: {earliest}."),
    };
    Some(Notification::warning("Codex Reset Expiring", text))
}

#[cfg(test)]
mod tests {
    use super::redemption::AttemptStatus;
    use super::storage::tests::TempDir;
    use super::*;
    use crate::model::{FIVE_HOUR_MINUTES, WEEKLY_MINUTES};
    use crate::paths::{BANKED_RESETS_FILE, RESET_REMINDERS_FILE};

    const NOW: i64 = 1_783_700_000;
    const MINUTE: i64 = 60;
    const HOUR: i64 = 3_600;

    fn usage(weekly_remaining: i32, five_hour_remaining: i32) -> Usage {
        let window = |remaining: i32, window_minutes| {
            Some(LimitWindow {
                used_percent: f64::from(100 - remaining),
                window_minutes,
                resets_at: None,
            })
        };
        Usage::from_windows(
            NOW,
            window(five_hour_remaining, FIVE_HOUR_MINUTES),
            window(weekly_remaining, WEEKLY_MINUTES),
        )
    }

    fn credit(id: &str, expires_at: i64) -> ResetCredit {
        ResetCredit {
            id: Some(id.to_string()),
            reset_type: Some("codexRateLimits".to_string()),
            title: Some("Full reset (Weekly + 5 hr)".to_string()),
            expires_at: Some(expires_at),
        }
    }

    fn with_resets(mut usage: Usage, resets: Vec<ResetCredit>) -> Usage {
        usage.reset_count = Some(resets.len());
        usage.resets = resets;
        usage
    }

    fn with_reset_count(mut usage: Usage, count: usize) -> Usage {
        usage.reset_count = Some(count);
        usage
    }

    /// Settings with every optional behavior turned off.
    fn quiet() -> ResetSettings {
        ResetSettings {
            reset_expiry_reminders: false,
            ..ResetSettings::default()
        }
    }

    fn alerting() -> ResetSettings {
        ResetSettings {
            threshold_notifications: true,
            ..quiet()
        }
    }

    fn redeeming() -> ResetSettings {
        ResetSettings {
            auto_redeem_reset_credits: true,
            ..quiet()
        }
    }

    /// Usage with one credit that is four minutes from expiring.
    fn expiring() -> Usage {
        with_resets(usage(60, 80), vec![credit("credit-1", NOW + 4 * MINUTE)])
    }

    fn no_key() -> Option<String> {
        None
    }

    fn numbered_keys() -> impl FnMut() -> Option<String> {
        let mut issued = 0;
        move || {
            issued += 1;
            Some(format!("key-{issued}"))
        }
    }

    fn refresh(
        engine: &mut ResetEngine,
        usage: &Usage,
        settings: &ResetSettings,
    ) -> Vec<Notification> {
        engine
            .refresh_succeeded(usage, settings, NOW, false, &mut no_key)
            .notifications
    }

    fn attempt(credit_id: &str, key: &str, status: AttemptStatus, last: i64) -> AutoRedeemAttempt {
        AutoRedeemAttempt {
            credit_id: Some(credit_id.to_string()),
            idempotency_key: Some(key.to_string()),
            status: Some(status),
            last_attempt_utc: Some(last),
        }
    }

    /// Starts a redemption of `expiring()`'s credit and returns the settings
    /// as they are once the tray has saved the plan's attempt record.
    fn start_redemption(
        engine: &mut ResetEngine,
        new_key: &mut dyn FnMut() -> Option<String>,
    ) -> ResetSettings {
        let mut settings = redeeming();
        let step = engine.refresh_succeeded(&expiring(), &settings, NOW, false, new_key);
        settings.auto_redeem_attempt = step.redemption.unwrap().attempt;
        settings
    }

    fn finish(
        engine: &mut ResetEngine,
        outcome: &RedemptionOutcome,
        settings: &ResetSettings,
        now: i64,
    ) -> Step {
        let usage = expiring();
        engine.redemption_finished(outcome, None, Some(&usage), settings, now, &mut no_key)
    }

    fn failed(message: &str) -> RedemptionOutcome {
        RedemptionOutcome::Failed(message.to_string())
    }

    #[test]
    fn defaults_match_the_existing_app() {
        let defaults = ResetSettings::default();
        assert!(!defaults.threshold_notifications && !defaults.auto_redeem_reset_credits);
        assert!(defaults.reset_expiry_reminders);
        assert_eq!(
            (defaults.critical_threshold, defaults.low_threshold),
            (15, 25)
        );
        assert_eq!(defaults.reset_expiry_lead_hours, 24);
        assert_eq!(defaults.auto_redeem_lead_minutes, 5);
        assert_eq!(defaults.auto_redeem_attempt, AutoRedeemAttempt::default());

        // Either credit feature needs one reading at startup.
        assert!(defaults.needs_startup_refresh());
        assert!(redeeming().needs_startup_refresh());
        assert!(!quiet().needs_startup_refresh());
    }

    #[test]
    fn the_first_reading_only_sets_baselines() {
        let mut engine = ResetEngine::default();
        // Already critical, already full, and with resets banked.
        let first = with_reset_count(usage(10, 100), 3);
        let step = engine.refresh_succeeded(&first, &alerting(), NOW, false, &mut no_key);
        assert_eq!(
            step,
            Step {
                reminder_timer: TimerChange::Cancel,
                redemption_timer: TimerChange::Cancel,
                save_banked_resets: true,
                ..Step::default()
            }
        );
    }

    #[test]
    fn a_worse_threshold_level_alerts_once() {
        let mut engine = ResetEngine::default();
        let settings = alerting();
        let mut alerts = |weekly: i32, five_hour: i32| {
            refresh(&mut engine, &usage(weekly, five_hour), &settings)
        };
        assert!(alerts(50, 50).is_empty());
        assert_eq!(
            alerts(25, 50),
            [Notification::warning(
                "Codex Usage Low",
                "Weekly usage is low: 25% remaining."
            )]
        );
        assert!(alerts(20, 50).is_empty());
        assert_eq!(
            alerts(15, 20),
            [Notification::warning(
                "Codex Usage Critical",
                "Weekly usage is critical: 15% remaining.\r\n5h usage is low: 20% remaining."
            )]
        );
        // Recovering is silent; dropping again alerts again.
        assert!(alerts(80, 80).is_empty());
        assert_eq!(
            alerts(80, 10),
            [Notification::warning(
                "Codex Usage Critical",
                "5h usage is critical: 10% remaining."
            )]
        );
    }

    #[test]
    fn threshold_levels_are_tracked_while_alerts_are_off() {
        let mut engine = ResetEngine::default();
        assert!(refresh(&mut engine, &usage(50, 50), &quiet()).is_empty());
        assert!(refresh(&mut engine, &usage(10, 50), &quiet()).is_empty());
        // Turning alerts on does not announce the level reached before.
        assert!(refresh(&mut engine, &usage(10, 50), &alerting()).is_empty());
        assert_eq!(
            refresh(&mut engine, &usage(10, 20), &alerting()),
            [Notification::warning(
                "Codex Usage Low",
                "5h usage is low: 20% remaining."
            )]
        );
    }

    #[test]
    fn saved_settings_restart_the_threshold_baseline() {
        let mut engine = ResetEngine::default();
        refresh(&mut engine, &usage(50, 50), &alerting());
        // With thresholds raised, the last reading is already low.
        let raised = ResetSettings {
            low_threshold: 60,
            ..alerting()
        };
        let step = engine.settings_changed(Some(&usage(50, 50)), &raised, false, NOW);
        assert_eq!(
            step,
            Step {
                reminder_timer: TimerChange::Cancel,
                redemption_timer: TimerChange::Cancel,
                ..Step::default()
            }
        );
        assert!(refresh(&mut engine, &usage(50, 50), &raised).is_empty());
        assert_eq!(refresh(&mut engine, &usage(50, 10), &raised).len(), 1);

        // Without a reading, the next one is the baseline.
        engine.settings_changed(None, &raised, false, NOW);
        assert!(refresh(&mut engine, &usage(10, 10), &raised).is_empty());
    }

    #[test]
    fn saved_settings_check_credits_when_a_credit_feature_is_on() {
        let mut engine = ResetEngine::default();
        let reminding = ResetSettings::default();
        let last = with_resets(usage(60, 80), vec![credit("credit-1", NOW + 30 * HOUR)]);

        let step = engine.settings_changed(Some(&last), &reminding, false, NOW);
        assert_eq!(
            step,
            Step {
                reminder_timer: TimerChange::FireAt(NOW + 6 * HOUR),
                redemption_timer: TimerChange::Cancel,
                refresh: true,
                ..Step::default()
            }
        );
        // A refresh already running will do the checking.
        assert!(
            !engine
                .settings_changed(Some(&last), &reminding, true, NOW)
                .refresh
        );

        let idle = engine.settings_changed(Some(&last), &redeeming(), false, NOW);
        assert!(idle.refresh);
        assert_eq!(idle.redemption_timer, TimerChange::Keep);
        let busy = engine.settings_changed(Some(&last), &redeeming(), true, NOW);
        assert!(!busy.refresh);
        assert_eq!(busy.redemption_timer, TimerChange::FireAt(NOW + 5));

        // A running redemption checks again by itself when it finishes.
        start_redemption(&mut engine, &mut numbered_keys());
        let redeeming_now = engine.settings_changed(Some(&last), &redeeming(), false, NOW);
        assert!(!redeeming_now.refresh);
        assert_eq!(redeeming_now.redemption_timer, TimerChange::Keep);
    }

    #[test]
    fn limit_resets_are_announced_by_window() {
        let reset = |text: &str| [Notification::info("Codex Usage Reset", text)];
        let mut engine = ResetEngine::default();
        assert!(refresh(&mut engine, &usage(60, 70), &quiet()).is_empty());
        assert_eq!(
            refresh(&mut engine, &usage(100, 70), &quiet()),
            reset("Weekly usage is back to 100%.")
        );
        assert_eq!(
            refresh(&mut engine, &usage(100, 100), &quiet()),
            reset("5-hour usage is back to 100%.")
        );
        assert!(refresh(&mut engine, &usage(100, 100), &quiet()).is_empty());

        let mut engine = ResetEngine::default();
        refresh(&mut engine, &usage(60, 70), &quiet());
        assert_eq!(
            refresh(&mut engine, &usage(100, 100), &quiet()),
            reset("Weekly and 5-hour usage are back to 100%.")
        );
    }

    #[test]
    fn a_manual_refresh_is_answered_with_the_readings() {
        let manual = |engine: &mut ResetEngine, usage: &Usage, settings: &ResetSettings| {
            engine
                .refresh_succeeded(usage, settings, NOW + 30, true, &mut no_key)
                .notifications
        };
        let summary = |text: &str| [Notification::info("Codex Usage", text)];

        let mut reading = usage(72, 55);
        if let Some(weekly) = reading.weekly.as_mut() {
            weekly.resets_at = Some(NOW + 2 * 24 * HOUR);
        }
        let mut engine = ResetEngine::default();
        assert_eq!(
            manual(&mut engine, &reading, &quiet()),
            summary("Weekly 72% left, resets 2d 0h | 5h 55% left")
        );
        assert!(refresh(&mut engine, &reading, &quiet()).is_empty());

        // A reset time already passed counts down to nothing.
        let mut overdue = Usage::from_windows(NOW, reading.five_hour.clone(), None);
        if let Some(five_hour) = overdue.five_hour.as_mut() {
            five_hour.resets_at = Some(NOW - MINUTE);
        }
        assert_eq!(
            manual(&mut ResetEngine::default(), &overdue, &quiet()),
            summary("5h 55% left, resets 0s")
        );
        let nothing = Usage::from_windows(NOW, None, None);
        assert_eq!(
            manual(&mut ResetEngine::default(), &nothing, &quiet()),
            summary("Usage unavailable")
        );
    }

    #[test]
    fn other_news_replaces_the_manual_refresh_summary() {
        let titles = |engine: &mut ResetEngine, usage: &Usage| -> Vec<String> {
            engine
                .refresh_succeeded(usage, &alerting(), NOW, true, &mut no_key)
                .notifications
                .into_iter()
                .map(|notification| notification.title)
                .collect()
        };
        let mut engine = ResetEngine::default();
        let baseline = with_reset_count(usage(50, 50), 1);
        assert_eq!(titles(&mut engine, &baseline), ["Codex Usage"]);

        let low = with_reset_count(usage(20, 50), 1);
        assert_eq!(titles(&mut engine, &low), ["Codex Usage Low"]);
        let reset = with_reset_count(usage(100, 50), 1);
        assert_eq!(titles(&mut engine, &reset), ["Codex Usage Reset"]);
        let banked = with_reset_count(usage(100, 50), 2);
        assert_eq!(titles(&mut engine, &banked), ["Codex Limit Reset Banked"]);

        // Several kinds of news keep the order of the existing app.
        let everything = with_reset_count(usage(100, 10), 3);
        refresh(
            &mut engine,
            &with_reset_count(usage(60, 50), 2),
            &alerting(),
        );
        assert_eq!(
            titles(&mut engine, &everything),
            [
                "Codex Usage Critical",
                "Codex Usage Reset",
                "Codex Limit Reset Banked"
            ]
        );
    }

    #[test]
    fn a_failed_manual_refresh_is_reported() {
        let engine = ResetEngine::default();
        let report = |message: &str, manual: bool| {
            engine
                .refresh_failed(message, None, &quiet(), NOW, manual)
                .notifications
        };
        assert_eq!(
            report(" Codex is not signed in. Run codex login. ", true),
            [Notification::warning(
                "Codex Usage",
                "Codex is not signed in. Run codex login."
            )]
        );
        assert_eq!(
            report(" ", true),
            [Notification::warning(
                "Codex Usage",
                "Unable to refresh usage. Try again."
            )]
        );
        assert!(report("Codex app-server could not be started.", false).is_empty());
    }

    #[test]
    fn new_banked_resets_are_announced_with_the_total() {
        let banked = |text: &str| [Notification::info("Codex Limit Reset Banked", text)];
        let mut engine = ResetEngine::default();
        let mut observe = |count: usize| {
            refresh(
                &mut engine,
                &with_reset_count(usage(80, 70), count),
                &quiet(),
            )
        };
        assert!(observe(0).is_empty());
        assert_eq!(
            observe(1),
            banked("A new limit reset was added. 1 banked reset is now available.")
        );
        assert_eq!(
            observe(2),
            banked("A new limit reset was added. 2 banked resets are now available.")
        );
        assert_eq!(
            observe(5),
            banked("3 new limit resets were added. 5 banked resets are now available.")
        );
        // Using resets is not news.
        assert!(observe(1).is_empty());
    }

    #[test]
    fn expiring_credits_are_reminded_once_each() {
        let reminder = |text: String| [Notification::warning("Codex Reset Expiring", text)];
        let settings = ResetSettings::default();
        let reading = with_resets(
            usage(80, 70),
            vec![
                credit("credit-1", NOW + 5 * HOUR),
                credit("credit-2", NOW + 3 * HOUR),
                credit("credit-3", NOW + 30 * HOUR),
            ],
        );
        let mut engine = ResetEngine::default();
        let step = engine.refresh_succeeded(&reading, &settings, NOW, false, &mut no_key);
        let earliest = format_date_time(NOW + 3 * HOUR, NOW);
        assert_eq!(
            step.notifications,
            reminder(format!(
                "2 banked resets expire soon. Earliest: {earliest}."
            ))
        );
        assert!(step.save_reminders);
        assert_eq!(step.reminder_timer, TimerChange::FireAt(NOW + 6 * HOUR));

        let repeat = engine.refresh_succeeded(&reading, &settings, NOW, false, &mut no_key);
        assert!(repeat.notifications.is_empty() && !repeat.save_reminders);

        // The third credit comes due when its timer fires.
        let later = NOW + 6 * HOUR;
        let step = engine.refresh_succeeded(&reading, &settings, later, false, &mut no_key);
        let expiry = format_date_time(NOW + 30 * HOUR, later);
        assert_eq!(
            step.notifications,
            reminder(format!("A banked reset expires {expiry}."))
        );
        assert!(step.save_reminders);
        assert_eq!(step.reminder_timer, TimerChange::Cancel);
    }

    #[test]
    fn reminders_can_be_turned_off() {
        let reading = with_resets(usage(80, 70), vec![credit("credit-1", NOW + HOUR)]);
        let mut engine = ResetEngine::default();
        let step = engine.refresh_succeeded(&reading, &quiet(), NOW, false, &mut no_key);
        assert!(step.notifications.is_empty() && !step.save_reminders);
        assert_eq!(step.reminder_timer, TimerChange::Cancel);
        // Nothing was recorded, so turning them on later still reminds.
        let on = ResetSettings::default();
        assert_eq!(refresh(&mut engine, &reading, &on).len(), 1);
    }

    #[test]
    fn a_reminder_left_over_is_checked_again_a_second_later() {
        // More credits than the reminder cache holds, all due at once.
        let credits = (0..2_049)
            .map(|index| credit(&format!("credit-{index}"), NOW + HOUR))
            .collect();
        let reading = with_resets(usage(80, 70), credits);
        let mut engine = ResetEngine::default();
        let settings = ResetSettings::default();
        let step = engine.refresh_succeeded(&reading, &settings, NOW, false, &mut no_key);
        assert_eq!(step.reminder_timer, TimerChange::FireAt(NOW + 1));
    }

    #[test]
    fn a_failed_refresh_schedules_from_the_last_reading() {
        let engine = ResetEngine::default();
        let settings = ResetSettings {
            reset_expiry_reminders: true,
            ..redeeming()
        };
        let schedule = |last: Option<&Usage>, settings: &ResetSettings| {
            let step = engine.refresh_failed("failed", last, settings, NOW, false);
            (step.reminder_timer, step.redemption_timer)
        };

        let mut last = with_resets(usage(80, 70), vec![credit("credit-1", NOW + 30 * HOUR)]);
        assert_eq!(
            schedule(Some(&last), &settings),
            (
                TimerChange::FireAt(NOW + 6 * HOUR),
                TimerChange::FireAt(NOW + 30 * HOUR - 5 * MINUTE)
            )
        );
        // A reminder already due is retried after five minutes, not at once.
        last.resets[0].expires_at = Some(NOW + HOUR);
        assert_eq!(
            schedule(Some(&last), &settings),
            (
                TimerChange::FireAt(NOW + 5 * MINUTE),
                TimerChange::FireAt(NOW + 55 * MINUTE)
            )
        );
        // Before any reading, only the redemption check knows to try again.
        assert_eq!(
            schedule(None, &settings),
            (TimerChange::Cancel, TimerChange::FireAt(NOW + MINUTE))
        );
        assert_eq!(
            schedule(Some(&last), &quiet()),
            (TimerChange::Cancel, TimerChange::Cancel)
        );
    }

    #[test]
    fn state_survives_a_restart() {
        let directory = TempDir::new("engine");
        let banked_path = directory.file(BANKED_RESETS_FILE);
        let reminders_path = directory.file(RESET_REMINDERS_FILE);
        let settings = ResetSettings::default();
        let titles = |step: &Step| -> Vec<String> {
            step.notifications
                .iter()
                .map(|notification| notification.title.clone())
                .collect()
        };

        // A first run has no cache: banked resets set a quiet baseline.
        let mut engine = ResetEngine::load(&banked_path, &reminders_path);
        assert_eq!(engine, ResetEngine::default());
        let mut credits = vec![credit("credit-1", NOW + 2 * HOUR)];
        let reading = with_resets(usage(80, 70), credits.clone());
        let step = engine.refresh_succeeded(&reading, &settings, NOW, false, &mut no_key);
        assert_eq!(titles(&step), ["Codex Reset Expiring"]);
        assert!(step.save_banked_resets && step.save_reminders);
        engine.save(&step, &banked_path, &reminders_path);

        // A reset banked while the app was closed is announced on the next
        // start, and the reminder already given is not repeated.
        credits.push(credit("credit-2", NOW + 100 * HOUR));
        let reading = with_resets(usage(80, 70), credits);
        let mut restarted = ResetEngine::load(&banked_path, &reminders_path);
        let step = restarted.refresh_succeeded(&reading, &settings, NOW + 60, false, &mut no_key);
        assert_eq!(titles(&step), ["Codex Limit Reset Banked"]);
        assert!(step.save_banked_resets && !step.save_reminders);
    }

    #[test]
    fn only_changed_state_is_written() {
        let directory = TempDir::new("engine-save");
        let banked_path = directory.file(BANKED_RESETS_FILE);
        let reminders_path = directory.file(RESET_REMINDERS_FILE);
        let engine = ResetEngine::default();

        engine.save(&Step::default(), &banked_path, &reminders_path);
        assert!(!banked_path.exists() && !reminders_path.exists());
        let banked_only = Step {
            save_banked_resets: true,
            ..Step::default()
        };
        engine.save(&banked_only, &banked_path, &reminders_path);
        assert!(banked_path.exists() && !reminders_path.exists());
        let reminders_only = Step {
            save_reminders: true,
            ..Step::default()
        };
        engine.save(&reminders_only, &banked_path, &reminders_path);
        assert_eq!(std::fs::read_to_string(reminders_path).unwrap(), "{}");
    }

    #[test]
    fn an_expiring_reset_is_redeemed_once() {
        let mut engine = ResetEngine::default();
        let mut keys = numbered_keys();
        let mut settings = redeeming();
        let reading = expiring();

        let step = engine.refresh_succeeded(&reading, &settings, NOW, false, &mut keys);
        let pending = attempt("credit-1", "key-1", AttemptStatus::Pending, NOW);
        let plan = RedemptionPlan {
            credit_id: "credit-1".to_string(),
            idempotency_key: "key-1".to_string(),
            attempt: pending.clone(),
        };
        assert_eq!(
            step,
            Step {
                reminder_timer: TimerChange::Cancel,
                redemption_timer: TimerChange::Cancel,
                redemption: Some(plan),
                save_banked_resets: true,
                ..Step::default()
            }
        );
        settings.auto_redeem_attempt = pending;

        let finished = NOW + 2;
        let completed = attempt("credit-1", "key-1", AttemptStatus::Completed, finished);
        assert_eq!(
            engine.redemption_record(&RedemptionOutcome::Reset, finished),
            Some(completed.clone())
        );
        let step = finish(&mut engine, &RedemptionOutcome::Reset, &settings, finished);
        assert_eq!(
            step,
            Step {
                notifications: vec![Notification::info(
                    "Codex Limit Reset Used",
                    "An expiring limit reset was used automatically."
                )],
                refresh: true,
                ..Step::default()
            }
        );
        settings.auto_redeem_attempt = completed;

        // The reading that follows still lists the credit and shows the
        // limits back at 100%. Neither is acted on.
        let after = with_resets(usage(100, 100), reading.resets.clone());
        let step = engine.refresh_succeeded(&after, &settings, NOW + 5, false, &mut keys);
        assert!(step.notifications.is_empty());
        assert_eq!(step.redemption, None);
        assert_eq!(step.redemption_timer, TimerChange::Cancel);

        // Half an hour on, limit resets are announced again.
        let used = with_resets(usage(40, 40), reading.resets.clone());
        engine.refresh_succeeded(&used, &settings, NOW + 10 * MINUTE, false, &mut keys);
        let later = finished + 30 * MINUTE + 1;
        let step = engine.refresh_succeeded(&after, &settings, later, false, &mut keys);
        assert_eq!(step.notifications.len(), 1);
        assert_eq!(step.notifications[0].title, "Codex Usage Reset");
    }

    #[test]
    fn redemption_waits_for_an_expiring_credit_and_usage_to_reset() {
        let plan = |usage: &Usage, settings: &ResetSettings| {
            let mut engine = ResetEngine::default();
            let step = engine.refresh_succeeded(usage, settings, NOW, false, &mut numbered_keys());
            assert!(step.notifications.is_empty());
            (step.redemption.is_some(), step.redemption_timer)
        };
        assert_eq!(plan(&expiring(), &redeeming()), (true, TimerChange::Cancel));
        // Automatic redemption is opt-in.
        assert_eq!(plan(&expiring(), &quiet()), (false, TimerChange::Cancel));

        // With nothing to reset, the credit is looked at again every minute.
        let unused = with_resets(usage(100, 100), expiring().resets);
        assert_eq!(
            plan(&unused, &redeeming()),
            (false, TimerChange::FireAt(NOW + MINUTE))
        );
        // Outside the lead time, the check waits for it.
        let early = with_resets(usage(60, 80), vec![credit("credit-1", NOW + 30 * MINUTE)]);
        assert_eq!(
            plan(&early, &redeeming()),
            (false, TimerChange::FireAt(NOW + 25 * MINUTE))
        );
        let longer_lead = ResetSettings {
            auto_redeem_lead_minutes: 30,
            ..redeeming()
        };
        assert_eq!(plan(&early, &longer_lead), (true, TimerChange::Cancel));
        // Without credits there is nothing to wait for.
        assert_eq!(
            plan(&usage(60, 80), &redeeming()),
            (false, TimerChange::Cancel)
        );
    }

    #[test]
    fn an_unanswered_request_is_repeated_under_the_same_key() {
        let mut engine = ResetEngine::default();
        let mut keys = numbered_keys();
        let mut settings = start_redemption(&mut engine, &mut keys);
        let timeout = failed("Codex app-server timed out while using the reset credit.");
        let error = [Notification::warning(
            "Codex Automatic Reset",
            "Codex app-server timed out while using the reset credit.",
        )];

        let finished = NOW + 20;
        let record = engine.redemption_record(&timeout, finished).unwrap();
        assert_eq!(
            record,
            attempt("credit-1", "key-1", AttemptStatus::Pending, finished)
        );
        let step = finish(&mut engine, &timeout, &settings, finished);
        assert_eq!(
            step,
            Step {
                notifications: error.to_vec(),
                redemption_timer: TimerChange::FireAt(finished + MINUTE),
                ..Step::default()
            }
        );
        settings.auto_redeem_attempt = record;

        // A reading inside the minute keeps waiting.
        let early = engine.refresh_succeeded(&expiring(), &settings, NOW + 30, false, &mut keys);
        assert_eq!(early.redemption, None);
        assert_eq!(
            early.redemption_timer,
            TimerChange::FireAt(finished + MINUTE)
        );

        let retried = finished + MINUTE;
        let step = engine.refresh_succeeded(&expiring(), &settings, retried, false, &mut keys);
        let plan = step.redemption.unwrap();
        assert_eq!(plan.idempotency_key, "key-1");
        assert_eq!(plan.attempt.last_attempt_utc, Some(retried));
        settings.auto_redeem_attempt = plan.attempt;

        // The same failure is not reported twice; a different one is.
        let step = finish(&mut engine, &timeout, &settings, retried + 20);
        assert!(step.notifications.is_empty());
        settings.auto_redeem_attempt.last_attempt_utc = Some(retried + 20);
        let again = retried + 20 + MINUTE;
        let step = engine.refresh_succeeded(&expiring(), &settings, again, false, &mut keys);
        assert!(step.redemption.is_some());
        let step = finish(&mut engine, &failed(" "), &settings, again + 1);
        assert_eq!(
            step.notifications,
            [Notification::warning(
                "Codex Automatic Reset",
                "The automatic reset request failed."
            )]
        );
    }

    #[test]
    fn an_answered_request_is_repeated_under_a_new_key() {
        let mut engine = ResetEngine::default();
        let mut keys = numbered_keys();
        let mut settings = start_redemption(&mut engine, &mut keys);
        let outcome = RedemptionOutcome::NothingToReset;

        let record = engine.redemption_record(&outcome, NOW + 1).unwrap();
        assert_eq!(record.status, Some(AttemptStatus::NothingToReset));
        let step = finish(&mut engine, &outcome, &settings, NOW + 1);
        assert_eq!(
            step,
            Step {
                redemption_timer: TimerChange::FireAt(NOW + 1 + MINUTE),
                ..Step::default()
            }
        );
        settings.auto_redeem_attempt = record;

        let retried = NOW + 1 + MINUTE;
        let step = engine.refresh_succeeded(&expiring(), &settings, retried, false, &mut keys);
        assert_eq!(step.redemption.unwrap().idempotency_key, "key-2");
    }

    #[test]
    fn a_spent_or_missing_credit_ends_the_attempt() {
        let outcome = |outcome: RedemptionOutcome| {
            let mut engine = ResetEngine::default();
            let settings = start_redemption(&mut engine, &mut numbered_keys());
            let record = engine.redemption_record(&outcome, NOW + 2).unwrap();
            assert_eq!(record.status, Some(AttemptStatus::Completed));
            let step = finish(&mut engine, &outcome, &settings, NOW + 2);
            assert!(step.refresh && step.redemption.is_none());
            assert_eq!(step.redemption_timer, TimerChange::Keep);

            // Whether the limits returning to 100% is news afterwards.
            let after =
                engine.refresh_succeeded(&usage(100, 100), &quiet(), NOW + 5, false, &mut no_key);
            (step.notifications, after.notifications.len())
        };
        assert_eq!(
            outcome(RedemptionOutcome::AlreadyRedeemed),
            (
                vec![Notification::info(
                    "Codex Limit Reset",
                    "The expiring reset had already been redeemed."
                )],
                0
            )
        );
        // Nothing was redeemed, so a reset seen afterwards happened by itself.
        assert_eq!(outcome(RedemptionOutcome::NoCredit), (Vec::new(), 1));
    }

    #[test]
    fn a_success_lets_an_earlier_error_be_reported_again() {
        let mut engine = ResetEngine::default();
        let mut keys = numbered_keys();
        let boom = failed("boom");
        let count = |step: Step| step.notifications.len();

        let settings = start_redemption(&mut engine, &mut keys);
        assert_eq!(count(finish(&mut engine, &boom, &settings, NOW + 1)), 1);
        let retry = |engine: &mut ResetEngine, keys: &mut dyn FnMut() -> Option<String>| {
            engine.refresh_succeeded(&expiring(), &redeeming(), NOW + 2 * MINUTE, false, keys);
        };
        retry(&mut engine, &mut keys);
        assert_eq!(
            count(finish(&mut engine, &boom, &settings, NOW + 3 * MINUTE)),
            0
        );

        retry(&mut engine, &mut keys);
        let no_credit = RedemptionOutcome::NoCredit;
        assert_eq!(
            count(finish(&mut engine, &no_credit, &settings, NOW + 4 * MINUTE)),
            0
        );
        retry(&mut engine, &mut keys);
        assert_eq!(
            count(finish(&mut engine, &boom, &settings, NOW + 5 * MINUTE)),
            1
        );
    }

    #[test]
    fn a_refresh_requested_during_a_redemption_follows_it() {
        let mut engine = ResetEngine::default();
        assert!(engine.begin_refresh());
        let settings = start_redemption(&mut engine, &mut numbered_keys());
        assert!(!engine.begin_refresh());
        // A reading that arrives anyway does not start a second redemption.
        let stray = engine.refresh_succeeded(&expiring(), &settings, NOW, false, &mut no_key);
        assert_eq!(stray.redemption, None);
        assert_eq!(stray.redemption_timer, TimerChange::Cancel);

        let step = finish(&mut engine, &failed("boom"), &settings, NOW + 1);
        assert_eq!(step.notifications.len(), 1);
        assert!(step.refresh);
        assert_eq!(step.redemption_timer, TimerChange::Keep);
        assert!(engine.begin_refresh());

        // The request is remembered for one redemption only.
        let later = NOW + 2 * MINUTE;
        engine.refresh_succeeded(
            &expiring(),
            &redeeming(),
            later,
            false,
            &mut numbered_keys(),
        );
        assert!(!finish(&mut engine, &failed("boom"), &settings, later + 1).refresh);
    }

    #[test]
    fn a_request_is_not_sent_when_its_record_cannot_be_saved() {
        let mut engine = ResetEngine::default();
        let mut keys = numbered_keys();
        let settings = redeeming();
        let error = Notification::warning(
            "Codex Automatic Reset",
            "Could not save redemption state: Access is denied.",
        );

        let step = engine.refresh_succeeded(&expiring(), &settings, NOW, false, &mut keys);
        assert!(step.redemption.is_some());
        let step = engine.redemption_not_saved("Access is denied.", NOW);
        assert_eq!(
            step,
            Step {
                notifications: vec![error],
                redemption_timer: TimerChange::FireAt(NOW + MINUTE),
                ..Step::default()
            }
        );
        // The redemption is over: refreshes run and nothing is left to fold in.
        assert!(engine.begin_refresh());
        assert_eq!(
            engine.redemption_record(&RedemptionOutcome::Reset, NOW),
            None
        );
        assert_eq!(
            finish(&mut engine, &RedemptionOutcome::Reset, &settings, NOW),
            Step::default()
        );
        assert_eq!(
            engine.redemption_not_saved("Access is denied.", NOW),
            Step::default()
        );

        // Nothing was saved, so the next attempt starts over, and failing
        // the same way again is not reported twice.
        let retried = NOW + MINUTE;
        let step = engine.refresh_succeeded(&expiring(), &settings, retried, false, &mut keys);
        assert_eq!(step.redemption.unwrap().idempotency_key, "key-2");
        let step = engine.redemption_not_saved("Access is denied.", retried);
        assert!(step.notifications.is_empty());
        assert_eq!(step.redemption_timer, TimerChange::FireAt(retried + MINUTE));
    }

    #[test]
    fn an_unsaved_outcome_is_reported_before_the_outcome_itself() {
        let titles_and_texts = |outcome: RedemptionOutcome| -> Vec<(String, String)> {
            let mut engine = ResetEngine::default();
            let settings = start_redemption(&mut engine, &mut numbered_keys());
            let usage = expiring();
            engine
                .redemption_finished(
                    &outcome,
                    Some("The disk is full."),
                    Some(&usage),
                    &settings,
                    NOW + 1,
                    &mut no_key,
                )
                .notifications
                .into_iter()
                .map(|notification| (notification.title, notification.text))
                .collect()
        };
        let unsaved = (
            "Codex Automatic Reset".to_string(),
            "Could not update redemption state: The disk is full.".to_string(),
        );
        assert_eq!(
            titles_and_texts(RedemptionOutcome::Reset),
            [
                unsaved.clone(),
                (
                    "Codex Limit Reset Used".to_string(),
                    "An expiring limit reset was used automatically.".to_string()
                )
            ]
        );
        assert_eq!(
            titles_and_texts(failed("boom")),
            [
                unsaved,
                ("Codex Automatic Reset".to_string(), "boom".to_string())
            ]
        );
    }

    #[test]
    fn a_missing_key_postpones_the_redemption() {
        let mut engine = ResetEngine::default();
        let step = engine.refresh_succeeded(&expiring(), &redeeming(), NOW, false, &mut no_key);
        assert_eq!(
            step,
            Step {
                notifications: vec![Notification::warning(
                    "Codex Automatic Reset",
                    "Could not create a redemption request key."
                )],
                reminder_timer: TimerChange::Cancel,
                redemption_timer: TimerChange::FireAt(NOW + MINUTE),
                save_banked_resets: true,
                ..Step::default()
            }
        );
        assert!(engine.begin_refresh());
    }

    #[test]
    fn the_next_expiring_credit_follows_at_once() {
        let mut engine = ResetEngine::default();
        let mut keys = numbered_keys();
        let mut settings = redeeming();
        let reading = with_resets(
            usage(60, 80),
            vec![
                credit("credit-1", NOW + MINUTE),
                credit("credit-2", NOW + 4 * MINUTE),
            ],
        );
        let step = engine.refresh_succeeded(&reading, &settings, NOW, false, &mut keys);
        let first = step.redemption.unwrap();
        assert_eq!(first.credit_id, "credit-1");
        settings.auto_redeem_attempt = first.attempt;

        // The first credit expired while its request was running.
        let finished = NOW + MINUTE;
        let outcome = RedemptionOutcome::NothingToReset;
        let step = engine.redemption_finished(
            &outcome,
            None,
            Some(&reading),
            &settings,
            finished,
            &mut keys,
        );
        assert_eq!(
            step.redemption,
            Some(RedemptionPlan {
                credit_id: "credit-2".to_string(),
                idempotency_key: "key-2".to_string(),
                attempt: attempt("credit-2", "key-2", AttemptStatus::Pending, finished),
            })
        );
        assert!(!step.refresh);
    }

    #[test]
    fn timers_start_a_refresh_unless_one_is_running() {
        let engine = ResetEngine::default();
        let reminding = ResetSettings::default();
        let refresh = |step: Step| (step.refresh, step.reminder_timer, step.redemption_timer);

        assert_eq!(
            refresh(engine.reminder_timer_fired(&reminding, false, NOW)),
            (true, TimerChange::Cancel, TimerChange::Keep)
        );
        assert_eq!(
            refresh(engine.reminder_timer_fired(&reminding, true, NOW)),
            (false, TimerChange::FireAt(NOW + 5), TimerChange::Keep)
        );
        assert_eq!(
            refresh(engine.reminder_timer_fired(&quiet(), false, NOW)),
            (false, TimerChange::Cancel, TimerChange::Keep)
        );

        assert_eq!(
            refresh(engine.redemption_timer_fired(&redeeming(), false, NOW)),
            (true, TimerChange::Keep, TimerChange::Cancel)
        );
        assert_eq!(
            refresh(engine.redemption_timer_fired(&redeeming(), true, NOW)),
            (false, TimerChange::Keep, TimerChange::FireAt(NOW + 5))
        );
        assert_eq!(
            refresh(engine.redemption_timer_fired(&quiet(), false, NOW)),
            (false, TimerChange::Keep, TimerChange::Cancel)
        );

        // During a redemption the reminder waits and the redemption timer rests.
        let mut redeeming_engine = ResetEngine::default();
        start_redemption(&mut redeeming_engine, &mut numbered_keys());
        assert_eq!(
            refresh(redeeming_engine.reminder_timer_fired(&reminding, false, NOW)),
            (false, TimerChange::FireAt(NOW + 5), TimerChange::Keep)
        );
        assert_eq!(
            refresh(redeeming_engine.redemption_timer_fired(&redeeming(), false, NOW)),
            (false, TimerChange::Keep, TimerChange::Cancel)
        );
    }

    #[test]
    fn timer_delays_fit_a_windows_timer() {
        assert_eq!(timer_delay_milliseconds(NOW + 90, NOW), 90_000);
        assert_eq!(timer_delay_milliseconds(NOW, NOW), 1_000);
        assert_eq!(timer_delay_milliseconds(NOW - 5, NOW), 1_000);
        let longest = i32::MAX as u32;
        assert_eq!(timer_delay_milliseconds(NOW + 40 * 24 * HOUR, NOW), longest);
        assert_eq!(timer_delay_milliseconds(i64::MAX, i64::MIN), longest);
    }

    #[test]
    fn times_beyond_the_existing_apps_range_count_as_unknown() {
        assert_eq!(representable(0), Some(0));
        assert_eq!(
            representable(-DOTNET_EPOCH_SECONDS),
            Some(-DOTNET_EPOCH_SECONDS)
        );
        assert_eq!(representable(-DOTNET_EPOCH_SECONDS - 1), None);
        assert_eq!(representable(DOTNET_LAST_SECOND), Some(DOTNET_LAST_SECOND));
        assert_eq!(representable(DOTNET_LAST_SECOND + 1), None);

        let endless = credit("credit-1", i64::MAX);
        assert_eq!(known_expiry(&endless), None);
        let reading = with_resets(usage(60, 80), vec![endless]);
        let settings = ResetSettings {
            reset_expiry_reminders: true,
            ..redeeming()
        };
        let mut engine = ResetEngine::default();
        let step = engine.refresh_succeeded(&reading, &settings, NOW, true, &mut no_key);
        assert_eq!(step.notifications.len(), 1);
        assert_eq!(step.notifications[0].title, "Codex Usage");
        assert_eq!(step.reminder_timer, TimerChange::Cancel);
        assert_eq!(step.redemption_timer, TimerChange::Cancel);
    }
}
