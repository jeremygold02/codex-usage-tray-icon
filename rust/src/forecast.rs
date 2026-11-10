//! Depletion estimates for a usage window: the popup's forecast line and the
//! chart's projected path.
//!
//! An estimate uses the best evidence available. Weekly windows first try a
//! learned pattern (see `pattern.rs`). Otherwise recent observations give a
//! time-weighted pace, and until there are enough of those the usage so far
//! is divided by the time elapsed in the cycle.

use std::cell::Cell;
use std::f64::consts::LN_2;

use crate::history::{
    reset_after_seconds, valid_timestamp_ms, window_samples, HistorySample, WindowSample,
    PERCENT_TOLERANCE,
};
use crate::localtime::{
    add_hours, hours_between, now_to_ms, seconds_between, TimeZone, MAX_TIMESTAMP_MS, MS_PER_HOUR,
    MS_PER_MINUTE, MS_PER_SECOND,
};
use crate::model::{LimitWindow, Usage};
use crate::pattern::PatternForecast;
pub use crate::settings::{ForecastMethod, QuietHoursMode};
use crate::timefmt::{format_date_time, format_duration};

/// Readings older than this no longer describe current usage.
const MAX_READING_AGE_MS: i64 = 30 * MS_PER_MINUTE;
/// A reading may be this far ahead of the caller's clock.
const MAX_CLOCK_SKEW_MS: i64 = MS_PER_MINUTE;

/// The user's choices that shape a forecast. They apply to both providers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForecastSettings {
    pub method: ForecastMethod,
    pub quiet_hours: QuietHoursMode,
}

impl Default for ForecastSettings {
    /// The existing app's defaults: the learned routine, with quiet hours.
    fn default() -> Self {
        Self {
            method: ForecastMethod::LearnedRoutine,
            quiet_hours: QuietHoursMode::Automatic,
        }
    }
}

/// What can be said about when a window's allowance runs out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Forecast {
    /// The reading is failed, paused or too old to estimate from.
    NeedsRefresh,
    /// The reading lacks this window, or the estimate is out of range.
    Unavailable,
    LimitReached,
    /// The reset deadline has passed but no newer reading has arrived.
    AwaitingReset,
    /// Too little is known about this cycle to estimate yet.
    Learning,
    NoUsageYet,
    NoRecentIncrease,
    LastsUntilReset,
    Estimated {
        /// Unix milliseconds at which the allowance is projected to run out.
        depletes_at_ms: i64,
        /// Seconds from the reading until then, rounded to the nearest second.
        duration_seconds: i64,
    },
}

impl Forecast {
    /// The line shown under a usage bar and in the chart footer. `now`
    /// decides whether the date needs its year.
    pub fn text(&self, now: i64) -> String {
        match *self {
            Self::NeedsRefresh => "Refresh usage for an estimate".to_string(),
            Self::Unavailable => "Usage estimate unavailable".to_string(),
            Self::LimitReached => "Usage limit reached".to_string(),
            Self::AwaitingReset => "Awaiting reset update".to_string(),
            Self::Learning => "Learning usage pace...".to_string(),
            Self::NoUsageYet => "No usage yet this cycle".to_string(),
            Self::NoRecentIncrease => "No usage increase in recent history".to_string(),
            Self::LastsUntilReset => "Will last until reset".to_string(),
            Self::Estimated {
                depletes_at_ms,
                duration_seconds,
            } => format!(
                "Estimated: {} ({})",
                format_date_time(depletes_at_ms.div_euclid(MS_PER_SECOND), now),
                format_duration(duration_seconds)
            ),
        }
    }
}

/// A point on a projected path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectionPoint {
    /// Unix milliseconds.
    pub timestamp_ms: i64,
    pub remaining_percent: f64,
}

/// The projected remaining allowance from a reading until it runs out or the
/// window resets, whichever comes first.
#[derive(Clone, Debug, PartialEq)]
pub struct Projection {
    /// The time of the reading the path starts from, in Unix milliseconds.
    pub start_ms: i64,
    /// Where the path ends: depletion, or the reset deadline.
    pub end_ms: i64,
    pub start_remaining_percent: f64,
    pub end_remaining_percent: f64,
    /// True when the allowance lasts until the reset.
    pub ends_at_reset: bool,
    pub uses_daily_pattern: bool,
    /// True when the model behind the path beat a plain pace on held-out readings.
    pub uses_validated_model: bool,
    pub uses_session_pace: bool,
    /// True when learned quiet hours flatten parts of the path.
    pub uses_inactive_hours: bool,
    /// The path hour by hour, starting with the reading itself. Empty for a
    /// straight line from start to end.
    pub points: Vec<ProjectionPoint>,
}

impl Projection {
    /// The remaining percentage the path shows at a time: interpolated along
    /// its points, and level at the final value from the end onwards. For
    /// display, round this once and take used as its complement.
    pub fn remaining_at(&self, timestamp_ms: i64) -> f64 {
        if timestamp_ms >= self.end_ms {
            return self.end_remaining_percent;
        }
        let mut before_ms = self.start_ms;
        let mut remaining = self.start_remaining_percent;
        for point in &self.points {
            if point.timestamp_ms <= before_ms {
                continue;
            }
            if timestamp_ms <= point.timestamp_ms {
                return remaining
                    + (point.remaining_percent - remaining)
                        * seconds_between(timestamp_ms, before_ms)
                        / seconds_between(point.timestamp_ms, before_ms);
            }
            before_ms = point.timestamp_ms;
            remaining = point.remaining_percent;
        }
        remaining
            + (self.end_remaining_percent - remaining) * seconds_between(timestamp_ms, before_ms)
                / seconds_between(self.end_ms, before_ms).max(1.0)
    }

    /// The stretches a learned quiet period keeps level, as start and end
    /// times, for the chart's optional highlight. Empty unless the path uses
    /// quiet hours and consumes something; the level run after depletion is
    /// not a quiet period.
    pub fn quiet_periods(&self) -> Vec<(i64, i64)> {
        let consumes = self
            .points
            .windows(2)
            .any(|pair| pair[0].remaining_percent - pair[1].remaining_percent > 0.000_001);
        if !self.uses_inactive_hours || !consumes {
            return Vec::new();
        }
        self.points
            .windows(2)
            .filter(|pair| {
                pair[0].remaining_percent > 0.0
                    && (pair[0].remaining_percent - pair[1].remaining_percent).abs() < 0.000_001
                    && pair[1].timestamp_ms > pair[0].timestamp_ms
            })
            .map(|pair| (pair[0].timestamp_ms, pair[1].timestamp_ms))
            .collect()
    }
}

/// One window of a reading, in the terms the rate estimates work in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowReading {
    /// Unix milliseconds.
    pub timestamp_ms: i64,
    pub used_percent: f64,
    pub window_minutes: Option<u32>,
    /// Whole seconds from the reading until the window resets.
    pub reset_after_seconds: Option<i64>,
}

impl WindowReading {
    fn from_limit(limit: &LimitWindow, updated_at: i64, timestamp_ms: i64) -> Self {
        Self {
            timestamp_ms,
            used_percent: limit.used_percent,
            window_minutes: Some(limit.window_minutes),
            reset_after_seconds: reset_after_seconds(limit, updated_at),
        }
    }

    /// A stored reading as it would have looked when it was the current one.
    pub fn from_sample(sample: &WindowSample) -> Self {
        Self {
            timestamp_ms: sample.timestamp_ms,
            used_percent: sample.window.used_percent,
            window_minutes: sample.window.window_minutes,
            reset_after_seconds: sample
                .window
                .reset_at_ms
                .map(|reset| reset.saturating_sub(sample.timestamp_ms) / MS_PER_SECOND),
        }
    }

    /// When the window resets, in Unix milliseconds.
    pub fn reset_at_ms(&self) -> Option<i64> {
        self.reset_after_seconds.map(|seconds| {
            self.timestamp_ms
                .saturating_add(seconds.saturating_mul(MS_PER_SECOND))
        })
    }
}

/// How far back a recent-pace estimate looks and how it weighs what it finds.
struct Weighting {
    lookback_ms: i64,
    /// The age at which an interval counts half as much as the newest one.
    half_life_hours: f64,
    minimum_hours: f64,
    /// Whether gaps over two hours may be spanned when both sides share a
    /// reset deadline, which proves they belong to the same cycle.
    bridge_gaps: bool,
}

const WEEKLY_WEIGHTING: Weighting = Weighting {
    lookback_ms: 48 * MS_PER_HOUR,
    half_life_hours: 6.0,
    minimum_hours: 6.0,
    bridge_gaps: true,
};

const FIVE_HOUR_WEIGHTING: Weighting = Weighting {
    lookback_ms: MS_PER_HOUR,
    half_life_hours: 0.25,
    minimum_hours: 0.25,
    bridge_gaps: false,
};

const SESSION_WEIGHTING: Weighting = Weighting {
    lookback_ms: 6 * MS_PER_HOUR,
    half_life_hours: 1.0,
    minimum_hours: 0.5,
    bridge_gaps: false,
};

/// Percent per hour over the recent history that ends with `reading`: up to 48
/// hours for the weekly window, one hour for the five-hour window. `None`
/// without three readings that span the minimum time in the current cycle.
pub fn recent_rate(weekly: bool, reading: &WindowReading, samples: &[WindowSample]) -> Option<f64> {
    let weighting = if weekly {
        &WEEKLY_WEIGHTING
    } else {
        &FIVE_HOUR_WEIGHTING
    };
    weighted_rate(reading, samples, weighting)
}

/// Percent per hour over the working session that ends with `reading`: up to
/// six hours, without spanning monitoring gaps.
pub fn session_rate(reading: &WindowReading, samples: &[WindowSample]) -> Option<f64> {
    weighted_rate(reading, samples, &SESSION_WEIGHTING)
}

fn weighted_rate(
    reading: &WindowReading,
    samples: &[WindowSample],
    weighting: &Weighting,
) -> Option<f64> {
    let last = samples.last().filter(|_| samples.len() >= 3)?;
    if last.timestamp_ms.abs_diff(reading.timestamp_ms) > MS_PER_SECOND as u64
        || (last.window.used_percent - reading.used_percent).abs() > PERCENT_TOLERANCE
    {
        return None;
    }

    let end = samples.len() - 1;
    let cutoff = last.timestamp_ms.saturating_sub(weighting.lookback_ms);
    let mut start = end;
    while start > 0 && samples[start - 1].timestamp_ms >= cutoff {
        let (before, after) = (&samples[start - 1], &samples[start]);
        if !after.same_cycle_as(before) {
            break;
        }
        if !after.continues(before) && !(weighting.bridge_gaps && after.shares_reset_with(before)) {
            break;
        }
        start -= 1;
    }

    let hours = hours_between(last.timestamp_ms, samples[start].timestamp_ms);
    if end - start < 2 || hours < weighting.minimum_hours {
        return None;
    }
    // Weight elapsed time, not sample count: frequent polling must not amplify
    // a burst. Integrating the decay over each interval also keeps a steady
    // rate unchanged when the same interval is split into more observations.
    let decay_per_hour = LN_2 / weighting.half_life_hours;
    let weight_at = |timestamp_ms: i64| {
        (-decay_per_hour * hours_between(last.timestamp_ms, timestamp_ms)).exp()
    };
    let mut weighted_rate = 0.0;
    let mut total_weight = 0.0;
    for pair in samples[start..].windows(2) {
        let (before, after) = (&pair[0], &pair[1]);
        let interval_hours = hours_between(after.timestamp_ms, before.timestamp_ms);
        let weight = weight_at(after.timestamp_ms) - weight_at(before.timestamp_ms);
        let increase = (after.window.used_percent - before.window.used_percent).max(0.0);
        weighted_rate += increase / interval_hours * weight;
        total_weight += weight;
    }
    Some(weighted_rate / total_weight)
}

/// Percent per hour averaged over the cycle so far: usage divided by the time
/// since the window's inferred start. `None` in the first 1% of the window,
/// without known timing, or once usage has been seen to drop in this cycle.
pub fn cycle_average_rate(reading: &WindowReading, samples: &[WindowSample]) -> Option<f64> {
    let reset_after = reading.reset_after_seconds? as f64;
    let duration = f64::from(reading.window_minutes?) * 60.0;
    let elapsed = duration - reset_after;
    // OpenQuota's pacing approach: avoid projecting the very start of a window.
    if elapsed < f64::max(60.0, duration * 0.01) || elapsed >= duration {
        return None;
    }
    let reset_ms = reading.reset_at_ms()?;
    if samples.last().is_some_and(|last| {
        last.window.cycle_average_unavailable && last.window.resets_near(reset_ms)
    }) {
        return None;
    }
    for pair in samples.windows(2).rev() {
        let (before, after) = (&pair[0], &pair[1]);
        if !after.window.resets_near(reset_ms) {
            break;
        }
        // Use the same decrease tolerance as a reset so measurement jitter
        // cannot discard a valid cycle-average forecast and chart projection.
        // A real decrease means a banked reset moved the cycle's true start.
        if after.shares_reset_with(before)
            && after.window.used_percent < before.window.used_percent - PERCENT_TOLERANCE
        {
            return None;
        }
    }
    Some(reading.used_percent * 3_600.0 / elapsed)
}

/// Keeps the last learned pattern, which is costly to rebuild and only
/// changes when the history or its inputs do.
#[derive(Default)]
pub struct PatternCache {
    entry: Cell<Option<CachedPattern>>,
}

struct CachedPattern {
    sample_count: usize,
    latest_ms: i64,
    rate: f64,
    allow_inactive_hours: bool,
    pattern: Option<PatternForecast>,
}

impl PatternCache {
    /// Forgets the cached pattern after the history changed.
    pub fn invalidate(&self) {
        self.entry.set(None);
    }

    fn pattern(
        &self,
        samples: &[WindowSample],
        rate: f64,
        allow_inactive_hours: bool,
        zone: &TimeZone,
    ) -> Option<PatternForecast> {
        let latest_ms = samples.last()?.timestamp_ms;
        let entry = self
            .entry
            .take()
            .filter(|entry| {
                entry.sample_count == samples.len()
                    && entry.latest_ms == latest_ms
                    && entry.rate == rate
                    && entry.allow_inactive_hours == allow_inactive_hours
            })
            .unwrap_or_else(|| CachedPattern {
                sample_count: samples.len(),
                latest_ms,
                rate,
                allow_inactive_hours,
                pattern: PatternForecast::try_create(samples, rate, allow_inactive_hours, zone),
            });
        let pattern = entry.pattern.clone();
        self.entry.set(Some(entry));
        pattern
    }
}

/// What an estimate is derived from besides the reading on display.
pub struct Sources<'a> {
    pub samples: &'a [HistorySample],
    pub zone: &'a TimeZone,
    pub patterns: &'a PatternCache,
}

/// See [`crate::history::HistoryStore::forecast`].
pub fn forecast(
    sources: &Sources,
    weekly: bool,
    usage: &Usage,
    fresh: bool,
    settings: ForecastSettings,
    now: i64,
) -> Forecast {
    let now_ms = now_to_ms(now);
    let Some(timestamp_ms) = fresh_timestamp_ms(usage, fresh, now_ms) else {
        return Forecast::NeedsRefresh;
    };
    let Some(limit) = limit_of(usage, weekly) else {
        return Forecast::Unavailable;
    };
    if limit.used_percent >= 100.0 {
        return Forecast::LimitReached;
    }
    let reading = WindowReading::from_limit(limit, usage.updated_at, timestamp_ms);
    let reset_ms = reading.reset_at_ms();
    if reset_ms.is_some_and(|reset| reset <= now_ms) {
        return Forecast::AwaitingReset;
    }

    let samples = window_samples(sources.samples, weekly);
    if weekly {
        let pattern = project(sources, &samples, weekly, &reading, settings, now_ms)
            .filter(|projection| projection.uses_validated_model || projection.uses_inactive_hours);
        if let Some(pattern) = pattern {
            return if pattern.ends_at_reset {
                Forecast::LastsUntilReset
            } else {
                estimate(
                    pattern.start_ms,
                    hours_between(pattern.end_ms, pattern.start_ms),
                )
            };
        }
    }

    let (rate, idle) = match recent_rate(weekly, &reading, &samples) {
        Some(rate) => (rate, Forecast::NoRecentIncrease),
        None => match cycle_average_rate(&reading, &samples) {
            Some(rate) => (rate, Forecast::NoUsageYet),
            None => return Forecast::Learning,
        },
    };
    if rate <= 0.0 {
        return idle;
    }
    let remaining_hours = (100.0 - limit.used_percent) / rate;
    if reset_ms.is_some_and(|reset| remaining_hours >= hours_between(reset, timestamp_ms)) {
        return Forecast::LastsUntilReset;
    }
    estimate(timestamp_ms, remaining_hours)
}

/// See [`crate::history::HistoryStore::projection`].
pub fn projection(
    sources: &Sources,
    weekly: bool,
    usage: &Usage,
    fresh: bool,
    settings: ForecastSettings,
    now: i64,
) -> Option<Projection> {
    let now_ms = now_to_ms(now);
    let timestamp_ms = fresh_timestamp_ms(usage, fresh, now_ms)?;
    let limit = limit_of(usage, weekly).filter(|limit| limit.used_percent < 100.0)?;
    let reading = WindowReading::from_limit(limit, usage.updated_at, timestamp_ms);
    let samples = window_samples(sources.samples, weekly);
    project(sources, &samples, weekly, &reading, settings, now_ms)
}

fn project(
    sources: &Sources,
    samples: &[WindowSample],
    weekly: bool,
    reading: &WindowReading,
    settings: ForecastSettings,
    now_ms: i64,
) -> Option<Projection> {
    let start_ms = reading.timestamp_ms;
    let reset_ms = reading.reset_at_ms().filter(|&reset| reset > now_ms)?;
    let rate =
        recent_rate(weekly, reading, samples).or_else(|| cycle_average_rate(reading, samples));
    let remaining = 100.0 - reading.used_percent;

    // A learned pattern only applies while the history ends with this reading.
    let history_is_current = samples.last().is_some_and(|latest| {
        latest.timestamp_ms.abs_diff(start_ms) <= MS_PER_SECOND as u64
            && (latest.window.used_percent - reading.used_percent).abs() < PERCENT_TOLERANCE
    });
    if weekly && settings.method != ForecastMethod::RecentPace && history_is_current {
        let allow_inactive_hours = settings.quiet_hours != QuietHoursMode::Off;
        let pattern = sources.patterns.pattern(
            samples,
            rate.unwrap_or(0.0),
            allow_inactive_hours,
            sources.zone,
        );
        if let Some(pattern) = pattern {
            return Some(pattern.project(start_ms, reset_ms, remaining, sources.zone));
        }
    }

    let rate = rate?;
    let hours_until_reset = hours_between(reset_ms, start_ms);
    let ends_at_reset = rate <= 0.0 || remaining / rate >= hours_until_reset;
    Some(Projection {
        start_ms,
        end_ms: if ends_at_reset {
            reset_ms
        } else {
            add_hours(start_ms, remaining / rate)
        },
        start_remaining_percent: remaining,
        end_remaining_percent: if ends_at_reset {
            (remaining - rate * hours_until_reset).max(0.0)
        } else {
            0.0
        },
        ends_at_reset,
        uses_daily_pattern: false,
        uses_validated_model: false,
        uses_session_pace: false,
        uses_inactive_hours: false,
        points: Vec::new(),
    })
}

/// The reading's timestamp when it may be used for an estimate.
fn fresh_timestamp_ms(usage: &Usage, fresh: bool, now_ms: i64) -> Option<i64> {
    let timestamp_ms = valid_timestamp_ms(usage).filter(|_| fresh)?;
    (-MAX_CLOCK_SKEW_MS..=MAX_READING_AGE_MS)
        .contains(&(now_ms - timestamp_ms))
        .then_some(timestamp_ms)
}

fn limit_of(usage: &Usage, weekly: bool) -> Option<&LimitWindow> {
    if weekly {
        usage.weekly.as_ref()
    } else {
        usage.five_hour.as_ref()
    }
}

fn estimate(start_ms: i64, hours: f64) -> Forecast {
    let seconds = hours * 3_600.0;
    let latest_seconds = seconds_between(MAX_TIMESTAMP_MS, start_ms);
    if !seconds.is_finite()
        || seconds < 0.0
        || seconds > f64::from(i32::MAX)
        || seconds > latest_seconds
    {
        return Forecast::Unavailable;
    }
    Forecast::Estimated {
        depletes_at_ms: add_hours(start_ms, hours),
        duration_seconds: seconds.round() as i64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::tests::{open, snapshot, TempDir, DAY, HOUR, MINUTE, NOW};
    use crate::history::{is_reset, HistoryStore, HistoryWindow};
    use crate::history_file;

    fn forecast_at(store: &HistoryStore, weekly: bool, usage: &Usage, now: i64) -> Forecast {
        store.forecast(weekly, usage, true, ForecastSettings::default(), now)
    }

    fn projection_at(
        store: &HistoryStore,
        weekly: bool,
        usage: &Usage,
        now: i64,
    ) -> Option<Projection> {
        store.projection(weekly, usage, true, ForecastSettings::default(), now)
    }

    fn observe(store: &mut HistoryStore, timestamp: i64, used: f64, reset: Option<i64>) -> Usage {
        let usage = snapshot(timestamp, used, reset);
        assert!(store.observe(&usage, NOW), "observation is accepted");
        usage
    }

    /// Asserts an estimate of `seconds` from the reading at `start`, worded
    /// like the popup's reset line: local date and time, then the duration.
    #[track_caller]
    fn assert_estimate(forecast: Forecast, start: i64, seconds: i64, duration: &str) {
        let Forecast::Estimated {
            depletes_at_ms,
            duration_seconds,
        } = forecast
        else {
            panic!("expected an estimate, got {forecast:?}");
        };
        assert_eq!(duration_seconds, seconds);
        assert!((depletes_at_ms - (start + seconds) * 1_000).abs() <= 1_000);
        assert_eq!(
            forecast.text(NOW),
            format!(
                "Estimated: {} ({duration})",
                format_date_time(start + seconds, NOW)
            )
        );
    }

    fn weekly_of(usage: &mut Usage) -> &mut LimitWindow {
        usage.weekly.as_mut().unwrap()
    }

    fn five_hour_of(usage: &mut Usage) -> &mut LimitWindow {
        usage.five_hour.as_mut().unwrap()
    }

    fn hours_ahead(projection: &Projection, start: i64) -> f64 {
        hours_between(projection.end_ms, start * 1_000)
    }

    #[test]
    fn forecast_text_matches_the_existing_wording() {
        let lines = [
            (Forecast::NeedsRefresh, "Refresh usage for an estimate"),
            (Forecast::Unavailable, "Usage estimate unavailable"),
            (Forecast::LimitReached, "Usage limit reached"),
            (Forecast::AwaitingReset, "Awaiting reset update"),
            (Forecast::Learning, "Learning usage pace..."),
            (Forecast::NoUsageYet, "No usage yet this cycle"),
            (
                Forecast::NoRecentIncrease,
                "No usage increase in recent history",
            ),
            (Forecast::LastsUntilReset, "Will last until reset"),
        ];
        for (forecast, text) in lines {
            assert_eq!(forecast.text(NOW), text);
        }
        assert_estimate(estimate(NOW * 1_000, 1.75), NOW, 6_300, "1h 45m");
        assert_estimate(estimate(NOW * 1_000, 75.0), NOW, 270_000, "3d 3h");
        assert_estimate(estimate(NOW * 1_000, 0.0125), NOW, 45, "45s");
        for hours in [f64::NAN, f64::INFINITY, -1.0, 600_000.0] {
            assert_eq!(estimate(NOW * 1_000, hours), Forecast::Unavailable);
        }
    }

    #[test]
    fn forecasts_default_to_the_learned_routine_with_quiet_hours() {
        assert_eq!(
            ForecastSettings::default(),
            ForecastSettings {
                method: ForecastMethod::LearnedRoutine,
                quiet_hours: QuietHoursMode::Automatic,
            }
        );
    }

    #[test]
    fn observed_pace_forecasts_depletion() {
        let directory = TempDir::new("forecast");
        let mut store = open(&directory.file("forecast.json"));
        let reset = Some(NOW + 4 * HOUR);
        observe(&mut store, NOW - 30 * MINUTE, 25.0, reset);
        observe(&mut store, NOW - 15 * MINUTE, 37.5, reset);
        let mut latest = observe(&mut store, NOW, 50.0, reset);
        // Observed 50 percent per hour forecasts one hour.
        assert_estimate(
            forecast_at(&store, false, &latest, NOW),
            NOW,
            3_600,
            "1h 0m",
        );
        assert_eq!(
            forecast_at(&store, true, &latest, NOW),
            Forecast::LastsUntilReset,
            "cycle average is capped by the reset"
        );
        assert_eq!(
            forecast_at(&store, false, &latest, NOW + 31 * MINUTE),
            Forecast::NeedsRefresh,
            "old observations do not yield forecasts"
        );
        assert_eq!(
            store.forecast(false, &latest, false, ForecastSettings::default(), NOW),
            Forecast::NeedsRefresh,
            "a failed or paused reading suppresses the forecast"
        );
        latest.five_hour = None;
        assert_eq!(
            forecast_at(&store, false, &latest, NOW),
            Forecast::Unavailable
        );

        assert!(store.clear());
        let reset = Some(NOW + 30 * MINUTE);
        observe(&mut store, NOW - 30 * MINUTE, 25.0, reset);
        observe(&mut store, NOW - 15 * MINUTE, 37.5, reset);
        let mut latest = observe(&mut store, NOW, 50.0, reset);
        assert_eq!(
            forecast_at(&store, false, &latest, NOW),
            Forecast::LastsUntilReset,
            "earlier reset replaces the depletion duration"
        );
        five_hour_of(&mut latest).resets_at = Some(NOW + 3_600);
        assert_eq!(
            forecast_at(&store, false, &latest, NOW),
            Forecast::LastsUntilReset,
            "depletion exactly at reset lasts until reset"
        );
        five_hour_of(&mut latest).resets_at = Some(NOW + 3_601);
        // Depletion before reset retains its duration.
        assert_estimate(
            forecast_at(&store, false, &latest, NOW),
            NOW,
            3_600,
            "1h 0m",
        );

        assert!(store.clear());
        let reset = Some(NOW + 4 * HOUR);
        observe(&mut store, NOW - 30 * MINUTE, 25.0, reset);
        observe(&mut store, NOW - 15 * MINUTE, 25.0, reset);
        let latest = observe(&mut store, NOW, 25.0, reset);
        assert_eq!(
            forecast_at(&store, false, &latest, NOW),
            Forecast::NoRecentIncrease,
            "idle observations do not invent exhaustion"
        );
    }

    #[test]
    fn projections_follow_the_forecast() {
        let directory = TempDir::new("projection");
        let mut store = open(&directory.file("projection.json"));
        let reading = NOW - 10 * MINUTE;
        let queried_at = reading + 5 * MINUTE;
        let reset = reading + 4 * HOUR;
        observe(&mut store, reading - 30 * MINUTE, 25.0, Some(reset));
        observe(&mut store, reading - 15 * MINUTE, 37.5, Some(reset));
        let mut latest = observe(&mut store, reading, 50.0, Some(reset));

        let projection = projection_at(&store, false, &latest, queried_at).unwrap();
        assert!(
            projection.start_ms == reading * 1_000
                && projection.end_ms == (reading + HOUR) * 1_000
                && projection.start_remaining_percent == 50.0
                && projection.end_remaining_percent == 0.0
                && !projection.ends_at_reset
                && projection.points.is_empty(),
            "observed pace starts at the actual reading and stops at depletion before reset"
        );
        five_hour_of(&mut latest).resets_at = None;
        assert_eq!(
            projection_at(&store, false, &latest, queried_at),
            None,
            "observed pace without a known reset deadline has no projection"
        );

        five_hour_of(&mut latest).resets_at = Some(reading + 1_800);
        let projection = projection_at(&store, false, &latest, queried_at).unwrap();
        assert!(
            projection.end_ms == (reading + 30 * MINUTE) * 1_000
                && projection.end_remaining_percent == 25.0
                && projection.ends_at_reset,
            "projection stops at an earlier reset with positive remaining usage"
        );
        five_hour_of(&mut latest).resets_at = Some(reading + 3_600);
        let projection = projection_at(&store, false, &latest, queried_at).unwrap();
        assert!(
            projection.end_ms == (reading + HOUR) * 1_000
                && projection.end_remaining_percent == 0.0
                && projection.ends_at_reset,
            "depletion exactly at reset is marked as ending at reset"
        );
        five_hour_of(&mut latest).resets_at = Some(reading + 3_601);
        let projection = projection_at(&store, false, &latest, queried_at).unwrap();
        assert!(projection.end_ms == (reading + HOUR) * 1_000 && !projection.ends_at_reset);
        // Forecast and projection agree for a fresh reading taken before the query.
        assert_estimate(
            forecast_at(&store, false, &latest, queried_at),
            reading,
            3_600,
            "1h 0m",
        );

        assert!(store.clear());
        observe(&mut store, reading - 30 * MINUTE, 25.0, Some(reset));
        observe(&mut store, reading - 15 * MINUTE, 25.0, Some(reset));
        let latest = observe(&mut store, reading, 25.0, Some(reset));
        let projection = projection_at(&store, false, &latest, queried_at).unwrap();
        assert!(
            projection.end_ms == reset * 1_000
                && projection.start_remaining_percent == 75.0
                && projection.end_remaining_percent == 75.0
                && projection.ends_at_reset,
            "known zero recent consumption stays flat through reset"
        );

        assert!(store.clear());
        observe(&mut store, reading - 30 * MINUTE, 25.0005, Some(reset));
        observe(&mut store, reading - 15 * MINUTE, 25.00025, Some(reset));
        let latest = observe(&mut store, reading, 25.0, Some(reset));
        let projection = projection_at(&store, false, &latest, queried_at).unwrap();
        assert!(
            projection.end_ms == reset * 1_000
                && projection.end_remaining_percent == 75.0
                && projection.ends_at_reset,
            "tiny measurement declines do not increase projected remaining usage"
        );
    }

    #[test]
    fn a_cycle_average_projects_before_history_is_sufficient() {
        let directory = TempDir::new("initial-projection");
        let store = open(&directory.file("projection.json"));
        let reading = NOW - 10 * MINUTE;
        let queried_at = reading + 5 * MINUTE;
        let mut latest = snapshot(reading, 20.0, Some(reading + 6 * DAY));
        let projection = projection_at(&store, true, &latest, queried_at).unwrap();
        assert!(
            projection.start_ms == reading * 1_000
                && projection.end_ms == (reading + 4 * DAY) * 1_000
                && projection.start_remaining_percent == 80.0
                && projection.end_remaining_percent == 0.0
                && !projection.ends_at_reset,
            "valid cycle average supplies a projection before history is sufficient"
        );
        weekly_of(&mut latest).used_percent = 10.0;
        let projection = projection_at(&store, true, &latest, queried_at).unwrap();
        assert!(
            projection.end_ms == (reading + 6 * DAY) * 1_000
                && projection.end_remaining_percent == 30.0
                && projection.ends_at_reset,
            "cycle average projection retains usage at reset"
        );

        weekly_of(&mut latest).resets_at = None;
        assert_eq!(
            projection_at(&store, true, &latest, queried_at),
            None,
            "unknown reset deadline has no projection"
        );
        weekly_of(&mut latest).resets_at = Some(reading + 7 * DAY - 30);
        assert_eq!(
            projection_at(&store, true, &latest, queried_at),
            None,
            "insufficient rate information has no projection"
        );
        let mut latest = snapshot(reading, 20.0, Some(reading + 6 * DAY));
        assert!(projection_at(&store, true, &latest, queried_at).is_some());
        assert_eq!(
            store.projection(
                true,
                &latest,
                false,
                ForecastSettings::default(),
                queried_at
            ),
            None,
            "paused, stale or failed data suppresses projection"
        );
        weekly_of(&mut latest).used_percent = 100.0;
        assert_eq!(
            projection_at(&store, true, &latest, queried_at),
            None,
            "exhausted usage has no projection"
        );
        weekly_of(&mut latest).used_percent = 20.0;
        assert_eq!(
            projection_at(&store, true, &latest, reading + 31 * MINUTE),
            None,
            "expired reading has no projection"
        );
        assert!(projection_at(&store, true, &latest, reading - MINUTE).is_some());
        assert_eq!(
            projection_at(&store, true, &latest, reading - MINUTE - 1),
            None,
            "a reading too far ahead of the clock has no projection"
        );
        weekly_of(&mut latest).resets_at = Some(reading + 60);
        assert_eq!(
            projection_at(&store, true, &latest, queried_at),
            None,
            "expired reset deadline has no projection"
        );
        latest.weekly = None;
        assert_eq!(projection_at(&store, true, &latest, queried_at), None);
    }

    #[test]
    fn weekly_pace_bridges_gaps_only_within_a_known_cycle() {
        let directory = TempDir::new("gaps");
        let mut store = open(&directory.file("cycles.json"));
        let reset = Some(NOW + 3 * DAY);
        observe(&mut store, NOW - 24 * HOUR, 10.0, reset);
        observe(&mut store, NOW - 12 * HOUR, 30.0, reset);
        let latest = observe(&mut store, NOW, 50.0, reset);
        // Weekly wall-clock rate bridges overnight observations in a known cycle.
        assert_estimate(
            forecast_at(&store, true, &latest, NOW),
            NOW,
            30 * HOUR,
            "1d 6h",
        );

        assert!(store.clear());
        observe(&mut store, NOW - 24 * HOUR, 10.0, None);
        observe(&mut store, NOW - 12 * HOUR, 30.0, None);
        let latest = observe(&mut store, NOW, 50.0, None);
        assert_eq!(
            forecast_at(&store, true, &latest, NOW),
            Forecast::Learning,
            "unknown cycles cannot bridge overnight gaps"
        );

        assert!(store.clear());
        observe(&mut store, NOW - 30 * MINUTE, 90.0, reset);
        observe(&mut store, NOW - 15 * MINUTE, 10.0, reset);
        let latest = observe(&mut store, NOW, 20.0, reset);
        assert_eq!(
            forecast_at(&store, false, &latest, NOW),
            Forecast::Learning,
            "forecast cannot reuse consumption before reset"
        );
    }

    #[test]
    fn weekly_depletion_uses_observed_consumption() {
        let directory = TempDir::new("weekly");
        let mut store = open(&directory.file("weekly.json"));
        let reset = Some(NOW + 6 * DAY);
        observe(&mut store, NOW - DAY, 0.0, reset);
        observe(&mut store, NOW - 12 * HOUR, 10.0, reset);
        let mut latest = observe(&mut store, NOW, 20.0, reset);
        // 80 percent remaining at 20 percent per day lasts four days.
        assert_estimate(
            forecast_at(&store, true, &latest, NOW),
            NOW,
            4 * DAY,
            "4d 0h",
        );
        assert_eq!(
            forecast_at(&store, true, &latest, NOW + HOUR),
            Forecast::NeedsRefresh,
            "stale observations cannot project depletion"
        );
        weekly_of(&mut latest).resets_at = None;
        // Depletion uses observed consumption rather than time until reset.
        assert_estimate(
            forecast_at(&store, true, &latest, NOW),
            NOW,
            4 * DAY,
            "4d 0h",
        );
        weekly_of(&mut latest).resets_at = Some(NOW + 2 * DAY);
        assert_eq!(
            forecast_at(&store, true, &latest, NOW),
            Forecast::LastsUntilReset,
            "weekly estimate is capped by its actual reset deadline"
        );
        weekly_of(&mut latest).resets_at = Some(NOW);
        assert_eq!(
            forecast_at(&store, true, &latest, NOW),
            Forecast::AwaitingReset,
            "expired reset needs a fresh reading"
        );
        weekly_of(&mut latest).used_percent = 100.0;
        assert_eq!(
            forecast_at(&store, true, &latest, NOW),
            Forecast::LimitReached
        );
    }

    #[test]
    fn recent_consumption_outweighs_older_consumption() {
        let directory = TempDir::new("recent");
        let mut store = open(&directory.file("recent.json"));
        let reset = Some(NOW + 3 * DAY);
        observe(&mut store, NOW - 12 * HOUR, 0.0, reset);
        observe(&mut store, NOW - 6 * HOUR, 6.0, reset);
        let latest = observe(&mut store, NOW, 30.0, reset);
        let projection = projection_at(&store, true, &latest, NOW).unwrap();
        assert!(
            (hours_ahead(&projection, NOW) - 70.0 / 3.0).abs() < 0.001,
            "weekly acceleration favors recent consumption over the old 28-hour estimate"
        );
        // Forecast text uses the same recent weighted rate as the projection.
        assert_estimate(
            forecast_at(&store, true, &latest, NOW),
            NOW,
            84_000,
            "23h 20m",
        );

        assert!(store.clear());
        observe(&mut store, NOW - 12 * HOUR, 0.0, reset);
        observe(&mut store, NOW - 6 * HOUR, 24.0, reset);
        let latest = observe(&mut store, NOW, 30.0, reset);
        let projection = projection_at(&store, true, &latest, NOW).unwrap();
        assert!(
            (hours_ahead(&projection, NOW) - 35.0).abs() < 0.001,
            "weekly slowdown extends the estimate instead of always biasing toward exhaustion"
        );

        assert!(store.clear());
        observe(&mut store, NOW - 12 * HOUR, 0.0, reset);
        observe(&mut store, NOW - 6 * HOUR, 6.0, reset);
        observe(&mut store, NOW - 5 * HOUR, 10.0, reset);
        observe(&mut store, NOW - HOUR, 26.0, reset);
        let latest = observe(&mut store, NOW, 30.0, reset);
        let projection = projection_at(&store, true, &latest, NOW).unwrap();
        assert!(
            (hours_ahead(&projection, NOW) - 70.0 / 3.0).abs() < 0.001,
            "additional irregular readings along the same trend do not change the estimate"
        );

        assert!(store.clear());
        let reset = Some(NOW + 4 * HOUR);
        observe(&mut store, NOW - HOUR, 0.0, reset);
        observe(&mut store, NOW - 30 * MINUTE, 6.0, reset);
        let latest = observe(&mut store, NOW, 30.0, reset);
        let projection = projection_at(&store, false, &latest, NOW).unwrap();
        assert!(
            (hours_ahead(&projection, NOW) - 70.0 / 40.8).abs() < 0.001,
            "five-hour acceleration responds to the last half-hour of usage"
        );

        assert!(store.clear());
        observe(&mut store, NOW - HOUR, 0.0, reset);
        observe(&mut store, NOW - 30 * MINUTE, 30.0, reset);
        let latest = observe(&mut store, NOW, 30.0, reset);
        let projection = projection_at(&store, false, &latest, NOW).unwrap();
        assert!(
            projection.ends_at_reset && (projection.end_remaining_percent - 22.0).abs() < 0.001,
            "recent idle time slows consumption instead of being discarded"
        );
    }

    #[test]
    fn estimates_keep_the_precise_depletion_time() {
        let directory = TempDir::new("duration");
        let mut store = open(&directory.file("duration.json"));
        for (duration, label) in [(55.2, "2d 7h"), (55.35, "2d 7h"), (1.75, "1h 45m")] {
            assert!(store.clear());
            let weekly = duration >= 24.0;
            let span = if weekly { 12 * HOUR } else { 30 * MINUTE };
            let rate = 50.0 / duration;
            let used_before = |seconds: i64| 50.0 - rate * seconds as f64 / 3_600.0;
            let reset = Some(if weekly {
                NOW + 3 * DAY
            } else {
                NOW + 4 * HOUR
            });
            observe(&mut store, NOW - span, used_before(span), reset);
            observe(&mut store, NOW - span / 2, used_before(span / 2), reset);
            let latest = observe(&mut store, NOW, 50.0, reset);
            // Estimates match the reset's local date/time and bracketed duration format.
            assert_estimate(
                forecast_at(&store, weekly, &latest, NOW),
                NOW,
                (duration * 3_600.0_f64).round() as i64,
                label,
            );
            let projection = projection_at(&store, weekly, &latest, NOW).unwrap();
            assert!(
                (hours_ahead(&projection, NOW) - duration).abs() < 0.001,
                "duration formatting retains the precise projected depletion time"
            );
        }
    }

    #[test]
    fn the_first_reading_is_paced_by_its_cycle_average() {
        let directory = TempDir::new("initial");
        let mut store = open(&directory.file("initial.json"));
        let mut reading = snapshot(NOW, 20.0, Some(NOW + 6 * DAY));
        // First reading of 20 percent in one day estimates four days without learning.
        assert_estimate(
            forecast_at(&store, true, &reading, NOW),
            NOW,
            4 * DAY,
            "4d 0h",
        );
        weekly_of(&mut reading).used_percent = 10.0;
        assert_eq!(
            forecast_at(&store, true, &reading, NOW),
            Forecast::LastsUntilReset,
            "initial weekly estimate beyond reset uses the survival label"
        );
        five_hour_of(&mut reading).used_percent = 20.0;
        five_hour_of(&mut reading).resets_at = Some(NOW + 4 * HOUR);
        assert_eq!(
            forecast_at(&store, false, &reading, NOW),
            Forecast::LastsUntilReset,
            "initial five-hour estimate exactly at reset uses the survival label"
        );
        weekly_of(&mut reading).used_percent = 0.0;
        assert_eq!(
            forecast_at(&store, true, &reading, NOW),
            Forecast::NoUsageYet,
            "zero consumption has no invented depletion"
        );
        weekly_of(&mut reading).used_percent = 20.0;
        weekly_of(&mut reading).resets_at = Some(NOW + 7 * DAY - 30);
        assert_eq!(
            forecast_at(&store, true, &reading, NOW),
            Forecast::Learning,
            "first moments of the cycle are not projected"
        );
        weekly_of(&mut reading).resets_at = None;
        assert_eq!(
            forecast_at(&store, true, &reading, NOW),
            Forecast::Learning,
            "unknown cycle start needs observations"
        );

        let reset = Some(NOW + 6 * DAY);
        observe(&mut store, NOW - 2 * HOUR, 60.0, reset);
        observe(&mut store, NOW - HOUR, 10.0, reset);
        let reading = observe(&mut store, NOW, 20.0, reset);
        assert_eq!(
            forecast_at(&store, true, &reading, NOW),
            Forecast::Learning,
            "observed banked reset blocks a misleading cycle average"
        );
    }

    #[test]
    fn measurement_jitter_keeps_the_cycle_average() {
        let directory = TempDir::new("jitter");
        let mut store = open(&directory.file("jitter.json"));
        for weekly in [true, false] {
            let reset = Some(if weekly {
                NOW + 6 * DAY
            } else {
                NOW + 4 * HOUR
            });
            let used = if weekly { 20.0 } else { 25.0 };
            for decline in [0.0005, 0.001] {
                assert!(store.clear());
                observe(&mut store, NOW - 30 * MINUTE, used + decline, reset);
                let latest = observe(&mut store, NOW, used, reset);
                let samples = store.samples();
                assert!(
                    !is_reset(&samples[0], &samples[1], weekly),
                    "a decline within the reset tolerance is not a reset"
                );
                // Sub-tolerance declines must preserve the cycle-average forecast.
                let (seconds, label, hours) = if weekly {
                    (4 * DAY, "4d 0h", 96.0)
                } else {
                    (3 * HOUR, "3h 0m", 3.0)
                };
                assert_estimate(
                    forecast_at(&store, weekly, &latest, NOW),
                    NOW,
                    seconds,
                    label,
                );
                let projection = projection_at(&store, weekly, &latest, NOW).unwrap();
                assert!(
                    !projection.ends_at_reset
                        && (hours_ahead(&projection, NOW) - hours).abs() < 0.001,
                    "cycle-average projection survives tolerated measurement jitter"
                );
            }

            assert!(store.clear());
            observe(&mut store, NOW - 30 * MINUTE, used + 0.002, reset);
            let adjusted = observe(&mut store, NOW, used, reset);
            assert!(
                forecast_at(&store, weekly, &adjusted, NOW) == Forecast::Learning
                    && projection_at(&store, weekly, &adjusted, NOW).is_none(),
                "a decrease beyond the tolerance still invalidates the inferred cycle start"
            );
        }
    }

    #[test]
    fn pace_uses_only_readings_after_a_reset() {
        let directory = TempDir::new("reset");
        let mut store = open(&directory.file("reset.json"));
        for weekly in [true, false] {
            let reset = if weekly {
                NOW + 3 * DAY
            } else {
                NOW + 4 * HOUR
            };
            let step = if weekly { 6 * HOUR } else { 15 * MINUTE };
            for changed_deadline in [false, true] {
                assert!(store.clear());
                // A banked reset decreases usage; a scheduled reset changes its deadline.
                if changed_deadline {
                    observe(&mut store, NOW - 3 * step, 0.0, Some(NOW - 5 * step / 2));
                } else {
                    observe(&mut store, NOW - 3 * step, 95.0, Some(reset));
                }
                observe(&mut store, NOW - 2 * step, 10.0, Some(reset));
                let middle = if weekly { 16.0 } else { 20.0 };
                observe(&mut store, NOW - step, middle, Some(reset));
                let latest = if weekly { 40.0 } else { 30.0 };
                let latest = observe(&mut store, NOW, latest, Some(reset));
                let samples = store.samples();
                assert!(
                    samples.len() == 4 && is_reset(&samples[0], &samples[1], weekly),
                    "old cycles are retained without hiding the reset boundary"
                );
                let projection = projection_at(&store, weekly, &latest, NOW).unwrap();
                let expected_hours = if weekly { 20.0 } else { 1.75 };
                assert!(
                    !projection.ends_at_reset
                        && (hours_ahead(&projection, NOW) - expected_hours).abs() < 0.001,
                    "weighted pace uses only observations after a banked or scheduled reset"
                );
                // Forecast text and projection agree after the reset.
                let (seconds, label) = if weekly {
                    (20 * HOUR, "20h 0m")
                } else {
                    (105 * MINUTE, "1h 45m")
                };
                assert_estimate(
                    forecast_at(&store, weekly, &latest, NOW),
                    NOW,
                    seconds,
                    label,
                );
            }
        }
    }

    #[test]
    fn consumption_before_the_lookback_is_ignored() {
        let directory = TempDir::new("cutoff");
        let mut store = open(&directory.file("cutoff.json"));
        for weekly in [true, false] {
            assert!(store.clear());
            let reset = Some(if weekly {
                NOW + 3 * DAY
            } else {
                NOW + 3 * HOUR
            });
            let horizon = if weekly { 48 * HOUR } else { HOUR };
            observe(&mut store, NOW - horizon - 15 * MINUTE, 0.0, reset);
            observe(&mut store, NOW - horizon, 50.0, reset);
            observe(&mut store, NOW - horizon / 2, 50.0, reset);
            let latest = observe(&mut store, NOW, 50.0, reset);
            assert_eq!(
                forecast_at(&store, weekly, &latest, NOW),
                Forecast::NoRecentIncrease,
                "a burst ending at the cutoff must not affect subsequent idle observations"
            );
            let projection = projection_at(&store, weekly, &latest, NOW).unwrap();
            assert!(
                projection.ends_at_reset && projection.end_remaining_percent == 50.0,
                "old consumption outside the recent horizon does not slope the projection"
            );
        }
    }

    #[test]
    fn recent_pace_needs_its_minimum_span() {
        let directory = TempDir::new("span");
        let mut store = open(&directory.file("span.json"));
        for weekly in [true, false] {
            let reset = Some(if weekly {
                NOW + 3 * DAY
            } else {
                NOW + 4 * HOUR
            });
            let minimum = if weekly { 6 * HOUR } else { 15 * MINUTE };
            for sufficient in [false, true] {
                assert!(store.clear());
                let span = if sufficient { minimum } else { minimum - 1 };
                observe(&mut store, NOW - span, 0.0, reset);
                observe(&mut store, NOW - span / 2, 6.0, reset);
                let latest = observe(&mut store, NOW, 12.0, reset);
                let forecast = forecast_at(&store, weekly, &latest, NOW);
                if sufficient {
                    let projection = projection_at(&store, weekly, &latest, NOW).unwrap();
                    let expected_hours = 88.0 * minimum as f64 / 3_600.0 / 12.0;
                    assert!(
                        matches!(forecast, Forecast::Estimated { .. })
                            && (hours_ahead(&projection, NOW) - expected_hours).abs() < 0.001,
                        "the exact minimum span enables recent weighted pace"
                    );
                } else {
                    assert_eq!(
                        forecast,
                        Forecast::LastsUntilReset,
                        "a span just below the minimum uses the cycle average"
                    );
                }
            }
        }
    }

    #[test]
    fn irregular_polling_preserves_time_weighted_pace() {
        let directory = TempDir::new("splitting");
        let mut store = open(&directory.file("splitting.json"));
        let reset = Some(NOW + 4 * HOUR);
        let scenarios: [&[(i64, f64)]; 2] = [
            &[(-60, 0.0), (-30, 6.0), (0, 30.0)],
            &[
                (-60, 0.0),
                (-50, 2.0),
                (-40, 4.0),
                (-30, 6.0),
                (-29, 6.8),
                (-15, 18.0),
                (-1, 29.2),
                (0, 30.0),
            ],
        ];
        for readings in scenarios {
            assert!(store.clear());
            for &(minutes, used) in readings {
                observe(&mut store, NOW + minutes * MINUTE, used, reset);
            }
            let latest = snapshot(NOW, 30.0, reset);
            let projection = projection_at(&store, false, &latest, NOW).unwrap();
            assert!(
                (hours_ahead(&projection, NOW) - 70.0 / 40.8).abs() < 0.001,
                "irregular five-hour polling preserves time-weighted acceleration"
            );
        }
    }

    #[test]
    fn millisecond_intervals_keep_a_finite_rate() {
        let directory = TempDir::new("subsecond");
        let path = directory.file("subsecond.json");
        let reset = NOW + 4 * HOUR;
        // Files saved by the C# app carry millisecond timestamps.
        let saved = |timestamp_ms: i64, used_percent: f64| HistorySample {
            timestamp_ms,
            weekly: None,
            five_hour: Some(HistoryWindow {
                used_percent,
                reset_at_ms: Some(reset * 1_000),
                window_minutes: Some(300),
                cycle_average_unavailable: false,
                is_imported: false,
            }),
        };
        let samples = [
            saved((NOW - 30 * MINUTE) * 1_000, 20.0),
            saved((NOW - 15 * MINUTE) * 1_000, 35.0),
            saved(NOW * 1_000 - 1, 50.0 - 1.0 / 60_000.0),
        ];
        assert!(history_file::write_samples(&path, &samples));
        let mut store = open(&path);
        let latest = observe(&mut store, NOW, 50.0, Some(reset));
        let projection = projection_at(&store, false, &latest, NOW).unwrap();
        assert!(
            (hours_ahead(&projection, NOW) - 50.0 / 60.0).abs() < 0.001,
            "a millisecond interval keeps a finite steady forecast rate"
        );
        assert!(
            !store.observe(&latest, NOW),
            "duplicate timestamps never reach the interval division"
        );
    }

    #[test]
    fn the_chart_keeps_an_estimate_after_harmless_jitter() {
        let directory = TempDir::new("chart");
        let mut store = open(&directory.file("chart.json"));
        let reset = NOW + 6 * DAY;
        observe(&mut store, NOW - HOUR, 20.0005, Some(reset));
        let latest = observe(&mut store, NOW, 20.0, Some(reset));
        // Chart footer retains its estimate after harmless jitter.
        assert_estimate(
            forecast_at(&store, true, &latest, NOW),
            NOW,
            4 * DAY,
            "4d 0h",
        );
        let projection = projection_at(&store, true, &latest, NOW).unwrap();
        assert!(
            projection.start_ms == NOW * 1_000 && projection.end_ms == (NOW + 4 * DAY) * 1_000,
            "the cycle-average projection stays attached to the latest real reading"
        );
        let chart = store.chart(true, NOW);
        assert!(chart.accepts(&projection));
        assert_eq!(chart.end_ms(Some(&projection)), reset * 1_000);
    }

    #[test]
    fn weekly_only_readings_are_forecast() {
        let directory = TempDir::new("weekly-only");
        let mut store = open(&directory.file("history.json"));
        let reset = NOW + 3 * DAY;
        let mut latest = snapshot(NOW, 0.0, None);
        for hours in [-12, -6, 0] {
            latest = snapshot(NOW + hours * HOUR, 20.0 + hours as f64 / 2.0, Some(reset));
            latest.five_hour = None;
            assert!(store.observe(&latest, NOW));
        }
        let projection = projection_at(&store, true, &latest, NOW).unwrap();
        assert!(store.chart(true, NOW).accepts(&projection));
        // Half a percent per hour leaves 160 hours, beyond the reset in 72.
        assert!(projection.ends_at_reset && projection.end_ms == reset * 1_000);
        assert_eq!(
            forecast_at(&store, true, &latest, NOW),
            Forecast::LastsUntilReset
        );
        assert_eq!(
            forecast_at(&store, false, &latest, NOW),
            Forecast::Unavailable
        );
        assert!(store.chart(false, NOW).samples.is_empty());
    }

    #[test]
    fn recent_pace_mode_never_uses_a_learned_pattern() {
        let directory = TempDir::new("mode");
        let mut store = open(&directory.file("history.json"));
        let reset = Some(NOW + 3 * DAY);
        observe(&mut store, NOW - 12 * HOUR, 0.0, reset);
        observe(&mut store, NOW - 6 * HOUR, 6.0, reset);
        let latest = observe(&mut store, NOW, 30.0, reset);
        let recent_pace = ForecastSettings {
            method: ForecastMethod::RecentPace,
            quiet_hours: QuietHoursMode::Automatic,
        };
        let projection = store
            .projection(true, &latest, true, recent_pace, NOW)
            .unwrap();
        assert!(projection.points.is_empty() && !projection.uses_inactive_hours);
        assert_eq!(
            store.forecast(true, &latest, true, recent_pace, NOW),
            forecast_at(&store, true, &latest, NOW)
        );
    }

    #[test]
    fn projected_remaining_follows_the_path() {
        let point = |timestamp_ms, remaining_percent| ProjectionPoint {
            timestamp_ms,
            remaining_percent,
        };
        let now_ms = NOW * 1_000;
        let hour_ms = HOUR * 1_000;
        let mut projection = Projection {
            start_ms: now_ms,
            end_ms: now_ms + 2 * hour_ms,
            start_remaining_percent: 81.0,
            end_remaining_percent: 60.0,
            ends_at_reset: false,
            uses_daily_pattern: false,
            uses_validated_model: false,
            uses_session_pace: false,
            uses_inactive_hours: false,
            points: vec![
                point(now_ms, 81.0),
                point(now_ms + hour_ms, 80.0),
                point(now_ms + 2 * hour_ms, 60.0),
            ],
        };
        assert_eq!(
            projection.remaining_at(now_ms + hour_ms),
            80.0,
            "projected hover uses the curved path rather than a straight endpoint interpolation"
        );
        assert_eq!(projection.remaining_at(now_ms + hour_ms / 2), 80.5);
        assert_eq!(projection.remaining_at(now_ms + 3 * hour_ms / 2), 70.0);
        assert_eq!(projection.remaining_at(now_ms + 5 * hour_ms), 60.0);

        // A straight line has no points; after depletion the path stays at zero.
        projection.points.clear();
        projection.end_remaining_percent = 0.0;
        assert_eq!(projection.remaining_at(now_ms + hour_ms), 40.5);
        assert_eq!(projection.remaining_at(now_ms + 2 * hour_ms + 1), 0.0);
    }

    #[test]
    fn only_learned_quiet_periods_are_highlighted() {
        let point = |hours: i64, remaining_percent| ProjectionPoint {
            timestamp_ms: (NOW + hours * HOUR) * 1_000,
            remaining_percent,
        };
        let mut projection = Projection {
            start_ms: NOW * 1_000,
            end_ms: (NOW + 4 * HOUR) * 1_000,
            start_remaining_percent: 70.0,
            end_remaining_percent: 60.0,
            ends_at_reset: true,
            uses_daily_pattern: false,
            uses_validated_model: false,
            uses_session_pace: true,
            uses_inactive_hours: true,
            points: vec![point(0, 70.0), point(1, 70.0), point(4, 60.0)],
        };
        assert_eq!(
            projection.quiet_periods(),
            [(NOW * 1_000, (NOW + HOUR) * 1_000)],
            "highlighting affects learned quiet periods only"
        );
        projection.uses_inactive_hours = false;
        assert!(
            projection.quiet_periods().is_empty(),
            "a flat line from a non-inactivity model is not presented as a learned quiet period"
        );
        projection.uses_inactive_hours = true;
        projection.points = vec![point(0, 70.0), point(1, 70.0), point(4, 70.0)];
        assert!(
            projection.quiet_periods().is_empty(),
            "a zero usage baseline does not invent learned quiet hours"
        );
        projection.points = vec![point(0, 70.0), point(1, 0.0), point(4, 0.0)];
        assert!(
            projection.quiet_periods().is_empty(),
            "the exhausted allowance plateau is not highlighted as a quiet period"
        );
    }
}
