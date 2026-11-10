use std::ptr::null;
use std::time::{SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows_sys::Win32::Globalization::{
    GetDateFormatEx, GetTimeFormatEx, DATE_SHORTDATE, TIME_NOSECONDS,
};
use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

use crate::win::{from_wide, wide};

const UNIX_EPOCH_AS_FILETIME_SECONDS: i64 = 11_644_473_600;

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

pub fn format_duration(total_seconds: i64) -> String {
    let seconds = total_seconds.max(0);
    if seconds >= 86_400 {
        format!("{}d {}h", seconds / 86_400, seconds % 86_400 / 3_600)
    } else if seconds >= 3_600 {
        format!("{}h {}m", seconds / 3_600, seconds % 3_600 / 60)
    } else if seconds >= 60 {
        format!("{}m", seconds / 60)
    } else {
        format!("{seconds}s")
    }
}

/// Local short time in the user's locale, such as `8:38 PM`.
pub fn format_clock(unix: i64) -> String {
    local_time(unix)
        .map(|time| time_text(&time))
        .unwrap_or_else(|| "?".to_string())
}

/// Local date and time, such as `Nov 5, 8:38 PM`. Dates outside the current
/// year use the locale's short date so the year stays visible.
pub fn format_date_time(unix: i64, now: i64) -> String {
    let Some(time) = local_time(unix) else {
        return "?".to_string();
    };
    let same_year = local_time(now).is_some_and(|current| current.wYear == time.wYear);
    format!("{}, {}", date_text(&time, same_year), time_text(&time))
}

/// Parses an RFC 3339 timestamp into Unix seconds. A missing offset is read as UTC.
pub fn parse_rfc3339(text: &str) -> Option<i64> {
    let bytes = text.trim().as_bytes();
    let number = |start: usize, length: usize| -> Option<i64> {
        let digits = bytes.get(start..start + length)?;
        if !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(digits).ok()?.parse().ok()
    };
    let separator = |index: usize, expected: &[u8]| {
        bytes
            .get(index)
            .is_some_and(|value| expected.contains(value))
    };

    if !separator(4, b"-") || !separator(7, b"-") || !separator(10, b"Tt ") {
        return None;
    }
    if !separator(13, b":") || !separator(16, b":") {
        return None;
    }
    let (year, month, day) = (number(0, 4)?, number(5, 2)?, number(8, 2)?);
    let (hour, minute, second) = (number(11, 2)?, number(14, 2)?, number(17, 2)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let mut index = 19;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
    }
    let offset_seconds = match bytes.get(index) {
        None => 0,
        Some(b'Z' | b'z') if index + 1 == bytes.len() => 0,
        Some(sign @ (b'+' | b'-')) if index + 6 == bytes.len() && separator(index + 3, b":") => {
            let magnitude = number(index + 1, 2)? * 3_600 + number(index + 4, 2)? * 60;
            if *sign == b'-' {
                -magnitude
            } else {
                magnitude
            }
        }
        _ => return None,
    };

    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second - offset_seconds)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn local_time(unix: i64) -> Option<SYSTEMTIME> {
    let ticks = unix
        .checked_add(UNIX_EPOCH_AS_FILETIME_SECONDS)?
        .checked_mul(10_000_000)?;
    if ticks < 0 {
        return None;
    }
    let file_time = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    // SAFETY: both calls only write to the SYSTEMTIME values passed to them.
    unsafe {
        let mut utc: SYSTEMTIME = std::mem::zeroed();
        let mut local: SYSTEMTIME = std::mem::zeroed();
        if FileTimeToSystemTime(&file_time, &mut utc) == 0 {
            return None;
        }
        if SystemTimeToTzSpecificLocalTime(null(), &utc, &mut local) == 0 {
            return None;
        }
        Some(local)
    }
}

fn time_text(time: &SYSTEMTIME) -> String {
    let mut buffer = [0u16; 64];
    // SAFETY: the buffer length passed matches the buffer.
    let written = unsafe {
        GetTimeFormatEx(
            null(),
            TIME_NOSECONDS,
            time,
            null(),
            buffer.as_mut_ptr(),
            buffer.len() as i32,
        )
    };
    if written > 0 {
        from_wide(&buffer)
    } else {
        format!("{:02}:{:02}", time.wHour, time.wMinute)
    }
}

fn date_text(time: &SYSTEMTIME, same_year: bool) -> String {
    let mut buffer = [0u16; 64];
    let pattern = wide("MMM d");
    // SAFETY: the buffer length passed matches the buffer; the pattern is null-terminated.
    let written = unsafe {
        if same_year {
            GetDateFormatEx(
                null(),
                0,
                time,
                pattern.as_ptr(),
                buffer.as_mut_ptr(),
                buffer.len() as i32,
                null(),
            )
        } else {
            GetDateFormatEx(
                null(),
                DATE_SHORTDATE,
                time,
                null(),
                buffer.as_mut_ptr(),
                buffer.len() as i32,
                null(),
            )
        }
    };
    if written > 0 {
        from_wide(&buffer)
    } else {
        format!("{:04}-{:02}-{:02}", time.wYear, time.wMonth, time.wDay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_match_the_existing_format() {
        assert_eq!(format_duration(-5), "0s");
        assert_eq!(format_duration(42), "42s");
        assert_eq!(format_duration(7_200), "2h 0m");
        assert_eq!(format_duration(6_300), "1h 45m");
        assert_eq!(format_duration(3 * 86_400 + 3 * 3_600), "3d 3h");
        assert_eq!(format_duration(125), "2m");
    }

    #[test]
    fn rfc3339_timestamps_convert_to_unix_seconds() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2000-01-01T00:00:00Z"), Some(946_684_800));
        assert_eq!(
            parse_rfc3339("2024-02-29T12:00:00+02:00"),
            Some(1_709_200_800)
        );
        assert_eq!(
            parse_rfc3339("2024-02-29T10:00:00.123456+00:00"),
            Some(1_709_200_800)
        );
        assert_eq!(parse_rfc3339("2024-02-29T10:00:00"), Some(1_709_200_800));
        assert_eq!(parse_rfc3339("2024-02-29"), None);
        assert_eq!(parse_rfc3339("not a timestamp at all"), None);
    }
}
