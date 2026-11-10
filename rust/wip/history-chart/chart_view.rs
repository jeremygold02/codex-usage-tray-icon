//! What a usage history window shows and how it reacts to input: the header
//! with the Weekly / 5-hour choice and the summary, the chart, and the
//! footer. `chart_window.rs` owns the window and feeds this view; nothing
//! here touches a window handle.
//!
//! The view draws what `chart_data` prepared. Everything that depends on the
//! history (what is plotted, which readings count as changes, which
//! projection applies) was decided by the tray and is only read here.

use std::ptr::null;

use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::Globalization::{
    GetDateFormatEx, GetTimeFormatEx, DATE_SHORTDATE, TIME_NOSECONDS,
};

use crate::chart_data::{ChartData, ChartWindow, Link};
use crate::controls::{self, mix, Cursor, Interaction, Painter, Style};
use crate::forecast::Projection;
use crate::gfx::{Align, Dash, Font, Graphics, Rect};
use crate::history_view::axis_ticks;
use crate::localtime::{decimal_separator, format_date, LocalTime, TimeZone};
use crate::render::Theme;
use crate::timefmt::now_unix;
use crate::win::from_wide;

/// The client size the window opens with, at 100% scale.
pub const CLIENT_SIZE: (f32, f32) = (1000.0, 700.0);
/// The smallest size the window can be resized to, frame included, at 100%.
pub const MIN_WINDOW_SIZE: (f32, f32) = (760.0, 540.0);

const UI_FONTS: [&str; 3] = ["Segoe UI", "Tahoma", "Microsoft Sans Serif"];
const SEMIBOLD_FONTS: [&str; 1] = ["Segoe UI Semibold"];
/// Ten points, the size of the chart's labels and the footer.
const BODY_FONT_SIZE: f32 = 40.0 / 3.0;

const WINDOW_PADDING: (f32, f32) = (26.0, 18.0);
const HEADER_HEIGHT: f32 = 142.0;
/// The part of the header that holds the Weekly / 5-hour choice.
const PERIOD_ROW_HEIGHT: f32 = 46.0;
const PERIOD_SIZE: (f32, f32) = (125.0, 40.0);
const SUMMARY_MARGIN: f32 = 8.0;
/// The summary's columns: remaining, next reset and forecast.
const SUMMARY_SHARES: (f32, f32) = (0.20, 0.34);
const SUMMARY_ICON_COLUMN: f32 = 32.0;
const HEADER_GAP: f32 = 12.0;
/// The footer's left column, which holds the inspected reading.
const DETAIL_SHARE: f32 = 0.48;
const FOOTER_MARGIN: f32 = 8.0;
/// Space left of the plot for the percent labels, and right of it.
const PLOT_INSET: (f32, f32) = (48.0, 16.0);

const PROJECTION_DARK: u32 = 0xFFF2_B24D;
const PROJECTION_LIGHT: u32 = 0xFFA0_5900;
const MARKER_OUTLINE: u32 = 0xFFFF_FFFF;
const PROJECTION_TINT_ALPHA: u32 = 18;
const QUIET_TINT_ALPHA: u32 = 24;
const GUIDE_ALPHA: u32 = 160;

const NO_READINGS: &str = "No recorded readings";
const EMPTY_CHART: [&str; 2] = [
    "No readings recorded yet.",
    "History starts with successful usage refreshes.",
];
const UNKNOWN: &str = "\u{2014}";
/// As wide as a time label gets in most locales; ticks are spaced for it.
const WIDE_TIME: &str = "11:59 PM";

/// The parts of the window that take the pointer or the keyboard focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Id {
    Weekly,
    FiveHour,
    Chart,
}

/// A key the view reacts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Tab,
    Escape,
    Left,
    Right,
    Home,
    End,
}

/// Something the window must do on the view's behalf.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Close,
}

/// What the footer describes and the chart marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Selected {
    /// A plotted reading, by index.
    Reading(usize),
    /// A time on the projected path, in Unix milliseconds.
    Projected(i64),
}

/// The plot area and the time range across it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Plot {
    area: Rect,
    start_ms: i64,
    end_ms: i64,
}

impl Plot {
    fn x(&self, timestamp_ms: i64) -> f32 {
        let span = self.end_ms.saturating_sub(self.start_ms).max(1) as f64;
        let fraction = timestamp_ms.saturating_sub(self.start_ms) as f64 / span;
        self.area.x + (f64::from(self.area.w) * fraction) as f32
    }

    fn y(&self, remaining_percent: f64) -> f32 {
        let fraction = remaining_percent.clamp(0.0, 100.0) / 100.0;
        self.area.bottom() - (f64::from(self.area.h) * fraction) as f32
    }

    /// The time at a horizontal position, held within the axis.
    fn time_at(&self, x: f32) -> i64 {
        let fraction = f64::from(((x - self.area.x) / self.area.w).clamp(0.0, 1.0));
        let span = self.end_ms.saturating_sub(self.start_ms) as f64;
        self.start_ms.saturating_add((span * fraction).round() as i64)
    }
}

/// What the pointer selects at a position: nothing outside the plot, a time
/// on the projected path to the right of the latest reading, and otherwise
/// the nearest reading whose percentage changed.
fn hit(window: &ChartWindow, plot: &Plot, x: f32, y: f32) -> Option<Selected> {
    if !plot.area.contains(x, y) {
        return None;
    }
    if let Some(projection) = &window.projection {
        if x > plot.x(projection.start_ms) {
            return Some(Selected::Projected(plot.time_at(x)));
        }
    }
    let mut nearest = None;
    let mut distance = f32::MAX;
    for (index, point) in window.points.iter().enumerate() {
        let candidate = (plot.x(point.timestamp_ms) - x).abs();
        if point.changed && candidate < distance {
            distance = candidate;
            nearest = Some(index);
        }
    }
    nearest.map(Selected::Reading)
}

/// The latest reading that can be selected.
fn latest_change(window: &ChartWindow) -> Option<usize> {
    window.points.iter().rposition(|point| point.changed)
}

/// The fonts of the header, the chart and the footer at one scale.
struct Fonts {
    scale: f32,
    body: Font,
    /// "Remaining" above the large percentage.
    caption: Font,
    /// "Next reset" and "Forecast".
    label: Font,
    value: Font,
    remaining: Font,
}

impl Fonts {
    fn new(scale: f32) -> Option<Self> {
        let regular = |size: f32| Font::new(&UI_FONTS, size * scale, false);
        let semibold = |size: f32| {
            Font::new(&SEMIBOLD_FONTS, size * scale, false)
                .or_else(|| Font::new(&UI_FONTS, size * scale, true))
        };
        Some(Self {
            scale,
            body: regular(BODY_FONT_SIZE)?,
            caption: regular(14.0)?,
            label: regular(13.0)?,
            value: semibold(15.0)?,
            remaining: semibold(29.0)?,
        })
    }
}

/// Dates and numbers written the way the user's locale writes them.
struct Texts<'a> {
    zone: &'a TimeZone,
    separator: &'a str,
}

impl Texts<'_> {
    /// The time of day without seconds, such as `4:38 PM`.
    fn time(&self, timestamp_ms: i64) -> String {
        time_text(self.zone.to_local(timestamp_ms), false)
    }

    /// The day, such as `Oct 28`.
    fn date(&self, timestamp_ms: i64) -> String {
        format_date(self.zone.to_local(timestamp_ms).date(), "MMM d")
    }

    /// The short date and the time with seconds, such as `2026-11-03 3:38:01 PM`.
    fn date_time(&self, timestamp_ms: i64) -> String {
        let local = self.zone.to_local(timestamp_ms);
        format!("{} {}", short_date_text(local), time_text(local, true))
    }

    /// The footer's description of a plotted reading.
    fn reading(&self, window: &ChartWindow, index: usize) -> Option<String> {
        let point = window.points.get(index)?;
        let boundary = if index > 0 && point.link == Link::Reset {
            "  |  Reset or adjustment"
        } else {
            ""
        };
        Some(format!(
            "{}\n{}% remaining  |  {}% used{boundary}",
            self.date_time(point.timestamp_ms),
            one_decimal(100.0 - point.used_percent, self.separator),
            one_decimal(point.used_percent, self.separator),
        ))
    }

    /// The footer's description of a time on the projected path. Remaining
    /// is rounded once and used is its complement, so the two add to 100.
    fn projected(&self, projection: &Projection, timestamp_ms: i64) -> String {
        let remaining = projection
            .remaining_at(timestamp_ms)
            .clamp(0.0, 100.0)
            .round() as i64;
        format!(
            "Projected: {}\n\u{2248}{remaining}% remaining  |  \u{2248}{}% used",
            self.date_time(timestamp_ms),
            100 - remaining
        )
    }
}

fn system_time(local: LocalTime) -> Option<SYSTEMTIME> {
    let date = local.date();
    Some(SYSTEMTIME {
        wYear: u16::try_from(date.year()).ok()?,
        wMonth: date.month() as u16,
        wDayOfWeek: date.weekday() as u16,
        wDay: date.day() as u16,
        wHour: local.hour() as u16,
        wMinute: local.minute() as u16,
        wSecond: local.second() as u16,
        wMilliseconds: 0,
    })
}

fn time_text(local: LocalTime, seconds: bool) -> String {
    let mut buffer = [0u16; 80];
    let written = system_time(local).map_or(0, |time| {
        let flags = if seconds { 0 } else { TIME_NOSECONDS };
        // SAFETY: the buffer length passed matches the buffer, and the time
        // outlives the call.
        unsafe {
            GetTimeFormatEx(
                null(),
                flags,
                &time,
                null(),
                buffer.as_mut_ptr(),
                buffer.len() as i32,
            )
        }
    });
    if written > 0 {
        from_wide(&buffer)
    } else if seconds {
        format!(
            "{:02}:{:02}:{:02}",
            local.hour(),
            local.minute(),
            local.second()
        )
    } else {
        format!("{:02}:{:02}", local.hour(), local.minute())
    }
}

fn short_date_text(local: LocalTime) -> String {
    let mut buffer = [0u16; 80];
    let written = system_time(local).map_or(0, |time| {
        // SAFETY: the buffer length passed matches the buffer, and the time
        // outlives the call.
        unsafe {
            GetDateFormatEx(
                null(),
                DATE_SHORTDATE,
                &time,
                null(),
                buffer.as_mut_ptr(),
                buffer.len() as i32,
                null(),
            )
        }
    });
    if written > 0 {
        from_wide(&buffer)
    } else {
        let date = local.date();
        format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
    }
}

/// A number with at most one decimal and no trailing zero, rounded half away
/// from zero the way the C# app's `0.#` format rounds.
fn one_decimal(value: f64, separator: &str) -> String {
    // .NET rounds the decimal form of the number, so 0.15, which a float
    // stores slightly low, still goes up. The nudge reproduces that.
    let tenths = (value.abs() * 10.0 + 0.5 + 1e-9).floor() as i64;
    let sign = if value < 0.0 && tenths > 0 { "-" } else { "" };
    match tenths % 10 {
        0 => format!("{sign}{}", tenths / 10),
        tenth => format!("{sign}{}{separator}{tenth}", tenths / 10),
    }
}

fn with_alpha(color: u32, alpha: u32) -> u32 {
    (color & 0x00FF_FFFF) | (alpha << 24)
}

fn projection_color(dark: bool) -> u32 {
    if dark {
        PROJECTION_DARK
    } else {
        PROJECTION_LIGHT
    }
}

/// The color for the window's title bar: a step darker than the window, as
/// in the Settings window, or `None` to keep the system's light title bar.
pub fn caption_color(dark: bool) -> Option<u32> {
    dark.then(|| mix(Theme::new(true).surface, 0xFF00_0000, 0.12))
}

fn disc(g: &Graphics, color: u32, center: (f32, f32), radius: f32) {
    let bounds = Rect::new(
        center.0 - radius,
        center.1 - radius,
        2.0 * radius,
        2.0 * radius,
    );
    g.fill_ellipse(color, bounds);
}

/// A circle whose line straddles `radius`, as a pen's does.
fn ring(g: &Graphics, color: u32, center: (f32, f32), radius: f32, width: f32) {
    let outer = radius + width / 2.0;
    let bounds = Rect::new(center.0 - outer, center.1 - outer, 2.0 * outer, 2.0 * outer);
    g.stroke_ellipse(color, bounds, width);
}

/// A vertical line `width` whole pixels wide, centered on `x` as nearly as
/// whole pixels allow so it stays sharp.
fn upright(g: &Graphics, color: u32, x: f32, top: f32, height: f32, width: f32) {
    g.fill_rect(
        color,
        Rect::new((x - width / 2.0).round(), top, width, height),
    );
}

/// The calendar beside "Next reset" or the rising line beside "Forecast",
/// drawn in a 23-unit box centered in `rect`.
fn summary_icon(g: &Graphics, forecast: bool, color: u32, rect: Rect) {
    let unit = (rect.w / 32.0).min(rect.h / 32.0);
    let left = rect.x + ((rect.w - 23.0 * unit) / 2.0 - 2.0 * unit).max(0.0);
    let top = rect.y + (rect.h - 23.0 * unit) / 2.0;
    let at = |x: f32, y: f32| (left + x * unit, top + y * unit);
    let width = (1.7 * unit).max(1.4);
    if forecast {
        g.polyline(
            color,
            &[at(2.0, 17.0), at(8.0, 11.0), at(12.0, 14.0), at(20.0, 5.0)],
            width,
        );
        g.polyline(color, &[at(15.0, 5.0), at(20.0, 5.0), at(20.0, 10.0)], width);
    } else {
        let (x, y) = at(2.0, 5.0);
        let page = Rect::new(x, y, 18.0 * unit, 16.0 * unit).inflate(width / 2.0);
        g.stroke_round_rect_wide(color, page, 3.0 * unit, width);
        g.line(color, at(2.0, 10.0), at(20.0, 10.0), width);
        g.line(color, at(6.0, 2.0), at(6.0, 7.0), width);
        g.line(color, at(16.0, 2.0), at(16.0, 7.0), width);
    }
}

/// Where the chart and its plot ended up in a layout pass.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame {
    chart: Rect,
    plot: Plot,
    label_height: f32,
    /// Whether the time axis spans more than one local day and so shows dates.
    show_dates: bool,
}

/// What one layout-and-paint pass draws with.
struct Scene<'a> {
    g: &'a Graphics,
    style: &'a Style,
    fonts: &'a Fonts,
    texts: Texts<'a>,
}

impl Scene<'_> {
    fn line_height(&self, font: &Font) -> f32 {
        self.g.measure("Ag", font).1
    }

    /// Draws the summary under the Weekly / 5-hour choice: the remaining
    /// percentage, the next reset and the forecast, in columns of 20, 34
    /// and 46 percent with a line between them.
    fn summary(&self, window: &ChartWindow, area: Rect) {
        let (g, style, fonts) = (self.g, self.style, self.fonts);
        let palette = &style.palette;
        let px = |value: f32| style.px(value);
        let hairline = style.hairline();
        let columns = (area.w - 2.0 * hairline).max(0.0);
        let first = (columns * SUMMARY_SHARES.0).round();
        let second = (columns * SUMMARY_SHARES.1).round();
        let third = columns - first - second;

        let remaining = window
            .remaining_percent
            .map_or_else(|| UNKNOWN.to_string(), |percent| format!("{percent}%"));
        let caption_height = self.line_height(&fonts.caption);
        let remaining_height = self.line_height(&fonts.remaining);
        let x = area.x + px(4.0);
        let top = area.y + ((area.h - caption_height - remaining_height) / 2.0).round();
        g.text(
            "Remaining",
            &fonts.caption,
            palette.text,
            Rect::new(x, top, 0.0, caption_height),
            Align::Near,
            Align::Center,
        );
        g.text(
            &remaining,
            &fonts.remaining,
            palette.text,
            Rect::new(x, top + caption_height, 0.0, remaining_height),
            Align::Near,
            Align::Center,
        );

        let reset = window.reset.as_deref().unwrap_or(UNKNOWN);
        let accent = palette.accent;
        let amber = projection_color(palette.dark);
        let mut left = area.x + first;
        for (label, value, width, forecast, color, margin) in [
            ("Next reset", reset, second, false, accent, px(8.0)),
            ("Forecast", window.forecast.as_str(), third, true, amber, 0.0),
        ] {
            let divider = Rect::new(left, area.y + px(6.0), hairline, area.h - 2.0 * px(6.0));
            g.fill_rect(palette.border, divider);
            let icon_left = left + hairline + px(14.0);
            let text_left = icon_left + px(SUMMARY_ICON_COLUMN);
            let text_width = (left + hairline + width - margin - text_left).max(px(40.0));
            let label_height = self.line_height(&fonts.label);
            let value_height = g.measure_wrapped(value, &fonts.value, text_width);
            let top = area.y + ((area.h - label_height - value_height) / 2.0).round();
            let side = px(SUMMARY_ICON_COLUMN);
            let icon = Rect::new(icon_left, area.y + ((area.h - side) / 2.0).round(), side, side);
            summary_icon(g, forecast, color, icon);
            g.text(
                label,
                &fonts.label,
                palette.text,
                Rect::new(text_left, top, 0.0, label_height),
                Align::Near,
                Align::Center,
            );
            let value_rect = Rect::new(text_left, top + label_height, text_width, 0.0);
            g.text_wrapped(value, &fonts.value, palette.text, value_rect);
            left += hairline + width;
        }
    }

    /// Draws the footer: the inspected reading on the left and the forecast
    /// on the right, under a line across the window.
    fn footer(&self, detail: &str, forecast: &str, area: Rect) {
        let (g, style, body) = (self.g, self.style, &self.fonts.body);
        let palette = &style.palette;
        let px = |value: f32| style.px(value);
        g.fill_rect(
            palette.border,
            Rect::new(area.x, area.y, area.w, style.hairline()),
        );
        let line_height = self.line_height(body);
        let top = area.y + px(FOOTER_MARGIN);
        for (index, line) in detail.lines().enumerate() {
            let rect = Rect::new(area.x, top + index as f32 * line_height, 0.0, line_height);
            g.text(line, body, palette.text, rect, Align::Near, Align::Center);
        }
        let left = area.x + self.forecast_offset(area.w);
        let rect = Rect::new(left, top, (area.right() - left).max(1.0), 0.0);
        g.text_wrapped(forecast, body, palette.text, rect);
    }

    /// How far into the footer its forecast starts.
    fn forecast_offset(&self, footer_width: f32) -> f32 {
        (footer_width * DETAIL_SHARE).round() + self.style.px(16.0)
    }

    /// The footer's height for a forecast wrapped into its column. The
    /// inspected reading always gets two lines so the chart does not jump.
    fn footer_height(&self, forecast: &str, width: f32) -> f32 {
        let body = &self.fonts.body;
        let column = (width - self.forecast_offset(width)).max(1.0);
        let lines = 2.0 * self.line_height(body) + self.style.px(8.0);
        self.style.px(FOOTER_MARGIN) + lines.max(self.g.measure_wrapped(forecast, body, column))
    }

    /// Draws the chart: labels, grid, axes, the projected region and path,
    /// and the recorded line with its markers.
    fn chart(
        &self,
        window: &ChartWindow,
        frame: &Frame,
        selection: Option<Selected>,
        highlight_quiet_hours: bool,
    ) {
        let (g, style, body) = (self.g, self.style, &self.fonts.body);
        let palette = &style.palette;
        let px = |value: f32| style.px(value);
        let hairline = style.hairline();
        let Frame {
            chart,
            plot,
            label_height,
            ..
        } = *frame;
        let area = plot.area;
        let line = palette.accent;
        let amber = projection_color(palette.dark);
        let stroke = (2.0 * style.scale).max(1.5);
        let heading = |x: f32, width: f32| Rect::new(x, chart.y, width, label_height);

        g.text(
            "Remaining",
            body,
            palette.muted,
            heading(chart.x, 0.0),
            Align::Near,
            Align::Center,
        );
        let latest = window.projection.as_ref().map(|projection| {
            (
                plot.x(projection.start_ms),
                plot.y(projection.start_remaining_percent),
            )
        });
        if let Some((now_x, _)) = latest {
            let projected = Rect::new(now_x, area.y, (area.right() - now_x).max(0.0), area.h);
            g.fill_rect(with_alpha(amber, PROJECTION_TINT_ALPHA), projected);
            if highlight_quiet_hours {
                for &(from, to) in &window.quiet_periods {
                    let left = area.x.max(plot.x(from));
                    let right = area.right().min(plot.x(to));
                    if right > left {
                        let quiet = Rect::new(left, area.y, right - left, area.h);
                        g.fill_rect(with_alpha(line, QUIET_TINT_ALPHA), quiet);
                    }
                }
            }
            let recorded_left = area.x + px(25.0);
            let recorded_width = now_x - recorded_left - px(24.0);
            if recorded_width > px(55.0) {
                g.text(
                    "Recorded",
                    body,
                    line,
                    heading(recorded_left, recorded_width),
                    Align::Center,
                    Align::Center,
                );
            }
            // Near either edge the marker would collide with the labels there.
            if now_x > area.x + px(115.0) && now_x < area.right() - px(115.0) {
                let marker = Rect::new((now_x - px(20.0)).round(), chart.y, px(40.0), label_height);
                let tip = px(4.0);
                let base = marker.bottom() - 1.0;
                g.fill_round_rect(palette.primary, marker, px(5.0));
                g.stroke_round_rect(palette.primary_edge, marker, px(5.0));
                g.fill_polygon(
                    palette.primary,
                    &[(now_x - tip, base), (now_x + tip, base), (now_x, area.y)],
                );
                g.text(
                    "Now",
                    body,
                    palette.on_primary,
                    marker,
                    Align::Center,
                    Align::Center,
                );
            }
            g.text(
                "Projected",
                body,
                amber,
                heading(area.right() - px(100.0), px(100.0)),
                Align::Far,
                Align::Center,
            );
        }

        for percent in [0u8, 25, 50, 75, 100] {
            let y = area.bottom() - area.h * f32::from(percent) / 100.0;
            // An empty chart keeps only its frame, behind the notice.
            if !window.points.is_empty() || percent % 100 == 0 {
                let row = Rect::new(area.x, (y - hairline / 2.0).round(), area.w, hairline);
                g.fill_rect(palette.border, row);
            }
            let label = Rect::new(
                chart.x,
                y - label_height / 2.0,
                px(PLOT_INSET.0) - px(6.0),
                label_height,
            );
            g.text(
                &format!("{percent}%"),
                body,
                palette.muted,
                label,
                Align::Far,
                Align::Center,
            );
        }
        if window.points.is_empty() {
            let line_height = self.line_height(body);
            let top = area.y + ((area.h - 2.0 * line_height) / 2.0).round();
            for (index, text) in EMPTY_CHART.into_iter().enumerate() {
                let row = Rect::new(area.x, top + index as f32 * line_height, area.w, line_height);
                g.text(text, body, palette.text, row, Align::Center, Align::Center);
            }
            return;
        }
        self.time_axis(frame);

        if let (Some(projection), Some(start)) = (&window.projection, latest) {
            upright(g, line, start.0, area.y, area.h, hairline);
            disc(g, line, start, 5.0 * style.scale);
            ring(g, MARKER_OUTLINE, start, 5.0 * style.scale, hairline);
            let end = (
                plot.x(projection.end_ms),
                plot.y(projection.end_remaining_percent),
            );
            let mut path = vec![start];
            if projection.points.is_empty() {
                path.push(end);
            } else {
                path.extend(projection.points.iter().map(|point| {
                    (
                        plot.x(point.timestamp_ms),
                        plot.y(point.remaining_percent),
                    )
                }));
            }
            // Once used up, the allowance stays at zero until the reset.
            if projection.end_remaining_percent <= 0.0 && plot.end_ms > projection.end_ms {
                path.push((plot.x(plot.end_ms), plot.y(0.0)));
            }
            g.dashed_polyline(amber, &path, stroke, Dash::Dashes);
            ring(g, amber, end, 3.0 * style.scale, stroke);
            if let Some(Selected::Projected(timestamp_ms)) = selection {
                let remaining = projection.remaining_at(timestamp_ms);
                let at = (plot.x(timestamp_ms), plot.y(remaining));
                upright(g, amber, at.0, area.y, area.h, stroke.round());
                ring(g, amber, at, 4.0 * style.scale, stroke);
            }
        }

        let selected = match selection {
            Some(Selected::Reading(index)) => Some(index),
            _ => None,
        };
        let spots: Vec<(f32, f32)> = window
            .points
            .iter()
            .map(|point| {
                (
                    plot.x(point.timestamp_ms),
                    plot.y(100.0 - point.used_percent),
                )
            })
            .collect();
        if let Some(&(x, _)) = selected.and_then(|index| spots.get(index)) {
            upright(g, with_alpha(line, GUIDE_ALPHA), x, area.y, area.h, hairline);
        }
        // Readings that follow one another without a break share one line.
        let mut run: Vec<(f32, f32)> = Vec::new();
        for (index, point) in window.points.iter().enumerate() {
            if index > 0 && point.link != Link::Continuous {
                g.polyline(line, &run, stroke);
                run.clear();
                if point.link == Link::Gap {
                    let gap = [spots[index - 1], spots[index]];
                    g.dashed_polyline(line, &gap, stroke, Dash::Dashes);
                }
            }
            run.push(spots[index]);
        }
        g.polyline(line, &run, stroke);

        let last = spots.len() - 1;
        for (index, (point, &spot)) in window.points.iter().zip(&spots).enumerate() {
            if index > 0 && point.link == Link::Reset {
                let center = (spot.0 - hairline / 2.0).round() + hairline / 2.0;
                let marker = [(center, area.y), (center, area.bottom())];
                g.dashed_polyline(palette.muted, &marker, hairline, Dash::Dots);
                let half = 3.0 * style.scale;
                let square = Rect::new(spot.0 - half, spot.1 - half, 2.0 * half, 2.0 * half);
                g.fill_rect(line, square);
            } else if index == 0 || ((index == last || selected == Some(index)) && point.changed) {
                disc(g, line, spot, 2.0 * style.scale);
            }
            if selected == Some(index) {
                ring(g, line, spot, 5.0 * style.scale, stroke);
            }
        }
    }

    /// Draws the time axis: a tick and a label at both ends and at round
    /// local times between them, leaving out labels that would crowd.
    fn time_axis(&self, frame: &Frame) {
        let (g, style, body) = (self.g, self.style, &self.fonts.body);
        let palette = &style.palette;
        let px = |value: f32| style.px(value);
        let hairline = style.hairline();
        let Frame {
            plot,
            label_height,
            show_dates,
            ..
        } = *frame;
        let area = plot.area;
        let label_width = |timestamp_ms: i64| {
            let time = g.measure(&self.texts.time(timestamp_ms), body).0;
            if show_dates {
                time.max(g.measure(&self.texts.date(timestamp_ms), body).0)
            } else {
                time
            }
        };
        let widest = label_width(plot.start_ms)
            .max(label_width(plot.end_ms))
            .max(g.measure(WIDE_TIME, body).0)
            + px(4.0);
        let intervals = (area.w / (widest + px(16.0))) as usize;
        let ticks = axis_ticks(plot.start_ms, plot.end_ms, intervals, self.texts.zone);
        let gap = px(12.0);
        let mut previous_right = area.x - gap;
        let last_left = area.right() - label_width(plot.end_ms) - px(4.0);
        for (index, &tick) in ticks.iter().enumerate() {
            let x = plot.x(tick);
            let width = label_width(tick) + px(4.0);
            let (left, align) = if index == 0 {
                (area.x, Align::Near)
            } else if index + 1 == ticks.len() {
                (area.right() - width, Align::Far)
            } else {
                let left = x - width / 2.0;
                if left < previous_right + gap || left + width > last_left - gap {
                    continue;
                }
                (left, Align::Center)
            };
            let mark = Rect::new((x - hairline / 2.0).round(), area.bottom(), hairline, px(4.0));
            g.fill_rect(palette.border, mark);
            let mut row = Rect::new(left.round(), area.bottom() + px(6.0), width, label_height);
            if show_dates {
                let date = self.texts.date(tick);
                g.text(&date, body, palette.muted, row, align, Align::Center);
                row.y += label_height;
            }
            let time = self.texts.time(tick);
            g.text(&time, body, palette.muted, row, align, Align::Center);
            previous_right = left + width;
        }
    }
}

/// A usage history window's state and behavior.
pub struct View {
    zone: TimeZone,
    separator: String,
    system_dark: bool,
    data: ChartData,
    /// Which window is shown: weekly, or five-hour while the data has one.
    weekly: bool,
    selection: Option<Selected>,
    interaction: Interaction<Id>,
    /// True while the pointer is over the chart or dragging from it.
    pointer_on_chart: bool,
    /// True while the left button is held after going down on the chart.
    dragging: bool,
    style: Option<Style>,
    fonts: Option<Fonts>,
    frame: Option<Frame>,
}

impl View {
    /// An empty chart. `zone` is the time zone its times are shown in, and
    /// `system_dark` the Windows app theme, used while the theme setting
    /// follows the system.
    pub fn new(zone: TimeZone, system_dark: bool) -> Self {
        let mut interaction = Interaction::default();
        interaction.set_focus(Some(Id::Weekly), false);
        Self {
            zone,
            separator: decimal_separator(),
            system_dark,
            data: ChartData::empty(now_unix()),
            weekly: true,
            selection: None,
            interaction,
            pointer_on_chart: false,
            dragging: false,
            style: None,
            fonts: None,
            frame: None,
        }
    }

    /// Whether the window is drawn dark.
    pub fn dark(&self) -> bool {
        self.data.theme.is_dark(self.system_dark)
    }

    /// Tells the view the Windows app theme changed.
    pub fn set_system_dark(&mut self, system_dark: bool) {
        self.system_dark = system_dark;
    }

    /// True while the weekly window is shown rather than the five-hour one.
    pub fn shows_weekly(&self) -> bool {
        self.weekly
    }

    /// True while the account reports a five-hour limit, which is when the
    /// header offers the 5-hour choice.
    pub fn offers_five_hour(&self) -> bool {
        self.data.five_hour.is_some()
    }

    /// Takes in new data from the tray. An inspected reading stays selected
    /// if it is still plotted, and an inspected time on the projected path
    /// if a projection still covers it; a view of a five-hour limit that is
    /// no longer reported falls back to weekly. Returns false when the data
    /// is what is already shown.
    pub fn set_data(&mut self, data: ChartData) -> bool {
        if self.data == data {
            return false;
        }
        // Indexes mean nothing in the new data, so a reading is found again
        // by its time.
        let (reading_ms, projected_ms) = match self.selection {
            Some(Selected::Reading(index)) => {
                let point = self.window().points.get(index);
                (point.map(|point| point.timestamp_ms), None)
            }
            Some(Selected::Projected(timestamp_ms)) => (None, Some(timestamp_ms)),
            None => (None, None),
        };
        self.data = data;
        self.selection = None;
        if !self.weekly && self.data.five_hour.is_none() {
            self.weekly = true;
            if self.interaction.focused() == Some(Id::FiveHour) {
                self.interaction.set_focus(Some(Id::Weekly), false);
            }
            return true;
        }
        let window = self.window();
        let reading = reading_ms.and_then(|timestamp_ms| {
            window
                .points
                .iter()
                .position(|point| point.timestamp_ms == timestamp_ms && point.changed)
        });
        let projected = projected_ms.filter(|timestamp_ms| {
            window
                .projection
                .as_ref()
                .is_some_and(|path| (path.start_ms..=window.end_ms).contains(timestamp_ms))
        });
        self.selection = reading
            .map(Selected::Reading)
            .or(projected.map(Selected::Projected));
        true
    }

    /// The footer's left text: the inspected reading or projected time, and
    /// otherwise the latest reading. Two lines separated by a line feed.
    pub fn detail(&self) -> String {
        let window = self.window();
        let texts = self.texts();
        let selected = match (self.selection, &window.projection) {
            (Some(Selected::Reading(index)), _) => texts.reading(window, index),
            (Some(Selected::Projected(timestamp_ms)), Some(projection)) => {
                Some(texts.projected(projection, timestamp_ms))
            }
            _ => None,
        };
        selected
            .or_else(|| {
                let latest = window.points.len().checked_sub(1)?;
                Some(format!("Latest: {}", texts.reading(window, latest)?))
            })
            .unwrap_or_else(|| NO_READINGS.to_string())
    }

    /// The mouse pointer to show at a point of the window.
    pub fn cursor_at(&self, x: f32, y: f32) -> Cursor {
        self.interaction.cursor_at(x, y)
    }

    /// Follows the pointer; `None` means it left the window. Over the plot
    /// it selects what it points at, elsewhere on the chart it selects
    /// nothing, and leaving the chart goes back to the latest reading.
    /// Returns true when something changed appearance.
    pub fn pointer_moved(&mut self, point: Option<(f32, f32)>) -> bool {
        let mut changed = self.interaction.pointer_moved(point);
        let on_chart = point.filter(|&(x, y)| {
            self.dragging
                || self
                    .frame
                    .is_some_and(|frame| frame.chart.contains(x, y))
        });
        if let Some((x, y)) = on_chart {
            let selection = self
                .frame
                .and_then(|frame| hit(self.window(), &frame.plot, x, y));
            changed |= self.select(selection);
        } else if self.pointer_on_chart {
            changed |= self.select(None);
        }
        self.pointer_on_chart = on_chart.is_some();
        changed
    }

    /// Handles the left button going down: the control under the pointer
    /// takes the focus, and a press on the chart starts a drag across it.
    pub fn pointer_pressed(&mut self, x: f32, y: f32) {
        self.pointer_moved(Some((x, y)));
        let before = self.interaction.focused();
        let pressed = self.interaction.pointer_pressed(x, y);
        self.dragging = pressed == Some(Id::Chart);
        self.focus_moved(before);
    }

    /// Handles the left button coming up, which completes a click on the
    /// Weekly / 5-hour choice and ends a drag across the chart.
    pub fn pointer_released(&mut self, x: f32, y: f32) {
        self.dragging = false;
        match self.interaction.pointer_released(x, y) {
            Some(Id::Weekly) => self.show(true),
            Some(Id::FiveHour) => self.show(false),
            Some(Id::Chart) | None => {}
        }
        self.pointer_moved(Some((x, y)));
    }

    /// The window lost the mouse in the middle of a click or drag.
    pub fn capture_lost(&mut self) {
        self.dragging = false;
        self.interaction.cancel_press();
    }

    /// The window regained the keyboard focus. A focused chart goes back to
    /// inspecting its latest change.
    pub fn focus_gained(&mut self) {
        if self.interaction.focused() == Some(Id::Chart) && self.selection.is_none() {
            self.selection = latest_change(self.window()).map(Selected::Reading);
        }
    }

    /// The window lost the keyboard focus, which ends a keyboard inspection.
    pub fn focus_lost(&mut self) {
        if self.interaction.focused() == Some(Id::Chart) {
            self.selection = None;
        }
    }

    /// Handles a key press. `shift` reverses Tab.
    pub fn key(&mut self, key: Key, shift: bool) -> Option<Command> {
        let focused = self.interaction.focused();
        match (key, focused) {
            (Key::Escape, _) => return Some(Command::Close),
            (Key::Tab, _) => {
                self.interaction.focus_next(shift);
                self.focus_moved(focused);
            }
            (_, Some(Id::Chart)) => self.step(key),
            (Key::Left | Key::Right, Some(Id::Weekly | Id::FiveHour)) => {
                self.choose(key == Key::Left);
            }
            _ => {}
        }
        None
    }

    /// Handles Alt with a character: W shows the weekly limit and 5 the
    /// five-hour one. Returns false for any other character.
    pub fn access_key(&mut self, character: char) -> bool {
        let weekly = match character.to_ascii_lowercase() {
            'w' => true,
            '5' if self.offers_five_hour() => false,
            _ => return false,
        };
        self.choose(weekly);
        true
    }

    /// Lays the window out for a client area of `width` by `height` device
    /// pixels at `scale`, recording where every control is, and paints it
    /// when `paint` is set.
    pub fn render(&mut self, g: &Graphics, scale: f32, width: f32, height: f32, paint: bool) {
        let dark = self.dark();
        let style = match self.style.take() {
            Some(style) if style.scale == scale && style.palette.dark == dark => style,
            _ => match Style::new(scale, dark) {
                Some(style) => style,
                None => return,
            },
        };
        let fonts = match self.fonts.take() {
            Some(fonts) if fonts.scale == scale => Some(fonts),
            _ => Fonts::new(scale),
        };
        if let Some(fonts) = &fonts {
            let mut interaction = std::mem::take(&mut self.interaction);
            interaction.begin_layout();
            self.frame = Some(self.layout(g, &style, fonts, &mut interaction, (width, height), paint));
            interaction.end_layout();
            self.interaction = interaction;
        }
        self.fonts = fonts;
        self.style = Some(style);
    }

    fn layout(
        &self,
        g: &Graphics,
        style: &Style,
        fonts: &Fonts,
        interaction: &mut Interaction<Id>,
        size: (f32, f32),
        paint: bool,
    ) -> Frame {
        let palette = &style.palette;
        let px = |value: f32| style.px(value);
        let window = self.window();
        let scene = Scene {
            g,
            style,
            fonts,
            texts: self.texts(),
        };
        let mut painter = Painter {
            g,
            style,
            interaction,
            paint,
            clip: None,
            backdrop: palette.surface,
        };
        if paint {
            g.clear(palette.surface);
        }
        let (left, top) = (px(WINDOW_PADDING.0), px(WINDOW_PADDING.1));
        let content_width = (size.0 - 2.0 * left).max(1.0);

        // The Weekly / 5-hour choice, which is a single segment for an
        // account without a five-hour limit.
        let choices: &[(Id, &str)] = if self.offers_five_hour() {
            &[(Id::Weekly, "Weekly"), (Id::FiveHour, "5-hour")]
        } else {
            &[(Id::Weekly, "Weekly")]
        };
        let chosen = usize::from(!self.weekly);
        let group = Rect::new(
            left,
            top,
            px(PERIOD_SIZE.0) * choices.len() as f32,
            px(PERIOD_SIZE.1),
        );
        let visuals: Vec<_> = controls::segment_rects(style, group, choices.len())
            .into_iter()
            .zip(choices)
            .enumerate()
            .map(|(index, (rect, (id, _)))| {
                painter.target(*id, rect, true, index == chosen, Cursor::Hand)
            })
            .collect();
        if paint {
            let labels: Vec<&str> = choices.iter().map(|(_, label)| *label).collect();
            g.fill_round_rect(palette.card, group, px(10.0));
            controls::segments(g, style, group, &labels, chosen, &visuals);
            let summary = Rect::new(
                left + px(4.0),
                top + px(PERIOD_ROW_HEIGHT) + px(SUMMARY_MARGIN),
                (content_width - px(4.0)).max(1.0),
                px(HEADER_HEIGHT - PERIOD_ROW_HEIGHT - 2.0 * SUMMARY_MARGIN),
            );
            scene.summary(window, summary);
        }

        // The footer is measured first: the chart gets what is left between.
        let footer_height = scene.footer_height(&window.forecast, content_width);
        let footer_top = size.1 - top - footer_height;
        let chart_top = top + px(HEADER_HEIGHT) + px(HEADER_GAP);
        let chart = Rect::new(
            left,
            chart_top,
            content_width,
            (footer_top - chart_top).max(1.0),
        );
        painter.target(Id::Chart, chart, true, true, Cursor::Arrow);

        let label_height = (scene.line_height(&fonts.body) + px(4.0)).max(px(18.0));
        let day = |timestamp_ms: i64| self.zone.to_local(timestamp_ms).date();
        let show_dates = day(window.start_ms) != day(window.end_ms);
        // Above the plot: a row of labels. Below it: the tick labels, in two
        // rows when they include dates, and breathing room.
        let rows = if show_dates { 4.0 } else { 3.0 };
        let frame = Frame {
            chart,
            plot: Plot {
                area: Rect::new(
                    chart.x + px(PLOT_INSET.0),
                    chart.y + label_height + px(4.0),
                    (chart.w - px(PLOT_INSET.0) - px(PLOT_INSET.1)).max(1.0),
                    (chart.h - rows * label_height - px(8.0)).max(1.0),
                ),
                start_ms: window.start_ms,
                end_ms: window.end_ms,
            },
            label_height,
            show_dates,
        };
        if paint {
            scene.chart(
                window,
                &frame,
                self.selection,
                self.data.highlight_quiet_hours,
            );
            let footer = Rect::new(left, footer_top, content_width, footer_height);
            scene.footer(&self.detail(), &window.forecast, footer);
        }
        frame
    }

    fn window(&self) -> &ChartWindow {
        self.data.window(self.weekly).unwrap_or(&self.data.weekly)
    }

    fn texts(&self) -> Texts<'_> {
        Texts {
            zone: &self.zone,
            separator: &self.separator,
        }
    }

    fn select(&mut self, selection: Option<Selected>) -> bool {
        let changed = self.selection != selection;
        self.selection = selection;
        changed
    }

    /// Switches between the weekly and the five-hour window, which starts a
    /// fresh inspection.
    fn show(&mut self, weekly: bool) {
        let weekly = weekly || !self.offers_five_hour();
        if self.weekly != weekly {
            self.weekly = weekly;
            self.selection = None;
        }
    }

    /// Shows a window from the keyboard, which also moves the focus to its
    /// segment.
    fn choose(&mut self, weekly: bool) {
        let before = self.interaction.focused();
        self.show(weekly);
        let segment = if self.weekly {
            Id::Weekly
        } else {
            Id::FiveHour
        };
        self.interaction.set_focus(Some(segment), true);
        self.focus_moved(before);
    }

    /// Reacts to the focus having moved from `before`. The chart inspects
    /// its latest change when it gains the focus with nothing selected, and
    /// stops inspecting when it loses the focus.
    fn focus_moved(&mut self, before: Option<Id>) {
        let after = self.interaction.focused();
        if before == after {
            return;
        }
        if before == Some(Id::Chart) {
            self.selection = None;
        } else if after == Some(Id::Chart) && self.selection.is_none() {
            self.selection = latest_change(self.window()).map(Selected::Reading);
        }
    }

    /// Moves the inspection with an arrow key, Home or End. Only readings
    /// whose percentage changed are stops. Left and Right start from the
    /// latest and the first reading when none is inspected.
    fn step(&mut self, key: Key) {
        let points = &self.window().points;
        let changed = |index: &usize| points[*index].changed;
        let current = match self.selection {
            Some(Selected::Reading(index)) if index < points.len() => Some(index),
            _ => None,
        };
        let next = match key {
            Key::Left => (0..current.unwrap_or(points.len())).rfind(changed),
            Key::Right => (current.map_or(0, |index| index + 1)..points.len()).find(changed),
            Key::Home => (0..points.len()).find(changed),
            Key::End => (0..points.len()).rfind(changed),
            Key::Tab | Key::Escape => None,
        };
        if let Some(next) = next {
            self.selection = Some(Selected::Reading(next));
        }
    }
}
