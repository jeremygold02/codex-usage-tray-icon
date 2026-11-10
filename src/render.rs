//! Layout and painting of the usage popup. The same pass measures and paints,
//! so the window height and hit targets always match what is drawn.

use crate::gfx::{Align, Font, Graphics, Rect};
use crate::view::PopupView;

const UI_FONTS: [&str; 3] = ["Segoe UI", "Tahoma", "Microsoft Sans Serif"];
const ICON_FONTS: [&str; 2] = ["Segoe Fluent Icons", "Segoe MDL2 Assets"];
const GLYPH_REFRESH: &str = "\u{E72C}";
const GLYPH_SETTINGS: &str = "\u{E713}";
const DAY_BAR: u32 = 0xFF3E_9CE6;
const DAY_BAR_HOVER: u32 = 0xFF83_CDFF;
const GLYPH_CALENDAR: &str = "\u{E787}";
const GLYPH_LIGHTNING: &str = "\u{E945}";
const SPINNER_DIAMETER: f32 = 18.0;
/// Degrees of the loading circle covered by its moving arc.
const SPINNER_SWEEP: f32 = 100.0;
const WEEKLY_BADGE: (u32, u32) = (0xFF29_445F, 0xFF6C_C4FF);
const FIVE_HOUR_BADGE: (u32, u32) = (0xFF57_4817, 0xFFFF_DB0A);
const FALLBACK_HEIGHT: f32 = 120.0;
/// Below this percentage the label no longer fits inside the filled bar.
const MIN_PERCENT_FOR_INSIDE_LABEL: i32 = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Refresh,
    Provider,
    History,
    Settings,
    /// A column of the seven-day chart, by index.
    Day(u8),
}

pub struct Theme {
    pub surface: u32,
    pub card: u32,
    pub hover: u32,
    pub border: u32,
    pub text: u32,
    pub muted: u32,
    pub track: u32,
    pub accent: u32,
}

impl Theme {
    pub fn new(dark: bool) -> Self {
        if dark {
            Self {
                surface: 0xFF1E_2125,
                card: 0xFF25_292E,
                hover: 0xFF32_373E,
                border: 0xFF48_515B,
                text: 0xFFF6_F7F9,
                muted: 0xFFCD_D1D7,
                track: 0xFF35_3A41,
                accent: 0xFF69_C0FF,
            }
        } else {
            Self {
                surface: 0xFFF8_F9FB,
                card: 0xFFFF_FFFF,
                hover: 0xFFEC_EFF3,
                border: 0xFFB6_BEC8,
                text: 0xFF17_1C23,
                muted: 0xFF45_515D,
                track: 0xFFD5_DBE2,
                accent: 0xFF00_67C0,
            }
        }
    }
}

pub struct Layout {
    pub height: f32,
    pub hits: Vec<(Hit, Rect)>,
}

struct Fonts {
    title: Font,
    small: Font,
    small_bold: Font,
    heading: Font,
    detail: Font,
    count: Font,
    tiny: Font,
    glyph: Option<Font>,
    badge_glyph: Option<Font>,
}

impl Fonts {
    fn new(scale: f32) -> Option<Self> {
        let ui = |size: f32, bold: bool| Font::new(&UI_FONTS, size * scale, bold);
        Some(Self {
            title: ui(18.0, true)?,
            small: ui(12.0, false)?,
            small_bold: ui(12.0, true)?,
            heading: ui(16.0, true)?,
            detail: ui(11.5, false)?,
            count: ui(13.0, false)?,
            tiny: ui(10.0, false)?,
            glyph: Font::new(&ICON_FONTS, 15.0 * scale, false),
            badge_glyph: Font::new(&ICON_FONTS, 17.0 * scale, false),
        })
    }
}

/// Lays out the popup at `width` device pixels. When `paint` is false only
/// the height and hit targets are computed. `spinner_degrees` is the current
/// rotation of the loading circle shown while the view is refreshing.
pub fn render(
    g: &Graphics,
    view: &PopupView,
    scale: f32,
    width: f32,
    hover: Option<Hit>,
    spinner_degrees: f32,
    paint: bool,
) -> Layout {
    let theme = &Theme::new(view.dark);
    let px = |value: f32| (value * scale).round();
    let Some(fonts) = Fonts::new(scale) else {
        return Layout {
            height: px(FALLBACK_HEIGHT),
            hits: Vec::new(),
        };
    };
    let hairline = px(1.0).max(1.0);
    let left = px(12.0);
    let inner = width - 2.0 * left;
    let mut hits = Vec::new();
    if paint {
        g.clear(theme.surface);
    }

    // Header: provider title, last update, refresh button.
    let top = px(10.0);
    let header_height = px(32.0);
    let title = if view.can_switch {
        format!("{} \u{2194}", view.provider)
    } else {
        format!("{} Usage", view.provider)
    };
    let title_x = left + px(6.0);
    let title_width = g.measure(&title, &fonts.title).0;
    if view.can_switch {
        let target = Rect::new(title_x - px(4.0), top, title_width + px(8.0), header_height);
        hits.push((Hit::Provider, target));
    }
    // Header actions, left to right: history, refresh, settings.
    let button_at = |slots_from_right: f32| {
        let x = left + inner - px(40.0) - slots_from_right * px(43.0);
        Rect::new(x, top + px(2.0), px(35.0), px(28.0))
    };
    let history_button = button_at(2.0);
    let button = button_at(1.0);
    let settings_button = button_at(0.0);
    hits.push((Hit::History, history_button));
    if !view.refreshing {
        hits.push((Hit::Refresh, button));
    }
    hits.push((Hit::Settings, settings_button));
    if paint {
        let frame = |target: Rect, hit: Hit| {
            let fill = if hover == Some(hit) {
                theme.hover
            } else {
                theme.card
            };
            g.fill_round_rect(fill, target, px(6.0));
            g.stroke_round_rect(theme.border, target, px(6.0));
        };
        frame(history_button, Hit::History);
        // Three rising bars, drawn rather than taken from the icon font so
        // they can carry the accent color at any scale.
        let icon_left = history_button.x + (history_button.w - 16.0 * scale) / 2.0;
        let icon_top = history_button.y + (history_button.h - 16.0 * scale) / 2.0;
        for (x, bar_top) in [(3.0, 11.0), (8.0, 7.0), (13.0, 1.0)] {
            let x = icon_left + x * scale;
            g.line(
                theme.accent,
                (x, icon_top + bar_top * scale),
                (x, icon_top + 15.0 * scale),
                (2.2 * scale).max(2.0),
            );
        }
        frame(settings_button, Hit::Settings);
        if let Some(glyph) = &fonts.glyph {
            g.text(
                GLYPH_SETTINGS,
                glyph,
                theme.text,
                settings_button,
                Align::Center,
                Align::Center,
            );
        }

        let title_color = if hover == Some(Hit::Provider) {
            theme.accent
        } else {
            theme.text
        };
        let title_rect = Rect::new(title_x, top, 0.0, header_height);
        g.text(
            &title,
            &fonts.title,
            title_color,
            title_rect,
            Align::Near,
            Align::Center,
        );
        if let Some(updated) = &view.updated {
            let rect = Rect::new(title_x + title_width + px(12.0), top, 0.0, header_height);
            g.text(
                updated,
                &fonts.small,
                theme.muted,
                rect,
                Align::Near,
                Align::Center,
            );
        }
        if view.refreshing {
            // A loading circle takes the button's place while a refresh runs.
            let diameter = px(SPINNER_DIAMETER);
            let ring = Rect::new(
                button.x + (button.w - diameter) / 2.0,
                button.y + (button.h - diameter) / 2.0,
                diameter,
                diameter,
            );
            let stroke = px(2.0).max(1.5);
            g.arc(theme.track, ring, 0.0, 360.0, stroke);
            g.arc(
                theme.accent,
                ring,
                spinner_degrees - 90.0,
                SPINNER_SWEEP,
                stroke,
            );
        } else {
            frame(button, Hit::Refresh);
            if let Some(glyph) = &fonts.glyph {
                g.text(
                    GLYPH_REFRESH,
                    glyph,
                    theme.text,
                    button,
                    Align::Center,
                    Align::Center,
                );
            }
        }
    }
    let mut y = top + header_height;

    // Usage observed on each of the last seven days.
    if !view.daily.is_empty() {
        y += px(9.0);
        let heading_height = px(17.0);
        let chart_height = px(48.0);
        let detail_height = px(17.0);
        let card_height =
            px(8.0) + heading_height + px(6.0) + chart_height + px(5.0) + detail_height + px(8.0);
        let x = left + px(10.0);
        let content = inner - px(20.0);
        let chart_top = y + px(8.0) + heading_height + px(6.0);
        let column = content / view.daily.len() as f32;
        for index in 0..view.daily.len() {
            // The whole column is the hover target, so empty days respond too.
            let target = Rect::new(x + column * index as f32, chart_top, column, chart_height);
            hits.push((Hit::Day(index as u8), target));
        }
        if paint {
            let card = Rect::new(left, y, inner, card_height);
            g.fill_round_rect(theme.card, card, px(4.0));
            g.stroke_round_rect(theme.border, card, px(4.0));
            let heading = Rect::new(x, y + px(8.0), content, heading_height);
            g.text(
                "Usage history",
                &fonts.small_bold,
                theme.text,
                heading,
                Align::Near,
                Align::Center,
            );
            g.text(
                "Last 7 days",
                &fonts.detail,
                theme.muted,
                heading,
                Align::Far,
                Align::Center,
            );

            let label_height = px(16.0);
            let baseline = chart_top + chart_height - label_height;
            let tallest = view
                .daily
                .iter()
                .filter_map(|day| day.used)
                .fold(1.0_f64, f64::max);
            let bar_width = px(20.0).min(column - px(10.0)).max(1.0);
            for (index, day) in view.daily.iter().enumerate() {
                let column_left = x + column * index as f32;
                let hovered = hover == Some(Hit::Day(index as u8));
                let (height, color) = match day.used {
                    None => (hairline, theme.border),
                    Some(used) => {
                        let height = if used <= 0.0 {
                            px(2.0)
                        } else {
                            px((used * 28.0 / tallest).clamp(4.0, 28.0) as f32)
                        };
                        (height, if hovered { DAY_BAR_HOVER } else { DAY_BAR })
                    }
                };
                let bar = Rect::new(
                    (column_left + (column - bar_width) / 2.0).round(),
                    baseline - height,
                    bar_width,
                    height,
                );
                g.fill_rect(color, bar);
                let label = Rect::new(column_left, baseline, column, label_height);
                g.text(
                    &day.label,
                    &fonts.tiny,
                    theme.muted,
                    label,
                    Align::Center,
                    Align::Far,
                );
            }

            let detail = match hover {
                Some(Hit::Day(index)) => view.daily.get(usize::from(index)),
                _ => None,
            }
            .map_or("Hover over a day for details", |day| day.detail.as_str());
            let detail_rect = Rect::new(x, chart_top + chart_height + px(5.0), 0.0, detail_height);
            g.text(
                detail,
                &fonts.detail,
                theme.muted,
                detail_rect,
                Align::Near,
                Align::Center,
            );
        }
        y += card_height;
    }

    // One card per limit window.
    let name_height = px(34.0);
    let bar_height = px(19.0);
    let card_height = px(8.0) + name_height + px(8.0) + bar_height + px(8.0);
    for row in &view.rows {
        y += px(9.0);
        if paint {
            let card = Rect::new(left, y, inner, card_height);
            g.fill_round_rect(theme.card, card, px(4.0));
            g.stroke_round_rect(theme.border, card, px(4.0));

            let x = left + px(10.0);
            let content = inner - px(20.0);
            let row_top = y + px(8.0);
            let (badge_fill, badge_ink) = if row.weekly {
                WEEKLY_BADGE
            } else {
                FIVE_HOUR_BADGE
            };
            let badge = Rect::new(x, row_top + px(2.0), px(30.0), px(30.0));
            g.fill_round_rect(badge_fill, badge, px(6.0));
            if let Some(glyph) = &fonts.badge_glyph {
                let symbol = if row.weekly {
                    GLYPH_CALENDAR
                } else {
                    GLYPH_LIGHTNING
                };
                g.text(
                    symbol,
                    glyph,
                    badge_ink,
                    badge,
                    Align::Center,
                    Align::Center,
                );
            }
            let name = if row.weekly { "Weekly" } else { "5-hour" };
            let name_rect = Rect::new(x + px(41.0), row_top, 0.0, name_height);
            g.text(
                name,
                &fonts.heading,
                theme.text,
                name_rect,
                Align::Near,
                Align::Center,
            );

            // The name takes a third of the row; details take the rest.
            let separator_x = x + px(41.0) + ((content - px(41.0)) / 3.0).round();
            let separator = Rect::new(separator_x, row_top + px(1.0), hairline, px(32.0));
            g.fill_rect(theme.border, separator);
            let lines: Vec<&str> = [row.reset.as_str(), row.forecast.as_str()]
                .into_iter()
                .filter(|line| !line.is_empty())
                .collect();
            let line_height = px(16.0);
            let mut line_y = row_top + (name_height - line_height * lines.len() as f32) / 2.0;
            for line in lines {
                let rect = Rect::new(separator_x + px(13.0), line_y.round(), 0.0, line_height);
                g.text(
                    line,
                    &fonts.detail,
                    theme.muted,
                    rect,
                    Align::Near,
                    Align::Center,
                );
                line_y += line_height;
            }

            let percent = row.remaining.clamp(0, 100);
            let radius = px(6.0);
            let track = Rect::new(x, row_top + name_height + px(8.0), content, bar_height);
            g.fill_round_rect(theme.track, track, radius);
            let filled = if percent > 0 {
                (content * percent as f32 / 100.0).round().max(radius * 2.0)
            } else {
                0.0
            };
            if percent > 0 {
                g.fill_round_rect(
                    row.fill,
                    Rect::new(track.x, track.y, filled, bar_height),
                    radius,
                );
            }
            let label = format!("{percent}%");
            if percent >= MIN_PERCENT_FOR_INSIDE_LABEL {
                let rect = Rect::new(track.x, track.y, filled - px(5.0), bar_height);
                let ink = readable_ink(row.fill);
                g.text(
                    &label,
                    &fonts.small_bold,
                    ink,
                    rect,
                    Align::Far,
                    Align::Center,
                );
            } else {
                let rect = Rect::new(track.x + filled + px(5.0), track.y, 0.0, bar_height);
                g.text(
                    &label,
                    &fonts.small_bold,
                    theme.text,
                    rect,
                    Align::Near,
                    Align::Center,
                );
            }
        }
        y += card_height;
    }

    if let Some(status) = &view.status {
        y += px(10.0);
        let status_width = inner - px(4.0);
        let height = g.measure_wrapped(status, &fonts.small, status_width);
        if paint {
            let rect = Rect::new(left + px(2.0), y, status_width, 0.0);
            g.text_wrapped(status, &fonts.small, theme.muted, rect);
        }
        y += height;
    }

    if view.reset_count > 0 {
        y += px(12.0);
        if paint {
            g.fill_rect(theme.border, Rect::new(left, y, inner, hairline));
        }
        y += hairline + px(10.0);

        let heading_height = px(24.0);
        if paint {
            let heading = "Limit resets";
            let heading_x = left + px(4.0);
            let heading_width = g.measure(heading, &fonts.heading).0;
            let rect = Rect::new(heading_x, y, 0.0, heading_height);
            g.text(
                heading,
                &fonts.heading,
                theme.text,
                rect,
                Align::Near,
                Align::Center,
            );
            let count = format!("({} available)", view.reset_count);
            let rect = Rect::new(heading_x + heading_width + px(8.0), y, 0.0, heading_height);
            g.text(
                &count,
                &fonts.count,
                theme.muted,
                rect,
                Align::Near,
                Align::Center,
            );
        }
        y += heading_height;

        if !view.resets.is_empty() {
            y += px(4.0);
            let row_height = px(25.0);
            let count = view.resets.len() as f32;
            let list_height = row_height * count + hairline * (count + 1.0);
            if paint {
                let list = Rect::new(left, y, inner, list_height);
                g.fill_round_rect(theme.card, list, px(6.0));
                g.stroke_round_rect(theme.border, list, px(6.0));
                let mut row_y = y + hairline;
                for (index, reset) in view.resets.iter().enumerate() {
                    if index > 0 {
                        g.fill_rect(
                            theme.border,
                            Rect::new(left, row_y - hairline, inner, hairline),
                        );
                    }
                    let title_rect = Rect::new(left + px(13.0), row_y, 0.0, row_height);
                    g.text(
                        &reset.title,
                        &fonts.small,
                        theme.text,
                        title_rect,
                        Align::Near,
                        Align::Center,
                    );
                    let half = inner / 2.0;
                    let expiration_rect =
                        Rect::new(left + half, row_y, half - px(13.0), row_height);
                    g.text(
                        &reset.expiration,
                        &fonts.small,
                        theme.muted,
                        expiration_rect,
                        Align::Far,
                        Align::Center,
                    );
                    row_y += row_height + hairline;
                }
            }
            y += list_height;
        }
    }

    Layout {
        height: y + px(12.0),
        hits,
    }
}

/// Black or white, whichever reads better on `fill`.
fn readable_ink(fill: u32) -> u32 {
    let (red, green, blue) = ((fill >> 16) & 0xFF, (fill >> 8) & 0xFF, fill & 0xFF);
    if red * 299 + green * 587 + blue * 114 >= 150_000 {
        0xFF00_0000
    } else {
        0xFFFF_FFFF
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfx::tests::{sample_path, Canvas};
    use crate::gfx::GdiPlus;
    use crate::view::tests::sample;

    #[test]
    fn bar_labels_contrast_with_their_fill() {
        assert_eq!(readable_ink(0xFF00_78D7), 0xFFFF_FFFF);
        assert_eq!(readable_ink(0xFFF0_D200), 0xFF00_0000);
        assert_eq!(readable_ink(0xFFDC_2828), 0xFFFF_FFFF);
    }

    #[test]
    fn measuring_and_painting_agree_on_layout() {
        let _gdiplus = GdiPlus::start().unwrap();
        let canvas = Canvas::new(520, 600);
        let g = canvas.graphics();
        let view = sample(true);
        let measured = render(&g, &view, 1.0, 520.0, None, 0.0, false);
        let painted = render(&g, &view, 1.0, 520.0, Some(Hit::Refresh), 0.0, true);
        assert_eq!(measured.height, painted.height);
        assert_eq!(measured.hits, painted.hits);
        // The switchable title and the refresh button are both clickable.
        let targets: Vec<Hit> = painted.hits.iter().map(|(hit, _)| *hit).collect();
        let days = (0..7).map(Hit::Day);
        let expected: Vec<Hit> = [Hit::Provider, Hit::History, Hit::Refresh, Hit::Settings]
            .into_iter()
            .chain(days)
            .collect();
        assert_eq!(targets, expected);
        // No two targets overlap, so a click is never ambiguous.
        for (index, (_, first)) in painted.hits.iter().enumerate() {
            for (_, second) in &painted.hits[index + 1..] {
                let apart = first.x + first.w <= second.x
                    || second.x + second.w <= first.x
                    || first.y + first.h <= second.y
                    || second.y + second.h <= first.y;
                assert!(apart, "{first:?} overlaps {second:?}");
            }
        }

        // Removing content makes the popup shorter.
        let mut bare = sample(true);
        bare.rows.pop();
        bare.status = None;
        bare.reset_count = 0;
        assert!(render(&g, &bare, 1.0, 520.0, None, 0.0, false).height < measured.height);
    }

    #[test]
    fn refreshing_replaces_the_button_without_moving_the_layout() {
        let _gdiplus = GdiPlus::start().unwrap();
        let canvas = Canvas::new(520, 600);
        let g = canvas.graphics();
        let idle = sample(true);
        let refreshing = PopupView {
            refreshing: true,
            ..sample(true)
        };
        let idle_layout = render(&g, &idle, 1.0, 520.0, None, 0.0, true);
        let refreshing_layout = render(&g, &refreshing, 1.0, 520.0, None, 135.0, true);
        // The loading circle is not clickable, and nothing else shifts.
        let targets: Vec<Hit> = refreshing_layout.hits.iter().map(|(hit, _)| *hit).collect();
        assert!(!targets.contains(&Hit::Refresh));
        assert_eq!(targets.len(), idle_layout.hits.len() - 1);
        assert_eq!(refreshing_layout.height, idle_layout.height);
    }

    /// Writes the popup in both themes, at 150% scale and while refreshing,
    /// for visual review. Run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "writes sample images to target/samples"]
    fn write_popup_samples() {
        let _gdiplus = GdiPlus::start().unwrap();
        for (name, dark, scale, refreshing) in [
            ("dark", true, 1.0, false),
            ("light", false, 1.0, false),
            ("dark-150", true, 1.5, false),
            ("dark-refreshing", true, 1.0, true),
            ("light-refreshing", false, 1.0, true),
        ] {
            let view = PopupView {
                refreshing,
                ..sample(dark)
            };
            let width = (520.0_f32 * scale).round();
            let height = {
                let probe = Canvas::new(1, 1);
                render(&probe.graphics(), &view, scale, width, None, 0.0, false).height
            };
            let canvas = Canvas::new(width as i32, height.ceil() as i32);
            let hover = Some(Hit::Refresh);
            render(&canvas.graphics(), &view, scale, width, hover, 135.0, true);
            canvas.save_png(&sample_path(&format!("popup-{name}.png")));
        }
    }
}
