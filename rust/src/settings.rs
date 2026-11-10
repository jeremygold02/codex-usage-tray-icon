//! The app's settings: the same properties, defaults, limits, schema
//! migrations and `settings.json` format as the C# app's `AppSettings`, so
//! either app can read a file the other wrote.
//!
//! The C# app serializes with `JavaScriptSerializer`. Reading follows its
//! conversion rules (property names match without regard to case, numbers and
//! flags may be quoted, a value of the wrong kind makes the whole file
//! unusable) and writing reproduces its output byte for byte, including the
//! `"\/Date(ms)\/"` form for dates.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};
use windows_sys::w;
use windows_sys::Win32::Storage::FileSystem::{
    ReplaceFileW, FILE_FLAG_WRITE_THROUGH, REPLACEFILE_IGNORE_MERGE_ERRORS,
    REPLACEFILE_WRITE_THROUGH,
};
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

use crate::paths;
use crate::timefmt::parse_rfc3339;

/// The settings schema this build reads and writes.
pub const CURRENT_SETTINGS_VERSION: i32 = 9;
/// Tray box color used when the saved one is missing or unreadable.
pub const DEFAULT_TRAY_BOX_COLOR: u32 = 0xFF00_78D7;
/// Tray text color used when the saved one is missing or unreadable.
pub const DEFAULT_TRAY_TEXT_COLOR: u32 = 0xFFFF_FFFF;

const DEFAULT_REFRESH_SECONDS: i32 = 300;
const CRITICAL_COLOR: u32 = 0xFFDC_2828;
const LOW_COLOR: u32 = 0xFFF0_D200;
const BACKUP_SUFFIX: &str = ".bak";

/// Which limit the tray icon shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconMetric {
    Weekly,
    FiveHour,
    Auto,
}

impl IconMetric {
    /// The value stored in `settings.json`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Weekly => "Weekly",
            Self::FiveHour => "FiveHour",
            Self::Auto => "Auto",
        }
    }

    fn from_name(name: &str) -> Self {
        if name.eq_ignore_ascii_case(Self::Auto.name()) {
            Self::Auto
        } else if name.eq_ignore_ascii_case(Self::FiveHour.name()) {
            Self::FiveHour
        } else {
            Self::Weekly
        }
    }
}

/// The color scheme the user chose for the app's windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeChoice {
    System,
    Dark,
    Light,
}

impl ThemeChoice {
    /// The value stored in `settings.json`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::System => "System Default",
            Self::Dark => "Dark Mode",
            Self::Light => "Light Mode",
        }
    }

    fn from_name(name: &str) -> Self {
        if name.eq_ignore_ascii_case(Self::Dark.name()) {
            Self::Dark
        } else if name.eq_ignore_ascii_case(Self::Light.name()) {
            Self::Light
        } else {
            Self::System
        }
    }

    /// Whether this choice means a dark window, given the Windows app theme.
    pub const fn is_dark(self, system_dark: bool) -> bool {
        match self {
            Self::Dark => true,
            Self::Light => false,
            Self::System => system_dark,
        }
    }

    /// Whether this choice means a dark window on this machine right now.
    pub fn resolve_dark(self) -> bool {
        self.is_dark(system_uses_dark_theme())
    }
}

/// How future usage is estimated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForecastMethod {
    LearnedRoutine,
    RecentPace,
}

impl ForecastMethod {
    /// The value stored in `settings.json`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::LearnedRoutine => "LearnedRoutine",
            Self::RecentPace => "RecentPace",
        }
    }
}

/// Whether weekly forecasts use the learned quiet-hour profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuietHoursMode {
    Automatic,
    Off,
}

impl QuietHoursMode {
    /// The value stored in `settings.json`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Automatic => "Automatic",
            Self::Off => "Off",
        }
    }
}

/// Everything the app persists in `settings.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// The schema the values follow; `CURRENT_SETTINGS_VERSION` unless they
    /// came from a newer app version's file.
    pub settings_version: i32,
    /// Kept for file compatibility; the percentage is always drawn.
    pub overlay_number: bool,
    /// Remaining percentage at or below which usage counts as critical, 1 to 99.
    pub critical_threshold: i32,
    /// Remaining percentage at or below which usage counts as low, 1 to 99
    /// and never under `critical_threshold`.
    pub low_threshold: i32,
    /// Which limit the tray icon shows.
    pub icon_metric: IconMetric,
    /// Whether popup bars are colored by the thresholds.
    pub color_bars: bool,
    /// Whether the popup shows reset times. `ShowPopupResetTimes` in the file.
    pub show_reset_times: bool,
    /// Whether the popup shows when usage was last refreshed.
    /// `ShowPopupLastUpdated` in the file.
    pub show_last_updated: bool,
    /// Whether limits beyond weekly and 5-hour are shown.
    pub show_additional_limits: bool,
    /// Whether to remind before a reset credit expires.
    pub reset_expiry_reminders: bool,
    /// How long before a credit expires the reminder is due, 1 to 72 hours.
    pub reset_expiry_lead_hours: i32,
    /// The stored wish to start with Windows; the tray applies it.
    pub start_with_windows: bool,
    /// Whether reaching the low or critical threshold raises an alert.
    pub threshold_notifications: bool,
    /// Whether an expiring reset credit is used automatically.
    pub auto_redeem_reset_credits: bool,
    /// How long before expiry automatic use may begin, 1 to 120 minutes.
    pub auto_redeem_lead_minutes: i32,
    /// Refresh interval while Codex is running, 30 to 3600 seconds.
    pub refresh_seconds: u64,
    /// Refresh interval while no Codex process is running; zero pauses checks.
    pub idle_refresh_seconds: u64,
    /// Whether the tray icon has a filled box behind the percentage.
    pub show_tray_box: bool,
    /// A fixed tray box color; `None` colors the box by remaining percentage.
    pub tray_box_color: Option<u32>,
    /// The `TrayBoxColor` kept in the file while `tray_box_color` is `None`,
    /// so the color the user last picked survives switching to automatic.
    pub saved_tray_box_color: u32,
    /// The color of the percentage in the tray icon, as opaque ARGB.
    pub tray_text_color: u32,
    /// The color scheme of the app's windows.
    pub theme: ThemeChoice,
    /// How future usage is estimated.
    pub forecast_method: ForecastMethod,
    /// Whether weekly forecasts use the learned quiet-hour profile.
    pub quiet_hours_mode: QuietHoursMode,
    /// Whether usage graphs draw the projected path.
    pub show_projected_usage: bool,
    /// Whether usage graphs highlight projected quiet hours.
    pub highlight_quiet_hours: bool,
    /// The reset credit of the automatic redemption in progress or last made.
    pub auto_redeem_credit_id: Option<String>,
    /// The key that keeps that redemption from being made twice.
    pub auto_redeem_idempotency_key: Option<String>,
    /// How that redemption attempt ended, as recorded by the tray.
    pub auto_redeem_attempt_status: Option<String>,
    /// When the last automatic redemption was attempted, in milliseconds
    /// since the Unix epoch (UTC). `AutoRedeemLastAttemptUtc` in the file.
    pub auto_redeem_last_attempt_ms: Option<i64>,
    /// Set when the values came from a file written by a newer app version.
    newer_file: Option<NewerFile>,
}

/// A settings file with a schema this build must not overwrite.
#[derive(Clone, Debug, PartialEq, Eq)]
struct NewerFile {
    version: i32,
    path: PathBuf,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            settings_version: CURRENT_SETTINGS_VERSION,
            overlay_number: true,
            critical_threshold: 15,
            low_threshold: 25,
            icon_metric: IconMetric::Weekly,
            color_bars: true,
            show_reset_times: true,
            show_last_updated: true,
            show_additional_limits: true,
            reset_expiry_reminders: true,
            reset_expiry_lead_hours: 24,
            start_with_windows: false,
            threshold_notifications: false,
            auto_redeem_reset_credits: false,
            auto_redeem_lead_minutes: 5,
            refresh_seconds: u64::from(DEFAULT_REFRESH_SECONDS.unsigned_abs()),
            idle_refresh_seconds: 0,
            show_tray_box: true,
            tray_box_color: None,
            saved_tray_box_color: DEFAULT_TRAY_BOX_COLOR,
            tray_text_color: DEFAULT_TRAY_TEXT_COLOR,
            theme: ThemeChoice::System,
            forecast_method: ForecastMethod::LearnedRoutine,
            quiet_hours_mode: QuietHoursMode::Automatic,
            show_projected_usage: true,
            highlight_quiet_hours: false,
            auto_redeem_credit_id: None,
            auto_redeem_idempotency_key: None,
            auto_redeem_attempt_status: None,
            auto_redeem_last_attempt_ms: None,
            newer_file: None,
        }
    }
}

/// Why settings could not be saved.
#[derive(Debug)]
pub enum SaveError {
    /// The file, or its backup, was written by a newer app version.
    NewerVersion {
        version: i32,
        path: PathBuf,
    },
    /// The settings path has no parent folder to create the file in.
    NoFolder,
    Io(io::Error),
}

impl fmt::Display for SaveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NewerVersion { version, path } => {
                formatter.write_str(&newer_version_message(*version, path))
            }
            Self::NoFolder => formatter.write_str("Could not determine the settings folder."),
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl From<io::Error> for SaveError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl Settings {
    /// Loads this build's settings file, falling back to its backup and then
    /// to defaults.
    pub fn load() -> Self {
        default_path()
            .map(|path| Self::load_from(&path))
            .unwrap_or_default()
    }

    /// Loads `path`, falling back to `path.bak` when the file is missing or
    /// unreadable, and to defaults when the backup is too.
    pub fn load_from(path: &Path) -> Self {
        read_file(path)
            .or_else(|| read_file(&backup_path(path)))
            .unwrap_or_default()
    }

    /// Saves to this build's settings file.
    pub fn save(&mut self) -> Result<(), SaveError> {
        let path = default_path().ok_or(SaveError::NoFolder)?;
        self.save_to(&path)
    }

    /// Writes the settings to `path` without ever leaving a partial file: the
    /// JSON goes to a temporary file that then replaces `path`, and the file
    /// it replaces is kept as `path.bak`. Refuses to overwrite a file written
    /// by a newer app version.
    pub fn save_to(&mut self, path: &Path) -> Result<(), SaveError> {
        self.ensure_save_does_not_overwrite_newer_settings(path)?;
        let directory = path
            .parent()
            .filter(|directory| !directory.as_os_str().is_empty())
            .ok_or(SaveError::NoFolder)?;
        fs::create_dir_all(directory)?;
        self.settings_version = CURRENT_SETTINGS_VERSION;
        let json = self.to_json();

        let temporary = temporary_path(path);
        let written = write_new_file(&temporary, json.as_bytes()).and_then(|()| {
            if path.exists() {
                replace_file(path, &temporary, &backup_path(path))
            } else {
                fs::rename(&temporary, path)
            }
        });
        if written.is_err() {
            // Nothing useful can be done if the leftover cannot be removed.
            let _ = fs::remove_file(&temporary);
        }
        written.map_err(SaveError::Io)
    }

    /// The explanation to show when saving is blocked because these values
    /// came from a newer app version's file, which this build will not
    /// overwrite.
    pub fn save_blocked_message(&self) -> Option<String> {
        self.newer_file
            .as_ref()
            .map(|newer| newer_version_message(newer.version, &newer.path))
    }

    /// Opaque ARGB fill for a remaining percentage.
    pub fn color_for_percent(&self, percent: i32) -> u32 {
        threshold_color(percent, self.critical_threshold, self.low_threshold)
    }

    /// The tray box fill for a remaining percentage: the fixed color when one
    /// is set, otherwise the color for that percentage.
    pub fn box_color_for_percent(&self, percent: i32) -> u32 {
        self.tray_box_color
            .unwrap_or_else(|| self.color_for_percent(percent))
    }

    /// The tray box color to offer when the user picks one: the fixed color,
    /// or the one remembered from before automatic coloring was chosen.
    pub fn picked_tray_box_color(&self) -> u32 {
        self.tray_box_color.unwrap_or(self.saved_tray_box_color)
    }

    /// Chooses between a fixed tray box `color` and automatic coloring, and
    /// remembers the color either way.
    pub fn set_tray_box_color(&mut self, fixed: bool, color: u32) {
        self.tray_box_color = fixed.then_some(color);
        self.saved_tray_box_color = color;
    }

    /// The file content, with the properties in the order and form the C#
    /// app writes them.
    pub fn to_json(&self) -> String {
        let mut json = JsonObject::default();
        json.number("SettingsVersion", i64::from(self.settings_version));
        json.flag("OverlayNumber", self.overlay_number);
        json.number("CriticalThreshold", i64::from(self.critical_threshold));
        json.number("LowThreshold", i64::from(self.low_threshold));
        json.text("IconMetric", Some(self.icon_metric.name()));
        json.flag("ColorBars", self.color_bars);
        json.flag("ShowPopupResetTimes", self.show_reset_times);
        json.flag("ShowPopupLastUpdated", self.show_last_updated);
        json.flag("ShowAdditionalLimits", self.show_additional_limits);
        json.flag("ResetExpiryReminders", self.reset_expiry_reminders);
        json.number(
            "ResetExpiryLeadHours",
            i64::from(self.reset_expiry_lead_hours),
        );
        json.flag("StartWithWindows", self.start_with_windows);
        json.flag("ThresholdNotifications", self.threshold_notifications);
        json.flag("AutoRedeemResetCredits", self.auto_redeem_reset_credits);
        json.number(
            "AutoRedeemLeadMinutes",
            i64::from(self.auto_redeem_lead_minutes),
        );
        json.number(
            "RefreshSeconds",
            i64::try_from(self.refresh_seconds).unwrap_or(i64::MAX),
        );
        json.number(
            "IdleRefreshSeconds",
            i64::try_from(self.idle_refresh_seconds).unwrap_or(i64::MAX),
        );
        json.flag("ShowTrayBox", self.show_tray_box);
        json.flag("UseCustomTrayBoxColor", self.tray_box_color.is_some());
        json.text(
            "TrayBoxColor",
            Some(&format_color(self.picked_tray_box_color())),
        );
        json.text("TrayTextColor", Some(&format_color(self.tray_text_color)));
        json.text("Theme", Some(self.theme.name()));
        json.text("ForecastMethod", Some(self.forecast_method.name()));
        json.text("QuietHoursMode", Some(self.quiet_hours_mode.name()));
        json.flag("ShowProjectedUsage", self.show_projected_usage);
        json.flag("HighlightQuietHours", self.highlight_quiet_hours);
        json.text("AutoRedeemCreditId", self.auto_redeem_credit_id.as_deref());
        json.text(
            "AutoRedeemIdempotencyKey",
            self.auto_redeem_idempotency_key.as_deref(),
        );
        json.text(
            "AutoRedeemAttemptStatus",
            self.auto_redeem_attempt_status.as_deref(),
        );
        json.date("AutoRedeemLastAttemptUtc", self.auto_redeem_last_attempt_ms);
        json.finish()
    }

    /// Reads every known property from a parsed file and applies the limits
    /// and the migrations for a file of schema `source_version`. Returns
    /// `None` when a value is of a kind the C# app could not convert, which
    /// makes the whole file unusable there too.
    fn from_json(root: &Map<String, Value>, source_version: i32) -> Option<Self> {
        let properties = Properties(root);
        let defaults = Self::default();

        let mut show_additional_limits =
            properties.flag("ShowAdditionalLimits", defaults.show_additional_limits)?;
        let mut reset_expiry_reminders =
            properties.flag("ResetExpiryReminders", defaults.reset_expiry_reminders)?;
        let mut reset_expiry_lead_hours =
            properties.integer("ResetExpiryLeadHours", defaults.reset_expiry_lead_hours)?;
        let mut auto_redeem_reset_credits =
            properties.flag("AutoRedeemResetCredits", defaults.auto_redeem_reset_credits)?;
        let mut auto_redeem_lead_minutes =
            properties.integer("AutoRedeemLeadMinutes", defaults.auto_redeem_lead_minutes)?;
        let mut auto_redeem_credit_id = properties.text("AutoRedeemCreditId")?;
        let mut auto_redeem_idempotency_key = properties.text("AutoRedeemIdempotencyKey")?;
        let mut auto_redeem_attempt_status = properties.text("AutoRedeemAttemptStatus")?;
        let mut auto_redeem_last_attempt_ms = properties.date("AutoRedeemLastAttemptUtc")?;
        let mut forecast_method = properties.text("ForecastMethod")?;
        let mut quiet_hours_mode = properties.text("QuietHoursMode")?;
        let mut show_projected_usage =
            properties.flag("ShowProjectedUsage", defaults.show_projected_usage)?;
        let mut highlight_quiet_hours =
            properties.flag("HighlightQuietHours", defaults.highlight_quiet_hours)?;
        // Read only so that an unusable value rejects the file as it does in
        // the C# app; the number is always shown and the version is set by
        // the caller.
        properties.flag("OverlayNumber", defaults.overlay_number)?;
        properties.integer("SettingsVersion", defaults.settings_version)?;

        // Settings introduced by a schema version start from their defaults
        // when the file predates it, whatever the file happens to contain.
        if source_version < 6 {
            show_additional_limits = true;
        }
        if source_version < 7 {
            auto_redeem_reset_credits = false;
            auto_redeem_lead_minutes = defaults.auto_redeem_lead_minutes;
            auto_redeem_credit_id = None;
            auto_redeem_idempotency_key = None;
            auto_redeem_attempt_status = None;
            auto_redeem_last_attempt_ms = None;
        }
        if source_version < 8 {
            reset_expiry_reminders = true;
            reset_expiry_lead_hours = defaults.reset_expiry_lead_hours;
        }
        if source_version < 9 {
            forecast_method = None;
            quiet_hours_mode = None;
            show_projected_usage = true;
            highlight_quiet_hours = false;
        }

        let named = |value: &Option<String>, name: &str| {
            value
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case(name))
        };
        if !(1..=72).contains(&reset_expiry_lead_hours) {
            reset_expiry_lead_hours = defaults.reset_expiry_lead_hours;
        }
        let threshold = |name: &str, default: i32| {
            properties.integer(name, default).map(|value| {
                if (1..=99).contains(&value) {
                    value
                } else {
                    default
                }
            })
        };
        let low_threshold = threshold("LowThreshold", defaults.low_threshold)?;
        let critical_threshold =
            threshold("CriticalThreshold", defaults.critical_threshold)?.min(low_threshold);
        let refresh_seconds = properties
            .integer("RefreshSeconds", DEFAULT_REFRESH_SECONDS)?
            .clamp(30, 3_600);
        let idle_refresh_seconds = match properties.integer("IdleRefreshSeconds", 0)? {
            seconds if seconds <= 0 => 0,
            seconds if seconds > 7_200 => 7_200,
            seconds => seconds.max(refresh_seconds),
        };
        if !(1..=120).contains(&auto_redeem_lead_minutes) {
            auto_redeem_lead_minutes = defaults.auto_redeem_lead_minutes;
        }
        // A redemption attempt is only meaningful with both identifiers.
        let blank =
            |value: &Option<String>| value.as_deref().is_none_or(|value| value.trim().is_empty());
        if blank(&auto_redeem_credit_id) || blank(&auto_redeem_idempotency_key) {
            auto_redeem_credit_id = None;
            auto_redeem_idempotency_key = None;
            auto_redeem_attempt_status = None;
            auto_redeem_last_attempt_ms = None;
        }
        let color = |name: &str, default: u32| {
            properties
                .text(name)
                .map(|value| value.as_deref().and_then(parse_color).unwrap_or(default))
        };
        let saved_tray_box_color = color("TrayBoxColor", DEFAULT_TRAY_BOX_COLOR)?;
        let use_custom_tray_box_color = properties.flag("UseCustomTrayBoxColor", false)?;

        Some(Self {
            settings_version: CURRENT_SETTINGS_VERSION,
            overlay_number: true,
            critical_threshold,
            low_threshold,
            icon_metric: properties
                .text("IconMetric")?
                .map_or(defaults.icon_metric, |name| IconMetric::from_name(&name)),
            color_bars: properties.flag("ColorBars", defaults.color_bars)?,
            show_reset_times: properties.flag("ShowPopupResetTimes", defaults.show_reset_times)?,
            show_last_updated: properties
                .flag("ShowPopupLastUpdated", defaults.show_last_updated)?,
            show_additional_limits,
            reset_expiry_reminders,
            reset_expiry_lead_hours,
            start_with_windows: properties.flag("StartWithWindows", defaults.start_with_windows)?,
            threshold_notifications: properties
                .flag("ThresholdNotifications", defaults.threshold_notifications)?,
            auto_redeem_reset_credits,
            auto_redeem_lead_minutes,
            refresh_seconds: u64::from(refresh_seconds.unsigned_abs()),
            idle_refresh_seconds: u64::from(idle_refresh_seconds.unsigned_abs()),
            show_tray_box: properties.flag("ShowTrayBox", defaults.show_tray_box)?,
            tray_box_color: use_custom_tray_box_color.then_some(saved_tray_box_color),
            saved_tray_box_color,
            tray_text_color: color("TrayTextColor", DEFAULT_TRAY_TEXT_COLOR)?,
            theme: properties
                .text("Theme")?
                .map_or(defaults.theme, |name| ThemeChoice::from_name(&name)),
            forecast_method: if named(&forecast_method, ForecastMethod::RecentPace.name()) {
                ForecastMethod::RecentPace
            } else {
                ForecastMethod::LearnedRoutine
            },
            quiet_hours_mode: if named(&quiet_hours_mode, QuietHoursMode::Off.name()) {
                QuietHoursMode::Off
            } else {
                QuietHoursMode::Automatic
            },
            show_projected_usage,
            highlight_quiet_hours,
            auto_redeem_credit_id,
            auto_redeem_idempotency_key,
            auto_redeem_attempt_status,
            auto_redeem_last_attempt_ms,
            newer_file: None,
        })
    }

    fn ensure_save_does_not_overwrite_newer_settings(&self, path: &Path) -> Result<(), SaveError> {
        if let Some(newer) = &self.newer_file {
            return Err(SaveError::NewerVersion {
                version: newer.version,
                path: newer.path.clone(),
            });
        }
        // The file may have been replaced by a newer version since it was read.
        for candidate in [path.to_path_buf(), backup_path(path)] {
            if let Some(version) = read_settings_version(&candidate) {
                if version > CURRENT_SETTINGS_VERSION {
                    return Err(SaveError::NewerVersion {
                        version,
                        path: candidate,
                    });
                }
            }
        }
        Ok(())
    }
}

/// Opaque ARGB fill for a remaining percentage under the given thresholds:
/// red at or below `critical`, yellow at or below `low`, otherwise blue.
pub fn threshold_color(percent: i32, critical: i32, low: i32) -> u32 {
    if percent <= critical {
        CRITICAL_COLOR
    } else if percent <= low {
        LOW_COLOR
    } else {
        DEFAULT_TRAY_BOX_COLOR
    }
}

/// This build's settings file, or `None` when `%APPDATA%` is unavailable.
pub fn default_path() -> Option<PathBuf> {
    paths::data_directory().map(|directory| directory.join(paths::SETTINGS_FILE))
}

/// Whether Windows is set to use the dark theme for apps.
pub fn system_uses_dark_theme() -> bool {
    let mut value = 1u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: value and size describe a four-byte buffer for a DWORD, and the
    // key and value names are static null-terminated strings.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        )
    };
    status == 0 && value == 0
}

/// Parses `#RRGGBB` or `#RGB`, the forms the settings window saves, into
/// opaque ARGB.
pub fn parse_color(value: &str) -> Option<u32> {
    let digits = value.trim().strip_prefix('#')?;
    if !digits.chars().all(|digit| digit.is_ascii_hexdigit()) {
        return None;
    }
    let rgb = match digits.len() {
        6 => u32::from_str_radix(digits, 16).ok()?,
        3 => {
            let short = u32::from_str_radix(digits, 16).ok()?;
            let (red, green, blue) = ((short >> 8) & 0xF, (short >> 4) & 0xF, short & 0xF);
            (red * 0x11) << 16 | (green * 0x11) << 8 | (blue * 0x11)
        }
        _ => return None,
    };
    Some(0xFF00_0000 | rgb)
}

/// Formats the color as `#RRGGBB`, ignoring its alpha.
pub fn format_color(argb: u32) -> String {
    format!("#{:06X}", argb & 0x00FF_FFFF)
}

fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(BACKUP_SUFFIX);
    PathBuf::from(name)
}

fn newer_version_message(version: i32, path: &Path) -> String {
    format!(
        "Settings were created by a newer app version (schema {version}). This version supports \
         schema {CURRENT_SETTINGS_VERSION} and will not overwrite the file: {}",
        path.display()
    )
}

/// Parses a settings file into its top-level object. A file that is missing,
/// unreadable or not a JSON object gives `None`.
fn read_object(path: &Path) -> Option<Map<String, Value>> {
    let bytes = fs::read(path).ok()?;
    // Editors may add a byte-order mark that the JSON parser does not accept.
    let json = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    match serde_json::from_slice::<Value>(json) {
        Ok(Value::Object(root)) => Some(root),
        _ => None,
    }
}

/// Reads the settings in one file, or `None` when the file is missing or
/// cannot be used and the caller should look elsewhere.
fn read_file(path: &Path) -> Option<Settings> {
    let root = read_object(path)?;
    let version = settings_version(&root);
    let source_version = version.unwrap_or(0);
    if source_version > CURRENT_SETTINGS_VERSION {
        // Show what can be understood of the newer file, but never save over it.
        let mut settings = Settings::from_json(&root, source_version).unwrap_or_default();
        settings.settings_version = source_version;
        settings.newer_file = Some(NewerFile {
            version: source_version,
            path: path.to_path_buf(),
        });
        return Some(settings);
    }
    let mut settings = Settings::from_json(&root, source_version)?;
    // Files from before schema versions defaulted to the dark theme rather
    // than recording a choice.
    if version.is_none() && settings.theme == ThemeChoice::Dark {
        settings.theme = ThemeChoice::System;
    }
    Some(settings)
}

fn read_settings_version(path: &Path) -> Option<i32> {
    settings_version(&read_object(path)?)
}

/// The schema version recorded in a file, if it has a usable one.
fn settings_version(root: &Map<String, Value>) -> Option<i32> {
    let version = match Properties(root).get("SettingsVersion")? {
        Value::Number(number) => match number.as_i64() {
            Some(whole) => i32::try_from(whole).ok()?,
            None => {
                let rounded = number.as_f64()?.round_ties_even();
                if !(f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&rounded) {
                    return None;
                }
                rounded as i32
            }
        },
        Value::String(text) => text.trim().parse().ok()?,
        Value::Bool(flag) => i32::from(*flag),
        _ => return None,
    };
    (version >= 0).then_some(version)
}

/// Typed access to a parsed settings file, converting values the way the C#
/// app's deserializer does. The outer `Option` of each reader is `None` when
/// the value cannot be converted.
struct Properties<'a>(&'a Map<String, Value>);

impl Properties<'_> {
    /// Finds a property without regard to the case of its name.
    fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name).or_else(|| {
            self.0
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value)
        })
    }

    fn flag(&self, name: &str, default: bool) -> Option<bool> {
        match self.get(name) {
            None => Some(default),
            Some(Value::Bool(flag)) => Some(*flag),
            Some(Value::String(text)) => {
                let text = text.trim();
                if text.eq_ignore_ascii_case("true") {
                    Some(true)
                } else if text.eq_ignore_ascii_case("false") {
                    Some(false)
                } else {
                    None
                }
            }
            Some(_) => None,
        }
    }

    fn integer(&self, name: &str, default: i32) -> Option<i32> {
        match self.get(name) {
            None => Some(default),
            Some(Value::Number(number)) => match number.as_i64() {
                Some(whole) => i32::try_from(whole).ok(),
                None => {
                    let value = number.as_f64()?;
                    let whole = value.fract() == 0.0
                        && (f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&value);
                    whole.then_some(value as i32)
                }
            },
            Some(Value::String(text)) => text.trim().parse().ok(),
            Some(_) => None,
        }
    }

    /// A text property; missing and `null` both read as no text.
    fn text(&self, name: &str) -> Option<Option<String>> {
        match self.get(name) {
            None | Some(Value::Null) => Some(None),
            Some(Value::String(text)) => Some(Some(text.clone())),
            Some(Value::Number(number)) => Some(Some(number.to_string())),
            Some(Value::Bool(flag)) => Some(Some(if *flag { "True" } else { "False" }.to_string())),
            Some(_) => None,
        }
    }

    /// A date property in milliseconds since the Unix epoch.
    fn date(&self, name: &str) -> Option<Option<i64>> {
        match self.get(name) {
            None | Some(Value::Null) => Some(None),
            Some(Value::String(text)) if text.is_empty() => Some(None),
            Some(Value::String(text)) => parse_json_date(text)
                .or_else(|| parse_rfc3339(text)?.checked_mul(1_000))
                .map(Some),
            Some(_) => None,
        }
    }
}

/// Parses `/Date(ms)/`, optionally with a time-zone suffix after the number,
/// which the C# serializer ignores as well.
fn parse_json_date(text: &str) -> Option<i64> {
    let rest = text.strip_prefix("/Date(")?;
    let (inside, _) = rest.split_once(")/")?;
    let digits_end = inside
        .char_indices()
        .find(|&(index, character)| !(character.is_ascii_digit() || index == 0 && character == '-'))
        .map_or(inside.len(), |(index, _)| index);
    let (number, suffix) = inside.split_at(digits_end);
    let offset = suffix.strip_prefix(['+', '-']).is_some_and(|digits| {
        digits.len() == 4 && digits.bytes().all(|byte| byte.is_ascii_digit())
    });
    let letter = suffix.len() == 1 && suffix.bytes().all(|byte| byte.is_ascii_alphabetic());
    if !(suffix.is_empty() || offset || letter) {
        return None;
    }
    number.parse().ok()
}

/// Builds a JSON object in the C# serializer's output form.
#[derive(Default)]
struct JsonObject(String);

impl JsonObject {
    fn name(&mut self, name: &str) {
        self.0.push(if self.0.is_empty() { '{' } else { ',' });
        self.0.push('"');
        self.0.push_str(name);
        self.0.push_str("\":");
    }

    fn flag(&mut self, name: &str, value: bool) {
        self.name(name);
        self.0.push_str(if value { "true" } else { "false" });
    }

    fn number(&mut self, name: &str, value: i64) {
        self.name(name);
        self.0.push_str(&value.to_string());
    }

    fn text(&mut self, name: &str, value: Option<&str>) {
        self.name(name);
        let Some(value) = value else {
            self.0.push_str("null");
            return;
        };
        self.0.push('"');
        for character in value.chars() {
            match character {
                '"' => self.0.push_str("\\\""),
                '\\' => self.0.push_str("\\\\"),
                '\u{8}' => self.0.push_str("\\b"),
                '\t' => self.0.push_str("\\t"),
                '\n' => self.0.push_str("\\n"),
                '\u{c}' => self.0.push_str("\\f"),
                '\r' => self.0.push_str("\\r"),
                // The C# serializer also escapes characters that are unsafe
                // inside HTML or end a line in JavaScript.
                '<' | '>' | '\'' | '&' | '\u{85}' | '\u{2028}' | '\u{2029}' => {
                    self.0.push_str(&format!("\\u{:04x}", character as u32));
                }
                control if control < ' ' => {
                    self.0.push_str(&format!("\\u{:04x}", control as u32));
                }
                other => self.0.push(other),
            }
        }
        self.0.push('"');
    }

    fn date(&mut self, name: &str, milliseconds: Option<i64>) {
        self.name(name);
        match milliseconds {
            Some(milliseconds) => self.0.push_str(&format!("\"\\/Date({milliseconds})\\/\"")),
            None => self.0.push_str("null"),
        }
    }

    fn finish(mut self) -> String {
        if self.0.is_empty() {
            self.0.push('{');
        }
        self.0.push('}');
        self.0
    }
}

/// A sibling of `path` that no other save, in this or another process, uses.
fn temporary_path(path: &Path) -> PathBuf {
    static SEQUENCE: AtomicU32 = AtomicU32::new(0);
    let nanoseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(
        ".{:08x}{:016x}{:08x}.tmp",
        std::process::id(),
        nanoseconds as u64,
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    PathBuf::from(name)
}

/// Creates `path`, which must not exist, and writes `contents` through to disk.
fn write_new_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .custom_flags(FILE_FLAG_WRITE_THROUGH)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

/// Replaces `replaced` with `replacement` in one step, keeping the old
/// content as `backup`.
fn replace_file(replaced: &Path, replacement: &Path, backup: &Path) -> io::Result<()> {
    let wide = |path: &Path| -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };
    let (replaced, replacement, backup) = (wide(replaced), wide(replacement), wide(backup));
    // SAFETY: the three paths are null-terminated and outlive the call; the
    // remaining arguments are reserved and must be null.
    let replaced = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            backup.as_ptr(),
            REPLACEFILE_WRITE_THROUGH | REPLACEFILE_IGNORE_MERGE_ERRORS,
            null(),
            null(),
        )
    };
    if replaced == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A uniquely named directory under the system temporary directory that
    /// is removed, with everything in it, when the value is dropped.
    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new(purpose: &str) -> Self {
            static SEQUENCE: AtomicU32 = AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "usage-tray-rs-{purpose}-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }

        pub fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn parse(json: &str) -> Settings {
        let Value::Object(root) = serde_json::from_str(json).unwrap() else {
            panic!("not an object: {json}");
        };
        let version = settings_version(&root).unwrap_or(CURRENT_SETTINGS_VERSION);
        Settings::from_json(&root, version).unwrap()
    }

    fn load_text(json: &str) -> Settings {
        let directory = TempDir::new("load");
        let path = directory.file("settings.json");
        fs::write(&path, json).unwrap();
        Settings::load_from(&path)
    }

    fn file_names(directory: &TempDir) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// A file exactly as the C# app writes it, with made-up identifiers.
    const CSHARP_FILE: &str = concat!(
        r##"{"SettingsVersion":9,"OverlayNumber":true,"CriticalThreshold":5,"LowThreshold":20,"##,
        r##""IconMetric":"Weekly","ColorBars":true,"ShowPopupResetTimes":true,"##,
        r##""ShowPopupLastUpdated":true,"ShowAdditionalLimits":true,"ResetExpiryReminders":true,"##,
        r##""ResetExpiryLeadHours":24,"StartWithWindows":true,"ThresholdNotifications":true,"##,
        r##""AutoRedeemResetCredits":true,"AutoRedeemLeadMinutes":1,"RefreshSeconds":60,"##,
        r##""IdleRefreshSeconds":300,"ShowTrayBox":true,"UseCustomTrayBoxColor":true,"##,
        r##""TrayBoxColor":"#000000","TrayTextColor":"#FFFFFF","Theme":"System Default","##,
        r##""ForecastMethod":"LearnedRoutine","QuietHoursMode":"Automatic","##,
        r##""ShowProjectedUsage":true,"HighlightQuietHours":false,"##,
        r##""AutoRedeemCreditId":"RateLimitResetCredit_0123","##,
        r##""AutoRedeemIdempotencyKey":"11111111-2222-3333-4444-555555555555","##,
        r##""AutoRedeemAttemptStatus":"completed","##,
        r##""AutoRedeemLastAttemptUtc":"\/Date(1793300371325)\/"}"##
    );

    #[test]
    fn saved_tray_appearance_is_read() {
        let settings = parse(
            r##"{"CriticalThreshold":5,"LowThreshold":20,"IconMetric":"Weekly",
                "RefreshSeconds":60,"IdleRefreshSeconds":300,"ShowTrayBox":true,
                "UseCustomTrayBoxColor":true,"TrayBoxColor":"#000000","TrayTextColor":"#FFFFFF",
                "Theme":"Dark Mode","ColorBars":false}"##,
        );
        assert_eq!(settings.critical_threshold, 5);
        assert_eq!(settings.low_threshold, 20);
        assert_eq!(settings.icon_metric, IconMetric::Weekly);
        assert_eq!(settings.refresh_seconds, 60);
        assert_eq!(settings.idle_refresh_seconds, 300);
        assert_eq!(settings.tray_box_color, Some(0xFF00_0000));
        assert_eq!(settings.tray_text_color, 0xFFFF_FFFF);
        assert_eq!(settings.theme, ThemeChoice::Dark);
        assert!(!settings.color_bars);
    }

    #[test]
    fn missing_or_invalid_values_fall_back_like_the_existing_app() {
        assert_eq!(parse("{}"), Settings::default());
        let settings = parse(
            r#"{"CriticalThreshold":60,"LowThreshold":40,"RefreshSeconds":5,
                "IdleRefreshSeconds":10,"IconMetric":"auto","TrayTextColor":"teal-ish",
                "UseCustomTrayBoxColor":true,"TrayBoxColor":""}"#,
        );
        assert_eq!(
            (settings.critical_threshold, settings.low_threshold),
            (40, 40)
        );
        assert_eq!(settings.refresh_seconds, 30);
        assert_eq!(settings.idle_refresh_seconds, 30);
        assert_eq!(settings.icon_metric, IconMetric::Auto);
        assert_eq!(settings.tray_text_color, DEFAULT_TRAY_TEXT_COLOR);
        assert_eq!(settings.tray_box_color, Some(DEFAULT_TRAY_BOX_COLOR));
    }

    #[test]
    fn out_of_range_values_are_normalized() {
        let settings = parse(
            r#"{"CriticalThreshold":0,"LowThreshold":100,"RefreshSeconds":99999,
                "IdleRefreshSeconds":-4,"ResetExpiryLeadHours":73,"AutoRedeemLeadMinutes":0,
                "OverlayNumber":false,"IconMetric":"","Theme":"sepia",
                "ForecastMethod":"recentpace","QuietHoursMode":"OFF"}"#,
        );
        assert_eq!(
            (settings.critical_threshold, settings.low_threshold),
            (15, 25)
        );
        assert_eq!(settings.refresh_seconds, 3_600);
        assert_eq!(settings.idle_refresh_seconds, 0);
        assert_eq!(settings.reset_expiry_lead_hours, 24);
        assert_eq!(settings.auto_redeem_lead_minutes, 5);
        assert!(settings.overlay_number);
        assert_eq!(settings.icon_metric, IconMetric::Weekly);
        assert_eq!(settings.theme, ThemeChoice::System);
        assert_eq!(settings.forecast_method, ForecastMethod::RecentPace);
        assert_eq!(settings.quiet_hours_mode, QuietHoursMode::Off);

        // An idle interval is capped, and otherwise never shorter than the
        // interval used while Codex is running.
        let idle = |json: &str| parse(json).idle_refresh_seconds;
        assert_eq!(idle(r#"{"IdleRefreshSeconds":9000}"#), 7_200);
        assert_eq!(
            idle(r#"{"RefreshSeconds":600,"IdleRefreshSeconds":120}"#),
            600
        );
        assert_eq!(
            idle(r#"{"RefreshSeconds":600,"IdleRefreshSeconds":900}"#),
            900
        );
    }

    #[test]
    fn redemption_state_needs_both_identifiers() {
        let complete = parse(CSHARP_FILE);
        assert_eq!(
            complete.auto_redeem_credit_id.as_deref(),
            Some("RateLimitResetCredit_0123")
        );
        assert_eq!(
            complete.auto_redeem_attempt_status.as_deref(),
            Some("completed")
        );
        assert_eq!(
            complete.auto_redeem_last_attempt_ms,
            Some(1_793_300_371_325)
        );

        let partial = parse(
            r#"{"AutoRedeemCreditId":"credit","AutoRedeemIdempotencyKey":"  ",
                "AutoRedeemAttemptStatus":"pending",
                "AutoRedeemLastAttemptUtc":"\/Date(1793300371325)\/"}"#,
        );
        assert_eq!(partial.auto_redeem_credit_id, None);
        assert_eq!(partial.auto_redeem_idempotency_key, None);
        assert_eq!(partial.auto_redeem_attempt_status, None);
        assert_eq!(partial.auto_redeem_last_attempt_ms, None);
    }

    #[test]
    fn colors_parse_from_html_hex() {
        assert_eq!(parse_color("#0078D7"), Some(0xFF00_78D7));
        assert_eq!(parse_color("#fa0"), Some(0xFFFF_AA00));
        assert_eq!(parse_color("0078D7"), None);
        assert_eq!(parse_color("#12345"), None);
    }

    #[test]
    fn colors_format_as_uppercase_hex_without_alpha() {
        assert_eq!(format_color(0xFF00_78D7), "#0078D7");
        assert_eq!(format_color(0x80FF_AA00), "#FFAA00");
        assert_eq!(parse_color(&format_color(0xFF12_ABEF)), Some(0xFF12_ABEF));
    }

    #[test]
    fn percent_colors_follow_the_configured_thresholds() {
        let mut settings = parse(r#"{"CriticalThreshold":5,"LowThreshold":20}"#);
        assert_eq!(settings.color_for_percent(5), 0xFFDC_2828);
        assert_eq!(settings.color_for_percent(20), 0xFFF0_D200);
        assert_eq!(settings.color_for_percent(21), 0xFF00_78D7);

        // A fixed box color overrides the threshold colors and is remembered
        // after switching back to automatic coloring.
        assert_eq!(settings.box_color_for_percent(5), 0xFFDC_2828);
        settings.set_tray_box_color(true, 0xFF10_2030);
        assert_eq!(settings.box_color_for_percent(5), 0xFF10_2030);
        settings.set_tray_box_color(false, 0xFF10_2030);
        assert_eq!(settings.tray_box_color, None);
        assert_eq!(settings.picked_tray_box_color(), 0xFF10_2030);
        assert!(settings
            .to_json()
            .contains(r##""UseCustomTrayBoxColor":false,"TrayBoxColor":"#102030""##));
    }

    #[test]
    fn themes_resolve_against_the_system_setting() {
        assert!(ThemeChoice::Dark.is_dark(false));
        assert!(!ThemeChoice::Light.is_dark(true));
        assert!(ThemeChoice::System.is_dark(true));
        assert!(!ThemeChoice::System.is_dark(false));
    }

    #[test]
    fn a_file_from_the_existing_app_round_trips_unchanged() {
        let directory = TempDir::new("roundtrip");
        let path = directory.file("settings.json");
        fs::write(&path, CSHARP_FILE).unwrap();

        let mut settings = Settings::load_from(&path);
        assert!(settings.save_blocked_message().is_none());
        assert_eq!(settings.to_json(), CSHARP_FILE);
        settings.save_to(&path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), CSHARP_FILE);
        assert_eq!(Settings::load_from(&path), settings);
    }

    #[test]
    fn defaults_serialize_like_the_existing_app() {
        assert_eq!(
            Settings::default().to_json(),
            concat!(
                r##"{"SettingsVersion":9,"OverlayNumber":true,"CriticalThreshold":15,"##,
                r##""LowThreshold":25,"IconMetric":"Weekly","ColorBars":true,"##,
                r##""ShowPopupResetTimes":true,"ShowPopupLastUpdated":true,"##,
                r##""ShowAdditionalLimits":true,"ResetExpiryReminders":true,"##,
                r##""ResetExpiryLeadHours":24,"StartWithWindows":false,"##,
                r##""ThresholdNotifications":false,"AutoRedeemResetCredits":false,"##,
                r##""AutoRedeemLeadMinutes":5,"RefreshSeconds":300,"IdleRefreshSeconds":0,"##,
                r##""ShowTrayBox":true,"UseCustomTrayBoxColor":false,"TrayBoxColor":"#0078D7","##,
                r##""TrayTextColor":"#FFFFFF","Theme":"System Default","##,
                r##""ForecastMethod":"LearnedRoutine","QuietHoursMode":"Automatic","##,
                r##""ShowProjectedUsage":true,"HighlightQuietHours":false,"##,
                r##""AutoRedeemCreditId":null,"AutoRedeemIdempotencyKey":null,"##,
                r##""AutoRedeemAttemptStatus":null,"AutoRedeemLastAttemptUtc":null}"##
            )
        );
    }

    #[test]
    fn text_is_escaped_like_the_existing_serializer() {
        let settings = Settings {
            auto_redeem_credit_id: Some("a/b<c>'d\"e\\f&g\u{e9}\n\u{1}".to_string()),
            auto_redeem_idempotency_key: Some("key".to_string()),
            ..Settings::default()
        };
        let json = settings.to_json();
        assert!(json.contains(
            "\"AutoRedeemCreditId\":\"a/b\\u003cc\\u003e\\u0027d\\\"e\\\\f\\u0026g\u{e9}\\n\\u0001\""
        ));
        assert_eq!(
            parse(&json).auto_redeem_credit_id,
            settings.auto_redeem_credit_id
        );
    }

    #[test]
    fn dates_accept_the_forms_the_existing_app_reads() {
        let date = |json: &str| {
            let Value::Object(root) = serde_json::from_str(json).unwrap() else {
                panic!("not an object: {json}");
            };
            Properties(&root).date("When")
        };
        assert_eq!(
            date(r#"{"When":"\/Date(1793300371325)\/"}"#),
            Some(Some(1_793_300_371_325))
        );
        assert_eq!(date(r#"{"When":"\/Date(-5)\/"}"#), Some(Some(-5)));
        assert_eq!(date(r#"{"When":"\/Date(1000+0200)\/"}"#), Some(Some(1_000)));
        assert_eq!(
            date(r#"{"When":"2026-10-30T01:02:03Z"}"#),
            Some(Some(1_793_322_123_000))
        );
        assert_eq!(
            date(r#"{"When":"2026-10-30T01:02:03"}"#),
            Some(Some(1_793_322_123_000))
        );
        assert_eq!(date(r#"{"When":null}"#), Some(None));
        assert_eq!(date(r#"{"When":""}"#), Some(None));
        assert_eq!(date("{}"), Some(None));
        assert_eq!(date(r#"{"When":"nonsense"}"#), None);
        assert_eq!(date(r#"{"When":"\/Date(12x4)\/"}"#), None);
        assert_eq!(date(r#"{"When":1793300371325}"#), None);
    }

    #[test]
    fn values_convert_like_the_existing_deserializer() {
        // Names match without regard to case; numbers and flags may be quoted.
        let settings = load_text(
            r#"{"settingsversion":"9","REFRESHSECONDS":" 120 ","colorBars":"False",
                "showtraybox":" TRUE ","LowThreshold":4e1,"IconMetric":5}"#,
        );
        assert_eq!(settings.refresh_seconds, 120);
        assert!(!settings.color_bars);
        assert!(settings.show_tray_box);
        assert_eq!(settings.low_threshold, 40);
        assert_eq!(settings.icon_metric, IconMetric::Weekly);

        // A value of the wrong kind makes the whole file unusable.
        for unusable in [
            r#"{"SettingsVersion":9,"RefreshSeconds":120,"ColorBars":1}"#,
            r#"{"SettingsVersion":9,"RefreshSeconds":120,"ColorBars":null}"#,
            r#"{"SettingsVersion":9,"RefreshSeconds":"soon"}"#,
            r#"{"SettingsVersion":9,"RefreshSeconds":120.5}"#,
            r#"{"SettingsVersion":9,"RefreshSeconds":true}"#,
            r#"{"SettingsVersion":9,"RefreshSeconds":99999999999}"#,
            r#"{"SettingsVersion":9,"RefreshSeconds":120,"Theme":["Dark Mode"]}"#,
            r#"{"SettingsVersion":9,"RefreshSeconds":120,"AutoRedeemLastAttemptUtc":5}"#,
            r#"[{"RefreshSeconds":120}]"#,
            "not json",
            "",
        ] {
            assert_eq!(load_text(unusable), Settings::default(), "{unusable}");
        }
    }

    #[test]
    fn older_files_are_migrated() {
        // The upgrade test from the existing app: a version 7 file keeps the
        // chosen metric and gains the settings added since.
        let directory = TempDir::new("migrate");
        let path = directory.file("settings.json");
        fs::write(
            &path,
            r#"{"SettingsVersion":7,"IconMetric":"FiveHour","ShowResetAvailability":false}"#,
        )
        .unwrap();
        let mut settings = Settings::load_from(&path);
        assert_eq!(settings.icon_metric, IconMetric::FiveHour);
        assert!(settings.reset_expiry_reminders);
        assert_eq!(settings.reset_expiry_lead_hours, 24);
        assert_eq!(settings.forecast_method, ForecastMethod::LearnedRoutine);
        assert_eq!(settings.quiet_hours_mode, QuietHoursMode::Automatic);
        assert!(settings.show_projected_usage && !settings.highlight_quiet_hours);
        assert_eq!(settings.settings_version, CURRENT_SETTINGS_VERSION);

        settings.icon_metric = IconMetric::Auto;
        settings.reset_expiry_reminders = false;
        settings.reset_expiry_lead_hours = 6;
        settings.forecast_method = ForecastMethod::RecentPace;
        settings.quiet_hours_mode = QuietHoursMode::Off;
        settings.show_projected_usage = false;
        settings.highlight_quiet_hours = true;
        settings.save_to(&path).unwrap();

        let reloaded = Settings::load_from(&path);
        assert_eq!(reloaded, settings);
        assert_eq!(reloaded.icon_metric, IconMetric::Auto);
        assert!(!reloaded.reset_expiry_reminders);
        assert_eq!(reloaded.reset_expiry_lead_hours, 6);
        assert_eq!(reloaded.forecast_method, ForecastMethod::RecentPace);
        assert_eq!(reloaded.quiet_hours_mode, QuietHoursMode::Off);
        assert!(!reloaded.show_projected_usage && reloaded.highlight_quiet_hours);
        // Properties this version does not know are not written back.
        assert!(!fs::read_to_string(&path)
            .unwrap()
            .contains("ShowResetAvailability"));
    }

    #[test]
    fn each_schema_version_resets_only_the_settings_it_introduced() {
        let values = r#""ShowAdditionalLimits":false,"AutoRedeemResetCredits":true,
            "AutoRedeemLeadMinutes":30,"AutoRedeemCreditId":"credit",
            "AutoRedeemIdempotencyKey":"key","ResetExpiryReminders":false,
            "ResetExpiryLeadHours":6,"ForecastMethod":"RecentPace","QuietHoursMode":"Off",
            "ShowProjectedUsage":false,"HighlightQuietHours":true"#;
        let at = |version: i32| load_text(&format!(r#"{{"SettingsVersion":{version},{values}}}"#));

        let current = at(9);
        assert!(!current.show_additional_limits);
        assert!(current.auto_redeem_reset_credits);
        assert_eq!(current.auto_redeem_lead_minutes, 30);
        assert_eq!(current.auto_redeem_credit_id.as_deref(), Some("credit"));
        assert!(!current.reset_expiry_reminders);
        assert_eq!(current.reset_expiry_lead_hours, 6);
        assert_eq!(current.forecast_method, ForecastMethod::RecentPace);
        assert_eq!(current.quiet_hours_mode, QuietHoursMode::Off);
        assert!(!current.show_projected_usage && current.highlight_quiet_hours);

        let eight = at(8);
        assert_eq!(eight.forecast_method, ForecastMethod::LearnedRoutine);
        assert_eq!(eight.quiet_hours_mode, QuietHoursMode::Automatic);
        assert!(eight.show_projected_usage && !eight.highlight_quiet_hours);
        assert!(!eight.reset_expiry_reminders);

        let seven = at(7);
        assert!(seven.reset_expiry_reminders);
        assert_eq!(seven.reset_expiry_lead_hours, 24);
        assert!(seven.auto_redeem_reset_credits);

        let six = at(6);
        assert!(!six.auto_redeem_reset_credits);
        assert_eq!(six.auto_redeem_lead_minutes, 5);
        assert_eq!(six.auto_redeem_credit_id, None);
        assert!(!six.show_additional_limits);

        assert!(at(5).show_additional_limits);
        assert_eq!(at(5).settings_version, CURRENT_SETTINGS_VERSION);
    }

    #[test]
    fn unversioned_files_do_not_keep_the_old_default_dark_theme() {
        assert_eq!(
            load_text(r#"{"Theme":"Dark Mode"}"#).theme,
            ThemeChoice::System
        );
        assert_eq!(
            load_text(r#"{"Theme":"Light Mode"}"#).theme,
            ThemeChoice::Light
        );
        assert_eq!(
            load_text(r#"{"SettingsVersion":-3,"Theme":"dark mode"}"#).theme,
            ThemeChoice::System
        );
        assert_eq!(
            load_text(r#"{"SettingsVersion":1,"Theme":"dark mode"}"#).theme,
            ThemeChoice::Dark
        );
    }

    #[test]
    fn the_backup_is_used_when_the_file_is_missing_or_unreadable() {
        let directory = TempDir::new("backup");
        let path = directory.file("settings.json");
        assert_eq!(Settings::load_from(&path), Settings::default());

        fs::write(
            backup_path(&path),
            r#"{"SettingsVersion":9,"RefreshSeconds":90}"#,
        )
        .unwrap();
        assert_eq!(Settings::load_from(&path).refresh_seconds, 90);

        fs::write(&path, "{ truncated").unwrap();
        assert_eq!(Settings::load_from(&path).refresh_seconds, 90);

        fs::write(&path, r#"{"SettingsVersion":9,"RefreshSeconds":45}"#).unwrap();
        assert_eq!(Settings::load_from(&path).refresh_seconds, 45);

        fs::write(backup_path(&path), "also broken").unwrap();
        fs::write(&path, "{ truncated").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
    }

    #[test]
    fn saving_creates_the_folder_and_keeps_the_previous_file_as_backup() {
        let directory = TempDir::new("save");
        let path = directory.path().join("nested").join("settings.json");
        let mut settings = Settings {
            refresh_seconds: 60,
            ..Settings::default()
        };
        settings.save_to(&path).unwrap();
        assert!(!backup_path(&path).exists());
        let first = fs::read_to_string(&path).unwrap();

        settings.refresh_seconds = 120;
        settings.save_to(&path).unwrap();
        assert_eq!(fs::read_to_string(backup_path(&path)).unwrap(), first);
        assert_eq!(Settings::load_from(&path).refresh_seconds, 120);

        // A third save replaces the backup and leaves no temporary files.
        let second = fs::read_to_string(&path).unwrap();
        settings.refresh_seconds = 180;
        settings.save_to(&path).unwrap();
        assert_eq!(fs::read_to_string(backup_path(&path)).unwrap(), second);
        let mut names: Vec<String> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["settings.json", "settings.json.bak"]);

        assert!(matches!(
            Settings::default().save_to(Path::new("settings.json")),
            Err(SaveError::NoFolder)
        ));
    }

    #[test]
    fn a_newer_file_is_shown_but_never_overwritten() {
        let directory = TempDir::new("newer");
        let path = directory.file("settings.json");
        let newer = r#"{"SettingsVersion":12,"RefreshSeconds":90,"FutureSetting":{"a":1}}"#;
        fs::write(&path, newer).unwrap();

        let mut settings = Settings::load_from(&path);
        assert_eq!(settings.settings_version, 12);
        assert_eq!(settings.refresh_seconds, 90);
        let message = settings.save_blocked_message().unwrap();
        assert_eq!(
            message,
            format!(
                "Settings were created by a newer app version (schema 12). This version \
                 supports schema 9 and will not overwrite the file: {}",
                path.display()
            )
        );
        let error = settings.save_to(&path).unwrap_err();
        assert_eq!(error.to_string(), message);
        assert_eq!(fs::read_to_string(&path).unwrap(), newer);

        // Values that never came from the newer file cannot replace it either,
        // whether it is the file itself or its backup that is newer.
        let error = Settings::default().save_to(&path).unwrap_err();
        assert!(matches!(error, SaveError::NewerVersion { version: 12, .. }));
        fs::rename(&path, backup_path(&path)).unwrap();
        let error = Settings::default().save_to(&path).unwrap_err();
        assert!(
            matches!(error, SaveError::NewerVersion { version: 12, path: blocked } if blocked == backup_path(&path))
        );
        assert_eq!(file_names(&directory), ["settings.json.bak"]);

        // A newer file that cannot be fully understood still blocks saving.
        fs::write(&path, r#"{"SettingsVersion":10,"RefreshSeconds":"soon"}"#).unwrap();
        let unreadable = Settings::load_from(&path);
        assert!(unreadable.save_blocked_message().is_some());
        assert_eq!(unreadable.refresh_seconds, 300);
    }

    #[test]
    fn a_byte_order_mark_does_not_hide_the_settings() {
        let directory = TempDir::new("bom");
        let path = directory.file("settings.json");
        let mut bytes = b"\xEF\xBB\xBF".to_vec();
        bytes.extend_from_slice(br#"{"SettingsVersion":9,"RefreshSeconds":75}"#);
        fs::write(&path, bytes).unwrap();
        assert_eq!(Settings::load_from(&path).refresh_seconds, 75);
    }
}
