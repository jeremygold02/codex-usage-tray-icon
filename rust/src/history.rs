//! Usage history for one provider: the readings that forecasts, the popup's
//! daily bars and the history chart are built from.
//!
//! Every successful reading is kept for 28 days, up to 45,000 readings, in a
//! file the C# app can read too. Earlier reset cycles stay in the history so
//! routines can be learned across them; [`is_reset`] marks the boundaries.
//! Codex and Claude each get their own store over their own file.
//!
//! Times passed in as `now` are Unix seconds like everywhere else in the
//! crate. Stored timestamps carry an `_ms` suffix and are Unix milliseconds,
//! the precision of the file format.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use crate::forecast::{self, Forecast, ForecastSettings, PatternCache, Projection, Sources};
use crate::history_file::{self, CoverageAnchor, MAX_SAMPLES};
use crate::history_view::{self, ChartSeries, DailyUsageDay};
use crate::localtime::{
    hours_between, now_to_ms, seconds_to_ms, TimeZone, MS_PER_DAY, MS_PER_MINUTE, MS_PER_SECOND,
    TICKS_PER_MS,
};
use crate::model::{LimitWindow, Usage};

/// How long readings are kept, and how far back routines are learned from.
pub const RETENTION_DAYS: i64 = 28;
const RETENTION_MS: i64 = RETENTION_DAYS * MS_PER_DAY;
/// Usage differences up to this many percentage points are measurement jitter.
pub const PERCENT_TOLERANCE: f64 = 0.001;
/// Reset deadlines this close together belong to the same cycle.
const RESET_TOLERANCE_MS: i64 = 2 * MS_PER_MINUTE;
/// Readings further apart than this leave a gap in what was observed.
pub const MAX_CONTINUOUS_HOURS: f64 = 2.0;
/// The completion marker lists at most one day per window per retained day.
const MAX_COVERAGE_ANCHORS: usize = 2 * (RETENTION_DAYS as usize + 1);

/// One limit window as it stood at a reading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HistoryWindow {
    pub used_percent: f64,
    /// When the window resets, in Unix milliseconds, if the provider said.
    pub reset_at_ms: Option<i64>,
    pub window_minutes: Option<u32>,
    /// Set once usage was seen to drop within this cycle, as when a banked
    /// reset is used: usage divided by elapsed time no longer describes it.
    pub cycle_average_unavailable: bool,
    /// True for readings recovered from session logs instead of observed live.
    pub is_imported: bool,
}

impl HistoryWindow {
    fn from_limit(limit: &LimitWindow, updated_at: i64, timestamp_ms: i64, imported: bool) -> Self {
        Self {
            used_percent: limit.used_percent,
            reset_at_ms: reset_after_seconds(limit, updated_at)
                .map(|seconds| timestamp_ms + seconds * MS_PER_SECOND),
            window_minutes: Some(limit.window_minutes),
            cycle_average_unavailable: false,
            is_imported: imported,
        }
    }

    /// True when this window's reset deadline is `reset_at_ms`, give or take clock jitter.
    pub fn resets_near(&self, reset_at_ms: i64) -> bool {
        self.reset_at_ms
            .is_some_and(|reset| reset.abs_diff(reset_at_ms) <= RESET_TOLERANCE_MS.unsigned_abs())
    }
}

/// Seconds from a reading until its window resets. Like the C# client, a
/// deadline already behind the reading counts as due now.
pub fn reset_after_seconds(limit: &LimitWindow, updated_at: i64) -> Option<i64> {
    limit.resets_at.map(|resets_at| {
        resets_at
            .saturating_sub(updated_at)
            .clamp(0, i64::from(i32::MAX))
    })
}

/// One reading: both limit windows at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HistorySample {
    /// Unix milliseconds.
    pub timestamp_ms: i64,
    pub weekly: Option<HistoryWindow>,
    pub five_hour: Option<HistoryWindow>,
}

impl HistorySample {
    /// The weekly window when `weekly` is true, otherwise the five-hour one.
    pub fn window(&self, weekly: bool) -> Option<&HistoryWindow> {
        if weekly {
            self.weekly.as_ref()
        } else {
            self.five_hour.as_ref()
        }
    }

    /// This reading narrowed to one window, if it has that window.
    pub fn narrowed(&self, weekly: bool) -> Option<WindowSample> {
        self.window(weekly).map(|window| WindowSample {
            timestamp_ms: self.timestamp_ms,
            window: *window,
        })
    }

    fn set_window(&mut self, weekly: bool, window: Option<HistoryWindow>) {
        if weekly {
            self.weekly = window;
        } else {
            self.five_hour = window;
        }
    }

    fn has_window(&self) -> bool {
        self.weekly.is_some() || self.five_hour.is_some()
    }
}

/// A reading of a single limit window. Forecasts and the chart work on
/// sequences of these, oldest first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowSample {
    /// Unix milliseconds.
    pub timestamp_ms: i64,
    pub window: HistoryWindow,
}

impl WindowSample {
    /// True when this reading starts a new cycle after `previous`: usage
    /// fell, the earlier deadline has passed, or the deadline moved.
    pub fn is_reset_from(&self, previous: &WindowSample) -> bool {
        let (before, after) = (&previous.window, &self.window);
        after.used_percent < before.used_percent - PERCENT_TOLERANCE
            || before
                .reset_at_ms
                .is_some_and(|reset| self.timestamp_ms >= reset)
            || (before.reset_at_ms.is_some()
                && after.reset_at_ms.is_some()
                && !self.shares_reset_with(previous))
    }

    /// True when this reading follows `previous` within one cycle of one plan.
    pub fn same_cycle_as(&self, previous: &WindowSample) -> bool {
        self.timestamp_ms > previous.timestamp_ms
            && previous.window.window_minutes == self.window.window_minutes
            && !self.is_reset_from(previous)
    }

    /// True when this reading follows `previous` in the same cycle without a
    /// gap in monitoring. The chart draws other same-cycle segments dashed.
    pub fn continues(&self, previous: &WindowSample) -> bool {
        self.same_cycle_as(previous)
            && hours_between(self.timestamp_ms, previous.timestamp_ms) <= MAX_CONTINUOUS_HOURS
    }

    /// True when both readings report the same reset deadline.
    pub fn shares_reset_with(&self, other: &WindowSample) -> bool {
        other
            .window
            .reset_at_ms
            .is_some_and(|reset| self.window.resets_near(reset))
    }
}

/// True when `current` starts a new cycle of the chosen window after
/// `previous`. False when either reading lacks that window.
#[cfg(test)]
pub fn is_reset(previous: &HistorySample, current: &HistorySample, weekly: bool) -> bool {
    match (previous.narrowed(weekly), current.narrowed(weekly)) {
        (Some(before), Some(after)) => after.is_reset_from(&before),
        _ => false,
    }
}

/// True when `current` follows `previous` in the same cycle of the chosen
/// window with no more than two hours between them.
#[cfg(test)]
pub fn is_continuous(previous: &HistorySample, current: &HistorySample, weekly: bool) -> bool {
    match (previous.narrowed(weekly), current.narrowed(weekly)) {
        (Some(before), Some(after)) => after.continues(&before),
        _ => false,
    }
}

/// The readings that have the chosen window, narrowed to it.
pub fn window_samples(samples: &[HistorySample], weekly: bool) -> Vec<WindowSample> {
    samples
        .iter()
        .filter_map(|sample| sample.narrowed(weekly))
        .collect()
}

/// The reading's timestamp in milliseconds, or `None` when the reading cannot
/// be used: no primary window, an impossible percentage or window length, or
/// a timestamp the history file cannot hold.
pub fn valid_timestamp_ms(usage: &Usage) -> Option<i64> {
    let valid = |limit: &Option<LimitWindow>| {
        limit.as_ref().is_none_or(|limit| {
            (0.0..=100.0).contains(&limit.used_percent) && limit.window_minutes > 0
        })
    };
    let has_window = usage.weekly.is_some() || usage.five_hour.is_some();
    if !has_window || !valid(&usage.weekly) || !valid(&usage.five_hour) {
        return None;
    }
    seconds_to_ms(usage.updated_at)
}

fn sample_from(usage: &Usage, timestamp_ms: i64, imported: bool) -> HistorySample {
    let window = |limit: &Option<LimitWindow>| {
        limit
            .as_ref()
            .map(|limit| HistoryWindow::from_limit(limit, usage.updated_at, timestamp_ms, imported))
    };
    HistorySample {
        timestamp_ms,
        weekly: window(&usage.weekly),
        five_hour: window(&usage.five_hour),
    }
}

fn same_reading(left: &HistorySample, right: &HistorySample) -> bool {
    same_window(&left.weekly, &right.weekly) && same_window(&left.five_hour, &right.five_hour)
}

fn same_window(left: &Option<HistoryWindow>, right: &Option<HistoryWindow>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.used_percent == right.used_percent
                && left.window_minutes == right.window_minutes
                && left.is_imported == right.is_imported
                && left.cycle_average_unavailable == right.cycle_average_unavailable
                && match (left.reset_at_ms, right.reset_at_ms) {
                    (None, None) => true,
                    // The C# app derives the deadline from whole seconds
                    // remaining, so it wobbles between polls.
                    (Some(left), Some(right)) => left.abs_diff(right) <= 2 * MS_PER_SECOND as u64,
                    _ => false,
                }
        }
        _ => false,
    }
}

/// The saved usage history of one provider.
pub struct HistoryStore {
    path: PathBuf,
    zone: TimeZone,
    samples: Vec<HistorySample>,
    import_completed: bool,
    patterns: PatternCache,
}

impl HistoryStore {
    /// Loads the history saved at `path`, dropping readings older than the
    /// retention period. A missing or unreadable file gives an empty history;
    /// history is optional and never blocks a usage refresh. `zone` is the
    /// time zone routines and calendar days are worked out in.
    pub fn open(path: impl Into<PathBuf>, zone: TimeZone, now: i64) -> Self {
        let mut store = Self {
            path: path.into(),
            zone,
            samples: Vec::new(),
            import_completed: false,
            patterns: PatternCache::default(),
        };
        let now_ms = now_to_ms(now);
        if store.load(now_ms) {
            store.import_completed = store.imported_coverage_is_intact(now_ms);
        }
        store
    }

    /// Every retained reading, oldest first, including earlier reset cycles.
    #[cfg(test)]
    pub fn samples(&self) -> &[HistorySample] {
        &self.samples
    }

    /// True once the one-time import of older readings has finished and the
    /// days it supplied are still present, so the logs need not be scanned again.
    pub fn import_completed(&self) -> bool {
        self.import_completed
    }

    /// Records a successful reading and saves the history. Call it once for
    /// each reading a refresh returns, not for redisplayed, failed or paused
    /// states. Returns false when the reading is invalid, older than the
    /// retention period, more than a minute ahead of `now`, or not newer than
    /// the latest stored reading.
    pub fn observe(&mut self, usage: &Usage, now: i64) -> bool {
        let Some(timestamp_ms) = valid_timestamp_ms(usage) else {
            return false;
        };
        let now_ms = now_to_ms(now);
        let newest = self.samples.last().map(|sample| sample.timestamp_ms);
        if timestamp_ms < now_ms - RETENTION_MS
            || timestamp_ms > now_ms + MS_PER_MINUTE
            || newest.is_some_and(|newest| timestamp_ms <= newest)
        {
            return false;
        }
        self.samples.push(sample_from(usage, timestamp_ms, false));
        self.prune(now_ms);
        // A failed save keeps the reading in memory; the next one saves both.
        self.save();
        true
    }

    /// Forgets every reading. The import stays marked as completed so cleared
    /// history is not quietly restored from the logs. Returns false when the
    /// empty history could not be saved.
    #[cfg(test)]
    pub fn clear(&mut self) -> bool {
        self.samples.clear();
        self.patterns.invalidate();
        let saved = self.save();
        self.mark_import_completed(saved);
        saved
    }

    /// Merges readings recovered from logs and returns how many windows were
    /// added. Live readings win: an imported reading only fills a timestamp or
    /// window the history lacks, and imported values that contradict their
    /// neighbours are dropped as stale log entries.
    ///
    /// A completed import is not repeated unless `incremental` is set.
    /// `completed` is false while the log scan is unfinished, which leaves the
    /// import eligible to resume.
    pub fn import(
        &mut self,
        readings: &[Usage],
        incremental: bool,
        completed: bool,
        now: i64,
    ) -> usize {
        if self.import_completed && !incremental {
            return 0;
        }
        let now_ms = now_to_ms(now);
        let mut merged: BTreeMap<i64, HistorySample> = self
            .samples
            .iter()
            .map(|sample| (sample.timestamp_ms, *sample))
            .collect();
        let mut added = 0;
        for usage in readings {
            let Some(timestamp_ms) = valid_timestamp_ms(usage) else {
                continue;
            };
            if timestamp_ms < now_ms - RETENTION_MS || timestamp_ms > now_ms {
                continue;
            }
            let imported = sample_from(usage, timestamp_ms, true);
            match merged.entry(timestamp_ms) {
                Entry::Vacant(entry) => {
                    entry.insert(imported);
                    added += 1;
                }
                Entry::Occupied(mut entry) => {
                    let existing = entry.get_mut();
                    if existing.weekly.is_none() && imported.weekly.is_some() {
                        existing.weekly = imported.weekly;
                        added += 1;
                    }
                    if existing.five_hour.is_none() && imported.five_hour.is_some() {
                        existing.five_hour = imported.five_hour;
                        added += 1;
                    }
                }
            }
        }
        if added == 0 && self.import_completed {
            return 0;
        }
        self.samples = merged.into_values().collect();
        self.drop_stale_imports(true);
        self.drop_stale_imports(false);
        self.prune(now_ms);
        let saved = self.save();
        if completed {
            self.mark_import_completed(saved);
        }
        added
    }

    /// The popup's forecast line for one window of the reading on display.
    /// `fresh` is false when the latest refresh failed or refreshing is
    /// paused; such readings, and ones older than 30 minutes, get no estimate.
    /// Use [`Forecast::text`] for the wording.
    pub fn forecast(
        &self,
        weekly: bool,
        usage: &Usage,
        fresh: bool,
        settings: ForecastSettings,
        now: i64,
    ) -> Forecast {
        forecast::forecast(&self.sources(), weekly, usage, fresh, settings, now)
    }

    /// The chart's projected path for one window, from the reading on display
    /// to depletion or the reset. `None` when no estimate can be made; the
    /// arguments are those of [`HistoryStore::forecast`].
    pub fn projection(
        &self,
        weekly: bool,
        usage: &Usage,
        fresh: bool,
        settings: ForecastSettings,
        now: i64,
    ) -> Option<Projection> {
        forecast::projection(&self.sources(), weekly, usage, fresh, settings, now)
    }

    /// Weekly usage observed on each of the last seven local calendar days,
    /// oldest first, for the popup's daily bars. Show them only while the
    /// reading on display has a weekly window.
    pub fn daily_usage(&self, now: i64) -> [DailyUsageDay; 7] {
        history_view::daily_usage(&self.samples, now, &self.zone)
    }

    /// The readings the history chart plots for one window: the current cycle
    /// only, with unchanged polls reduced to the ends of each plateau.
    pub fn chart(&self, weekly: bool, now: i64) -> ChartSeries {
        ChartSeries::new(&self.samples, weekly, now)
    }

    fn sources(&self) -> Sources<'_> {
        Sources {
            samples: &self.samples,
            zone: &self.zone,
            patterns: &self.patterns,
        }
    }

    fn load(&mut self, now_ms: i64) -> bool {
        let Some(loaded) = history_file::read_samples(&self.path) else {
            return false;
        };
        let mut previous = i64::MIN;
        for sample in loaded {
            if sample.timestamp_ms > previous && sample.timestamp_ms <= now_ms + MS_PER_MINUTE {
                previous = sample.timestamp_ms;
                self.samples.push(sample);
            }
        }
        let loaded_count = self.samples.len();
        self.prune(now_ms);
        if self.samples.len() != loaded_count {
            self.save();
        }
        true
    }

    fn save(&self) -> bool {
        history_file::write_samples(&self.path, &self.samples)
    }

    fn completion_path(&self) -> PathBuf {
        history_file::with_suffix(&self.path, ".imported")
    }

    /// A completion flag alone can outlive history pruned by an older app, so
    /// the marker lists one imported reading per day. The import only counts
    /// as complete while each listed day inside the retention period is still
    /// represented by an imported reading.
    fn imported_coverage_is_intact(&self, now_ms: i64) -> bool {
        let Some(coverage) = history_file::read_completion(&self.completion_path()) else {
            return false;
        };
        if coverage.len() > MAX_COVERAGE_ANCHORS {
            return false;
        }
        let now_ticks = history_file::ticks_from_ms(now_ms);
        let earliest_ticks = history_file::ticks_from_ms(now_ms - RETENTION_MS);
        coverage.iter().all(|anchor| {
            if anchor.timestamp_ticks <= 0 || anchor.timestamp_ticks > now_ticks {
                return false;
            }
            // A full history may have shed its oldest imported readings.
            if anchor.timestamp_ticks < earliest_ticks || self.samples.len() == MAX_SAMPLES {
                return true;
            }
            self.samples.iter().any(|sample| {
                history_file::ticks_from_ms(sample.timestamp_ms).abs_diff(anchor.timestamp_ticks)
                    <= TICKS_PER_MS as u64
                    && sample
                        .window(anchor.weekly)
                        .is_some_and(|window| window.is_imported)
            })
        })
    }

    fn mark_import_completed(&mut self, persist: bool) {
        self.import_completed = true;
        if !persist {
            return;
        }
        let mut coverage = Vec::new();
        for weekly in [true, false] {
            let mut days = HashSet::new();
            for sample in &self.samples {
                let imported = sample
                    .window(weekly)
                    .is_some_and(|window| window.is_imported);
                if imported && days.insert(sample.timestamp_ms.div_euclid(MS_PER_DAY)) {
                    coverage.push(CoverageAnchor {
                        timestamp_ticks: history_file::ticks_from_ms(sample.timestamp_ms),
                        weekly,
                    });
                }
            }
        }
        history_file::write_completion(&self.completion_path(), &coverage);
    }

    /// Logs can repeat quota cached from an older response. Such a reading
    /// must not look like a reset or contradict a live reading, so imported
    /// windows that fall below the reading before them, exceed the next live
    /// reading, or step back to an earlier deadline are removed.
    fn drop_stale_imports(&mut self, weekly: bool) {
        let live: Vec<WindowSample> = window_samples(&self.samples, weekly)
            .into_iter()
            .filter(|sample| !sample.window.is_imported)
            .collect();
        let mut next_live = 0;
        let mut previous: Option<WindowSample> = None;
        for sample in &mut self.samples {
            let Some(current) = sample.narrowed(weekly) else {
                continue;
            };
            while live
                .get(next_live)
                .is_some_and(|live| live.timestamp_ms < current.timestamp_ms)
            {
                next_live += 1;
            }
            let used = current.window.used_percent;
            let regressed = previous.as_ref().is_some_and(|previous| {
                let earlier_deadline = matches!(
                    (previous.window.reset_at_ms, current.window.reset_at_ms),
                    (Some(before), Some(after))
                        if after < before.saturating_sub(RESET_TOLERANCE_MS)
                );
                earlier_deadline
                    || (current.shares_reset_with(previous)
                        && used < previous.window.used_percent - PERCENT_TOLERANCE)
            });
            let contradicted = live.get(next_live).is_some_and(|live| {
                current.shares_reset_with(live)
                    && used > live.window.used_percent + PERCENT_TOLERANCE
            });
            if current.window.is_imported && (regressed || contradicted) {
                sample.set_window(weekly, None);
            } else {
                previous = Some(current);
            }
        }
    }

    fn prune(&mut self, now_ms: i64) {
        let earliest = now_ms - RETENTION_MS;
        self.samples
            .retain(|sample| sample.timestamp_ms >= earliest && sample.has_window());
        self.coalesce();
        if self.samples.len() > MAX_SAMPLES {
            let excess = self.samples.len() - MAX_SAMPLES;
            self.samples.drain(..excess);
        }
        self.mark_cycle_adjustments(true);
        self.mark_cycle_adjustments(false);
        self.patterns.invalidate();
    }

    /// Compacts repeated unchanged readings. Once three in a row match, a
    /// further match within the hour replaces the newest instead of extending
    /// the run, so a plateau keeps its first reading, an anchor each hour and
    /// its latest time.
    fn coalesce(&mut self) {
        let mut kept = 0;
        for index in 0..self.samples.len() {
            let sample = self.samples[index];
            let extends_plateau = kept >= 3
                && hours_between(sample.timestamp_ms, self.samples[kept - 2].timestamp_ms) <= 1.0
                && same_reading(&self.samples[kept - 3], &self.samples[kept - 2])
                && same_reading(&self.samples[kept - 2], &self.samples[kept - 1])
                && same_reading(&self.samples[kept - 1], &sample);
            if extends_plateau {
                self.samples[kept - 1] = sample;
            } else {
                self.samples[kept] = sample;
                kept += 1;
            }
        }
        self.samples.truncate(kept);
    }

    /// Carries the "usage dropped within this cycle" mark forward to every
    /// later reading of the same cycle, so it survives a restart.
    fn mark_cycle_adjustments(&mut self, weekly: bool) {
        let mut previous: Option<WindowSample> = None;
        let mut invalid_average = false;
        for sample in &mut self.samples {
            let Some(mut current) = sample.narrowed(weekly) else {
                continue;
            };
            if let Some(previous) = previous.filter(|previous| current.is_reset_from(previous)) {
                invalid_average = current.shares_reset_with(&previous)
                    && current.window.used_percent
                        < previous.window.used_percent - PERCENT_TOLERANCE;
            }
            invalid_average |= current.window.cycle_average_unavailable;
            current.window.cycle_average_unavailable = invalid_average;
            sample.set_window(weekly, Some(current.window));
            previous = Some(current);
        }
    }
}

#[cfg(test)]
pub mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::localtime::LocalDate;
    use crate::model::{FIVE_HOUR_MINUTES, WEEKLY_MINUTES};

    /// 2026-10-09 12:00:17 UTC, a Friday. Off the minute, so an estimate a
    /// millisecond early still formats to the expected minute.
    pub const NOW: i64 = 1_791_547_217;
    pub const MINUTE: i64 = 60;
    pub const HOUR: i64 = 3_600;
    pub const DAY: i64 = 86_400;

    /// A uniquely named directory under the system temp folder, removed on drop.
    pub struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        pub fn new(label: &str) -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos());
            let path = std::env::temp_dir().join(format!(
                "usage-tray-rs-{label}-{}-{nanos}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        pub fn file(&self, name: &str) -> PathBuf {
            self.path.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    pub fn open(path: &Path) -> HistoryStore {
        HistoryStore::open(path, TimeZone::utc(), NOW)
    }

    /// A reading whose weekly and five-hour windows share one percentage and reset.
    pub fn snapshot(timestamp: i64, used: f64, reset: Option<i64>) -> Usage {
        let window = |window_minutes| LimitWindow {
            used_percent: used,
            window_minutes,
            resets_at: reset,
        };
        Usage::from_windows(
            timestamp,
            Some(window(FIVE_HOUR_MINUTES)),
            Some(window(WEEKLY_MINUTES)),
        )
    }

    fn weekly(sample: &HistorySample) -> HistoryWindow {
        sample.weekly.unwrap()
    }

    fn completion_path(path: &Path) -> PathBuf {
        history_file::with_suffix(path, ".imported")
    }

    fn anchor(timestamp: i64) -> CoverageAnchor {
        CoverageAnchor {
            timestamp_ticks: history_file::ticks_from_ms(timestamp * MS_PER_SECOND),
            weekly: true,
        }
    }

    #[test]
    fn readings_persist_and_invalid_ones_are_rejected() {
        let directory = TempDir::new("history");
        let path = directory.file("history.json");
        let mut store = open(&path);
        let reset = NOW + 3 * DAY;
        let reading = snapshot(NOW, 25.0, Some(reset));
        assert!(
            store.observe(&reading, NOW),
            "first valid observation accepted"
        );
        assert!(
            !store.observe(&reading, NOW),
            "duplicate timestamps rejected"
        );
        assert!(
            !store.observe(&snapshot(NOW - MINUTE, 20.0, Some(reset)), NOW),
            "out-of-order sample rejected"
        );
        for used in [f64::NAN, f64::INFINITY, 101.0, -0.5] {
            let mut invalid = snapshot(NOW + 1, 25.0, Some(reset));
            invalid.weekly.as_mut().unwrap().used_percent = used;
            assert!(!store.observe(&invalid, NOW), "{used} percent rejected");
        }
        let mut no_duration = snapshot(NOW + 1, 25.0, Some(reset));
        no_duration.five_hour.as_mut().unwrap().window_minutes = 0;
        assert!(!store.observe(&no_duration, NOW), "empty window rejected");
        assert!(
            !store.observe(&Usage::from_windows(NOW + 1, None, None), NOW),
            "a reading without a primary window is rejected"
        );
        assert!(
            !store.observe(&snapshot(NOW - 29 * DAY, 10.0, Some(NOW)), NOW),
            "expired observation rejected"
        );
        assert!(
            !store.observe(&snapshot(NOW + 5 * MINUTE, 10.0, Some(reset)), NOW),
            "future observation rejected"
        );
        assert!(
            !store.observe(&snapshot(i64::MAX, 10.0, Some(i64::MAX)), NOW),
            "extreme dates are rejected before deriving reset deadlines"
        );
        assert_eq!(store.samples().len(), 1);

        let mut loaded = open(&path);
        assert_eq!(loaded.samples().len(), 1, "history survives restart");
        let sample = loaded.samples()[0];
        assert_eq!(sample.timestamp_ms, NOW * 1_000);
        assert_eq!(weekly(&sample).used_percent, 25.0);
        assert_eq!(weekly(&sample).window_minutes, Some(WEEKLY_MINUTES));
        assert_eq!(
            weekly(&sample).reset_at_ms,
            Some(reset * 1_000),
            "reset deadline round-trips"
        );
        assert_eq!(
            sample.five_hour.unwrap().window_minutes,
            Some(FIVE_HOUR_MINUTES)
        );
        assert!(loaded.clear());
        assert!(
            open(&path).samples().is_empty(),
            "clearing history persists"
        );
    }

    #[test]
    fn readings_are_accepted_up_to_the_retention_and_clock_limits() {
        let directory = TempDir::new("limits");
        let mut store = open(&directory.file("history.json"));
        assert!(!store.observe(&snapshot(NOW - 28 * DAY - 1, 10.0, None), NOW));
        assert!(store.observe(&snapshot(NOW - 28 * DAY, 10.0, None), NOW));
        assert!(store.observe(&snapshot(NOW + MINUTE, 10.0, None), NOW));
        assert!(!store.observe(&snapshot(NOW + MINUTE + 1, 10.0, None), NOW));

        // A day later the oldest reading has aged out, in memory and on disk.
        assert!(store.observe(&snapshot(NOW + DAY, 10.0, None), NOW + DAY));
        assert_eq!(store.samples().len(), 2);
        let reopened =
            HistoryStore::open(directory.file("history.json"), TimeZone::utc(), NOW + DAY);
        assert_eq!(reopened.samples(), store.samples());
        // Loading also drops what expired while the app was closed.
        let later = HistoryStore::open(
            directory.file("history.json"),
            TimeZone::utc(),
            NOW + 28 * DAY + 2 * MINUTE,
        );
        assert_eq!(later.samples().len(), 1);
        assert_eq!(later.samples()[0].timestamp_ms, (NOW + DAY) * 1_000);
    }

    #[test]
    fn a_reset_deadline_behind_the_reading_counts_as_due() {
        let directory = TempDir::new("due");
        let mut store = open(&directory.file("history.json"));
        assert!(store.observe(&snapshot(NOW, 10.0, Some(NOW - HOUR)), NOW));
        assert_eq!(
            weekly(&store.samples()[0]).reset_at_ms,
            Some(NOW * 1_000),
            "the C# client clamps the time until reset at zero"
        );
    }

    #[test]
    fn unchanged_readings_are_compacted_to_endpoints_and_hourly_anchors() {
        let directory = TempDir::new("compaction");
        let path = directory.file("history.json");
        let mut store = open(&path);
        let start = NOW - 3 * HOUR;
        for poll in 0..=36 {
            // The deadline wobbles by a second, as it does in files saved by the C# app.
            let reset = NOW + DAY + poll % 2;
            assert!(store.observe(&snapshot(start + poll * 5 * MINUTE, 40.0, Some(reset)), NOW));
        }
        let minutes: Vec<i64> = store
            .samples()
            .iter()
            .map(|sample| (sample.timestamp_ms / 1_000 - start) / MINUTE)
            .collect();
        assert_eq!(minutes, [0, 5, 65, 125, 180]);
        assert_eq!(open(&path).samples(), store.samples());

        // A change ends the plateau and is never merged away.
        assert!(store.observe(&snapshot(NOW + 1, 41.0, Some(NOW + DAY)), NOW));
        assert!(store.observe(&snapshot(NOW + 2, 42.0, Some(NOW + DAY)), NOW));
        assert_eq!(store.samples().len(), 7);
    }

    #[test]
    fn the_history_never_exceeds_its_sample_cap() {
        let directory = TempDir::new("cap");
        let path = directory.file("history.json");
        let first = NOW - MAX_SAMPLES as i64 * 30;
        let full: Vec<HistorySample> = (0..MAX_SAMPLES as i64)
            .map(|index| HistorySample {
                timestamp_ms: (first + index * 30) * 1_000,
                weekly: Some(HistoryWindow {
                    used_percent: (index % 100) as f64,
                    reset_at_ms: None,
                    window_minutes: Some(WEEKLY_MINUTES),
                    cycle_average_unavailable: false,
                    is_imported: false,
                }),
                five_hour: None,
            })
            .collect();
        assert!(history_file::write_samples(&path, &full));
        let mut store = open(&path);
        assert_eq!(store.samples().len(), MAX_SAMPLES);
        assert!(store.observe(&snapshot(NOW, 50.5, None), NOW));
        assert_eq!(store.samples().len(), MAX_SAMPLES);
        assert_eq!(store.samples()[0].timestamp_ms, full[1].timestamp_ms);
        assert_eq!(open(&path).samples().len(), MAX_SAMPLES);
    }

    #[test]
    fn corrupt_history_starts_empty_and_recovers() {
        let directory = TempDir::new("corrupt");
        let path = directory.file("corrupt.json");
        std::fs::write(&path, "{invalid history").unwrap();
        let mut store = open(&path);
        assert!(store.samples().is_empty(), "corrupt history starts empty");
        assert!(
            store.observe(&snapshot(NOW, 10.0, Some(NOW + DAY)), NOW),
            "new observations recover corrupt history"
        );
        assert_eq!(open(&path).samples().len(), 1, "recovered history persists");
    }

    #[test]
    fn files_saved_by_the_existing_app_load_unchanged() {
        let directory = TempDir::new("legacy");
        let path = directory.file("usage-history.json");
        // Millisecond timestamps and a deadline that wobbles, as the C# app saves them.
        let saved = concat!(
            r#"[{"TimestampUtc":"\/Date(1791547000546)\/","Weekly":{"UsedPercent":21,"#,
            r#""ResetAtUtc":"\/Date(1792101192546)\/","WindowMinutes":10080,"#,
            r#""CycleAverageUnavailable":false,"IsImported":false},"FiveHour":null},"#,
            r#"{"TimestampUtc":"\/Date(1791547060569)\/","Weekly":{"UsedPercent":22,"#,
            r#""ResetAtUtc":"\/Date(1792101193569)\/","WindowMinutes":10080,"#,
            r#""CycleAverageUnavailable":false,"IsImported":false},"FiveHour":null}]"#
        );
        std::fs::write(&path, saved).unwrap();
        let mut store = open(&path);
        assert_eq!(store.samples().len(), 2);
        assert_eq!(store.samples()[1].timestamp_ms, 1_791_547_060_569);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            saved,
            "loading an intact file does not rewrite it"
        );
        assert!(!is_reset(&store.samples()[0], &store.samples()[1], true));

        // The next reading is appended in the same format.
        let mut reading = snapshot(NOW, 23.0, Some(1_792_101_193));
        reading.five_hour = None;
        assert!(store.observe(&reading, NOW));
        let appended = concat!(
            r#",{"TimestampUtc":"\/Date(1791547217000)\/","Weekly":{"UsedPercent":23,"#,
            r#""ResetAtUtc":"\/Date(1792101193000)\/","WindowMinutes":10080,"#,
            r#""CycleAverageUnavailable":false,"IsImported":false},"FiveHour":null}]"#
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("{}{appended}", saved.trim_end_matches(']'))
        );
    }

    #[test]
    fn imported_readings_fill_gaps_without_replacing_live_ones() {
        let directory = TempDir::new("import");
        let path = directory.file("import.json");
        let reset = NOW + 6 * DAY;
        let mut store = open(&path);
        let current = snapshot(NOW, 20.0, Some(reset));
        assert!(store.observe(&current, NOW));
        let mut unrelated_window = snapshot(NOW - 6 * HOUR, 15.0, Some(reset));
        unrelated_window.weekly = None;
        let imported = [
            snapshot(NOW - 12 * HOUR, 10.0, Some(reset)),
            snapshot(NOW - DAY, 0.0, Some(reset)),
            snapshot(NOW, 99.0, Some(reset)),
            unrelated_window,
        ];
        assert_eq!(
            store.import(&imported, false, true, NOW),
            3,
            "earlier observations are merged and duplicate live timestamp is preserved"
        );
        assert_eq!(
            weekly(&store.samples()[3]).used_percent,
            20.0,
            "import cannot replace a live reading"
        );
        assert!(weekly(&store.samples()[0]).is_imported);
        assert!(!weekly(&store.samples()[3]).is_imported);
        assert_eq!(store.samples()[2].weekly, None);

        let mut loaded = open(&path);
        assert!(
            loaded.import_completed() && loaded.samples().len() == 4,
            "import and completion survive restart"
        );
        assert!(loaded.clear());
        let mut cleared = open(&path);
        assert!(
            cleared.import_completed()
                && cleared.import(&imported, false, true, NOW) == 0
                && cleared.samples().is_empty(),
            "cleared history is not silently reimported"
        );
    }

    #[test]
    fn a_failed_save_leaves_the_import_to_be_retried() {
        let directory = TempDir::new("unwritable");
        let path = directory.file("unwritable.json");
        std::fs::create_dir(&path).unwrap();
        let mut store = open(&path);
        store.import(
            &[snapshot(NOW, 20.0, Some(NOW + 6 * DAY))],
            false,
            true,
            NOW,
        );
        assert_eq!(store.samples().len(), 1, "the readings stay in memory");
        assert!(
            !completion_path(&path).exists() && !open(&path).import_completed(),
            "failed history save must allow bootstrap retry after restart"
        );
        let leftovers = std::fs::read_dir(directory.file("")).unwrap().count();
        assert_eq!(
            leftovers, 1,
            "temporary files are removed after a failed save"
        );
    }

    #[test]
    fn earlier_cycles_and_reset_evidence_survive_restart() {
        let directory = TempDir::new("retention");
        let path = directory.file("retention.json");
        let old_reset = NOW - 3 * HOUR;
        let reset = old_reset + 7 * DAY;
        let mut store = open(&path);
        assert!(store.observe(&snapshot(NOW - DAY, 50.0, Some(old_reset)), NOW));
        assert!(store.observe(&snapshot(NOW - 4 * HOUR, 90.0, Some(old_reset)), NOW));
        assert!(store.observe(&snapshot(NOW - 2 * HOUR, 0.0, Some(reset)), NOW));
        assert!(store.observe(&snapshot(NOW - HOUR, 10.0, Some(reset)), NOW));
        assert!(
            store.samples().len() == 4 && store.samples()[0].timestamp_ms == (NOW - DAY) * 1_000,
            "prior and current cycles remain in memory for learning"
        );
        let mut store = open(&path);
        assert!(
            store.samples().len() == 4
                && std::fs::read_to_string(&path)
                    .unwrap()
                    .contains("UsedPercent\":90"),
            "prior cycles survive in the bounded saved history"
        );
        assert!(store.observe(&snapshot(NOW, 10.0, Some(reset)), NOW));
        let mut store = open(&path);
        assert_eq!(
            store.samples().len(),
            5,
            "later idle readings preserve both cycles after restart"
        );

        assert!(store.clear());
        assert!(store.observe(&snapshot(NOW - 2 * HOUR, 90.0, Some(reset)), NOW));
        assert!(store.observe(&snapshot(NOW - 30 * MINUTE, 10.0, Some(reset)), NOW));
        let mut store = open(&path);
        let latest = snapshot(NOW, 15.0, Some(reset));
        assert!(store.observe(&latest, NOW));
        let marks: Vec<bool> = store
            .samples()
            .iter()
            .map(|sample| weekly(sample).cycle_average_unavailable)
            .collect();
        assert_eq!(marks, [false, true, true]);
        assert_eq!(
            store.forecast(true, &latest, true, ForecastSettings::default(), NOW),
            Forecast::Learning,
            "restart preserves banked-reset evidence so a cycle average cannot reuse the old start"
        );

        std::fs::write(completion_path(&path), "1").unwrap();
        let mut store = open(&path);
        assert!(
            !store.import_completed(),
            "older import markers allow prior-cycle backfill after upgrading"
        );
        store.import(&[], false, true, NOW);
        assert!(
            open(&path).import_completed(),
            "updated import version persists"
        );
    }

    #[test]
    fn imported_readings_cannot_invent_resets_or_contradict_live_ones() {
        let directory = TempDir::new("consistency");
        let path = directory.file("import-consistency.json");
        let reset = Some(NOW + 3 * DAY);
        let mut store = open(&path);
        assert!(store.observe(&snapshot(NOW - HOUR, 20.0, reset), NOW));
        assert!(store.observe(&snapshot(NOW, 25.0, reset), NOW));
        let stale = [
            snapshot(NOW - 50 * MINUTE, 19.0, reset),
            snapshot(NOW - 45 * MINUTE, 21.0, reset),
            snapshot(NOW - 40 * MINUTE, 20.0, reset),
            snapshot(NOW - 30 * MINUTE, 99.0, reset),
        ];
        store.import(&stale, false, false, NOW);
        assert!(
            store.samples().len() == 3 && weekly(&store.samples()[1]).used_percent == 21.0,
            "cached lower quota and quota contradicting a live reading do not invent resets"
        );
        assert!(
            !store.import_completed() && !completion_path(&path).exists(),
            "an interrupted import remains eligible to resume"
        );
        store.import(&[], false, true, NOW);
        let mut store = open(&path);
        assert!(
            store.import_completed() && weekly(&store.samples()[1]).is_imported,
            "import completion and provenance survive restart"
        );
        assert_eq!(
            store.import(
                &[snapshot(NOW - 20 * MINUTE, 22.0, reset)],
                false,
                true,
                NOW
            ),
            0,
            "completed backfill is not repeated"
        );

        // An imported reading may not step back to an earlier deadline either.
        let earlier_deadline = snapshot(NOW - 10 * MINUTE, 24.0, Some(NOW + 2 * DAY));
        assert_eq!(store.import(&[earlier_deadline], true, true, NOW), 1);
        assert_eq!(store.samples().len(), 3);

        assert!(store.clear());
        assert!(store.observe(&snapshot(NOW - 21 * DAY, 10.0, Some(NOW - 16 * DAY)), NOW));
        assert!(store.observe(&snapshot(NOW - 7 * DAY, 10.0, Some(NOW - 2 * DAY)), NOW));
        assert!(store.observe(&snapshot(NOW, 10.0, reset), NOW));
        assert_eq!(
            open(&path).samples().len(),
            3,
            "four-week learning retains readings older than two weeks"
        );
    }

    #[test]
    fn missing_imported_days_reopen_the_import() {
        let directory = TempDir::new("coverage");
        let path = directory.file("import-coverage.json");
        let earlier = snapshot(NOW - 10 * DAY, 40.0, Some(NOW - 4 * DAY));
        let current = snapshot(NOW, 20.0, Some(NOW + 3 * DAY));
        let recent_only = |samples: &[HistorySample]| -> Vec<HistorySample> {
            samples
                .iter()
                .filter(|sample| sample.timestamp_ms >= (NOW - 7 * DAY) * 1_000)
                .copied()
                .collect()
        };
        let mut store = open(&path);
        assert!(store.observe(&current, NOW));
        store.import(std::slice::from_ref(&earlier), false, true, NOW);
        assert!(
            open(&path).import_completed(),
            "an intact backfill stays complete across normal launches"
        );

        assert!(history_file::write_samples(
            &path,
            &recent_only(store.samples())
        ));
        let mut store = open(&path);
        assert!(
            !store.import_completed() && store.samples().len() == 1,
            "a legacy app pruning prior cycles invalidates the retained coverage marker"
        );
        assert!(
            store.import(std::slice::from_ref(&earlier), false, true, NOW) == 1
                && open(&path).import_completed(),
            "missing backfill can be repaired once and remains complete after restart"
        );

        let intact = store.samples().to_vec();
        let mut relabelled = intact.clone();
        relabelled[0].weekly.as_mut().unwrap().is_imported = false;
        relabelled[0].five_hour.as_mut().unwrap().is_imported = false;
        assert!(history_file::write_samples(&path, &relabelled));
        assert!(
            !open(&path).import_completed(),
            "a live reading at the same timestamp cannot mask missing imported coverage"
        );
        assert!(history_file::write_samples(&path, &intact));

        std::fs::write(completion_path(&path), "2").unwrap();
        assert!(
            !open(&path).import_completed(),
            "legacy completion flags without coverage allow one repair import"
        );
        history_file::write_completion(&completion_path(&path), &[anchor(NOW - 29 * DAY)]);
        assert!(
            open(&path).import_completed(),
            "normal expiry beyond four weeks does not trigger another log scan"
        );
        history_file::write_completion(
            &completion_path(&path),
            &[anchor(NOW - 29 * DAY), anchor(NOW - 10 * DAY)],
        );
        assert!(open(&path).import_completed());
        assert!(history_file::write_samples(&path, &recent_only(&intact)));
        assert!(
            !open(&path).import_completed(),
            "an expired first anchor cannot mask missing later imported days"
        );
        history_file::write_completion(&completion_path(&path), &[anchor(NOW + 1)]);
        assert!(
            !open(&path).import_completed(),
            "a marker from the future is not trusted"
        );
        std::fs::write(completion_path(&path), "{invalid completion").unwrap();
        assert!(
            !open(&path).import_completed(),
            "an interrupted completion write allows recovery on restart"
        );
        assert!(store.clear());
        let mut store = open(&path);
        assert!(
            store.import_completed()
                && store.samples().is_empty()
                && store.import(&[earlier], false, true, NOW) == 0,
            "an intentional clear retains completion without resurrecting history"
        );
    }

    #[test]
    fn compacted_and_polled_history_keeps_its_imported_coverage() {
        let directory = TempDir::new("polling");
        let path = directory.file("import-polling.json");
        let reset = Some(NOW + 3 * DAY);
        let imported: Vec<Usage> = (0..=288)
            .map(|step| snapshot(NOW - 2 * DAY + step * 5 * MINUTE, 50.0, reset))
            .collect();
        let mut store = open(&path);
        assert_eq!(store.import(&imported, false, true, NOW), imported.len());
        assert!(store.samples().len() < 40, "the plateau is compacted");
        assert!(
            open(&path).import_completed(),
            "compacted imported plateaus retain their daily coverage"
        );
        for step in 0..=24 {
            let time = NOW - 12 * HOUR + step * 30 * MINUTE;
            assert!(store.observe(&snapshot(time, 50.0, reset), NOW));
        }
        assert!(
            open(&path).import_completed(),
            "later unchanged live polls cannot invalidate imported coverage or cause a rescan"
        );
    }

    #[test]
    fn views_are_worked_out_in_the_store_time_zone() {
        // Thirteen hours ahead of UTC it is 01:00 on Saturday October 10.
        let directory = TempDir::new("views");
        let zone = TimeZone::fixed(13 * 60);
        let mut store = HistoryStore::open(directory.file("history.json"), zone, NOW);
        let reset = Some(NOW + 3 * DAY);
        for (minutes, used) in [(-90, 10.0), (-60, 12.0), (-30, 12.0), (0, 15.0)] {
            assert!(store.observe(&snapshot(NOW + minutes * MINUTE, used, reset), NOW));
        }
        assert_eq!(store.zone.to_local(NOW * 1_000).hour(), 1);
        let days = store.daily_usage(NOW);
        let saturday = LocalDate::from_ymd(2026, 10, 10);
        assert_eq!(days[6].date, saturday);
        assert_eq!(days[6].used_percent, Some(3.0));
        assert!((days[6].coverage_hours - 1.0).abs() < 1e-9 && days[6].partial);
        assert!(
            days[..6].iter().all(|day| day.used_percent.is_none()),
            "the first interval crosses local midnight and belongs to neither day"
        );

        let chart = store.chart(true, NOW);
        assert_eq!(chart.samples.len(), 4);
        assert_eq!(chart.start_ms, (NOW - 90 * MINUTE) * 1_000);
        assert_eq!(chart.end_ms(None), (NOW + 3 * DAY) * 1_000);
    }

    #[test]
    fn cycle_boundaries_are_recognized() {
        let directory = TempDir::new("cycles");
        let mut store = open(&directory.file("cycles.json"));
        let reset = NOW + 3 * DAY;
        assert!(store.observe(&snapshot(NOW - DAY, 10.0, Some(reset)), NOW));
        assert!(store.observe(&snapshot(NOW - 12 * HOUR, 30.0, Some(reset)), NOW));
        let samples = store.samples();
        assert!(
            !is_continuous(&samples[0], &samples[1], true),
            "overnight chart gaps remain visible"
        );
        assert!(
            !is_reset(&samples[0], &samples[1], true),
            "a chart gap is not a reset"
        );

        assert!(store.clear());
        assert!(store.observe(&snapshot(NOW - 30 * MINUTE, 90.0, Some(reset)), NOW));
        assert!(store.observe(&snapshot(NOW - 15 * MINUTE, 10.0, Some(reset)), NOW));
        assert!(store.observe(&snapshot(NOW, 20.0, Some(reset)), NOW));
        let samples = store.samples();
        assert!(
            is_reset(&samples[0], &samples[1], false),
            "banked reset detected from a decrease"
        );
        assert!(
            !is_continuous(&samples[0], &samples[1], false),
            "chart does not connect across banked reset"
        );
        assert!(is_continuous(&samples[1], &samples[2], false));
        assert!(
            samples.len() == 3 && samples[1].five_hour.unwrap().used_percent == 10.0,
            "earlier cycles remain available for pattern learning"
        );

        let mut moved = samples[1];
        let window = moved.five_hour.as_mut().unwrap();
        window.used_percent = 95.0;
        window.reset_at_ms = Some((reset + 5 * HOUR) * 1_000);
        assert!(
            is_reset(&samples[0], &moved, false),
            "changed reset deadline detects new cycle even when usage increased"
        );
        let mut passed = samples[1];
        passed.five_hour.as_mut().unwrap().used_percent = 95.0;
        passed.timestamp_ms = reset * 1_000;
        assert!(
            is_reset(&samples[0], &passed, false),
            "a reading at the earlier deadline belongs to the next cycle"
        );
        let mut weekly_only = samples[1];
        weekly_only.five_hour = None;
        assert!(!is_reset(&samples[0], &weekly_only, false));
        assert!(!is_continuous(&weekly_only, &samples[2], false));
    }
}
