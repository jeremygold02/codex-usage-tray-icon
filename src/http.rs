//! A small blocking HTTPS client over WinHTTP, so the app needs no TLS stack
//! of its own. Only `https://` URLs are accepted.

use std::ffi::c_void;
use std::io::Write;
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows_sys::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption,
    WinHttpSetTimeouts, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER, WINHTTP_QUERY_CUSTOM,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
};
use windows_sys::Win32::System::Time::SystemTimeToFileTime;

use crate::win::{from_wide, wide};

const DEFAULT_PORT: u16 = 443;
const CHUNK_BYTES: usize = 16_384;

pub struct Request<'a> {
    /// `"GET"` or `"POST"`.
    pub method: &'a str,
    pub url: &'a str,
    pub headers: &'a [(&'a str, &'a str)],
    pub body: Option<&'a [u8]>,
    /// Sent as the `User-Agent` header when present.
    pub user_agent: Option<&'a str>,
    /// Leave false whenever a credential is attached, so it is only ever
    /// sent to the host named in `url`.
    pub follow_redirects: bool,
    /// Responses with a larger body fail instead of being truncated.
    pub max_bytes: usize,
    pub timeout_milliseconds: i32,
}

impl<'a> Request<'a> {
    /// A request without body, user agent or redirects.
    pub fn get(url: &'a str, headers: &'a [(&'a str, &'a str)], max_bytes: usize) -> Self {
        Self {
            method: "GET",
            url,
            headers,
            body: None,
            user_agent: None,
            follow_redirects: false,
            max_bytes,
            timeout_milliseconds: 10_000,
        }
    }
}

pub struct Response {
    pub status: u32,
    pub body: Vec<u8>,
    /// The server's delay or UTC deadline, if the `Retry-After` header is valid.
    pub retry_after: RetryAfter,
}

/// An HTTP Retry-After value, kept independent of the clock until a client
/// decides when to retry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryAfter {
    Seconds(u64),
    Date(i64),
}

impl RetryAfter {
    /// Returns a nonnegative delay, matching the C# client's 32-bit bound.
    pub fn delay_seconds(self, now: i64) -> i64 {
        match self {
            Self::Seconds(seconds) => i64::from(i32::try_from(seconds).unwrap_or(0)),
            Self::Date(deadline) => deadline.saturating_sub(now).clamp(0, i64::from(i32::MAX)),
        }
    }
}

fn parse_retry_after(value: &str) -> RetryAfter {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return RetryAfter::Seconds(seconds);
    }
    parse_http_date(value).map_or(RetryAfter::Seconds(0), RetryAfter::Date)
}

/// Parses HTTP's GMT date formats without consulting the system clock.
fn parse_http_date(value: &str) -> Option<i64> {
    const UNIX_EPOCH_AS_FILETIME_SECONDS: i64 = 11_644_473_600;
    let parts: Vec<&str> = value.split_ascii_whitespace().collect();
    let (weekday, day, month, year, time) = match parts.as_slice() {
        [weekday, day, month, year, time, zone] if zone.eq_ignore_ascii_case("GMT") => {
            (*weekday, *day, *month, *year, *time)
        }
        // Obsolete RFC 850 form, still accepted in HTTP responses.
        [weekday, date, time, zone] if zone.eq_ignore_ascii_case("GMT") => {
            let mut date = date.split('-');
            let fields = (date.next()?, date.next()?, date.next()?);
            if date.next().is_some() {
                return None;
            }
            (*weekday, fields.0, fields.1, fields.2, *time)
        }
        // Obsolete asctime form. HTTP defines its zone as GMT.
        [weekday, month, day, time, year] => (*weekday, *day, *month, *year, *time),
        _ => return None,
    };
    let weekday = weekday.trim_end_matches(',');
    if ![
        "Mon",
        "Tue",
        "Wed",
        "Thu",
        "Fri",
        "Sat",
        "Sun",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
    ]
    .iter()
    .any(|name| weekday.eq_ignore_ascii_case(name))
    {
        return None;
    }
    let number = |text: &str| -> Option<u16> {
        (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| text.parse().ok())?
    };
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .iter()
    .position(|name| month.eq_ignore_ascii_case(name))? as u16
        + 1;
    let mut year_number = number(year)?;
    match year.len() {
        2 => year_number += if year_number <= 29 { 2000 } else { 1900 },
        4 => {}
        _ => return None,
    }
    let mut clock = time.split(':');
    let (hour, minute, second) = (
        number(clock.next()?)?,
        number(clock.next()?)?,
        number(clock.next()?)?,
    );
    if clock.next().is_some() {
        return None;
    }
    let system_time = SYSTEMTIME {
        wYear: year_number,
        wMonth: month,
        wDay: number(day)?,
        wHour: hour,
        wMinute: minute,
        wSecond: second,
        ..SYSTEMTIME::default()
    };
    let mut file_time = FILETIME::default();
    // SAFETY: both pointers refer to live structures, and the system function
    // validates the date and time before writing the UTC FILETIME.
    if unsafe { SystemTimeToFileTime(&system_time, &mut file_time) } == 0 {
        return None;
    }
    let ticks = (u64::from(file_time.dwHighDateTime) << 32) | u64::from(file_time.dwLowDateTime);
    let seconds = (ticks / 10_000_000) as i64 - UNIX_EPOCH_AS_FILETIME_SECONDS;
    Some(seconds)
}

/// Sends `request` and reads the whole response into memory. Returns `None`
/// on any transport failure or when the body exceeds `max_bytes`.
pub fn send(request: &Request) -> Option<Response> {
    let mut body = Vec::new();
    let (status, retry_after) = exchange(request, |chunk| {
        body.extend_from_slice(chunk);
        true
    })?;
    Some(Response {
        status,
        body,
        retry_after,
    })
}

/// Sends `request` and streams a `200` response body to `destination`,
/// returning the HTTP status. The file is removed if the transfer fails.
pub fn download(request: &Request, destination: &Path) -> Option<u32> {
    let mut file = std::fs::File::create(destination).ok()?;
    let mut write_failed = false;
    let outcome = exchange(request, |chunk| {
        write_failed = file.write_all(chunk).is_err();
        !write_failed
    });
    let flushed = file.flush().is_ok();
    drop(file);
    match outcome {
        Some((200, _)) if flushed && !write_failed => Some(200),
        Some((status, _)) if status != 200 => {
            let _ = std::fs::remove_file(destination);
            Some(status)
        }
        _ => {
            let _ = std::fs::remove_file(destination);
            None
        }
    }
}

struct Url {
    host: String,
    port: u16,
    path: String,
}

fn parse_url(url: &str) -> Option<Url> {
    let rest = url.strip_prefix("https://")?;
    let (authority, path) = match rest.find(['/', '?']) {
        Some(index) if rest.as_bytes()[index] == b'/' => {
            (&rest[..index], rest[index..].to_string())
        }
        Some(index) => (&rest[..index], format!("/{}", &rest[index..])),
        None => (rest, "/".to_string()),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, port.parse().ok()?),
        None => (authority, DEFAULT_PORT),
    };
    let valid_host = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    valid_host.then(|| Url {
        host: host.to_string(),
        port,
        path,
    })
}

/// Joins headers into the block WinHTTP expects, refusing any that could
/// smuggle in an extra header line.
fn header_block(headers: &[(&str, &str)]) -> Option<String> {
    let safe = |text: &str| !text.contains(['\r', '\n']);
    headers
        .iter()
        .map(|(name, value)| (safe(name) && safe(value)).then(|| format!("{name}: {value}")))
        .collect::<Option<Vec<_>>>()
        .map(|lines| lines.join("\r\n"))
}

struct InternetHandle(*mut c_void);

impl Drop for InternetHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the handle came from WinHTTP and is closed exactly once.
            unsafe {
                WinHttpCloseHandle(self.0);
            }
        }
    }
}

/// Performs the request and feeds the response body to `sink` in chunks.
/// Returns the status and `Retry-After` seconds once the body is complete;
/// `sink` returning false, or more than `max_bytes`, aborts with `None`.
fn exchange(request: &Request, mut sink: impl FnMut(&[u8]) -> bool) -> Option<(u32, RetryAfter)> {
    let url = parse_url(request.url)?;
    let host = wide(&url.host);
    let path = wide(&url.path);
    let method = wide(request.method);
    let headers = wide(&header_block(request.headers)?);
    let user_agent = request.user_agent.map(wide);
    let body = request.body.unwrap_or_default();
    let body_length = u32::try_from(body.len()).ok()?;
    let timeout = request.timeout_milliseconds;

    // SAFETY: every pointer passed below refers to a live local buffer whose
    // length matches the size argument; handles are closed by InternetHandle.
    unsafe {
        let session = InternetHandle(WinHttpOpen(
            user_agent.as_ref().map_or(null(), |agent| agent.as_ptr()),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            null(),
            null(),
            0,
        ));
        if session.0.is_null() {
            return None;
        }
        WinHttpSetTimeouts(session.0, timeout, timeout, timeout, timeout);
        let connection = InternetHandle(WinHttpConnect(session.0, host.as_ptr(), url.port, 0));
        if connection.0.is_null() {
            return None;
        }
        let handle = InternetHandle(WinHttpOpenRequest(
            connection.0,
            method.as_ptr(),
            path.as_ptr(),
            null(),
            null(),
            null(),
            WINHTTP_FLAG_SECURE,
        ));
        if handle.0.is_null() {
            return None;
        }
        if !request.follow_redirects {
            let policy: u32 = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
            if WinHttpSetOption(
                handle.0,
                WINHTTP_OPTION_REDIRECT_POLICY,
                (&policy as *const u32).cast(),
                size_of::<u32>() as u32,
            ) == 0
            {
                return None;
            }
        }
        // A header length of -1 means "null-terminated"; an empty block is skipped.
        let (header_pointer, header_length) = if headers.len() > 1 {
            (headers.as_ptr(), u32::MAX)
        } else {
            (null(), 0)
        };
        let body_pointer: *const c_void = if body.is_empty() {
            null()
        } else {
            body.as_ptr().cast()
        };
        if WinHttpSendRequest(
            handle.0,
            header_pointer,
            header_length,
            body_pointer,
            body_length,
            body_length,
            0,
        ) == 0
        {
            return None;
        }
        if WinHttpReceiveResponse(handle.0, null_mut()) == 0 {
            return None;
        }

        let mut status = 0u32;
        let mut status_size = size_of::<u32>() as u32;
        if WinHttpQueryHeaders(
            handle.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            (&mut status as *mut u32).cast(),
            &mut status_size,
            null_mut(),
        ) == 0
        {
            return None;
        }

        let name = wide("Retry-After");
        let mut retry_after = [0u16; 128];
        let mut retry_after_size = (retry_after.len() * size_of::<u16>()) as u32;
        let retry_after = if WinHttpQueryHeaders(
            handle.0,
            WINHTTP_QUERY_CUSTOM,
            name.as_ptr(),
            retry_after.as_mut_ptr().cast(),
            &mut retry_after_size,
            null_mut(),
        ) != 0
        {
            parse_retry_after(&from_wide(&retry_after))
        } else {
            RetryAfter::Seconds(0)
        };

        let mut received = 0usize;
        let mut chunk = vec![0u8; CHUNK_BYTES];
        loop {
            let mut read = 0u32;
            if WinHttpReadData(
                handle.0,
                chunk.as_mut_ptr().cast(),
                chunk.len() as u32,
                &mut read,
            ) == 0
            {
                return None;
            }
            if read == 0 {
                break;
            }
            received += read as usize;
            if received > request.max_bytes || !sink(&chunk[..read as usize]) {
                return None;
            }
        }
        Some((status, retry_after))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_urls_split_into_host_port_and_path() {
        let url = parse_url("https://api.example.com/v1/items?page=2").unwrap();
        assert_eq!((url.host.as_str(), url.port), ("api.example.com", 443));
        assert_eq!(url.path, "/v1/items?page=2");

        let url = parse_url("https://localhost:8443").unwrap();
        assert_eq!(
            (url.host.as_str(), url.port, url.path.as_str()),
            ("localhost", 8443, "/")
        );

        let url = parse_url("https://example.com?x=1").unwrap();
        assert_eq!(url.path, "/?x=1");
    }

    #[test]
    fn unsupported_urls_are_rejected() {
        assert!(parse_url("http://example.com/").is_none());
        assert!(parse_url("https:///path").is_none());
        assert!(parse_url("https://user@example.com/").is_none());
        assert!(parse_url("https://example.com:port/").is_none());
    }

    #[test]
    fn headers_cannot_inject_extra_lines() {
        assert_eq!(
            header_block(&[("Accept", "application/json"), ("X-Test", "1")]).as_deref(),
            Some("Accept: application/json\r\nX-Test: 1")
        );
        assert_eq!(header_block(&[]).as_deref(), Some(""));
        assert!(header_block(&[("Authorization", "Bearer a\r\nX-Evil: 1")]).is_none());
    }

    #[test]
    fn retry_after_accepts_seconds_and_http_dates() {
        assert_eq!(parse_retry_after(" 900 "), RetryAfter::Seconds(900));
        assert_eq!(
            parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT"),
            RetryAfter::Date(1_445_412_480)
        );
        for value in [
            "Sun, 06 Nov 1994 08:49:37 GMT",
            "Sunday, 06-Nov-94 08:49:37 GMT",
            "Sun Nov  6 08:49:37 1994",
        ] {
            assert_eq!(parse_retry_after(value), RetryAfter::Date(784_111_777));
        }
        for value in [
            "",
            "not a date",
            "-10",
            "Wed, 31 Feb 2015 07:28:00 GMT",
            "Wed, 21 Oct 2015 25:28:00 GMT",
            "Wed, 21 Oct 2015 07:28:00 GMT\0extra",
        ] {
            assert_eq!(
                parse_retry_after(value),
                RetryAfter::Seconds(0),
                "{value:?}"
            );
        }
    }

    #[test]
    fn retry_after_delays_use_the_supplied_time_and_csharp_bounds() {
        let deadline = RetryAfter::Date(1_445_412_480);
        assert_eq!(deadline.delay_seconds(1_445_411_579), 901);
        assert_eq!(deadline.delay_seconds(1_445_412_480), 0);
        assert_eq!(deadline.delay_seconds(1_445_412_481), 0);
        assert_eq!(
            RetryAfter::Date(i64::MAX).delay_seconds(i64::MIN),
            i64::from(i32::MAX)
        );
        assert_eq!(RetryAfter::Seconds(u64::MAX).delay_seconds(0), 0);
    }
}
