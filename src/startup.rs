//! "Start with Windows": a per-user `Run` entry that starts this executable
//! at sign-in.
//!
//! The entry is the value `CodexUsageTrayRs`. The C# app keeps its own entry,
//! `CodexUsageTray`, under the same key. This module never reads, changes or
//! deletes that one, so each build can be switched on and off separately.

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{LocalFree, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::Globalization::{CompareStringOrdinal, CSTR_EQUAL};
use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_EXPAND_SZ,
    REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows_sys::Win32::UI::Shell::CommandLineToArgvW;

use crate::win::{from_wide, wide};

const RUN_KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "CodexUsageTrayRs";
/// Enough for any path; longer expansions grow the buffer.
const EXPANSION_UNITS: usize = 512;

/// Why the startup entry could not be changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupError {
    KeyUnavailable,
    WriteFailed,
}

impl StartupError {
    /// A description suitable for showing to the user.
    pub fn message(self) -> &'static str {
        match self {
            Self::KeyUnavailable => "Could not open the Windows startup registry key.",
            Self::WriteFailed => "Could not update the Windows startup registry entry.",
        }
    }
}

/// True when this build's entry starts `executable` at sign-in. An entry
/// left behind by a copy in another folder does not count.
pub fn is_enabled(executable: &Path) -> bool {
    is_enabled_in(RUN_KEY_PATH, executable)
}

/// Adds this build's entry for `executable`, or removes the entry.
pub fn set_enabled(executable: &Path, enabled: bool) -> Result<(), StartupError> {
    set_enabled_in(RUN_KEY_PATH, executable, enabled)
}

fn is_enabled_in(key_path: &str, executable: &Path) -> bool {
    Key::open(key_path)
        .and_then(|key| key.read_string(VALUE_NAME))
        .is_some_and(|command| targets_executable(&command, executable))
}

fn set_enabled_in(key_path: &str, executable: &Path, enabled: bool) -> Result<(), StartupError> {
    let key = Key::create(key_path).ok_or(StartupError::KeyUnavailable)?;
    let changed = if enabled {
        key.write_string(VALUE_NAME, &quote(executable))
    } else {
        key.delete_value(VALUE_NAME)
    };
    changed.then_some(()).ok_or(StartupError::WriteFailed)
}

/// The command stored for `executable`. Quoting keeps a path with spaces in
/// one piece when Windows runs it.
fn quote(executable: &Path) -> OsString {
    let mut command = OsString::from("\"");
    command.push(executable);
    command.push("\"");
    command
}

fn targets_executable(command: &str, executable: &Path) -> bool {
    let Some(target) = executable_target(command).filter(|target| !target.trim().is_empty()) else {
        return false;
    };
    let Some(target) = expand_environment(&target).filter(|target| is_rooted(target)) else {
        return false;
    };
    match (std::path::absolute(target), std::path::absolute(executable)) {
        (Ok(registered), Ok(current)) => same_path(&registered, &current),
        _ => false,
    }
}

/// The program a command line starts, split the way Windows splits it.
fn executable_target(command: &str) -> Option<String> {
    let command = command.trim();
    // An empty command line would make the API report this process's own path.
    if command.is_empty() {
        return None;
    }
    let command = wide(command);
    let mut count = 0i32;
    // SAFETY: `command` is NUL-terminated and outlives the call; `count` is a
    // valid place for the argument count.
    let arguments = unsafe { CommandLineToArgvW(command.as_ptr(), &mut count) };
    if arguments.is_null() {
        return None;
    }
    // SAFETY: on success `arguments` points to `count` pointers to
    // NUL-terminated strings, all inside one allocation that is released
    // exactly once, after the first string has been copied out.
    unsafe {
        let first = if count >= 1 { *arguments } else { null_mut() };
        let target = (!first.is_null()).then(|| {
            let mut length = 0;
            while *first.add(length) != 0 {
                length += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(first, length))
        });
        LocalFree(arguments.cast());
        target
    }
}

/// Replaces `%NAME%` references with the values of environment variables.
fn expand_environment(text: &str) -> Option<String> {
    let source = wide(text);
    let mut buffer = vec![0u16; EXPANSION_UNITS];
    // The second pass uses the size the first one asked for; a third would
    // only be needed if the environment grew in between.
    for _ in 0..3 {
        let capacity = u32::try_from(buffer.len()).ok()?;
        // SAFETY: `source` is NUL-terminated and `buffer` is valid for
        // `capacity` UTF-16 units.
        let needed =
            unsafe { ExpandEnvironmentStringsW(source.as_ptr(), buffer.as_mut_ptr(), capacity) }
                as usize;
        if needed == 0 {
            return None;
        }
        if needed <= buffer.len() {
            return Some(from_wide(&buffer));
        }
        buffer.resize(needed, 0);
    }
    None
}

/// True for paths that start at a drive or a root, such as `C:\a` and `\a`.
fn is_rooted(path: &str) -> bool {
    let mut characters = path.chars();
    matches!(
        (characters.next(), characters.next()),
        (Some('\\' | '/'), _) | (Some(_), Some(':'))
    )
}

/// Compares two paths the way Windows compares file names: by UTF-16 code
/// unit, ignoring case.
fn same_path(left: &Path, right: &Path) -> bool {
    let left: Vec<u16> = left.as_os_str().encode_wide().collect();
    let right: Vec<u16> = right.as_os_str().encode_wide().collect();
    let (Ok(left_length), Ok(right_length)) =
        (i32::try_from(left.len()), i32::try_from(right.len()))
    else {
        return false;
    };
    // SAFETY: each pointer is valid for the length passed with it.
    let result = unsafe {
        CompareStringOrdinal(left.as_ptr(), left_length, right.as_ptr(), right_length, 1)
    };
    result == CSTR_EQUAL
}

/// An open registry key under `HKEY_CURRENT_USER`.
struct Key(HKEY);

impl Key {
    /// Opens an existing key for reading.
    fn open(path: &str) -> Option<Self> {
        let path = wide(path);
        let mut handle: HKEY = null_mut();
        // SAFETY: `path` is NUL-terminated and `handle` receives the new key.
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                KEY_QUERY_VALUE,
                &mut handle,
            )
        };
        (status == ERROR_SUCCESS).then_some(Self(handle))
    }

    /// Opens a key for writing, creating it if it does not exist.
    fn create(path: &str) -> Option<Self> {
        let path = wide(path);
        let mut handle: HKEY = null_mut();
        // SAFETY: `path` is NUL-terminated, `handle` receives the new key and
        // every optional argument is null.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                null(),
                &mut handle,
                null_mut(),
            )
        };
        (status == ERROR_SUCCESS).then_some(Self(handle))
    }

    /// Reads a text value, expanding it first if it is stored as expandable.
    fn read_string(&self, name: &str) -> Option<String> {
        let name = wide(name);
        let is_text = |kind: u32| kind == REG_SZ || kind == REG_EXPAND_SZ;
        let mut kind = 0u32;
        let mut size = 0u32;
        // SAFETY: a null data pointer asks only for the value's type and size.
        let status = unsafe {
            RegQueryValueExW(
                self.0,
                name.as_ptr(),
                null(),
                &mut kind,
                null_mut(),
                &mut size,
            )
        };
        if status != ERROR_SUCCESS || !is_text(kind) {
            return None;
        }

        let mut buffer = vec![0u16; (size as usize).div_ceil(size_of::<u16>())];
        let mut size = u32::try_from(size_of_val(buffer.as_slice())).ok()?;
        // SAFETY: `buffer` is valid for `size` bytes, which is what the call
        // is told it may write.
        let status = unsafe {
            RegQueryValueExW(
                self.0,
                name.as_ptr(),
                null(),
                &mut kind,
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if status != ERROR_SUCCESS || !is_text(kind) {
            return None;
        }
        // Stored text is not required to end with a terminator.
        buffer.truncate(size as usize / size_of::<u16>());
        let text = from_wide(&buffer);
        if kind == REG_EXPAND_SZ {
            expand_environment(&text)
        } else {
            Some(text)
        }
    }

    fn write_string(&self, name: &str, text: &OsStr) -> bool {
        let name = wide(name);
        let data: Vec<u16> = text.encode_wide().chain(std::iter::once(0)).collect();
        let Ok(size) = u32::try_from(size_of_val(data.as_slice())) else {
            return false;
        };
        // SAFETY: `data` is valid for `size` bytes and includes its terminator.
        let status =
            unsafe { RegSetValueExW(self.0, name.as_ptr(), 0, REG_SZ, data.as_ptr().cast(), size) };
        status == ERROR_SUCCESS
    }

    /// Deletes a value. A value that is already gone counts as deleted.
    fn delete_value(&self, name: &str) -> bool {
        let name = wide(name);
        // SAFETY: `name` is NUL-terminated and the key is open.
        let status = unsafe { RegDeleteValueW(self.0, name.as_ptr()) };
        status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle came from the registry and is closed exactly once.
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use windows_sys::Win32::System::Registry::RegDeleteKeyW;

    use super::*;

    const EXECUTABLE: &str = r"C:\Apps\Usage Tray\usage-tray-rs.exe";
    const TEST_KEY_ROOT: &str = r"Software\UsageTrayRsTests";

    /// A registry key that exists only for one test. Nothing here touches the
    /// real `Run` key.
    struct ThrowawayKey(String);

    impl ThrowawayKey {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let unique = NEXT.fetch_add(1, Ordering::Relaxed);
            Self(format!("{TEST_KEY_ROOT}\\{}-{unique}", std::process::id()))
        }
    }

    impl Drop for ThrowawayKey {
        fn drop(&mut self) {
            // The shared parent goes too once no other test's key is left in it.
            for path in [self.0.as_str(), TEST_KEY_ROOT] {
                let path = wide(path);
                // SAFETY: `path` is NUL-terminated; a key that still has
                // subkeys is left alone by this call.
                unsafe {
                    RegDeleteKeyW(HKEY_CURRENT_USER, path.as_ptr());
                }
            }
        }
    }

    fn targets(command: &str) -> bool {
        targets_executable(command, Path::new(EXECUTABLE))
    }

    #[test]
    fn this_build_has_its_own_entry_in_the_run_key() {
        assert_eq!(
            RUN_KEY_PATH,
            r"Software\Microsoft\Windows\CurrentVersion\Run"
        );
        assert_eq!(VALUE_NAME, "CodexUsageTrayRs");
    }

    #[test]
    fn the_stored_command_is_the_quoted_executable_path() {
        assert_eq!(
            quote(Path::new(EXECUTABLE)),
            OsString::from(r#""C:\Apps\Usage Tray\usage-tray-rs.exe""#)
        );
    }

    #[test]
    fn the_target_is_the_first_argument_of_a_command() {
        assert_eq!(
            executable_target(r#""C:\Apps\Usage Tray\usage-tray-rs.exe" --show-usage"#).as_deref(),
            Some(EXECUTABLE)
        );
        assert_eq!(
            executable_target(r"C:\Apps\tray.exe --flag").as_deref(),
            Some(r"C:\Apps\tray.exe")
        );
        assert_eq!(
            executable_target("  \"C:\\Apps\\tray.exe\"  ").as_deref(),
            Some(r"C:\Apps\tray.exe")
        );
        assert_eq!(executable_target(""), None);
        assert_eq!(executable_target("   "), None);
    }

    #[test]
    fn a_command_for_this_executable_is_recognized_however_it_is_spelled() {
        assert!(targets(r#""C:\Apps\Usage Tray\usage-tray-rs.exe""#));
        assert!(targets(
            r#""C:\Apps\Usage Tray\usage-tray-rs.exe" --show-usage"#
        ));
        assert!(targets(r#""c:\apps\USAGE TRAY\Usage-Tray-RS.EXE""#));
        assert!(targets(r#""C:/Apps/Usage Tray/usage-tray-rs.exe""#));
        assert!(targets(
            r#""C:\Apps\Other\..\Usage Tray\.\usage-tray-rs.exe""#
        ));
    }

    #[test]
    fn a_command_for_anything_else_is_not_this_executable() {
        assert!(!targets(""));
        assert!(!targets("\"\""));
        assert!(!targets(r#""C:\Apps\Usage Tray\CodexUsageTray.exe""#));
        assert!(!targets(r#""D:\Apps\Usage Tray\usage-tray-rs.exe""#));
        assert!(!targets("usage-tray-rs.exe"));
        assert!(!targets(r#""Usage Tray\usage-tray-rs.exe""#));
        // Without quotes the command line ends the program at the first space.
        assert!(!targets(r"C:\Apps\Usage Tray\usage-tray-rs.exe"));
    }

    #[test]
    fn environment_variables_in_a_command_are_expanded() {
        let windows = std::env::var("SystemRoot").unwrap();
        let executable = Path::new(&windows).join("usage-tray-rs.exe");
        assert!(targets_executable(
            r#""%SystemRoot%\usage-tray-rs.exe""#,
            &executable
        ));
        assert!(!targets_executable(
            r#""%UsageTrayRsUndefinedVariable%\usage-tray-rs.exe""#,
            &executable
        ));
        assert_eq!(
            expand_environment("plain text").as_deref(),
            Some("plain text")
        );
    }

    #[test]
    fn rooted_paths_start_at_a_drive_or_a_root() {
        assert!(is_rooted(r"C:\Apps\tray.exe"));
        assert!(is_rooted(r"\\server\share\tray.exe"));
        assert!(is_rooted(r"\Apps\tray.exe"));
        assert!(is_rooted("/Apps/tray.exe"));
        assert!(is_rooted("C:tray.exe"));
        assert!(!is_rooted(r"Apps\tray.exe"));
        assert!(!is_rooted("tray.exe"));
        assert!(!is_rooted(""));
    }

    #[test]
    fn the_entry_round_trips_through_the_registry() {
        let key = ThrowawayKey::new();
        let executable = Path::new(EXECUTABLE);
        let elsewhere = Path::new(r"C:\Elsewhere\usage-tray-rs.exe");
        assert!(!is_enabled_in(&key.0, executable));

        assert_eq!(set_enabled_in(&key.0, executable, true), Ok(()));
        assert!(is_enabled_in(&key.0, executable));
        assert!(!is_enabled_in(&key.0, elsewhere));
        assert_eq!(
            Key::open(&key.0).unwrap().read_string(VALUE_NAME).unwrap(),
            r#""C:\Apps\Usage Tray\usage-tray-rs.exe""#
        );

        // Enabling from another folder repoints the entry.
        assert_eq!(set_enabled_in(&key.0, elsewhere, true), Ok(()));
        assert!(is_enabled_in(&key.0, elsewhere));
        assert!(!is_enabled_in(&key.0, executable));

        assert_eq!(set_enabled_in(&key.0, elsewhere, false), Ok(()));
        assert!(!is_enabled_in(&key.0, elsewhere));
        assert_eq!(Key::open(&key.0).unwrap().read_string(VALUE_NAME), None);
        // Disabling again is not an error.
        assert_eq!(set_enabled_in(&key.0, elsewhere, false), Ok(()));
    }

    #[test]
    fn the_other_apps_entry_is_left_alone() {
        const OTHER_VALUE: &str = "CodexUsageTray";
        const OTHER_COMMAND: &str = r#""C:\Apps\Usage Tray\CodexUsageTray.exe""#;
        let key = ThrowawayKey::new();
        let executable = Path::new(EXECUTABLE);
        assert!(Key::create(&key.0)
            .unwrap()
            .write_string(OTHER_VALUE, OsStr::new(OTHER_COMMAND)));
        let other = || Key::open(&key.0).unwrap().read_string(OTHER_VALUE);

        // The other entry does not make this build look enabled.
        assert!(!is_enabled_in(&key.0, executable));
        assert_eq!(set_enabled_in(&key.0, executable, true), Ok(()));
        assert_eq!(other().as_deref(), Some(OTHER_COMMAND));
        assert_eq!(set_enabled_in(&key.0, executable, false), Ok(()));
        assert_eq!(other().as_deref(), Some(OTHER_COMMAND));
    }

    #[test]
    fn expandable_and_non_text_values_are_read_like_the_existing_app() {
        use windows_sys::Win32::System::Registry::REG_DWORD;

        let key = ThrowawayKey::new();
        let windows = std::env::var("SystemRoot").unwrap();
        let executable = Path::new(&windows).join("usage-tray-rs.exe");
        let name = wide(VALUE_NAME);
        let write = |kind: u32, data: &[u16]| {
            let writer = Key::create(&key.0).unwrap();
            // SAFETY: `data` is valid for the byte length passed with it.
            let status = unsafe {
                RegSetValueExW(
                    writer.0,
                    name.as_ptr(),
                    0,
                    kind,
                    data.as_ptr().cast(),
                    size_of_val(data) as u32,
                )
            };
            assert_eq!(status, ERROR_SUCCESS);
        };

        write(REG_EXPAND_SZ, &wide(r#""%SystemRoot%\usage-tray-rs.exe""#));
        assert!(is_enabled_in(&key.0, &executable));

        // Text stored without its terminator is still read in full.
        let quoted: Vec<u16> = quote(&executable).encode_wide().collect();
        write(REG_SZ, &quoted);
        assert!(is_enabled_in(&key.0, &executable));

        write(REG_DWORD, &[1, 0]);
        assert!(!is_enabled_in(&key.0, &executable));
    }
}
