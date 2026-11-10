//! Self-update from GitHub releases, done by the app itself.
//!
//! [`check`] reads the latest release, [`stage`] downloads and verifies its
//! `usage-tray-rs.exe` asset beside the installed executable, and
//! [`apply_and_restart`] swaps the two files and hands over to the new
//! version. [`Session`] holds what the tray needs to drive these from its
//! timer, worker thread and Settings window. Network, processes and waiting
//! go through [`Host`], so tests replace all of them.
//!
//! # Files
//!
//! Update files sit beside the installed executable. `<exe>` is its file
//! name and `<id>` is 32 hex digits chosen for one update:
//!
//! - `.<exe>.update-<id>.tmp`: the download, verified before anything moves.
//! - `.<exe>.update-<id>.old`: the previous executable, kept until the new
//!   one has shown that it runs.
//! - `.<exe>.update-<id>.failed`: a new executable that was rolled back, on
//!   its way to being deleted.
//! - `.<exe>.update-<id>.ok`: created by the new process to confirm that it
//!   is running.
//! - `.<exe>.update-write-<id>.target`, `.replacement` and `.backup`: one-byte
//!   files that exist only while staging checks that the folder allows the
//!   swap.
//!
//! # Hand-over
//!
//! 1. The old process closes its tray and releases the single-instance lock,
//!    then calls [`apply_and_restart`]. That verifies `.tmp` again, renames
//!    `<exe>` to `.old` and `.tmp` to `<exe>` (a running executable can be
//!    renamed but not deleted), and starts `<exe> --update-confirm <id>`.
//! 2. In the new process [`startup`] returns [`Startup::Updated`]. Once its
//!    tray is up it calls [`Confirmation::confirm`], which creates `.ok` and
//!    then deletes `.old` and `.ok`, retrying every 250 ms for up to 10 s
//!    until the old process has exited.
//! 3. The old process looks for `.ok` every 50 ms for up to 15 s and exits
//!    as soon as it appears.
//! 4. If the new executable cannot be started, exits before confirming or
//!    does not confirm in time, the old process stops it (allowing 5 s),
//!    renames `<exe>` to `.failed` and `.old` back to `<exe>`, deletes
//!    `.failed`, and starts `<exe> --update-failed <reason>`. The reason is
//!    `staged`, `replace`, `start`, `exited` or `timeout`; the first two
//!    mean nothing was replaced. It watches that process for 5 s before
//!    exiting. There [`startup`] returns [`Startup::UpdateFailed`], which
//!    pauses automatic checks for that run.
//!
//! Renames are retried, 20 times 250 ms apart going forward and 20 times
//! 500 ms apart when rolling back. Leftovers of an interrupted update are
//! deleted the next time an update is staged.
//!
//! # Differences from the C# updater
//!
//! The C# app exits and leaves the swap to a PowerShell script, which
//! treats a new process that survives 5 s as started. Here the old process
//! does the swap and waits for an explicit confirmation, so no script, no
//! temporary folder and no search for other processes by path is involved.
//! Only `usage-tray-rs.exe` is accepted as an asset, by its exact name. The
//! HTTP client reports neither timeouts nor response headers, so a timeout
//! reads as an unreachable server and a wrong length is caught after the
//! download rather than before it.

use std::cmp::Ordering;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::ptr::{null, null_mut};
use std::thread;
use std::time::Duration;

use serde_json::{Map, Value};
use windows_sys::Win32::Security::Cryptography::{
    BCryptCreateHash, BCryptDestroyHash, BCryptFinishHash, BCryptGenRandom, BCryptHashData,
    BCRYPT_HASH_HANDLE, BCRYPT_SHA256_ALG_HANDLE, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
};

use crate::http::{self, Request};
use crate::version::{self, RELEASE_ASSET_NAME};

/// Tells a new process that it was started to take over after an update.
/// The update's id follows it.
const CONFIRM_ARGUMENT: &str = "--update-confirm";
/// Tells a restarted process that an update was rolled back. A reason code
/// may follow it.
const FAILED_ARGUMENT: &str = "--update-failed";

/// Seconds between automatic checks.
pub const CHECK_INTERVAL_SECONDS: i64 = 3_600;

/// Title for update notifications.
pub const NOTIFICATION_TITLE: &str = "Codex Usage Updates";
/// Shown when an update found in the background could not be prepared.
pub const STAGE_FAILED_NOTIFICATION: &str =
    "Update installation failed. Open Settings for details.";
/// Shown by the restored app after an update was rolled back.
pub const ROLLED_BACK_NOTIFICATION: &str =
    "Update could not be installed. The previous app remains available.";
/// For [`ApplyOutcome::RecoveryFailed`], when no app is left running to say it.
pub const RECOVERY_FAILED_NOTIFICATION: &str =
    "Update failed and the previous app could not be restored. Manual installation may be required.";

const USER_AGENT: &str = "Codex-Usage-Tray";
const GITHUB_TIMEOUT_MILLISECONDS: i32 = 30_000;
const DOWNLOAD_TIMEOUT_MILLISECONDS: i32 = 60_000;
const MAX_RELEASE_ASSET_BYTES: u64 = 100 * 1024 * 1024;
const MAX_RELEASE_JSON_BYTES: usize = 1024 * 1024;

const POLL_MILLISECONDS: u32 = 50;
const CONFIRMATION_TIMEOUT_MILLISECONDS: u32 = 15_000;
const STARTUP_VERIFICATION_MILLISECONDS: u32 = 5_000;
const STOP_TIMEOUT_MILLISECONDS: u32 = 5_000;
const REPLACE_ATTEMPTS: u32 = 20;
const REPLACE_RETRY_MILLISECONDS: u32 = 250;
const ROLLBACK_ATTEMPTS: u32 = 20;
const ROLLBACK_RETRY_MILLISECONDS: u32 = 500;
const CLEANUP_ATTEMPTS: u32 = 40;
const CLEANUP_RETRY_MILLISECONDS: u32 = 250;

const HASH_CHUNK_BYTES: usize = 65_536;
const SHA256_BYTES: usize = 32;
const SHA256_HEX_LENGTH: usize = 64;
const UPDATE_ID_BYTES: usize = 16;
const UPDATE_ID_LENGTH: usize = 32;
const DOS_HEADER_BYTES: usize = 64;
const PE_OFFSET_FIELD: usize = 0x3c;
const UPDATE_FILE_SUFFIXES: [&str; 4] = ["tmp", "old", "failed", "ok"];
const PROBE_FILE_SUFFIXES: [&str; 3] = ["target", "replacement", "backup"];

/// What the latest release means for this copy of the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateInfo {
    pub current_version: String,
    /// Empty when the repository has no published release.
    pub latest_version: String,
    pub update_available: bool,
    /// True when [`stage`] can be expected to accept this update.
    pub can_install: bool,
    pub release_url: String,
    pub repository_url: String,
    pub asset_name: String,
    pub asset_url: String,
    pub asset_size: u64,
    /// Lower-case hex, or empty when the release publishes no valid digest.
    pub asset_sha256: String,
    /// A sentence describing the result to the user.
    pub message: String,
}

/// Why an update could not be found or prepared. No app file has been
/// replaced when one of these is reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateError {
    GitHubUnreachable,
    InvalidReleaseData,
    NoUpdate,
    NoAsset,
    UnexpectedAssetName,
    MissingIntegrityMetadata,
    InvalidDownloadAddress,
    InstallUnsupported,
    FolderNotReplaceable,
    PreflightCleanupFailed,
    PreparationFailed,
    DownloadFailed,
    SizeMismatch,
    DigestMismatch,
    NotExecutable,
}

impl UpdateError {
    /// A description suitable for showing to the user.
    pub fn message(self) -> &'static str {
        match self {
            Self::GitHubUnreachable => {
                "Could not check GitHub for updates. Check your connection and try again."
            }
            Self::InvalidReleaseData => {
                "GitHub returned update information that could not be read safely. Try again later."
            }
            Self::NoUpdate => "No update is available.",
            Self::NoAsset => {
                "The latest release does not include a compatible Windows update asset."
            }
            Self::UnexpectedAssetName => {
                "The release asset name is not valid for automatic installation."
            }
            Self::MissingIntegrityMetadata => {
                "The release does not include valid integrity metadata. Open the release page to install it manually."
            }
            Self::InvalidDownloadAddress => {
                "The release download address is not valid for automatic installation."
            }
            Self::InstallUnsupported => {
                "Self-update is only available when the app runs from a writable .exe file."
            }
            Self::FolderNotReplaceable => {
                "The app folder does not support safe automatic replacement. Install the release manually instead."
            }
            Self::PreflightCleanupFailed => {
                "The app folder could not clean up an update preflight file. Automatic installation was stopped."
            }
            Self::PreparationFailed => {
                "The update could not be prepared safely. No app files were replaced."
            }
            Self::DownloadFailed => {
                "The update could not be downloaded securely. Check your connection and try again."
            }
            Self::SizeMismatch => {
                "The update file size failed verification. No files were replaced."
            }
            Self::DigestMismatch => {
                "The update failed SHA-256 verification. No files were replaced."
            }
            Self::NotExecutable => {
                "The verified release asset is not a Windows executable. No files were replaced."
            }
        }
    }
}

/// Why an installation was rolled back. Travels to the restarted app as the
/// code after `--update-failed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallFailure {
    /// The download no longer matched what was verified when it was staged.
    StagedFileChanged,
    /// The executable could not be moved aside or the new one into place.
    ReplaceFailed,
    /// The new executable could not be started.
    StartFailed,
    /// The new process exited before confirming.
    ExitedEarly,
    /// The new process did not confirm in time.
    TimedOut,
    /// `--update-failed` came without a reason this version knows.
    Unknown,
}

impl InstallFailure {
    fn code(self) -> &'static str {
        match self {
            Self::StagedFileChanged => "staged",
            Self::ReplaceFailed => "replace",
            Self::StartFailed => "start",
            Self::ExitedEarly => "exited",
            Self::TimedOut => "timeout",
            Self::Unknown => "unknown",
        }
    }

    fn from_code(code: &str) -> Self {
        [
            Self::StagedFileChanged,
            Self::ReplaceFailed,
            Self::StartFailed,
            Self::ExitedEarly,
            Self::TimedOut,
        ]
        .into_iter()
        .find(|failure| failure.code() == code)
        .unwrap_or(Self::Unknown)
    }

    /// A description suitable for showing to the user.
    pub fn message(self) -> &'static str {
        match self {
            Self::StagedFileChanged => {
                "The staged update file failed verification. No files were replaced."
            }
            Self::ReplaceFailed => "The app executable could not be replaced.",
            Self::StartFailed => "The updated app could not be started.",
            Self::ExitedEarly => "The updated app exited during startup.",
            Self::TimedOut => "The updated app did not finish starting.",
            Self::Unknown => "The update could not be installed.",
        }
    }
}

/// What asking GitHub for the latest release produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReleaseResponse {
    /// The JSON body of a successful response.
    Found(Vec<u8>),
    /// The repository has no published release.
    NotFound,
    /// Any other status, or no response at all.
    Unreachable,
}

/// A process started through [`Host::start`].
pub trait Process {
    /// True once the process is no longer running.
    fn has_exited(&mut self) -> bool;
    /// Stops the process. False if it may still be running afterwards.
    fn terminate(&mut self) -> bool;
}

/// Everything the updater needs from outside its own logic.
pub trait Host {
    /// Asks GitHub for the repository's latest release.
    fn latest_release(&self) -> ReleaseResponse;
    /// Downloads `url` to `destination`. True only when the complete body,
    /// of at most `size` bytes, was written.
    fn download(&self, url: &str, destination: &Path, size: u64) -> bool;
    /// A fresh, unpredictable id of 32 hex digits.
    fn update_id(&self) -> Option<String>;
    /// Starts `executable` with `arguments`, with its folder as the working
    /// directory. `None` if it could not be started.
    fn start(&self, executable: &Path, arguments: &[&str]) -> Option<Box<dyn Process>>;
    /// Waits. All of the updater's waiting goes through here, so its
    /// timeouts are counted in sleeps rather than read from a clock.
    fn sleep(&self, milliseconds: u32);
}

/// The real network, random numbers, processes and clock.
pub struct SystemHost;

impl Host for SystemHost {
    fn latest_release(&self) -> ReleaseResponse {
        let url = format!("{}/releases/latest", version::api_url());
        let headers = [
            ("Accept", "application/vnd.github+json"),
            ("X-GitHub-Api-Version", "2022-11-28"),
        ];
        let request = Request {
            method: "GET",
            url: &url,
            headers: &headers,
            body: None,
            user_agent: Some(USER_AGENT),
            // A renamed repository answers with a redirect, and the request
            // carries no credential that could leak to another host.
            follow_redirects: true,
            max_bytes: MAX_RELEASE_JSON_BYTES,
            timeout_milliseconds: GITHUB_TIMEOUT_MILLISECONDS,
        };
        match http::send(&request) {
            Some(response) if response.status == 200 => ReleaseResponse::Found(response.body),
            Some(response) if response.status == 404 => ReleaseResponse::NotFound,
            _ => ReleaseResponse::Unreachable,
        }
    }

    fn download(&self, url: &str, destination: &Path, size: u64) -> bool {
        let headers = [("Accept", "application/octet-stream")];
        let request = Request {
            method: "GET",
            url,
            headers: &headers,
            body: None,
            user_agent: Some(USER_AGENT),
            // GitHub serves release assets from a storage host it redirects to.
            follow_redirects: true,
            max_bytes: usize::try_from(size).unwrap_or(usize::MAX),
            timeout_milliseconds: DOWNLOAD_TIMEOUT_MILLISECONDS,
        };
        http::download(&request, destination) == Some(200)
    }

    fn update_id(&self) -> Option<String> {
        let mut bytes = [0u8; UPDATE_ID_BYTES];
        // SAFETY: `bytes` is valid for the length passed; the flag selects the
        // system generator, so no algorithm handle is needed.
        let status = unsafe {
            BCryptGenRandom(
                null_mut(),
                bytes.as_mut_ptr(),
                bytes.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        (status >= 0).then(|| hex(&bytes))
    }

    fn start(&self, executable: &Path, arguments: &[&str]) -> Option<Box<dyn Process>> {
        let mut command = Command::new(executable);
        command.args(arguments);
        if let Some(directory) = executable.parent() {
            command.current_dir(directory);
        }
        let child = command.spawn().ok()?;
        Some(Box::new(ChildProcess(child)))
    }

    fn sleep(&self, milliseconds: u32) {
        thread::sleep(Duration::from_millis(u64::from(milliseconds)));
    }
}

/// Dropping it leaves the process running.
struct ChildProcess(Child);

impl Process for ChildProcess {
    fn has_exited(&mut self) -> bool {
        !matches!(self.0.try_wait(), Ok(None))
    }

    fn terminate(&mut self) -> bool {
        let _ = self.0.kill();
        // A killed process can take a moment to disappear.
        for _ in 0..STOP_TIMEOUT_MILLISECONDS / POLL_MILLISECONDS {
            if self.has_exited() {
                return true;
            }
            thread::sleep(Duration::from_millis(u64::from(POLL_MILLISECONDS)));
        }
        self.has_exited()
    }
}

/// The copy of the app that updates apply to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installation {
    /// Full path of the installed executable.
    pub executable: PathBuf,
    /// The version that executable reports.
    pub version: String,
}

impl Installation {
    /// The running executable and its version.
    pub fn current() -> Option<Self> {
        Some(Self {
            executable: std::env::current_exe().ok()?,
            version: version::current(),
        })
    }

    /// True when this copy can replace itself: a regular, writable `.exe`
    /// file reached by its full path.
    pub fn can_install_updates(&self) -> bool {
        let executable = &self.executable;
        let is_exe = executable
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"));
        let writable_file = executable
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.is_file() && !metadata.permissions().readonly());
        executable.is_absolute()
            && is_exe
            && writable_file
            && executable.parent().is_some_and(Path::is_dir)
            && executable
                .file_name()
                .and_then(|name| name.to_str())
                .is_some()
    }
}

/// Reads the latest release and decides whether it is an update that can be
/// installed. Blocks on the network; run it on a worker thread.
pub fn check(installation: &Installation, host: &dyn Host) -> Result<UpdateInfo, UpdateError> {
    let body = match host.latest_release() {
        ReleaseResponse::Found(body) => body,
        ReleaseResponse::NotFound => return Ok(no_published_release(installation)),
        ReleaseResponse::Unreachable => return Err(UpdateError::GitHubUnreachable),
    };
    let parsed: Value =
        serde_json::from_slice(&body).map_err(|_| UpdateError::InvalidReleaseData)?;
    Ok(match parsed.as_object() {
        Some(release) => describe_release(release, installation),
        None => no_published_release(installation),
    })
}

fn no_published_release(installation: &Installation) -> UpdateInfo {
    UpdateInfo {
        current_version: version::display(&installation.version),
        latest_version: String::new(),
        update_available: false,
        can_install: false,
        release_url: version::repository_url(),
        repository_url: version::repository_url(),
        asset_name: String::new(),
        asset_url: String::new(),
        asset_size: 0,
        asset_sha256: String::new(),
        message: "No published releases were found.".to_string(),
    }
}

fn describe_release(release: &Map<String, Value>, installation: &Installation) -> UpdateInfo {
    let current_version = version::display(&installation.version);
    let latest_version = version::display(&text(release, "tag_name"));
    let release_url = Some(text(release, "html_url"))
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(version::repository_url);

    let asset = release_asset(release);
    let asset_name = asset.map(|asset| text(asset, "name")).unwrap_or_default();
    let asset_url = asset
        .map(|asset| text(asset, "browser_download_url"))
        .unwrap_or_default();
    let asset_size = asset.map_or(0, |asset| whole_number(asset, "size"));
    let asset_sha256 = asset
        .and_then(|asset| parse_sha256_digest(&text(asset, "digest")))
        .unwrap_or_default();

    let update_available = version::compare(&latest_version, &current_version) == Ordering::Greater;
    let has_asset = !asset_url.trim().is_empty();
    let has_valid_address = has_asset && release_asset_url(&asset_url).is_some();
    let has_integrity_metadata = is_valid_asset_size(asset_size) && !asset_sha256.is_empty();
    let install_supported = installation.can_install_updates();

    let message = if !update_available {
        format!("You are on the latest published version ({latest_version}).")
    } else if !has_asset {
        format!(
            "Version {latest_version} does not include {RELEASE_ASSET_NAME}. No compatible update is available yet."
        )
    } else if !has_valid_address {
        format!(
            "Update available: {latest_version}, but its download address is not valid for automatic installation."
        )
    } else if !has_integrity_metadata {
        format!(
            "Update available: {latest_version}, but automatic installation is unavailable because the release has no valid size and SHA-256 metadata."
        )
    } else if !install_supported {
        format!(
            "Update available: {latest_version}. Automatic installation is not available in this copy."
        )
    } else {
        format!("Update available: {latest_version}.")
    };

    UpdateInfo {
        current_version,
        latest_version,
        update_available,
        can_install: update_available
            && has_valid_address
            && has_integrity_metadata
            && install_supported,
        release_url,
        repository_url: version::repository_url(),
        asset_name,
        asset_url,
        asset_size,
        asset_sha256,
        message,
    }
}

/// The asset this build installs, wherever it is in the release's list.
fn release_asset(release: &Map<String, Value>) -> Option<&Map<String, Value>> {
    release
        .get("assets")?
        .as_array()?
        .iter()
        .filter_map(Value::as_object)
        .find(|asset| is_expected_release_asset(&text(asset, "name")))
}

fn is_expected_release_asset(name: &str) -> bool {
    name.eq_ignore_ascii_case(RELEASE_ASSET_NAME)
}

/// A member as text; anything that is not text or a number reads as empty.
fn text(values: &Map<String, Value>, key: &str) -> String {
    match values.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}

/// A member as a whole, non-negative number; anything else reads as zero.
fn whole_number(values: &Map<String, Value>, key: &str) -> u64 {
    let whole = |number: f64| {
        let in_range = (0.0..i64::MAX as f64).contains(&number);
        (number.fract() == 0.0 && in_range).then_some(number as u64)
    };
    let number = match values.get(key) {
        Some(Value::Number(number)) => number.as_u64().or_else(|| number.as_f64().and_then(whole)),
        Some(Value::String(text)) => text.trim().parse().ok(),
        _ => None,
    };
    number
        .filter(|number| i64::try_from(*number).is_ok())
        .unwrap_or(0)
}

fn is_valid_asset_size(size: u64) -> bool {
    size > 0 && size <= MAX_RELEASE_ASSET_BYTES
}

/// The hash in a release asset's `digest`, such as `sha256:ab12...`, in
/// lower case.
fn parse_sha256_digest(digest: &str) -> Option<String> {
    const PREFIX: &str = "sha256:";
    let digest = digest.trim();
    let prefix = digest.get(..PREFIX.len())?;
    let hash = &digest[PREFIX.len()..];
    (prefix.eq_ignore_ascii_case(PREFIX) && is_sha256_hex(hash)).then(|| hash.to_ascii_lowercase())
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == SHA256_HEX_LENGTH && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Accepts only HTTPS downloads of this repository's release assets from
/// github.com, and returns the address in the form the HTTP client expects.
fn release_asset_url(value: &str) -> Option<String> {
    const SCHEME: &str = "https://";
    const HOST: &str = "github.com";
    let value = value.trim();
    let scheme = value.get(..SCHEME.len())?;
    let rest = &value[SCHEME.len()..];
    let authority_end = rest.find(['/', '?', '#', '\\']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(authority_end);
    let host = authority.strip_suffix(":443").unwrap_or(authority);

    let prefix = format!(
        "/{}/{}/releases/download/",
        version::GITHUB_OWNER,
        version::GITHUB_REPO
    );
    let in_repository = path
        .get(..prefix.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(&prefix));
    // A URL parser would resolve these before the checks above could see
    // where they lead, so they are refused outright.
    let route = path.split('?').next().unwrap_or_default();
    let has_dot_segment = route.split('/').any(|segment| {
        let segment = segment.to_ascii_lowercase().replace("%2e", ".");
        segment == "." || segment == ".."
    });
    let plain = !path.contains(['#', '\\'])
        && !path
            .chars()
            .any(|character| character.is_control() || character.is_whitespace());

    (scheme.eq_ignore_ascii_case(SCHEME)
        && host.eq_ignore_ascii_case(HOST)
        && in_repository
        && !has_dot_segment
        && plain)
        .then(|| format!("{SCHEME}{HOST}{path}"))
}

/// The files one update uses. See the module documentation.
#[derive(Clone, Debug, PartialEq, Eq)]
struct UpdateFiles {
    target: PathBuf,
    staged: PathBuf,
    previous: PathBuf,
    failed: PathBuf,
    confirmed: PathBuf,
}

impl UpdateFiles {
    fn new(target: &Path, id: &str) -> Option<Self> {
        if !is_update_id(id) {
            return None;
        }
        let directory = target.parent()?;
        let name = target.file_name()?.to_str()?;
        let file = |suffix: &str| directory.join(format!(".{name}.update-{id}.{suffix}"));
        Some(Self {
            target: target.to_path_buf(),
            staged: file("tmp"),
            previous: file("old"),
            failed: file("failed"),
            confirmed: file("ok"),
        })
    }

    /// True when something already has one of the names the hand-over will
    /// create. Using it would mean overwriting a file that is not this
    /// update's, or mistaking it for a confirmation.
    fn hand_over_names_taken(&self) -> bool {
        [&self.previous, &self.failed, &self.confirmed]
            .into_iter()
            .any(|path| path_exists(path))
    }
}

fn is_update_id(id: &str) -> bool {
    id.len() == UPDATE_ID_LENGTH && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// True for the update and preflight files of the executable called
/// `executable_name`, and for nothing else that may be in its folder.
fn is_update_file_name(candidate: &str, executable_name: &str) -> bool {
    let Some(rest) = candidate
        .strip_prefix('.')
        .and_then(|rest| rest.strip_prefix(executable_name))
        .and_then(|rest| rest.strip_prefix(".update-"))
    else {
        return false;
    };
    let (rest, suffixes): (&str, &[&str]) = match rest.strip_prefix("write-") {
        Some(probe) => (probe, &PROBE_FILE_SUFFIXES),
        None => (rest, &UPDATE_FILE_SUFFIXES),
    };
    rest.split_once('.')
        .is_some_and(|(id, suffix)| is_update_id(id) && suffixes.contains(&suffix))
}

/// Deletes what an interrupted update left beside `target`. Only call this
/// while no update of this installation is in progress.
fn remove_stale_files(target: &Path) {
    let (Some(directory), Some(name)) = (
        target.parent(),
        target.file_name().and_then(|name| name.to_str()),
    ) else {
        return;
    };
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let leftovers = entries.flatten().filter(|entry| {
        entry
            .file_name()
            .to_str()
            .is_some_and(|candidate| is_update_file_name(candidate, name))
            && entry.file_type().is_ok_and(|kind| kind.is_file())
    });
    let (confirmations, others): (Vec<PathBuf>, Vec<PathBuf>) = leftovers
        .map(|entry| entry.path())
        .partition(|path| path.extension().is_some_and(|suffix| suffix == "ok"));
    for path in others {
        let _ = fs::remove_file(path);
    }
    // A previous executable that cannot be deleted belongs to a process that
    // is still running and may still be waiting to see its confirmation.
    for path in confirmations {
        if !path_exists(&path.with_extension("old")) {
            let _ = fs::remove_file(path);
        }
    }
}

/// A verified download waiting beside the executable to be installed.
/// Dropping it deletes the download.
#[derive(Debug)]
pub struct StagedUpdate {
    files: UpdateFiles,
    id: String,
    size: u64,
    sha256: String,
}

impl StagedUpdate {
    fn verify(&self) -> Result<(), UpdateError> {
        verify_asset_file(&self.files.staged, self.size, &self.sha256)?;
        match is_portable_executable(&self.files.staged) {
            Ok(true) => Ok(()),
            Ok(false) => Err(UpdateError::NotExecutable),
            Err(_) => Err(UpdateError::PreparationFailed),
        }
    }
}

impl Drop for StagedUpdate {
    fn drop(&mut self) {
        // Already gone once the update has been applied.
        let _ = fs::remove_file(&self.files.staged);
    }
}

/// Downloads the update described by `update` beside the installed
/// executable and verifies it. Nothing is replaced yet. Blocks on the
/// network; run it on a worker thread, one update at a time.
pub fn stage(
    update: &UpdateInfo,
    installation: &Installation,
    host: &dyn Host,
) -> Result<StagedUpdate, UpdateError> {
    if !update.update_available {
        return Err(UpdateError::NoUpdate);
    }
    if update.asset_url.trim().is_empty() {
        return Err(UpdateError::NoAsset);
    }
    if !is_expected_release_asset(&update.asset_name) {
        return Err(UpdateError::UnexpectedAssetName);
    }
    if !is_valid_asset_size(update.asset_size) || !is_sha256_hex(&update.asset_sha256) {
        return Err(UpdateError::MissingIntegrityMetadata);
    }
    let url = release_asset_url(&update.asset_url).ok_or(UpdateError::InvalidDownloadAddress)?;
    if !installation.can_install_updates() {
        return Err(UpdateError::InstallUnsupported);
    }

    let target = &installation.executable;
    remove_stale_files(target);
    verify_install_directory(target, host)?;

    let id = host.update_id().ok_or(UpdateError::PreparationFailed)?;
    let files = UpdateFiles::new(target, &id).ok_or(UpdateError::PreparationFailed)?;
    if path_exists(&files.staged) || files.hand_over_names_taken() {
        return Err(UpdateError::PreparationFailed);
    }

    // From here on, leaving early deletes whatever was downloaded.
    let staged = StagedUpdate {
        files,
        id,
        size: update.asset_size,
        sha256: update.asset_sha256.to_ascii_lowercase(),
    };
    if !host.download(&url, &staged.files.staged, staged.size) {
        return Err(UpdateError::DownloadFailed);
    }
    staged.verify()?;
    Ok(staged)
}

/// Checks, with throwaway files, that the app folder allows what the swap
/// needs: creating files, renaming one aside, renaming another into its
/// place, and deleting them.
fn verify_install_directory(target: &Path, host: &dyn Host) -> Result<(), UpdateError> {
    let unusable = UpdateError::FolderNotReplaceable;
    let probe_id = host.update_id().filter(|id| is_update_id(id));
    let (Some(directory), Some(name), Some(probe_id)) = (
        target.parent(),
        target.file_name().and_then(|name| name.to_str()),
        probe_id,
    ) else {
        return Err(unusable);
    };
    let probe = |suffix: &str| directory.join(format!(".{name}.update-write-{probe_id}.{suffix}"));
    let probes = PROBE_FILE_SUFFIXES.map(probe);
    let [probe_target, replacement, backup] = &probes;

    let replaced = write_new_file(probe_target, &[0]).is_ok()
        && write_new_file(replacement, &[1]).is_ok()
        && fs::rename(probe_target, backup).is_ok()
        && fs::rename(replacement, probe_target).is_ok()
        && probe_target.is_file()
        && backup.is_file();
    for path in &probes {
        let _ = fs::remove_file(path);
    }
    if !replaced {
        return Err(unusable);
    }
    if probes.iter().any(|path| path_exists(path)) {
        return Err(UpdateError::PreflightCleanupFailed);
    }
    Ok(())
}

fn write_new_file(path: &Path, content: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(content)?;
    file.flush()
}

/// True for anything at `path`, including a link whose target is missing.
fn path_exists(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

fn verify_asset_file(path: &Path, size: u64, sha256: &str) -> Result<(), UpdateError> {
    let length = fs::metadata(path).map(|metadata| metadata.len()).ok();
    if length != Some(size) {
        return Err(UpdateError::SizeMismatch);
    }
    let actual = File::open(path)
        .ok()
        .and_then(sha256_hex)
        .ok_or(UpdateError::PreparationFailed)?;
    if actual.eq_ignore_ascii_case(sha256) {
        Ok(())
    } else {
        Err(UpdateError::DigestMismatch)
    }
}

/// True when the file starts like a Windows executable: an `MZ` header that
/// points at a `PE` signature inside the file.
fn is_portable_executable(path: &Path) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let mut header = [0u8; DOS_HEADER_BYTES];
    if length < header.len() as u64 {
        return Ok(false);
    }
    file.read_exact(&mut header)?;
    let mut offset = [0u8; 4];
    offset.copy_from_slice(&header[PE_OFFSET_FIELD..PE_OFFSET_FIELD + 4]);
    let offset = u64::from(u32::from_le_bytes(offset));
    if !header.starts_with(b"MZ") || offset < header.len() as u64 || offset > length - 4 {
        return Ok(false);
    }
    let mut signature = [0u8; 4];
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(&mut signature)?;
    Ok(&signature == b"PE\0\0")
}

/// SHA-256 of everything `source` yields, as lower-case hex.
fn sha256_hex(mut source: impl Read) -> Option<String> {
    let mut hash = Sha256::new()?;
    let mut chunk = vec![0u8; HASH_CHUNK_BYTES];
    loop {
        match source.read(&mut chunk) {
            Ok(0) => return hash.finish().map(|digest| hex(&digest)),
            Ok(read) => {
                if !hash.update(&chunk[..read]) {
                    return None;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A SHA-256 computation by Windows CNG.
struct Sha256(BCRYPT_HASH_HANDLE);

impl Sha256 {
    fn new() -> Option<Self> {
        let mut handle: BCRYPT_HASH_HANDLE = null_mut();
        // SAFETY: the pseudo-handle names the system's SHA-256 provider,
        // `handle` receives the new hash object, and without a caller buffer
        // or a key CNG allocates the object itself.
        let status = unsafe {
            BCryptCreateHash(
                BCRYPT_SHA256_ALG_HANDLE,
                &mut handle,
                null_mut(),
                0,
                null(),
                0,
                0,
            )
        };
        (status >= 0).then_some(Self(handle))
    }

    fn update(&mut self, data: &[u8]) -> bool {
        let Ok(length) = u32::try_from(data.len()) else {
            return false;
        };
        // SAFETY: `data` is valid for `length` bytes and the hash is live.
        unsafe { BCryptHashData(self.0, data.as_ptr(), length, 0) >= 0 }
    }

    fn finish(self) -> Option<[u8; SHA256_BYTES]> {
        let mut digest = [0u8; SHA256_BYTES];
        // SAFETY: `digest` has exactly the size of a SHA-256 result, which
        // is the length passed.
        let status =
            unsafe { BCryptFinishHash(self.0, digest.as_mut_ptr(), digest.len() as u32, 0) };
        (status >= 0).then_some(digest)
    }
}

impl Drop for Sha256 {
    fn drop(&mut self) {
        // SAFETY: the handle came from BCryptCreateHash and is destroyed once.
        unsafe {
            BCryptDestroyHash(self.0);
        }
    }
}

/// How [`apply_and_restart`] ended. The calling process exits in every case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// The new version is running and has confirmed.
    Installed,
    /// The previous executable is back and was restarted with
    /// `--update-failed`; it reports the failure.
    RolledBack(InstallFailure),
    /// The update failed and the previous app could not be put back or
    /// restarted. Nothing is left running to report it.
    RecoveryFailed(InstallFailure),
}

/// Installs a staged update and hands over to the new version, or puts the
/// previous version back and restarts it.
///
/// Call this last, after the tray is gone and the single-instance lock is
/// released, because the process started here has to take both over. It
/// returns within a few seconds when the update works and may take around
/// half a minute when it has to be rolled back.
pub fn apply_and_restart(staged: StagedUpdate, host: &dyn Host) -> ApplyOutcome {
    let stopped = match install(&staged, host) {
        Ok(()) => return ApplyOutcome::Installed,
        Err(stopped) => stopped,
    };
    if stopped.restored && restart_previous(&staged.files, stopped.reason, host) {
        ApplyOutcome::RolledBack(stopped.reason)
    } else {
        ApplyOutcome::RecoveryFailed(stopped.reason)
    }
}

/// Why an installation stopped, and whether the previous executable is the
/// installed one again.
struct Stopped {
    reason: InstallFailure,
    restored: bool,
}

fn install(staged: &StagedUpdate, host: &dyn Host) -> Result<(), Stopped> {
    let files = &staged.files;
    let untouched = |reason| Stopped {
        reason,
        restored: true,
    };
    // The download has been lying in the app folder since it was verified,
    // possibly for hours while Settings was open.
    if staged.verify().is_err() {
        return Err(untouched(InstallFailure::StagedFileChanged));
    }
    if files.hand_over_names_taken() {
        return Err(untouched(InstallFailure::ReplaceFailed));
    }
    let replace = |from: &Path, to: &Path| {
        move_file(from, to, REPLACE_ATTEMPTS, REPLACE_RETRY_MILLISECONDS, host)
    };
    if !replace(&files.target, &files.previous) {
        return Err(untouched(InstallFailure::ReplaceFailed));
    }
    if !replace(&files.staged, &files.target) {
        return Err(Stopped {
            reason: InstallFailure::ReplaceFailed,
            restored: restore(&files.previous, &files.target, host),
        });
    }

    let arguments = [CONFIRM_ARGUMENT, staged.id.as_str()];
    let Some(mut process) = host.start(&files.target, &arguments) else {
        return Err(Stopped {
            reason: InstallFailure::StartFailed,
            restored: roll_back(files, None, host),
        });
    };
    match wait_for_confirmation(files, process.as_mut(), host) {
        None => Ok(()),
        Some(reason) => Err(Stopped {
            reason,
            restored: roll_back(files, Some(process.as_mut()), host),
        }),
    }
}

/// Waits for the new process to confirm. Returns why it did not, if so.
fn wait_for_confirmation(
    files: &UpdateFiles,
    process: &mut dyn Process,
    host: &dyn Host,
) -> Option<InstallFailure> {
    let mut waited = 0;
    loop {
        if path_exists(&files.confirmed) {
            return None;
        }
        if process.has_exited() {
            // It may have confirmed and then been closed between the two checks.
            return (!path_exists(&files.confirmed)).then_some(InstallFailure::ExitedEarly);
        }
        if waited >= CONFIRMATION_TIMEOUT_MILLISECONDS {
            return Some(InstallFailure::TimedOut);
        }
        host.sleep(POLL_MILLISECONDS);
        waited += POLL_MILLISECONDS;
    }
}

/// Puts the previous executable back in place of a new one that did not
/// confirm. True when the previous executable is the installed one again.
fn roll_back(files: &UpdateFiles, process: Option<&mut dyn Process>, host: &dyn Host) -> bool {
    // A new process that keeps running would go on acting as the app, so
    // the files stay as they are if it cannot be stopped.
    if process.is_some_and(|process| !process.terminate()) {
        return false;
    }
    // Moving the new executable aside works even while something still has
    // it open, which deleting or overwriting it would not.
    if !restore(&files.target, &files.failed, host) {
        return false;
    }
    if !restore(&files.previous, &files.target, host) {
        // Better the unconfirmed version than no executable where shortcuts
        // and the startup entry expect one.
        restore(&files.failed, &files.target, host);
        return false;
    }
    remove_file(
        &files.failed,
        ROLLBACK_ATTEMPTS,
        ROLLBACK_RETRY_MILLISECONDS,
        host,
    );
    let _ = fs::remove_file(&files.confirmed);
    true
}

fn restore(from: &Path, to: &Path, host: &dyn Host) -> bool {
    move_file(
        from,
        to,
        ROLLBACK_ATTEMPTS,
        ROLLBACK_RETRY_MILLISECONDS,
        host,
    )
}

/// Starts the restored executable and checks that it stays running.
fn restart_previous(files: &UpdateFiles, reason: InstallFailure, host: &dyn Host) -> bool {
    let Some(mut process) = host.start(&files.target, &[FAILED_ARGUMENT, reason.code()]) else {
        return false;
    };
    let mut waited = 0;
    while waited < STARTUP_VERIFICATION_MILLISECONDS {
        if process.has_exited() {
            return false;
        }
        host.sleep(POLL_MILLISECONDS);
        waited += POLL_MILLISECONDS;
    }
    true
}

/// Renames `from` to `to` without ever replacing a file, retrying while
/// something else briefly holds one of them.
fn move_file(from: &Path, to: &Path, attempts: u32, delay: u32, host: &dyn Host) -> bool {
    for attempt in 1..=attempts {
        if path_exists(to) {
            return false;
        }
        match fs::rename(from, to) {
            Ok(()) => return true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return false,
            Err(_) if attempt < attempts => host.sleep(delay),
            Err(_) => {}
        }
    }
    false
}

/// Deletes a file, retrying while it is still in use. True once it is gone.
fn remove_file(path: &Path, attempts: u32, delay: u32, host: &dyn Host) -> bool {
    for attempt in 1..=attempts {
        match fs::remove_file(path) {
            Ok(()) => return true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return true,
            Err(_) if attempt < attempts => host.sleep(delay),
            Err(_) => {}
        }
    }
    false
}

/// What kind of start this is, read from the command line.
#[derive(Debug, PartialEq, Eq)]
pub enum Startup {
    /// Started by the user or by Windows.
    Normal,
    /// The previous version started this process to take over after an
    /// update and is waiting for it to confirm.
    Updated(Confirmation),
    /// An update was rolled back and this is the previous version again.
    UpdateFailed(InstallFailure),
}

/// Decides which kind of start this is. `arguments` are the process
/// arguments without the program name and `executable` is the running
/// executable. Reads nothing but its parameters.
pub fn startup(arguments: &[String], executable: &Path) -> Startup {
    let value_after = |flag: &str| {
        arguments
            .iter()
            .position(|argument| argument.eq_ignore_ascii_case(flag))
            .map(|index| arguments.get(index + 1).map_or("", String::as_str))
    };
    if let Some(files) =
        value_after(CONFIRM_ARGUMENT).and_then(|id| UpdateFiles::new(executable, id))
    {
        return Startup::Updated(Confirmation { files });
    }
    match value_after(FAILED_ARGUMENT) {
        Some(code) => Startup::UpdateFailed(InstallFailure::from_code(code)),
        None => Startup::Normal,
    }
}

/// Held by a freshly updated process until it has shown that it runs.
#[derive(Debug, PartialEq, Eq)]
#[must_use = "an update that is never confirmed is rolled back"]
pub struct Confirmation {
    files: UpdateFiles,
}

impl Confirmation {
    /// Tells the waiting previous version that this process is running, and
    /// deletes the previous executable in the background once that process
    /// has exited. Call it when the tray has started.
    pub fn confirm(self) {
        if self.signal() {
            thread::spawn(move || self.remove_previous(&SystemHost));
        }
    }

    fn signal(&self) -> bool {
        File::create(&self.files.confirmed).is_ok()
    }

    /// True once the previous executable and the confirmation are gone.
    fn remove_previous(&self, host: &dyn Host) -> bool {
        // While the previous process is alive its executable cannot be
        // deleted, and it may not have seen the confirmation yet, so the
        // confirmation outlives the executable.
        remove_file(
            &self.files.previous,
            CLEANUP_ATTEMPTS,
            CLEANUP_RETRY_MILLISECONDS,
            host,
        ) && fs::remove_file(&self.files.confirmed).is_ok()
    }
}

/// What started an update check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// The check at startup or the hourly one.
    Automatic,
    /// "Check updates" in Settings.
    Manual,
}

/// The Settings window as far as restarting for an update is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsWindow {
    Closed,
    Open { unsaved_changes: bool },
}

/// True while an update must not restart the app: a background update waits
/// for Settings to close, a requested one only for unsaved changes.
pub fn should_wait_for_settings(settings: SettingsWindow, trigger: Trigger) -> bool {
    match settings {
        SettingsWindow::Closed => false,
        SettingsWindow::Open { unsaved_changes } => {
            trigger == Trigger::Automatic || unsaved_changes
        }
    }
}

/// The answer to a request for an update check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckRequest {
    /// Run [`check`] and pass its result to [`Session::check_finished`].
    Start,
    /// The next automatic check is not due yet.
    NotDue,
    /// Automatic checks are paused for this session after a failed update.
    Paused,
    /// A check or an installation is already under way.
    Busy,
}

/// What a finished check asks of the tray.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckOutcome {
    /// Nothing newer is published.
    UpToDate {
        message: String,
    },
    /// Run [`stage`] with this and pass its result to
    /// [`Session::stage_finished`].
    Install(UpdateInfo),
    /// A newer release exists but cannot be installed automatically.
    NotInstallable {
        message: String,
        release_url: String,
    },
    Failed {
        message: String,
    },
}

/// What a finished download asks of the tray.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StageOutcome {
    /// The update is ready; ask [`Session::take_ready`] for it.
    Ready,
    Failed {
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Activity {
    Idle,
    Checking(Trigger),
    Installing(Trigger),
}

/// Update state for one run of the tray: when to check, what is in
/// progress, and the status line Settings shows. It never starts work or
/// reads the clock itself.
#[derive(Debug)]
pub struct Session {
    /// Set when this run began with a rolled-back update.
    failure: Option<InstallFailure>,
    next_automatic_check: Option<i64>,
    activity: Activity,
    ready: Option<StagedUpdate>,
    status: String,
}

impl Session {
    /// The state at the start of a run. After [`Startup::UpdateFailed`]
    /// automatic checks stay paused and the status says why.
    pub fn new(startup: &Startup) -> Self {
        let failure = match startup {
            Startup::UpdateFailed(reason) => Some(*reason),
            _ => None,
        };
        Self {
            failure,
            next_automatic_check: None,
            activity: Activity::Idle,
            ready: None,
            status: failure.map(paused_status).unwrap_or_default(),
        }
    }

    /// The update status line for Settings. Empty until something happened.
    pub fn status(&self) -> &str {
        &self.status
    }

    /// The label for the "Check updates" button while it is disabled.
    pub fn busy_label(&self) -> Option<&'static str> {
        match self.activity {
            Activity::Idle => None,
            Activity::Checking(_) => Some("Checking..."),
            Activity::Installing(_) => Some("Installing..."),
        }
    }

    /// Asks whether a check should start now. Automatic requests are due at
    /// startup and then once per [`CHECK_INTERVAL_SECONDS`], so they can be
    /// made from any timer; `now` is in seconds.
    pub fn request_check(&mut self, trigger: Trigger, now: i64) -> CheckRequest {
        if trigger == Trigger::Automatic {
            // A clock that was set back must not postpone checks indefinitely.
            let due = self
                .next_automatic_check
                .is_none_or(|next| now >= next || next - now > CHECK_INTERVAL_SECONDS);
            if !due {
                return CheckRequest::NotDue;
            }
            self.next_automatic_check = Some(now.saturating_add(CHECK_INTERVAL_SECONDS));
            if let Some(reason) = self.failure {
                self.status = paused_status(reason);
                return CheckRequest::Paused;
            }
        }
        if self.activity != Activity::Idle {
            return CheckRequest::Busy;
        }
        self.activity = Activity::Checking(trigger);
        self.status = "Checking for updates...".to_string();
        CheckRequest::Start
    }

    /// Records the result of [`check`]. Returns what started that check and
    /// what to do next.
    pub fn check_finished(
        &mut self,
        result: Result<UpdateInfo, UpdateError>,
    ) -> (Trigger, CheckOutcome) {
        let trigger = self.trigger();
        self.activity = Activity::Idle;
        let outcome = match result {
            Err(error) => {
                self.status = "Update check failed.".to_string();
                CheckOutcome::Failed {
                    message: error.message().to_string(),
                }
            }
            Ok(info) if !info.update_available => {
                self.status = format!("Up to date: {}", info.latest_version);
                CheckOutcome::UpToDate {
                    message: info.message,
                }
            }
            Ok(info) if info.can_install => {
                self.activity = Activity::Installing(trigger);
                self.status = format!("Installing version {}...", info.latest_version);
                CheckOutcome::Install(info)
            }
            Ok(info) => {
                self.status = info.message.clone();
                CheckOutcome::NotInstallable {
                    message: info.message,
                    release_url: info.release_url,
                }
            }
        };
        (trigger, outcome)
    }

    /// Records the result of [`stage`]. Returns what started the update and
    /// what to do next.
    pub fn stage_finished(
        &mut self,
        result: Result<StagedUpdate, UpdateError>,
    ) -> (Trigger, StageOutcome) {
        let trigger = self.trigger();
        let outcome = match result {
            Ok(staged) => {
                self.ready = Some(staged);
                StageOutcome::Ready
            }
            Err(error) => {
                self.activity = Activity::Idle;
                self.status = format!("Update installation failed: {}", error.message());
                StageOutcome::Failed {
                    message: error.message().to_string(),
                }
            }
        };
        (trigger, outcome)
    }

    /// Hands out the downloaded update once Settings allows a restart. Ask
    /// when the download finishes and whenever the window's state changes;
    /// with the result, close the tray and call [`apply_and_restart`].
    pub fn take_ready(&mut self, settings: SettingsWindow) -> Option<StagedUpdate> {
        self.ready.as_ref()?;
        let trigger = self.trigger();
        if should_wait_for_settings(settings, trigger) {
            self.status = match trigger {
                Trigger::Manual => {
                    "Update ready. Save or discard your Settings changes to restart."
                }
                Trigger::Automatic => "Update ready. Close Settings to restart.",
            }
            .to_string();
            return None;
        }
        self.ready.take()
    }

    fn trigger(&self) -> Trigger {
        match self.activity {
            Activity::Checking(trigger) | Activity::Installing(trigger) => trigger,
            Activity::Idle => Trigger::Automatic,
        }
    }
}

fn paused_status(reason: InstallFailure) -> String {
    format!(
        "Update installation failed: {} Automatic updates paused after a failed update. Check updates to retry.",
        reason.message()
    )
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::os::windows::fs::OpenOptionsExt;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU32, Ordering as AtomicOrdering};

    use serde_json::json;

    use super::*;

    const EXECUTABLE_NAME: &str = "usage-tray-rs.exe";
    const UNRELATED_NAME: &str = "user-note.txt";
    const CURRENT_VERSION: &str = "1.0.0";
    const FIRST_ID: &str = "00000000000000000000000000000002";
    /// Sharing that lets others read a file but not rename or delete it.
    const SHARE_READ_ONLY: u32 = 1;

    /// A folder of dummy files that is removed when the test ends.
    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let unique = NEXT.fetch_add(1, AtomicOrdering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "usage-tray-rs-update-{label}-{}-{unique}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }

        fn names(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(&self.0)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// What a started dummy process does.
    #[derive(Clone, Copy)]
    enum Launch {
        /// Cannot be started at all.
        Fails,
        Confirms,
        ConfirmsThenExits,
        /// Runs without confirming until the moment it is stopped.
        ConfirmsWhileBeingStopped,
        Exits,
        /// Exits, while something else keeps the previous executable open.
        ExitsWithPreviousHeld,
        /// Keeps running without confirming.
        Runs,
        CannotBeStopped,
    }

    struct FakeProcess {
        exited: bool,
        stoppable: bool,
        stopped: Rc<Cell<u32>>,
        /// A confirmation this process writes just as it is being stopped.
        late_confirmation: Option<PathBuf>,
    }

    impl Process for FakeProcess {
        fn has_exited(&mut self) -> bool {
            self.exited
        }

        fn terminate(&mut self) -> bool {
            if !self.exited && self.stoppable {
                if let Some(confirmation) = self.late_confirmation.take() {
                    fs::write(confirmation, "").unwrap();
                }
                self.exited = true;
                self.stopped.set(self.stopped.get() + 1);
            }
            self.exited
        }
    }

    struct FakeHost {
        release: ReleaseResponse,
        /// What a download writes; `None` makes downloads fail.
        asset: Option<Vec<u8>>,
        downloads: RefCell<Vec<String>>,
        ids: Cell<u32>,
        launches: RefCell<VecDeque<Launch>>,
        started: RefCell<Vec<(PathBuf, Vec<String>)>>,
        stopped: Rc<Cell<u32>>,
        slept: Cell<u64>,
        on_sleep: RefCell<Option<Box<dyn FnMut()>>>,
        held: RefCell<Option<File>>,
    }

    impl FakeHost {
        fn new(asset: Option<Vec<u8>>) -> Self {
            Self {
                release: ReleaseResponse::NotFound,
                asset,
                downloads: RefCell::new(Vec::new()),
                ids: Cell::new(0),
                launches: RefCell::new(VecDeque::new()),
                started: RefCell::new(Vec::new()),
                stopped: Rc::new(Cell::new(0)),
                slept: Cell::new(0),
                on_sleep: RefCell::new(None),
                held: RefCell::new(None),
            }
        }

        fn with_release(release: ReleaseResponse) -> Self {
            Self {
                release,
                ..Self::new(None)
            }
        }

        fn launching(self, launches: &[Launch]) -> Self {
            self.launches.borrow_mut().extend(launches);
            self
        }

        fn started(&self) -> Vec<(PathBuf, Vec<String>)> {
            self.started.borrow().clone()
        }
    }

    impl Host for FakeHost {
        fn latest_release(&self) -> ReleaseResponse {
            self.release.clone()
        }

        fn download(&self, url: &str, destination: &Path, size: u64) -> bool {
            self.downloads.borrow_mut().push(url.to_string());
            match &self.asset {
                Some(asset) if asset.len() as u64 <= size => fs::write(destination, asset).is_ok(),
                _ => false,
            }
        }

        fn update_id(&self) -> Option<String> {
            self.ids.set(self.ids.get() + 1);
            Some(format!("{:032x}", self.ids.get()))
        }

        fn start(&self, executable: &Path, arguments: &[&str]) -> Option<Box<dyn Process>> {
            let arguments: Vec<String> = arguments.iter().map(|text| text.to_string()).collect();
            self.started
                .borrow_mut()
                .push((executable.to_path_buf(), arguments.clone()));
            let launch = self.launches.borrow_mut().pop_front();
            // The dummy new process runs the real start-up side of the protocol.
            let confirm = || match startup(&arguments, executable) {
                Startup::Updated(confirmation) => assert!(confirmation.signal()),
                other => panic!("expected an updated start, got {other:?}"),
            };
            let files = || UpdateFiles::new(executable, &arguments[1]).unwrap();
            let mut process = FakeProcess {
                exited: false,
                stoppable: true,
                stopped: Rc::clone(&self.stopped),
                late_confirmation: None,
            };
            match launch.unwrap_or(Launch::Runs) {
                Launch::Fails => return None,
                Launch::Confirms => confirm(),
                Launch::ConfirmsThenExits => {
                    confirm();
                    process.exited = true;
                }
                Launch::ConfirmsWhileBeingStopped => {
                    process.late_confirmation = Some(files().confirmed);
                }
                Launch::Exits => process.exited = true,
                Launch::ExitsWithPreviousHeld => {
                    *self.held.borrow_mut() = Some(lock_against_moving(&files().previous));
                    process.exited = true;
                }
                Launch::Runs => {}
                Launch::CannotBeStopped => process.stoppable = false,
            }
            Some(Box::new(process))
        }

        fn sleep(&self, milliseconds: u32) {
            self.slept.set(self.slept.get() + u64::from(milliseconds));
            if let Some(action) = self.on_sleep.borrow_mut().as_mut() {
                action();
            }
        }
    }

    /// The smallest file that passes for a Windows executable, made
    /// distinguishable by `marker`.
    fn executable_bytes(marker: &str) -> Vec<u8> {
        let mut bytes = vec![0u8; DOS_HEADER_BYTES];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[PE_OFFSET_FIELD] = DOS_HEADER_BYTES as u8;
        bytes.extend_from_slice(b"PE\0\0");
        bytes.extend_from_slice(marker.as_bytes());
        bytes
    }

    fn old_executable() -> Vec<u8> {
        executable_bytes("previous version")
    }

    fn new_executable() -> Vec<u8> {
        executable_bytes("updated version")
    }

    fn sha256_of(bytes: &[u8]) -> String {
        sha256_hex(bytes).unwrap()
    }

    fn asset_url(name: &str) -> String {
        format!(
            "https://github.com/jeremygold02/codex-usage-tray-icon/releases/download/v2.0.0/{name}"
        )
    }

    fn rust_asset(content: &[u8]) -> Value {
        json!({
            "name": RELEASE_ASSET_NAME,
            "browser_download_url": asset_url(RELEASE_ASSET_NAME),
            "size": content.len(),
            "digest": format!("sha256:{}", sha256_of(content)),
        })
    }

    fn release(tag: &str, assets: Value) -> ReleaseResponse {
        let release = json!({
            "tag_name": tag,
            "html_url": "https://github.com/jeremygold02/codex-usage-tray-icon/releases/tag/v2.0.0",
            "assets": assets,
        });
        ReleaseResponse::Found(release.to_string().into_bytes())
    }

    /// An app folder holding the installed executable and an unrelated file.
    fn installed(label: &str) -> (TestDirectory, Installation) {
        let directory = TestDirectory::new(label);
        fs::write(directory.file(EXECUTABLE_NAME), old_executable()).unwrap();
        fs::write(directory.file(UNRELATED_NAME), "keep").unwrap();
        let installation = Installation {
            executable: directory.file(EXECUTABLE_NAME),
            version: CURRENT_VERSION.to_string(),
        };
        (directory, installation)
    }

    fn installable_update(content: &[u8]) -> UpdateInfo {
        UpdateInfo {
            current_version: CURRENT_VERSION.to_string(),
            latest_version: "2.0.0".to_string(),
            update_available: true,
            can_install: true,
            release_url: version::repository_url(),
            repository_url: version::repository_url(),
            asset_name: RELEASE_ASSET_NAME.to_string(),
            asset_url: asset_url(RELEASE_ASSET_NAME),
            asset_size: content.len() as u64,
            asset_sha256: sha256_of(content),
            message: "Update available: 2.0.0.".to_string(),
        }
    }

    /// An installation with the new version downloaded and verified.
    fn staged_update(label: &str) -> (TestDirectory, Installation, FakeHost, StagedUpdate) {
        let (directory, installation) = installed(label);
        let host = FakeHost::new(Some(new_executable()));
        let staged = stage(&installable_update(&new_executable()), &installation, &host).unwrap();
        (directory, installation, host, staged)
    }

    fn update_files(installation: &Installation) -> UpdateFiles {
        UpdateFiles::new(&installation.executable, FIRST_ID).unwrap()
    }

    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn lock_against_moving(path: &Path) -> File {
        OpenOptions::new()
            .read(true)
            .share_mode(SHARE_READ_ONLY)
            .open(path)
            .unwrap()
    }

    #[test]
    fn only_the_rust_executable_is_an_expected_asset() {
        assert!(is_expected_release_asset("usage-tray-rs.exe"));
        assert!(is_expected_release_asset("USAGE-TRAY-RS.EXE"));
        for name in [
            "CodexUsageTray.exe",
            "CodexUsageTray-Wpf.zip",
            "other/usage-tray-rs.exe",
            "usage-tray-rs.exe.zip",
            "usage-tray-rs.zip",
            "usagetrayrs.exe",
            "",
        ] {
            assert!(!is_expected_release_asset(name), "{name}");
        }
    }

    #[test]
    fn the_rust_asset_is_picked_wherever_it_is_listed() {
        let classic = json!({ "name": "CodexUsageTray.exe" });
        let wpf = json!({ "name": "CodexUsageTray-Wpf.zip" });
        let rust = json!({ "name": "usage-tray-rs.exe", "size": 7 });
        let pick = |assets: Value| {
            let release = json!({ "assets": assets });
            release_asset(release.as_object().unwrap()).cloned()
        };
        let expected = rust.as_object().cloned();

        assert_eq!(pick(json!([classic, wpf, rust])), expected);
        assert_eq!(pick(json!([rust, wpf, classic])), expected);
        assert_eq!(pick(json!(["text", 5, null, rust])), expected);
    }

    #[test]
    fn releases_with_only_the_other_builds_have_nothing_to_install() {
        let pick = |release: Value| release_asset(release.as_object().unwrap()).cloned();
        assert_eq!(
            pick(json!({ "assets": [
                { "name": "CodexUsageTray.exe" },
                { "name": "CodexUsageTray-Wpf.zip" }
            ] })),
            None
        );
        assert_eq!(pick(json!({ "assets": [] })), None);
        assert_eq!(pick(json!({ "assets": "usage-tray-rs.exe" })), None);
        assert_eq!(pick(json!({ "assets": null })), None);
        assert_eq!(pick(json!({})), None);
    }

    #[test]
    fn sha256_digests_are_read_from_release_metadata() {
        let hash = "a".repeat(64);
        assert_eq!(
            parse_sha256_digest(&format!("sha256:{hash}")),
            Some(hash.clone())
        );
        assert_eq!(
            parse_sha256_digest(&format!("  SHA256:{}  ", "AB".repeat(32))),
            Some("ab".repeat(32))
        );
        for invalid in [
            String::new(),
            "sha256:abcd".to_string(),
            format!("md5:{hash}"),
            format!("sha256:{}", "z".repeat(64)),
            format!("sha256:{hash}0"),
            hash,
        ] {
            assert_eq!(parse_sha256_digest(&invalid), None, "{invalid}");
        }
    }

    #[test]
    fn sha256_matches_published_test_vectors() {
        assert_eq!(
            sha256_of(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_of(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // Longer than one read, so the hash is built from several chunks.
        assert_eq!(
            sha256_of(&vec![b'a'; 1_000_000]),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn downloads_are_verified_by_size_and_digest() {
        let directory = TestDirectory::new("integrity");
        let path = directory.file("integrity.bin");
        fs::write(&path, [1, 2, 3]).unwrap();
        let hash = "039058c6f2c0cb492c533b0a4d14ef77cc0f78abccced5287d84a1a2011cfb81";

        assert_eq!(verify_asset_file(&path, 3, hash), Ok(()));
        assert_eq!(
            verify_asset_file(&path, 3, &hash.to_ascii_uppercase()),
            Ok(())
        );
        assert_eq!(
            verify_asset_file(&path, 3, &"0".repeat(64)),
            Err(UpdateError::DigestMismatch)
        );
        assert_eq!(
            verify_asset_file(&path, 4, hash),
            Err(UpdateError::SizeMismatch)
        );
        assert_eq!(
            verify_asset_file(&directory.file("missing.bin"), 3, hash),
            Err(UpdateError::SizeMismatch)
        );
    }

    #[test]
    fn executables_are_recognized_by_their_headers() {
        let directory = TestDirectory::new("headers");
        let check = |content: &[u8]| {
            let path = directory.file("candidate.exe");
            fs::write(&path, content).unwrap();
            is_portable_executable(&path).unwrap()
        };
        assert!(check(&new_executable()));
        assert!(!check(b"not a PE"));
        assert!(!check(b""));
        assert!(!check(&[b'M', b'Z', 1]));

        let mut wrong_magic = new_executable();
        wrong_magic[0] = b'P';
        assert!(!check(&wrong_magic));

        let mut wrong_signature = new_executable();
        wrong_signature[DOS_HEADER_BYTES] = b'N';
        assert!(!check(&wrong_signature));

        let mut offset_into_header = new_executable();
        offset_into_header[PE_OFFSET_FIELD] = 8;
        assert!(!check(&offset_into_header));

        let mut offset_past_end = new_executable();
        offset_past_end[PE_OFFSET_FIELD + 2] = 0x7f;
        assert!(!check(&offset_past_end));
    }

    #[test]
    fn asset_addresses_must_be_this_repositorys_github_downloads() {
        let download = asset_url(RELEASE_ASSET_NAME);
        assert_eq!(release_asset_url(&download), Some(download.clone()));
        assert_eq!(
            release_asset_url(
                "HTTPS://GitHub.com:443/JeremyGold02/Codex-Usage-Tray-Icon/releases/download/v2/a.exe"
            )
            .as_deref(),
            Some("https://github.com/JeremyGold02/Codex-Usage-Tray-Icon/releases/download/v2/a.exe")
        );

        let path = "/jeremygold02/codex-usage-tray-icon/releases/download/v2/a.exe";
        for invalid in [
            String::new(),
            "not a url".to_string(),
            format!("http://github.com{path}"),
            format!("https://example.com{path}"),
            format!("https://github.com.example.com{path}"),
            format!("https://api.github.com{path}"),
            format!("https://github.com:8443{path}"),
            format!("https://user@github.com{path}"),
            format!("https://github.com@example.com{path}"),
            format!("https://github.com{path}#fragment"),
            "https://github.com/someone/else/releases/download/v2/a.exe".to_string(),
            "https://github.com/jeremygold02/codex-usage-tray-icon/archive/v2.zip".to_string(),
            "https://github.com/jeremygold02/codex-usage-tray-icon/releases/download/../../../../other/a.exe"
                .to_string(),
            "https://github.com/jeremygold02/codex-usage-tray-icon/releases/download/%2E%2e/a.exe"
                .to_string(),
            "https://github.com/jeremygold02/codex-usage-tray-icon/releases/download/v2\\a.exe"
                .to_string(),
            "https://github.com/jeremygold02/codex-usage-tray-icon/releases/download/v2/a b.exe"
                .to_string(),
            "https://github.com".to_string(),
        ] {
            assert_eq!(release_asset_url(&invalid), None, "{invalid}");
        }
    }

    #[test]
    fn asset_sizes_are_whole_non_negative_numbers() {
        let size = |value: Value| {
            let asset = json!({ "size": value });
            whole_number(asset.as_object().unwrap(), "size")
        };
        assert_eq!(size(json!(3)), 3);
        assert_eq!(size(json!(3.0)), 3);
        assert_eq!(size(json!("12")), 12);
        assert_eq!(size(json!(3.5)), 0);
        assert_eq!(size(json!(-1)), 0);
        assert_eq!(size(json!(true)), 0);
        assert_eq!(size(json!(null)), 0);
        assert_eq!(size(json!("twelve")), 0);
        assert_eq!(size(json!(u64::MAX)), 0);
        assert_eq!(size(json!(1e300)), 0);

        assert!(is_valid_asset_size(1));
        assert!(is_valid_asset_size(MAX_RELEASE_ASSET_BYTES));
        assert!(!is_valid_asset_size(0));
        assert!(!is_valid_asset_size(MAX_RELEASE_ASSET_BYTES + 1));
    }

    #[test]
    fn a_newer_release_with_a_verified_asset_can_be_installed() {
        let (_directory, installation) = installed("check-installable");
        let content = new_executable();
        let host = FakeHost::with_release(release(
            "v2.0.0",
            json!([{ "name": "CodexUsageTray.exe" }, rust_asset(&content)]),
        ));

        let info = check(&installation, &host).unwrap();
        assert_eq!(info.current_version, "1.0.0");
        assert_eq!(info.latest_version, "2.0.0");
        assert!(info.update_available && info.can_install);
        assert_eq!(info.asset_name, RELEASE_ASSET_NAME);
        assert_eq!(info.asset_url, asset_url(RELEASE_ASSET_NAME));
        assert_eq!(info.asset_size, content.len() as u64);
        assert_eq!(info.asset_sha256, sha256_of(&content));
        assert_eq!(
            info.release_url,
            "https://github.com/jeremygold02/codex-usage-tray-icon/releases/tag/v2.0.0"
        );
        assert_eq!(info.repository_url, version::repository_url());
        assert_eq!(info.message, "Update available: 2.0.0.");
    }

    #[test]
    fn the_installed_version_or_an_older_release_is_not_an_update() {
        let (_directory, installation) = installed("check-current");
        for tag in ["v1.0.0", "1.0", "v0.9.9"] {
            let host = FakeHost::with_release(release(tag, json!([rust_asset(&new_executable())])));
            let info = check(&installation, &host).unwrap();
            assert!(!info.update_available && !info.can_install, "{tag}");
            assert_eq!(
                info.message,
                format!(
                    "You are on the latest published version ({}).",
                    info.latest_version
                )
            );
        }
    }

    #[test]
    fn a_release_without_the_rust_asset_is_not_a_compatible_update() {
        let (_directory, installation) = installed("check-incompatible");
        let host = FakeHost::with_release(release(
            "v2.0.0",
            json!([
                { "name": "CodexUsageTray.exe", "browser_download_url": asset_url("CodexUsageTray.exe"),
                  "size": 10, "digest": format!("sha256:{}", "a".repeat(64)) },
                { "name": "CodexUsageTray-Wpf.zip", "browser_download_url": asset_url("CodexUsageTray-Wpf.zip"),
                  "size": 10, "digest": format!("sha256:{}", "b".repeat(64)) }
            ]),
        ));

        let info = check(&installation, &host).unwrap();
        assert!(info.update_available && !info.can_install);
        assert_eq!(
            (info.asset_name.as_str(), info.asset_url.as_str()),
            ("", "")
        );
        assert_eq!(
            info.message,
            "Version 2.0.0 does not include usage-tray-rs.exe. No compatible update is available yet."
        );
        // The other builds' assets are never downloaded.
        assert_eq!(
            stage(&info, &installation, &host).unwrap_err(),
            UpdateError::NoAsset
        );
        assert!(host.downloads.borrow().is_empty());
    }

    #[test]
    fn releases_without_safe_metadata_cannot_be_installed() {
        let (_directory, installation) = installed("check-metadata");
        let content = new_executable();
        let describe = |change: &dyn Fn(&mut Value)| {
            let mut asset = rust_asset(&content);
            change(&mut asset);
            let host = FakeHost::with_release(release("v2.0.0", json!([asset])));
            check(&installation, &host).unwrap()
        };
        let no_integrity = "Update available: 2.0.0, but automatic installation is unavailable because the release has no valid size and SHA-256 metadata.";

        let info = describe(&|asset| {
            asset["browser_download_url"] = json!("https://example.com/usage-tray-rs.exe")
        });
        assert!(info.update_available && !info.can_install);
        assert_eq!(
            info.message,
            "Update available: 2.0.0, but its download address is not valid for automatic installation."
        );

        // GitHub publishes no digest for assets uploaded before it computed them.
        let info = describe(&|asset| {
            asset.as_object_mut().unwrap().remove("digest");
        });
        assert!(info.update_available && !info.can_install);
        assert_eq!(info.asset_sha256, "");
        assert_eq!(info.message, no_integrity);
        let host = FakeHost::new(Some(content.clone()));
        assert_eq!(
            stage(&info, &installation, &host).unwrap_err(),
            UpdateError::MissingIntegrityMetadata
        );
        assert!(host.downloads.borrow().is_empty());

        let info = describe(&|asset| asset["digest"] = json!("md5:0123456789abcdef"));
        assert_eq!(
            (info.can_install, info.message.as_str()),
            (false, no_integrity)
        );

        let info = describe(&|asset| asset["size"] = json!(0));
        assert_eq!(
            (info.can_install, info.message.as_str()),
            (false, no_integrity)
        );

        let info = describe(&|asset| asset["size"] = json!(MAX_RELEASE_ASSET_BYTES + 1));
        assert_eq!(
            (info.can_install, info.message.as_str()),
            (false, no_integrity)
        );
    }

    #[test]
    fn a_copy_that_cannot_replace_itself_says_so() {
        let (directory, installation) = installed("check-unsupported");
        let host =
            FakeHost::with_release(release("v2.0.0", json!([rust_asset(&new_executable())])));
        assert!(installation.can_install_updates());

        let writable = fs::metadata(&installation.executable)
            .unwrap()
            .permissions();
        let mut read_only = writable.clone();
        read_only.set_readonly(true);
        fs::set_permissions(&installation.executable, read_only).unwrap();
        assert!(!installation.can_install_updates());
        let info = check(&installation, &host).unwrap();
        assert!(info.update_available && !info.can_install);
        assert_eq!(
            info.message,
            "Update available: 2.0.0. Automatic installation is not available in this copy."
        );
        let mut forced = info.clone();
        forced.can_install = true;
        assert_eq!(
            stage(&forced, &installation, &host).unwrap_err(),
            UpdateError::InstallUnsupported
        );
        fs::set_permissions(&installation.executable, writable).unwrap();

        let unsupported = |executable: PathBuf| {
            !Installation {
                executable,
                version: CURRENT_VERSION.to_string(),
            }
            .can_install_updates()
        };
        fs::write(directory.file("tray.bin"), old_executable()).unwrap();
        fs::create_dir(directory.file("folder.exe")).unwrap();
        assert!(unsupported(directory.file("tray.bin")));
        assert!(unsupported(directory.file("folder.exe")));
        assert!(unsupported(directory.file("missing.exe")));
        assert!(unsupported(PathBuf::from(EXECUTABLE_NAME)));
    }

    #[test]
    fn missing_and_unreadable_releases_are_told_apart() {
        let (_directory, installation) = installed("check-responses");
        let check_with =
            |release: ReleaseResponse| check(&installation, &FakeHost::with_release(release));

        let info = check_with(ReleaseResponse::NotFound).unwrap();
        assert!(!info.update_available && !info.can_install);
        assert_eq!(info.latest_version, "");
        assert_eq!(info.release_url, version::repository_url());
        assert_eq!(info.message, "No published releases were found.");

        let info = check_with(ReleaseResponse::Found(b"[1, 2]".to_vec())).unwrap();
        assert_eq!(info.message, "No published releases were found.");

        assert_eq!(
            check_with(ReleaseResponse::Unreachable),
            Err(UpdateError::GitHubUnreachable)
        );
        assert_eq!(
            check_with(ReleaseResponse::Found(b"<html>".to_vec())),
            Err(UpdateError::InvalidReleaseData)
        );

        // A release page address that is missing falls back to the repository.
        let info = check_with(ReleaseResponse::Found(
            json!({ "tag_name": "v1.0.0", "html_url": " " })
                .to_string()
                .into_bytes(),
        ))
        .unwrap();
        assert_eq!(info.release_url, version::repository_url());
    }

    #[test]
    fn staging_downloads_and_verifies_beside_the_executable() {
        let (directory, installation, host, staged) = staged_update("stage");
        let files = update_files(&installation);

        assert_eq!(staged.files, files);
        assert_eq!(*host.downloads.borrow(), [asset_url(RELEASE_ASSET_NAME)]);
        assert_eq!(fs::read(&files.staged).unwrap(), new_executable());
        assert_eq!(
            directory.names(),
            [
                format!(".{EXECUTABLE_NAME}.update-{FIRST_ID}.tmp"),
                EXECUTABLE_NAME.to_string(),
                UNRELATED_NAME.to_string()
            ]
        );
        // Nothing installed has changed yet.
        assert_eq!(
            fs::read(&installation.executable).unwrap(),
            old_executable()
        );
        assert_eq!(fs::read(directory.file(UNRELATED_NAME)).unwrap(), b"keep");

        drop(staged);
        assert_eq!(directory.names(), [EXECUTABLE_NAME, UNRELATED_NAME]);
    }

    #[test]
    fn staging_refuses_what_is_not_a_verifiable_update() {
        let (directory, installation) = installed("stage-guards");
        let host = FakeHost::new(Some(new_executable()));
        let refused = |change: &dyn Fn(&mut UpdateInfo)| {
            let mut update = installable_update(&new_executable());
            change(&mut update);
            stage(&update, &installation, &host).unwrap_err()
        };

        assert_eq!(
            refused(&|update| update.update_available = false),
            UpdateError::NoUpdate
        );
        assert_eq!(
            refused(&|update| update.asset_url = " ".to_string()),
            UpdateError::NoAsset
        );
        assert_eq!(
            refused(&|update| update.asset_name = "CodexUsageTray.exe".to_string()),
            UpdateError::UnexpectedAssetName
        );
        assert_eq!(
            refused(&|update| update.asset_size = 0),
            UpdateError::MissingIntegrityMetadata
        );
        assert_eq!(
            refused(&|update| update.asset_size = MAX_RELEASE_ASSET_BYTES + 1),
            UpdateError::MissingIntegrityMetadata
        );
        assert_eq!(
            refused(&|update| update.asset_sha256 = String::new()),
            UpdateError::MissingIntegrityMetadata
        );
        assert_eq!(
            refused(
                &|update| update.asset_url = "https://example.com/usage-tray-rs.exe".to_string()
            ),
            UpdateError::InvalidDownloadAddress
        );

        assert!(host.downloads.borrow().is_empty());
        assert_eq!(directory.names(), [EXECUTABLE_NAME, UNRELATED_NAME]);
    }

    #[test]
    fn a_failed_or_wrong_download_leaves_nothing_behind() {
        let (directory, installation) = installed("stage-failures");
        let expected = new_executable();
        let attempt = |downloaded: Option<Vec<u8>>| {
            let host = FakeHost::new(downloaded);
            let error = stage(&installable_update(&expected), &installation, &host).unwrap_err();
            assert_eq!(directory.names(), [EXECUTABLE_NAME, UNRELATED_NAME]);
            assert_eq!(
                fs::read(&installation.executable).unwrap(),
                old_executable()
            );
            error
        };

        assert_eq!(attempt(None), UpdateError::DownloadFailed);

        let mut tampered = expected.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert_eq!(attempt(Some(tampered)), UpdateError::DigestMismatch);

        let truncated = expected[..expected.len() - 1].to_vec();
        assert_eq!(attempt(Some(truncated)), UpdateError::SizeMismatch);

        let mut oversized = expected.clone();
        oversized.push(0);
        assert_eq!(attempt(Some(oversized)), UpdateError::DownloadFailed);

        // A file that matches its published digest but is not a program.
        let text = b"not a PE".to_vec();
        let host = FakeHost::new(Some(text.clone()));
        assert_eq!(
            stage(&installable_update(&text), &installation, &host).unwrap_err(),
            UpdateError::NotExecutable
        );
        assert_eq!(directory.names(), [EXECUTABLE_NAME, UNRELATED_NAME]);
    }

    #[test]
    fn staging_clears_what_an_interrupted_update_left_behind() {
        let (directory, installation) = installed("stage-leftovers");
        let old_id = "0123456789abcdef0123456789abcdef";
        let leftovers = [
            format!(".{EXECUTABLE_NAME}.update-{old_id}.tmp"),
            format!(".{EXECUTABLE_NAME}.update-{old_id}.old"),
            format!(".{EXECUTABLE_NAME}.update-{old_id}.failed"),
            format!(".{EXECUTABLE_NAME}.update-{old_id}.ok"),
            format!(".{EXECUTABLE_NAME}.update-write-{old_id}.backup"),
        ];
        let kept = [
            // Another program's update, an unknown kind of file, and a folder.
            format!(".other.exe.update-{old_id}.tmp"),
            format!(".{EXECUTABLE_NAME}.update-{old_id}.log"),
            format!(".{EXECUTABLE_NAME}.update-fedcba9876543210fedcba9876543210.old"),
        ];
        for name in leftovers.iter().chain(&kept[..2]) {
            fs::write(directory.file(name), "leftover").unwrap();
        }
        fs::create_dir(directory.file(&kept[2])).unwrap();
        fs::write(directory.file(&kept[2]).join(EXECUTABLE_NAME), "unrelated").unwrap();

        let host = FakeHost::new(Some(new_executable()));
        let staged = stage(&installable_update(&new_executable()), &installation, &host).unwrap();
        drop(staged);

        let mut expected: Vec<String> = kept.to_vec();
        expected.extend([EXECUTABLE_NAME.to_string(), UNRELATED_NAME.to_string()]);
        expected.sort();
        assert_eq!(directory.names(), expected);
        assert_eq!(
            fs::read(directory.file(&kept[2]).join(EXECUTABLE_NAME)).unwrap(),
            b"unrelated"
        );
    }

    #[test]
    fn only_this_executables_update_files_count_as_leftovers() {
        let id = "0123456789abcdef0123456789abcdef";
        let is_leftover = |name: String| is_update_file_name(&name, EXECUTABLE_NAME);
        for suffix in UPDATE_FILE_SUFFIXES {
            assert!(is_leftover(format!(
                ".{EXECUTABLE_NAME}.update-{id}.{suffix}"
            )));
        }
        for suffix in PROBE_FILE_SUFFIXES {
            assert!(is_leftover(format!(
                ".{EXECUTABLE_NAME}.update-write-{id}.{suffix}"
            )));
        }
        for name in [
            EXECUTABLE_NAME.to_string(),
            format!("{EXECUTABLE_NAME}.update-{id}.tmp"),
            format!(".{EXECUTABLE_NAME}.update-{id}"),
            format!(".{EXECUTABLE_NAME}.update-{id}.tmp.bak"),
            format!(".{EXECUTABLE_NAME}.update-{id}.exe"),
            format!(".{EXECUTABLE_NAME}.update-{}.tmp", &id[1..]),
            format!(".{EXECUTABLE_NAME}.update-{}.tmp", "g".repeat(32)),
            format!(".{EXECUTABLE_NAME}.update-{id}.target"),
            format!(".{EXECUTABLE_NAME}.update-write-{id}.tmp"),
            format!(".CodexUsageTray.exe.update-{id}.tmp"),
        ] {
            assert!(!is_leftover(name.clone()), "{name}");
        }
    }

    #[test]
    fn a_folder_that_cannot_swap_files_is_refused_before_downloading() {
        let (directory, installation) = installed("stage-preflight");
        let host = FakeHost::new(Some(new_executable()));
        // The name the preflight will use for its first file is taken.
        let blocked = directory.file(&format!(
            ".{EXECUTABLE_NAME}.update-write-{}.target",
            "0".repeat(31) + "1"
        ));
        fs::create_dir(&blocked).unwrap();

        assert_eq!(
            stage(&installable_update(&new_executable()), &installation, &host).unwrap_err(),
            UpdateError::FolderNotReplaceable
        );
        assert!(host.downloads.borrow().is_empty());
        assert!(blocked.is_dir());
        assert_eq!(directory.names().len(), 3);
    }

    #[test]
    fn start_arguments_select_the_kind_of_start() {
        let executable = Path::new(r"C:\Apps\usage-tray-rs.exe");
        let kind = |values: &[&str]| startup(&arguments(values), executable);

        assert_eq!(kind(&[]), Startup::Normal);
        assert_eq!(kind(&["--popup", "1", "2"]), Startup::Normal);
        assert_eq!(
            kind(&[CONFIRM_ARGUMENT, FIRST_ID]),
            Startup::Updated(Confirmation {
                files: UpdateFiles::new(executable, FIRST_ID).unwrap()
            })
        );
        // An id is never used as given unless it is one this protocol makes.
        assert_eq!(kind(&[CONFIRM_ARGUMENT]), Startup::Normal);
        assert_eq!(kind(&[CONFIRM_ARGUMENT, r"..\..\other"]), Startup::Normal);
        assert_eq!(kind(&[CONFIRM_ARGUMENT, &FIRST_ID[1..]]), Startup::Normal);

        assert_eq!(
            kind(&[FAILED_ARGUMENT, "exited"]),
            Startup::UpdateFailed(InstallFailure::ExitedEarly)
        );
        assert_eq!(
            kind(&["--show-settings", "--UPDATE-FAILED"]),
            Startup::UpdateFailed(InstallFailure::Unknown)
        );
        assert_eq!(
            kind(&[FAILED_ARGUMENT, "something new"]),
            Startup::UpdateFailed(InstallFailure::Unknown)
        );
        for failure in [
            InstallFailure::StagedFileChanged,
            InstallFailure::ReplaceFailed,
            InstallFailure::StartFailed,
            InstallFailure::ExitedEarly,
            InstallFailure::TimedOut,
            InstallFailure::Unknown,
        ] {
            assert_eq!(
                kind(&[FAILED_ARGUMENT, failure.code()]),
                Startup::UpdateFailed(failure)
            );
        }
    }

    #[test]
    fn a_confirmed_update_replaces_the_executable() {
        let (directory, installation, host, staged) = staged_update("apply-confirmed");
        let host = host.launching(&[Launch::Confirms]);
        let files = update_files(&installation);

        assert_eq!(apply_and_restart(staged, &host), ApplyOutcome::Installed);

        assert_eq!(
            host.started(),
            [(
                installation.executable.clone(),
                arguments(&[CONFIRM_ARGUMENT, FIRST_ID])
            )]
        );
        assert_eq!(
            fs::read(&installation.executable).unwrap(),
            new_executable()
        );
        assert_eq!(fs::read(&files.previous).unwrap(), old_executable());
        assert!(files.confirmed.is_file());
        assert!(!path_exists(&files.staged));
        assert_eq!(host.stopped.get(), 0);

        // The new process then removes what the old one left for it.
        let Startup::Updated(confirmation) =
            startup(&host.started()[0].1, &installation.executable)
        else {
            panic!("expected an updated start");
        };
        assert!(confirmation.remove_previous(&host));
        assert_eq!(directory.names(), [EXECUTABLE_NAME, UNRELATED_NAME]);
        assert_eq!(fs::read(directory.file(UNRELATED_NAME)).unwrap(), b"keep");
    }

    #[test]
    fn a_confirmation_counts_even_if_the_new_app_is_closed_right_away() {
        let (_directory, installation, host, staged) = staged_update("apply-closed");
        let host = host.launching(&[Launch::ConfirmsThenExits]);

        assert_eq!(apply_and_restart(staged, &host), ApplyOutcome::Installed);
        assert_eq!(
            fs::read(&installation.executable).unwrap(),
            new_executable()
        );
        assert_eq!(host.started().len(), 1);
    }

    #[test]
    fn an_update_that_exits_during_startup_is_rolled_back() {
        let (directory, installation, host, staged) = staged_update("apply-exits");
        let host = host.launching(&[Launch::Exits, Launch::Runs]);

        assert_eq!(
            apply_and_restart(staged, &host),
            ApplyOutcome::RolledBack(InstallFailure::ExitedEarly)
        );

        assert_eq!(
            fs::read(&installation.executable).unwrap(),
            old_executable()
        );
        assert_eq!(directory.names(), [EXECUTABLE_NAME, UNRELATED_NAME]);
        assert_eq!(fs::read(directory.file(UNRELATED_NAME)).unwrap(), b"keep");
        let started = host.started();
        assert_eq!(
            started[1],
            (
                installation.executable.clone(),
                arguments(&[FAILED_ARGUMENT, "exited"])
            )
        );
        // The restored app learns why it was started.
        assert_eq!(
            startup(&started[1].1, &installation.executable),
            Startup::UpdateFailed(InstallFailure::ExitedEarly)
        );
        assert_eq!(
            host.slept.get(),
            u64::from(STARTUP_VERIFICATION_MILLISECONDS)
        );
    }

    #[test]
    fn an_update_that_never_confirms_is_stopped_and_rolled_back() {
        let (directory, installation, host, staged) = staged_update("apply-timeout");
        let host = host.launching(&[Launch::Runs, Launch::Runs]);

        assert_eq!(
            apply_and_restart(staged, &host),
            ApplyOutcome::RolledBack(InstallFailure::TimedOut)
        );

        assert_eq!(host.stopped.get(), 1);
        assert_eq!(
            fs::read(&installation.executable).unwrap(),
            old_executable()
        );
        assert_eq!(directory.names(), [EXECUTABLE_NAME, UNRELATED_NAME]);
        assert_eq!(
            host.started()[1].1,
            arguments(&[FAILED_ARGUMENT, "timeout"])
        );
        assert_eq!(
            host.slept.get(),
            u64::from(CONFIRMATION_TIMEOUT_MILLISECONDS + STARTUP_VERIFICATION_MILLISECONDS)
        );
    }

    #[test]
    fn an_update_that_cannot_be_started_is_rolled_back() {
        let (directory, installation, host, staged) = staged_update("apply-no-start");
        let host = host.launching(&[Launch::Fails, Launch::Runs]);

        assert_eq!(
            apply_and_restart(staged, &host),
            ApplyOutcome::RolledBack(InstallFailure::StartFailed)
        );

        assert_eq!(
            fs::read(&installation.executable).unwrap(),
            old_executable()
        );
        assert_eq!(directory.names(), [EXECUTABLE_NAME, UNRELATED_NAME]);
        assert_eq!(host.started()[1].1, arguments(&[FAILED_ARGUMENT, "start"]));
    }

    #[test]
    fn a_download_changed_after_staging_is_never_installed() {
        let (directory, installation, host, staged) = staged_update("apply-tampered");
        let mut tampered = new_executable();
        *tampered.last_mut().unwrap() ^= 1;
        fs::write(&staged.files.staged, tampered).unwrap();

        assert_eq!(
            apply_and_restart(staged, &host),
            ApplyOutcome::RolledBack(InstallFailure::StagedFileChanged)
        );

        assert_eq!(
            fs::read(&installation.executable).unwrap(),
            old_executable()
        );
        assert_eq!(directory.names(), [EXECUTABLE_NAME, UNRELATED_NAME]);
        // Only the unchanged app was started, never the download.
        assert_eq!(
            host.started(),
            [(
                installation.executable.clone(),
                arguments(&[FAILED_ARGUMENT, "staged"])
            )]
        );
    }

    #[test]
    fn an_executable_that_cannot_be_moved_stays_installed() {
        let (directory, installation, host, staged) = staged_update("apply-locked");
        let lock = lock_against_moving(&installation.executable);

        assert_eq!(
            apply_and_restart(staged, &host),
            ApplyOutcome::RolledBack(InstallFailure::ReplaceFailed)
        );
        drop(lock);

        assert_eq!(
            fs::read(&installation.executable).unwrap(),
            old_executable()
        );
        assert_eq!(directory.names(), [EXECUTABLE_NAME, UNRELATED_NAME]);
        assert_eq!(
            host.started(),
            [(
                installation.executable.clone(),
                arguments(&[FAILED_ARGUMENT, "replace"])
            )]
        );
        let retries = u64::from((REPLACE_ATTEMPTS - 1) * REPLACE_RETRY_MILLISECONDS);
        assert_eq!(
            host.slept.get(),
            retries + u64::from(STARTUP_VERIFICATION_MILLISECONDS)
        );
    }

    #[test]
    fn a_download_that_cannot_be_moved_into_place_restores_the_executable() {
        let (directory, installation, host, staged) = staged_update("apply-locked-download");
        let lock = lock_against_moving(&staged.files.staged);

        assert_eq!(
            apply_and_restart(staged, &host),
            ApplyOutcome::RolledBack(InstallFailure::ReplaceFailed)
        );
        assert_eq!(
            fs::read(&installation.executable).unwrap(),
            old_executable()
        );
        assert_eq!(
            host.started()[0].1,
            arguments(&[FAILED_ARGUMENT, "replace"])
        );

        // The download could not be deleted while it was held open.
        drop(lock);
        fs::remove_file(update_files(&installation).staged).unwrap();
        assert_eq!(directory.names(), [EXECUTABLE_NAME, UNRELATED_NAME]);
    }

    #[test]
    fn a_briefly_held_executable_is_moved_on_a_later_attempt() {
        let (_directory, installation, host, staged) = staged_update("apply-retry");
        let host = host.launching(&[Launch::Confirms]);
        let mut lock = Some(lock_against_moving(&installation.executable));
        *host.on_sleep.borrow_mut() = Some(Box::new(move || drop(lock.take())));

        assert_eq!(apply_and_restart(staged, &host), ApplyOutcome::Installed);
        assert_eq!(
            fs::read(&installation.executable).unwrap(),
            new_executable()
        );
        assert_eq!(host.slept.get(), u64::from(REPLACE_RETRY_MILLISECONDS));
    }

    #[test]
    fn a_new_app_that_cannot_be_stopped_is_left_in_place() {
        let (_directory, installation, host, staged) = staged_update("apply-unstoppable");
        let host = host.launching(&[Launch::CannotBeStopped]);
        let files = update_files(&installation);

        assert_eq!(
            apply_and_restart(staged, &host),
            ApplyOutcome::RecoveryFailed(InstallFailure::TimedOut)
        );

        // Swapping files under a running app would leave two versions active.
        assert_eq!(
            fs::read(&installation.executable).unwrap(),
            new_executable()
        );
        assert_eq!(fs::read(&files.previous).unwrap(), old_executable());
        assert_eq!(host.started().len(), 1);
    }

    #[test]
    fn a_restored_app_that_does_not_stay_running_is_reported() {
        let (_directory, installation, host, staged) = staged_update("apply-restart-exits");
        let host = host.launching(&[Launch::Exits, Launch::Exits]);
        assert_eq!(
            apply_and_restart(staged, &host),
            ApplyOutcome::RecoveryFailed(InstallFailure::ExitedEarly)
        );
        assert_eq!(
            fs::read(&installation.executable).unwrap(),
            old_executable()
        );

        let (_directory, installation, host, staged) = staged_update("apply-restart-fails");
        let host = host.launching(&[Launch::Fails, Launch::Fails]);
        assert_eq!(
            apply_and_restart(staged, &host),
            ApplyOutcome::RecoveryFailed(InstallFailure::StartFailed)
        );
        assert_eq!(
            fs::read(&installation.executable).unwrap(),
            old_executable()
        );
        assert_eq!(host.started().len(), 2);
    }

    #[test]
    fn the_previous_executable_is_removed_once_its_process_lets_go() {
        let (directory, installation, host, staged) = staged_update("confirm-cleanup");
        let host = host.launching(&[Launch::Confirms]);
        let files = update_files(&installation);
        assert_eq!(apply_and_restart(staged, &host), ApplyOutcome::Installed);
        let confirmation = Confirmation {
            files: files.clone(),
        };

        // Held the way a still-running process holds its executable.
        let lock = lock_against_moving(&files.previous);
        assert!(!confirmation.remove_previous(&host));
        assert!(files.previous.is_file());
        // The old process may not have seen the confirmation yet.
        assert!(files.confirmed.is_file());
        assert_eq!(
            host.slept.get(),
            u64::from((CLEANUP_ATTEMPTS - 1) * CLEANUP_RETRY_MILLISECONDS)
        );

        drop(lock);
        assert!(confirmation.remove_previous(&host));
        assert_eq!(directory.names(), [EXECUTABLE_NAME, UNRELATED_NAME]);
    }

    #[test]
    fn confirming_tells_the_old_process_and_then_removes_its_executable() {
        let (directory, installation) = installed("confirm");
        let files = update_files(&installation);
        fs::write(&files.previous, old_executable()).unwrap();
        let lock = lock_against_moving(&files.previous);

        Confirmation {
            files: files.clone(),
        }
        .confirm();
        // The confirmation is there at once; the executable goes when it can.
        assert!(files.confirmed.is_file() && files.previous.is_file());

        drop(lock);
        let cleaned = (0..200).any(|_| {
            SystemHost.sleep(POLL_MILLISECONDS);
            !path_exists(&files.previous) && !path_exists(&files.confirmed)
        });
        assert!(cleaned);
        assert_eq!(directory.names(), [EXECUTABLE_NAME, UNRELATED_NAME]);
    }

    #[test]
    fn a_confirmation_that_comes_too_late_does_not_survive_the_rollback() {
        let (directory, installation, host, staged) = staged_update("apply-late");
        let host = host.launching(&[Launch::ConfirmsWhileBeingStopped, Launch::Runs]);

        assert_eq!(
            apply_and_restart(staged, &host),
            ApplyOutcome::RolledBack(InstallFailure::TimedOut)
        );

        assert_eq!(host.stopped.get(), 1);
        assert_eq!(
            fs::read(&installation.executable).unwrap(),
            old_executable()
        );
        assert_eq!(directory.names(), [EXECUTABLE_NAME, UNRELATED_NAME]);
    }

    #[test]
    fn names_already_in_use_are_never_overwritten() {
        // A folder is not a leftover, so staging does not clear it away.
        let (directory, installation) = installed("stage-name-taken");
        let taken = update_files(&installation).previous;
        fs::create_dir(&taken).unwrap();
        let host = FakeHost::new(Some(new_executable()));
        assert_eq!(
            stage(&installable_update(&new_executable()), &installation, &host).unwrap_err(),
            UpdateError::PreparationFailed
        );
        assert!(host.downloads.borrow().is_empty());
        assert_eq!(directory.names().len(), 3);

        // Something that takes a name after staging stops the swap. A stray
        // confirmation in particular must not pass for the new app's own.
        for name in ["old", "failed", "ok"] {
            let (directory, installation, host, staged) = staged_update("apply-name-taken");
            let taken = update_files(&installation)
                .target
                .with_file_name(format!(".{EXECUTABLE_NAME}.update-{FIRST_ID}.{name}"));
            fs::write(&taken, "not an update file").unwrap();
            assert_eq!(
                apply_and_restart(staged, &host),
                ApplyOutcome::RolledBack(InstallFailure::ReplaceFailed),
                "{name}"
            );
            assert_eq!(
                fs::read(&installation.executable).unwrap(),
                old_executable()
            );
            assert_eq!(fs::read(&taken).unwrap(), b"not an update file");
            assert_eq!(directory.names().len(), 3);
            assert_eq!(
                host.started()[0].1,
                arguments(&[FAILED_ARGUMENT, "replace"])
            );
        }
    }

    #[test]
    fn a_rollback_that_cannot_restore_still_leaves_an_executable_installed() {
        let (_directory, installation, host, staged) = staged_update("apply-restore-blocked");
        let host = host.launching(&[Launch::ExitsWithPreviousHeld]);
        let files = update_files(&installation);

        assert_eq!(
            apply_and_restart(staged, &host),
            ApplyOutcome::RecoveryFailed(InstallFailure::ExitedEarly)
        );

        assert_eq!(
            fs::read(&installation.executable).unwrap(),
            new_executable()
        );
        assert_eq!(fs::read(&files.previous).unwrap(), old_executable());
        assert!(!path_exists(&files.failed));
        assert_eq!(host.started().len(), 1);
    }

    #[test]
    fn a_confirmation_that_may_still_be_awaited_is_not_cleared() {
        let (directory, installation) = installed("stage-awaited");
        let files = update_files(&installation);
        fs::write(&files.previous, old_executable()).unwrap();
        fs::write(&files.confirmed, "").unwrap();
        // Held the way a still-running process holds its executable.
        let lock = lock_against_moving(&files.previous);

        remove_stale_files(&installation.executable);
        assert!(files.previous.is_file() && files.confirmed.is_file());

        drop(lock);
        remove_stale_files(&installation.executable);
        assert_eq!(directory.names(), [EXECUTABLE_NAME, UNRELATED_NAME]);
    }

    #[test]
    fn restarts_wait_for_settings_like_the_existing_app() {
        let closed = SettingsWindow::Closed;
        let unchanged = SettingsWindow::Open {
            unsaved_changes: false,
        };
        let edited = SettingsWindow::Open {
            unsaved_changes: true,
        };
        assert!(!should_wait_for_settings(closed, Trigger::Automatic));
        assert!(!should_wait_for_settings(closed, Trigger::Manual));
        // Background updates wait for the window to close.
        assert!(should_wait_for_settings(unchanged, Trigger::Automatic));
        assert!(should_wait_for_settings(edited, Trigger::Automatic));
        // A requested update restarts unless that would lose changes.
        assert!(!should_wait_for_settings(unchanged, Trigger::Manual));
        assert!(should_wait_for_settings(edited, Trigger::Manual));
    }

    #[test]
    fn automatic_checks_run_at_startup_and_then_hourly() {
        let mut session = Session::new(&Startup::Normal);
        let start = 1_800_000_000;
        let up_to_date = || {
            let mut info = installable_update(&new_executable());
            info.update_available = false;
            info.can_install = false;
            info.latest_version = CURRENT_VERSION.to_string();
            Ok(info)
        };
        assert_eq!(session.status(), "");
        assert_eq!(session.busy_label(), None);

        assert_eq!(
            session.request_check(Trigger::Automatic, start),
            CheckRequest::Start
        );
        assert_eq!(session.status(), "Checking for updates...");
        assert_eq!(session.busy_label(), Some("Checking..."));
        session.check_finished(up_to_date());
        assert_eq!(session.status(), "Up to date: 1.0.0");
        assert_eq!(session.busy_label(), None);

        assert_eq!(
            session.request_check(Trigger::Automatic, start + 60),
            CheckRequest::NotDue
        );
        assert_eq!(
            session.request_check(Trigger::Automatic, start + CHECK_INTERVAL_SECONDS - 1),
            CheckRequest::NotDue
        );
        assert_eq!(session.status(), "Up to date: 1.0.0");
        assert_eq!(
            session.request_check(Trigger::Automatic, start + CHECK_INTERVAL_SECONDS),
            CheckRequest::Start
        );
        session.check_finished(up_to_date());

        // A clock set far back does not postpone the next check for long.
        assert_eq!(
            session.request_check(Trigger::Automatic, start - 86_400),
            CheckRequest::Start
        );
    }

    #[test]
    fn a_manual_check_runs_immediately_unless_one_is_running() {
        let mut session = Session::new(&Startup::Normal);
        assert_eq!(
            session.request_check(Trigger::Automatic, 0),
            CheckRequest::Start
        );
        assert_eq!(
            session.request_check(Trigger::Manual, 1),
            CheckRequest::Busy
        );
        let (trigger, outcome) = session.check_finished(Err(UpdateError::GitHubUnreachable));
        assert_eq!(trigger, Trigger::Automatic);
        assert_eq!(
            outcome,
            CheckOutcome::Failed {
                message: UpdateError::GitHubUnreachable.message().to_string()
            }
        );
        assert_eq!(session.status(), "Update check failed.");

        // Not due as an automatic check, but asked for.
        assert_eq!(
            session.request_check(Trigger::Automatic, 2),
            CheckRequest::NotDue
        );
        assert_eq!(
            session.request_check(Trigger::Manual, 2),
            CheckRequest::Start
        );
        // The hourly check that falls due meanwhile is skipped, not queued.
        assert_eq!(
            session.request_check(Trigger::Automatic, CHECK_INTERVAL_SECONDS),
            CheckRequest::Busy
        );
        let (trigger, _) = session.check_finished(Err(UpdateError::InvalidReleaseData));
        assert_eq!(trigger, Trigger::Manual);
        assert_eq!(
            session.request_check(Trigger::Automatic, CHECK_INTERVAL_SECONDS + 1),
            CheckRequest::NotDue
        );
    }

    #[test]
    fn check_results_say_what_to_do_next() {
        let mut session = Session::new(&Startup::Normal);
        let installable = installable_update(&new_executable());

        session.request_check(Trigger::Manual, 0);
        let mut current = installable.clone();
        current.update_available = false;
        current.can_install = false;
        current.message = "You are on the latest published version (2.0.0).".to_string();
        assert_eq!(
            session.check_finished(Ok(current.clone())),
            (
                Trigger::Manual,
                CheckOutcome::UpToDate {
                    message: current.message
                }
            )
        );
        assert_eq!(session.status(), "Up to date: 2.0.0");

        session.request_check(Trigger::Manual, 0);
        let mut incompatible = installable.clone();
        incompatible.can_install = false;
        incompatible.message = "Version 2.0.0 does not include usage-tray-rs.exe. No compatible update is available yet.".to_string();
        assert_eq!(
            session.check_finished(Ok(incompatible.clone())),
            (
                Trigger::Manual,
                CheckOutcome::NotInstallable {
                    message: incompatible.message.clone(),
                    release_url: incompatible.release_url
                }
            )
        );
        assert_eq!(session.status(), incompatible.message);
        assert_eq!(session.busy_label(), None);

        session.request_check(Trigger::Automatic, 0);
        assert_eq!(
            session.check_finished(Ok(installable.clone())),
            (Trigger::Automatic, CheckOutcome::Install(installable))
        );
        assert_eq!(session.status(), "Installing version 2.0.0...");
        assert_eq!(session.busy_label(), Some("Installing..."));
        assert_eq!(
            session.request_check(Trigger::Manual, 1),
            CheckRequest::Busy
        );
    }

    #[test]
    fn a_background_update_waits_until_settings_closes() {
        let (directory, installation, _host, staged) = staged_update("session-background");
        let files = update_files(&installation);
        let mut session = Session::new(&Startup::Normal);
        session.request_check(Trigger::Automatic, 0);
        session.check_finished(Ok(installable_update(&new_executable())));
        assert!(!session.ready.is_some());
        assert!(session.take_ready(SettingsWindow::Closed).is_none());

        assert_eq!(
            session.stage_finished(Ok(staged)),
            (Trigger::Automatic, StageOutcome::Ready)
        );
        let open = SettingsWindow::Open {
            unsaved_changes: false,
        };
        assert!(session.take_ready(open).is_none());
        assert!(session.take_ready(open).is_none());
        assert_eq!(session.status(), "Update ready. Close Settings to restart.");
        // Waiting keeps the download, and nothing else starts meanwhile.
        assert!(session.ready.is_some() && files.staged.is_file());
        assert_eq!(session.busy_label(), Some("Installing..."));
        assert_eq!(
            session.request_check(Trigger::Automatic, CHECK_INTERVAL_SECONDS),
            CheckRequest::Busy
        );

        let ready = session.take_ready(SettingsWindow::Closed).unwrap();
        assert_eq!(ready.files, files);
        assert!(!session.ready.is_some());
        assert!(session.take_ready(SettingsWindow::Closed).is_none());
        // Whoever took the update now owns its download.
        drop(session);
        assert!(files.staged.is_file());
        drop(ready);
        assert_eq!(directory.names(), [EXECUTABLE_NAME, UNRELATED_NAME]);
    }

    #[test]
    fn a_requested_update_waits_only_for_unsaved_changes() {
        let (_directory, _installation, _host, staged) = staged_update("session-manual");
        let mut session = Session::new(&Startup::Normal);
        session.request_check(Trigger::Manual, 0);
        session.check_finished(Ok(installable_update(&new_executable())));
        assert_eq!(
            session.stage_finished(Ok(staged)),
            (Trigger::Manual, StageOutcome::Ready)
        );

        let edited = SettingsWindow::Open {
            unsaved_changes: true,
        };
        assert!(session.take_ready(edited).is_none());
        assert_eq!(
            session.status(),
            "Update ready. Save or discard your Settings changes to restart."
        );
        let saved = SettingsWindow::Open {
            unsaved_changes: false,
        };
        assert!(session.take_ready(saved).is_some());
    }

    #[test]
    fn shutting_down_discards_a_waiting_update() {
        let (directory, _installation, _host, staged) = staged_update("session-shutdown");
        let mut session = Session::new(&Startup::Normal);
        session.request_check(Trigger::Automatic, 0);
        session.check_finished(Ok(installable_update(&new_executable())));
        session.stage_finished(Ok(staged));
        assert!(session
            .take_ready(SettingsWindow::Open {
                unsaved_changes: false
            })
            .is_none());
        assert_eq!(directory.names().len(), 3);

        drop(session);
        assert_eq!(directory.names(), [EXECUTABLE_NAME, UNRELATED_NAME]);
    }

    #[test]
    fn a_failed_download_shows_its_reason_and_allows_a_retry() {
        let mut session = Session::new(&Startup::Normal);
        session.request_check(Trigger::Automatic, 0);
        session.check_finished(Ok(installable_update(&new_executable())));

        let (trigger, outcome) = session.stage_finished(Err(UpdateError::DigestMismatch));
        assert_eq!(trigger, Trigger::Automatic);
        assert_eq!(
            outcome,
            StageOutcome::Failed {
                message: UpdateError::DigestMismatch.message().to_string()
            }
        );
        assert_eq!(
            session.status(),
            "Update installation failed: The update failed SHA-256 verification. No files were replaced."
        );
        assert_eq!(session.busy_label(), None);
        assert!(!session.ready.is_some());
        assert_eq!(
            session.request_check(Trigger::Manual, 1),
            CheckRequest::Start
        );
    }

    #[test]
    fn automatic_checks_pause_for_the_session_after_a_failed_update() {
        let paused = "Update installation failed: The updated app exited during startup. Automatic updates paused after a failed update. Check updates to retry.";
        let mut session = Session::new(&Startup::UpdateFailed(InstallFailure::ExitedEarly));
        assert_eq!(session.status(), paused);

        assert_eq!(
            session.request_check(Trigger::Automatic, 0),
            CheckRequest::Paused
        );
        assert_eq!(
            session.request_check(Trigger::Automatic, 1),
            CheckRequest::NotDue
        );
        assert_eq!(session.busy_label(), None);

        // Checking by hand still works, and does not lift the pause.
        assert_eq!(
            session.request_check(Trigger::Manual, 2),
            CheckRequest::Start
        );
        session.check_finished(Err(UpdateError::GitHubUnreachable));
        assert_eq!(session.status(), "Update check failed.");
        assert_eq!(
            session.request_check(Trigger::Automatic, CHECK_INTERVAL_SECONDS),
            CheckRequest::Paused
        );
        assert_eq!(session.status(), paused);

        // A normal start, or one after a successful update, is not paused.
        let updated = startup(
            &arguments(&[CONFIRM_ARGUMENT, FIRST_ID]),
            Path::new(r"C:\Apps\usage-tray-rs.exe"),
        );
        let mut session = Session::new(&updated);
        assert_eq!(session.status(), "");
        assert_eq!(
            session.request_check(Trigger::Automatic, 0),
            CheckRequest::Start
        );
    }

    #[test]
    fn update_ids_are_unpredictable_and_well_formed() {
        let first = SystemHost.update_id().unwrap();
        let second = SystemHost.update_id().unwrap();
        assert!(is_update_id(&first) && is_update_id(&second));
        assert_ne!(first, second);
        assert!(!is_update_id(""));
        assert!(!is_update_id(&first[1..]));
        assert!(!is_update_id(&format!("{}g", &first[1..])));
    }

    #[test]
    fn started_processes_report_their_exit_and_can_be_stopped() {
        let system = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
        let wait_for_exit = |process: &mut dyn Process| {
            (0..200).any(|_| {
                SystemHost.sleep(POLL_MILLISECONDS);
                process.has_exited()
            })
        };

        let mut quick = SystemHost
            .start(&system.join("cmd.exe"), &["/c", "exit", "3"])
            .unwrap();
        assert!(wait_for_exit(quick.as_mut()));
        assert!(quick.terminate());

        // Waits quietly for a signal that never comes.
        let signal = format!("UsageTrayRsTest{}", std::process::id());
        let mut waiting = SystemHost
            .start(&system.join("waitfor.exe"), &["/t", "60", &signal])
            .unwrap();
        assert!(!waiting.has_exited());
        assert!(waiting.terminate());
        assert!(waiting.has_exited());

        assert!(SystemHost
            .start(&system.join("no-such-program.exe"), &[])
            .is_none());
    }
}
