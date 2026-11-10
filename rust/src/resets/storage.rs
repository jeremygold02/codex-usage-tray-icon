//! File access shared by the persisted reset state.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Reads a UTF-8 text file. Returns `None` when it is missing, unreadable or
/// larger than `max_bytes`.
pub(super) fn read_capped(path: &Path, max_bytes: u64) -> Option<String> {
    if fs::metadata(path).ok()?.len() > max_bytes {
        return None;
    }
    fs::read_to_string(path).ok()
}

/// Replaces `path` with `contents` through a temporary file beside it, so an
/// interrupted write leaves the previous file intact.
pub(super) fn write_atomically(path: &Path, contents: &str) -> io::Result<()> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the path has no directory"))?;
    fs::create_dir_all(directory)?;

    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".{}.tmp", std::process::id()));
    let temporary = PathBuf::from(temporary);
    let written = fs::write(&temporary, contents).and_then(|()| fs::rename(&temporary, path));
    if written.is_err() {
        // Best effort: the write already failed, and that is the error to report.
        let _ = fs::remove_file(&temporary);
    }
    written
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_DIRECTORY: AtomicU32 = AtomicU32::new(0);

    /// A uniquely named directory under the system temp directory, removed on drop.
    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new(label: &str) -> Self {
            let nanoseconds = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos());
            let name = format!(
                "usage-tray-rs-{label}-{}-{nanoseconds}-{}",
                std::process::id(),
                NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            );
            let path = std::env::temp_dir().join(name);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        pub fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn writes_replace_the_file_and_leave_nothing_else_behind() {
        let directory = TempDir::new("storage");
        // The directory is created on demand, as on a first run.
        let path = directory.file("nested").join("state.json");
        write_atomically(&path, "first").unwrap();
        write_atomically(&path, "second").unwrap();
        assert_eq!(read_capped(&path, 64).as_deref(), Some("second"));

        let names: Vec<_> = fs::read_dir(directory.file("nested"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["state.json"]);
    }

    #[test]
    fn missing_and_oversized_files_are_not_read() {
        let directory = TempDir::new("storage-limits");
        let path = directory.file("state.json");
        assert_eq!(read_capped(&path, 64), None);
        write_atomically(&path, "12345").unwrap();
        assert_eq!(read_capped(&path, 5).as_deref(), Some("12345"));
        assert_eq!(read_capped(&path, 4), None);
    }

    #[test]
    fn a_path_without_a_directory_is_refused() {
        let error = write_atomically(Path::new("usage-tray-rs-bare-name.json"), "{}").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
