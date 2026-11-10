//! Claude subscription quota, read with the Claude CLI's existing OAuth login.
//!
//! [`ClaudeClient`] polls the subscription endpoints used by Claude Code and
//! OpenQuota; no model requests are sent. It renews an expiring login and
//! saves the rotated tokens back to the CLI's credentials file. Tokens travel
//! only to the two fixed HTTPS hosts below, with redirects disabled, and never
//! reach a status message, a cache file or an error value.

mod cache;
mod credentials;
mod files;

use std::path::PathBuf;
use std::ptr::null;

use serde_json::{json, Map, Value};
use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows_sys::Win32::Globalization::GetTimeFormatEx;
use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

use crate::http::{self, Request, Response};
use crate::model::Usage;
use crate::paths::{CLAUDE_ACCOUNT_FILE, CLAUDE_STATUS_LINE_FILE};
use crate::timefmt::{format_clock, now_unix, parse_rfc3339};
use crate::win::from_wide;

use credentials::{same_tokens, text, CredentialFile, Object};

pub use cache::handle_status_line;

/// The command-line argument that selects [`handle_status_line`].
pub const STATUS_LINE_ARGUMENT: &str = "--claude-statusline";

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const SCOPE: &str =
    "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
const BETA_HEADER: &str = "oauth-2025-04-20";
/// The largest credentials file or response body that is read.
const MAX_BYTES: usize = 65_536;
const TIMEOUT_MILLISECONDS: i32 = 10_000;

const POLL_SECONDS: i64 = 300;
const RETRY_SECONDS: i64 = 60;
const MIN_RATE_LIMIT_SECONDS: i64 = 300;
/// A login this close to its expiry is renewed before it is used.
const EXPIRY_MARGIN_SECONDS: i64 = 300;
const MAX_EXPIRES_IN_SECONDS: f64 = 31_536_000.0;

const RENEWAL_NEEDED: &str = "Claude sign-in needs renewal - showing saved usage";
const RATE_LIMITED: &str = "Claude refresh rate limited - showing saved usage";
const REFRESH_FAILED: &str = "Claude refresh failed - showing saved usage";
const ROTATION_UNSAVED: &str = "Claude login refresh could not be saved";
const USAGE_UPDATED: &str = "Claude usage updated";

/// Performs one HTTPS exchange, as [`http::send`] does. `None` is a transport
/// failure.
pub type Sender = Box<dyn Fn(&Request<'_>) -> Option<Response> + Send>;

/// Returns the current time in Unix seconds.
pub type Clock = Box<dyn Fn() -> i64 + Send>;

/// What to show for Claude after a read.
#[derive(Clone, Debug, PartialEq)]
pub struct ClaudeReading {
    /// The newest saved reading. Windows that have reset are already removed.
    pub usage: Usage,
    /// True when the reading is more than thirty minutes old or the last
    /// request failed. Forecasts should be withheld.
    pub stale: bool,
    /// A notice to show with the reading: why it is stale, or the outcome of
    /// a manual refresh.
    pub status: Option<String>,
}

/// Reads Claude quota and decides when the next request is due.
///
/// The client owns the whole schedule: five-minute polls, a one-minute retry
/// after a failure and the server's rate-limit cooldown. A caller only has to
/// call [`ClaudeClient::read`] regularly and show what it returns.
pub struct ClaudeClient {
    credentials: CredentialFile,
    cache_path: PathBuf,
    fallback_path: PathBuf,
    send: Sender,
    clock: Clock,
    /// Automatic reads before this time return the saved reading.
    next_poll: Option<i64>,
    /// Set by a 429 response. Until then even manual reads are not sent.
    rate_limit_until: Option<i64>,
    /// The notice for the most recent request, if it failed.
    failure: Option<&'static str>,
    pending: Option<PendingRotation>,
}

/// Rotated tokens that could not be written to the credentials file. The
/// server may already have invalidated the tokens they replace, so they are
/// used from memory and saved by a later read.
struct PendingRotation {
    /// The login the rotation started from, as it still is in the file.
    original: Object,
    rotation: Object,
}

/// Why a request produced no reading.
enum Failure {
    /// The server answered with a status other than 200.
    Status {
        status: u32,
        retry_after: http::RetryAfter,
    },
    /// A transport error, malformed data, or a file that could not be read
    /// or written.
    Other,
}

enum Fetched {
    /// A new reading is in the account cache.
    Stored,
    /// The login disappeared from the credentials file while it was renewed.
    LoginGone,
}

impl Default for ClaudeClient {
    /// A client for the signed-in user's CLI credentials and this app's data
    /// directory, using the real network and clock.
    fn default() -> Self {
        Self::new(
            CredentialFile::default_path(),
            cache::default_path(CLAUDE_ACCOUNT_FILE),
            cache::default_path(CLAUDE_STATUS_LINE_FILE),
            Box::new(http::send),
            Box::new(now_unix),
        )
    }
}

impl ClaudeClient {
    /// A client with every dependency supplied. `cache_path` receives the
    /// account readings; `fallback_path` is the status-line cache, which is
    /// only ever read.
    pub fn new(
        credentials_path: PathBuf,
        cache_path: PathBuf,
        fallback_path: PathBuf,
        send: Sender,
        clock: Clock,
    ) -> Self {
        Self {
            credentials: CredentialFile::new(credentials_path),
            cache_path,
            fallback_path,
            send,
            clock,
            next_poll: None,
            rate_limit_until: None,
            failure: None,
            pending: None,
        }
    }

    /// Returns the reading to show, requesting a new one from the account
    /// when a poll is due. `None` hides Claude: the CLI is not signed in, or
    /// no saved reading has a window that is still running.
    ///
    /// `force` is a manual refresh. It skips the wait between polls and the
    /// retry delay, but not a server rate limit; its result says either that
    /// usage was updated or when the rate limit ends. `request_started` is
    /// called once if, and just before, a request is sent.
    pub fn read(&mut self, force: bool, request_started: impl FnOnce()) -> Option<ClaudeReading> {
        let now = (self.clock)();
        let oauth = self.load_login()?;

        if let Some(until) = self.rate_limit_until.filter(|until| now < *until) {
            let mut saved = self.cached(now);
            if force {
                if let Some(saved) = &mut saved {
                    saved.status = Some(format!(
                        "Claude rate limited - retry at {}",
                        long_time(until)
                    ));
                }
            }
            return saved;
        }
        if !force && self.next_poll.is_some_and(|next_poll| now < next_poll) {
            return self.cached(now);
        }

        self.next_poll = Some(now.saturating_add(POLL_SECONDS));
        self.rate_limit_until = None;
        request_started();
        match self.fetch(oauth, now) {
            Ok(Fetched::Stored) => {
                self.failure = None;
                let mut updated = self.cached(now);
                if force {
                    if let Some(updated) = &mut updated {
                        updated
                            .status
                            .get_or_insert_with(|| USAGE_UPDATED.to_string());
                    }
                }
                updated
            }
            Ok(Fetched::LoginGone) => None,
            Err(failure) => {
                let failed_at = (self.clock)();
                self.failed(&failure, failed_at);
                self.cached(failed_at)
            }
        }
    }

    /// The login for this read, or `None` when the CLI is not signed in. A
    /// rotation that could not be saved earlier is saved now if the file
    /// still holds the login it replaces.
    fn load_login(&mut self) -> Option<Object> {
        let mut oauth = self.credentials.load_oauth().ok()??;
        access_token(&oauth)?;
        if let Some(pending) = self.pending.take() {
            // When the CLI has signed in again, the rotation is obsolete and
            // is dropped here.
            if same_tokens(Some(&oauth), Some(&pending.original)) {
                match self
                    .credentials
                    .persist_rotation(&pending.original, &pending.rotation)
                {
                    Ok(latest) => oauth = latest.unwrap_or_default(),
                    Err(_) => {
                        oauth = pending.rotation.clone();
                        self.pending = Some(pending);
                    }
                }
            }
        }
        Some(oauth)
    }

    /// Requests usage, renewing the login first when it is about to expire
    /// and once more if the server rejects it, and stores the reading.
    ///
    /// A login that vanished from the file is an empty object here. It has no
    /// access token, so the request ends without counting as a failure.
    fn fetch(&mut self, mut oauth: Object, now: i64) -> Result<Fetched, Failure> {
        let renewal_deadline = now.saturating_add(EXPIRY_MARGIN_SECONDS) as f64 * 1000.0;
        let expires_soon = oauth
            .get("expiresAt")
            .and_then(Value::as_f64)
            .is_some_and(|expiry| expiry > 0.0 && expiry <= renewal_deadline);
        if expires_soon {
            oauth = self.refresh(&oauth, now)?;
        }
        let Some(token) = access_token(&oauth) else {
            return Ok(Fetched::LoginGone);
        };
        let mut response = self.request_usage(token)?;
        if matches!(response.status, 401 | 403) {
            oauth = self.refresh(&oauth, now)?;
            let Some(token) = access_token(&oauth) else {
                return Ok(Fetched::LoginGone);
            };
            response = self.request_usage(token)?;
        }
        if response.status != 200 {
            return Err(Failure::from_response(&response));
        }
        let input = convert_usage(&response.body, now).ok_or(Failure::Other)?;
        if !cache::capture_status_line(&input, &self.cache_path, now) {
            return Err(Failure::Other);
        }
        Ok(Fetched::Stored)
    }

    /// Exchanges the refresh token for new tokens, saves them and returns the
    /// login to use from now on.
    fn refresh(&mut self, oauth: &Object, now: i64) -> Result<Object, Failure> {
        // Re-read before rotating: the CLI may already have renewed this generation.
        let latest = self.credentials.load_oauth().map_err(|_| Failure::Other)?;
        let pending_original = self.pending.as_ref().map(|pending| &pending.original);
        if !same_tokens(latest.as_ref(), Some(oauth))
            && !same_tokens(latest.as_ref(), pending_original)
        {
            return Ok(latest.unwrap_or_default());
        }
        let refresh_token = text(oauth, "refreshToken")
            .filter(|token| !token.trim().is_empty())
            .ok_or(Failure::Status {
                status: 401,
                retry_after: http::RetryAfter::Seconds(0),
            })?;
        let response = self.request_tokens(refresh_token)?;
        if response.status != 200 {
            return Err(Failure::from_response(&response));
        }
        let tokens: Value = serde_json::from_slice(&response.body).map_err(|_| Failure::Other)?;
        let token = |name: &str| {
            tokens
                .get(name)
                .and_then(Value::as_str)
                .filter(|token| !token.trim().is_empty())
        };

        let mut replacement = oauth.clone();
        let access = token("access_token").ok_or(Failure::Other)?;
        replacement.insert("accessToken".to_string(), access.into());
        if let Some(rotated) = token("refresh_token") {
            replacement.insert("refreshToken".to_string(), rotated.into());
        }
        let expires_in = tokens
            .get("expires_in")
            .and_then(Value::as_f64)
            .filter(|seconds| *seconds > 0.0 && *seconds <= MAX_EXPIRES_IN_SECONDS);
        match expires_in {
            Some(seconds) => {
                let expires_at = now
                    .saturating_mul(1000)
                    .saturating_add((seconds * 1000.0).round() as i64);
                replacement.insert("expiresAt".to_string(), expires_at.into());
            }
            // Expiry is optional. Keep the rotated tokens and renew on a later 401.
            None => {
                replacement.remove("expiresAt");
            }
        }

        let original = match self.pending.take() {
            Some(pending) => pending.original,
            None => oauth.clone(),
        };
        match self.credentials.persist_rotation(&original, &replacement) {
            Ok(latest) => Ok(latest.unwrap_or_default()),
            Err(_) => {
                // A rotated token can already invalidate the old token. Keep it in memory
                // and retry persistence, instead of rotating the obsolete token again.
                self.pending = Some(PendingRotation {
                    original,
                    rotation: replacement.clone(),
                });
                Ok(replacement)
            }
        }
    }

    /// Asks the usage endpoint, the only place the access token is sent.
    fn request_usage(&self, access_token: &str) -> Result<Response, Failure> {
        let authorization = format!("Bearer {access_token}");
        let headers = [
            ("Accept", "application/json"),
            ("Authorization", authorization.as_str()),
            ("anthropic-beta", BETA_HEADER),
        ];
        self.exchange(USAGE_URL, &headers, None)
    }

    /// Asks the token endpoint, the only place the refresh token is sent.
    fn request_tokens(&self, refresh_token: &str) -> Result<Response, Failure> {
        let body = json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": CLIENT_ID,
            "scope": SCOPE,
        })
        .to_string();
        let headers = [
            ("Accept", "application/json"),
            ("Content-Type", "application/json"),
        ];
        self.exchange(TOKEN_URL, &headers, Some(body.as_bytes()))
    }

    fn exchange(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<Response, Failure> {
        (self.send)(&Request {
            method: if body.is_some() { "POST" } else { "GET" },
            body,
            // A redirect would carry a token to a host other than the one named here.
            follow_redirects: false,
            timeout_milliseconds: TIMEOUT_MILLISECONDS,
            ..Request::get(url, headers, MAX_BYTES)
        })
        .ok_or(Failure::Other)
    }

    fn failed(&mut self, failure: &Failure, now: i64) {
        let (status, retry_after) = match *failure {
            Failure::Status {
                status,
                retry_after,
            } => (status, retry_after),
            Failure::Other => (0, http::RetryAfter::Seconds(0)),
        };
        let rate_limited = status == 429;
        let wait = if rate_limited {
            retry_after.delay_seconds(now).max(MIN_RATE_LIMIT_SECONDS)
        } else {
            RETRY_SECONDS
        };
        let next_poll = now.saturating_add(wait);
        self.next_poll = Some(next_poll);
        self.rate_limit_until = rate_limited.then_some(next_poll);
        self.failure = Some(match status {
            400 | 401 | 403 => RENEWAL_NEEDED,
            429 => RATE_LIMITED,
            _ => REFRESH_FAILED,
        });
    }

    /// The newer of the account reading and the status-line reading, marked
    /// with the outcome of the most recent request.
    fn cached(&self, now: i64) -> Option<ClaudeReading> {
        let saved = cache::read_snapshot(&self.cache_path, now);
        let fallback = cache::read_snapshot(&self.fallback_path, now);
        let newest = match (saved, fallback) {
            (Some(saved), Some(fallback)) if fallback.captured_ticks > saved.captured_ticks => {
                fallback
            }
            (saved, fallback) => saved.or(fallback)?,
        };
        let (stale, status) = match (self.failure, &self.pending) {
            (Some(failure), _) => (true, Some(failure)),
            (None, Some(_)) => (newest.stale, Some(ROTATION_UNSAVED)),
            (None, None) => (newest.stale, None),
        };
        Some(ClaudeReading {
            usage: newest.usage,
            stale,
            status: status.map(str::to_string),
        })
    }
}

impl Failure {
    fn from_response(response: &Response) -> Self {
        Self::Status {
            status: response.status,
            retry_after: response.retry_after,
        }
    }
}

/// The access token, unless it is missing or blank.
fn access_token(oauth: &Object) -> Option<&str> {
    text(oauth, "accessToken").filter(|token| !token.trim().is_empty())
}

/// Rewrites an account usage response as the status-line payload that
/// [`cache::capture_status_line`] stores, keeping each window that has a
/// valid percentage and a reset still ahead. `None` when no window qualifies.
fn convert_usage(body: &[u8], now: i64) -> Option<String> {
    let response: Value = serde_json::from_slice(body).ok()?;
    let mut limits = Map::new();
    for key in ["five_hour", "seven_day"] {
        let window = response.get(key);
        let used = window
            .and_then(|window| window.get("utilization"))
            .and_then(Value::as_f64)
            .filter(|used| (0.0..=100.0).contains(used));
        let reset = window
            .and_then(|window| window.get("resets_at"))
            .and_then(Value::as_str)
            .and_then(parse_rfc3339)
            .filter(|reset| *reset > now);
        if let (Some(used), Some(reset)) = (used, reset) {
            let window = json!({ "used_percentage": used, "resets_at": reset });
            limits.insert(key.to_string(), window);
        }
    }
    (!limits.is_empty()).then(|| json!({ "rate_limits": limits }).to_string())
}

/// Local time with seconds in the user's format, such as `8:38:05 PM`. A
/// rate limit ends on an arbitrary second, so a time without seconds would
/// read as already past for the rest of that minute.
fn long_time(unix: i64) -> String {
    const UNIX_EPOCH_AS_FILETIME_SECONDS: i64 = 11_644_473_600;
    unix.checked_add(UNIX_EPOCH_AS_FILETIME_SECONDS)
        .and_then(|seconds| seconds.checked_mul(10_000_000))
        .filter(|ticks| *ticks >= 0)
        .and_then(|ticks| {
            let file_time = FILETIME {
                dwLowDateTime: ticks as u32,
                dwHighDateTime: (ticks >> 32) as u32,
            };
            let mut utc = SYSTEMTIME::default();
            let mut local = SYSTEMTIME::default();
            let mut buffer = [0u16; 64];
            // SAFETY: every pointer refers to a live local, the buffer length
            // passed matches the buffer, and null selects the current time
            // zone, the user's locale and the locale's own format.
            let written = unsafe {
                if FileTimeToSystemTime(&file_time, &mut utc) == 0
                    || SystemTimeToTzSpecificLocalTime(null(), &utc, &mut local) == 0
                {
                    return None;
                }
                GetTimeFormatEx(
                    null(),
                    0,
                    &local,
                    null(),
                    buffer.as_mut_ptr(),
                    buffer.len() as i32,
                )
            };
            (written > 0).then(|| from_wide(&buffer))
        })
        .unwrap_or_else(|| format_clock(unix))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs::{self, File, OpenOptions};
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
    use std::sync::{Arc, Mutex};

    use super::files::tests::TestDirectory;

    const FILE_SHARE_READ: u32 = 1;
    const MINUTE: i64 = 60;
    const HOUR: i64 = 3_600;
    const DAY: i64 = 86_400;
    const ROTATED: &str =
        r#"{"access_token":"rotated-access","refresh_token":"rotated-refresh","expires_in":3600}"#;

    /// A request as the fake server saw it.
    #[derive(Clone)]
    struct Sent {
        url: String,
        method: String,
        headers: Vec<(String, String)>,
        body: Option<String>,
        has_user_agent: bool,
        follow_redirects: bool,
        max_bytes: usize,
        timeout_milliseconds: i32,
    }

    impl Sent {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(header, _)| header == name)
                .map(|(_, value)| value.as_str())
        }

        fn token(&self) -> Option<&str> {
            self.header("Authorization")?.strip_prefix("Bearer ")
        }
    }

    type Responder = Box<dyn FnMut(&Sent) -> Option<Response> + Send>;

    struct Fixture {
        directory: TestDirectory,
        credentials_path: PathBuf,
        cache_path: PathBuf,
        fallback_path: PathBuf,
        client: ClaudeClient,
        now: Arc<AtomicI64>,
        requests: Arc<Mutex<Vec<Sent>>>,
        responder: Arc<Mutex<Responder>>,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = TestDirectory::new("ClaudeQuotaTests");
            let credentials_path = directory.file("credentials.json");
            let cache_path = directory.file("account.json");
            let fallback_path = directory.file("fallback.json");
            let now = Arc::new(AtomicI64::new(
                parse_rfc3339("2026-09-28T12:00:00Z").unwrap(),
            ));
            let requests = Arc::new(Mutex::new(Vec::new()));
            let server_now = Arc::clone(&now);
            let responder: Arc<Mutex<Responder>> = Arc::new(Mutex::new(Box::new(move |_| {
                Some(success(server_now.load(Ordering::SeqCst)))
            })));

            let (seen, respond, clock_now) = (
                Arc::clone(&requests),
                Arc::clone(&responder),
                Arc::clone(&now),
            );
            let client = ClaudeClient::new(
                credentials_path.clone(),
                cache_path.clone(),
                fallback_path.clone(),
                Box::new(move |request| {
                    let sent = Sent {
                        url: request.url.to_string(),
                        method: request.method.to_string(),
                        headers: request
                            .headers
                            .iter()
                            .map(|(name, value)| (name.to_string(), value.to_string()))
                            .collect(),
                        body: request
                            .body
                            .map(|body| String::from_utf8(body.to_vec()).unwrap()),
                        has_user_agent: request.user_agent.is_some(),
                        follow_redirects: request.follow_redirects,
                        max_bytes: request.max_bytes,
                        timeout_milliseconds: request.timeout_milliseconds,
                    };
                    seen.lock().unwrap().push(sent.clone());
                    (respond.lock().unwrap())(&sent)
                }),
                Box::new(move || clock_now.load(Ordering::SeqCst)),
            );
            Self {
                directory,
                credentials_path,
                cache_path,
                fallback_path,
                client,
                now,
                requests,
                responder,
            }
        }

        fn now(&self) -> i64 {
            self.now.load(Ordering::SeqCst)
        }

        fn advance(&self, seconds: i64) {
            self.now.fetch_add(seconds, Ordering::SeqCst);
        }

        fn respond(&self, responder: impl FnMut(&Sent) -> Option<Response> + Send + 'static) {
            *self.responder.lock().unwrap() = Box::new(responder);
        }

        /// Answers every request with this status and `Retry-After` value.
        fn respond_status(&self, status: u32, retry_after_seconds: u64) {
            self.respond(move |_| {
                Some(Response {
                    retry_after: http::RetryAfter::Seconds(retry_after_seconds),
                    ..rejection(status)
                })
            });
        }

        /// Answers token requests with `tokens` and usage requests with a reading.
        fn respond_rotating(&self, tokens: &'static str) {
            let now = Arc::clone(&self.now);
            self.respond(move |request| {
                Some(match request.body {
                    Some(_) => ok(tokens),
                    None => success(now.load(Ordering::SeqCst)),
                })
            });
        }

        fn respond_success(&self) {
            let now = Arc::clone(&self.now);
            self.respond(move |_| Some(success(now.load(Ordering::SeqCst))));
        }

        fn requests(&self) -> Vec<Sent> {
            self.requests.lock().unwrap().clone()
        }

        fn request_count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }

        fn write_credentials(&self, access_token: &str, refresh_token: &str, expiry: i64) {
            write_credentials(&self.credentials_path, access_token, refresh_token, expiry);
        }

        fn saved_credentials(&self) -> Value {
            serde_json::from_str(&fs::read_to_string(&self.credentials_path).unwrap()).unwrap()
        }

        fn read(&mut self, force: bool) -> Option<ClaudeReading> {
            self.client.read(force, || {})
        }
    }

    fn write_credentials(path: &Path, access_token: &str, refresh_token: &str, expiry: i64) {
        let credentials = json!({
            "unrelatedSetting": "keep-root",
            "claudeAiOauth": {
                "accessToken": access_token,
                "refreshToken": refresh_token,
                "expiresAt": expiry * 1000,
                "subscriptionType": "max",
                "scopes": ["user:profile", "user:inference"],
            },
        });
        fs::write(path, credentials.to_string()).unwrap();
    }

    fn ok(body: &str) -> Response {
        Response {
            status: 200,
            body: body.as_bytes().to_vec(),
            retry_after: http::RetryAfter::Seconds(0),
        }
    }

    fn rejection(status: u32) -> Response {
        Response {
            status,
            body: Vec::new(),
            retry_after: http::RetryAfter::Seconds(0),
        }
    }

    /// A usage response with one window, a quarter used, that resets in two hours.
    fn success(now: i64) -> Response {
        ok(&usage_body("25", &iso(now + 2 * HOUR)))
    }

    fn usage_body(utilization: &str, resets_at: &str) -> String {
        format!(r#"{{"five_hour":{{"utilization":{utilization},"resets_at":"{resets_at}"}}}}"#)
    }

    /// Unix seconds in .NET's round-trip format, which the C# tests send.
    fn iso(unix: i64) -> String {
        let (days, seconds) = (unix.div_euclid(DAY), unix.rem_euclid(DAY));
        let shifted = days + 719_468;
        let (era, day_of_era) = (shifted.div_euclid(146_097), shifted.rem_euclid(146_097));
        let year_of_era =
            (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let shifted_month = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
        let month = if shifted_month < 10 {
            shifted_month + 3
        } else {
            shifted_month - 9
        };
        let year = year_of_era + era * 400 + i64::from(month <= 2);
        format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.0000000Z",
            seconds / HOUR,
            seconds % HOUR / MINUTE,
            seconds % MINUTE
        )
    }

    fn converted_windows(input: &str, now: i64) -> Map<String, Value> {
        let converted = convert_usage(input.as_bytes(), now).expect("valid windows are converted");
        let converted: Value = serde_json::from_str(&converted).unwrap();
        converted["rate_limits"].as_object().unwrap().clone()
    }

    #[test]
    fn the_test_timestamps_round_trip() {
        assert_eq!(iso(0), "1970-01-01T00:00:00.0000000Z");
        assert_eq!(iso(1_709_200_800), "2024-02-29T10:00:00.0000000Z");
        let now = Fixture::new().now();
        assert_eq!(iso(now), "2026-09-28T12:00:00.0000000Z");
        assert_eq!(parse_rfc3339(&iso(now + 2 * HOUR)), Some(now + 2 * HOUR));
    }

    #[test]
    fn the_client_can_move_to_a_worker_thread() {
        fn assert_send<T: Send>() {}
        assert_send::<ClaudeClient>();
    }

    #[test]
    fn polling_waits_five_minutes_unless_forced() {
        let mut test = Fixture::new();
        test.write_credentials("original", "refresh", test.now() + HOUR);
        let mut started = 0;
        // The first read fetches usage and announces the request.
        assert!(test.client.read(false, || started += 1).is_some());
        assert_eq!((test.request_count(), started), (1, 1));
        // A manual refresh can run immediately and confirms success.
        let refreshed = test.client.read(true, || started += 1).unwrap();
        assert_eq!((test.request_count(), started), (2, 2));
        assert_eq!(refreshed.status.as_deref(), Some("Claude usage updated"));
        assert!(!refreshed.stale);

        // Automatic polling waits five minutes after the manual refresh.
        test.advance(5 * MINUTE - 1);
        let waiting = test.client.read(false, || started += 1).unwrap();
        assert_eq!((test.request_count(), started), (2, 2));
        assert_eq!(waiting.status, None);
        test.advance(1);
        test.read(false);
        assert_eq!(test.request_count(), 3);

        // Usage polling sends the access token without a request body.
        let requests = test.requests();
        assert_eq!(requests[0].url, "https://api.anthropic.com/api/oauth/usage");
        assert_eq!(requests[0].token(), Some("original"));
        assert!(requests[0].body.is_none());
    }

    #[test]
    fn requests_carry_the_expected_headers_and_limits() {
        let mut test = Fixture::new();
        test.write_credentials("original", "refresh", test.now());
        test.respond_rotating(ROTATED);
        assert!(test.read(false).is_some());

        let requests = test.requests();
        let (token, usage) = (&requests[0], &requests[1]);
        assert_eq!(token.url, "https://platform.claude.com/v1/oauth/token");
        assert_eq!(token.method, "POST");
        assert_eq!(
            token.headers,
            [
                ("Accept".to_string(), "application/json".to_string()),
                ("Content-Type".to_string(), "application/json".to_string()),
            ]
        );
        let body: Value = serde_json::from_str(token.body.as_deref().unwrap()).unwrap();
        assert_eq!(
            body,
            json!({
                "grant_type": "refresh_token",
                "refresh_token": "refresh",
                "client_id": "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
                "scope": "user:profile user:inference user:sessions:claude_code \
                          user:mcp_servers user:file_upload",
            })
        );

        assert_eq!(usage.method, "GET");
        assert_eq!(
            usage.headers,
            [
                ("Accept".to_string(), "application/json".to_string()),
                (
                    "Authorization".to_string(),
                    "Bearer rotated-access".to_string()
                ),
                ("anthropic-beta".to_string(), "oauth-2025-04-20".to_string()),
            ]
        );
        for request in &requests {
            assert!(!request.follow_redirects && !request.has_user_agent);
            assert_eq!(request.max_bytes, 65_536);
            assert_eq!(request.timeout_milliseconds, 10_000);
        }
    }

    #[test]
    fn rate_limits_pause_automatic_and_forced_reads() {
        for retry_after in [0, 900, 90_000] {
            let mut test = Fixture::new();
            test.write_credentials("original", "refresh", test.now() + 7 * DAY);
            test.respond_status(429, retry_after);
            test.read(false);
            test.advance((retry_after as i64).max(300) - 1);
            test.read(false);
            let limited = test.read(true);
            assert!(limited.is_none(), "{retry_after}");
            assert_eq!(test.request_count(), 1, "{retry_after}");

            // Refresh resumes at the rate limit deadline.
            test.advance(1);
            test.respond_success();
            assert!(test.read(true).is_some(), "{retry_after}");
            assert_eq!(test.request_count(), 2, "{retry_after}");
        }
    }

    #[test]
    fn rate_limit_dates_set_the_cooldown_from_the_injected_clock() {
        for offset in [-1, 901] {
            let mut test = Fixture::new();
            test.write_credentials("original", "refresh", test.now() + 7 * DAY);
            let deadline = test.now() + offset;
            test.respond(move |_| {
                Some(Response {
                    retry_after: http::RetryAfter::Date(deadline),
                    ..rejection(429)
                })
            });
            test.read(false);
            test.advance(offset.max(MIN_RATE_LIMIT_SECONDS) - 1);
            test.read(true);
            assert_eq!(test.request_count(), 1);
            test.advance(1);
            test.read(true);
            assert_eq!(test.request_count(), 2);
        }
    }

    #[test]
    fn an_oversized_retry_after_falls_back_to_the_minimum_cooldown() {
        let mut test = Fixture::new();
        test.write_credentials("original", "refresh", test.now() + 7 * DAY);
        test.respond_status(429, u64::MAX);
        test.read(false);
        test.advance(299);
        test.read(true);
        assert_eq!(test.request_count(), 1);
        test.advance(1);
        test.read(true);
        assert_eq!(test.request_count(), 2);
    }

    #[test]
    fn a_failed_refresh_keeps_the_last_observation() {
        let mut test = Fixture::new();
        test.write_credentials("original", "refresh", test.now() + HOUR);
        let observed = test.now();
        test.read(false);
        let stored = fs::read_to_string(&test.cache_path).unwrap();

        // Failed requests keep the old reading and its time, marked stale.
        test.advance(5 * MINUTE);
        test.respond_status(503, 0);
        let failed = test.read(false).unwrap();
        assert!(failed.stale);
        assert_eq!(failed.usage.updated_at, observed);
        assert_eq!(failed.usage.five_hour.as_ref().unwrap().used_percent, 25.0);
        assert_eq!(
            failed.status.as_deref(),
            Some("Claude refresh failed - showing saved usage")
        );
        assert_eq!(fs::read_to_string(&test.cache_path).unwrap(), stored);

        // Automatic refresh respects the ordinary failure backoff.
        test.advance(59);
        test.read(false);
        assert_eq!(test.request_count(), 2);

        // A manual retry bypasses it and restores a fresh observation.
        test.respond_success();
        let recovered = test.read(true).unwrap();
        assert_eq!(test.request_count(), 3);
        assert!(!recovered.stale);
        assert_eq!(recovered.usage.updated_at, test.now());

        // Malformed server data cannot replace the last valid observation.
        test.advance(5 * MINUTE);
        test.respond(|_| Some(ok("not json")));
        let malformed = test.read(false).unwrap();
        assert!(malformed.stale);
        assert_eq!(malformed.usage.updated_at, recovered.usage.updated_at);

        // A server cooldown explains why a manual refresh waits, and keeps
        // the saved usage.
        test.respond_status(429, 900);
        let rate_limited = test.read(true).unwrap();
        assert_eq!(
            rate_limited.status.as_deref(),
            Some("Claude refresh rate limited - showing saved usage")
        );
        let requests_before_cooldown = test.request_count();
        let mut started = false;
        let limited = test.client.read(true, || started = true).unwrap();
        assert!(!started);
        assert_eq!(test.request_count(), requests_before_cooldown);
        assert!(limited.stale);
        let status = limited.status.unwrap();
        let deadline = status
            .strip_prefix("Claude rate limited - retry at ")
            .unwrap();
        assert!(deadline.contains(|character: char| character.is_ascii_digit()));
        // An automatic read during the cooldown keeps the plain notice.
        let automatic = test.read(false).unwrap();
        assert_eq!(
            automatic.status.as_deref(),
            Some("Claude refresh rate limited - showing saved usage")
        );
    }

    #[test]
    fn automatic_retries_resume_one_minute_after_a_failure() {
        let mut test = Fixture::new();
        test.write_credentials("original", "refresh", test.now() + HOUR);
        test.respond_status(500, 0);
        assert!(test.read(false).is_none());
        test.advance(59);
        test.read(false);
        assert_eq!(test.request_count(), 1);
        test.advance(1);
        test.respond_success();
        let recovered = test.read(false).unwrap();
        assert_eq!(test.request_count(), 2);
        assert_eq!((recovered.stale, recovered.status), (false, None));
    }

    fn token_refresh(unauthorized: bool) {
        let mut test = Fixture::new();
        let refreshed_at = test.now();
        let minutes_left = if unauthorized { 60 } else { 5 };
        test.write_credentials("original", "refresh", refreshed_at + minutes_left * MINUTE);
        let now = Arc::clone(&test.now);
        test.respond(move |request| {
            Some(if request.body.is_some() {
                ok(ROTATED)
            } else if unauthorized && request.token() == Some("original") {
                rejection(401)
            } else {
                success(now.load(Ordering::SeqCst))
            })
        });

        // The token refresh recovers account usage.
        let reading = test.read(false).unwrap();
        assert!(!reading.stale);
        assert_eq!(reading.status, None);
        // An expiry refresh precedes usage; an unauthorized one retries usage once.
        let requests = test.requests();
        assert_eq!(requests.len(), if unauthorized { 3 } else { 2 });
        let refresh = &requests[if unauthorized { 1 } else { 0 }];
        let body: Value = serde_json::from_str(refresh.body.as_deref().unwrap()).unwrap();
        assert!(refresh.url.ends_with("/v1/oauth/token"));
        assert_eq!(refresh.token(), None);
        assert_eq!(body["grant_type"], "refresh_token");
        assert_eq!(body["refresh_token"], "refresh");
        assert_eq!(requests.last().unwrap().token(), Some("rotated-access"));

        // The rotated tokens and expiry are saved.
        let saved = test.saved_credentials();
        let oauth = &saved["claudeAiOauth"];
        assert_eq!(oauth["accessToken"], "rotated-access");
        assert_eq!(oauth["refreshToken"], "rotated-refresh");
        assert_eq!(oauth["expiresAt"], json!((refreshed_at + HOUR) * 1000));
        // Unrelated root and OAuth fields are preserved.
        assert_eq!(saved["unrelatedSetting"], "keep-root");
        assert_eq!(oauth["subscriptionType"], "max");
        assert_eq!(oauth["scopes"].as_array().unwrap().len(), 2);

        // Tokens stay out of the cache, and no temporary copy is left behind.
        let cache = fs::read_to_string(&test.cache_path).unwrap();
        for secret in ["original", "refresh", "rotated"] {
            assert!(!cache.contains(secret), "{secret}");
        }
        assert!(test.directory.leftovers().is_empty());
    }

    #[test]
    fn an_expiring_login_is_refreshed_before_usage() {
        token_refresh(false);
    }

    #[test]
    fn a_rejected_login_is_refreshed_and_usage_retried_once() {
        token_refresh(true);
    }

    #[test]
    fn a_login_with_more_than_five_minutes_left_is_not_refreshed() {
        let mut test = Fixture::new();
        test.write_credentials("original", "refresh", test.now() + 5 * MINUTE + 1);
        assert!(test.read(false).is_some());
        let requests = test.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].token(), Some("original"));
    }

    #[test]
    fn a_login_replaced_by_the_cli_during_refresh_is_kept_and_used() {
        let mut test = Fixture::new();
        test.write_credentials("original", "refresh", test.now());
        let (path, now) = (test.credentials_path.clone(), Arc::clone(&test.now));
        let concurrent = Arc::new(Mutex::new(String::new()));
        let written = Arc::clone(&concurrent);
        test.respond(move |request| {
            let now = now.load(Ordering::SeqCst);
            if request.body.is_none() {
                return Some(success(now));
            }
            write_credentials(&path, "cli-access", "cli-refresh", now + 2 * HOUR);
            *written.lock().unwrap() = fs::read_to_string(&path).unwrap();
            Some(ok(
                r#"{"access_token":"discard-access","refresh_token":"discard-refresh","expires_in":3600}"#,
            ))
        });

        // Usage remains available after a concurrent CLI refresh.
        assert!(test.read(false).is_some());
        let requests = test.requests();
        assert_eq!(requests.len(), 2);
        // The credential generation that replaced ours is preserved and used.
        assert_eq!(
            fs::read_to_string(&test.credentials_path).unwrap(),
            *concurrent.lock().unwrap()
        );
        assert_eq!(requests[1].token(), Some("cli-access"));
        assert!(test.directory.leftovers().is_empty());
    }

    #[test]
    fn a_login_already_renewed_by_the_cli_is_used_without_a_token_request() {
        let mut test = Fixture::new();
        test.write_credentials("original", "refresh", test.now() + HOUR);
        let (path, now) = (test.credentials_path.clone(), Arc::clone(&test.now));
        test.respond(move |request| {
            let now = now.load(Ordering::SeqCst);
            Some(if request.token() == Some("original") {
                write_credentials(&path, "cli-access", "cli-refresh", now + 2 * HOUR);
                rejection(401)
            } else {
                success(now)
            })
        });
        assert!(test.read(false).is_some());
        let requests = test.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests.iter().all(|request| request.body.is_none()));
        assert_eq!(requests[1].token(), Some("cli-access"));
    }

    #[test]
    fn a_rotation_that_cannot_be_saved_is_used_and_saved_later() {
        let mut test = Fixture::new();
        test.write_credentials("original", "refresh", test.now());
        let (path, now) = (test.credentials_path.clone(), Arc::clone(&test.now));
        let credential_lock: Arc<Mutex<Option<File>>> = Arc::new(Mutex::new(None));
        let held = Arc::clone(&credential_lock);
        test.respond(move |request| {
            if request.body.is_none() {
                return Some(success(now.load(Ordering::SeqCst)));
            }
            // Readers may share the file, but it cannot be replaced.
            let lock = OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .open(&path)
                .unwrap();
            *held.lock().unwrap() = Some(lock);
            Some(ok(ROTATED))
        });

        // A save failure still uses the newly rotated credentials, visibly.
        let reading = test.read(false).unwrap();
        assert!(!reading.stale);
        let requests = test.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].token(), Some("rotated-access"));
        assert_eq!(
            reading.status.as_deref(),
            Some("Claude login refresh could not be saved")
        );
        // The locked credentials stay unchanged.
        assert_eq!(
            test.saved_credentials()["claudeAiOauth"]["accessToken"],
            "original"
        );
        assert!(test.directory.leftovers().is_empty());

        // The next read saves the pending rotation without another token request.
        credential_lock.lock().unwrap().take();
        let reading = test.read(false).unwrap();
        assert_eq!(reading.status, None);
        let saved = test.saved_credentials();
        assert_eq!(saved["claudeAiOauth"]["accessToken"], "rotated-access");
        assert_eq!(saved["claudeAiOauth"]["refreshToken"], "rotated-refresh");
        assert_eq!(saved["unrelatedSetting"], "keep-root");
        assert_eq!(test.request_count(), 2);
        assert!(test.directory.leftovers().is_empty());
    }

    #[test]
    fn an_unsaved_rotation_is_dropped_when_the_cli_signs_in_again() {
        let mut test = Fixture::new();
        test.write_credentials("original", "refresh", test.now());
        let (path, now) = (test.credentials_path.clone(), Arc::clone(&test.now));
        let credential_lock: Arc<Mutex<Option<File>>> = Arc::new(Mutex::new(None));
        let held = Arc::clone(&credential_lock);
        test.respond(move |request| {
            if request.body.is_none() {
                return Some(success(now.load(Ordering::SeqCst)));
            }
            let lock = OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .open(&path)
                .unwrap();
            *held.lock().unwrap() = Some(lock);
            Some(ok(ROTATED))
        });
        test.read(false);
        credential_lock.lock().unwrap().take();

        test.write_credentials("cli-access", "cli-refresh", test.now() + 2 * HOUR);
        let from_cli = fs::read_to_string(&test.credentials_path).unwrap();
        let reading = test.read(true).unwrap();
        assert_eq!(reading.status.as_deref(), Some("Claude usage updated"));
        assert_eq!(test.requests().last().unwrap().token(), Some("cli-access"));
        assert_eq!(
            fs::read_to_string(&test.credentials_path).unwrap(),
            from_cli
        );
    }

    #[test]
    fn a_refresh_without_expiry_drops_the_obsolete_expiry() {
        let mut test = Fixture::new();
        test.write_credentials("original", "refresh", test.now());
        test.respond_rotating(
            r#"{"access_token":"rotated-access","refresh_token":"rotated-refresh"}"#,
        );
        // The refresh accepts a response without the optional expiry.
        assert!(test.read(false).is_some());
        let saved = test.saved_credentials();
        let oauth = saved["claudeAiOauth"].as_object().unwrap();
        assert_eq!(oauth["accessToken"], "rotated-access");
        assert_eq!(oauth["refreshToken"], "rotated-refresh");
        assert!(!oauth.contains_key("expiresAt"));

        // The missing expiry does not cause another rotation on the next poll.
        test.advance(30);
        assert!(test.read(true).is_some());
        let requests = test.requests();
        assert_eq!(requests.len(), 3);
        assert!(requests[2].body.is_none());
        assert_eq!(requests[2].token(), Some("rotated-access"));
    }

    #[test]
    fn a_refresh_that_keeps_the_refresh_token_saves_only_the_access_token() {
        let mut test = Fixture::new();
        test.write_credentials("original", "refresh", test.now());
        test.respond_rotating(r#"{"access_token":"rotated-access","expires_in":7200.5}"#);
        assert!(test.read(false).is_some());
        let oauth = &test.saved_credentials()["claudeAiOauth"];
        assert_eq!(oauth["accessToken"], "rotated-access");
        assert_eq!(oauth["refreshToken"], "refresh");
        assert_eq!(oauth["expiresAt"], json!(test.now() * 1000 + 7_200_500));
    }

    #[test]
    fn sign_in_failures_ask_for_renewal() {
        const RENEWAL: Option<&str> = Some("Claude sign-in needs renewal - showing saved usage");

        // The token endpoint rejects the refresh token.
        let mut test = Fixture::new();
        test.write_credentials("original", "refresh", test.now() + HOUR);
        assert!(test.read(false).is_some());
        test.write_credentials("original", "refresh", test.now());
        test.respond_status(400, 0);
        let rejected = test.read(true).unwrap();
        assert!(rejected.stale);
        assert_eq!(rejected.status.as_deref(), RENEWAL);
        assert_eq!(
            test.saved_credentials()["claudeAiOauth"]["accessToken"],
            "original"
        );

        // The usage endpoint rejects the login before and after its renewal.
        let mut test = Fixture::new();
        test.write_credentials("original", "refresh", test.now() + HOUR);
        assert!(test.read(false).is_some());
        test.respond(|request| {
            Some(match (&request.body, request.token()) {
                (Some(_), _) => ok(ROTATED),
                (None, Some("original")) => rejection(401),
                (None, _) => rejection(403),
            })
        });
        assert_eq!(test.read(true).unwrap().status.as_deref(), RENEWAL);
        assert_eq!(test.request_count(), 4);
        assert_eq!(
            test.saved_credentials()["claudeAiOauth"]["accessToken"],
            "rotated-access"
        );

        // There is no refresh token to renew an expired login with.
        let mut test = Fixture::new();
        test.write_credentials("original", "refresh", test.now() + HOUR);
        assert!(test.read(false).is_some());
        test.write_credentials("original", " ", test.now());
        assert_eq!(test.read(true).unwrap().status.as_deref(), RENEWAL);
        assert_eq!(test.request_count(), 1);
    }

    #[test]
    fn transport_failures_and_unusable_token_responses_count_as_failed() {
        const FAILED: Option<&str> = Some("Claude refresh failed - showing saved usage");
        let mut test = Fixture::new();
        test.write_credentials("original", "refresh", test.now() + HOUR);
        assert!(test.read(false).is_some());

        test.respond(|_| None);
        let offline = test.read(true).unwrap();
        assert!(offline.stale);
        assert_eq!(offline.status.as_deref(), FAILED);

        test.write_credentials("original", "refresh", test.now());
        for tokens in [
            "not json",
            "[]",
            r#"{"access_token":" "}"#,
            r#"{"access_token":5}"#,
        ] {
            test.respond_rotating(tokens);
            assert_eq!(
                test.read(true).unwrap().status.as_deref(),
                FAILED,
                "{tokens}"
            );
            assert_eq!(
                test.saved_credentials()["claudeAiOauth"]["accessToken"],
                "original"
            );
        }
        assert!(test.directory.leftovers().is_empty());
    }

    #[test]
    fn missing_or_invalid_credentials_never_issue_requests() {
        let mut test = Fixture::new();
        // Missing credentials return no account reading.
        assert!(test.read(false).is_none());
        for json in [
            "not json",
            "{}",
            "[]",
            r#"{"claudeAiOauth":{"accessToken":" "}}"#,
            r#"{"claudeAiOauth":{"accessToken":7}}"#,
        ] {
            fs::write(&test.credentials_path, json).unwrap();
            assert!(test.read(true).is_none(), "{json}");
        }
        assert_eq!(test.request_count(), 0);

        // Even a saved reading stays hidden without a login.
        test.write_credentials("original", "refresh", test.now() + HOUR);
        assert!(test.read(false).is_some());
        fs::remove_file(&test.credentials_path).unwrap();
        assert!(test.read(false).is_none());
        assert_eq!(test.request_count(), 1);
    }

    #[test]
    fn the_newer_of_the_two_caches_is_shown() {
        let mut test = Fixture::new();
        test.write_credentials("original", "refresh", test.now() + HOUR);
        let status_line = |used: u32, reset: i64| {
            format!(
                r#"{{"rate_limits":{{"seven_day":{{"used_percentage":{used},"resets_at":{reset}}}}}}}"#
            )
        };
        let week_reset = test.now() + 3 * DAY;

        // An older status-line reading does not displace the account reading.
        let older = status_line(40, week_reset);
        assert!(cache::capture_status_line(
            &older,
            &test.fallback_path,
            test.now() - 1
        ));
        let account = test.read(false).unwrap();
        assert!(account.usage.five_hour.is_some() && account.usage.weekly.is_none());

        // A newer one is shown until the account is read again.
        test.advance(MINUTE);
        let newer = status_line(55, week_reset);
        assert!(cache::capture_status_line(
            &newer,
            &test.fallback_path,
            test.now()
        ));
        let fallback = test.read(false).unwrap();
        assert_eq!(fallback.usage.updated_at, test.now());
        assert_eq!(fallback.usage.weekly.as_ref().unwrap().used_percent, 55.0);
        assert!(fallback.usage.five_hour.is_none());
        assert_eq!(test.request_count(), 1);

        // With no account reading at all, the status-line reading is used.
        fs::remove_file(&test.cache_path).unwrap();
        assert_eq!(test.read(false), Some(fallback));
    }

    #[test]
    fn a_response_that_cannot_be_stored_counts_as_failed() {
        let directory = TestDirectory::new("ClaudeQuotaTests");
        let credentials_path = directory.file("credentials.json");
        let now = parse_rfc3339("2026-09-28T12:00:00Z").unwrap();
        write_credentials(&credentials_path, "original", "refresh", now + HOUR);
        let requested = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&requested);
        let mut client = ClaudeClient::new(
            credentials_path,
            PathBuf::new(),
            PathBuf::new(),
            Box::new(move |_| {
                seen.store(true, Ordering::SeqCst);
                Some(success(now))
            }),
            Box::new(move || now),
        );
        assert!(client.read(false, || {}).is_none());
        assert!(requested.load(Ordering::SeqCst));
        assert_eq!(client.failure, Some(REFRESH_FAILED));
        assert_eq!(client.next_poll, Some(now + RETRY_SECONDS));
    }

    #[test]
    fn usage_windows_are_validated_independently() {
        let now = parse_rfc3339("2026-09-28T12:00:00Z").unwrap();
        let reset = iso(now + 2 * HOUR);
        // Invalid utilization cannot hide a valid independent weekly window.
        for value in ["null", "true", "\"20\"", "-1", "100.01", "{}", "[]"] {
            let input = format!(
                r#"{{"five_hour":{{"utilization":{value},"resets_at":"{reset}"}},
                    "seven_day":{{"utilization":75,"resets_at":"{reset}"}}}}"#
            );
            let limits = converted_windows(&input, now);
            assert_eq!(limits.len(), 1, "{value}");
            assert_eq!(limits["seven_day"]["used_percentage"], 75.0, "{value}");
            assert_eq!(limits["seven_day"]["resets_at"], json!(now + 2 * HOUR));
        }
        // Invalid or expired resets are ignored.
        for invalid_reset in ["not-a-date".to_string(), iso(now), iso(now - 1)] {
            let input = usage_body("25", &invalid_reset);
            assert_eq!(
                convert_usage(input.as_bytes(), now),
                None,
                "{invalid_reset}"
            );
        }
        // Missing quota fields provide no reading.
        for input in [
            "{}".to_string(),
            "null".to_string(),
            "[]".to_string(),
            "not json".to_string(),
            r#"{"five_hour":null}"#.to_string(),
            format!(r#"{{"five_hour":{{"resets_at":"{reset}"}}}}"#),
            r#"{"five_hour":{"utilization":25}}"#.to_string(),
            format!(
                r#"{{"five_hour":{{"utilization":25,"resets_at":{}}}}}"#,
                now + HOUR
            ),
        ] {
            assert_eq!(convert_usage(input.as_bytes(), now), None, "{input}");
        }
        // Zero and one hundred percent are valid quota boundaries.
        let bounds = converted_windows(
            &format!(
                r#"{{"five_hour":{{"utilization":0,"resets_at":"{reset}"}},
                    "seven_day":{{"utilization":100,"resets_at":"{reset}"}}}}"#
            ),
            now,
        );
        assert_eq!(bounds["five_hour"]["used_percentage"], 0.0);
        assert_eq!(bounds["seven_day"]["used_percentage"], 100.0);
    }

    #[test]
    fn retry_deadlines_use_the_local_time_format() {
        let noon = parse_rfc3339("2026-09-28T12:00:00Z").unwrap();
        // The text depends on the user's time zone and format, so only its
        // shape is checked here.
        let shown = long_time(noon + 7);
        assert!(shown.contains(|character: char| character.is_ascii_digit()));
        assert!(shown.len() >= format_clock(noon + 7).len());
        assert_eq!(long_time(i64::MAX), "?");
    }
}
