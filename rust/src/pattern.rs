//! Learned usage patterns for the weekly forecast.
//!
//! Four candidates are compared on held-out readings: the recent working
//! session's pace, a 24-hour average, an hourly daily pattern, and the
//! session pace paused during learned quiet hours. A candidate is only used
//! when it beats the plain pace forecast on those readings. Hours without
//! observations stay unknown; they are never assumed idle.

use std::cmp::Reverse;
use std::collections::HashMap;

use crate::forecast::{
    cycle_average_rate, recent_rate, session_rate, Projection, ProjectionPoint, WindowReading,
};
use crate::history::{WindowSample, MAX_CONTINUOUS_HOURS, PERCENT_TOLERANCE, RETENTION_DAYS};
use crate::localtime::{
    add_hours, floor_hour, hours_between, LocalDate, LocalTime, TimeZone, MS_PER_DAY, MS_PER_HOUR,
};

/// An hour counts as observed when readings cover at least this much of it.
const MIN_HOUR_COVERAGE: f64 = 0.9;
/// A clock hour joins the daily pattern once this many days have observed it.
const MIN_PATTERN_DAYS: usize = 3;
/// The daily pattern needs this many supported clock hours.
const MIN_PATTERN_HOURS: usize = 8;
/// Hours of the previous day that must be observed to rescale the daily pattern.
const MIN_RESCALE_COVERAGE: f64 = 18.0;
/// The number of recent days of one kind that decide whether an hour is quiet.
const QUIET_DAYS: u8 = 2;
/// How far back held-out checks start.
const VALIDATION_MS: i64 = 7 * MS_PER_DAY;
/// How far back to look for the last working session.
const LAST_SESSION_MS: i64 = 7 * MS_PER_DAY;
/// The least time between the starting points of held-out checks.
const ORIGIN_SPACING_MS: i64 = 3 * MS_PER_HOUR;
/// The age at which a held-out check counts half as much as the newest one.
const VALIDATION_HALF_LIFE_HOURS: f64 = 72.0;
/// How far ahead each check predicts, in hours.
const HORIZONS: [i64; 3] = [1, 6, 24];
/// A validated model needs this many checks, spanning at least a day.
const MIN_VALIDATION_CHECKS: usize = 8;
/// A model replaces the plain pace only when its error is below this share of the pace error.
const REQUIRED_ERROR_RATIO: f64 = 0.9;

/// Usage rates in percent per hour, by hour of the day.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HourlyRates {
    /// One rate for each UTC clock hour.
    Utc([f64; 24]),
    /// Local-time rates: 24 weekday hours followed by 24 weekend hours.
    LocalRoutine([f64; 48]),
}

impl HourlyRates {
    fn at(&self, timestamp_ms: i64, zone: &TimeZone) -> f64 {
        match self {
            Self::Utc(rates) => rates[utc_clock_hour(timestamp_ms)],
            Self::LocalRoutine(rates) => rates[routine_slot(zone.to_local(timestamp_ms))],
        }
    }
}

/// The model chosen for a weekly forecast, with the evidence behind the choice.
#[derive(Clone, Debug, PartialEq)]
pub struct PatternForecast {
    pub rates: HourlyRates,
    /// How many held-out readings the candidates were checked against.
    pub validation_count: usize,
    /// Weighted mean error, in percent per hour, of each candidate.
    pub pace_error: f64,
    pub pattern_error: f64,
    pub daily_average_error: f64,
    pub session_error: f64,
    pub inactive_hours_error: f64,
    /// True when the chosen model beat the plain pace by the required margin.
    pub is_validated: bool,
    pub uses_daily_pattern: bool,
    pub uses_session_pace: bool,
    pub uses_inactive_hours: bool,
    /// The rate for the hour the forecast starts in, when activity is under
    /// way during what is usually a quiet hour.
    pub start_hour_rate: Option<f64>,
}

impl PatternForecast {
    /// Learns a forecast model from weekly readings, oldest first.
    /// `current_rate` is the plain pace estimate, in percent per hour, that
    /// the model has to beat and falls back on. Returns `None` when a straight
    /// line is the better forecast.
    pub fn try_create(
        samples: &[WindowSample],
        current_rate: f64,
        allow_inactive_hours: bool,
        zone: &TimeZone,
    ) -> Option<Self> {
        let candidate = evaluate(samples, current_rate, allow_inactive_hours, zone)
            .filter(|candidate| candidate.is_validated);
        if candidate
            .as_ref()
            .is_some_and(|candidate| candidate.uses_daily_pattern || candidate.uses_inactive_hours)
        {
            return candidate;
        }

        // Keep observed quiet hours as a rough guide even when backtesting
        // favors a constant pace or there are too few validation checks.
        if allow_inactive_hours && samples.len() >= 3 {
            let routine = routine_profile(&build_hours(samples), samples, current_rate, zone)
                .filter(|rates| rates.iter().any(|&rate| rate > 0.0));
            if let Some(routine) = routine {
                return Some(Self {
                    rates: HourlyRates::LocalRoutine(routine),
                    validation_count: 0,
                    pace_error: 0.0,
                    pattern_error: 0.0,
                    daily_average_error: 0.0,
                    session_error: 0.0,
                    inactive_hours_error: 0.0,
                    is_validated: false,
                    uses_daily_pattern: false,
                    uses_session_pace: true,
                    uses_inactive_hours: true,
                    start_hour_rate: is_currently_active(samples)
                        .then(|| recent_active_rate(samples, current_rate)),
                });
            }
        }
        candidate
    }

    /// Follows the model hour by hour from `start_ms` until `remaining`
    /// percent is used up or the window resets at `reset_ms`.
    pub fn project(
        &self,
        start_ms: i64,
        reset_ms: i64,
        remaining: f64,
        zone: &TimeZone,
    ) -> Projection {
        let first_hour = floor_hour(start_ms);
        let mut left = remaining;
        let mut points = vec![ProjectionPoint {
            timestamp_ms: start_ms,
            remaining_percent: left,
        }];
        let mut cursor = start_ms;
        while cursor < reset_ms {
            let mut end = (floor_hour(cursor) + MS_PER_HOUR).min(reset_ms);
            let rate = match self.start_hour_rate {
                Some(rate) if floor_hour(cursor) == first_hour => rate,
                _ => self.rates.at(cursor, zone),
            };
            let increase = rate * hours_between(end, cursor);
            if rate > 0.0 && increase >= left {
                end = add_hours(cursor, left / rate);
                left = 0.0;
            } else {
                left = (left - increase).max(0.0);
            }
            points.push(ProjectionPoint {
                timestamp_ms: end,
                remaining_percent: left,
            });
            cursor = end;
            if left <= 0.0 {
                break;
            }
        }
        Projection {
            start_ms,
            end_ms: cursor,
            start_remaining_percent: remaining,
            end_remaining_percent: left,
            ends_at_reset: cursor >= reset_ms,
            uses_daily_pattern: self.uses_daily_pattern,
            uses_validated_model: self.is_validated,
            uses_session_pace: self.uses_session_pace,
            uses_inactive_hours: self.uses_inactive_hours,
            points,
        }
    }
}

/// Percent used between two times at the given hourly rates.
/// `start_hour_rate` replaces the rate of the clock hour `start_ms` falls in.
pub fn consumption(
    rates: &HourlyRates,
    start_ms: i64,
    end_ms: i64,
    start_hour_rate: Option<f64>,
    zone: &TimeZone,
) -> f64 {
    let first_hour = floor_hour(start_ms);
    let mut used = 0.0;
    let mut cursor = start_ms;
    while cursor < end_ms {
        let next = (floor_hour(cursor) + MS_PER_HOUR).min(end_ms);
        let rate = match start_hour_rate {
            Some(rate) if floor_hour(cursor) == first_hour => rate,
            _ => rates.at(cursor, zone),
        };
        used += rate * hours_between(next, cursor);
        cursor = next;
    }
    used
}

/// What was observed during one UTC clock hour.
#[derive(Clone, Copy, Default)]
struct Hour {
    used: f64,
    /// The part of the hour, from zero to one, that readings cover.
    coverage: f64,
}

/// Observed hours by their start time.
type Hours = HashMap<i64, Hour>;

#[derive(Default)]
struct Errors {
    pace: f64,
    pattern: f64,
    daily: f64,
    session: f64,
    inactive: f64,
}

/// The held-out checks a candidate took part in.
#[derive(Default)]
struct Checks {
    count: usize,
    first_origin_ms: i64,
    last_origin_ms: i64,
}

impl Checks {
    fn record(&mut self, origin_ms: i64) {
        if self.count == 0 {
            self.first_origin_ms = origin_ms;
        }
        self.last_origin_ms = origin_ms;
        self.count += 1;
    }

    /// Evidence gate: enough checks, from more than one day.
    fn are_sufficient(&self) -> bool {
        self.count >= MIN_VALIDATION_CHECKS
            && hours_between(self.last_origin_ms, self.first_origin_ms) >= 24.0
    }
}

/// Scores every candidate with rolling-origin evaluation: at each earlier
/// reading, predict later readings from only what was known then.
fn evaluate(
    samples: &[WindowSample],
    current_rate: f64,
    allow_inactive_hours: bool,
    zone: &TimeZone,
) -> Option<PatternForecast> {
    let latest = samples.last().filter(|_| samples.len() >= 3)?;
    let hours = build_hours(samples);
    let current_profile = daily_profile(&hours, latest.timestamp_ms, current_rate);
    let current_routine = allow_inactive_hours
        .then(|| routine_profile(&hours, samples, current_rate, zone))
        .flatten();
    let current_start_rate = (current_routine.is_some() && is_currently_active(samples))
        .then(|| recent_active_rate(samples, current_rate));

    let mut errors = Errors::default();
    let mut checks = Checks::default();
    let mut routine_checks = Checks::default();
    let mut profile_checks = Checks::default();
    let mut validation_weight = 0.0;
    let mut next_origin_ms = latest.timestamp_ms - VALIDATION_MS;
    for index in 0..samples.len() - 1 {
        let origin = &samples[index];
        if origin.timestamp_ms < next_origin_ms
            || origin.window.used_percent >= 100.0
            || origin.window.window_minutes != latest.window.window_minutes
        {
            continue;
        }
        let training = &samples[..=index];
        let reading = WindowReading::from_sample(origin);
        let Some(rate) = recent_rate(true, &reading, training)
            .or_else(|| cycle_average_rate(&reading, training))
        else {
            continue;
        };
        // Profiles use only completed hours before this origin; the held-out
        // readings must not affect any candidate's training data.
        let training_hours = build_hours(training);
        let profile =
            daily_profile(&training_hours, origin.timestamp_ms, rate).map(HourlyRates::Utc);
        let daily = daily_rate(training, rate);
        let session = session_rate(&reading, training).unwrap_or(rate);
        let routine = allow_inactive_hours
            .then(|| routine_profile(&training_hours, training, session, zone))
            .flatten()
            .map(HourlyRates::LocalRoutine);
        let start_rate = (routine.is_some() && is_currently_active(training))
            .then(|| recent_active_rate(training, session));
        next_origin_ms = origin.timestamp_ms + ORIGIN_SPACING_MS;

        let mut target = index + 1;
        for horizon in HORIZONS {
            let deadline = origin.timestamp_ms + horizon * MS_PER_HOUR;
            let mut crossed_reset = false;
            while target < samples.len() {
                if samples[target].window.window_minutes != origin.window.window_minutes
                    || samples[target].is_reset_from(&samples[target - 1])
                {
                    crossed_reset = true;
                    break;
                }
                if samples[target].timestamp_ms >= deadline {
                    break;
                }
                target += 1;
            }
            if crossed_reset || target >= samples.len() {
                break;
            }
            let actual = &samples[target];
            let lateness_allowed = if horizon == 1 { 0.25 } else { 1.0 };
            if hours_between(actual.timestamp_ms, deadline) > lateness_allowed
                || actual.window.used_percent >= 100.0
            {
                continue;
            }
            let elapsed = hours_between(actual.timestamp_ms, origin.timestamp_ms);
            let age = hours_between(latest.timestamp_ms, origin.timestamp_ms);
            let weight = 0.5f64.powf(age / VALIDATION_HALF_LIFE_HOURS);
            let increase = (actual.window.used_percent - origin.window.used_percent).max(0.0);
            let allowance = 100.0 - origin.window.used_percent;
            // Normalize by the horizon so a 24-hour error cannot drown out
            // the near-term forecasts used during an active session.
            let error_of = |predicted: f64| (predicted.min(allowance) - increase).abs() / elapsed;
            let predicted_by = |rates: &HourlyRates, start_rate: Option<f64>| {
                consumption(
                    rates,
                    origin.timestamp_ms,
                    actual.timestamp_ms,
                    start_rate,
                    zone,
                )
            };
            let pace_error = error_of(rate * elapsed);
            let session_error = error_of(session * elapsed);
            errors.pace += pace_error * weight;
            errors.daily += error_of(daily * elapsed) * weight;
            errors.session += session_error * weight;
            errors.inactive += routine.as_ref().map_or(session_error, |rates| {
                error_of(predicted_by(rates, start_rate))
            }) * weight;
            errors.pattern += profile
                .as_ref()
                .map_or(pace_error, |rates| error_of(predicted_by(rates, None)))
                * weight;
            checks.record(origin.timestamp_ms);
            validation_weight += weight;
            if routine.is_some() {
                routine_checks.record(origin.timestamp_ms);
            }
            if profile.is_some() {
                profile_checks.record(origin.timestamp_ms);
            }
        }
    }

    // Require multiple evaluation days and a material improvement; a tie
    // keeps the simpler forecast. These are evidence gates, not usage rates.
    let pattern_ready = current_profile.is_some() && profile_checks.are_sufficient();
    let use_inactive = current_routine.is_some()
        && routine_checks.are_sufficient()
        && errors.inactive <= errors.session
        && errors.inactive < errors.daily
        && (!pattern_ready || errors.inactive <= errors.pattern);
    let use_pattern =
        !use_inactive && pattern_ready && errors.pattern < errors.daily.min(errors.session);
    let use_session = use_inactive || (!use_pattern && errors.session < errors.daily);
    let rates = match (current_routine, current_profile) {
        (Some(routine), _) if use_inactive => HourlyRates::LocalRoutine(routine),
        (_, Some(profile)) if use_pattern => HourlyRates::Utc(profile),
        _ => {
            let rate = if use_session {
                session_rate(&WindowReading::from_sample(latest), samples).unwrap_or(current_rate)
            } else {
                daily_rate(samples, current_rate)
            };
            HourlyRates::Utc([rate; 24])
        }
    };
    let chosen_error = if use_inactive {
        errors.inactive
    } else if use_pattern {
        errors.pattern
    } else if use_session {
        errors.session
    } else {
        errors.daily
    };
    let mean = |error: f64| {
        if checks.count == 0 {
            0.0
        } else {
            error / validation_weight
        }
    };
    Some(PatternForecast {
        rates,
        validation_count: checks.count,
        pace_error: mean(errors.pace),
        pattern_error: mean(errors.pattern),
        daily_average_error: mean(errors.daily),
        session_error: mean(errors.session),
        inactive_hours_error: mean(errors.inactive),
        is_validated: checks.are_sufficient()
            && errors.pace > 0.001
            && chosen_error < errors.pace * REQUIRED_ERROR_RATIO,
        uses_daily_pattern: use_pattern,
        uses_session_pace: use_session,
        uses_inactive_hours: use_inactive,
        start_hour_rate: current_start_rate.filter(|_| use_inactive),
    })
}

/// Percent per hour over the last 24 hours of the current cycle, or
/// `fallback` without three readings spanning six hours.
fn daily_rate(samples: &[WindowSample], fallback: f64) -> f64 {
    let Some(latest) = samples.last() else {
        return fallback;
    };
    let end = samples.len() - 1;
    let cutoff = latest.timestamp_ms - MS_PER_DAY;
    let mut start = end;
    while start > 0 && samples[start - 1].timestamp_ms >= cutoff {
        let (before, after) = (&samples[start - 1], &samples[start]);
        if before.window.window_minutes != after.window.window_minutes
            || after.is_reset_from(before)
            || (hours_between(after.timestamp_ms, before.timestamp_ms) > MAX_CONTINUOUS_HOURS
                && !after.shares_reset_with(before))
        {
            break;
        }
        start -= 1;
    }
    let hours = hours_between(latest.timestamp_ms, samples[start].timestamp_ms);
    if end - start >= 2 && hours >= 6.0 {
        (latest.window.used_percent - samples[start].window.used_percent).max(0.0) / hours
    } else {
        fallback
    }
}

/// Spreads each interval's usage over the clock hours it covers.
fn build_hours(samples: &[WindowSample]) -> Hours {
    let mut hours = Hours::new();
    let Some(latest) = samples.last() else {
        return hours;
    };
    for pair in samples.windows(2) {
        let (before, after) = (&pair[0], &pair[1]);
        if before.window.window_minutes != latest.window.window_minutes
            || before.window.window_minutes != after.window.window_minutes
            || after.is_reset_from(before)
        {
            continue;
        }
        let elapsed = hours_between(after.timestamp_ms, before.timestamp_ms);
        let increase = (after.window.used_percent - before.window.used_percent).max(0.0);
        if elapsed <= 0.0
            || before.window.used_percent >= 100.0
            || after.window.used_percent >= 100.0
        {
            continue;
        }
        // A long positive gap cannot reveal which hours were busy. A flat
        // gap is evidence of inactivity only with a matching reset deadline.
        if elapsed > MAX_CONTINUOUS_HOURS && (increase > 0.0 || !after.shares_reset_with(before)) {
            continue;
        }
        let rate = increase / elapsed;
        let mut cursor = before.timestamp_ms;
        while cursor < after.timestamp_ms {
            let hour_start = floor_hour(cursor);
            let end = (hour_start + MS_PER_HOUR).min(after.timestamp_ms);
            let duration = hours_between(end, cursor);
            let hour = hours.entry(hour_start).or_default();
            hour.used += rate * duration;
            hour.coverage += duration;
            cursor = end;
        }
    }
    hours
}

/// The typical rate of each UTC clock hour, from the same hour on earlier
/// days with newer days weighted more. `None` unless enough hours have been
/// observed on enough days.
fn daily_profile(hours: &Hours, origin_ms: i64, fallback_rate: f64) -> Option<[f64; 24]> {
    let completed = floor_hour(origin_ms);
    let midnight = completed.div_euclid(MS_PER_DAY) * MS_PER_DAY;
    let observed = |hour_start: i64| {
        hours
            .get(&hour_start)
            .filter(|hour| hour.coverage >= MIN_HOUR_COVERAGE)
    };
    let mut rates = [fallback_rate; 24];
    let mut supported = 0;
    for (clock_hour, rate) in rates.iter_mut().enumerate() {
        let mut same_hour = midnight + clock_hour as i64 * MS_PER_HOUR;
        if same_hour >= completed {
            same_hour -= MS_PER_DAY;
        }
        let mut usage = 0.0;
        let mut coverage = 0.0;
        let mut days = 0;
        for day in 0..RETENTION_DAYS {
            let Some(hour) = observed(same_hour - day * MS_PER_DAY) else {
                continue;
            };
            let weight = 0.5f64.powf(day as f64 / 7.0);
            usage += hour.used * weight;
            coverage += hour.coverage * weight;
            days += 1;
        }
        if days >= MIN_PATTERN_DAYS {
            *rate = usage / coverage;
            supported += 1;
        }
    }
    if supported < MIN_PATTERN_HOURS {
        return None;
    }

    // Separate the daily shape from its current level. Match the previous
    // day's observed consumption against those same hours of the profile;
    // this learns a scale factor instead of projecting old activity levels.
    let mut recent_usage = 0.0;
    let mut expected_usage = 0.0;
    let mut recent_coverage = 0.0;
    for hours_ago in (1..=24).rev() {
        let time = completed - hours_ago * MS_PER_HOUR;
        let Some(hour) = observed(time) else {
            continue;
        };
        recent_usage += hour.used;
        expected_usage += rates[utc_clock_hour(time)] * hour.coverage;
        recent_coverage += hour.coverage;
    }
    if recent_coverage >= MIN_RESCALE_COVERAGE && expected_usage > 0.001 {
        let scale = recent_usage / expected_usage;
        for rate in &mut rates {
            *rate *= scale;
        }
    }
    Some(rates)
}

/// What the most recent days of one kind show for one local clock hour.
#[derive(Clone, Copy, Default)]
struct RoutineSlot {
    days: u8,
    active: bool,
    first_date: Option<LocalDate>,
}

/// The recent working pace with learned quiet hours set to zero, by local
/// weekday and weekend hour. `None` when no quiet period has been learned.
fn routine_profile(
    hours: &Hours,
    samples: &[WindowSample],
    fallback_rate: f64,
    zone: &TimeZone,
) -> Option<[f64; 48]> {
    let completed = floor_hour(samples.last()?.timestamp_ms);
    let earliest = completed - RETENTION_DAYS * MS_PER_DAY;
    let mut observed: Vec<(i64, Hour)> = hours
        .iter()
        .filter(|(&start, hour)| {
            (earliest..completed).contains(&start) && hour.coverage >= MIN_HOUR_COVERAGE
        })
        .map(|(&start, &hour)| (start, hour))
        .collect();
    observed.sort_unstable_by_key(|&(start, _)| Reverse(start));

    // Weekday and weekend routines are learned separately so a quiet
    // weekend cannot turn a busy weekday morning into predicted sleep.
    let mut slots = [RoutineSlot::default(); 48];
    for (start, hour) in observed {
        let local = zone.to_local(start);
        let slot = &mut slots[routine_slot(local)];
        // Use the two most recent observed days, so a recent schedule
        // change supersedes an older weekend or previous work routine.
        if slot.days == QUIET_DAYS || slot.first_date == Some(local.date()) {
            continue;
        }
        slot.first_date.get_or_insert(local.date());
        slot.days += 1;
        slot.active |= hour.used > PERCENT_TOLERANCE;
    }

    let quiet = slots.map(|slot| slot.days == QUIET_DAYS && !slot.active);
    // Only repeated, contiguous quiet periods count; one flat hour
    // can simply be low consumption or percentage rounding.
    let sustained: [bool; 48] = std::array::from_fn(|slot| {
        let (first_slot, clock_hour) = (slot / 24 * 24, slot % 24);
        let quiet_at = |hours_later: usize| quiet[first_slot + (clock_hour + hours_later) % 24];
        quiet[slot]
            && ((quiet_at(22) && quiet_at(23))
                || (quiet_at(23) && quiet_at(1))
                || (quiet_at(1) && quiet_at(2)))
    });
    let quiet_count = sustained.iter().filter(|&&quiet| quiet).count();
    if quiet_count == 0 || quiet_count == sustained.len() {
        return None;
    }
    let active_rate = recent_active_rate(samples, fallback_rate);
    Some(sustained.map(|quiet| if quiet { 0.0 } else { active_rate }))
}

/// The pace of the session under way, or else of the most recent session in
/// the last week, so a forecast made after sleep or a weekend resumes at a
/// working pace.
fn recent_active_rate(samples: &[WindowSample], fallback_rate: f64) -> f64 {
    let Some(latest) = samples.last() else {
        return fallback_rate;
    };
    if is_currently_active(samples) {
        if let Some(rate) = session_rate(&WindowReading::from_sample(latest), samples) {
            return rate;
        }
    }
    let cutoff = latest.timestamp_ms - LAST_SESSION_MS;
    for index in (1..samples.len()).rev() {
        let (before, after) = (&samples[index - 1], &samples[index]);
        if after.timestamp_ms < cutoff
            || before.window.window_minutes != after.window.window_minutes
        {
            break;
        }
        if after.is_reset_from(before) {
            continue;
        }
        let elapsed = hours_between(after.timestamp_ms, before.timestamp_ms);
        let increase = after.window.used_percent - before.window.used_percent;
        if elapsed <= 0.0 || elapsed > MAX_CONTINUOUS_HOURS || increase <= PERCENT_TOLERANCE {
            continue;
        }
        return session_rate(&WindowReading::from_sample(after), &samples[..=index])
            .unwrap_or(fallback_rate);
    }
    fallback_rate
}

/// True when usage rose within the last hour of the history.
fn is_currently_active(samples: &[WindowSample]) -> bool {
    let Some(latest) = samples.last() else {
        return false;
    };
    let cutoff = latest.timestamp_ms - MS_PER_HOUR;
    for pair in samples.windows(2).rev() {
        let (before, after) = (&pair[0], &pair[1]);
        if after.timestamp_ms <= cutoff
            || after.is_reset_from(before)
            || before.window.window_minutes != after.window.window_minutes
        {
            break;
        }
        if hours_between(after.timestamp_ms, before.timestamp_ms) <= MAX_CONTINUOUS_HOURS
            && after.window.used_percent > before.window.used_percent + PERCENT_TOLERANCE
        {
            return true;
        }
    }
    false
}

fn utc_clock_hour(timestamp_ms: i64) -> usize {
    timestamp_ms.div_euclid(MS_PER_HOUR).rem_euclid(24) as usize
}

/// The index of a local time's hour in a routine: weekdays first, then weekends.
fn routine_slot(local: LocalTime) -> usize {
    local.hour() as usize + if local.date().is_weekend() { 24 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forecast::{Forecast, ForecastMethod, ForecastSettings, QuietHoursMode};
    use crate::history::tests::TempDir;
    use crate::history::{HistoryStore, HistoryWindow};
    use crate::localtime::tests::{eastern, utc_ms};
    use crate::model::{LimitWindow, Usage, WEEKLY_MINUTES};

    const WEEK_MS: i64 = 7 * MS_PER_DAY;
    const ZONES: [fn() -> TimeZone; 2] = [TimeZone::utc, eastern];

    fn sample(timestamp_ms: i64, used_percent: f64, reset_at_ms: i64) -> WindowSample {
        WindowSample {
            timestamp_ms,
            window: HistoryWindow {
                used_percent,
                reset_at_ms: Some(reset_at_ms),
                window_minutes: Some(WEEKLY_MINUTES),
                cycle_average_unavailable: false,
                is_imported: false,
            },
        }
    }

    /// Hourly weekly readings for the `days` before `latest_ms`, resetting
    /// every seven days. `used_during` gives the percent used in the hour
    /// that starts at a time.
    fn hourly(latest_ms: i64, days: i64, used_during: impl Fn(i64) -> f64) -> Vec<WindowSample> {
        let first = latest_ms - days * MS_PER_DAY;
        let mut reset = first + WEEK_MS;
        let mut used = 0.0;
        let mut samples = Vec::new();
        let mut time = first;
        while time <= latest_ms {
            if time >= reset {
                used = 0.0;
                reset += WEEK_MS;
            }
            samples.push(sample(time, used, reset));
            used += used_during(time);
            time += MS_PER_HOUR;
        }
        samples
    }

    /// One percent an hour from 08:00 to 16:00 UTC every day, or a third of a
    /// percent around the clock.
    fn daily_pattern(latest_ms: i64, days: i64, steady: bool) -> Vec<WindowSample> {
        hourly(latest_ms, days, |time| {
            if steady {
                1.0 / 3.0
            } else if (8..16).contains(&utc_clock_hour(time)) {
                1.0
            } else {
                0.0
            }
        })
    }

    /// Three weeks of work from 08:00 to 20:00 local time on weekdays, at
    /// double the pace during the last two days.
    fn routine(latest_ms: i64, zone: &TimeZone) -> Vec<WindowSample> {
        hourly(latest_ms, 21, |time| {
            let local = zone.to_local(time);
            if local.date().is_weekend() || !(8..20).contains(&local.hour()) {
                0.0
            } else if time >= latest_ms - 2 * MS_PER_DAY {
                0.6
            } else {
                0.3
            }
        })
    }

    fn local_ms(zone: &TimeZone, date: LocalDate, hour: u32) -> i64 {
        zone.to_utc(LocalTime::new(date, hour, 0, 0))
    }

    /// The plain pace a forecast would use for the latest reading.
    fn plain_rate(samples: &[WindowSample]) -> f64 {
        let reading = WindowReading::from_sample(samples.last().unwrap());
        recent_rate(true, &reading, samples)
            .or_else(|| cycle_average_rate(&reading, samples))
            .unwrap_or(0.0)
    }

    fn reading(sample: &WindowSample) -> Usage {
        let weekly = LimitWindow {
            used_percent: sample.window.used_percent,
            window_minutes: WEEKLY_MINUTES,
            resets_at: sample.window.reset_at_ms.map(|reset| reset / 1_000),
        };
        Usage::from_windows(sample.timestamp_ms / 1_000, None, Some(weekly))
    }

    fn readings(samples: &[WindowSample]) -> Vec<Usage> {
        samples.iter().map(reading).collect()
    }

    fn project(
        store: &HistoryStore,
        usage: &Usage,
        settings: ForecastSettings,
        now: i64,
    ) -> Option<Projection> {
        store.projection(true, usage, true, settings, now)
    }

    #[test]
    fn a_recurring_daily_pattern_must_beat_the_pace_forecast() {
        for zone in ZONES.map(|zone| zone()) {
            let latest = utc_ms(2026, 10, 8, 16, 0);
            let samples = daily_pattern(latest, 9, false);
            let model =
                PatternForecast::try_create(&samples, plain_rate(&samples), true, &zone).unwrap();
            assert!(
                model.validation_count >= 8 && model.pattern_error < model.pace_error * 0.9,
                "a recurring daily pattern is selected only after beating the pace forecast on held-out readings"
            );
            let projection = model.project(latest, latest + 2 * MS_PER_DAY, 20.0, &zone);
            assert!(
                projection.points.len() > 2
                    && projection.points[1].remaining_percent == 20.0
                    && projection.ends_at_reset
                    && (projection.end_remaining_percent - 4.0).abs() < 0.001,
                "quiet hours are flat and busy hours consume observed allowance until reset"
            );
            let depleted = model.project(latest, latest + 3 * MS_PER_DAY, 10.5, &zone);
            assert!(
                !depleted.ends_at_reset
                    && depleted.end_remaining_percent == 0.0
                    && depleted.end_ms == utc_ms(2026, 10, 10, 10, 30),
                "daily pattern stops exactly at depletion inside a busy hour"
            );
            let mut previous = f64::MAX;
            for point in &depleted.points {
                assert!(
                    point.remaining_percent >= 0.0 && point.remaining_percent <= previous,
                    "projection never invents replenishment or negative allowance"
                );
                previous = point.remaining_percent;
            }

            let steady = daily_pattern(latest, 9, true);
            assert_eq!(
                PatternForecast::try_create(&steady, plain_rate(&steady), true, &zone),
                None,
                "steady consumption keeps the simpler forecast when seasonal prediction has no advantage"
            );
            let sparse: Vec<WindowSample> = steady.iter().step_by(6).copied().collect();
            assert_eq!(
                PatternForecast::try_create(&sparse, plain_rate(&sparse), true, &zone),
                None,
                "long positive gaps cannot be used to fabricate hourly usage patterns"
            );
            let short_history = &samples[samples.len() - 49..];
            let short_model =
                PatternForecast::try_create(short_history, plain_rate(short_history), true, &zone);
            assert!(
                short_model.is_none_or(|model| !model.uses_daily_pattern),
                "two days of observations do not establish a recurring pattern"
            );
        }
    }

    fn assert_store_agrees(samples: &[WindowSample], latest_ms: i64, depletes: bool) {
        let directory = TempDir::new("pattern");
        let path = directory.file("history.json");
        let now = latest_ms / 1_000;
        let settings = ForecastSettings::default();
        let mut store = HistoryStore::open(&path, TimeZone::utc(), now);
        let readings = readings(samples);
        store.import(&readings, false, true, now);
        let latest = readings.last().unwrap();
        let projection = project(&store, latest, settings, now).unwrap();
        assert!(
            projection.uses_daily_pattern && projection.points.len() > 2,
            "store returns a validated path instead of an artificial curved trend"
        );
        let forecast = store.forecast(true, latest, true, settings, now);
        if depletes {
            let hours_into_day = 8.0 + (100.0 - 48.0 * 1.9) / 1.9;
            let expected_end = add_hours(utc_ms(2026, 10, 9, 0, 0), hours_into_day);
            assert!(
                !projection.ends_at_reset && (projection.end_ms - expected_end).abs() <= 1,
                "the seasonal depletion estimate retains the calculated endpoint"
            );
            let seconds = ((expected_end - latest_ms) as f64 / 1_000.0).round() as i64;
            let Forecast::Estimated {
                depletes_at_ms,
                duration_seconds,
            } = forecast
            else {
                panic!("expected an estimate, got {forecast:?}");
            };
            assert!((depletes_at_ms - expected_end).abs() <= 1 && duration_seconds == seconds);
            assert!(
                forecast.text(now).ends_with("(20h 37m)"),
                "validated patterns use the same estimated date and duration format as recent pace"
            );
        } else {
            assert_eq!(
                forecast,
                Forecast::LastsUntilReset,
                "forecast text and seasonal path agree about lasting through reset"
            );
        }
        assert_eq!(
            project(&store, latest, settings, now + 31 * 60),
            None,
            "stale snapshots still suppress the seasonal forecast"
        );
        assert!(store.clear());
        let projection = project(&store, latest, settings, now).unwrap();
        assert!(
            !projection.uses_daily_pattern && projection.points.is_empty(),
            "clearing history discards learned patterns and restores the cycle-average fallback"
        );
    }

    #[test]
    fn the_store_projects_along_a_validated_pattern() {
        let latest = utc_ms(2026, 10, 8, 16, 0);
        assert_store_agrees(&daily_pattern(latest, 6, false), latest, false);
        let mut faster = daily_pattern(latest, 6, false);
        for sample in &mut faster {
            sample.window.used_percent *= 1.9;
        }
        assert_store_agrees(&faster, latest, true);
    }

    #[test]
    fn weekday_and_weekend_quiet_hours_are_learned() {
        for zone in ZONES {
            let friday = LocalDate::from_ymd(2026, 10, 9);
            let latest = local_ms(&zone(), friday, 12);
            let samples = routine(latest, &zone());
            let model =
                PatternForecast::try_create(&samples, plain_rate(&samples), true, &zone()).unwrap();
            assert!(
                model.uses_inactive_hours && matches!(model.rates, HourlyRates::LocalRoutine(_)),
                "weekday and weekend inactivity can beat constant recent pace on held-out readings"
            );
            let saturday = local_ms(&zone(), friday.plus_days(1), 0);
            let monday = local_ms(&zone(), friday.plus_days(3), 8);
            assert_eq!(
                consumption(&model.rates, saturday, monday, None, &zone()),
                0.0,
                "repeated quiet nights and weekends do not consume projected allowance"
            );
            assert!(
                consumption(&model.rates, monday, monday + MS_PER_HOUR, None, &zone()) > 0.45,
                "work resumes at the recent increased pace rather than the older working rate"
            );
            let projection = model.project(latest, monday + 4 * MS_PER_HOUR, 80.0, &zone());
            assert!(
                projection.uses_inactive_hours
                    && projection.points.len() > 24
                    && projection.ends_at_reset,
                "inactivity-aware projection includes hourly points for hover inspection"
            );

            let night = local_ms(&zone(), friday.plus_days(-2), 2);
            let mut unusual_activity = routine(night + MS_PER_HOUR, &zone());
            unusual_activity.pop();
            unusual_activity.last_mut().unwrap().window.used_percent += 0.9;
            let active_at_night = PatternForecast::try_create(
                &unusual_activity,
                plain_rate(&unusual_activity),
                true,
                &zone(),
            )
            .unwrap();
            assert!(
                active_at_night.uses_inactive_hours
                    && active_at_night
                        .start_hour_rate
                        .is_some_and(|rate| rate > 0.0),
                "actual recent activity overrides a learned quiet hour at the forecast start"
            );
            let projection = active_at_night.project(night, night + 2 * MS_PER_HOUR, 80.0, &zone());
            assert!(
                projection.points[1].remaining_percent < 80.0,
                "working during usual sleep hours consumes allowance immediately"
            );

            // A new weekend reset must retain the previous weeks' learned routine.
            let mut samples = routine(saturday + 12 * MS_PER_HOUR, &zone());
            let first_after_reset = samples.last_mut().unwrap();
            first_after_reset.window.used_percent = 0.0;
            first_after_reset.window.reset_at_ms = Some(first_after_reset.timestamp_ms + WEEK_MS);
            let first_after_reset = *first_after_reset;
            let now = first_after_reset.timestamp_ms / 1_000;
            let directory = TempDir::new("routine");
            let path = directory.file("history.json");
            let settings = ForecastSettings::default();
            let mut store = HistoryStore::open(&path, zone(), now);
            store.import(&readings(&samples), false, true, now);
            let snapshot = reading(&first_after_reset);
            let projection = project(&store, &snapshot, settings, now).unwrap();
            assert!(
                projection.uses_inactive_hours && projection.points.len() > 2,
                "one reading after reset can reuse prior-cycle patterns without relearning from zero"
            );
            assert!(
                store.samples()[0].timestamp_ms < first_after_reset.timestamp_ms - WEEK_MS,
                "learning retains prior weeks across resets"
            );
            let store = HistoryStore::open(&path, zone(), now);
            assert!(
                project(&store, &snapshot, settings, now)
                    .unwrap()
                    .uses_inactive_hours,
                "learned inactivity survives restart through retained usage readings"
            );
        }
    }

    #[test]
    fn observed_quiet_nights_guide_a_rough_forecast_after_a_reset() {
        for zone in ZONES {
            let wednesday = LocalDate::from_ymd(2026, 10, 7);
            let latest_ms = local_ms(&zone(), wednesday, 21);
            let now = latest_ms / 1_000;
            let three_weeks = routine(latest_ms, &zone());
            let mut samples = three_weeks[three_weeks.len() - 49..].to_vec();
            let latest = samples.last_mut().unwrap();
            latest.window.used_percent = 1.0;
            latest.window.reset_at_ms = Some(latest_ms + 160 * MS_PER_HOUR);
            let mut latest = *latest;

            let model = PatternForecast::try_create(&samples, 0.0, true, &zone()).unwrap();
            assert!(
                model.uses_inactive_hours && !model.is_validated,
                "two observed quiet nights can guide a rough forecast without enough backtesting"
            );
            let night = local_ms(&zone(), wednesday.plus_days(1), 2);
            let morning = local_ms(&zone(), wednesday.plus_days(1), 10);
            let during =
                |start: i64| consumption(&model.rates, start, start + MS_PER_HOUR, None, &zone());
            assert!(
                during(night) == 0.0 && during(morning) > 0.0,
                "the rough forecast pauses at observed quiet hours and resumes at a learned working pace"
            );
            let one_day = &samples[samples.len() - 25..];
            let incomplete = PatternForecast::try_create(one_day, 0.0, true, &zone());
            assert!(
                incomplete.is_none_or(|model| !model.uses_inactive_hours),
                "one observed night does not establish a quiet routine"
            );
            let mut flat = samples.clone();
            for sample in &mut flat {
                sample.window.used_percent = 0.0;
            }
            assert_eq!(
                PatternForecast::try_create(&flat, 0.0, true, &zone()),
                None,
                "a quiet routine alone does not invent a positive working pace"
            );

            let mut active = samples.clone();
            for minutes in [15, 30] {
                let mut later = latest;
                later.timestamp_ms += minutes * 60_000;
                later.window.used_percent += minutes as f64 / 30.0;
                active.push(later);
            }
            let active_model = PatternForecast::try_create(&active, 0.0, true, &zone()).unwrap();
            assert!(
                active_model.uses_inactive_hours
                    && active_model.start_hour_rate.is_some_and(|rate| rate > 0.0),
                "activity after a reset overrides a usual quiet hour"
            );

            let directory = TempDir::new("rough-routine");
            let mut store = HistoryStore::open(directory.file("history.json"), zone(), now);
            let mut imported = readings(&samples);
            store.import(&imported, false, true, now);
            let snapshot = reading(&latest);
            let mut settings = ForecastSettings::default();
            let projection = project(&store, &snapshot, settings, now).unwrap();
            assert!(
                projection.uses_inactive_hours
                    && !projection.uses_validated_model
                    && projection.points.len() > 2
                    && projection.ends_at_reset,
                "a new cycle can use the rough routine with only one percent consumed"
            );
            assert_eq!(
                store.forecast(true, &snapshot, true, settings, now),
                Forecast::LastsUntilReset,
                "rough projection and forecast text agree"
            );

            settings.quiet_hours = QuietHoursMode::Off;
            assert!(
                !project(&store, &snapshot, settings, now)
                    .unwrap()
                    .uses_inactive_hours,
                "disabling quiet hours removes the learned inactivity forecast"
            );
            settings.quiet_hours = QuietHoursMode::Automatic;
            assert!(
                project(&store, &snapshot, settings, now)
                    .unwrap()
                    .uses_inactive_hours,
                "reenabling quiet hours invalidates the cached forecast"
            );
            settings.method = ForecastMethod::RecentPace;
            let recent = project(&store, &snapshot, settings, now).unwrap();
            assert!(
                recent.points.is_empty() && !recent.uses_inactive_hours,
                "recent-pace mode uses the straight cycle pace"
            );
            settings.method = ForecastMethod::LearnedRoutine;
            assert!(
                project(&store, &snapshot, settings, now)
                    .unwrap()
                    .uses_inactive_hours,
                "switching back restores the learned routine"
            );

            latest.window.used_percent = 99.0;
            *imported.last_mut().unwrap() = reading(&latest);
            assert!(store.clear());
            store.import(&imported, true, true, now);
            let snapshot = reading(&latest);
            let projection = project(&store, &snapshot, settings, now).unwrap();
            let seconds = ((projection.end_ms - latest_ms) as f64 / 1_000.0).round() as i64;
            assert!(!projection.ends_at_reset && seconds > 0);
            let Forecast::Estimated {
                depletes_at_ms,
                duration_seconds,
            } = store.forecast(true, &snapshot, true, settings, now)
            else {
                panic!("expected an estimate");
            };
            assert!(
                (depletes_at_ms - projection.end_ms).abs() <= 1 && duration_seconds == seconds,
                "rough depletion date matches the chart path"
            );
        }
    }

    #[test]
    fn routines_follow_the_wall_clock_across_a_daylight_saving_change() {
        // Clocks went back on Sunday November 1, so the two weekdays behind
        // the morning hours straddle the change: Friday on daylight time,
        // Monday on standard time. A single UTC offset for both would show
        // Friday's first working hour at 07:00.
        let zone = eastern();
        let monday = LocalDate::from_ymd(2026, 11, 2);
        let samples = routine(local_ms(&zone, monday, 12), &zone);
        let profile = routine_profile(&build_hours(&samples), &samples, 0.0, &zone).unwrap();
        let working_rate = profile[8];
        assert!(working_rate > 0.0);
        for (slot, rate) in profile.into_iter().enumerate() {
            let expected = if (8..20).contains(&slot) {
                working_rate
            } else {
                0.0
            };
            assert_eq!(rate, expected, "slot {slot}");
        }
    }
}
