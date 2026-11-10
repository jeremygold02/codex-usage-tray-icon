//! The one-time import of past quota readings from local Codex session logs.
//!
//! Codex records the account's rate limits in the session logs it keeps in
//! `sessions` and `archived_sessions`. After the first successful refresh
//! those logs are read once, up to four weeks back, so that forecasts have a
//! history to learn from on the first day. Earlier reset cycles are imported
//! as well.
//!
//! The work is split between two threads. Reading logs is slow, so [`scan`]
//! runs on a worker and deals only in plain values. The history stays on the
//! UI thread, where a [`Session`] decides when an attempt is due and hands
//! each outcome to the [`HistoryStore`].
//!
//! One attempt reads the most recent logs first and stops after 30 seconds
//! or 8 GiB. An unfinished import resumes on a later refresh, no sooner than
//! five minutes after the previous attempt began, and reads only what was
//! left unread or has been appended since. Completion belongs to the store:
//! once it reports the import as completed no log is opened again, and it
//! reopens the import itself when imported days it expects have gone missing.
//!
//! The logs are never modified.

mod event;
mod scan;

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::history::{HistoryStore, RETENTION_DAYS};
use crate::model::Usage;

#[cfg(test)]
use scan::Stopwatch;
pub use scan::{scan, ScanBudget, ScanOutcome, ScanRequest, ScanState, SystemStopwatch};

/// An unfinished import waits this long after an attempt began before the
/// next one, however often usage is refreshed in between.
pub const RETRY_SECONDS: i64 = 300;
const SECONDS_PER_DAY: i64 = 86_400;

/// Where Codex keeps its files: `CODEX_HOME` when that is set, otherwise
/// `.codex` in the user's profile. Empty, and so never scanned, when neither
/// is known.
pub fn default_codex_home() -> PathBuf {
    codex_home_from(
        std::env::var_os("CODEX_HOME"),
        std::env::var_os("USERPROFILE"),
    )
}

fn codex_home_from(configured: Option<OsString>, profile: Option<OsString>) -> PathBuf {
    let stated = |value: &OsString| !value.to_string_lossy().trim().is_empty();
    configured
        .filter(stated)
        .map(PathBuf::from)
        .or_else(|| {
            profile
                .filter(stated)
                .map(|profile| PathBuf::from(profile).join(".codex"))
        })
        .unwrap_or_default()
}

/// Import state for one run of the tray: whether a scan is under way, when
/// the next attempt may start and where the last one stopped. It never
/// starts work, opens a log or reads the clock itself.
#[derive(Debug)]
pub struct Session {
    codex_home: PathBuf,
    budget: ScanBudget,
    /// Where earlier attempts stopped. `None` while a scan has it.
    state: Option<ScanState>,
    /// The earliest time of the next attempt, in Unix seconds.
    next_attempt: Option<i64>,
    /// Shared with every request, to end a running scan at shutdown.
    cancel: Arc<AtomicBool>,
}

impl Session {
    /// The state at the start of a run, for the logs under `codex_home`.
    /// Nothing is remembered from earlier runs: after a restart an unfinished
    /// import reads the logs from the start, and the history drops the
    /// readings it already holds.
    pub fn new(codex_home: PathBuf) -> Self {
        Self {
            codex_home,
            budget: ScanBudget::default(),
            state: Some(ScanState::default()),
            next_attempt: None,
            cancel: Arc::default(),
        }
    }

    /// Asks whether a scan should start now. Call it after every successful
    /// Codex refresh, once `history` has observed `current`; `now` is in Unix
    /// seconds. With a request, run [`scan`] on a worker thread and pass its
    /// outcome to [`Session::scan_finished`].
    ///
    /// `None` when the import is completed, a scan is still running, or the
    /// previous attempt began less than [`RETRY_SECONDS`] ago.
    pub fn request_scan(
        &mut self,
        history: &HistoryStore,
        current: &Usage,
        now: i64,
    ) -> Option<ScanRequest> {
        let waiting = self.next_attempt.is_some_and(|next| now < next);
        if history.import_completed() || waiting || self.cancelled() {
            return None;
        }
        let state = self.state.take()?;
        self.next_attempt = Some(now.saturating_add(RETRY_SECONDS));
        Some(ScanRequest {
            codex_home: self.codex_home.clone(),
            current: current.clone(),
            now,
            horizon_seconds: RETENTION_DAYS * SECONDS_PER_DAY,
            budget: self.budget,
            state,
            cancel: Arc::clone(&self.cancel),
        })
    }

    /// Stores what a scan found and keeps its position for the next attempt.
    /// Returns how many windows the history gained; when that is not zero the
    /// forecasts and charts on display are out of date.
    ///
    /// The store marks the import as completed when the scan read every log
    /// to its end, and leaves it open to resume otherwise.
    pub fn scan_finished(
        &mut self,
        outcome: ScanOutcome,
        history: &mut HistoryStore,
        now: i64,
    ) -> usize {
        self.state = Some(outcome.state);
        if self.cancelled() {
            return 0;
        }
        // The readings are a backfill, never an update to a completed import.
        let incremental = false;
        history.import(&outcome.readings, incremental, outcome.completed, now)
    }

    /// Ends a running scan early and stops further ones, for shutdown. The
    /// outcome of a scan that was running is discarded.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use super::event::tests::{current, event, window};
    use super::*;
    use crate::forecast::{Forecast, ForecastSettings};
    use crate::history::tests::{open, snapshot, TempDir, DAY, HOUR, MINUTE, NOW};
    use crate::history::{is_reset, HistorySample};
    use crate::history_file::{self, CoverageAnchor};
    use crate::model::{LimitWindow, FIVE_HOUR_MINUTES, WEEKLY_MINUTES};

    const FIVE_RESET: i64 = NOW + 2 * HOUR;
    const WEEK_RESET: i64 = NOW + 3 * DAY;

    struct Stopped;

    impl Stopwatch for Stopped {
        fn elapsed(&self) -> Duration {
            Duration::ZERO
        }
    }

    fn write_log(home: &Path, name: &str, lines: &[String]) {
        let path = home.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, lines.concat()).unwrap();
    }

    /// A log line with a weekly reading of the current cycle.
    fn weekly_line(minutes_ago: i64, percent: f64) -> String {
        let weekly = window(percent, 10_080, WEEK_RESET);
        let line = event(
            NOW - minutes_ago * MINUTE,
            None,
            Some(&weekly),
            Some("codex"),
        );
        format!("{line}\n")
    }

    /// A log line with a weekly reading of the cycle before the current one.
    fn earlier_cycle_line(days_ago: i64, percent: f64) -> String {
        let weekly = window(percent, 10_080, WEEK_RESET - 7 * DAY);
        let line = event(NOW - days_ago * DAY, None, Some(&weekly), Some("codex"));
        format!("{line}\n")
    }

    fn marker(history: &Path) -> PathBuf {
        history_file::with_suffix(history, ".imported")
    }

    /// Runs the scan a request asks for and hands the outcome back, as the
    /// tray does across its two threads.
    fn run(session: &mut Session, request: ScanRequest, store: &mut HistoryStore) -> usize {
        let outcome = scan(request, &Stopped);
        session.scan_finished(outcome, store, NOW)
    }

    fn minutes_ago(samples: &[HistorySample]) -> Vec<i64> {
        samples
            .iter()
            .map(|sample| (NOW * 1_000 - sample.timestamp_ms) / (MINUTE * 1_000))
            .collect()
    }

    #[test]
    fn the_import_runs_once_after_the_first_successful_refresh() {
        let directory = TempDir::new("importer-session");
        let home = directory.file("home");
        let history = directory.file("usage-history.json");
        let five = |percent| window(percent, 300, FIVE_RESET);
        let week = |percent| window(percent, 10_080, WEEK_RESET);
        let codex = Some("codex");
        write_log(
            &home,
            "sessions/2026/10/09/rollout.jsonl",
            &[
                event(
                    NOW - 40 * MINUTE,
                    Some(&five(5.0)),
                    Some(&week(55.0)),
                    codex,
                ) + "\n",
                event(
                    NOW - 20 * MINUTE,
                    Some(&five(25.0)),
                    Some(&week(58.0)),
                    codex,
                ) + "\n",
            ],
        );
        write_log(
            &home,
            "archived_sessions/earlier.jsonl",
            &[earlier_cycle_line(6, 80.0)],
        );
        let live = current(NOW, FIVE_RESET, WEEK_RESET);
        let mut store = open(&history);
        assert!(store.observe(&live, NOW));
        let mut session = Session::new(home.clone());

        let request = session
            .request_scan(&store, &live, NOW)
            .expect("the first successful refresh starts the import");
        assert_eq!(request.codex_home, home);
        assert_eq!((request.now, request.horizon_seconds), (NOW, 28 * DAY));
        assert_eq!(request.budget, ScanBudget::default());
        assert_eq!(request.current, live);
        assert!(
            session.request_scan(&store, &live, NOW + HOUR).is_none(),
            "a second scan does not start while one is running"
        );

        assert_eq!(run(&mut session, request, &mut store), 3);
        assert_eq!(minutes_ago(store.samples()), [6 * 24 * 60, 40, 20, 0]);
        let prior = store.samples()[0].weekly.unwrap();
        assert!(prior.is_imported && prior.used_percent == 80.0);
        assert_eq!(
            prior.reset_at_ms,
            Some((WEEK_RESET - 7 * DAY) * 1_000),
            "prior reset cycles are included"
        );
        let live_sample = store.samples()[3];
        assert!(!live_sample.weekly.unwrap().is_imported);
        assert!(store.import_completed() && marker(&history).exists());
        assert!(
            session.request_scan(&store, &live, NOW + DAY).is_none(),
            "a completed import is not repeated"
        );

        let reopened = open(&history);
        assert!(reopened.import_completed());
        assert!(
            Session::new(home)
                .request_scan(&reopened, &live, NOW)
                .is_none(),
            "normal launches with intact history do not rescan"
        );
    }

    #[test]
    fn an_unfinished_import_resumes_on_a_later_refresh() {
        let directory = TempDir::new("importer-resume");
        let home = directory.file("home");
        let history = directory.file("usage-history.json");
        let lines = [
            weekly_line(30, 20.0),
            weekly_line(20, 21.0),
            weekly_line(10, 22.0),
        ];
        write_log(&home, "sessions/log.jsonl", &lines);
        let live = current(NOW, FIVE_RESET, WEEK_RESET);
        let mut store = open(&history);
        assert!(store.observe(&live, NOW));
        // Each attempt can read one line and half of the next.
        let mut session = Session {
            budget: ScanBudget {
                max_bytes: (lines[0].len() + lines[1].len() / 2) as u64,
                ..ScanBudget::default()
            },
            ..Session::new(home.clone())
        };

        let request = session.request_scan(&store, &live, NOW).unwrap();
        assert_eq!(run(&mut session, request, &mut store), 1);
        assert!(
            !store.import_completed() && !marker(&history).exists(),
            "an interrupted import remains eligible to resume"
        );
        assert!(
            session
                .request_scan(&store, &live, NOW + RETRY_SECONDS - 1)
                .is_none(),
            "the next attempt waits five minutes"
        );

        let mut attempts = 1;
        let mut now = NOW;
        while !store.import_completed() {
            now += RETRY_SECONDS;
            attempts += 1;
            let request = session
                .request_scan(&store, &live, now)
                .expect("a later refresh resumes the import");
            let outcome = scan(request, &Stopped);
            assert!(
                outcome.scanned_bytes <= lines[1].len() as u64 * 3 / 2,
                "what was read before is not read again"
            );
            assert_eq!(session.scan_finished(outcome, &mut store, now), 1);
        }
        assert_eq!(attempts, 3);
        assert_eq!(minutes_ago(store.samples()), [30, 20, 10, 0]);
        assert!(marker(&history).exists() && open(&history).import_completed());
        assert!(session
            .request_scan(&store, &live, now + RETRY_SECONDS)
            .is_none());
    }

    #[test]
    fn a_restart_reads_the_logs_again_and_the_history_keeps_one_copy() {
        let directory = TempDir::new("importer-restart");
        let home = directory.file("home");
        let history = directory.file("usage-history.json");
        let lines = [weekly_line(30, 20.0), weekly_line(20, 21.0)];
        write_log(&home, "sessions/log.jsonl", &lines);
        let live = current(NOW, FIVE_RESET, WEEK_RESET);
        let mut store = open(&history);
        assert!(store.observe(&live, NOW));
        let mut session = Session {
            budget: ScanBudget {
                max_bytes: lines[0].len() as u64,
                ..ScanBudget::default()
            },
            ..Session::new(home.clone())
        };
        let request = session.request_scan(&store, &live, NOW).unwrap();
        assert_eq!(run(&mut session, request, &mut store), 1);

        // The position is not saved, so the next run starts from the top.
        let mut store = open(&history);
        assert!(!store.import_completed());
        let mut session = Session::new(home);
        let request = session.request_scan(&store, &live, NOW).unwrap();
        let outcome = scan(request, &Stopped);
        assert_eq!(outcome.readings.len(), 2);
        assert_eq!(session.scan_finished(outcome, &mut store, NOW), 1);
        assert_eq!(minutes_ago(store.samples()), [30, 20, 0]);
        assert!(store.import_completed());
    }

    #[test]
    fn missing_backfill_is_repaired_and_expired_backfill_is_not() {
        let directory = TempDir::new("importer-repair");
        let home = directory.file("home");
        let history = directory.file("usage-history.json");
        write_log(
            &home,
            "sessions/log.jsonl",
            &[earlier_cycle_line(10, 40.0), weekly_line(20, 58.0)],
        );
        let live = current(NOW, FIVE_RESET, WEEK_RESET);
        let mut store = open(&history);
        assert!(store.observe(&live, NOW));
        let mut session = Session::new(home.clone());
        let request = session.request_scan(&store, &live, NOW).unwrap();
        assert_eq!(run(&mut session, request, &mut store), 2);
        assert!(open(&history).import_completed());

        // An older app that keeps one cycle removes the backfilled one.
        let current_cycle: Vec<HistorySample> = store
            .samples()
            .iter()
            .filter(|sample| sample.timestamp_ms >= (NOW - 7 * DAY) * 1_000)
            .copied()
            .collect();
        assert_eq!(current_cycle.len(), 2);
        assert!(history_file::write_samples(&history, &current_cycle));
        let mut store = open(&history);
        let mut session = Session::new(home.clone());
        let request = session
            .request_scan(&store, &live, NOW)
            .expect("missing coverage triggers a repair import");
        assert_eq!(run(&mut session, request, &mut store), 1);
        assert_eq!(store.samples().len(), 3);
        assert!(store.import_completed() && open(&history).import_completed());

        // Completion flags from before coverage was recorded.
        std::fs::write(marker(&history), "2").unwrap();
        let mut store = open(&history);
        let mut session = Session::new(home.clone());
        let request = session
            .request_scan(&store, &live, NOW)
            .expect("an older completion flag gets one repair backfill");
        assert_eq!(run(&mut session, request, &mut store), 0);
        assert_eq!(store.samples().len(), 3);
        assert!(
            Session::new(home.clone())
                .request_scan(&open(&history), &live, NOW)
                .is_none(),
            "the repair backfill happens once"
        );

        let expired = CoverageAnchor {
            timestamp_ticks: history_file::ticks_from_ms((NOW - 29 * DAY) * 1_000),
            weekly: true,
        };
        history_file::write_completion(&marker(&history), &[expired]);
        assert!(
            Session::new(home)
                .request_scan(&open(&history), &live, NOW)
                .is_none(),
            "normal four-week expiry does not trigger a scan"
        );
    }

    #[test]
    fn cached_and_contradicting_log_entries_do_not_become_resets() {
        let directory = TempDir::new("importer-consistency");
        let home = directory.file("home");
        let history = directory.file("usage-history.json");
        // An older response repeated from a cache, then quota that the live
        // reading after it rules out.
        write_log(
            &home,
            "sessions/log.jsonl",
            &[
                weekly_line(50, 19.0),
                weekly_line(45, 21.0),
                weekly_line(40, 20.0),
                weekly_line(30, 99.0),
            ],
        );
        let mut store = open(&history);
        assert!(store.observe(&snapshot(NOW - HOUR, 20.0, Some(WEEK_RESET)), NOW));
        let live = snapshot(NOW, 25.0, Some(WEEK_RESET));
        assert!(store.observe(&live, NOW));
        let mut session = Session::new(home);

        let request = session.request_scan(&store, &live, NOW).unwrap();
        run(&mut session, request, &mut store);
        assert_eq!(minutes_ago(store.samples()), [60, 45, 0]);
        let percentages: Vec<f64> = store
            .samples()
            .iter()
            .map(|sample| sample.weekly.unwrap().used_percent)
            .collect();
        assert_eq!(percentages, [20.0, 21.0, 25.0]);
        assert!(store
            .samples()
            .windows(2)
            .all(|pair| !is_reset(&pair[0], &pair[1], true)));
        assert!(store.import_completed());
    }

    #[test]
    fn backfilled_readings_give_a_forecast_at_once() {
        let directory = TempDir::new("importer-forecast");
        let home = directory.file("home");
        let history = directory.file("usage-history.json");
        let reset = NOW + 6 * DAY;
        let week = |percent| window(percent, 10_080, reset);
        let codex = Some("codex");
        // The middle reading is from a session that only saw the five-hour window.
        write_log(
            &home,
            "sessions/log.jsonl",
            &[
                event(NOW - DAY, None, Some(&week(0.0)), codex) + "\n",
                event(NOW - 12 * HOUR, None, Some(&week(10.0)), codex) + "\n",
                event(
                    NOW - 6 * HOUR,
                    Some(&window(15.0, 300, NOW - 2 * HOUR)),
                    None,
                    codex,
                ) + "\n",
            ],
        );
        let live = Usage::from_windows(
            NOW,
            Some(LimitWindow {
                used_percent: 50.0,
                window_minutes: FIVE_HOUR_MINUTES,
                resets_at: Some(FIVE_RESET),
            }),
            Some(LimitWindow {
                used_percent: 20.0,
                window_minutes: WEEKLY_MINUTES,
                resets_at: Some(reset),
            }),
        );
        let mut store = open(&history);
        assert!(store.observe(&live, NOW));
        let mut session = Session::new(home);

        let request = session.request_scan(&store, &live, NOW).unwrap();
        assert_eq!(run(&mut session, request, &mut store), 3);
        assert_eq!(store.samples()[2].weekly, None);
        // 80 percent left at 20 percent a day lasts four days.
        let forecast = store.forecast(true, &live, true, ForecastSettings::default(), NOW);
        let Forecast::Estimated {
            depletes_at_ms,
            duration_seconds,
        } = forecast
        else {
            panic!("expected an estimate, got {forecast:?}");
        };
        assert_eq!(
            duration_seconds,
            4 * DAY,
            "backfilled data forecasts immediately across partial-window observations"
        );
        assert!((depletes_at_ms - (NOW + 4 * DAY) * 1_000).abs() <= 1_000);
    }

    #[test]
    fn shutdown_ends_the_scan_and_discards_its_outcome() {
        let directory = TempDir::new("importer-shutdown");
        let home = directory.file("home");
        write_log(&home, "sessions/log.jsonl", &[weekly_line(20, 20.0)]);
        let live = current(NOW, FIVE_RESET, WEEK_RESET);
        let mut store = open(&directory.file("usage-history.json"));
        assert!(store.observe(&live, NOW));
        let mut session = Session::new(home.clone());

        let request = session.request_scan(&store, &live, NOW).unwrap();
        session.cancel();
        let outcome = scan(request, &Stopped);
        assert!(outcome.readings.is_empty() && !outcome.completed);
        assert_eq!(session.scan_finished(outcome, &mut store, NOW), 0);
        assert!(session.request_scan(&store, &live, NOW + DAY).is_none());

        // A scan that finished just before shutdown is not stored either.
        let mut session = Session::new(home);
        let request = session.request_scan(&store, &live, NOW).unwrap();
        let outcome = scan(request, &Stopped);
        assert!(outcome.completed && outcome.readings.len() == 1);
        session.cancel();
        assert_eq!(session.scan_finished(outcome, &mut store, NOW), 0);
        assert!(store.samples().len() == 1 && !store.import_completed());
    }

    #[test]
    fn the_codex_home_comes_from_the_environment() {
        let text = |value: &str| Some(OsString::from(value));
        assert_eq!(
            codex_home_from(text(r"D:\codex"), text(r"C:\Users\me")),
            PathBuf::from(r"D:\codex")
        );
        for unset in [None, text(""), text("  ")] {
            assert_eq!(
                codex_home_from(unset, text(r"C:\Users\me")),
                PathBuf::from(r"C:\Users\me\.codex")
            );
        }
        assert_eq!(codex_home_from(None, None), PathBuf::new());
        assert_eq!(codex_home_from(text(" "), text("")), PathBuf::new());
    }
}
