//! Where the app keeps its files.
//!
//! The Rust build has its own data directory so it can run beside the C# app
//! without either one rewriting the other's settings or history. File names
//! and formats match the C# app's, so its files can be imported as they are.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

const DATA_DIRECTORY: &str = "CodexUsageTrayRs";
const LEGACY_DATA_DIRECTORY: &str = "CodexUsageTray";
/// Overrides the data directory, for tests and trial runs.
const DATA_DIRECTORY_VARIABLE: &str = "USAGE_TRAY_DATA_DIR";

pub const SETTINGS_FILE: &str = "settings.json";
pub const CODEX_HISTORY_FILE: &str = "usage-history.json";
pub const CODEX_HISTORY_IMPORT_FILE: &str = "usage-history.json.imported";
pub const CLAUDE_HISTORY_FILE: &str = "claude-usage-history.json";
pub const CLAUDE_STATUS_LINE_FILE: &str = "claude-usage.json";
pub const CLAUDE_ACCOUNT_FILE: &str = "claude-account-usage.json";
pub const BANKED_RESETS_FILE: &str = "banked-reset-state.json";
pub const RESET_REMINDERS_FILE: &str = "reset-reminders.json";

/// Everything worth carrying over from the C# app on first run.
const IMPORTED_FILES: [&str; 8] = [
    SETTINGS_FILE,
    CODEX_HISTORY_FILE,
    CODEX_HISTORY_IMPORT_FILE,
    CLAUDE_HISTORY_FILE,
    CLAUDE_STATUS_LINE_FILE,
    CLAUDE_ACCOUNT_FILE,
    BANKED_RESETS_FILE,
    RESET_REMINDERS_FILE,
];

/// This build's data directory, or `None` when `%APPDATA%` is unavailable.
pub fn data_directory() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(DATA_DIRECTORY_VARIABLE).filter(|path| !path.is_empty()) {
        return Some(PathBuf::from(path));
    }
    roaming_directory().map(|roaming| roaming.join(DATA_DIRECTORY))
}

/// Where the file `name` lives. Falls back to a folder under the temporary
/// directory in the unlikely case that `%APPDATA%` is unavailable, so callers
/// always have somewhere to write.
pub fn data_file(name: &str) -> PathBuf {
    data_directory()
        .unwrap_or_else(|| std::env::temp_dir().join(DATA_DIRECTORY))
        .join(name)
}

/// The C# app's data directory, read only to import existing settings and history.
pub fn legacy_data_directory() -> Option<PathBuf> {
    roaming_directory().map(|roaming| roaming.join(LEGACY_DATA_DIRECTORY))
}

fn roaming_directory() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

/// Creates `data` and, the first time only, seeds it with copies of the C#
/// app's files from `legacy`. Returns whether an import happened. The legacy
/// files are never modified.
pub fn prepare_data_directory(data: &Path, legacy: &Path) -> io::Result<bool> {
    if data.is_dir() {
        return Ok(false);
    }
    std::fs::create_dir_all(data)?;
    let mut imported = false;
    for name in IMPORTED_FILES {
        let source = legacy.join(name);
        if source.is_file() {
            std::fs::copy(&source, data.join(name))?;
            imported = true;
        }
    }
    if imported {
        leave_opt_ins_to_the_user(&data.join(SETTINGS_FILE))?;
    }
    Ok(imported)
}

/// Settings that make the app act on its own are turned off in the imported
/// copy. They were chosen for the C# app: starting a second tray app at
/// sign-in, or letting a second app spend banked resets, must be decided for
/// this build separately.
fn leave_opt_ins_to_the_user(settings: &Path) -> io::Result<()> {
    let Ok(json) = std::fs::read_to_string(settings) else {
        return Ok(());
    };
    let Ok(Value::Object(mut root)) = serde_json::from_str::<Value>(&json) else {
        return Ok(());
    };
    let mut changed = false;
    for name in ["StartWithWindows", "AutoRedeemResetCredits"] {
        changed |= root.insert(name.to_string(), Value::Bool(false)) != Some(Value::Bool(false));
    }
    if !changed {
        return Ok(());
    }
    std::fs::write(settings, Value::Object(root).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory that is removed when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let unique = format!("usage-tray-rs-{name}-{}", std::process::id());
            let path = std::env::temp_dir().join(unique);
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn first_run_copies_legacy_files_without_enabling_autostart() {
        let scratch = Scratch::new("import");
        let legacy = scratch.0.join("legacy");
        let data = scratch.0.join("data");
        std::fs::create_dir_all(&legacy).unwrap();
        let settings = r##"{"StartWithWindows":true,"AutoRedeemResetCredits":true,"RefreshSeconds":60,"TrayBoxColor":"#000000"}"##;
        std::fs::write(legacy.join(SETTINGS_FILE), settings).unwrap();
        std::fs::write(legacy.join(CODEX_HISTORY_FILE), "[1,2,3]").unwrap();
        std::fs::write(legacy.join("unrelated.txt"), "ignored").unwrap();

        assert!(prepare_data_directory(&data, &legacy).unwrap());

        let copied: Value =
            serde_json::from_str(&std::fs::read_to_string(data.join(SETTINGS_FILE)).unwrap())
                .unwrap();
        assert_eq!(copied["StartWithWindows"], Value::Bool(false));
        assert_eq!(copied["AutoRedeemResetCredits"], Value::Bool(false));
        assert_eq!(copied["RefreshSeconds"], 60);
        assert_eq!(copied["TrayBoxColor"], "#000000");
        assert_eq!(
            std::fs::read_to_string(data.join(CODEX_HISTORY_FILE)).unwrap(),
            "[1,2,3]"
        );
        assert!(!data.join("unrelated.txt").exists());
        // The source is left exactly as it was.
        assert_eq!(
            std::fs::read_to_string(legacy.join(SETTINGS_FILE)).unwrap(),
            settings
        );
    }

    #[test]
    fn an_existing_data_directory_is_never_overwritten() {
        let scratch = Scratch::new("no-reimport");
        let legacy = scratch.0.join("legacy");
        let data = scratch.0.join("data");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(legacy.join(SETTINGS_FILE), r#"{"RefreshSeconds":60}"#).unwrap();
        std::fs::write(data.join(SETTINGS_FILE), r#"{"RefreshSeconds":120}"#).unwrap();

        assert!(!prepare_data_directory(&data, &legacy).unwrap());
        assert_eq!(
            std::fs::read_to_string(data.join(SETTINGS_FILE)).unwrap(),
            r#"{"RefreshSeconds":120}"#
        );
    }

    #[test]
    fn a_missing_legacy_directory_still_yields_a_data_directory() {
        let scratch = Scratch::new("fresh");
        let data = scratch.0.join("data");
        assert!(!prepare_data_directory(&data, &scratch.0.join("absent")).unwrap());
        assert!(data.is_dir());
    }
}
