//! Calendar and time-zone arithmetic for the usage history and its forecasts.
//!
//! History timestamps are UTC milliseconds, the precision of the history
//! file. Learned routines and daily totals follow the user's wall clock, so
//! they need the Windows time zone with its daylight-saving rules. The zone is
//! a value handed in by the caller, which keeps that logic deterministic under
//! test and lets it run without asking Windows for every conversion.

use std::cell::Cell;
use std::ptr::null;

use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::Globalization::{GetDateFormatEx, GetLocaleInfoEx, LOCALE_SDECIMAL};
use windows_sys::Win32::System::Time::{
    GetDynamicTimeZoneInformation, GetTimeZoneInformationForYear, DYNAMIC_TIME_ZONE_INFORMATION,
    TIME_ZONE_ID_INVALID, TIME_ZONE_INFORMATION,
};

use crate::win::{from_wide, wide};

/// Milliseconds in a second.
pub const MS_PER_SECOND: i64 = 1_000;
/// Milliseconds in a minute.
pub const MS_PER_MINUTE: i64 = 60_000;
/// Milliseconds in an hour.
pub const MS_PER_HOUR: i64 = 3_600_000;
/// Milliseconds in a day of 24 hours.
pub const MS_PER_DAY: i64 = 86_400_000;

/// The earliest instant .NET's `DateTime` can hold, in Unix milliseconds. The
/// history file is shared with the C# app, which cannot load anything outside
/// its range.
pub const MIN_TIMESTAMP_MS: i64 = -62_135_596_800_000;
/// The latest instant .NET's `DateTime` can hold, in Unix milliseconds.
pub const MAX_TIMESTAMP_MS: i64 = 253_402_300_799_999;

/// Unix seconds as milliseconds, or `None` outside the range the history file can hold.
pub fn seconds_to_ms(seconds: i64) -> Option<i64> {
    seconds
        .checked_mul(MS_PER_SECOND)
        .filter(|ms| (MIN_TIMESTAMP_MS..=MAX_TIMESTAMP_MS).contains(ms))
}

/// The caller's clock reading in milliseconds, held inside the supported range.
pub fn now_to_ms(now: i64) -> i64 {
    now.saturating_mul(MS_PER_SECOND)
        .clamp(MIN_TIMESTAMP_MS, MAX_TIMESTAMP_MS)
}

/// .NET ticks (100 ns) in a millisecond.
pub const TICKS_PER_MS: i64 = 10_000;
// The .NET Framework's `TimeSpan.TotalHours`, `TotalMinutes` and
// `TotalSeconds` multiply the tick count by these rounded reciprocals instead
// of dividing, so 54 minutes comes out just under 0.9 hours. The same
// arithmetic is used here to keep sums of durations on the same side of the
// forecast's thresholds as in the C# app.
const HOURS_PER_TICK: f64 = 1.0 / 36_000_000_000.0;
const MINUTES_PER_TICK: f64 = 1.0 / 600_000_000.0;
const SECONDS_PER_TICK: f64 = 1.0 / 10_000_000.0;

fn ticks_between(later_ms: i64, earlier_ms: i64) -> f64 {
    later_ms
        .saturating_sub(earlier_ms)
        .saturating_mul(TICKS_PER_MS) as f64
}

/// Fractional hours from `earlier_ms` to `later_ms`.
pub fn hours_between(later_ms: i64, earlier_ms: i64) -> f64 {
    ticks_between(later_ms, earlier_ms) * HOURS_PER_TICK
}

/// Fractional minutes from `earlier_ms` to `later_ms`.
pub fn minutes_between(later_ms: i64, earlier_ms: i64) -> f64 {
    ticks_between(later_ms, earlier_ms) * MINUTES_PER_TICK
}

/// Fractional seconds from `earlier_ms` to `later_ms`.
pub fn seconds_between(later_ms: i64, earlier_ms: i64) -> f64 {
    ticks_between(later_ms, earlier_ms) * SECONDS_PER_TICK
}

/// Adds fractional hours, rounding to the millisecond the way .NET's
/// `DateTime.AddHours` does so projected times match the C# app's.
pub fn add_hours(timestamp_ms: i64, hours: f64) -> i64 {
    let offset = hours * MS_PER_HOUR as f64;
    let rounded = (offset + if offset >= 0.0 { 0.5 } else { -0.5 }) as i64;
    timestamp_ms.saturating_add(rounded)
}

/// The start of the UTC clock hour containing `timestamp_ms`.
pub fn floor_hour(timestamp_ms: i64) -> i64 {
    timestamp_ms.div_euclid(MS_PER_HOUR) * MS_PER_HOUR
}

fn minutes_to_ms(minutes: i32) -> i64 {
    i64::from(minutes) * MS_PER_MINUTE
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

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
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
    (year, month, day)
}

/// A calendar date without a time zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocalDate {
    days_since_epoch: i64,
}

impl LocalDate {
    /// The date with the given year, month (1 to 12) and day of the month.
    #[cfg(test)]
    pub fn from_ymd(year: i32, month: u32, day: u32) -> Self {
        Self {
            days_since_epoch: days_from_civil(i64::from(year), i64::from(month), i64::from(day)),
        }
    }

    /// The calendar year.
    pub fn year(self) -> i32 {
        civil_from_days(self.days_since_epoch).0 as i32
    }

    /// The month, from 1 to 12.
    pub fn month(self) -> u32 {
        civil_from_days(self.days_since_epoch).1 as u32
    }

    /// The day of the month, from 1.
    pub fn day(self) -> u32 {
        civil_from_days(self.days_since_epoch).2 as u32
    }

    /// Day of the week, counted from Sunday as zero.
    pub fn weekday(self) -> u32 {
        // 1970-01-01 was a Thursday.
        (self.days_since_epoch + 4).rem_euclid(7) as u32
    }

    /// True on Saturdays and Sundays.
    pub fn is_weekend(self) -> bool {
        matches!(self.weekday(), 0 | 6)
    }

    /// The date `days` later, or earlier when negative.
    pub fn plus_days(self, days: i64) -> Self {
        Self {
            days_since_epoch: self.days_since_epoch.saturating_add(days),
        }
    }

    /// Whole days from `earlier` to this date; negative when this date is first.
    pub fn days_since(self, earlier: Self) -> i64 {
        self.days_since_epoch
            .saturating_sub(earlier.days_since_epoch)
    }
}

/// A wall-clock date and time in some zone, to the millisecond.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LocalTime {
    wall_ms: i64,
}

impl LocalTime {
    /// The given time of day on `date`.
    pub fn new(date: LocalDate, hour: u32, minute: u32, second: u32) -> Self {
        let time_of_day = i64::from(hour) * MS_PER_HOUR
            + i64::from(minute) * MS_PER_MINUTE
            + i64::from(second) * MS_PER_SECOND;
        Self {
            wall_ms: date
                .days_since_epoch
                .saturating_mul(MS_PER_DAY)
                .saturating_add(time_of_day),
        }
    }

    /// The start of `date`.
    pub fn midnight(date: LocalDate) -> Self {
        Self::new(date, 0, 0, 0)
    }

    /// A wall-clock time from the milliseconds its clock shows since 1970-01-01 00:00.
    pub fn from_wall_ms(wall_ms: i64) -> Self {
        Self { wall_ms }
    }

    /// Milliseconds this wall clock shows since 1970-01-01 00:00, for step arithmetic.
    pub fn wall_ms(self) -> i64 {
        self.wall_ms
    }

    /// The calendar date.
    pub fn date(self) -> LocalDate {
        LocalDate {
            days_since_epoch: self.wall_ms.div_euclid(MS_PER_DAY),
        }
    }

    /// The hour of the day, from 0 to 23.
    pub fn hour(self) -> u32 {
        (self.wall_ms.rem_euclid(MS_PER_DAY) / MS_PER_HOUR) as u32
    }

    /// The minute of the hour.
    pub fn minute(self) -> u32 {
        (self.wall_ms.rem_euclid(MS_PER_HOUR) / MS_PER_MINUTE) as u32
    }

    /// The second of the minute.
    pub fn second(self) -> u32 {
        (self.wall_ms.rem_euclid(MS_PER_MINUTE) / MS_PER_SECOND) as u32
    }
}

/// The day of the month on which a daylight-saving change happens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransitionDay {
    /// The nth given weekday of the month (Sunday is zero); five means the last one.
    Weekday { weekday: u32, occurrence: u32 },
    /// A fixed day of the month.
    Date(u32),
}

/// When a daylight-saving change happens each year, on the wall clock in use
/// just before the change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transition {
    pub month: u32,
    pub day: TransitionDay,
    /// Milliseconds after local midnight.
    pub time_of_day_ms: i64,
}

impl Transition {
    fn wall_ms(&self, year: i64) -> i64 {
        let month = i64::from(self.month.clamp(1, 12));
        let first = days_from_civil(year, month, 1);
        let next_month = if month == 12 {
            days_from_civil(year + 1, 1, 1)
        } else {
            days_from_civil(year, month + 1, 1)
        };
        let day_offset = match self.day {
            TransitionDay::Date(day) => i64::from(day.max(1)) - 1,
            TransitionDay::Weekday {
                weekday,
                occurrence,
            } => {
                let first_weekday = (first + 4).rem_euclid(7);
                let mut offset = (i64::from(weekday % 7) - first_weekday).rem_euclid(7)
                    + (i64::from(occurrence.clamp(1, 5)) - 1) * 7;
                while first + offset >= next_month {
                    offset -= 7;
                }
                offset
            }
        };
        (first + day_offset) * MS_PER_DAY + self.time_of_day_ms
    }
}

/// The daylight-saving half of a zone's rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DaylightRules {
    /// Local daylight time minus UTC, in minutes.
    pub offset_minutes: i32,
    pub starts: Transition,
    pub ends: Transition,
}

/// One year's rules for a time zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZoneRules {
    /// Local standard time minus UTC, in minutes.
    pub standard_offset_minutes: i32,
    pub daylight: Option<DaylightRules>,
}

impl ZoneRules {
    fn offset_minutes_at(&self, utc_ms: i64, year: i64) -> i32 {
        let Some(daylight) = &self.daylight else {
            return self.standard_offset_minutes;
        };
        let starts = daylight.starts.wall_ms(year) - minutes_to_ms(self.standard_offset_minutes);
        let ends = daylight.ends.wall_ms(year) - minutes_to_ms(daylight.offset_minutes);
        // South of the equator daylight time spans the turn of the year.
        let in_daylight = if starts <= ends {
            (starts..ends).contains(&utc_ms)
        } else {
            utc_ms >= starts || utc_ms < ends
        };
        if in_daylight {
            daylight.offset_minutes
        } else {
            self.standard_offset_minutes
        }
    }
}

enum Source {
    Rules(ZoneRules),
    /// The Windows zone. Rules can differ from year to year, so each year's
    /// are read from the system when first needed and the latest is kept.
    System {
        zone: Box<DYNAMIC_TIME_ZONE_INFORMATION>,
        latest: Cell<Option<(i64, ZoneRules)>>,
    },
}

/// Converts between UTC and local wall-clock time.
pub struct TimeZone {
    source: Source,
}

impl TimeZone {
    /// The time zone Windows is set to, including its daylight-saving rules.
    /// The zone is read once; create a new value to notice a changed setting.
    pub fn system() -> Self {
        let mut zone = Box::new(DYNAMIC_TIME_ZONE_INFORMATION::default());
        // SAFETY: the call only writes to the structure passed to it.
        let result = unsafe { GetDynamicTimeZoneInformation(&mut *zone) };
        if result == TIME_ZONE_ID_INVALID {
            return Self::utc();
        }
        Self {
            source: Source::System {
                zone,
                latest: Cell::new(None),
            },
        }
    }

    /// Coordinated Universal Time.
    pub fn utc() -> Self {
        Self::fixed(0)
    }

    /// A zone that is always `offset_minutes` ahead of UTC.
    pub fn fixed(offset_minutes: i32) -> Self {
        Self::with_rules(ZoneRules {
            standard_offset_minutes: offset_minutes,
            daylight: None,
        })
    }

    /// A zone that follows the same rules every year.
    pub fn with_rules(rules: ZoneRules) -> Self {
        Self {
            source: Source::Rules(rules),
        }
    }

    fn rules_for(&self, year: i64) -> ZoneRules {
        match &self.source {
            Source::Rules(rules) => *rules,
            Source::System { zone, latest } => match latest.get() {
                Some((cached, rules)) if cached == year => rules,
                _ => {
                    let rules = system_rules(zone, year);
                    latest.set(Some((year, rules)));
                    rules
                }
            },
        }
    }

    /// Local time minus UTC at the given instant, in milliseconds.
    pub fn offset_ms(&self, utc_ms: i64) -> i64 {
        let utc_year = year_of(utc_ms);
        let mut rules = self.rules_for(utc_year);
        // Rules belong to the local year, which differs from the UTC year only near New Year.
        let local_year =
            year_of(utc_ms.saturating_add(minutes_to_ms(rules.standard_offset_minutes)));
        if local_year != utc_year {
            rules = self.rules_for(local_year);
        }
        minutes_to_ms(rules.offset_minutes_at(utc_ms, local_year))
    }

    /// The wall-clock time in this zone at the given instant.
    pub fn to_local(&self, utc_ms: i64) -> LocalTime {
        LocalTime::from_wall_ms(utc_ms.saturating_add(self.offset_ms(utc_ms)))
    }

    /// The UTC instant of a wall-clock time. A time that occurs twice when
    /// clocks go back is read as standard time, and a time skipped when they
    /// go forward keeps the standard offset, as .NET's `ToUniversalTime` does.
    pub fn to_utc(&self, local: LocalTime) -> i64 {
        self.resolve(local).0
    }

    /// True for a wall-clock time skipped when clocks go forward.
    pub fn is_invalid(&self, local: LocalTime) -> bool {
        !self.resolve(local).1
    }

    fn resolve(&self, local: LocalTime) -> (i64, bool) {
        let rules = self.rules_for(year_of(local.wall_ms));
        let standard_offset = minutes_to_ms(rules.standard_offset_minutes);
        let standard = local.wall_ms.saturating_sub(standard_offset);
        let Some(daylight) = rules.daylight else {
            return (standard, true);
        };
        let standard_exists = self.offset_ms(standard) == standard_offset;
        let daylight_offset = minutes_to_ms(daylight.offset_minutes);
        let shifted = local.wall_ms.saturating_sub(daylight_offset);
        if !standard_exists && self.offset_ms(shifted) == daylight_offset {
            (shifted, true)
        } else {
            (standard, standard_exists)
        }
    }
}

fn year_of(ms: i64) -> i64 {
    civil_from_days(ms.div_euclid(MS_PER_DAY)).0
}

fn system_rules(zone: &DYNAMIC_TIME_ZONE_INFORMATION, year: i64) -> ZoneRules {
    let mut info = TIME_ZONE_INFORMATION::default();
    let found = u16::try_from(year).is_ok_and(|year| {
        // SAFETY: both pointers refer to live structures and the call only writes to `info`.
        unsafe { GetTimeZoneInformationForYear(year, zone, &mut info) != 0 }
    });
    if found {
        rules_from_windows(
            info.Bias,
            info.StandardBias,
            &info.StandardDate,
            info.DaylightBias,
            &info.DaylightDate,
        )
    } else {
        rules_from_windows(
            zone.Bias,
            zone.StandardBias,
            &zone.StandardDate,
            zone.DaylightBias,
            &zone.DaylightDate,
        )
    }
}

/// Windows states each bias as the minutes to add to local time to reach UTC.
fn rules_from_windows(
    bias: i32,
    standard_bias: i32,
    standard_date: &SYSTEMTIME,
    daylight_bias: i32,
    daylight_date: &SYSTEMTIME,
) -> ZoneRules {
    match (
        windows_transition(daylight_date),
        windows_transition(standard_date),
    ) {
        (Some(starts), Some(ends)) => ZoneRules {
            standard_offset_minutes: bias.saturating_add(standard_bias).saturating_neg(),
            daylight: Some(DaylightRules {
                offset_minutes: bias.saturating_add(daylight_bias).saturating_neg(),
                starts,
                ends,
            }),
        },
        // Without transition dates Windows ignores the standard bias too.
        _ => ZoneRules {
            standard_offset_minutes: bias.saturating_neg(),
            daylight: None,
        },
    }
}

fn windows_transition(date: &SYSTEMTIME) -> Option<Transition> {
    if date.wMonth == 0 {
        return None;
    }
    // A zero year marks a rule that recurs on the nth weekday of the month.
    let day = if date.wYear == 0 {
        TransitionDay::Weekday {
            weekday: u32::from(date.wDayOfWeek),
            occurrence: u32::from(date.wDay),
        }
    } else {
        TransitionDay::Date(u32::from(date.wDay))
    };
    Some(Transition {
        month: u32::from(date.wMonth),
        day,
        time_of_day_ms: i64::from(date.wHour) * MS_PER_HOUR
            + i64::from(date.wMinute) * MS_PER_MINUTE
            + i64::from(date.wSecond) * MS_PER_SECOND
            + i64::from(date.wMilliseconds),
    })
}

/// Formats a date in the user's locale from a Windows date picture such as `ddd, MMM d`.
pub fn format_date(date: LocalDate, picture: &str) -> String {
    let fallback = || format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day());
    let Ok(year) = u16::try_from(date.year()) else {
        return fallback();
    };
    let time = SYSTEMTIME {
        wYear: year,
        wMonth: date.month() as u16,
        wDayOfWeek: date.weekday() as u16,
        wDay: date.day() as u16,
        ..Default::default()
    };
    let picture = wide(picture);
    let mut buffer = [0u16; 80];
    // SAFETY: the buffer length passed matches the buffer; the picture is null-terminated.
    let written = unsafe {
        GetDateFormatEx(
            null(),
            0,
            &time,
            picture.as_ptr(),
            buffer.as_mut_ptr(),
            buffer.len() as i32,
            null(),
        )
    };
    if written > 0 {
        from_wide(&buffer)
    } else {
        fallback()
    }
}

/// The decimal separator of the user's locale.
pub fn decimal_separator() -> String {
    let mut buffer = [0u16; 8];
    // SAFETY: the buffer length passed matches the buffer.
    let written = unsafe {
        GetLocaleInfoEx(
            null(),
            LOCALE_SDECIMAL,
            buffer.as_mut_ptr(),
            buffer.len() as i32,
        )
    };
    if written > 0 {
        from_wide(&buffer)
    } else {
        ".".to_string()
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// United States Eastern time: UTC-5, daylight time from the second Sunday
    /// of March to the first Sunday of November.
    pub fn eastern() -> TimeZone {
        let sunday_at_two = |month, occurrence| Transition {
            month,
            day: TransitionDay::Weekday {
                weekday: 0,
                occurrence,
            },
            time_of_day_ms: 2 * MS_PER_HOUR,
        };
        TimeZone::with_rules(ZoneRules {
            standard_offset_minutes: -300,
            daylight: Some(DaylightRules {
                offset_minutes: -240,
                starts: sunday_at_two(3, 2),
                ends: sunday_at_two(11, 1),
            }),
        })
    }

    /// New South Wales: UTC+10, daylight time from the first Sunday of October
    /// to the first Sunday of April.
    fn sydney() -> TimeZone {
        let first_sunday = |month, hour| Transition {
            month,
            day: TransitionDay::Weekday {
                weekday: 0,
                occurrence: 1,
            },
            time_of_day_ms: hour * MS_PER_HOUR,
        };
        TimeZone::with_rules(ZoneRules {
            standard_offset_minutes: 600,
            daylight: Some(DaylightRules {
                offset_minutes: 660,
                starts: first_sunday(10, 2),
                ends: first_sunday(4, 3),
            }),
        })
    }

    pub fn utc_ms(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        LocalTime::new(LocalDate::from_ymd(year, month, day), hour, minute, 0).wall_ms()
    }

    #[test]
    fn dates_convert_to_and_from_day_counts() {
        let date = LocalDate::from_ymd(2026, 10, 9);
        assert_eq!((date.year(), date.month(), date.day()), (2026, 10, 9));
        assert_eq!(date.weekday(), 5);
        assert!(!date.is_weekend());
        assert!(date.plus_days(1).is_weekend() && date.plus_days(2).is_weekend());
        assert_eq!(date.plus_days(23), LocalDate::from_ymd(2026, 11, 1));
        assert_eq!(
            LocalDate::from_ymd(2024, 3, 1).days_since(LocalDate::from_ymd(2024, 2, 28)),
            2
        );
        let leap_day = LocalDate::from_ymd(2024, 2, 29);
        assert_eq!((leap_day.month(), leap_day.day()), (2, 29));
        assert_eq!(utc_ms(1970, 1, 1, 0, 0), 0);
        assert_eq!(utc_ms(2026, 10, 9, 12, 0), 1_791_547_200_000);
        let before_epoch = LocalTime::from_wall_ms(-1);
        assert_eq!(before_epoch.date(), LocalDate::from_ymd(1969, 12, 31));
        assert_eq!(
            (
                before_epoch.hour(),
                before_epoch.minute(),
                before_epoch.second()
            ),
            (23, 59, 59)
        );
    }

    #[test]
    fn readings_are_held_to_the_range_the_history_file_supports() {
        assert_eq!(seconds_to_ms(1_791_547_200), Some(1_791_547_200_000));
        assert_eq!(seconds_to_ms(i64::MAX), None);
        assert_eq!(seconds_to_ms(MAX_TIMESTAMP_MS / 1_000 + 1), None);
        assert_eq!(now_to_ms(i64::MAX), MAX_TIMESTAMP_MS);
        assert_eq!(now_to_ms(i64::MIN), MIN_TIMESTAMP_MS);
        assert_eq!(add_hours(0, 1.0 / 3.0), 1_200_000);
        assert_eq!(add_hours(10, 0.999_999_999_9), 3_600_010);
        assert_eq!(add_hours(0, -0.5), -1_800_000);
        assert_eq!(add_hours(0, f64::NAN), 0);
        assert_eq!(add_hours(MAX_TIMESTAMP_MS, f64::INFINITY), i64::MAX);
        assert_eq!(floor_hour(3_599_999), 0);
        assert_eq!(floor_hour(-1), -MS_PER_HOUR);
    }

    #[test]
    fn durations_are_measured_like_dotnet_timespans() {
        // Values printed by the .NET Framework for the same tick counts.
        assert_eq!(hours_between(5_400_000, 0), 1.5);
        assert_eq!(hours_between(0, 7_200_000), -2.0);
        assert_eq!(hours_between(21_600_000, 0), 6.0);
        assert_eq!(hours_between(900_000, 0), 0.25);
        let fifty_four_minutes = hours_between(3_240_000, 0);
        assert!(fifty_four_minutes < 0.9 && 0.9 - fifty_four_minutes < 1e-15);
        assert_eq!(minutes_between(120_000, 0), 2.0);
        assert_eq!(minutes_between(1_800_000, 0), 30.0);
        assert_eq!(seconds_between(2_000, 0), 2.0);
        assert_eq!(seconds_between(86_400_000, 0), 86_400.0);
    }

    #[test]
    fn daylight_saving_follows_the_zone_rules() {
        let zone = eastern();
        // 2026: daylight time runs from March 8 to November 1.
        let winter = zone.to_local(utc_ms(2026, 1, 15, 17, 0));
        assert_eq!(
            (winter.date(), winter.hour()),
            (LocalDate::from_ymd(2026, 1, 15), 12)
        );
        let summer = zone.to_local(utc_ms(2026, 7, 4, 3, 30));
        assert_eq!(
            (summer.date(), summer.hour(), summer.minute()),
            (LocalDate::from_ymd(2026, 7, 3), 23, 30)
        );
        assert_eq!(zone.to_local(utc_ms(2026, 3, 8, 6, 59)).hour(), 1);
        assert_eq!(zone.to_local(utc_ms(2026, 3, 8, 7, 0)).hour(), 3);
        assert_eq!(zone.to_local(utc_ms(2026, 11, 1, 5, 59)).hour(), 1);
        assert_eq!(zone.to_local(utc_ms(2026, 11, 1, 6, 0)).hour(), 1);
        assert_eq!(zone.to_local(utc_ms(2026, 11, 1, 7, 0)).hour(), 2);
        // The rule moves with the calendar: in 2027 the change is on March 14.
        assert_eq!(zone.offset_ms(utc_ms(2027, 3, 10, 12, 0)), -5 * MS_PER_HOUR);
        assert_eq!(zone.offset_ms(utc_ms(2027, 3, 14, 12, 0)), -4 * MS_PER_HOUR);
    }

    #[test]
    fn wall_clock_times_resolve_like_dotnet() {
        let zone = eastern();
        let local = |month, day, hour, minute| {
            LocalTime::new(LocalDate::from_ymd(2026, month, day), hour, minute, 0)
        };
        assert_eq!(zone.to_utc(local(1, 15, 12, 0)), utc_ms(2026, 1, 15, 17, 0));
        assert_eq!(zone.to_utc(local(7, 3, 23, 30)), utc_ms(2026, 7, 4, 3, 30));
        // 2:30 on March 8 never happens; it keeps the standard offset.
        assert!(zone.is_invalid(local(3, 8, 2, 30)));
        assert_eq!(zone.to_utc(local(3, 8, 2, 30)), utc_ms(2026, 3, 8, 7, 30));
        assert!(!zone.is_invalid(local(3, 8, 3, 0)));
        assert_eq!(zone.to_utc(local(3, 8, 3, 0)), utc_ms(2026, 3, 8, 7, 0));
        // 1:30 on November 1 happens twice; it is read as standard time.
        assert!(!zone.is_invalid(local(11, 1, 1, 30)));
        assert_eq!(zone.to_utc(local(11, 1, 1, 30)), utc_ms(2026, 11, 1, 6, 30));
        assert_eq!(zone.to_utc(local(11, 1, 0, 30)), utc_ms(2026, 11, 1, 4, 30));

        let fixed = TimeZone::fixed(330);
        let half_past = fixed.to_local(utc_ms(2026, 10, 9, 20, 0));
        assert_eq!(
            (half_past.date(), half_past.hour(), half_past.minute()),
            (LocalDate::from_ymd(2026, 10, 10), 1, 30)
        );
        assert_eq!(fixed.to_utc(half_past), utc_ms(2026, 10, 9, 20, 0));
        assert!(!fixed.is_invalid(half_past));
    }

    #[test]
    fn southern_daylight_time_spans_the_new_year() {
        let zone = sydney();
        assert_eq!(zone.offset_ms(utc_ms(2026, 1, 15, 0, 0)), 11 * MS_PER_HOUR);
        assert_eq!(zone.offset_ms(utc_ms(2026, 7, 15, 0, 0)), 10 * MS_PER_HOUR);
        assert_eq!(
            zone.offset_ms(utc_ms(2026, 12, 31, 23, 0)),
            11 * MS_PER_HOUR
        );
        let new_year = zone.to_local(utc_ms(2026, 12, 31, 13, 0));
        assert_eq!(new_year.date(), LocalDate::from_ymd(2027, 1, 1));
        assert_eq!(zone.to_utc(new_year), utc_ms(2026, 12, 31, 13, 0));
    }

    #[test]
    fn the_system_zone_round_trips_wall_clock_times() {
        let zone = TimeZone::system();
        for instant in [
            utc_ms(2026, 1, 15, 12, 0),
            utc_ms(2026, 7, 15, 12, 0),
            utc_ms(2026, 12, 31, 23, 30),
        ] {
            assert_eq!(zone.to_utc(zone.to_local(instant)), instant);
            assert!(zone.offset_ms(instant).abs() <= 14 * MS_PER_HOUR);
        }
    }

    #[test]
    fn windows_rules_translate_bias_and_transition_dates() {
        let recurring = |month, day, hour| SYSTEMTIME {
            wMonth: month,
            wDay: day,
            wHour: hour,
            ..Default::default()
        };
        let rules = rules_from_windows(300, 0, &recurring(11, 1, 2), -60, &recurring(3, 2, 2));
        assert_eq!(TimeZone::with_rules(rules).offset_ms(0), -5 * MS_PER_HOUR);
        assert_eq!(
            TimeZone::with_rules(rules).offset_ms(utc_ms(2026, 7, 1, 0, 0)),
            -4 * MS_PER_HOUR
        );

        let none = SYSTEMTIME::default();
        let fixed = rules_from_windows(-330, 15, &none, -60, &none);
        assert_eq!(fixed.standard_offset_minutes, 330);
        assert_eq!(fixed.daylight, None);

        // A year in the date pins the change to that day of the month.
        let dated = SYSTEMTIME {
            wYear: 2026,
            ..recurring(10, 25, 3)
        };
        assert_eq!(
            windows_transition(&dated).map(|transition| transition.day),
            Some(TransitionDay::Date(25))
        );
        let last_sunday = Transition {
            month: 10,
            day: TransitionDay::Weekday {
                weekday: 0,
                occurrence: 5,
            },
            time_of_day_ms: 0,
        };
        assert_eq!(last_sunday.wall_ms(2026), utc_ms(2026, 10, 25, 0, 0));
    }

    #[test]
    fn dates_format_in_the_user_locale() {
        let text = format_date(LocalDate::from_ymd(2026, 10, 9), "ddd, MMM d");
        assert!(text.contains('9'), "{text}");
        assert!(!decimal_separator().is_empty());
    }
}
