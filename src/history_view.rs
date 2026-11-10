//! Read-only views of the usage history: the popup's seven daily bars and
//! the series, time range and axis ticks of the history chart.

use crate::forecast::Projection;
use crate::history::{HistorySample, WindowSample, MAX_CONTINUOUS_HOURS, PERCENT_TOLERANCE};
use crate::localtime::{
    decimal_separator, format_date, hours_between, minutes_between, now_to_ms, LocalDate,
    LocalTime, TimeZone, MAX_TIMESTAMP_MS, MIN_TIMESTAMP_MS, MS_PER_MINUTE, MS_PER_SECOND,
};

/// Axis tick spacings, in minutes, from which the smallest sufficient one is chosen.
const TICK_STEPS_MINUTES: [i64; 13] = [
    1, 5, 15, 30, 60, 180, 360, 720, 1_440, 2_880, 10_080, 20_160, 40_320,
];
/// The chart never shows more tick intervals than this, however wide it is.
const MAX_TICK_INTERVALS: usize = 8;

/// Weekly usage observed during one local calendar day.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DailyUsageDay {
    pub date: LocalDate,
    /// Percentage points used during the day, or `None` when no usable
    /// interval was observed. Can exceed 100 across several reset cycles.
    pub used_percent: Option<f64>,
    /// Hours of the day that the counted intervals cover.
    pub coverage_hours: f64,
    /// True unless the counted intervals cover the whole day so far.
    pub partial: bool,
}

/// Weekly usage on each of the seven local calendar days ending today,
/// oldest first. An interval counts only when it lies within one day and one
/// cycle and lasts at most two hours: across midnight, a reset, a plan
/// change or a longer gap, its usage cannot be assigned to a day reliably.
pub fn daily_usage(samples: &[HistorySample], now: i64, zone: &TimeZone) -> [DailyUsageDay; 7] {
    let now_ms = now_to_ms(now);
    let first_day = zone.to_local(now_ms).date().plus_days(-6);
    let mut days: [DailyUsageDay; 7] = std::array::from_fn(|day| DailyUsageDay {
        date: first_day.plus_days(day as i64),
        used_percent: None,
        coverage_hours: 0.0,
        partial: true,
    });
    let first_ms = zone.to_utc(LocalTime::midnight(first_day));
    let mut used = [0.0; 7];
    for pair in samples.windows(2) {
        let (Some(before), Some(after)) = (pair[0].narrowed(true), pair[1].narrowed(true)) else {
            continue;
        };
        if before.window.window_minutes != after.window.window_minutes
            || after.is_reset_from(&before)
        {
            continue;
        }
        let elapsed = hours_between(after.timestamp_ms, before.timestamp_ms);
        if elapsed <= 0.0
            || elapsed > MAX_CONTINUOUS_HOURS
            || after.timestamp_ms <= first_ms
            || after.timestamp_ms > now_ms
        {
            continue;
        }
        // The timing of an increase across midnight is unknown. Only
        // credit intervals wholly within one local day and before now.
        let date = zone.to_local(before.timestamp_ms).date();
        let same_day = date == zone.to_local(after.timestamp_ms).date();
        let Some(day) = usize::try_from(date.days_since(first_day))
            .ok()
            .filter(|&day| same_day && day < days.len())
        else {
            continue;
        };
        let percentages = [before.window.used_percent, after.window.used_percent];
        if percentages
            .iter()
            .all(|percent| (0.0..=100.0).contains(percent))
        {
            used[day] += (percentages[1] - percentages[0]).max(0.0);
            days[day].coverage_hours += elapsed;
        }
    }
    for (day, used) in days.iter_mut().zip(used) {
        if day.coverage_hours > 0.0 {
            let start_ms = zone.to_utc(LocalTime::midnight(day.date));
            let end_ms = zone
                .to_utc(LocalTime::midnight(day.date.plus_days(1)))
                .min(now_ms);
            day.used_percent = Some(used);
            day.partial = day.coverage_hours + 0.000_001 < hours_between(end_ms, start_ms);
        }
    }
    days
}

/// The line shown beneath the daily bars for the selected day, such as
/// `Fri, Oct 9 · Observed: 12.5% used · Partial coverage`, in the user's locale.
pub fn format_daily_usage_detail(day: &DailyUsageDay) -> String {
    format!(
        "{} \u{b7} {}",
        format_date(day.date, "ddd, MMM d"),
        daily_usage_text(day.used_percent, day.partial, &decimal_separator())
    )
}

fn daily_usage_text(used_percent: Option<f64>, partial: bool, separator: &str) -> String {
    let Some(used) = used_percent else {
        return "Usage unavailable".to_string();
    };
    let value = if used > 0.0 && used < 0.001 {
        "<0.001".to_string()
    } else {
        // Up to three decimals without trailing zeros.
        let thousandths = (used * 1_000.0).round().max(0.0) as u64;
        let decimals = format!("{:03}", thousandths % 1_000);
        match decimals.trim_end_matches('0') {
            "" => (thousandths / 1_000).to_string(),
            decimals => format!("{}{separator}{decimals}", thousandths / 1_000),
        }
    };
    let coverage = if partial {
        " \u{b7} Partial coverage"
    } else {
        ""
    };
    format!("Observed: {value}% used{coverage}")
}

/// The readings the history chart plots for one window.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartSeries {
    pub weekly: bool,
    /// The current cycle's readings up to now, oldest first: everything from
    /// the first reading after the latest reset. Runs of unchanged readings
    /// keep only their first and last, so a plateau spans its real duration
    /// while only percentage changes remain selectable.
    pub samples: Vec<WindowSample>,
    /// The left edge of the time axis: the first plotted reading, or now when
    /// there is none.
    pub start_ms: i64,
    /// The time the series was built for, in Unix milliseconds.
    pub now_ms: i64,
}

impl ChartSeries {
    /// Selects what the chart shows of `samples` for one window at `now`.
    pub fn new(samples: &[HistorySample], weekly: bool, now: i64) -> Self {
        let now_ms = now_to_ms(now);
        let mut cycle: Vec<WindowSample> = samples
            .iter()
            .filter(|sample| sample.timestamp_ms <= now_ms)
            .filter_map(|sample| sample.narrowed(weekly))
            .collect();
        cycle.sort_by_key(|sample| sample.timestamp_ms);
        let latest_reset = (1..cycle.len())
            .rev()
            .find(|&index| cycle[index].is_reset_from(&cycle[index - 1]));
        if let Some(index) = latest_reset {
            cycle.drain(..index);
        }

        // Working backwards, a reading goes when neither it nor the reading
        // kept after it differs from its predecessor.
        let mut kept: Vec<WindowSample> = Vec::with_capacity(cycle.len());
        for index in (0..cycle.len()).rev() {
            let interior_of_plateau = index > 0
                && !is_changed(&cycle[index - 1], &cycle[index])
                && kept
                    .last()
                    .is_some_and(|next| !is_changed(&cycle[index], next));
            if !interior_of_plateau {
                kept.push(cycle[index]);
            }
        }
        kept.reverse();

        Self {
            weekly,
            start_ms: kept.first().map_or(now_ms, |first| first.timestamp_ms),
            samples: kept,
            now_ms,
        }
    }

    /// True for a reading whose percentage differs from the one before it,
    /// for the first reading, and for one that starts a new cycle. Only these
    /// can be selected with the pointer or the arrow keys.
    pub fn is_changed_reading(&self, index: usize) -> bool {
        match (
            index.checked_sub(1).and_then(|i| self.samples.get(i)),
            self.samples.get(index),
        ) {
            (Some(before), Some(after)) => is_changed(before, after),
            (None, Some(_)) => true,
            _ => false,
        }
    }

    /// True when a projection continues this series: it starts at the latest
    /// plotted reading and extends past it. Others are not drawn.
    pub fn accepts(&self, projection: &Projection) -> bool {
        self.samples.last().is_some_and(|latest| {
            latest.timestamp_ms.abs_diff(projection.start_ms) <= MS_PER_SECOND as u64
                && (100.0 - latest.window.used_percent - projection.start_remaining_percent).abs()
                    < PERCENT_TOLERANCE
                && projection.end_ms > projection.start_ms
        })
    }

    /// The right edge of the time axis: now, extended to the latest
    /// reading's reset deadline and to the end of an accepted projection.
    pub fn end_ms(&self, projection: Option<&Projection>) -> i64 {
        let reset_ms = self
            .samples
            .last()
            .and_then(|latest| latest.window.reset_at_ms);
        let projected_ms = projection
            .filter(|projection| self.accepts(projection))
            .map(|projection| projection.end_ms);
        let end_ms = [reset_ms, projected_ms]
            .into_iter()
            .flatten()
            .fold(self.now_ms, i64::max);
        if end_ms <= self.start_ms {
            self.start_ms.saturating_add(MS_PER_MINUTE)
        } else {
            end_ms
        }
    }
}

fn is_changed(before: &WindowSample, after: &WindowSample) -> bool {
    (after.window.used_percent - before.window.used_percent).abs() > PERCENT_TOLERANCE
        || after.is_reset_from(before)
}

/// Tick times for a chart's time axis from `start_ms` to `end_ms`: both ends,
/// and between them round local times such as whole hours or midnights.
/// `max_intervals` is how many tick intervals fit the plot's width.
pub fn axis_ticks(start_ms: i64, end_ms: i64, max_intervals: usize, zone: &TimeZone) -> Vec<i64> {
    let intervals = max_intervals.clamp(1, MAX_TICK_INTERVALS);
    let target_minutes = minutes_between(end_ms, start_ms) / intervals as f64;
    let step_minutes = TICK_STEPS_MINUTES
        .into_iter()
        .find(|&step| step as f64 >= target_minutes)
        .unwrap_or(TICK_STEPS_MINUTES[TICK_STEPS_MINUTES.len() - 1]);
    let step_ms = step_minutes * MS_PER_MINUTE;

    // Multi-day steps are aligned to the start of the calendar the C# app
    // counts from, so both apps put their ticks on the same days.
    let calendar_start_ms = MIN_TIMESTAMP_MS;
    let local_ms = zone
        .to_local(start_ms)
        .wall_ms()
        .saturating_sub(calendar_start_ms);
    let mut next_ms = local_ms - local_ms.rem_euclid(step_ms);
    let mut ticks = vec![start_ms];
    while next_ms <= MAX_TIMESTAMP_MS - calendar_start_ms - step_ms {
        next_ms += step_ms;
        let local = LocalTime::from_wall_ms(next_ms + calendar_start_ms);
        if zone.is_invalid(local) {
            continue;
        }
        let tick_ms = zone.to_utc(local);
        if tick_ms >= end_ms {
            break;
        }
        if tick_ms > start_ms {
            ticks.push(tick_ms);
        }
    }
    ticks.push(end_ms);
    ticks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forecast::ProjectionPoint;
    use crate::history::tests::{DAY, HOUR, MINUTE, NOW};
    use crate::history::HistoryWindow;
    use crate::localtime::tests::{eastern, utc_ms};
    use crate::localtime::{MS_PER_DAY, MS_PER_HOUR};
    use crate::model::{FIVE_HOUR_MINUTES, WEEKLY_MINUTES};

    fn window(used_percent: f64, reset_at_ms: Option<i64>, minutes: u32) -> HistoryWindow {
        HistoryWindow {
            used_percent,
            reset_at_ms,
            window_minutes: Some(minutes),
            cycle_average_unavailable: false,
            is_imported: false,
        }
    }

    fn weekly_reading(timestamp_ms: i64, used: f64, reset_at_ms: Option<i64>) -> HistorySample {
        HistorySample {
            timestamp_ms,
            weekly: Some(window(used, reset_at_ms, WEEKLY_MINUTES)),
            five_hour: None,
        }
    }

    /// A weekly reading at a time in Unix seconds, without a reset deadline.
    fn sample(timestamp: i64, used: f64) -> HistorySample {
        weekly_reading(timestamp * 1_000, used, None)
    }

    fn with_reset(mut sample: HistorySample, reset: i64) -> HistorySample {
        sample.weekly.as_mut().unwrap().reset_at_ms = Some(reset * 1_000);
        sample
    }

    fn local_ms(zone: &TimeZone, date: LocalDate, hour: u32, minute: u32) -> i64 {
        zone.to_utc(LocalTime::new(date, hour, minute, 0))
    }

    fn projection(start: i64, end: i64, remaining: (f64, f64)) -> Projection {
        Projection {
            start_ms: start * 1_000,
            end_ms: end * 1_000,
            start_remaining_percent: remaining.0,
            end_remaining_percent: remaining.1,
            ends_at_reset: false,
            uses_daily_pattern: false,
            uses_validated_model: false,
            uses_session_pace: false,
            uses_inactive_hours: false,
            points: Vec::new(),
        }
    }

    fn close(actual: Option<f64>, expected: f64) -> bool {
        actual.is_some_and(|actual| (actual - expected).abs() < 0.001)
    }

    #[test]
    fn daily_usage_sums_intervals_within_each_local_day() {
        for zone in [TimeZone::utc(), eastern(), TimeZone::fixed(330)] {
            let today = LocalDate::from_ymd(2026, 10, 9);
            let yesterday = today.plus_days(-1);
            let previous_day = today.plus_days(-2);
            let now_ms = local_ms(&zone, today, 16, 0);
            let now = now_ms / 1_000;
            let reset = Some(now_ms + 2 * MS_PER_DAY);
            let next_reset = Some(now_ms + 9 * MS_PER_DAY);
            let at = |date, hour, minute, used, reset| {
                weekly_reading(local_ms(&zone, date, hour, minute), used, reset)
            };
            let readings = [
                at(previous_day, 8, 0, 10.0, reset),
                at(previous_day, 9, 0, 10.0, reset),
                at(yesterday, 9, 0, 20.0, reset),
                at(yesterday, 10, 0, 30.0, reset),
                // A three-hour gap is unknown.
                at(yesterday, 13, 0, 30.0, reset),
                at(yesterday, 14, 0, 40.0, reset),
                // A reset is not negative usage.
                at(yesterday, 15, 0, 5.0, next_reset),
                at(yesterday, 16, 0, 15.0, next_reset),
                at(today, 10, 0, 15.0, next_reset),
                // Only half an hour of coverage.
                at(today, 10, 30, 16.0, next_reset),
            ];
            let days = daily_usage(&readings, now, &zone);
            assert!(
                close(days[4].used_percent, 0.0),
                "a sufficiently observed inactive day reports zero usage"
            );
            assert!(
                close(days[5].used_percent, 30.0),
                "daily usage sums valid increases while excluding long gaps and reset boundaries"
            );
            assert!(
                close(days[6].used_percent, 1.0)
                    && (days[6].coverage_hours - 0.5).abs() < 0.001
                    && days[3].used_percent.is_none(),
                "partially observed usage retains its coverage while missing days remain unavailable"
            );
            assert!(days.iter().all(|day| day.partial));
            assert_eq!(days[6].date, today);

            let midnight = [
                at(previous_day, 22, 0, 10.0, reset),
                at(previous_day, 23, 0, 15.0, reset),
                // Usage across midnight cannot be assigned reliably.
                at(yesterday, 0, 0, 35.0, reset),
                at(yesterday, 1, 0, 38.0, reset),
            ];
            let split = daily_usage(&midnight, now, &zone);
            assert!(
                close(split[4].used_percent, 5.0) && close(split[5].used_percent, 3.0),
                "intervals crossing midnight are excluded rather than assigned to either day"
            );

            let multiple_cycles = [
                at(yesterday, 8, 0, 0.0, reset),
                at(yesterday, 9, 0, 60.0, reset),
                at(yesterday, 10, 0, 0.0, next_reset),
                at(yesterday, 11, 0, 70.0, next_reset),
            ];
            let cycles = daily_usage(&multiple_cycles, now, &zone);
            assert!(
                close(cycles[5].used_percent, 130.0) && cycles[5].date == yesterday,
                "multiple observed reset cycles can exceed 100 percent used in the correct local day"
            );

            // Readings after now, on another plan, or of the other window do not count.
            let mut other_plan = at(today, 13, 0, 21.0, next_reset);
            other_plan.weekly.as_mut().unwrap().window_minutes = Some(1_440);
            let mut five_hour_only = at(today, 12, 30, 0.0, None);
            five_hour_only.five_hour = five_hour_only.weekly.take();
            let excluded = [
                at(today, 12, 0, 16.0, next_reset),
                five_hour_only,
                other_plan,
                at(today, 14, 0, 30.0, next_reset),
                at(today, 15, 30, 31.0, next_reset),
                at(today, 16, 30, 40.0, next_reset),
            ];
            let ignored = daily_usage(&excluded, now, &zone);
            assert!(close(ignored[6].used_percent, 1.0));
            assert!((ignored[6].coverage_hours - 1.5).abs() < 0.001);

            let empty = daily_usage(&[], now, &zone);
            for (day, summary) in empty.iter().enumerate() {
                assert!(
                    summary.used_percent.is_none()
                        && summary.date == today.plus_days(day as i64 - 6),
                    "empty history has seven dated, unavailable days"
                );
            }
        }
    }

    #[test]
    fn daily_coverage_follows_the_length_of_the_local_day() {
        // Clocks go back on November 1, 2026, which makes that day 25 hours long.
        let zone = eastern();
        let long_day = LocalDate::from_ymd(2026, 11, 1);
        let start_ms = local_ms(&zone, long_day, 0, 0);
        let end_ms = local_ms(&zone, long_day.plus_days(1), 0, 0);
        assert_eq!(end_ms - start_ms, 25 * MS_PER_HOUR);
        let now = local_ms(&zone, long_day.plus_days(1), 12, 0) / 1_000;
        let mut readings: Vec<HistorySample> = (0..25)
            .map(|hour| weekly_reading(start_ms + hour * MS_PER_HOUR, hour as f64, None))
            .collect();
        readings.push(weekly_reading(end_ms - 1, 25.0, None));

        let days = daily_usage(&readings, now, &zone);
        assert_eq!(days[5].date, long_day);
        assert!(close(days[5].used_percent, 25.0));
        assert!((days[5].coverage_hours - 25.0).abs() < 0.001);
        assert!(!days[5].partial, "the whole 25-hour day was observed");

        // Missing the first hour leaves 24 hours of a 25-hour day.
        let days = daily_usage(&readings[1..], now, &zone);
        assert!(close(days[5].used_percent, 24.0) && days[5].partial);

        // Today is complete so far when coverage reaches the current time.
        let today_ms = local_ms(&zone, long_day.plus_days(1), 0, 0);
        let today: Vec<HistorySample> = (0..=12)
            .map(|hour| weekly_reading(today_ms + hour * MS_PER_HOUR, hour as f64, None))
            .collect();
        let days = daily_usage(&today, now, &zone);
        assert!(close(days[6].used_percent, 12.0) && !days[6].partial);
    }

    #[test]
    fn daily_details_read_like_the_existing_popup() {
        assert_eq!(daily_usage_text(None, true, "."), "Usage unavailable");
        assert_eq!(daily_usage_text(Some(0.0), false, "."), "Observed: 0% used");
        assert_eq!(
            daily_usage_text(Some(12.5), true, "."),
            "Observed: 12.5% used \u{b7} Partial coverage"
        );
        assert_eq!(
            daily_usage_text(Some(12.5), false, ","),
            "Observed: 12,5% used"
        );
        assert_eq!(
            daily_usage_text(Some(1.0 / 3.0), false, "."),
            "Observed: 0.333% used"
        );
        assert_eq!(
            daily_usage_text(Some(2.0005), false, "."),
            "Observed: 2.001% used"
        );
        assert_eq!(
            daily_usage_text(Some(0.0004), false, "."),
            "Observed: <0.001% used"
        );
        assert_eq!(
            daily_usage_text(Some(130.0), false, "."),
            "Observed: 130% used"
        );
        assert_eq!(
            daily_usage_text(Some(5.04), false, "."),
            "Observed: 5.04% used"
        );

        let day = DailyUsageDay {
            date: LocalDate::from_ymd(2026, 10, 9),
            used_percent: Some(4.0),
            coverage_hours: 2.0,
            partial: true,
        };
        assert_eq!(
            format_daily_usage_detail(&day),
            format!(
                "{} \u{b7} Observed: 4% used \u{b7} Partial coverage",
                format_date(day.date, "ddd, MMM d")
            )
        );
        let unavailable = DailyUsageDay {
            used_percent: None,
            ..day
        };
        assert!(format_daily_usage_detail(&unavailable).ends_with(" \u{b7} Usage unavailable"));
    }

    #[test]
    fn the_chart_starts_at_the_first_reading_of_the_current_cycle() {
        let mut samples = vec![
            sample(NOW - 2 * DAY, 40.0),
            sample(NOW - 8 * DAY, 10.0),
            HistorySample {
                timestamp_ms: (NOW - 10 * DAY) * 1_000,
                weekly: None,
                five_hour: Some(window(20.0, None, FIVE_HOUR_MINUTES)),
            },
            sample(NOW + MINUTE, 50.0),
        ];
        let chart = ChartSeries::new(&samples, true, NOW);
        assert_eq!(
            chart.start_ms,
            (NOW - 8 * DAY) * 1_000,
            "axis starts at the earliest weekly reading, even before seven days"
        );
        assert_eq!(
            chart.end_ms(None),
            NOW * 1_000,
            "axis ends at the current time"
        );
        assert_eq!(chart.samples.len(), 2, "readings after now are left out");
        assert!(!chart.samples[1].continues(&chart.samples[0]));

        samples[0].weekly.as_mut().unwrap().used_percent = 0.0;
        let chart = ChartSeries::new(&samples, true, NOW);
        assert!(
            chart.start_ms == samples[0].timestamp_ms && chart.samples.len() == 1,
            "weekly view starts at the first reading after the latest usage reset"
        );
        assert_eq!(
            ChartSeries::new(&samples, false, NOW).start_ms,
            (NOW - 10 * DAY) * 1_000,
            "each usage window uses its own first reading"
        );
        let single = ChartSeries::new(&[sample(NOW, 20.0)], true, NOW);
        assert!(
            single.start_ms == NOW * 1_000 && single.end_ms(None) > NOW * 1_000,
            "single current reading has a nonzero range without earlier empty time"
        );
        let empty = ChartSeries::new(&[], true, NOW);
        assert!(
            empty.samples.is_empty() && empty.end_ms(None) > empty.start_ms,
            "empty chart has a safe range"
        );
        assert!(!empty.is_changed_reading(0));
    }

    #[test]
    fn earlier_cycles_are_left_off_the_chart() {
        let mut samples = vec![
            sample(NOW - 8 * DAY, 10.0),
            sample(NOW - 7 * DAY, 80.0),
            sample(NOW - 6 * DAY, 0.0),
            sample(NOW - 5 * DAY, 30.0),
        ];
        let chart = ChartSeries::new(&samples, true, NOW);
        assert!(
            chart.start_ms == samples[2].timestamp_ms && chart.samples.len() == 2,
            "previous weekly cycles are excluded after sorting the readings"
        );

        samples.push(sample(NOW - 2 * HOUR, 0.0));
        samples.push(sample(NOW - HOUR, 20.0));
        let chart = ChartSeries::new(&samples, true, NOW);
        assert!(
            chart.start_ms == samples[4].timestamp_ms && chart.samples.len() == 2,
            "another reset advances the chart"
        );

        let old_reset = NOW - 2 * HOUR;
        let samples = [
            with_reset(sample(NOW - 4 * HOUR, 20.0), old_reset),
            with_reset(sample(NOW - 3 * HOUR, 40.0), old_reset),
            with_reset(sample(NOW - HOUR, 45.0), old_reset + 7 * DAY),
            with_reset(sample(NOW, 50.0), old_reset + 7 * DAY),
        ];
        assert_eq!(
            ChartSeries::new(&samples, true, NOW).start_ms,
            samples[2].timestamp_ms,
            "a new reset deadline starts a new cycle even if the percentage did not decrease"
        );

        let samples = [
            with_reset(sample(NOW - 4 * HOUR, 20.0), NOW + DAY),
            with_reset(sample(NOW - 3 * HOUR, 40.0), NOW + DAY + 30),
            with_reset(sample(NOW - HOUR, 45.0), NOW + DAY + 45),
            with_reset(sample(NOW, 50.0), NOW + DAY + 60),
        ];
        assert_eq!(
            ChartSeries::new(&samples, true, NOW).start_ms,
            samples[0].timestamp_ms,
            "small reset deadline variations do not crop valid readings"
        );
    }

    #[test]
    fn unchanged_polls_keep_only_the_ends_of_a_plateau() {
        let samples = [
            sample(NOW - 2 * HOUR, 15.0),
            sample(NOW - HOUR, 19.0),
            sample(NOW - 45 * MINUTE, 19.0),
            sample(NOW - 30 * MINUTE, 19.0),
            sample(NOW, 19.0),
        ];
        let chart = ChartSeries::new(&samples, true, NOW);
        let times: Vec<i64> = chart
            .samples
            .iter()
            .map(|sample| sample.timestamp_ms / 1_000)
            .collect();
        assert_eq!(
            times,
            [NOW - 2 * HOUR, NOW - HOUR, NOW],
            "repeated polls are coalesced while the first and latest plateau times remain available"
        );
        let changed: Vec<bool> = (0..4)
            .map(|index| chart.is_changed_reading(index))
            .collect();
        assert_eq!(
            changed,
            [true, true, false, false],
            "inspection selects the percentage change rather than repeated unchanged polls"
        );

        // A plateau of two readings is already down to its ends.
        let pair = [sample(NOW - HOUR, 19.0), sample(NOW, 19.0)];
        assert_eq!(ChartSeries::new(&pair, true, NOW).samples.len(), 2);
    }

    #[test]
    fn a_projection_must_continue_the_latest_reading() {
        let samples = [sample(NOW - 4 * HOUR, 20.0), sample(NOW - HOUR, 40.0)];
        let chart = ChartSeries::new(&samples, true, NOW);
        let mut projected = projection(NOW - HOUR, NOW + 2 * HOUR, (60.0, 10.0));
        assert!(chart.accepts(&projected));
        assert_eq!(
            chart.end_ms(Some(&projected)),
            projected.end_ms,
            "time axis includes the forecast endpoint"
        );
        projected.start_ms = NOW * 1_000;
        assert!(!chart.accepts(&projected));
        assert_eq!(
            chart.end_ms(Some(&projected)),
            NOW * 1_000,
            "projection unrelated to latest recorded point is hidden"
        );
        let wrong_level = projection(NOW - HOUR, NOW + 2 * HOUR, (61.0, 10.0));
        assert!(!chart.accepts(&wrong_level));
        let no_length = projection(NOW - HOUR, NOW - HOUR, (60.0, 60.0));
        assert!(!chart.accepts(&no_length));
        assert!(!ChartSeries::new(&[], true, NOW).accepts(&projected));
    }

    #[test]
    fn the_axis_runs_through_a_known_reset() {
        let reset = utc_ms(2026, 10, 9, 21, 0) / 1_000;
        for weekly in [true, false] {
            for curved in [false, true] {
                let (start, observed, depleted) = if weekly {
                    (reset - 7 * DAY + 10 * MINUTE, reset - 3 * DAY, reset - DAY)
                } else {
                    (
                        reset - 5 * HOUR + 10 * MINUTE,
                        reset - 3 * HOUR,
                        reset - HOUR,
                    )
                };
                let reading = |timestamp: i64, used: f64| {
                    let minutes = if weekly {
                        WEEKLY_MINUTES
                    } else {
                        FIVE_HOUR_MINUTES
                    };
                    let window = Some(window(used, Some(reset * 1_000), minutes));
                    HistorySample {
                        timestamp_ms: timestamp * 1_000,
                        weekly: window.filter(|_| weekly),
                        five_hour: window.filter(|_| !weekly),
                    }
                };
                let samples = [reading(start, 0.0), reading(observed, 60.0)];
                let chart = ChartSeries::new(&samples, weekly, observed);
                assert!(
                    chart.start_ms == start * 1_000 && chart.end_ms(None) == reset * 1_000,
                    "known reset sets the full chart range even before a forecast is available"
                );
                let mut projected = projection(observed, depleted, (40.0, 0.0));
                if curved {
                    projected.points = vec![
                        ProjectionPoint {
                            timestamp_ms: observed * 1_000,
                            remaining_percent: 40.0,
                        },
                        ProjectionPoint {
                            timestamp_ms: depleted * 1_000,
                            remaining_percent: 0.0,
                        },
                    ];
                }
                assert!(chart.accepts(&projected));
                assert_eq!(
                    chart.end_ms(Some(&projected)),
                    reset * 1_000,
                    "early depletion extends the display through reset without changing the forecast endpoint"
                );
                let halfway = (observed + depleted) / 2 * 1_000;
                assert_eq!(projected.remaining_at(halfway), 20.0);
                let after_depletion = (depleted + reset) / 2 * 1_000;
                assert_eq!(
                    projected.remaining_at(after_depletion),
                    0.0,
                    "hover after depletion reports zero remaining at the hovered time"
                );
                assert_eq!(
                    ChartSeries::new(&samples, weekly, reset + MINUTE).end_ms(None),
                    (reset + MINUTE) * 1_000,
                    "expired deadlines do not move the time axis backwards"
                );
            }
        }
    }

    #[test]
    fn axis_ticks_fall_on_round_local_times() {
        for zone in [eastern(), TimeZone::fixed(330)] {
            let start = local_ms(&zone, LocalDate::from_ymd(2026, 10, 2), 17, 13) + 23_000;
            let reset = local_ms(&zone, LocalDate::from_ymd(2026, 10, 9), 17, 0);
            for intervals in [3, 5, 8, 10] {
                let ticks = axis_ticks(start, reset, intervals, &zone);
                assert!(
                    ticks[0] == start && ticks[ticks.len() - 1] == reset,
                    "axis always labels the actual first reading and exact reset time"
                );
                assert!(
                    (3..=10).contains(&ticks.len()),
                    "tick density adapts to chart width"
                );
                assert!(ticks.windows(2).all(|pair| pair[1] > pair[0]));
                for &tick in &ticks[1..ticks.len() - 1] {
                    let local = zone.to_local(tick);
                    assert_eq!(
                        (local.hour(), local.minute(), local.second()),
                        (0, 0, 0),
                        "weekly interior ticks are local calendar boundaries"
                    );
                }
            }
            // Week-long steps land on the Mondays both apps count weeks from.
            let monday = local_ms(&zone, LocalDate::from_ymd(2026, 10, 5), 0, 0);
            assert_eq!(axis_ticks(start, reset, 3, &zone), [start, monday, reset]);
            assert_eq!(axis_ticks(start, reset, 5, &zone).len(), 5);
            assert_eq!(axis_ticks(start, reset, 8, &zone).len(), 9);

            let start = reset - 5 * MS_PER_HOUR + 13 * 60_000 + 23_000;
            let ticks = axis_ticks(start, reset, 5, &zone);
            assert!(
                ticks.len() == 6 && ticks[5] == reset,
                "five-hour view includes its reset label"
            );
            for &tick in &ticks[1..5] {
                let local = zone.to_local(tick);
                assert_eq!(
                    (local.minute(), local.second()),
                    (0, 0),
                    "five-hour interior ticks use whole local hours"
                );
            }
        }
    }

    #[test]
    fn axis_ticks_skip_times_lost_to_daylight_saving() {
        // On March 8, 2026 Eastern clocks jump from 02:00 to 03:00.
        let zone = eastern();
        let day = LocalDate::from_ymd(2026, 3, 8);
        let start = local_ms(&zone, day, 0, 30);
        let end = local_ms(&zone, day, 5, 30);
        assert_eq!(end - start, 4 * MS_PER_HOUR);
        assert_eq!(
            axis_ticks(start, end, 4, &zone),
            [
                start,
                utc_ms(2026, 3, 8, 6, 0),
                utc_ms(2026, 3, 8, 7, 0),
                utc_ms(2026, 3, 8, 8, 0),
                utc_ms(2026, 3, 8, 9, 0),
                end,
            ]
        );
    }
}
