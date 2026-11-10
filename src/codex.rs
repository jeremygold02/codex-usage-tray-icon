//! Reads the signed-in account's rate limits from a short-lived local
//! `codex app-server --stdio` process. No prompt or model request is sent.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::model::{
    LimitSet, LimitWindow, RedemptionOutcome, ResetCredit, Usage, FIVE_HOUR_MINUTES, WEEKLY_MINUTES,
};
use crate::timefmt::now_unix;

/// The JSON-RPC id of the one request sent after the initialize handshake.
const REQUEST_ID: i64 = 2;
const REQUEST_TIMEOUT: Duration = Duration::from_millis(20_000);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(1_500);
const KILL_TIMEOUT: Duration = Duration::from_millis(1_000);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const MAX_ERROR_DETAIL_CHARS: usize = 160;
const MAX_PACKAGE_SEARCH_DEPTH: usize = 6;

#[derive(Debug, PartialEq)]
enum FetchError {
    CommandNotFound,
    StartFailed,
    TimedOut,
    AuthenticationRequired,
    ExitedEarly,
    RequestRejected(Option<String>),
    LimitsUnavailable,
    InvalidResponse,
}

impl FetchError {
    fn message(&self) -> String {
        match self {
            Self::CommandNotFound => {
                "Codex CLI was not found. Install Codex or add it to PATH.".to_string()
            }
            Self::StartFailed => "Codex app-server could not be started.".to_string(),
            Self::TimedOut => "Codex app-server timed out while reading usage.".to_string(),
            Self::AuthenticationRequired => "Codex is not signed in. Run codex login.".to_string(),
            Self::ExitedEarly => "Codex app-server exited before returning usage.".to_string(),
            Self::RequestRejected(Some(detail)) => {
                format!("Codex app-server rejected the usage request: {detail}")
            }
            Self::RequestRejected(None) => {
                "Codex app-server could not read usage limits.".to_string()
            }
            Self::LimitsUnavailable => {
                "Codex did not return weekly or 5-hour usage limits.".to_string()
            }
            Self::InvalidResponse => "Codex app-server returned invalid usage data.".to_string(),
        }
    }

    /// The same failure, worded for a reset request instead of a usage read.
    fn redemption_message(&self) -> &'static str {
        match self {
            Self::CommandNotFound => "Codex CLI was not found. Install Codex or add it to PATH.",
            Self::StartFailed => "Codex app-server could not be started.",
            Self::TimedOut => "Codex app-server timed out while using the reset credit.",
            Self::AuthenticationRequired => "Codex is not signed in. Run codex login.",
            Self::ExitedEarly => "Codex app-server exited before returning the reset result.",
            Self::RequestRejected(_) => "Codex app-server rejected the reset request.",
            Self::LimitsUnavailable | Self::InvalidResponse => {
                "Codex app-server returned invalid reset data."
            }
        }
    }
}

/// Fetches current usage, or a user-facing description of why it failed.
pub fn fetch_usage() -> Result<Usage, String> {
    let attempted_at = now_unix();
    execute(&json!({ "id": REQUEST_ID, "method": "account/rateLimits/read" }))
        .and_then(|response| parse_rate_limits(&response, attempted_at))
        .map_err(|error| error.message())
}

/// Asks Codex to use a banked reset. Repeating a call with the same
/// idempotency key cannot spend a second credit.
pub fn consume_reset_credit(credit_id: &str, idempotency_key: &str) -> RedemptionOutcome {
    if credit_id.trim().is_empty() {
        return RedemptionOutcome::Failed("The reset credit did not include an ID.".to_string());
    }
    if idempotency_key.trim().is_empty() {
        return RedemptionOutcome::Failed(
            "The reset request did not include an idempotency key.".to_string(),
        );
    }
    let request = json!({
        "id": REQUEST_ID,
        "method": "account/rateLimitResetCredit/consume",
        "params": { "idempotencyKey": idempotency_key, "creditId": credit_id }
    });
    match execute(&request) {
        Ok(response) => parse_redemption(&response),
        Err(error) => RedemptionOutcome::Failed(error.redemption_message().to_string()),
    }
}

fn parse_redemption(response: &Value) -> RedemptionOutcome {
    let failed = |message: &str| RedemptionOutcome::Failed(message.to_string());
    if let Some(error) = response.get("error").and_then(Value::as_object) {
        let message = optional_string(error, "message");
        if message.as_deref().is_some_and(is_authentication_message) {
            return failed("Codex is not signed in. Run codex login.");
        }
        return match message.as_deref().and_then(compact_error_detail) {
            Some(detail) => failed(&format!("Codex rejected the reset request: {detail}")),
            None => failed("Codex rejected the reset request."),
        };
    }
    let Some(result) = response.get("result").and_then(Value::as_object) else {
        return failed("Codex returned invalid reset data.");
    };
    match optional_string(result, "outcome") {
        Some(outcome) if outcome.eq_ignore_ascii_case("reset") => RedemptionOutcome::Reset,
        Some(outcome) if outcome.eq_ignore_ascii_case("alreadyRedeemed") => {
            RedemptionOutcome::AlreadyRedeemed
        }
        Some(outcome) if outcome.eq_ignore_ascii_case("nothingToReset") => {
            RedemptionOutcome::NothingToReset
        }
        Some(outcome) if outcome.eq_ignore_ascii_case("noCredit") => RedemptionOutcome::NoCredit,
        _ => failed("Codex returned an unrecognized reset result."),
    }
}

/// Starts a short-lived app-server, sends `request` and returns its response.
fn execute(request: &Value) -> Result<Value, FetchError> {
    let command = resolve_codex_command().ok_or(FetchError::CommandNotFound)?;
    // Rust runs .cmd/.bat launchers through cmd.exe with safe argument quoting.
    let mut child = Command::new(command)
        .args(["app-server", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|_| FetchError::StartFailed)?;

    let authentication_hint = Arc::new(AtomicBool::new(false));
    let result = exchange(&mut child, &authentication_hint, request);
    stop_process(&mut child);

    result.map_err(|error| {
        let process_failure = matches!(
            error,
            FetchError::StartFailed | FetchError::TimedOut | FetchError::ExitedEarly
        );
        if process_failure && authentication_hint.load(Ordering::Relaxed) {
            FetchError::AuthenticationRequired
        } else {
            error
        }
    })
}

/// Runs the initialize handshake and then `request`. Standard input closes
/// when this returns, which lets the app-server exit on its own.
fn exchange(
    child: &mut Child,
    authentication_hint: &Arc<AtomicBool>,
    request: &Value,
) -> Result<Value, FetchError> {
    let mut input = child.stdin.take().ok_or(FetchError::StartFailed)?;
    let output = child.stdout.take().ok_or(FetchError::StartFailed)?;
    let errors = child.stderr.take().ok_or(FetchError::StartFailed)?;

    let (sender, lines) = mpsc::channel::<String>();
    thread::spawn(move || {
        read_lines(output, |line| sender.send(line).is_ok());
    });
    let hint = Arc::clone(authentication_hint);
    thread::spawn(move || {
        read_lines(errors, |line| {
            if is_authentication_message(&line) {
                hint.store(true, Ordering::Relaxed);
            }
            true
        });
    });

    let deadline = Instant::now() + REQUEST_TIMEOUT;
    send(
        &mut input,
        &json!({
            "id": 1,
            "method": "initialize",
            "params": {
                "clientInfo": {
                    "name": "codex-usage-tray",
                    "title": "Codex Usage Tray",
                    "version": "1.0"
                },
                "capabilities": {}
            }
        }),
    )?;
    let initialize = wait_for_response(&lines, 1, deadline)?;
    if initialize.get("error").is_some_and(Value::is_object) {
        return Ok(initialize);
    }
    if !initialize.get("result").is_some_and(Value::is_object) {
        return Err(FetchError::InvalidResponse);
    }

    send(&mut input, &json!({ "method": "initialized" }))?;
    send(&mut input, request)?;
    wait_for_response(&lines, REQUEST_ID, deadline)
}

fn read_lines(source: impl Read, mut handle: impl FnMut(String) -> bool) {
    let mut reader = BufReader::new(source);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                if !handle(String::from_utf8_lossy(&buffer).trim().to_string()) {
                    return;
                }
            }
        }
    }
}

fn send(input: &mut ChildStdin, message: &Value) -> Result<(), FetchError> {
    writeln!(input, "{message}")
        .and_then(|()| input.flush())
        .map_err(|_| FetchError::StartFailed)
}

fn wait_for_response(
    lines: &Receiver<String>,
    request_id: i64,
    deadline: Instant,
) -> Result<Value, FetchError> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(FetchError::TimedOut);
        }
        match lines.recv_timeout(remaining) {
            Ok(line) => {
                if let Ok(candidate) = serde_json::from_str::<Value>(&line) {
                    if candidate.is_object() && has_request_id(&candidate, request_id) {
                        return Ok(candidate);
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => return Err(FetchError::TimedOut),
            Err(RecvTimeoutError::Disconnected) => return Err(FetchError::ExitedEarly),
        }
    }
}

fn has_request_id(response: &Value, request_id: i64) -> bool {
    match response.get("id") {
        Some(Value::String(text)) => *text == request_id.to_string(),
        Some(value) => integer(value) == Some(request_id),
        None => false,
    }
}

fn stop_process(child: &mut Child) {
    drop(child.stdin.take());
    if wait_for_exit(child, SHUTDOWN_TIMEOUT) {
        return;
    }
    if child.kill().is_ok() {
        wait_for_exit(child, KILL_TIMEOUT);
    }
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            _ => return false,
        }
    }
}

fn resolve_codex_command() -> Option<PathBuf> {
    let app_data = std::env::var_os("APPDATA").filter(|value| !value.is_empty());
    if let Some(app_data) = app_data.map(PathBuf::from) {
        let npm = app_data.join("npm");
        let package = npm.join("node_modules").join("@openai").join("codex");
        if let Some(native) = find_bundled_codex_executable(&package) {
            return Some(native);
        }
        let launcher = npm.join("codex.cmd");
        if launcher.is_file() && package.join("bin").join("codex.js").is_file() {
            return Some(launcher);
        }
    }

    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|directory| !directory.as_os_str().is_empty())
        .flat_map(|directory| {
            [".exe", ".com", ".cmd", ".bat", ""]
                .into_iter()
                .map(move |extension| directory.join(format!("codex{extension}")))
        })
        .find(|candidate| candidate.is_file())
}

fn find_bundled_codex_executable(package: &Path) -> Option<PathBuf> {
    let platform_packages = package.join("node_modules").join("@openai");
    std::fs::read_dir(platform_packages)
        .ok()?
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("codex-win32-")
        })
        .find_map(|entry| find_file(&entry.path(), "codex.exe", MAX_PACKAGE_SEARCH_DEPTH))
}

fn find_file(directory: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    let entries: Vec<PathBuf> = std::fs::read_dir(directory)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    let is_match = |path: &PathBuf| {
        path.is_file()
            && path
                .file_name()
                .is_some_and(|file| file.to_string_lossy().eq_ignore_ascii_case(name))
    };
    if let Some(found) = entries.iter().find(|path| is_match(path)) {
        return Some(found.clone());
    }
    if depth == 0 {
        return None;
    }
    entries
        .iter()
        .filter(|path| path.is_dir())
        .find_map(|path| find_file(path, name, depth - 1))
}

fn parse_rate_limits(response: &Value, attempted_at: i64) -> Result<Usage, FetchError> {
    let root = response.as_object().ok_or(FetchError::InvalidResponse)?;
    if let Some(error) = root.get("error").and_then(Value::as_object) {
        return Err(from_rpc_error(error));
    }
    let payload = match root.get("result") {
        Some(result) => result.as_object().ok_or(FetchError::InvalidResponse)?,
        None => root,
    };

    let limits_by_id = optional_object(payload, "rateLimitsByLimitId")?;
    let direct_limits = optional_object(payload, "rateLimits")?;
    let mapped_limits = limits_by_id.and_then(|limits| {
        limits
            .iter()
            .find(|(id, _)| id.eq_ignore_ascii_case("codex"))
            .and_then(|(_, limit)| limit.as_object())
    });

    let codex_limits = direct_limits.or(mapped_limits);
    let (five_hour, weekly) = codex_limits
        .map(parse_limit_windows)
        .unwrap_or((None, None));
    if five_hour.is_none() && weekly.is_none() {
        return Err(FetchError::LimitsUnavailable);
    }

    let plan_type = [codex_limits, mapped_limits]
        .into_iter()
        .flatten()
        .find_map(|limits| optional_string(limits, "planType"));
    let reset_credits = payload
        .get("rateLimitResetCredits")
        .and_then(Value::as_object);
    Ok(Usage {
        updated_at: attempted_at,
        five_hour,
        weekly,
        additional_limits: limits_by_id
            .map(parse_additional_limits)
            .unwrap_or_default(),
        resets: reset_credits
            .map(parse_available_resets)
            .unwrap_or_default(),
        reset_count: reset_credits
            .and_then(|credits| credits.get("availableCount"))
            .and_then(integer)
            .and_then(|count| usize::try_from(count).ok()),
        plan_type,
    })
}

/// Every limit except the account's own "codex" entry, ordered by name.
fn parse_additional_limits(limits_by_id: &Map<String, Value>) -> Vec<LimitSet> {
    let mut limits: Vec<LimitSet> = limits_by_id
        .iter()
        .filter(|(id, _)| !id.eq_ignore_ascii_case("codex"))
        .filter_map(|(id, limit)| {
            let limit = limit.as_object()?;
            let (five_hour, weekly) = parse_limit_windows(limit);
            Some(LimitSet {
                limit_id: id.clone(),
                display_name: optional_string(limit, "limitName").unwrap_or_else(|| id.clone()),
                five_hour,
                weekly,
            })
        })
        .collect();
    limits.sort_by_cached_key(|limit| {
        (
            limit.display_name.to_lowercase(),
            limit.limit_id.to_lowercase(),
        )
    });
    limits
}

/// A missing or null member is absent; any other non-object value is invalid.
fn optional_object<'a>(
    payload: &'a Map<String, Value>,
    name: &str,
) -> Result<Option<&'a Map<String, Value>>, FetchError> {
    match payload.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_object()
            .map(Some)
            .ok_or(FetchError::InvalidResponse),
    }
}

fn parse_limit_windows(limit: &Map<String, Value>) -> (Option<LimitWindow>, Option<LimitWindow>) {
    let mut five_hour = None;
    let mut weekly = None;
    for name in ["primary", "secondary"] {
        let Some(window) = parse_window(limit, name) else {
            continue;
        };
        if window.window_minutes == FIVE_HOUR_MINUTES && five_hour.is_none() {
            five_hour = Some(window);
        } else if window.window_minutes == WEEKLY_MINUTES && weekly.is_none() {
            weekly = Some(window);
        }
    }
    (five_hour, weekly)
}

fn parse_window(limit: &Map<String, Value>, name: &str) -> Option<LimitWindow> {
    let window = limit.get(name)?.as_object()?;
    let window_minutes = u32::try_from(integer(window.get("windowDurationMins")?)?).ok()?;
    if window_minutes != FIVE_HOUR_MINUTES && window_minutes != WEEKLY_MINUTES {
        return None;
    }
    let used_percent = window.get("usedPercent")?.as_f64()?;
    if !(0.0..=100.0).contains(&used_percent) {
        return None;
    }
    Some(LimitWindow {
        used_percent,
        window_minutes,
        resets_at: window.get("resetsAt").and_then(integer),
    })
}

fn parse_available_resets(reset_credits: &Map<String, Value>) -> Vec<ResetCredit> {
    let Some(credits) = reset_credits.get("credits").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut available: Vec<ResetCredit> = credits
        .iter()
        .filter_map(Value::as_object)
        .filter(|credit| {
            optional_string(credit, "status")
                .is_none_or(|status| status.eq_ignore_ascii_case("available"))
        })
        .map(|credit| ResetCredit {
            id: optional_string(credit, "id"),
            reset_type: optional_string(credit, "resetType"),
            title: optional_string(credit, "title"),
            expires_at: credit.get("expiresAt").and_then(integer),
        })
        .collect();
    available.sort_by_key(|credit| credit.expires_at.unwrap_or(i64::MAX));
    available
}

fn from_rpc_error(error: &Map<String, Value>) -> FetchError {
    let message = optional_string(error, "message");
    if message.as_deref().is_some_and(is_authentication_message) {
        FetchError::AuthenticationRequired
    } else {
        FetchError::RequestRejected(message.as_deref().and_then(compact_error_detail))
    }
}

fn is_authentication_message(message: &str) -> bool {
    let lower = message.to_lowercase();
    [
        "not logged in",
        "not signed in",
        "unauthorized",
        "authentication",
        "codex login",
        "401",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// Collapses whitespace and control characters so a server message fits on one line.
fn compact_error_detail(value: &str) -> Option<String> {
    let compact = value
        .split(|character: char| character.is_control() || character.is_whitespace())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let limited: String = compact.chars().take(MAX_ERROR_DETAIL_CHARS).collect();
    let limited = limited.trim_end();
    (!limited.is_empty()).then(|| limited.to_string())
}

fn optional_string(values: &Map<String, Value>, name: &str) -> Option<String> {
    let text = values.get(name)?.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Accepts JSON integers and integer-valued floats, as the app-server may send either.
fn integer(value: &Value) -> Option<i64> {
    if let Some(whole) = value.as_i64() {
        return Some(whole);
    }
    let number = value.as_f64()?;
    let in_range = number >= i64::MIN as f64 && number < i64::MAX as f64;
    (number.fract() == 0.0 && in_range).then_some(number as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/rate-limits.json");

    fn parse(json: &str) -> Result<Usage, FetchError> {
        parse_rate_limits(&serde_json::from_str(json).unwrap(), 1_783_700_000)
    }

    #[test]
    fn fixture_yields_primary_limits_and_available_resets() {
        let usage = parse(FIXTURE).unwrap();
        assert_eq!(usage.updated_at, 1_783_700_000);

        let five_hour = usage.five_hour.unwrap();
        assert_eq!(five_hour.used_percent, 20.0);
        assert_eq!(five_hour.window_minutes, FIVE_HOUR_MINUTES);
        assert_eq!(five_hour.resets_at, Some(1_783_710_153));

        let weekly = usage.weekly.unwrap();
        assert_eq!(weekly.used_percent, 22.0);
        assert_eq!(weekly.resets_at, Some(1_784_238_427));

        assert_eq!(usage.reset_count, Some(4));
        let expirations: Vec<_> = usage
            .resets
            .iter()
            .map(|credit| credit.expires_at)
            .collect();
        assert_eq!(
            expirations,
            [
                Some(1_783_820_680),
                Some(1_784_334_851),
                Some(1_785_109_634),
                Some(1_785_528_002)
            ]
        );
        assert_eq!(
            usage.resets[0].title.as_deref(),
            Some("Full reset (Weekly + 5 hr)")
        );
        assert_eq!(usage.resets[0].id.as_deref(), Some("credit-1"));
        assert_eq!(
            usage.resets[0].reset_type.as_deref(),
            Some("codexRateLimits")
        );
        assert_eq!(usage.plan_type.as_deref(), Some("pro"));

        // The account's own "codex" entry is not repeated as an extra limit.
        assert_eq!(usage.additional_limits.len(), 1);
        let extra = &usage.additional_limits[0];
        assert_eq!(extra.limit_id, "codex_bengalfox");
        assert_eq!(extra.display_name, "GPT-5.3-Codex-Spark");
        assert_eq!(extra.five_hour.as_ref().unwrap().used_percent, 0.0);
        assert_eq!(
            extra.weekly.as_ref().unwrap().resets_at,
            Some(1_784_305_891)
        );
    }

    #[test]
    fn redemption_outcomes_are_read_from_the_response() {
        let outcome = |json: &str| parse_redemption(&serde_json::from_str(json).unwrap());
        assert_eq!(
            outcome(r#"{"id":2,"result":{"outcome":"reset"}}"#),
            RedemptionOutcome::Reset
        );
        assert_eq!(
            outcome(r#"{"id":2,"result":{"outcome":"AlreadyRedeemed"}}"#),
            RedemptionOutcome::AlreadyRedeemed
        );
        assert_eq!(
            outcome(r#"{"id":2,"result":{"outcome":"nothingToReset"}}"#),
            RedemptionOutcome::NothingToReset
        );
        assert_eq!(
            outcome(r#"{"id":2,"result":{"outcome":"noCredit"}}"#),
            RedemptionOutcome::NoCredit
        );

        let failure = |json: &str| match outcome(json) {
            RedemptionOutcome::Failed(message) => message,
            other => panic!("expected a failure, got {other:?}"),
        };
        assert_eq!(
            failure(r#"{"id":2,"result":{"outcome":"mystery"}}"#),
            "Codex returned an unrecognized reset result."
        );
        assert_eq!(
            failure(r#"{"id":2,"result":5}"#),
            "Codex returned invalid reset data."
        );
        assert_eq!(
            failure(r#"{"id":2,"error":{"message":"401 Unauthorized"}}"#),
            "Codex is not signed in. Run codex login."
        );
        assert_eq!(
            failure(r#"{"id":2,"error":{"message":"credit\n expired"}}"#),
            "Codex rejected the reset request: credit expired"
        );
    }

    #[test]
    fn redemption_requires_a_credit_and_an_idempotency_key() {
        assert_eq!(
            consume_reset_credit(" ", "key"),
            RedemptionOutcome::Failed("The reset credit did not include an ID.".to_string())
        );
        assert_eq!(
            consume_reset_credit("credit-1", ""),
            RedemptionOutcome::Failed(
                "The reset request did not include an idempotency key.".to_string()
            )
        );
    }

    #[test]
    fn mapped_codex_limits_are_used_when_direct_limits_are_absent() {
        let usage = parse(
            r#"{"id":2,"result":{"rateLimits":null,"rateLimitsByLimitId":{"CODEX":{
                "primary":{"usedPercent":12.5,"windowDurationMins":10080.0,"resetsAt":1784238427}}}}}"#,
        )
        .unwrap();
        assert!(usage.five_hour.is_none());
        assert_eq!(usage.weekly.unwrap().used_percent, 12.5);
        assert!(usage.resets.is_empty());
        assert_eq!(usage.reset_count, None);
    }

    #[test]
    fn invalid_windows_are_reported_as_unavailable() {
        for window in [
            r#"{"usedPercent":101,"windowDurationMins":300}"#,
            r#"{"usedPercent":"20","windowDurationMins":300}"#,
            r#"{"usedPercent":20,"windowDurationMins":60}"#,
            r#"{"usedPercent":20,"windowDurationMins":300.5}"#,
        ] {
            let json = format!(r#"{{"id":2,"result":{{"rateLimits":{{"primary":{window}}}}}}}"#);
            assert_eq!(parse(&json), Err(FetchError::LimitsUnavailable), "{window}");
        }
    }

    #[test]
    fn malformed_payloads_are_invalid() {
        assert_eq!(parse(r#"[1,2]"#), Err(FetchError::InvalidResponse));
        assert_eq!(
            parse(r#"{"id":2,"result":7}"#),
            Err(FetchError::InvalidResponse)
        );
        assert_eq!(
            parse(r#"{"id":2,"result":{"rateLimits":"none"}}"#),
            Err(FetchError::InvalidResponse)
        );
    }

    #[test]
    fn rpc_errors_are_classified() {
        assert_eq!(
            parse(r#"{"id":2,"error":{"message":"Not logged in"}}"#),
            Err(FetchError::AuthenticationRequired)
        );
        assert_eq!(
            parse(r#"{"id":2,"error":{"message":"  backend\n unavailable\t now "}}"#),
            Err(FetchError::RequestRejected(Some(
                "backend unavailable now".to_string()
            )))
        );
        assert_eq!(
            parse(r#"{"id":2,"error":{"code":-1}}"#),
            Err(FetchError::RequestRejected(None))
        );
    }

    #[test]
    fn response_ids_match_numbers_and_numeric_strings() {
        assert!(has_request_id(&json!({ "id": 2 }), 2));
        assert!(has_request_id(&json!({ "id": 2.0 }), 2));
        assert!(has_request_id(&json!({ "id": "2" }), 2));
        assert!(!has_request_id(&json!({ "id": 1 }), 2));
        assert!(!has_request_id(&json!({ "method": "notice" }), 2));
    }

    /// Talks to the installed Codex CLI. Run with `cargo test -- --ignored --nocapture`.
    #[test]
    #[ignore = "requires a signed-in Codex CLI"]
    fn live_fetch_returns_usage() {
        let usage = fetch_usage().expect("live Codex usage");
        let describe = |window: &Option<LimitWindow>| {
            window
                .as_ref()
                .map(|window| (window.remaining_percent(), window.resets_at))
        };
        println!("weekly: {:?}", describe(&usage.weekly));
        println!("5-hour: {:?}", describe(&usage.five_hour));
        println!(
            "resets: {:?} listed, count {:?}",
            usage.resets.len(),
            usage.reset_count
        );
        assert!(usage.weekly.is_some() || usage.five_hour.is_some());
    }
}
