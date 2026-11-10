//! File primitives shared by the Claude caches and the credential write-back:
//! bounded reads, uniquely named temporary siblings and in-place replacement.

use std::collections::hash_map::RandomState;
use std::fs::File;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Read};
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::FromRawHandle;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    LocalFree, ERROR_SUCCESS, GENERIC_WRITE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReplaceFileW, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_NONE,
    REPLACEFILE_WRITE_THROUGH,
};

/// Reads a whole file, failing instead of truncating when it is larger than
/// `max_bytes`.
pub(super) fn read_limited(path: &Path, max_bytes: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(bytes)
}

/// A uniquely named sibling of the file it is meant to replace. It is deleted
/// when dropped, so no path leaves a copy behind; after a successful swap the
/// name no longer exists and there is nothing to delete.
pub(super) struct TemporaryFile(PathBuf);

impl TemporaryFile {
    pub(super) fn beside(target: &Path) -> Self {
        let mut name = target.as_os_str().to_owned();
        name.push(format!(".{}.tmp", unique_suffix()));
        Self(PathBuf::from(name))
    }

    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// 128 random bits as hex, standing in for the GUID the C# app uses. The
/// standard library seeds every `RandomState` from the system's generator.
fn unique_suffix() -> String {
    let random = || RandomState::new().build_hasher().finish();
    format!("{:016x}{:016x}", random(), random())
}

/// Puts `replacement` in place of `replaced` in one step. The result keeps the
/// replaced file's access control list, attributes and creation time, and the
/// replacement's name disappears.
pub(super) fn replace_file(replaced: &Path, replacement: &Path) -> io::Result<()> {
    let replaced = wide_path(replaced)?;
    let replacement = wide_path(replacement)?;
    // SAFETY: both paths are null-terminated buffers that outlive the call;
    // the remaining pointers are optional and null.
    let succeeded = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            null(),
            REPLACEFILE_WRITE_THROUGH,
            null(),
            null(),
        )
    };
    if succeeded == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Creates `path` for writing, failing if it exists, with the same access
/// control list as `reference`. Whatever is written to it is therefore never
/// readable more widely than the file it is about to replace.
pub(super) fn create_like(reference: &Path, path: &Path) -> io::Result<File> {
    create_new(path, &access_control_of(reference)?)
}

/// A security descriptor allocated by the system.
struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        // SAFETY: the pointer came from a system call that allocates with
        // LocalAlloc, and it is freed exactly once.
        unsafe {
            LocalFree(self.0);
        }
    }
}

fn access_control_of(path: &Path) -> io::Result<SecurityDescriptor> {
    let path = wide_path(path)?;
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: the path is null-terminated and outlives the call; only the
    // descriptor is requested, through a pointer to a live local.
    let status = unsafe {
        GetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    Ok(SecurityDescriptor(descriptor))
}

fn create_new(path: &Path, descriptor: &SecurityDescriptor) -> io::Result<File> {
    let path = wide_path(path)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    // SAFETY: the path is null-terminated, and the attributes and the
    // descriptor they point to outlive the call.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_NONE,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the handle is valid, open for writing and owned by nothing else.
    Ok(unsafe { File::from_raw_handle(handle) })
}

/// The path as a null-terminated UTF-16 string. An embedded null is refused
/// because the system would silently act on the shortened path.
fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    wide.push(0);
    Ok(wide)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use std::fs;

    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW,
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };

    use crate::win::wide;

    /// Not inherited, so it differs from whatever the temporary directory
    /// grants. Authenticated users keep full access so the tests can use it.
    pub(crate) const PROTECTED_DACL: &str = "D:P(A;;FA;;;AU)(A;;FR;;;BU)";

    /// A uniquely named directory under the system's temporary directory that
    /// is removed, with its contents, when dropped.
    pub(crate) struct TestDirectory(PathBuf);

    impl TestDirectory {
        pub(crate) fn new(prefix: &str) -> Self {
            let path = std::env::temp_dir().join(format!("{prefix}-{}", unique_suffix()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        pub(crate) fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }

        /// Temporary files that a write left behind.
        pub(crate) fn leftovers(&self) -> Vec<String> {
            fs::read_dir(&self.0)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|name| name.ends_with(".tmp"))
                .collect()
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The file's access control list in SDDL form: whether it is protected
    /// from inheritance, and its entries. The `AI` flag is left out because
    /// it grants nothing; it only records that an inheritance-aware call,
    /// such as the one inside `ReplaceFileW`, last wrote the list.
    pub(crate) fn dacl_of(path: &Path) -> String {
        let sddl = raw_dacl_of(path);
        let entries = sddl.find('(').unwrap_or(sddl.len());
        format!("{}{}", sddl[..entries].replace("AI", ""), &sddl[entries..])
    }

    fn raw_dacl_of(path: &Path) -> String {
        let descriptor = access_control_of(path).unwrap();
        let mut text = null_mut();
        // SAFETY: the descriptor is valid, and the text pointer receives a
        // null-terminated string allocated with LocalAlloc that is copied and
        // then freed exactly once.
        unsafe {
            assert_ne!(
                ConvertSecurityDescriptorToStringSecurityDescriptorW(
                    descriptor.0,
                    SDDL_REVISION_1,
                    DACL_SECURITY_INFORMATION,
                    &mut text,
                    null_mut(),
                ),
                0
            );
            let length = (0..).take_while(|&index| *text.add(index) != 0).count();
            let sddl = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
            LocalFree(text.cast());
            sddl
        }
    }

    /// Creates an empty file whose access control list is `sddl`.
    pub(crate) fn create_with_dacl(path: &Path, sddl: &str) {
        let sddl = wide(sddl);
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: the SDDL text is null-terminated and the descriptor pointer
        // refers to a live local.
        let converted = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            )
        };
        assert_ne!(converted, 0);
        drop(create_new(path, &SecurityDescriptor(descriptor)).unwrap());
    }

    #[test]
    fn reads_fail_instead_of_truncating() {
        let directory = TestDirectory::new("ClaudeFileTests");
        let path = directory.file("data.json");
        fs::write(&path, "12345").unwrap();
        assert_eq!(read_limited(&path, 5).unwrap(), b"12345");
        assert!(read_limited(&path, 4).is_err());
        assert!(read_limited(&directory.file("missing.json"), 5).is_err());
    }

    #[test]
    fn temporary_files_are_unique_siblings_that_clean_up_after_themselves() {
        let directory = TestDirectory::new("ClaudeFileTests");
        let target = directory.file("data.json");
        let first = TemporaryFile::beside(&target);
        let second = TemporaryFile::beside(&target);
        assert_ne!(first.path(), second.path());
        assert_eq!(first.path().parent(), target.parent());

        fs::write(first.path(), "{}").unwrap();
        assert_eq!(directory.leftovers().len(), 1);
        drop(first);
        drop(second);
        assert!(directory.leftovers().is_empty());
    }

    #[test]
    fn files_created_like_a_reference_carry_its_access_control_list() {
        let directory = TestDirectory::new("ClaudeFileTests");
        let protected = directory.file("protected.json");
        create_with_dacl(&protected, PROTECTED_DACL);
        let inherited = directory.file("inherited.json");
        fs::write(&inherited, "{}").unwrap();
        assert_ne!(dacl_of(&protected), dacl_of(&inherited));

        for reference in [&protected, &inherited] {
            let copy = TemporaryFile::beside(reference);
            drop(create_like(reference, copy.path()).unwrap());
            assert_eq!(dacl_of(copy.path()), dacl_of(reference));
            assert!(create_like(reference, copy.path()).is_err());
        }
    }

    #[test]
    fn replacing_keeps_the_replaced_access_control_list() {
        let directory = TestDirectory::new("ClaudeFileTests");
        let target = directory.file("data.json");
        create_with_dacl(&target, PROTECTED_DACL);
        fs::write(&target, "old").unwrap();
        let before = dacl_of(&target);
        assert!(before.starts_with("D:P"));

        // Created the ordinary way, so it starts with the directory's list.
        let replacement = TemporaryFile::beside(&target);
        fs::write(replacement.path(), "new").unwrap();
        assert_ne!(dacl_of(replacement.path()), before);

        replace_file(&target, replacement.path()).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
        assert_eq!(dacl_of(&target), before);
        assert!(directory.leftovers().is_empty());
    }

    #[test]
    fn paths_with_an_embedded_null_are_refused() {
        assert!(wide_path(Path::new("a\0b")).is_err());
        assert_eq!(wide_path(Path::new("ab")).unwrap(), [97, 98, 0]);
    }
}
