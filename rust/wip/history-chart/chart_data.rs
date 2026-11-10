//! What a usage history chart shows, prepared by the tray process and sent to
//! the chart process as JSON.
//!
//! The tray owns the history stores, so everything that depends on them is
//! decided here: which readings are plotted, how each joins the one before
//! it, the time range, the projected path and the forecast text. The chart
//! process only draws what it is given and lets the user inspect it.
//!
//! The payload is bounded. A window carries at most [`MAX_POINTS`] readings,
//! which is all a history can hold, so a whole cycle always fits however
//! often its percentage changed; its projected path and its quiet periods
//! carry at most [`MAX_PATH_POINTS`] entries each. With both windows full
//! that stays below `ipc::MAX_HISTORY_DATA_BYTES`, which the chart enforces.

use serde_json::{json, Value};

use crate::forecast::{Forecast, ForecastSettings, Projection, ProjectionPoint};
use crate::history::{reset_after_seconds, HistoryStore};
use crate::history_file::MAX_SAMPLES;
use crate::history_view::ChartSeries;
use crate::ipc::{HISTORY_PROVIDER_CLAUDE, HISTORY_PROVIDER_CODEX};
use crate::localtime::{MAX_TIMESTAMP_MS, MIN_TIMESTAMP_MS};
use crate::model::{LimitWindow, Usage};
use crate::settings::{Settings, ThemeChoice};
use crate::timefmt::format_date_time;

/// The most readings one window of a payload carries: a history filled to
/// its cap with readings that all belong to the current cycle and all differ.
pub const MAX_POINTS: usize = MAX_SAMPLES;
/// The most points of a projected path, and the most quiet periods, one
/// window carries. An hourly path to a weekly reset has about 170 points.
pub const MAX_PATH_POINTS: usize = 1_024;

const CHANGED_BIT: u64 = 1;
const LINK_SHIFT: u32 = 1;

/// Whose usage a chart shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChartProvider {
    Codex,
    Claude,
}

impl ChartProvider {
    /// The provider's name as it appears in the window title.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude",
        }
    }

    /// How the provider is spelled on the chart process's command line.
    pub const fn argument(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }

    /// The provider a command-line argument names.
    pub fn from_argument(argument: &str) -> Option<Self> {
        [Self::Codex, Self::Claude]
            .into_iter()
            .find(|provider| argument.eq_ignore_ascii_case(provider.argument()))
    }

    /// The provider's code in a `WM_HISTORY_EVENT`.
    pub const fn code(self) -> usize {
        match self {
            Self::Codex => HISTORY_PROVIDER_CODEX,
            Self::Claude => HISTORY_PROVIDER_CLAUDE,
        }
    }

    /// The provider a `WM_HISTORY_EVENT` code stands for.
    pub const fn from_code(code: usize) -> Option<Self> {
        match code {
            HISTORY_PROVIDER_CODEX => Some(Self::Codex),
            HISTORY_PROVIDER_CLAUDE => Some(Self::Claude),
            _ => None,
        }
    }
}

/// How a plotted reading joins the reading before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    /// Monitoring continued between the two: a solid line. Also used for
    /// the first reading, which has nothing to join.
    Continuous,
    /// The two are in one cycle but more than two hours apart: a dashed
    /// line, which connects them without implying readings in between.
    Gap,
    /// The reading starts a new cycle or follows an adjustment: no line, and
    /// a marker at the reading.
    Reset,
}

impl Link {
    const fn code(self) -> u64 {
        match self {
            Self::Continuous => 0,
            Self::Gap => 1,
            Self::Reset => 2,
        }
    }

    const fn from_code(code: u64) -> Option<Self> {
        match code {
            0 => Some(Self::Continuous),
            1 => Some(Self::Gap),
            2 => Some(Self::Reset),
            _ => None,
        }
    }
}

/// One plotted reading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChartPoint {
    /// Unix milliseconds.
    pub timestamp_ms: i64,
    pub used_percent: f64,
    /// True when the percentage differs from the reading before, for the
    /// first reading and for one that starts a new cycle. Only these can be
    /// selected with the pointer or the arrow keys.
    pub changed: bool,
    pub link: Link,
}

/// One limit window of a chart: its header values, its plotted readings and
/// its projected path.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartWindow {
    /// The whole percentage remaining in the reading on display, or `None`
    /// when there is no reading of this window.
    pub remaining_percent: Option<i32>,
    /// When the window resets, formatted for display, when that is known.
    pub reset: Option<String>,
    /// The estimate shown in the header and the footer.
    pub forecast: String,
    /// The current cycle's readings, oldest first, with runs of unchanged
    /// readings reduced to their first and last.
    pub points: Vec<ChartPoint>,
    /// The left edge of the time axis, in Unix milliseconds: the first
    /// plotted reading, or the time the data was built when there is none.
    pub start_ms: i64,
    /// The right edge of the time axis: the time the data was built,
    /// extended to the window's reset and to the end of the projection.
    /// Always later than `start_ms`.
    pub end_ms: i64,
    /// The projected path. Present only when projections are switched on and
    /// an estimate continues the latest plotted reading.
    pub projection: Option<Projection>,
    /// The stretches of the projection that a learned quiet period keeps
    /// level, as start and end times, with adjoining hours joined.
    pub quiet_periods: Vec<(i64, i64)>,
}

/// Everything one provider's history chart shows.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartData {
    /// The color scheme the user chose; the chart resolves "system" itself
    /// so it can follow Windows while it is open.
    pub theme: ThemeChoice,
    /// Whether the quiet periods of a projection are highlighted.
    pub highlight_quiet_hours: bool,
    pub weekly: ChartWindow,
    /// Present only while the reading on display has a five-hour limit.
    pub five_hour: Option<ChartWindow>,
}

impl ChartData {
    /// Prepares a chart from one provider's history and the reading on
    /// display. `fresh` is false when the latest refresh failed or
    /// refreshing is paused, which hides estimates as in the popup. The
    /// forecast method, quiet-hours mode and "show projected usage" settings
    /// are applied here; the theme and the quiet-hours highlight are passed on.
    pub fn build(
        store: &HistoryStore,
        usage: Option<&Usage>,
        fresh: bool,
        settings: &Settings,
        now: i64,
    ) -> Self {
        let window = |weekly: bool| ChartWindow::build(store, usage, weekly, fresh, settings, now);
        Self {
            theme: settings.theme,
            highlight_quiet_hours: settings.highlight_quiet_hours,
            weekly: window(true),
            five_hour: usage
                .is_some_and(|usage| usage.five_hour.is_some())
                .then(|| window(false)),
        }
    }

    /// What a chart shows until it has been given data: no readings and
    /// nothing to estimate from, in the system's color scheme.
    pub fn empty(now: i64) -> Self {
        Self {
            theme: ThemeChoice::System,
            highlight_quiet_hours: false,
            weekly: ChartWindow {
                forecast: Forecast::NeedsRefresh.text(now),
                ..ChartWindow::from_series(&ChartSeries::new(&[], true, now), None)
            },
            five_hour: None,
        }
    }

    /// The weekly window when `weekly` is true, otherwise the five-hour one.
    pub fn window(&self, weekly: bool) -> Option<&ChartWindow> {
        if weekly {
            Some(&self.weekly)
        } else {
            self.five_hour.as_ref()
        }
    }

    /// The payload sent to the chart process.
    pub fn to_json(&self) -> String {
        json!({
            "theme": self.theme.name(),
            "highlightQuietHours": self.highlight_quiet_hours,
            "weekly": self.weekly.to_value(),
            "fiveHour": self.five_hour.as_ref().map(ChartWindow::to_value),
        })
        .to_string()
    }

    /// Reads a payload. `None` when it is malformed, exceeds the bounds this
    /// module sends within, or describes an empty or reversed time range.
    pub fn from_json(bytes: &[u8]) -> Option<Self> {
        let root: Value = serde_json::from_slice(bytes).ok()?;
        let theme = root.get("theme")?.as_str()?;
        Some(Self {
            theme: [ThemeChoice::System, ThemeChoice::Dark, ThemeChoice::Light]
                .into_iter()
                .find(|choice| choice.name() == theme)?,
            highlight_quiet_hours: root.get("highlightQuietHours")?.as_bool()?,
            weekly: ChartWindow::from_value(root.get("weekly")?)?,
            five_hour: match root.get("fiveHour")? {
                Value::Null => None,
                window => Some(ChartWindow::from_value(window)?),
            },
        })
    }
}

impl ChartWindow {
    fn build(
        store: &HistoryStore,
        usage: Option<&Usage>,
        weekly: bool,
        fresh: bool,
        settings: &Settings,
        now: i64,
    ) -> Self {
        let forecast_settings = ForecastSettings {
            method: settings.forecast_method,
            quiet_hours: settings.quiet_hours_mode,
        };
        let projection = usage
            .filter(|_| settings.show_projected_usage)
            .and_then(|usage| store.projection(weekly, usage, fresh, forecast_settings, now));
        // Without a reading there is nothing to estimate from, as when the
        // reading is too old.
        let forecast = usage.map_or(Forecast::NeedsRefresh, |usage| {
            store.forecast(weekly, usage, fresh, forecast_settings, now)
        });
        let reading = usage.and_then(|usage| {
            let limit: Option<&LimitWindow> = if weekly {
                usage.weekly.as_ref()
            } else {
                usage.five_hour.as_ref()
            };
            limit.map(|limit| (usage.updated_at, limit))
        });
        let mut window = Self::from_series(&store.chart(weekly, now), projection);
        window.remaining_percent = reading.map(|(_, limit)| limit.remaining_percent());
        window.reset = reading.and_then(|(updated_at, limit)| {
            let seconds = reset_after_seconds(limit, updated_at)?;
            Some(format_date_time(updated_at.saturating_add(seconds), now))
        });
        window.forecast = forecast.text(now);
        window
    }

    /// The plotted part of a window: the readings of `series`, its time
    /// range, and `projection` if it continues the latest reading.
    fn from_series(series: &ChartSeries, projection: Option<Projection>) -> Self {
        let projection = projection.filter(|projection| series.accepts(projection));
        let points = series
            .samples
            .iter()
            .enumerate()
            .map(|(index, sample)| {
                let previous = index.checked_sub(1).map(|before| &series.samples[before]);
                ChartPoint {
                    timestamp_ms: sample.timestamp_ms,
                    used_percent: sample.window.used_percent,
                    changed: series.is_changed_reading(index),
                    link: match previous {
                        Some(previous) if sample.is_reset_from(previous) => Link::Reset,
                        Some(previous) if !sample.continues(previous) => Link::Gap,
                        _ => Link::Continuous,
                    },
                }
            })
            .collect();
        let quiet_periods = projection
            .as_ref()
            .map(|projection| joined(projection.quiet_periods()))
            .unwrap_or_default();
        Self {
            remaining_percent: None,
            reset: None,
            forecast: String::new(),
            points,
            start_ms: series.start_ms,
            end_ms: series.end_ms(projection.as_ref()),
            projection: projection.map(within_bounds),
            quiet_periods,
        }
    }

    fn to_value(&self) -> Value {
        let points: Vec<Value> = self
            .points
            .iter()
            .map(|point| {
                let code = u64::from(point.changed) | (point.link.code() << LINK_SHIFT);
                json!([point.timestamp_ms, point.used_percent, code])
            })
            .collect();
        let projection = self.projection.as_ref().map(|projection| {
            let path: Vec<Value> = projection
                .points
                .iter()
                .map(|point| json!([point.timestamp_ms, point.remaining_percent]))
                .collect();
            json!({
                "start": projection.start_ms,
                "end": projection.end_ms,
                "startRemaining": projection.start_remaining_percent,
                "endRemaining": projection.end_remaining_percent,
                "endsAtReset": projection.ends_at_reset,
                "usesDailyPattern": projection.uses_daily_pattern,
                "usesValidatedModel": projection.uses_validated_model,
                "usesSessionPace": projection.uses_session_pace,
                "usesInactiveHours": projection.uses_inactive_hours,
                "points": path,
            })
        });
        let quiet: Vec<Value> = self
            .quiet_periods
            .iter()
            .map(|&(start, end)| json!([start, end]))
            .collect();
        json!({
            "remaining": self.remaining_percent,
            "reset": self.reset,
            "forecast": self.forecast,
            "start": self.start_ms,
            "end": self.end_ms,
            "points": points,
            "projection": projection,
            "quiet": quiet,
        })
    }

    fn from_value(value: &Value) -> Option<Self> {
        let points = value.get("points")?.as_array()?;
        let quiet = value.get("quiet")?.as_array()?;
        if points.len() > MAX_POINTS || quiet.len() > MAX_PATH_POINTS {
            return None;
        }
        let start_ms = timestamp(value.get("start")?)?;
        let end_ms = timestamp(value.get("end")?)?;
        if end_ms <= start_ms {
            return None;
        }
        Some(Self {
            remaining_percent: match value.get("remaining")? {
                Value::Null => None,
                remaining => Some(i32::try_from(remaining.as_i64()?).ok()?),
            },
            reset: match value.get("reset")? {
                Value::Null => None,
                reset => Some(reset.as_str()?.to_string()),
            },
            forecast: value.get("forecast")?.as_str()?.to_string(),
            points: points
                .iter()
                .map(|point| {
                    let [timestamp_ms, used_percent, code] = point.as_array()?.as_slice() else {
                        return None;
                    };
                    let code = code.as_u64()?;
                    Some(ChartPoint {
                        timestamp_ms: timestamp(timestamp_ms)?,
                        used_percent: used_percent.as_f64()?,
                        changed: code & CHANGED_BIT != 0,
                        link: Link::from_code(code >> LINK_SHIFT)?,
                    })
                })
                .collect::<Option<_>>()?,
            start_ms,
            end_ms,
            projection: match value.get("projection")? {
                Value::Null => None,
                projection => Some(projection_from(projection)?),
            },
            quiet_periods: quiet
                .iter()
                .map(|period| {
                    let [start, end] = period.as_array()?.as_slice() else {
                        return None;
                    };
                    Some((timestamp(start)?, timestamp(end)?))
                })
                .collect::<Option<_>>()?,
        })
    }
}

/// A time the history could hold, in Unix milliseconds.
fn timestamp(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .filter(|ms| (MIN_TIMESTAMP_MS..=MAX_TIMESTAMP_MS).contains(ms))
}

fn projection_from(value: &Value) -> Option<Projection> {
    let flag = |name: &str| value.get(name)?.as_bool();
    let points = value.get("points")?.as_array()?;
    if points.len() > MAX_PATH_POINTS {
        return None;
    }
    Some(Projection {
        start_ms: timestamp(value.get("start")?)?,
        end_ms: timestamp(value.get("end")?)?,
        start_remaining_percent: value.get("startRemaining")?.as_f64()?,
        end_remaining_percent: value.get("endRemaining")?.as_f64()?,
        ends_at_reset: flag("endsAtReset")?,
        uses_daily_pattern: flag("usesDailyPattern")?,
        uses_validated_model: flag("usesValidatedModel")?,
        uses_session_pace: flag("usesSessionPace")?,
        uses_inactive_hours: flag("usesInactiveHours")?,
        points: points
            .iter()
            .map(|point| {
                let [timestamp_ms, remaining_percent] = point.as_array()?.as_slice() else {
                    return None;
                };
                Some(ProjectionPoint {
                    timestamp_ms: timestamp(timestamp_ms)?,
                    remaining_percent: remaining_percent.as_f64()?,
                })
            })
            .collect::<Option<_>>()?,
    })
}

/// Joins quiet periods that follow one another without a break, which an
/// hourly path reports hour by hour, and keeps the payload's bound.
fn joined(periods: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    let mut joined: Vec<(i64, i64)> = Vec::new();
    for (start, end) in periods {
        match joined.last_mut() {
            Some(last) if last.1 == start => last.1 = end,
            _ => joined.push((start, end)),
        }
    }
    joined.truncate(MAX_PATH_POINTS);
    joined
}

/// Thins a path that has more points than a payload carries, keeping its
/// ends. Only a reset reported absurdly far ahead produces such a path.
fn within_bounds(mut projection: Projection) -> Projection {
    let count = projection.points.len();
    if count > MAX_PATH_POINTS {
        projection.points = (0..MAX_PATH_POINTS)
            .map(|index| projection.points[index * (count - 1) / (MAX_PATH_POINTS - 1)])
            .collect();
    }
    projection
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::history::tests::{open, snapshot, TempDir, DAY, HOUR, MINUTE, NOW};
    use crate::history::{HistorySample, HistoryWindow};
    use crate::ipc::MAX_HISTORY_DATA_BYTES;
    use crate::localtime::{MS_PER_HOUR, MS_PER_MINUTE};
    use crate::model::{FIVE_HOUR_MINUTES, WEEKLY_MINUTES};

    /// A point that differs from the one before it and continues it.
    pub fn point(timestamp_ms: i64, used_percent: f64) -> ChartPoint {
        ChartPoint {
            timestamp_ms,
            used_percent,
            changed: true,
            link: Link::Continuous,
        }
    }

    /// A window with the given readings and time range, and no projection.
    pub fn window(points: Vec<ChartPoint>, start_ms: i64, end_ms: i64) -> ChartWindow {
        ChartWindow {
            remaining_percent: points
                .last()
                .map(|point| (100.0 - point.used_percent).round() as i32),
            reset: Some("Nov 5, 3:38 PM".to_string()),
            forecast: "Will last until reset".to_string(),
            points,
            start_ms,
            end_ms,
            projection: None,
            quiet_periods: Vec::new(),
        }
    }

    /// A straight projected path between two points.
    pub fn straight(start: (i64, f64), end: (i64, f64)) -> Projection {
        Projection {
            start_ms: start.0,
            end_ms: end.0,
            start_remaining_percent: start.1,
            end_remaining_percent: end.1,
            ends_at_reset: false,
            uses_daily_pattern: false,
            uses_validated_model: false,
            uses_session_pace: false,
            uses_inactive_hours: false,
            points: Vec::new(),
        }
    }

    /// A chart of one weekly window.
    pub fn weekly_only(weekly: ChartWindow) -> ChartData {
        ChartData {
            theme: ThemeChoice::Dark,
            highlight_quiet_hours: false,
            weekly,
            five_hour: None,
        }
    }

    fn weekly_sample(timestamp: i64, used: f64, reset: Option<i64>) -> HistorySample {
        HistorySample {
            timestamp_ms: timestamp * 1_000,
            weekly: Some(HistoryWindow {
                used_percent: used,
                reset_at_ms: reset.map(|reset| reset * 1_000),
                window_minutes: Some(WEEKLY_MINUTES),
                cycle_average_unavailable: false,
                is_imported: false,
            }),
            five_hour: None,
        }
    }

    fn full_window(count: usize, path: usize) -> ChartWindow {
        // The longest text each number can take in JSON.
        let longest_time = MIN_TIMESTAMP_MS;
        let longest_percent = -1.234_567_890_123_456_7e-100;
        let path_point = ProjectionPoint {
            timestamp_ms: longest_time,
            remaining_percent: longest_percent,
        };
        ChartWindow {
            remaining_percent: Some(i32::MIN),
            reset: Some("\u{1F600}".repeat(128)),
            forecast: "\u{1F600}".repeat(256),
            points: vec![
                ChartPoint {
                    timestamp_ms: longest_time,
                    used_percent: longest_percent,
                    changed: true,
                    link: Link::Reset,
                };
                count
            ],
            start_ms: longest_time,
            end_ms: longest_time + 1,
            projection: Some(Projection {
                points: vec![path_point; path],
                ..straight((longest_time, longest_percent), (longest_time, longest_percent))
            }),
            quiet_periods: vec![(longest_time, longest_time); path],
        }
    }

    #[test]
    fn providers_have_a_name_an_argument_and_a_code() {
        for provider in [ChartProvider::Codex, ChartProvider::Claude] {
            assert_eq!(
                ChartProvider::from_argument(provider.argument()),
                Some(provider)
            );
            assert_eq!(ChartProvider::from_code(provider.code()), Some(provider));
        }
        assert_eq!(ChartProvider::Codex.name(), "Codex");
        assert_eq!(ChartProvider::Claude.name(), "Claude");
        assert_eq!(
            ChartProvider::from_argument("Claude"),
            Some(ChartProvider::Claude)
        );
        assert_eq!(ChartProvider::from_argument("gemini"), None);
        assert_eq!(ChartProvider::from_argument(""), None);
        assert_eq!(ChartProvider::from_code(2), None);
    }

    #[test]
    fn payloads_survive_the_trip_between_processes() {
        let start = NOW * 1_000 - 5 * MS_PER_HOUR;
        let now = NOW * 1_000;
        let mut weekly = window(
            vec![
                point(start, 10.0),
                ChartPoint {
                    link: Link::Gap,
                    ..point(start + 3 * MS_PER_HOUR, 0.1 + 0.2)
                },
                ChartPoint {
                    changed: false,
                    ..point(now - MS_PER_HOUR, 0.1 + 0.2)
                },
                ChartPoint {
                    link: Link::Reset,
                    ..point(now, 2.5)
                },
            ],
            start,
            now + 4 * MS_PER_HOUR,
        );
        weekly.projection = Some(Projection {
            ends_at_reset: true,
            uses_daily_pattern: true,
            uses_inactive_hours: true,
            points: vec![
                ProjectionPoint {
                    timestamp_ms: now,
                    remaining_percent: 97.5,
                },
                ProjectionPoint {
                    timestamp_ms: now + MS_PER_HOUR,
                    remaining_percent: 97.5,
                },
                ProjectionPoint {
                    timestamp_ms: now + 4 * MS_PER_HOUR,
                    remaining_percent: 1.0 / 3.0,
                },
            ],
            ..straight((now, 97.5), (now + 4 * MS_PER_HOUR, 1.0 / 3.0))
        });
        weekly.quiet_periods = vec![(now, now + MS_PER_HOUR)];
        let data = ChartData {
            theme: ThemeChoice::System,
            highlight_quiet_hours: true,
            weekly,
            five_hour: Some(ChartWindow {
                remaining_percent: None,
                reset: None,
                forecast: "Refresh usage for an estimate".to_string(),
                ..window(Vec::new(), now, now + MS_PER_MINUTE)
            }),
        };
        assert_eq!(
            ChartData::from_json(data.to_json().as_bytes()),
            Some(data.clone())
        );
        assert!(std::ptr::eq(data.window(true).unwrap(), &data.weekly));
        assert_eq!(data.window(false), data.five_hour.as_ref());

        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let bare = ChartData {
                theme,
                five_hour: None,
                ..data.clone()
            };
            assert_eq!(ChartData::from_json(bare.to_json().as_bytes()), Some(bare));
        }
    }

    #[test]
    fn malformed_payloads_are_rejected() {
        let start = NOW * 1_000;
        let mut weekly = window(vec![point(start, 12.5)], start, start + MS_PER_HOUR);
        weekly.projection = Some(straight((start, 87.5), (start + MS_PER_HOUR, 80.0)));
        weekly.quiet_periods = vec![(start, start + 1)];
        let json = weekly_only(weekly).to_json();
        assert!(ChartData::from_json(json.as_bytes()).is_some());
        assert_eq!(ChartData::from_json(b"{}"), None);
        assert_eq!(ChartData::from_json(b"not json"), None);
        assert_eq!(ChartData::from_json(b"[]"), None);

        let end = format!("\"end\":{}", start + MS_PER_HOUR);
        for (valid, broken) in [
            ("\"theme\":\"Dark Mode\"", "\"theme\":\"Sepia\"".to_string()),
            ("\"highlightQuietHours\":false", "\"highlight\":0".to_string()),
            ("\"fiveHour\":null", "\"fiveHour\":7".to_string()),
            ("\"remaining\":88", "\"remaining\":\"88\"".to_string()),
            ("\"remaining\":88", "\"remaining\":5000000000".to_string()),
            ("\"forecast\":\"Will", "\"forecast\":7,\"x\":\"Will".to_string()),
            ("\"reset\":\"Nov", "\"reset\":7,\"x\":\"Nov".to_string()),
            (",12.5,1]", ",\"12.5\",1]".to_string()),
            // A link code that does not exist, and a reading with a part missing.
            (",12.5,1]", ",12.5,7]".to_string()),
            (",12.5,1]", ",12.5]".to_string()),
            ("\"usesSessionPace\":false", "\"usesSessionPace\":0".to_string()),
            ("\"endRemaining\":80.0", "\"endRemaining\":null".to_string()),
            ("\"points\":[]", "\"points\":[[1]]".to_string()),
            ("\"quiet\":[[", "\"quiet\":[[1],[".to_string()),
            // The time range must not be empty, reversed or out of range.
            (end.as_str(), format!("\"end\":{start},\"x\":{start}")),
            (end.as_str(), format!("\"end\":{},\"x\":0", start - 1)),
            (end.as_str(), format!("\"end\":{},\"x\":0", MAX_TIMESTAMP_MS + 1)),
        ] {
            assert!(json.contains(valid), "{valid} is not part of {json}");
            let broken = json.replacen(valid, &broken, 1);
            assert_eq!(ChartData::from_json(broken.as_bytes()), None, "{broken}");
        }
    }

    #[test]
    fn a_history_filled_to_its_cap_fits_the_payload_limit() {
        let data = ChartData {
            theme: ThemeChoice::System,
            highlight_quiet_hours: false,
            weekly: full_window(MAX_POINTS, MAX_PATH_POINTS),
            five_hour: Some(full_window(MAX_POINTS, MAX_PATH_POINTS)),
        };
        let json = data.to_json();
        assert!(
            json.len() <= MAX_HISTORY_DATA_BYTES as usize,
            "{} bytes",
            json.len()
        );
        // The worst case is not far below the limit, so the limit is not
        // generous enough to hide a payload that grew by mistake.
        assert!(json.len() > MAX_HISTORY_DATA_BYTES as usize / 2);
        assert_eq!(ChartData::from_json(json.as_bytes()), Some(data));

        // Anything larger than this module sends is refused outright.
        for (count, path) in [(MAX_POINTS + 1, 0), (1, MAX_PATH_POINTS + 1)] {
            let oversized = weekly_only(full_window(count, path));
            assert_eq!(ChartData::from_json(oversized.to_json().as_bytes()), None);
        }
        let mut quiet = full_window(1, 0);
        quiet.quiet_periods = vec![(0, 1); MAX_PATH_POINTS + 1];
        assert_eq!(
            ChartData::from_json(weekly_only(quiet).to_json().as_bytes()),
            None
        );
    }

    #[test]
    fn readings_are_marked_by_how_they_join_the_one_before() {
        let reset = NOW + 2 * DAY;
        let samples = [
            // An earlier cycle, which the chart leaves out.
            weekly_sample(NOW - 3 * DAY, 80.0, Some(NOW - 2 * DAY)),
            weekly_sample(NOW - 30 * HOUR, 0.0, Some(reset)),
            weekly_sample(NOW - 29 * HOUR, 10.0, Some(reset)),
            // Unchanged polls keep only the ends of the plateau.
            weekly_sample(NOW - 28 * HOUR, 10.0, Some(reset)),
            weekly_sample(NOW - 27 * HOUR, 10.0, Some(reset)),
            // More than two hours later.
            weekly_sample(NOW - 4 * HOUR, 30.0, Some(reset)),
            weekly_sample(NOW - 3 * HOUR, 30.0, Some(reset)),
            weekly_sample(NOW - 2 * HOUR, 45.0, Some(reset)),
        ];
        let series = ChartSeries::new(&samples, true, NOW);
        let window = ChartWindow::from_series(&series, None);
        let summary: Vec<(i64, f64, bool, Link)> = window
            .points
            .iter()
            .map(|point| {
                (
                    point.timestamp_ms / 1_000 - NOW,
                    point.used_percent,
                    point.changed,
                    point.link,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (-30 * HOUR, 0.0, true, Link::Continuous),
                (-29 * HOUR, 10.0, true, Link::Continuous),
                (-27 * HOUR, 10.0, false, Link::Continuous),
                (-4 * HOUR, 30.0, true, Link::Gap),
                (-3 * HOUR, 30.0, false, Link::Continuous),
                (-2 * HOUR, 45.0, true, Link::Continuous),
            ]
        );
        assert_eq!(window.start_ms, (NOW - 30 * HOUR) * 1_000);
        assert_eq!(
            window.end_ms,
            reset * 1_000,
            "the axis runs to the known reset"
        );
        assert!(window.projection.is_none() && window.quiet_periods.is_empty());

        // Dropping the plateau's interior can leave neighbours that no longer
        // share a deadline within tolerance; they are shown as a boundary.
        let mut adjusted = series.clone();
        adjusted.samples[5].window.reset_at_ms = Some((reset + DAY) * 1_000);
        let window = ChartWindow::from_series(&adjusted, None);
        assert_eq!(window.points[5].link, Link::Reset);
        assert!(window.points[5].changed);

        let empty = ChartWindow::from_series(&ChartSeries::new(&[], true, NOW), None);
        assert!(empty.points.is_empty());
        assert_eq!(
            (empty.start_ms, empty.end_ms),
            (NOW * 1_000, NOW * 1_000 + MS_PER_MINUTE)
        );
    }

    #[test]
    fn only_a_projection_that_continues_the_latest_reading_is_sent() {
        let samples = [
            weekly_sample(NOW - 4 * HOUR, 20.0, None),
            weekly_sample(NOW - HOUR, 40.0, None),
        ];
        let series = ChartSeries::new(&samples, true, NOW);
        let latest = (NOW - HOUR) * 1_000;
        let end = (NOW + 2 * HOUR) * 1_000;
        let mut projection = straight((latest, 60.0), (end, 10.0));
        projection.uses_inactive_hours = true;
        projection.points = [(0, 60.0), (1, 60.0), (2, 60.0), (3, 10.0)]
            .map(|(hours, remaining_percent)| ProjectionPoint {
                timestamp_ms: latest + hours * MS_PER_HOUR,
                remaining_percent,
            })
            .to_vec();

        let window = ChartWindow::from_series(&series, Some(projection.clone()));
        assert_eq!(window.projection.as_ref(), Some(&projection));
        assert_eq!(window.end_ms, end, "the axis includes the forecast endpoint");
        assert_eq!(
            window.quiet_periods,
            [(latest, latest + 2 * MS_PER_HOUR)],
            "adjoining quiet hours are one period"
        );

        let unrelated = straight((NOW * 1_000, 60.0), (end, 10.0));
        let window = ChartWindow::from_series(&series, Some(unrelated));
        assert!(window.projection.is_none() && window.quiet_periods.is_empty());
        assert_eq!(
            window.end_ms,
            NOW * 1_000,
            "a projection unrelated to the latest reading does not stretch the axis"
        );
    }

    #[test]
    fn paths_and_quiet_periods_stay_within_the_payload_bounds() {
        let step = |index: usize| index as i64 * MS_PER_HOUR;
        let count = 3 * MAX_PATH_POINTS + 7;
        let mut projection = straight((0, 100.0), (step(count - 1), 0.0));
        projection.points = (0..count)
            .map(|index| ProjectionPoint {
                timestamp_ms: step(index),
                remaining_percent: 100.0 - index as f64 / count as f64,
            })
            .collect();
        let thinned = within_bounds(projection.clone());
        assert_eq!(thinned.points.len(), MAX_PATH_POINTS);
        assert_eq!(thinned.points[0], projection.points[0]);
        assert_eq!(thinned.points.last(), projection.points.last());
        assert!(thinned
            .points
            .windows(2)
            .all(|pair| pair[1].timestamp_ms > pair[0].timestamp_ms));
        assert_eq!((thinned.start_ms, thinned.end_ms), (0, step(count - 1)));

        projection.points.truncate(MAX_PATH_POINTS);
        assert_eq!(within_bounds(projection.clone()), projection);

        // Separate periods stay separate, up to the bound.
        let separate: Vec<(i64, i64)> = (0..MAX_PATH_POINTS + 5)
            .map(|index| (step(2 * index), step(2 * index + 1)))
            .collect();
        assert_eq!(
            joined(separate.clone()),
            separate[..MAX_PATH_POINTS].to_vec()
        );
        assert_eq!(
            joined(vec![(0, 5), (5, 9), (12, 20), (20, 21), (30, 31)]),
            [(0, 9), (12, 21), (30, 31)]
        );
    }

    #[test]
    fn a_chart_is_built_from_the_store_and_the_reading_on_display() {
        let directory = TempDir::new("chart-data");
        let mut store = open(&directory.file("history.json"));
        let reset = NOW + 3 * DAY;
        let mut latest = None;
        for (hours_ago, used) in [(12, 14.0), (6, 17.0), (0, 20.0)] {
            let reading = snapshot(NOW - hours_ago * HOUR, used, Some(reset));
            assert!(store.observe(&reading, NOW));
            latest = Some(reading);
        }
        let usage = latest.unwrap();
        let settings = Settings::default();

        let data = ChartData::build(&store, Some(&usage), true, &settings, NOW);
        assert_eq!(data.theme, settings.theme);
        assert!(!data.highlight_quiet_hours);
        let weekly = &data.weekly;
        assert_eq!(weekly.remaining_percent, Some(80));
        assert_eq!(weekly.reset, Some(format_date_time(reset, NOW)));
        assert_eq!(weekly.points.len(), 3);
        assert_eq!(
            (weekly.start_ms, weekly.end_ms),
            ((NOW - 12 * HOUR) * 1_000, reset * 1_000)
        );
        let projection = weekly
            .projection
            .as_ref()
            .expect("three readings over twelve hours give an estimate");
        assert_eq!(projection.start_ms, NOW * 1_000);
        assert_eq!(projection.start_remaining_percent, 80.0);
        let forecast_settings = ForecastSettings::default();
        assert_eq!(
            weekly.forecast,
            store
                .forecast(true, &usage, true, forecast_settings, NOW)
                .text(NOW)
        );
        let five_hour = data
            .five_hour
            .as_ref()
            .expect("the reading has a five-hour limit");
        assert_eq!(five_hour.remaining_percent, Some(80));
        assert_eq!(five_hour.points.len(), 3);
        assert_eq!(ChartData::from_json(data.to_json().as_bytes()), Some(data));

        // Switching projections off removes the path but not the forecast.
        let hidden = Settings {
            show_projected_usage: false,
            highlight_quiet_hours: true,
            theme: ThemeChoice::Light,
            ..Settings::default()
        };
        let data = ChartData::build(&store, Some(&usage), true, &hidden, NOW);
        assert!(data.weekly.projection.is_none());
        assert_eq!(data.weekly.end_ms, reset * 1_000);
        assert!(!data.weekly.forecast.is_empty());
        assert!(data.highlight_quiet_hours);
        assert_eq!(data.theme, ThemeChoice::Light);

        // A failed or paused reading keeps its numbers but loses its estimate.
        let data = ChartData::build(&store, Some(&usage), false, &settings, NOW);
        assert!(data.weekly.projection.is_none());
        assert_eq!(data.weekly.forecast, Forecast::NeedsRefresh.text(NOW));
        assert_eq!(data.weekly.remaining_percent, Some(80));

        // Without a five-hour limit only the weekly window is offered.
        let weekly_only = Usage::from_windows(usage.updated_at, None, usage.weekly.clone());
        let data = ChartData::build(&store, Some(&weekly_only), true, &settings, NOW);
        assert!(data.five_hour.is_none());
        assert_eq!(data.window(false), None);

        // Before the first reading of a session the stored history is still shown.
        let data = ChartData::build(&store, None, true, &settings, NOW + MINUTE);
        assert_eq!(data.weekly.remaining_percent, None);
        assert_eq!(data.weekly.reset, None);
        assert_eq!(data.weekly.forecast, Forecast::NeedsRefresh.text(NOW));
        assert_eq!(data.weekly.points.len(), 3);
        assert!(data.weekly.projection.is_none() && data.five_hour.is_none());

        // A reading without a deadline has no reset to show.
        let no_deadline = snapshot(NOW, 20.0, None);
        let data = ChartData::build(&store, Some(&no_deadline), true, &settings, NOW);
        assert_eq!(data.weekly.reset, None);
        assert_eq!(data.weekly.remaining_percent, Some(80));

        // An empty history with no reading is what a chart starts out showing.
        let empty = open(&directory.file("empty.json"));
        let data = ChartData::build(&empty, None, true, &settings, NOW);
        assert!(data.weekly.points.is_empty());
        assert!(data.weekly.end_ms > data.weekly.start_ms);
        assert_eq!(data, ChartData::empty(NOW));
        assert_eq!(ChartData::from_json(data.to_json().as_bytes()), Some(data));
    }

    #[test]
    fn each_window_plots_its_own_readings() {
        let directory = TempDir::new("chart-windows");
        let mut store = open(&directory.file("history.json"));
        let window = |used_percent: f64, window_minutes: u32, resets_at: i64| LimitWindow {
            used_percent,
            window_minutes,
            resets_at: Some(resets_at),
        };
        let mut latest = None;
        for (minutes_ago, five_hour_used, weekly_used) in [(40, 5.0, 30.0), (20, 9.0, 30.0)] {
            let reading = Usage::from_windows(
                NOW - minutes_ago * MINUTE,
                Some(window(five_hour_used, FIVE_HOUR_MINUTES, NOW + 4 * HOUR)),
                Some(window(weekly_used, WEEKLY_MINUTES, NOW + 5 * DAY)),
            );
            assert!(store.observe(&reading, NOW));
            latest = Some(reading);
        }
        let data = ChartData::build(&store, latest.as_ref(), true, &Settings::default(), NOW);
        let five_hour = data.five_hour.as_ref().unwrap();
        assert_eq!(five_hour.remaining_percent, Some(91));
        assert_eq!(five_hour.end_ms, (NOW + 4 * HOUR) * 1_000);
        assert!(five_hour.points.iter().all(|point| point.changed));
        assert_eq!(data.weekly.remaining_percent, Some(70));
        assert_eq!(data.weekly.end_ms, (NOW + 5 * DAY) * 1_000);
        assert_eq!(
            data.weekly
                .points
                .iter()
                .map(|point| point.changed)
                .collect::<Vec<_>>(),
            [true, false]
        );
    }
}
