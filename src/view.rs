//! The popup's content, prepared by the tray process and sent to the popup
//! process as JSON. All text is already formatted, so the popup only draws.

use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub weekly: bool,
    pub remaining: i32,
    /// ARGB color of the filled part of the bar.
    pub fill: u32,
    pub reset: String,
    pub forecast: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResetRow {
    pub title: String,
    pub expiration: String,
}

/// One column of the seven-day usage chart.
#[derive(Clone, Debug, PartialEq)]
pub struct DailyBar {
    /// Short weekday name shown under the bar.
    pub label: String,
    /// Percentage of the weekly allowance used that day; `None` when no
    /// reliable interval was observed.
    pub used: Option<f64>,
    /// Shown under the chart while the day is hovered.
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PopupView {
    pub dark: bool,
    pub provider: String,
    pub can_switch: bool,
    pub updated: Option<String>,
    pub refreshing: bool,
    /// Oldest day first; empty hides the chart.
    pub daily: Vec<DailyBar>,
    pub rows: Vec<Row>,
    pub status: Option<String>,
    pub reset_count: usize,
    pub resets: Vec<ResetRow>,
}

impl PopupView {
    pub fn to_json(&self) -> String {
        json!({
            "dark": self.dark,
            "provider": self.provider,
            "canSwitch": self.can_switch,
            "updated": self.updated,
            "refreshing": self.refreshing,
            "daily": self.daily.iter().map(|day| json!({
                "label": day.label,
                "used": day.used,
                "detail": day.detail,
            })).collect::<Vec<_>>(),
            "rows": self.rows.iter().map(|row| json!({
                "weekly": row.weekly,
                "remaining": row.remaining,
                "fill": row.fill,
                "reset": row.reset,
                "forecast": row.forecast,
            })).collect::<Vec<_>>(),
            "status": self.status,
            "resetCount": self.reset_count,
            "resets": self.resets.iter().map(|reset| json!({
                "title": reset.title,
                "expiration": reset.expiration,
            })).collect::<Vec<_>>(),
        })
        .to_string()
    }

    pub fn from_json(bytes: &[u8]) -> Option<Self> {
        let root: Value = serde_json::from_slice(bytes).ok()?;
        let text = |value: &Value, name: &str| Some(value.get(name)?.as_str()?.to_string());
        let optional_text = |name: &str| root.get(name).and_then(Value::as_str).map(str::to_string);
        let list = |name: &str| root.get(name).and_then(Value::as_array);
        Some(Self {
            dark: root.get("dark")?.as_bool()?,
            provider: text(&root, "provider")?,
            can_switch: root.get("canSwitch")?.as_bool()?,
            updated: optional_text("updated"),
            refreshing: root.get("refreshing")?.as_bool()?,
            daily: list("daily")?
                .iter()
                .map(|day| {
                    Some(DailyBar {
                        label: text(day, "label")?,
                        used: match day.get("used")? {
                            Value::Null => None,
                            used => Some(used.as_f64()?),
                        },
                        detail: text(day, "detail")?,
                    })
                })
                .collect::<Option<_>>()?,
            rows: list("rows")?
                .iter()
                .map(|row| {
                    Some(Row {
                        weekly: row.get("weekly")?.as_bool()?,
                        remaining: i32::try_from(row.get("remaining")?.as_i64()?).ok()?,
                        fill: u32::try_from(row.get("fill")?.as_u64()?).ok()?,
                        reset: text(row, "reset")?,
                        forecast: text(row, "forecast")?,
                    })
                })
                .collect::<Option<_>>()?,
            status: optional_text("status"),
            reset_count: usize::try_from(root.get("resetCount")?.as_u64()?).ok()?,
            resets: list("resets")?
                .iter()
                .map(|reset| {
                    Some(ResetRow {
                        title: text(reset, "title")?,
                        expiration: text(reset, "expiration")?,
                    })
                })
                .collect::<Option<_>>()?,
        })
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn sample(dark: bool) -> PopupView {
        let day = |label: &str, used: Option<f64>| DailyBar {
            label: label.to_string(),
            used,
            detail: match used {
                Some(used) => format!("{label}, Nov 3 \u{b7} {used:.0}% used"),
                None => format!("{label}, Nov 3 \u{b7} Usage unavailable"),
            },
        };
        PopupView {
            dark,
            provider: "Codex".to_string(),
            can_switch: true,
            updated: Some("Updated 8:38 PM".to_string()),
            refreshing: false,
            daily: vec![
                day("Tue", Some(3.0)),
                day("Wed", Some(11.0)),
                day("Thu", Some(11.5)),
                day("Fri", None),
                day("Sat", Some(0.0)),
                day("Sun", Some(12.0)),
                day("Mon", Some(7.0)),
            ],
            rows: vec![
                Row {
                    weekly: true,
                    remaining: 72,
                    fill: 0xFF00_78D7,
                    reset: "Next reset: Nov 5, 8:38 PM (2d 0h)".to_string(),
                    forecast: "Will last until reset".to_string(),
                },
                Row {
                    weekly: false,
                    remaining: 8,
                    fill: 0xFFF0_D200,
                    reset: "Next reset: Nov 3, 10:38 PM (2h 0m)".to_string(),
                    forecast: "Estimated: Nov 3, 9:10 PM (32m)".to_string(),
                },
            ],
            status: Some("Automatic refresh paused while Codex is not running.".to_string()),
            reset_count: 2,
            resets: vec![
                ResetRow {
                    title: "1. Full reset".to_string(),
                    expiration: "Expires Nov 4, 3:38 PM".to_string(),
                },
                ResetRow {
                    title: "2. Weekly reset".to_string(),
                    expiration: "Expiration unavailable".to_string(),
                },
            ],
        }
    }

    #[test]
    fn views_survive_the_trip_between_processes() {
        let view = sample(true);
        assert_eq!(PopupView::from_json(view.to_json().as_bytes()), Some(view));

        let bare = PopupView {
            updated: None,
            status: None,
            daily: Vec::new(),
            rows: Vec::new(),
            ..sample(false)
        };
        assert_eq!(PopupView::from_json(bare.to_json().as_bytes()), Some(bare));
    }

    #[test]
    fn malformed_views_are_rejected() {
        assert_eq!(PopupView::from_json(b"{}"), None);
        assert_eq!(PopupView::from_json(b"not json"), None);
        let wrong_type = sample(true)
            .to_json()
            .replace("\"remaining\":72", "\"remaining\":\"72\"");
        assert_eq!(PopupView::from_json(wrong_type.as_bytes()), None);
        let wrong_day = sample(true)
            .to_json()
            .replace("\"used\":11.5", "\"used\":\"lots\"");
        assert_eq!(PopupView::from_json(wrong_day.as_bytes()), None);
    }
}
