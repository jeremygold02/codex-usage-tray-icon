//! The worker-thread half of the import: finding recent session logs and
//! reading the quota events out of them.
//!
//! Nothing here touches the history. [`scan`] takes plain values, so it can
//! run on any thread, and returns plain values for
//! [`Session::scan_finished`](super::Session::scan_finished) to store.
//!
//! Logs are opened for reading only, shared with whatever Codex is doing to
//! them at that moment. Nothing read from a log is kept except the readings:
//! the resume state holds an offset, a length and a modification time per
//! file.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_SEQUENTIAL_SCAN,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

use crate::model::Usage;

use super::event::{parse_line, Criteria, MAX_LINE_BYTES};

/// The most readings one attempt returns, well beyond what four weeks of use
/// produce. Past it the oldest make way.
const MAX_SAMPLES: usize = 180_000;
const READ_BUFFER_BYTES: usize = 8_192;
/// The folders of the Codex home that hold session logs.
const LOG_FOLDERS: [&str; 2] = ["sessions", "archived_sessions"];
const LOG_SUFFIX: &[u8] = b".jsonl";
/// Every rate-limit event contains both. Looking for them first spares
/// parsing the far more numerous lines of conversation.
const EVENT_MARKERS: [&[u8]; 2] = [b"token_count", b"rate_limits"];

/// What one scan attempt may spend before it stops and leaves the rest to
/// the next attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanBudget {
    /// Bytes read from logs.
    pub max_bytes: u64,
    /// Time since the scan started, finding the logs included.
    pub max_duration: Duration,
}

impl Default for ScanBudget {
    /// The existing app's budget: 8 GiB or 30 seconds, whichever runs out first.
    fn default() -> Self {
        Self {
            max_bytes: 8 * 1024 * 1024 * 1024,
            max_duration: Duration::from_secs(30),
        }
    }
}

/// Tells a scan how long it has been running.
pub trait Stopwatch {
    /// Time since the scan started.
    fn elapsed(&self) -> Duration;
}

/// A [`Stopwatch`] on the system's monotonic clock.
pub struct SystemStopwatch(Instant);

impl SystemStopwatch {
    /// Starts timing now. Call it on the thread that runs the scan.
    pub fn start() -> Self {
        Self(Instant::now())
    }
}

impl Stopwatch for SystemStopwatch {
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }
}

/// Where earlier attempts stopped: for each recent log, how far it has been
/// read and what the file looked like then. It exists only in memory. After a
/// restart the logs are read from the start again and the history drops the
/// readings it already holds.
#[derive(Debug, Default)]
pub struct ScanState {
    cursors: HashMap<PathBuf, FileCursor>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct FileCursor {
    /// Everything before this offset has been read as whole lines.
    offset: u64,
    /// The file's size and modification time when it was last read.
    length: u64,
    modified: Option<SystemTime>,
}

impl FileCursor {
    /// True when the file was read to its end and has not changed since.
    fn has_read(&self, file: &LogFile) -> bool {
        self.offset == file.length
            && self.length == file.length
            && self.modified == Some(file.modified)
    }

    /// True when the file no longer continues what was read: it shrank, or
    /// it changed without growing.
    fn was_replaced(&self, file: &LogFile) -> bool {
        file.length < self.length
            || (file.length == self.length && self.modified != Some(file.modified))
    }
}

/// Everything one scan attempt needs. All of it is plain data, so a request
/// can be made on the UI thread and moved to a worker.
#[derive(Debug)]
pub struct ScanRequest {
    /// The folder that holds `sessions` and `archived_sessions`.
    pub codex_home: PathBuf,
    /// The live reading, which says what windows and cycles the account has.
    pub current: Usage,
    /// The time of the attempt, in Unix seconds.
    pub now: i64,
    /// How far back logs and events are read, in seconds.
    pub horizon_seconds: i64,
    pub budget: ScanBudget,
    /// Where earlier attempts stopped; the default for a first attempt.
    pub state: ScanState,
    /// Set from another thread to end the scan early.
    pub cancel: Arc<AtomicBool>,
}

/// What one scan attempt found.
#[derive(Debug)]
pub struct ScanOutcome {
    /// The readings this attempt recovered, oldest first and at most one per
    /// second. A resumed attempt returns only what earlier ones had not read.
    pub readings: Vec<Usage>,
    /// Where this attempt stopped, for the next one.
    pub state: ScanState,
    /// True when every recent log was read to its end within the budget, so
    /// the import can be marked as completed.
    pub completed: bool,
    /// Bytes read from logs by this attempt.
    pub scanned_bytes: u64,
}

/// Reads the account's past quota readings from the session logs under
/// `request.codex_home`, most recently modified logs first, until everything
/// is read or the budget is spent. Logs untouched for longer than the horizon
/// are not opened, and a log already read is only read from where it grew.
///
/// Failures are never reported: a log that is missing, locked or damaged
/// contributes nothing, and a folder that does not exist is an empty one.
pub fn scan(request: ScanRequest, stopwatch: &dyn Stopwatch) -> ScanOutcome {
    let ScanRequest {
        codex_home,
        current,
        now,
        horizon_seconds,
        budget,
        mut state,
        cancel,
    } = request;
    let startable = !is_blank(&codex_home) && !cancel.load(Ordering::Relaxed);
    let criteria = Criteria::new(&current, now, horizon_seconds).filter(|_| startable);
    let earliest = criteria.and_then(|criteria| system_time(criteria.earliest()));
    let (Some(criteria), Some(earliest)) = (criteria, earliest) else {
        // Without a current cycle no event can be attributed yet. The logs
        // stay unread and the import unfinished, for a later attempt.
        return ScanOutcome {
            readings: Vec::new(),
            state,
            completed: false,
            scanned_bytes: 0,
        };
    };

    let limits = Limits {
        budget,
        stopwatch,
        cancel: &cancel,
    };
    let files = recent_logs(&codex_home, earliest, &limits);
    let mut reader = Reader {
        criteria,
        limits,
        samples: BTreeMap::new(),
        scanned_bytes: 0,
    };
    let mut processed = 0;
    for file in &files {
        if reader.is_spent() {
            break;
        }
        let cursor = state.cursors.entry(file.path.clone()).or_default();
        if !cursor.has_read(file) {
            if cursor.was_replaced(file) {
                cursor.offset = 0;
            }
            cursor.offset = reader.read_log(&file.path, cursor.offset);
            cursor.length = file.length;
            cursor.modified = Some(file.modified);
        }
        processed += 1;
    }
    // Offsets are kept for recent logs only, so the state stays as small as
    // the list of logs it describes.
    let recent: HashSet<&Path> = files.iter().map(|file| file.path.as_path()).collect();
    state
        .cursors
        .retain(|path, _| recent.contains(path.as_path()));
    let completed = processed == files.len() && !reader.is_spent();
    ScanOutcome {
        readings: reader.samples.into_values().collect(),
        state,
        completed,
        scanned_bytes: reader.scanned_bytes,
    }
}

/// What ends an attempt early.
struct Limits<'a> {
    budget: ScanBudget,
    stopwatch: &'a dyn Stopwatch,
    cancel: &'a AtomicBool,
}

impl Limits<'_> {
    /// True once the attempt is cancelled or has used up its time.
    fn out_of_time(&self) -> bool {
        self.cancel.load(Ordering::Relaxed) || self.stopwatch.elapsed() >= self.budget.max_duration
    }
}

/// A log worth reading, as it was when the attempt found it.
struct LogFile {
    path: PathBuf,
    length: u64,
    modified: SystemTime,
}

impl LogFile {
    /// The log's size and modification time, asked of the file itself: a
    /// folder listing can lag behind a log that is still being written.
    fn describe(path: PathBuf) -> Option<Self> {
        let metadata = fs::symlink_metadata(&path).ok()?;
        Some(Self {
            length: metadata.len(),
            modified: metadata.modified().ok()?,
            path,
        })
    }
}

/// Every log under the two log folders that was modified at or after
/// `earliest`, most recently modified first: if the budget runs out, the
/// current cycle has been read before older ones.
fn recent_logs(codex_home: &Path, earliest: SystemTime, limits: &Limits<'_>) -> Vec<LogFile> {
    let mut files = Vec::new();
    for folder in LOG_FOLDERS {
        let mut pending = vec![codex_home.join(folder)];
        while let Some(directory) = pending.pop() {
            if limits.out_of_time() {
                break;
            }
            // A folder that is missing or unreadable has nothing to import.
            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries {
                if limits.out_of_time() {
                    break;
                }
                // A listing that fails part-way can fail the same way on
                // every further step, so the rest of this folder is given up.
                let Ok(entry) = entry else {
                    break;
                };
                let Ok(listed) = entry.metadata() else {
                    continue;
                };
                let attributes = listed.file_attributes();
                if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                    // Links and junctions can lead out of the log folders, or
                    // back into them without end.
                    if attributes & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
                        pending.push(entry.path());
                    }
                } else if is_log_name(&entry.file_name()) {
                    files.extend(
                        LogFile::describe(entry.path()).filter(|file| file.modified >= earliest),
                    );
                }
            }
        }
    }
    files.sort_by(|left, right| right.modified.cmp(&left.modified));
    files
}

fn is_log_name(name: &OsStr) -> bool {
    let name = name.as_encoded_bytes();
    name.len() >= LOG_SUFFIX.len()
        && name[name.len() - LOG_SUFFIX.len()..].eq_ignore_ascii_case(LOG_SUFFIX)
}

fn is_blank(path: &Path) -> bool {
    path.as_os_str().to_string_lossy().trim().is_empty()
}

fn system_time(unix_seconds: i64) -> Option<SystemTime> {
    let distance = Duration::from_secs(unix_seconds.unsigned_abs());
    if unix_seconds >= 0 {
        UNIX_EPOCH.checked_add(distance)
    } else {
        UNIX_EPOCH.checked_sub(distance)
    }
}

/// Opens a log without getting in the way of Codex, which may be appending
/// to it, renaming it or moving it to the archive at this moment.
fn open_shared(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_SEQUENTIAL_SCAN)
        .open(path)
}

/// The line being put together from successive reads. It never holds more
/// than [`MAX_LINE_BYTES`]: once a line is longer than that, the rest of it
/// is dropped as it is read.
struct PendingLine {
    bytes: Vec<u8>,
    oversized: bool,
}

impl PendingLine {
    fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(MAX_LINE_BYTES),
            oversized: false,
        }
    }

    fn extend(&mut self, segment: &[u8]) {
        if self.oversized {
            return;
        }
        if self.bytes.len() + segment.len() > MAX_LINE_BYTES {
            self.oversized = true;
        } else {
            self.bytes.extend_from_slice(segment);
        }
    }
}

/// Reads logs for one attempt and collects the readings found in them.
struct Reader<'a> {
    criteria: Criteria,
    limits: Limits<'a>,
    /// Accepted readings by the second they were taken.
    samples: BTreeMap<i64, Usage>,
    scanned_bytes: u64,
}

impl Reader<'_> {
    /// True once the attempt has to stop reading: it is out of time, or it
    /// has read as many bytes as the budget allows.
    fn is_spent(&self) -> bool {
        self.limits.out_of_time() || self.scanned_bytes >= self.limits.budget.max_bytes
    }

    /// Reads whole lines of the log from `start` on and returns the offset
    /// after the last one. A log that cannot be opened or read keeps its
    /// offset and is tried again by the next attempt.
    fn read_log(&mut self, path: &Path, start: u64) -> u64 {
        let mut committed = start;
        let Ok(mut file) = open_shared(path) else {
            return committed;
        };
        if file.seek(SeekFrom::Start(start)).is_err() {
            return committed;
        }
        let mut buffer = [0u8; READ_BUFFER_BYTES];
        let mut line = PendingLine::new();
        let mut position = start;
        while !self.is_spent() {
            let allowance = self.limits.budget.max_bytes - self.scanned_bytes;
            let wanted = usize::try_from(allowance).map_or(buffer.len(), |allowance| {
                allowance.min(buffer.len())
            });
            let read = match file.read(&mut buffer[..wanted]) {
                Ok(0) => break,
                Ok(read) => read,
                Err(_) => return committed,
            };
            self.scanned_bytes += read as u64;
            let mut chunk = &buffer[..read];
            while let Some(newline) = chunk.iter().position(|&byte| byte == b'\n') {
                line.extend(&chunk[..newline]);
                self.end_line(&mut line);
                chunk = &chunk[newline + 1..];
                position += newline as u64 + 1;
                committed = position;
            }
            line.extend(chunk);
            position += chunk.len() as u64;
        }
        // A last line without a newline may be complete or half written. It
        // is read if it parses, but its offset is not committed, so a half
        // written line is read again once its writer has finished it.
        if !self.limits.out_of_time() {
            self.end_line(&mut line);
        }
        committed
    }

    fn end_line(&mut self, line: &mut PendingLine) {
        if !line.oversized && !line.bytes.is_empty() {
            self.add_line(&line.bytes);
        }
        line.bytes.clear();
        line.oversized = false;
    }

    fn add_line(&mut self, line: &[u8]) {
        if !EVENT_MARKERS.iter().all(|marker| contains(line, marker)) {
            return;
        }
        let Ok(text) = std::str::from_utf8(line) else {
            return;
        };
        let Some(reading) = parse_line(text, &self.criteria) else {
            return;
        };
        match self.samples.entry(reading.updated_at) {
            // Sessions that ran side by side log the same reading; each can
            // supply a window the other lacks.
            Entry::Occupied(mut entry) => {
                let existing = entry.get_mut();
                if existing.five_hour.is_none() {
                    existing.five_hour = reading.five_hour;
                }
                if existing.weekly.is_none() {
                    existing.weekly = reading.weekly;
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(reading);
                if self.samples.len() > MAX_SAMPLES {
                    self.samples.pop_first();
                }
            }
        }
    }
}

fn contains(line: &[u8], marker: &[u8]) -> bool {
    line.windows(marker.len())
        .any(|candidate| candidate == marker)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::io::Write;
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    use super::super::event::tests::{current, event, window, HORIZON};
    use super::*;
    use crate::history::tests::{TempDir, DAY, HOUR, MINUTE, NOW};

    const FIVE_RESET: i64 = NOW + 2 * HOUR;
    const WEEK_RESET: i64 = NOW + 3 * DAY;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// A stopwatch on which no time passes.
    struct Stopped;

    impl Stopwatch for Stopped {
        fn elapsed(&self) -> Duration {
            Duration::ZERO
        }
    }

    /// A stopwatch whose time is up once it has been read `limit` times.
    struct ExpiresAfter {
        reads: Cell<u32>,
        limit: u32,
    }

    impl Stopwatch for ExpiresAfter {
        fn elapsed(&self) -> Duration {
            self.reads.set(self.reads.get() + 1);
            if self.reads.get() > self.limit {
                Duration::MAX
            } else {
                Duration::ZERO
            }
        }
    }

    fn request(home: &Path, state: ScanState) -> ScanRequest {
        ScanRequest {
            codex_home: home.to_path_buf(),
            current: current(NOW, FIVE_RESET, WEEK_RESET),
            now: NOW,
            horizon_seconds: HORIZON,
            budget: ScanBudget::default(),
            state,
            cancel: Arc::default(),
        }
    }

    fn read(home: &Path, state: ScanState) -> ScanOutcome {
        scan(request(home, state), &Stopped)
    }

    /// A log line with a weekly reading of `percent`, taken `minutes_ago`.
    fn weekly_line(minutes_ago: i64, percent: f64) -> String {
        let weekly = window(percent, 10_080, WEEK_RESET);
        let line = event(NOW - minutes_ago * MINUTE, None, Some(&weekly), Some("codex"));
        format!("{line}\n")
    }

    /// How many minutes before `NOW` each reading was taken.
    fn minutes_ago(outcome: &ScanOutcome) -> Vec<i64> {
        outcome
            .readings
            .iter()
            .map(|reading| (NOW - reading.updated_at) / MINUTE)
            .collect()
    }

    fn write_log(path: &Path, contents: impl AsRef<[u8]>) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn append(path: &Path, contents: &str) {
        let mut file = OpenOptions::new().append(true).open(path).unwrap();
        file.write_all(contents.as_bytes()).unwrap();
    }

    fn set_modified(path: &Path, unix: i64) {
        let file = OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(system_time(unix).unwrap()).unwrap();
    }

    #[test]
    fn sessions_and_archived_sessions_are_scanned_and_duplicates_merged() {
        let directory = TempDir::new("importer-scan");
        let home = directory.file("home");
        let five = |percent| window(percent, 300, FIVE_RESET);
        let first = event(NOW - 40 * MINUTE, Some(&five(5.0)), None, Some("codex"));
        let second = event(NOW - 20 * MINUTE, Some(&five(25.0)), None, Some("codex"));
        let duplicate = event(
            NOW - 20 * MINUTE,
            None,
            Some(&window(40.0, 10_080, WEEK_RESET)),
            Some("codex"),
        );
        let oversized = "x".repeat(MAX_LINE_BYTES + 1);
        write_log(
            &home.join("sessions/2026/09/28/live.jsonl"),
            format!("{{invalid\n{oversized}\n{second}\n{first}\n"),
        );
        write_log(&home.join("archived_sessions/old.jsonl"), &duplicate);

        let outcome = read(&home, ScanState::default());
        assert_eq!(
            minutes_ago(&outcome),
            [40, 20],
            "samples are sorted by event time"
        );
        let merged = &outcome.readings[1];
        assert_eq!(merged.five_hour.as_ref().unwrap().used_percent, 25.0);
        assert_eq!(
            merged.weekly.as_ref().unwrap().used_percent,
            40.0,
            "duplicate event without a trailing newline retains independently available windows"
        );
        assert!(outcome.completed);

        let missing = read(&home.join("missing"), ScanState::default());
        assert!(
            missing.readings.is_empty() && missing.completed,
            "missing session directories are harmless, and leave nothing to import"
        );
    }

    #[test]
    fn later_attempts_read_only_what_was_appended() {
        let directory = TempDir::new("importer-incremental");
        let home = directory.file("incremental");
        let path = home.join("sessions").join("append.jsonl");
        let first = weekly_line(20, 20.0);
        let second = weekly_line(10, 21.0);
        write_log(&path, &first);

        let outcome = read(&home, ScanState::default());
        assert_eq!(
            minutes_ago(&outcome),
            [20],
            "initial incremental scan reads the existing log"
        );
        let outcome = read(&home, outcome.state);
        assert!(
            outcome.readings.is_empty() && outcome.scanned_bytes == 0 && outcome.completed,
            "unchanged logs are not reread"
        );

        let (head, tail) = second.split_at(second.len() / 2);
        append(&path, head);
        let outcome = read(&home, outcome.state);
        assert!(
            outcome.readings.is_empty() && outcome.completed,
            "partial appended lines wait until the writer finishes"
        );
        append(&path, tail);
        let outcome = read(&home, outcome.state);
        assert_eq!(minutes_ago(&outcome), [10]);
        assert_eq!(
            outcome.scanned_bytes,
            second.len() as u64,
            "completed appended lines are read without rescanning the prefix"
        );

        fs::write(&path, &first).unwrap();
        let outcome = read(&home, outcome.state);
        assert_eq!(
            minutes_ago(&outcome),
            [20],
            "truncated logs restart from the beginning"
        );

        // A log replaced by one of the same length is a different log.
        let replacement = weekly_line(30, 20.0);
        assert_eq!(replacement.len(), first.len());
        fs::write(&path, &replacement).unwrap();
        set_modified(&path, NOW - HOUR);
        let outcome = read(&home, outcome.state);
        assert_eq!(minutes_ago(&outcome), [30]);
        assert_eq!(read(&home, outcome.state).scanned_bytes, 0);
    }

    #[test]
    fn a_last_line_without_a_newline_is_read_again_until_it_is_finished() {
        let directory = TempDir::new("importer-tail");
        let home = directory.file("home");
        let path = home.join("sessions").join("open.jsonl");
        let first = weekly_line(20, 20.0);
        let second = weekly_line(10, 21.0);
        let unfinished = second.trim_end();
        write_log(&path, format!("{first}{unfinished}"));

        let outcome = read(&home, ScanState::default());
        assert_eq!(minutes_ago(&outcome), [20, 10]);
        assert!(outcome.completed);
        // Its offset is not committed, so it alone is read again.
        let outcome = read(&home, outcome.state);
        assert_eq!(minutes_ago(&outcome), [10]);
        assert_eq!(outcome.scanned_bytes, unfinished.len() as u64);
        append(&path, "\n");
        let outcome = read(&home, outcome.state);
        assert_eq!(minutes_ago(&outcome), [10]);
        assert_eq!(read(&home, outcome.state).scanned_bytes, 0);
    }

    #[test]
    fn a_cancelled_scan_reads_nothing() {
        let directory = TempDir::new("importer-cancel");
        let home = directory.file("home");
        write_log(&home.join("sessions/log.jsonl"), weekly_line(20, 20.0));

        let cancelled = ScanRequest {
            cancel: Arc::new(AtomicBool::new(true)),
            ..request(&home, ScanState::default())
        };
        let outcome = scan(cancelled, &Stopped);
        assert!(
            outcome.readings.is_empty() && !outcome.completed && outcome.scanned_bytes == 0,
            "pre-canceled scan returns no samples"
        );
        assert_eq!(minutes_ago(&read(&home, outcome.state)), [20]);
    }

    #[test]
    fn a_scan_without_a_current_cycle_or_a_home_leaves_the_import_unfinished() {
        let directory = TempDir::new("importer-idle");
        let home = directory.file("home");
        write_log(&home.join("sessions/log.jsonl"), weekly_line(20, 20.0));
        let first = read(&home, ScanState::default());
        assert!(first.completed);

        let no_cycle = ScanRequest {
            current: Usage::from_windows(NOW, None, None),
            ..request(&home, first.state)
        };
        let outcome = scan(no_cycle, &Stopped);
        assert!(outcome.readings.is_empty() && !outcome.completed);
        assert_eq!(
            outcome.state.cursors.len(),
            1,
            "what earlier attempts read is remembered"
        );
        for blank in ["", "  "] {
            let outcome = read(Path::new(blank), ScanState::default());
            assert!(outcome.readings.is_empty() && !outcome.completed);
        }
    }

    #[test]
    fn the_byte_budget_ends_an_attempt_and_the_next_one_resumes() {
        let directory = TempDir::new("importer-bytes");
        let home = directory.file("home");
        let lines = [
            weekly_line(30, 20.0),
            weekly_line(20, 21.0),
            weekly_line(10, 22.0),
        ];
        write_log(&home.join("sessions/log.jsonl"), lines.concat());
        let total = lines.concat().len() as u64;
        // Enough for the first line and half of the second.
        let budget = ScanBudget {
            max_bytes: (lines[0].len() + lines[1].len() / 2) as u64,
            ..ScanBudget::default()
        };

        let first = scan(
            ScanRequest {
                budget,
                ..request(&home, ScanState::default())
            },
            &Stopped,
        );
        assert_eq!(minutes_ago(&first), [30]);
        assert!(!first.completed, "the log was not read to its end");
        assert_eq!(first.scanned_bytes, budget.max_bytes);

        let second = scan(
            ScanRequest {
                budget,
                ..request(&home, first.state)
            },
            &Stopped,
        );
        assert_eq!(minutes_ago(&second), [20]);
        assert!(!second.completed);

        let third = read(&home, second.state);
        assert_eq!(minutes_ago(&third), [10]);
        assert!(third.completed);
        assert_eq!(
            third.scanned_bytes,
            lines[2].len() as u64,
            "only the unfinished part is read again"
        );
        assert!(first.scanned_bytes + second.scanned_bytes + third.scanned_bytes < 2 * total);

        // A budget that ends exactly at the end of the log still counts as
        // spent: whether more logs follow was never looked at.
        let exact = scan(
            ScanRequest {
                budget: ScanBudget {
                    max_bytes: total,
                    ..ScanBudget::default()
                },
                ..request(&home, ScanState::default())
            },
            &Stopped,
        );
        assert_eq!(minutes_ago(&exact), [30, 20, 10]);
        assert!(!exact.completed);
        let confirmed = read(&home, exact.state);
        assert!(confirmed.completed && confirmed.scanned_bytes == 0);
    }

    #[test]
    fn an_attempt_that_runs_out_of_time_resumes_without_losing_or_repeating_lines() {
        let directory = TempDir::new("importer-time");
        let home = directory.file("home");
        for (index, folder) in ["sessions/a", "sessions/b", "archived_sessions"]
            .iter()
            .enumerate()
        {
            let base = 60 - 20 * index as i64;
            let path = home.join(folder).join("log.jsonl");
            write_log(
                &path,
                format!("{}{}", weekly_line(base, 20.0), weekly_line(base - 10, 20.0)),
            );
        }
        let expired = scan(
            request(&home, ScanState::default()),
            &ExpiresAfter {
                reads: Cell::new(0),
                limit: 0,
            },
        );
        assert!(expired.readings.is_empty() && !expired.completed && expired.scanned_bytes == 0);

        // Wherever the time runs out, the two attempts together read every
        // line exactly once.
        let mut limit = 0;
        loop {
            let stopwatch = ExpiresAfter {
                reads: Cell::new(0),
                limit,
            };
            let first = scan(request(&home, ScanState::default()), &stopwatch);
            let ran_out = stopwatch.reads.get() > limit;
            assert_eq!(first.completed, !ran_out, "after {limit} clock readings");
            let second = read(&home, first.state);
            assert!(second.completed);
            let mut found = minutes_ago(&first);
            found.extend(minutes_ago(&second));
            found.sort_unstable_by(|left, right| right.cmp(left));
            assert_eq!(
                found,
                [60, 50, 40, 30, 20, 10],
                "after {limit} clock readings"
            );
            if !ran_out {
                break;
            }
            limit += 1;
            assert!(limit < 1_000, "the scan never finished");
        }
        assert!(limit > 3, "the time is checked while logs are read");
    }

    #[test]
    fn recently_modified_logs_are_read_first() {
        let directory = TempDir::new("importer-order");
        let home = directory.file("home");
        let older = home.join("sessions/2026/10/01/older.jsonl");
        let newer = home.join("archived_sessions/newer.jsonl");
        write_log(&older, weekly_line(20, 20.0));
        write_log(&newer, weekly_line(10, 21.0));
        set_modified(&older, NOW - 2 * DAY);
        set_modified(&newer, NOW - DAY);
        let budget = ScanBudget {
            max_bytes: weekly_line(10, 21.0).len() as u64,
            ..ScanBudget::default()
        };

        let first = scan(
            ScanRequest {
                budget,
                ..request(&home, ScanState::default())
            },
            &Stopped,
        );
        assert_eq!(minutes_ago(&first), [10]);
        assert!(!first.completed);
        let second = read(&home, first.state);
        assert_eq!(minutes_ago(&second), [20]);
        assert!(second.completed);
    }

    #[test]
    fn logs_untouched_for_four_weeks_are_not_opened() {
        let directory = TempDir::new("importer-horizon");
        let home = directory.file("home");
        let expired = home.join("sessions/expired.jsonl");
        let oldest = home.join("sessions/oldest.jsonl");
        let recent = home.join("sessions/recent.jsonl");
        write_log(&expired, weekly_line(30, 20.0));
        write_log(&oldest, weekly_line(20, 21.0));
        write_log(&recent, weekly_line(10, 22.0));
        set_modified(&expired, NOW - HORIZON - 1);
        set_modified(&oldest, NOW - HORIZON);
        set_modified(&recent, NOW);

        let outcome = read(&home, ScanState::default());
        assert_eq!(minutes_ago(&outcome), [20, 10]);
        assert!(outcome.completed);
        assert_eq!(
            outcome.scanned_bytes,
            (weekly_line(20, 21.0).len() + weekly_line(10, 22.0).len()) as u64
        );
        assert_eq!(outcome.state.cursors.len(), 2);

        // A log that ages out is forgotten rather than tracked forever.
        set_modified(&oldest, NOW - HORIZON - 1);
        let outcome = read(&home, outcome.state);
        assert!(outcome.completed && outcome.scanned_bytes == 0);
        assert_eq!(outcome.state.cursors.len(), 1);
        fs::remove_file(&recent).unwrap();
        assert!(read(&home, outcome.state).state.cursors.is_empty());
    }

    #[test]
    fn only_logs_in_real_folders_are_read() {
        let directory = TempDir::new("importer-folders");
        let home = directory.file("home");
        write_log(
            &home.join("sessions/2026/10/09/rollout.JSONL"),
            weekly_line(50, 20.0),
        );
        write_log(&home.join("sessions/notes.json"), weekly_line(40, 20.0));
        write_log(&home.join("sessions/jsonl"), weekly_line(30, 20.0));
        write_log(&home.join("other/log.jsonl"), weekly_line(20, 20.0));
        write_log(&home.join("log.jsonl"), weekly_line(10, 20.0));
        // A junction back to the sessions folder must not be followed.
        let created = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(home.join("sessions").join("loop"))
            .arg(home.join("sessions"))
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .unwrap();
        assert!(created.status.success(), "{created:?}");
        // A folder named like a log is still a folder.
        write_log(
            &home.join("archived_sessions/folder.jsonl/inner.jsonl"),
            weekly_line(5, 20.0),
        );

        let outcome = read(&home, ScanState::default());
        assert_eq!(minutes_ago(&outcome), [50, 5]);
        assert_eq!(
            outcome.state.cursors.len(),
            2,
            "no log is reached a second time through the junction"
        );
        assert!(outcome.completed);
    }

    #[test]
    fn logs_are_opened_read_only_and_shared() {
        let directory = TempDir::new("importer-sharing");
        let path = directory.file("log.jsonl");
        fs::write(&path, weekly_line(20, 20.0)).unwrap();

        let mut reader = open_shared(&path).unwrap();
        assert!(
            reader.write_all(b"x").is_err(),
            "the handle cannot change the log"
        );
        append(&path, &weekly_line(10, 21.0));
        let moved = directory.file("archived.jsonl");
        fs::rename(&path, &moved).unwrap();
        fs::remove_file(&moved).unwrap();
        drop(reader);
        assert!(!moved.exists() && !path.exists());
    }

    #[test]
    fn a_scan_leaves_the_logs_as_they_were() {
        let directory = TempDir::new("importer-untouched");
        let home = directory.file("home");
        let path = home.join("sessions").join("log.jsonl");
        let contents = format!("{}{}", weekly_line(20, 20.0), weekly_line(10, 21.0));
        write_log(&path, &contents);
        set_modified(&path, NOW - DAY);
        let before = fs::metadata(&path).unwrap();
        // Codex keeps the log of a running session open for appending.
        let writer = OpenOptions::new().append(true).open(&path).unwrap();

        let outcome = read(&home, ScanState::default());
        assert_eq!(minutes_ago(&outcome), [20, 10]);
        drop(writer);
        let after = fs::metadata(&path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), contents);
        assert_eq!(after.modified().unwrap(), before.modified().unwrap());
        assert_eq!(after.len(), before.len());
        assert_eq!(after.file_attributes(), before.file_attributes());
        let entries = fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(entries, 1, "nothing is written beside the log");
    }

    #[test]
    fn a_log_that_cannot_be_opened_is_tried_again_by_the_next_attempt() {
        let directory = TempDir::new("importer-locked");
        let home = directory.file("home");
        let locked = home.join("sessions").join("locked.jsonl");
        write_log(&locked, weekly_line(20, 20.0));
        write_log(&home.join("sessions/free.jsonl"), weekly_line(10, 21.0));
        let exclusive = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&locked)
            .unwrap();

        let outcome = read(&home, ScanState::default());
        assert_eq!(minutes_ago(&outcome), [10]);
        assert!(
            outcome.completed,
            "one unreadable log does not hold up the import"
        );
        drop(exclusive);
        assert_eq!(minutes_ago(&read(&home, outcome.state)), [20]);
    }

    #[test]
    fn lines_of_any_shape_are_split_and_filtered() {
        let directory = TempDir::new("importer-lines");
        let home = directory.file("home");
        let line = |minutes_ago| weekly_line(minutes_ago, 20.0).trim_end().to_string();
        let padded = |minutes_ago, length: usize| {
            let line: String = line(minutes_ago);
            format!("{line}{}", " ".repeat(length - line.len()))
        };
        let mut contents = Vec::new();
        // A byte-order mark, as some editors add when a log is saved by hand.
        contents.extend_from_slice(b"\xEF\xBB\xBF");
        contents.extend_from_slice(format!("{}\r\n", line(90)).as_bytes());
        contents.extend_from_slice(b"\n\r\n");
        // Longer than one read, and exactly as long as a line may be.
        contents.extend_from_slice(format!("{}\n", padded(80, READ_BUFFER_BYTES * 3)).as_bytes());
        contents.extend_from_slice(format!("{}\n", padded(70, MAX_LINE_BYTES)).as_bytes());
        contents.extend_from_slice(format!("{}\n", padded(60, MAX_LINE_BYTES + 1)).as_bytes());
        // An event in every respect, except that one of its strings is not text.
        contents.extend_from_slice(b"{\"note\":\"\xFF\xFE\",");
        contents.extend_from_slice(line(50)[1..].as_bytes());
        contents.push(b'\n');
        contents.extend_from_slice(&[0u8; 3 * READ_BUFFER_BYTES]);
        contents.extend_from_slice(format!("\n{}\n", line(40)).as_bytes());
        contents.extend_from_slice(
            format!("{}\n", line(30).replace("token_count", "token-count")).as_bytes(),
        );
        contents.extend_from_slice(format!("{}\n", &line(20)[..40]).as_bytes());
        contents.extend_from_slice(line(10).as_bytes());
        write_log(&home.join("sessions/shapes.jsonl"), &contents);

        let outcome = read(&home, ScanState::default());
        assert_eq!(minutes_ago(&outcome), [90, 80, 70, 40, 10]);
        assert!(outcome.completed);
        assert_eq!(outcome.scanned_bytes, contents.len() as u64);
    }

    #[test]
    fn readings_beyond_the_cap_displace_the_oldest() {
        let cancel = AtomicBool::new(false);
        let mut reader = Reader {
            criteria: Criteria::new(&current(NOW, FIVE_RESET, WEEK_RESET), NOW, HORIZON).unwrap(),
            limits: Limits {
                budget: ScanBudget::default(),
                stopwatch: &Stopped,
                cancel: &cancel,
            },
            samples: BTreeMap::new(),
            scanned_bytes: 0,
        };
        // One reading a second, starting twenty days ago.
        let first = NOW - 20 * DAY;
        for index in 0..MAX_SAMPLES as i64 {
            let second = first + index;
            reader
                .samples
                .insert(second, Usage::from_windows(second, None, None));
        }
        reader.add_line(weekly_line(1, 20.0).trim_end().as_bytes());
        assert_eq!(reader.samples.len(), MAX_SAMPLES);
        assert_eq!(reader.samples.keys().next(), Some(&(first + 1)));
        assert_eq!(reader.samples.keys().next_back(), Some(&(NOW - MINUTE)));
        // A reading older than everything kept is the one that gives way.
        let earlier_cycle = window(20.0, 10_080, WEEK_RESET - 21 * DAY);
        let older = event(NOW - 21 * DAY, None, Some(&earlier_cycle), Some("codex"));
        assert!(parse_line(&older, &reader.criteria).is_some());
        reader.add_line(older.as_bytes());
        assert_eq!(reader.samples.len(), MAX_SAMPLES);
        assert_eq!(reader.samples.keys().next(), Some(&(first + 1)));
    }

    #[test]
    fn a_scan_runs_on_a_worker_thread() {
        let directory = TempDir::new("importer-thread");
        let home = directory.file("home");
        write_log(&home.join("sessions/log.jsonl"), weekly_line(20, 20.0));
        let request = request(&home, ScanState::default());

        let worker = std::thread::spawn(move || scan(request, &SystemStopwatch::start()));
        let outcome = worker.join().unwrap();
        assert_eq!(minutes_ago(&outcome), [20]);
        assert!(outcome.completed);
    }

    #[test]
    fn the_default_budget_is_the_existing_app_s() {
        let budget = ScanBudget::default();
        assert_eq!(budget.max_bytes, 8_589_934_592);
        assert_eq!(budget.max_duration, Duration::from_millis(30_000));
    }
}
