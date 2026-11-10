//! The content of the Settings window: the values being edited, the pages of
//! cards that edit them, and what pointer and keyboard input does to them.
//!
//! This module owns no window. It draws on any `Graphics`, so the same code
//! paints the real window and the review images written by the tests, and it
//! answers input with a [`Command`] for `settings_window.rs` to carry out.
//! Nothing is written to disk from here: edits live in a [`Draft`] until the
//! window saves them.

use serde_json::{json, Map, Value};

use crate::controls::{
    self, mix, Card, Caret, CaretMove, Cursor, HAlign, Icon, IconSize, Interaction, Node,
    NumberEdit, Painter, Palette, Scroll, Split, Style, Tone, Visual,
};
use crate::gfx::{Align, Graphics, IconBitmap, Rect};
use crate::settings::{
    format_color, threshold_color, ForecastMethod, IconMetric, QuietHoursMode, Settings,
    ThemeChoice,
};
use crate::version;

/// Client size of the window at 100% scale.
pub const CLIENT_SIZE: (f32, f32) = (1200.0, 732.0);
/// Smallest window size, frame included, at 100% scale.
pub const MIN_WINDOW_SIZE: (f32, f32) = (980.0, 650.0);

const SIDEBAR_WIDTH: f32 = 252.0;
const NAVIGATION_HEIGHT: f32 = 46.0;
const NAVIGATION_GAP: f32 = 8.0;
const CARD_GAP: f32 = 12.0;
const SCROLLBAR_WIDTH: f32 = 6.0;
/// Width taken from the cards while the scroll bar is shown.
const SCROLLBAR_GUTTER: f32 = 14.0;
const MIN_THUMB_LENGTH: f32 = 28.0;
/// Distance scrolled by one notch of the mouse wheel or one arrow key.
const SCROLL_STEP: f32 = 48.0;
const REVEAL_MARGIN: f32 = 10.0;
const PREVIEW_TILE: f32 = 34.0;
/// The tray draws its icons this large at 100% scale.
const PREVIEW_ICON: f32 = 16.0;
const DEFAULT_PREVIEW_PERCENT: i32 = 50;
const PREVIEW_BACKDROP_DARK: u32 = 0xFF26_2626;
const PREVIEW_BACKDROP_LIGHT: u32 = 0xFFE8_E8E8;
/// Contrast below this is flagged, following the WCAG guidance for text.
const MIN_CONTRAST: f64 = 4.5;
const CHECK_UPDATES_LABEL: &str = "Check Updates";
const DEFAULT_BUSY_LABEL: &str = "Checking...";

const GLYPH_SETTINGS: Icon = Icon::Glyph("\u{E713}");
const GLYPH_TREND: Icon = Icon::Glyph("\u{EAFC}");
const GLYPH_COLOR: Icon = Icon::Glyph("\u{E790}");
const GLYPH_MESSAGE: Icon = Icon::Glyph("\u{E8BD}");
const GLYPH_REFRESH: Icon = Icon::Glyph("\u{E72C}");
const GLYPH_DESKTOP: Icon = Icon::Glyph("\u{E7F4}");
const GLYPH_ALERT: Icon = Icon::Glyph("\u{EA8F}");
const GLYPH_CLOCK: Icon = Icon::Glyph("\u{E917}");
const GLYPH_CALENDAR: Icon = Icon::Glyph("\u{E787}");
const GLYPH_INFO: Icon = Icon::Glyph("\u{E946}");

const QUIET_HOURS_ITEMS: [&str; 2] = ["Learn automatically", "Off"];
const THEME_ITEMS: [&str; 3] = ["System default", "Dark mode", "Light mode"];
const THEMES: [ThemeChoice; 3] = [ThemeChoice::System, ThemeChoice::Dark, ThemeChoice::Light];
const METRIC_GROUP: [Id; 3] = [Id::MetricWeekly, Id::MetricFiveHour, Id::MetricAuto];
const FORECAST_GROUP: [Id; 2] = [Id::ForecastLearned, Id::ForecastRecent];

/// The categories in the sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    General,
    Usage,
    Forecast,
    Appearance,
    Popup,
    Updates,
}

impl Page {
    /// Every page, in sidebar order.
    pub const ALL: [Page; 6] = [
        Page::General,
        Page::Usage,
        Page::Forecast,
        Page::Appearance,
        Page::Popup,
        Page::Updates,
    ];

    /// The page's name in the sidebar and its heading.
    pub const fn title(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Usage => "Usage & alerts",
            Self::Forecast => "Forecast & graphs",
            Self::Appearance => "Appearance",
            Self::Popup => "Popup",
            Self::Updates => "Updates",
        }
    }

    const fn description(self) -> &'static str {
        match self {
            Self::General => "Control refresh intervals and startup.",
            Self::Usage => {
                "Control how usage is shown in the tray and when you\u{2019}re notified."
            }
            Self::Forecast => {
                "Configure how usage forecasts are generated and how graphs present your usage."
            }
            Self::Appearance => "Personalize the app and tray icon.",
            Self::Popup => "Choose which details appear in the tray popup.",
            Self::Updates => "Check the installed version and look for updates.",
        }
    }

    const fn icon(self) -> Icon {
        match self {
            Self::General => GLYPH_SETTINGS,
            Self::Usage => Icon::Bars,
            Self::Forecast => GLYPH_TREND,
            Self::Appearance => GLYPH_COLOR,
            Self::Popup => GLYPH_MESSAGE,
            Self::Updates => GLYPH_REFRESH,
        }
    }
}

/// Everything in the window that can be clicked or focused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Id {
    Navigation(Page),
    MetricWeekly,
    MetricFiveHour,
    MetricAuto,
    ThresholdAlerts,
    Critical,
    Low,
    ReminderEnabled,
    ReminderHours,
    AutoUseEnabled,
    AutoUseMinutes,
    ForecastLearned,
    ForecastRecent,
    QuietHours,
    ShowProjection,
    HighlightQuiet,
    RunningRefresh,
    IdleRefresh,
    StartWithWindows,
    Theme,
    TrayBackground,
    TrayBoxColor,
    TrayBoxAuto,
    TrayTextColor,
    PreviewRemaining,
    ShowUpdatedTime,
    ShowResetTimes,
    ColorUsageBars,
    CheckUpdates,
    RestoreDefaults,
    Cancel,
    Save,
    /// An entry of the dropdown list that is open.
    DropdownItem(usize),
    Scrollbar,
}

impl Id {
    /// The range of a numeric field, or `None` for any other control.
    const fn range(self) -> Option<(i32, i32)> {
        match self {
            Self::Critical | Self::Low => Some((1, 99)),
            Self::ReminderHours => Some((1, 72)),
            Self::AutoUseMinutes => Some((1, 120)),
            Self::RunningRefresh => Some((30, 3_600)),
            Self::IdleRefresh => Some((0, 7_200)),
            Self::PreviewRemaining => Some((0, 100)),
            _ => None,
        }
    }

    /// True for controls that Enter presses instead of saving.
    const fn is_button(self) -> bool {
        matches!(
            self,
            Self::TrayBoxColor
                | Self::TrayBoxAuto
                | Self::TrayTextColor
                | Self::CheckUpdates
                | Self::RestoreDefaults
                | Self::Cancel
                | Self::Save
        )
    }

    /// True for controls inside the scrolling part of the window.
    const fn scrolls(self) -> bool {
        !matches!(
            self,
            Self::Navigation(_)
                | Self::RestoreDefaults
                | Self::Cancel
                | Self::Save
                | Self::DropdownItem(_)
                | Self::Scrollbar
        )
    }
}

/// Which tray color a color picker is choosing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorTarget {
    TrayBox,
    TrayText,
}

/// Something the window has to do in response to input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Write the settings and, if that works, close the window.
    Save,
    /// Close the window, discarding unsaved edits.
    Close,
    /// Show a color picker, then call `View::finish_color_choice`.
    PickColor(ColorTarget),
    /// Ask the tray to check for updates.
    CheckUpdates,
}

/// A key the view responds to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Tab,
    Enter,
    Escape,
    Space,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Backspace,
    Delete,
    /// Ctrl+A.
    SelectAll,
}

/// The values the window edits, kept apart from the saved settings until
/// Save is chosen.
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    pub critical_threshold: i32,
    pub low_threshold: i32,
    pub refresh_seconds: i32,
    pub idle_refresh_seconds: i32,
    pub auto_redeem_lead_minutes: i32,
    pub reset_expiry_lead_hours: i32,
    pub icon_metric: IconMetric,
    pub reset_expiry_reminders: bool,
    pub color_bars: bool,
    pub show_reset_times: bool,
    pub show_last_updated: bool,
    pub start_with_windows: bool,
    pub threshold_notifications: bool,
    pub auto_redeem_reset_credits: bool,
    pub show_tray_box: bool,
    pub use_custom_tray_box_color: bool,
    pub tray_box_color: u32,
    pub tray_text_color: u32,
    pub theme: ThemeChoice,
    pub forecast_method: ForecastMethod,
    pub quiet_hours_mode: QuietHoursMode,
    pub show_projected_usage: bool,
    pub highlight_quiet_hours: bool,
}

impl Draft {
    /// Starts a draft from saved settings, bringing every number into the
    /// range its field allows.
    pub fn from_settings(settings: &Settings) -> Self {
        let within = |value: i64, id: Id| {
            let (min, max) = id.range().unwrap_or((i32::MIN, i32::MAX));
            i32::try_from(value.clamp(i64::from(min), i64::from(max))).unwrap_or(min)
        };
        let seconds = |value: u64| i64::try_from(value).unwrap_or(i64::MAX);
        let mut draft = Self {
            critical_threshold: within(i64::from(settings.critical_threshold), Id::Critical),
            low_threshold: within(i64::from(settings.low_threshold), Id::Low),
            refresh_seconds: within(seconds(settings.refresh_seconds), Id::RunningRefresh),
            idle_refresh_seconds: within(seconds(settings.idle_refresh_seconds), Id::IdleRefresh),
            auto_redeem_lead_minutes: within(
                i64::from(settings.auto_redeem_lead_minutes),
                Id::AutoUseMinutes,
            ),
            reset_expiry_lead_hours: within(
                i64::from(settings.reset_expiry_lead_hours),
                Id::ReminderHours,
            ),
            icon_metric: settings.icon_metric,
            reset_expiry_reminders: settings.reset_expiry_reminders,
            color_bars: settings.color_bars,
            show_reset_times: settings.show_reset_times,
            show_last_updated: settings.show_last_updated,
            start_with_windows: settings.start_with_windows,
            threshold_notifications: settings.threshold_notifications,
            auto_redeem_reset_credits: settings.auto_redeem_reset_credits,
            show_tray_box: settings.show_tray_box,
            use_custom_tray_box_color: settings.tray_box_color.is_some(),
            tray_box_color: settings.picked_tray_box_color(),
            tray_text_color: settings.tray_text_color,
            theme: settings.theme,
            forecast_method: settings.forecast_method,
            quiet_hours_mode: settings.quiet_hours_mode,
            show_projected_usage: settings.show_projected_usage,
            highlight_quiet_hours: settings.highlight_quiet_hours,
        };
        draft.low_threshold = draft.low_threshold.max(draft.critical_threshold);
        draft.keep_idle_refresh_valid();
        draft
    }

    /// Copies the edited values into `settings`, leaving the rest as it is.
    pub fn apply_to(&self, settings: &mut Settings) {
        settings.critical_threshold = self.critical_threshold;
        settings.low_threshold = self.low_threshold;
        settings.icon_metric = self.icon_metric;
        settings.color_bars = self.color_bars;
        settings.show_reset_times = self.show_reset_times;
        settings.show_last_updated = self.show_last_updated;
        settings.reset_expiry_reminders = self.reset_expiry_reminders;
        settings.reset_expiry_lead_hours = self.reset_expiry_lead_hours;
        settings.start_with_windows = self.start_with_windows;
        settings.threshold_notifications = self.threshold_notifications;
        settings.auto_redeem_reset_credits = self.auto_redeem_reset_credits;
        settings.auto_redeem_lead_minutes = self.auto_redeem_lead_minutes;
        settings.show_tray_box = self.show_tray_box;
        settings.set_tray_box_color(self.use_custom_tray_box_color, self.tray_box_color);
        settings.tray_text_color = self.tray_text_color;
        settings.refresh_seconds = u64::from(self.refresh_seconds.unsigned_abs());
        settings.idle_refresh_seconds = u64::from(self.idle_refresh_seconds.unsigned_abs());
        settings.theme = self.theme;
        settings.forecast_method = self.forecast_method;
        settings.quiet_hours_mode = self.quiet_hours_mode;
        settings.show_projected_usage = self.show_projected_usage;
        settings.highlight_quiet_hours = self.highlight_quiet_hours;
    }

    /// An idle interval other than "off" is never shorter than the interval
    /// used while Codex is running.
    fn keep_idle_refresh_valid(&mut self) {
        if self.idle_refresh_seconds > 0 && self.idle_refresh_seconds < self.refresh_seconds {
            self.idle_refresh_seconds = self.refresh_seconds;
        }
    }
}

/// What the tray tells the window about itself. Fields left `None` keep the
/// value the window already shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// The installed version, shown as `Version <version>`.
    pub version: Option<String>,
    /// The line under the version, such as `Up to date: 1.2.3`.
    pub update_status: Option<String>,
    /// True while an update check or installation is running.
    pub checking: Option<bool>,
    /// The update button's label while `checking`; `Checking...` if absent.
    pub busy_text: Option<String>,
    /// Whether the app is registered to start with Windows.
    pub startup_enabled: Option<bool>,
}

impl Status {
    /// The payload the tray sends, leaving out the fields that are `None`.
    pub fn to_json(&self) -> String {
        let mut root = Map::new();
        let mut put = |name: &str, value: Option<Value>| {
            if let Some(value) = value {
                root.insert(name.to_string(), value);
            }
        };
        put("version", self.version.as_deref().map(|text| json!(text)));
        put(
            "updateStatus",
            self.update_status.as_deref().map(|text| json!(text)),
        );
        put("checking", self.checking.map(|flag| json!(flag)));
        put(
            "busyText",
            self.busy_text.as_deref().map(|text| json!(text)),
        );
        put(
            "startupEnabled",
            self.startup_enabled.map(|flag| json!(flag)),
        );
        Value::Object(root).to_string()
    }

    /// Parses a status; unknown properties are ignored and properties of the
    /// wrong kind are treated as absent.
    pub fn from_json(bytes: &[u8]) -> Option<Self> {
        let root: Value = serde_json::from_slice(bytes).ok()?;
        let root = root.as_object()?;
        let text = |name: &str| root.get(name).and_then(Value::as_str).map(str::to_string);
        let flag = |name: &str| root.get(name).and_then(Value::as_bool);
        Some(Self {
            version: text("version"),
            update_status: text("updateStatus"),
            checking: flag("checking"),
            busy_text: text("busyText"),
            startup_enabled: flag("startupEnabled"),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OpenDropdown {
    owner: Id,
    highlighted: usize,
}

/// The Settings window's state and behavior.
pub struct View {
    draft: Draft,
    /// The draft as last loaded or saved; differing from it means unsaved edits.
    saved: Draft,
    page: Page,
    preview_percent: i32,
    interaction: Interaction<Id>,
    /// The numeric field being typed into.
    edit: Option<(Id, NumberEdit)>,
    dropdown: Option<OpenDropdown>,
    scroll: Scroll,
    /// While the scroll thumb is dragged: where on the thumb it was grabbed.
    scroll_drag: Option<f32>,
    choosing_color: bool,
    version: String,
    update_status: String,
    checking: bool,
    busy_label: String,
    /// Whether the tray has reported the real Windows startup registration.
    startup_reported: bool,
    system_dark: bool,
    caret_visible: bool,
    style: Option<Style>,
    viewport: Rect,
    track: Rect,
}

impl View {
    /// Starts editing `settings`. `system_dark` is the Windows app theme,
    /// used while the theme setting follows the system.
    pub fn new(settings: &Settings, system_dark: bool) -> Self {
        let draft = Draft::from_settings(settings);
        let mut interaction = Interaction::default();
        interaction.set_focus(Some(Id::Navigation(Page::Usage)), false);
        Self {
            saved: draft.clone(),
            draft,
            page: Page::Usage,
            preview_percent: DEFAULT_PREVIEW_PERCENT,
            interaction,
            edit: None,
            dropdown: None,
            scroll: Scroll::default(),
            scroll_drag: None,
            choosing_color: false,
            version: version::display(&version::current()),
            update_status: String::new(),
            checking: false,
            busy_label: DEFAULT_BUSY_LABEL.to_string(),
            startup_reported: false,
            system_dark,
            caret_visible: true,
            style: None,
            viewport: Rect::new(0.0, 0.0, 0.0, 0.0),
            track: Rect::new(0.0, 0.0, 0.0, 0.0),
        }
    }

    /// Whether the window is drawn dark, following the theme being edited.
    pub fn dark(&self) -> bool {
        self.draft.theme.is_dark(self.system_dark)
    }

    /// Tells the view the Windows app theme changed.
    pub fn set_system_dark(&mut self, system_dark: bool) {
        self.system_dark = system_dark;
    }

    /// True while a numeric field shows a caret, which the window blinks.
    pub fn is_editing(&self) -> bool {
        self.edit.is_some()
    }

    /// Flips the caret between the two halves of its blink.
    pub fn blink_caret(&mut self) {
        self.caret_visible = !self.caret_visible;
    }

    /// True when closing now would lose something: an edited value, a number
    /// still being typed, or a color still being picked.
    pub fn has_unsaved_changes(&self) -> bool {
        let typing = self.edit.as_ref().is_some_and(|(id, edit)| {
            // The preview percentage is not a setting.
            *id != Id::PreviewRemaining && edit.differs_from(self.number(*id))
        });
        self.choosing_color || typing || self.draft != self.saved
    }

    /// Takes in what the tray reported.
    pub fn set_status(&mut self, status: Status) {
        if let Some(version) = status.version {
            self.version = version;
        }
        if let Some(update_status) = status.update_status {
            self.update_status = update_status;
        }
        if let Some(checking) = status.checking {
            self.checking = checking;
            self.busy_label = status
                .busy_text
                .unwrap_or_else(|| DEFAULT_BUSY_LABEL.to_string());
        }
        if let Some(enabled) = status.startup_enabled.filter(|_| !self.startup_reported) {
            // The switch starts from the real registration rather than the
            // stored wish, unless it has already been changed here.
            self.startup_reported = true;
            if self.draft.start_with_windows == self.saved.start_with_windows {
                self.draft.start_with_windows = enabled;
            }
            self.saved.start_with_windows = enabled;
        }
    }

    /// Finishes any number being typed and copies the edited values into
    /// `settings`.
    pub fn apply_to(&mut self, settings: &mut Settings) {
        self.commit_edit();
        self.draft.apply_to(settings);
    }

    /// Records that the current values are the saved ones.
    pub fn mark_saved(&mut self) {
        self.saved = self.draft.clone();
    }

    /// Starts choosing a color and returns the color to start from.
    pub fn begin_color_choice(&mut self, target: ColorTarget) -> u32 {
        self.choosing_color = true;
        match target {
            ColorTarget::TrayBox => self.draft.tray_box_color,
            ColorTarget::TrayText => self.draft.tray_text_color,
        }
    }

    /// Ends choosing a color; `chosen` is `None` when the picker was canceled.
    pub fn finish_color_choice(&mut self, target: ColorTarget, chosen: Option<u32>) {
        self.choosing_color = false;
        let Some(color) = chosen.map(|color| color | 0xFF00_0000) else {
            return;
        };
        match target {
            ColorTarget::TrayBox => {
                self.draft.tray_box_color = color;
                self.draft.use_custom_tray_box_color = true;
            }
            ColorTarget::TrayText => self.draft.tray_text_color = color,
        }
    }

    /// The mouse pointer to show at a point of the window.
    pub fn cursor_at(&self, x: f32, y: f32) -> Cursor {
        self.interaction.cursor_at(x, y)
    }

    /// Follows the pointer; `None` means it left the window. Returns true
    /// when something changed appearance.
    pub fn pointer_moved(&mut self, point: Option<(f32, f32)>) -> bool {
        if let (Some(grab), Some((_, y))) = (self.scroll_drag, point) {
            let min_length = self.px(MIN_THUMB_LENGTH);
            let offset = self
                .scroll
                .offset_for_thumb(self.track, min_length, y - grab);
            return self.scroll.scroll_to(offset);
        }
        let mut changed = self.interaction.pointer_moved(point);
        if let (Some(open), Some(Id::DropdownItem(index))) =
            (&mut self.dropdown, self.interaction.hovered())
        {
            changed |= open.highlighted != index;
            open.highlighted = index;
        }
        changed
    }

    /// Handles the left button going down. `g` is used to place the caret
    /// when a numeric field is clicked.
    pub fn pointer_pressed(&mut self, g: &Graphics, x: f32, y: f32, double_click: bool) {
        let under = self.interaction.target_at(x, y).map(|target| target.id);
        if let Some(open) = self.dropdown {
            let inside = matches!(under, Some(Id::DropdownItem(_))) || under == Some(open.owner);
            if !inside {
                // A press elsewhere only dismisses the list.
                self.dropdown = None;
                self.interaction.pointer_moved(Some((x, y)));
                return;
            }
        }
        let focus_before = self.interaction.focused();
        match self.interaction.pointer_pressed(x, y) {
            Some(Id::DropdownItem(_)) => {
                // The list belongs to its dropdown, which keeps the focus.
                self.interaction.set_focus(focus_before, false);
            }
            Some(Id::Scrollbar) => {
                self.interaction.set_focus(focus_before, false);
                let thumb = self
                    .interaction
                    .target(Id::Scrollbar)
                    .map(|thumb| thumb.bounds);
                self.scroll_drag = thumb.map(|thumb| y - thumb.y);
            }
            Some(id) if id.range().is_some() => self.press_number(g, id, x, y, double_click),
            Some(_) => self.commit_edit(),
            // Empty space takes neither the focus nor the caret.
            None => {}
        }
    }

    /// Handles the left button coming up, which completes a click.
    pub fn pointer_released(&mut self, x: f32, y: f32) -> Option<Command> {
        let clicked = self.interaction.pointer_released(x, y);
        if self.scroll_drag.take().is_some() {
            return None;
        }
        self.activate(clicked?)
    }

    /// The window lost the mouse in the middle of a click or drag.
    pub fn capture_lost(&mut self) {
        self.scroll_drag = None;
        self.interaction.cancel_press();
    }

    /// The window lost the keyboard focus.
    pub fn focus_lost(&mut self) {
        self.commit_edit();
        self.dropdown = None;
    }

    /// The window regained the keyboard focus.
    pub fn focus_gained(&mut self) {
        self.sync_edit_with_focus(true);
    }

    /// Scrolls the cards by mouse-wheel notches; positive is away from the
    /// user. Returns true when something changed appearance.
    pub fn wheel(&mut self, notches: f32) -> bool {
        // A list left open would no longer be under its dropdown.
        let closed = self.dropdown.take().is_some();
        let scrolled = self.scroll.scroll_by(-notches * self.px(SCROLL_STEP));
        closed || scrolled
    }

    /// Handles a character typed on the keyboard.
    pub fn character(&mut self, character: char) {
        if let Some((id, edit)) = &mut self.edit {
            let allow_negative = id.range().is_some_and(|(min, _)| min < 0);
            if edit.insert(character, allow_negative) {
                self.caret_visible = true;
            }
        }
    }

    /// Handles a key press. `shift` reverses Tab.
    pub fn key(&mut self, key: Key, shift: bool) -> Option<Command> {
        self.interaction.show_focus();
        self.caret_visible = true;
        if let Some(open) = self.dropdown {
            self.dropdown_key(open, key, shift);
            return None;
        }
        if self.edit_key(key) {
            return None;
        }
        let focused = self.interaction.focused();
        match key {
            Key::Tab => {
                self.interaction.focus_next(shift);
                self.sync_edit_with_focus(true);
                self.reveal_focus();
                None
            }
            Key::Escape => Some(Command::Close),
            Key::Enter => match focused {
                Some(id) if id.is_button() => self.activate(id),
                Some(id @ (Id::QuietHours | Id::Theme)) => self.activate(id),
                // Anywhere else Enter is the default button, Save.
                _ => Some(Command::Save),
            },
            Key::Space => focused
                .filter(|id| id.range().is_none())
                .and_then(|id| self.activate(id)),
            Key::Left | Key::Right | Key::Up | Key::Down => {
                let forward = matches!(key, Key::Right | Key::Down);
                let vertical = matches!(key, Key::Up | Key::Down);
                self.arrow(focused, forward, vertical);
                None
            }
            Key::PageUp | Key::PageDown => {
                let page = self.viewport.h * 0.9;
                self.scroll
                    .scroll_by(if key == Key::PageUp { -page } else { page });
                None
            }
            Key::Home => {
                self.scroll.scroll_to(0.0);
                None
            }
            Key::End => {
                self.scroll.scroll_to(self.scroll.max_offset());
                None
            }
            Key::Backspace | Key::Delete | Key::SelectAll => None,
        }
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
        let mut interaction = std::mem::take(&mut self.interaction);
        interaction.begin_layout();
        self.layout(g, &style, &mut interaction, width, height, paint);
        interaction.end_layout();
        self.interaction = interaction;
        self.style = Some(style);
    }

    fn layout(
        &mut self,
        g: &Graphics,
        style: &Style,
        interaction: &mut Interaction<Id>,
        width: f32,
        height: f32,
        paint: bool,
    ) {
        let palette = &style.palette;
        let px = |value: f32| style.px(value);
        let hairline = style.hairline();
        let mut painter = Painter {
            g,
            style,
            interaction,
            paint,
            clip: None,
            backdrop: palette.surface,
        };

        // Sidebar.
        let sidebar_width = px(SIDEBAR_WIDTH);
        if paint {
            g.clear(palette.surface);
            let sidebar = Rect::new(0.0, 0.0, sidebar_width, height);
            g.fill_rect(sidebar_color(palette), sidebar);
            let edge = Rect::new(sidebar_width - hairline, 0.0, hairline, height);
            g.fill_rect(palette.border, edge);
        }
        let mut y = px(24.0);
        for page in Page::ALL {
            let rect = Rect::new(
                px(13.0),
                y,
                sidebar_width - hairline - px(23.0),
                px(NAVIGATION_HEIGHT),
            );
            let selected = page == self.page;
            let visual = painter.target(Id::Navigation(page), rect, true, selected, Cursor::Hand);
            if paint {
                navigation_item(g, style, rect, page, selected, visual);
            }
            y += px(NAVIGATION_HEIGHT) + px(NAVIGATION_GAP);
        }

        // Page heading.
        let left = sidebar_width + px(32.0);
        let content_width = (width - px(24.0) - left).max(0.0);
        let mut y = px(27.0);
        let title_height = g.measure(self.page.title(), &style.fonts.heading).1;
        if paint {
            g.text(
                self.page.title(),
                &style.fonts.heading,
                palette.text,
                Rect::new(left, y, 0.0, title_height),
                Align::Near,
                Align::Center,
            );
        }
        y += title_height + px(4.0);
        let description = self.page.description();
        let description_height = g.measure_wrapped(description, &style.fonts.lead, content_width);
        if paint {
            let rect = Rect::new(left, y, content_width, 0.0);
            g.text_wrapped(description, &style.fonts.lead, palette.muted, rect);
        }
        y += description_height + px(20.0);

        // The footer is measured first: the cards get what is left between.
        let footer_button = |id: Id, label: &str, padding: f32, primary: bool| Node::Button {
            id,
            label: label.to_string(),
            padding: (padding, 10.0),
            primary,
            enabled: true,
        };
        let restore = footer_button(Id::RestoreDefaults, "Restore defaults", 20.0, false);
        let cancel = footer_button(Id::Cancel, "Cancel", 28.0, false);
        let save = footer_button(Id::Save, "Save", 30.0, true);
        let restore_size = restore.measure(g, style);
        let cancel_size = cancel.measure(g, style);
        let save_size = save.measure(g, style);
        let buttons_top = height - px(22.0) - save_size.1;
        let line_top = buttons_top - px(26.0) - hairline;
        let viewport = Rect::new(left, y, content_width, (line_top - px(20.0) - y).max(0.0));

        // Cards, scrolled inside the viewport.
        let cards = self.cards();
        let note = (self.page == Page::Forecast).then(forecast_note);
        let gap = px(CARD_GAP);
        let note_size = note.as_ref().map(|note| note.measure(g, style));
        let measure = |card_width: f32| {
            let heights: Vec<f32> = cards
                .iter()
                .map(|card| card.height(g, style, card_width))
                .collect();
            let gaps = gap * heights.len().saturating_sub(1) as f32;
            let note_height = note_size.map_or(0.0, |size| gap + px(36.0) + size.1);
            let total = heights.iter().sum::<f32>() + gaps + note_height;
            (heights, total)
        };
        let full_width = (viewport.w - px(2.0)).max(0.0);
        let (mut heights, mut content_height) = measure(full_width);
        let mut card_width = full_width;
        if content_height > viewport.h {
            // Make room for the scroll bar, which may wrap text and so
            // change the heights.
            card_width = (full_width - px(SCROLLBAR_GUTTER)).max(0.0);
            (heights, content_height) = measure(card_width);
        }
        self.scroll.set_extent(content_height, viewport.h);
        painter.clip = Some(viewport);
        if paint {
            g.set_clip(viewport);
        }
        let mut card_y = viewport.y - self.scroll.offset().round();
        for (card, card_height) in cards.iter().zip(&heights) {
            let in_view = card_y < viewport.bottom() && card_y + card_height > viewport.y;
            // Cards out of view are still laid out so Tab can reach them.
            painter.paint = paint && in_view;
            card.place(&mut painter, viewport.x, card_y, card_width);
            card_y += card_height + gap;
        }
        painter.paint = paint;
        if let (Some(note), Some(size)) = (&note, note_size) {
            let rect = Rect::new(viewport.x, card_y + px(36.0), size.0, size.1);
            note.place(&mut painter, rect);
        }
        painter.clip = None;
        if paint {
            g.reset_clip();
        }

        // Footer.
        if paint {
            let line = Rect::new(left, line_top, content_width, hairline);
            g.fill_rect(palette.border, line);
        }
        let right = left + content_width;
        let save_x = right - save_size.0;
        let cancel_x = save_x - px(16.0) - cancel_size.0;
        let at = |x: f32, size: (f32, f32)| Rect::new(x, buttons_top, size.0, size.1);
        restore.place(&mut painter, at(left, restore_size));
        cancel.place(&mut painter, at(cancel_x, cancel_size));
        save.place(&mut painter, at(save_x, save_size));

        // Scroll bar.
        let bar_width = px(SCROLLBAR_WIDTH);
        let track = Rect::new(
            viewport.right() - bar_width,
            viewport.y,
            bar_width,
            viewport.h,
        );
        if let Some(thumb) = self.scroll.thumb(track, px(MIN_THUMB_LENGTH)) {
            // Easier to grab than the thin bar that is drawn.
            let grip = Rect::new(
                track.right() - px(SCROLLBAR_GUTTER),
                thumb.y,
                px(SCROLLBAR_GUTTER),
                thumb.h,
            );
            let visual = painter.target(Id::Scrollbar, grip, true, false, Cursor::Arrow);
            if paint {
                let dragging = self.scroll_drag.is_some();
                let visual = Visual {
                    pressed: visual.pressed || dragging,
                    ..visual
                };
                controls::scrollbar_thumb(g, style, thumb, visual);
            }
        }
        self.viewport = viewport;
        self.track = track;

        // An open dropdown's list goes over everything else.
        if let Some(open) = self.dropdown {
            let field = painter
                .interaction
                .target(open.owner)
                .filter(|field| field.enabled && field.visible.is_some())
                .map(|field| field.bounds);
            match field {
                Some(field) => {
                    let items = dropdown_items(open.owner);
                    let window = Rect::new(0.0, 0.0, width, height);
                    let list = controls::dropdown_list_rect(style, field, items.len(), window);
                    for index in 0..items.len() {
                        let item = controls::dropdown_item_rect(style, list, index);
                        painter.target(Id::DropdownItem(index), item, true, false, Cursor::Hand);
                    }
                    if paint {
                        let selected = self.dropdown_index(open.owner);
                        controls::dropdown_list(
                            g,
                            style,
                            list,
                            items,
                            selected,
                            Some(open.highlighted),
                        );
                    }
                }
                // Its dropdown scrolled away or was disabled.
                None => self.dropdown = None,
            }
        }
    }

    /// The cards of the page being shown.
    fn cards(&self) -> Vec<Card<'static, Id>> {
        match self.page {
            Page::General => self.general_cards(),
            Page::Usage => self.usage_cards(),
            Page::Forecast => self.forecast_cards(),
            Page::Appearance => self.appearance_cards(),
            Page::Popup => self.popup_cards(),
            Page::Updates => self.updates_cards(),
        }
    }

    fn general_cards(&self) -> Vec<Card<'static, Id>> {
        let seconds = |id: Id| Node::Row {
            gap: 10.0,
            children: vec![self.number_node(id, true), Node::label("seconds")],
        };
        vec![
            Card {
                icon: GLYPH_CLOCK,
                title: "Refresh while Codex is running",
                description: "Choose how often to update usage.",
                min_height: 98.0,
                split: Split::Options(240.0),
                fill: None,
                options: seconds(Id::RunningRefresh),
            },
            Card {
                icon: GLYPH_CLOCK,
                title: "Refresh while Codex is closed",
                description: "Zero pauses automatic refresh while idle.",
                min_height: 98.0,
                split: Split::Options(240.0),
                fill: None,
                options: seconds(Id::IdleRefresh),
            },
            Card {
                icon: GLYPH_REFRESH,
                title: "Startup",
                description: "Open the app automatically when you sign in.",
                min_height: 98.0,
                split: Split::Options(240.0),
                fill: None,
                options: Node::Toggle {
                    id: Id::StartWithWindows,
                    label: Some("Start with Windows"),
                    on: self.draft.start_with_windows,
                    enabled: true,
                },
            },
        ]
    }

    fn usage_cards(&self) -> Vec<Card<'static, Id>> {
        let draft = &self.draft;
        let metric = |id: Id, label: &'static str, metric: IconMetric| Node::Radio {
            id,
            label,
            selected: draft.icon_metric == metric,
            enabled: true,
        };
        let percent = |label: &str, id: Id| Node::Row {
            gap: 12.0,
            children: vec![
                Node::label(label),
                Node::Row {
                    gap: 8.0,
                    children: vec![
                        self.number_node(id, draft.threshold_notifications),
                        Node::label("%"),
                    ],
                },
            ],
        };
        let with_checkbox =
            |id: Id, label: &'static str, checked: bool, detail: Node<'static, Id>| Node::Column {
                gap: 12.0,
                align: HAlign::Start,
                children: vec![
                    Node::Checkbox {
                        id,
                        label,
                        checked,
                        enabled: true,
                    },
                    Node::Indent(18.0, Box::new(detail)),
                ],
            };
        let wide = Split::Share {
            text: 1.1,
            options: 1.4,
        };
        vec![
            Card {
                icon: GLYPH_DESKTOP,
                title: "Tray icon display",
                description: "Choose how the tray icon shows your usage.",
                min_height: 92.0,
                split: Split::Share {
                    text: 2.0,
                    options: 2.1,
                },
                fill: None,
                options: Node::Row {
                    gap: 10.0,
                    children: vec![
                        metric(Id::MetricWeekly, "Weekly", IconMetric::Weekly),
                        metric(Id::MetricFiveHour, "5-hour", IconMetric::FiveHour),
                        metric(Id::MetricAuto, "Auto", IconMetric::Auto),
                    ],
                },
            },
            Card {
                icon: GLYPH_ALERT,
                title: "Usage threshold alerts",
                description: "Get alerted when usage reaches these levels.",
                min_height: 108.0,
                split: wide,
                fill: None,
                options: with_checkbox(
                    Id::ThresholdAlerts,
                    "Enable usage threshold alerts",
                    draft.threshold_notifications,
                    Node::Row {
                        gap: 25.0,
                        children: vec![percent("Critical", Id::Critical), percent("Low", Id::Low)],
                    },
                ),
            },
            Card {
                icon: GLYPH_REFRESH,
                title: "Reset expiry reminders",
                description: "Get a reminder before a reset credit expires.",
                min_height: 104.0,
                split: wide,
                fill: None,
                options: with_checkbox(
                    Id::ReminderEnabled,
                    "Reset expiry reminders",
                    draft.reset_expiry_reminders,
                    Node::Row {
                        gap: 12.0,
                        children: vec![
                            self.number_node(Id::ReminderHours, draft.reset_expiry_reminders),
                            Node::label("hours before expiry"),
                        ],
                    },
                ),
            },
            Card {
                icon: GLYPH_CLOCK,
                title: "Auto-use resets",
                description: "Use an expiring reset credit when there is usage to reset.",
                min_height: 108.0,
                split: wide,
                fill: None,
                options: with_checkbox(
                    Id::AutoUseEnabled,
                    "Auto-use resets",
                    draft.auto_redeem_reset_credits,
                    Node::Row {
                        gap: 16.0,
                        children: vec![
                            Node::label("Lead time"),
                            Node::Row {
                                gap: 12.0,
                                children: vec![
                                    self.number_node(
                                        Id::AutoUseMinutes,
                                        draft.auto_redeem_reset_credits,
                                    ),
                                    Node::label("minutes"),
                                ],
                            },
                        ],
                    },
                ),
            },
        ]
    }

    fn forecast_cards(&self) -> Vec<Card<'static, Id>> {
        let draft = &self.draft;
        let learned = draft.forecast_method == ForecastMethod::LearnedRoutine;
        let split = Split::Share {
            text: 1.1,
            options: 1.2,
        };
        let switch_row = |label: &str, id: Id, on: bool, enabled: bool| {
            Node::Spread(
                Box::new(Node::label(label)),
                Box::new(Node::Toggle {
                    id,
                    label: None,
                    on,
                    enabled,
                }),
            )
        };
        // Highlighting needs a projection to draw on and learned quiet hours
        // to highlight.
        let can_highlight = draft.show_projected_usage
            && learned
            && draft.quiet_hours_mode == QuietHoursMode::Automatic;
        vec![
            Card {
                icon: Icon::Bars,
                title: "Forecast method",
                description: "Choose how future usage is estimated.",
                min_height: 108.0,
                split,
                fill: Some(12.0),
                options: Node::Segments {
                    options: vec![
                        (Id::ForecastLearned, "Learned routine"),
                        (Id::ForecastRecent, "Recent pace"),
                    ],
                    selected: usize::from(!learned),
                },
            },
            Card {
                icon: GLYPH_CALENDAR,
                title: "Quiet hours",
                description: "Use recurring quiet periods in weekly projections.",
                min_height: 104.0,
                split,
                fill: None,
                options: Node::Dropdown {
                    id: Id::QuietHours,
                    text: QUIET_HOURS_ITEMS[self.dropdown_index(Id::QuietHours)],
                    width: 310.0,
                    open: self.is_open(Id::QuietHours),
                    enabled: learned,
                },
            },
            Card {
                icon: GLYPH_TREND,
                title: "Graph display",
                description: "Choose what to show in usage graphs.",
                min_height: 120.0,
                split,
                fill: Some(16.0),
                options: Node::Column {
                    gap: 12.0,
                    align: HAlign::Stretch,
                    children: vec![
                        switch_row(
                            "Show projected usage",
                            Id::ShowProjection,
                            draft.show_projected_usage,
                            true,
                        ),
                        switch_row(
                            "Highlight quiet hours",
                            Id::HighlightQuiet,
                            draft.highlight_quiet_hours,
                            can_highlight,
                        ),
                    ],
                },
            },
        ]
    }

    fn appearance_cards(&self) -> Vec<Card<'static, Id>> {
        let draft = &self.draft;
        let background = self.effective_tray_background();
        let background_hex = format_color(background);
        let box_text = if draft.use_custom_tray_box_color {
            background_hex
        } else {
            format!("Auto ({background_hex})")
        };
        let split = Split::Options(260.0);
        vec![
            Card {
                icon: GLYPH_COLOR,
                title: "Theme",
                description: "Choose the app\u{2019}s color scheme.",
                min_height: 98.0,
                split,
                fill: None,
                options: Node::Dropdown {
                    id: Id::Theme,
                    text: THEME_ITEMS[self.dropdown_index(Id::Theme)],
                    width: 240.0,
                    open: self.is_open(Id::Theme),
                    enabled: true,
                },
            },
            Card {
                icon: GLYPH_DESKTOP,
                title: "Tray background",
                description: "Choose whether to show a background and customize its color.",
                min_height: 108.0,
                split,
                fill: Some(0.0),
                options: Node::Column {
                    gap: 10.0,
                    align: HAlign::Start,
                    children: vec![
                        Node::Toggle {
                            id: Id::TrayBackground,
                            label: Some("Show tray background"),
                            on: draft.show_tray_box,
                            enabled: true,
                        },
                        Node::Row {
                            gap: 8.0,
                            children: vec![
                                Node::Swatch {
                                    id: Id::TrayBoxColor,
                                    color: background,
                                    enabled: draft.show_tray_box,
                                },
                                Node::Button {
                                    id: Id::TrayBoxAuto,
                                    label: "Auto".to_string(),
                                    padding: (14.0, 6.0),
                                    primary: false,
                                    enabled: draft.show_tray_box && draft.use_custom_tray_box_color,
                                },
                                Node::label(&box_text),
                            ],
                        },
                    ],
                },
            },
            Card {
                icon: GLYPH_COLOR,
                title: "Tray text color",
                description: "Choose the color used for the tray percentage.",
                min_height: 98.0,
                split,
                fill: Some(0.0),
                options: Node::Row {
                    gap: 14.0,
                    children: vec![
                        Node::Swatch {
                            id: Id::TrayTextColor,
                            color: draft.tray_text_color,
                            enabled: true,
                        },
                        Node::label(&format_color(draft.tray_text_color)),
                    ],
                },
            },
            Card {
                icon: GLYPH_DESKTOP,
                title: "Tray preview",
                description: "Preview the icon and check its contrast.",
                min_height: 98.0,
                split,
                fill: Some(0.0),
                options: self.preview_options(background),
            },
        ]
    }

    fn preview_options(&self, background: u32) -> Node<'static, Id> {
        let draft = &self.draft;
        let text = self.preview_percent.to_string();
        let fill = draft.show_tray_box.then_some(background);
        let ink = draft.tray_text_color;
        let dark = self.dark();
        let (contrast, low_contrast) = self.contrast_guidance(background);
        Node::Column {
            gap: 8.0,
            align: HAlign::Start,
            children: vec![
                Node::Row {
                    gap: 12.0,
                    children: vec![
                        Node::Custom {
                            size: (PREVIEW_TILE, PREVIEW_TILE),
                            paint: Box::new(move |g, style, rect| {
                                // Stands in for the taskbar the icon sits on.
                                let backdrop = if dark {
                                    PREVIEW_BACKDROP_DARK
                                } else {
                                    PREVIEW_BACKDROP_LIGHT
                                };
                                let radius = style.px(4.0);
                                g.fill_round_rect(backdrop, rect, radius);
                                g.stroke_round_rect(style.palette.border, rect, radius);
                                if let Some(icon) = IconBitmap::render(&text, fill, ink) {
                                    let side = style.px(PREVIEW_ICON);
                                    let inset = ((rect.w - side) / 2.0).round();
                                    let area =
                                        Rect::new(rect.x + inset, rect.y + inset, side, side);
                                    g.icon(&icon, area);
                                }
                            }),
                        },
                        Node::Row {
                            gap: 10.0,
                            children: vec![
                                Node::label("Remaining"),
                                Node::Row {
                                    gap: 5.0,
                                    children: vec![
                                        self.number_node(Id::PreviewRemaining, true),
                                        Node::label("%"),
                                    ],
                                },
                            ],
                        },
                    ],
                },
                Node::Text {
                    text: contrast,
                    tone: if low_contrast {
                        Tone::Warning
                    } else {
                        Tone::Text
                    },
                    wrap: Some(260.0),
                },
            ],
        }
    }

    fn popup_cards(&self) -> Vec<Card<'static, Id>> {
        let draft = &self.draft;
        let card = |icon: Icon,
                    title: &'static str,
                    description: &'static str,
                    id: Id,
                    label: &'static str,
                    on: bool| Card {
            icon,
            title,
            description,
            min_height: 98.0,
            split: Split::Options(270.0),
            fill: None,
            options: Node::Toggle {
                id,
                label: Some(label),
                on,
                enabled: true,
            },
        };
        vec![
            card(
                GLYPH_CLOCK,
                "Updated time",
                "Show when usage was last refreshed.",
                Id::ShowUpdatedTime,
                "Show updated time",
                draft.show_last_updated,
            ),
            card(
                GLYPH_CALENDAR,
                "Reset times",
                "Show when each usage limit resets.",
                Id::ShowResetTimes,
                "Show reset times",
                draft.show_reset_times,
            ),
            card(
                Icon::Bars,
                "Usage bar colors",
                "Use threshold colors for remaining usage.",
                Id::ColorUsageBars,
                "Color usage bars",
                draft.color_bars,
            ),
        ]
    }

    fn updates_cards(&self) -> Vec<Card<'static, Id>> {
        let mut children = vec![Node::label(&format!("Version {}", self.version))];
        if !self.update_status.is_empty() {
            children.push(Node::Text {
                text: self.update_status.clone(),
                tone: Tone::Text,
                wrap: Some(250.0),
            });
        }
        children.push(Node::Button {
            id: Id::CheckUpdates,
            label: if self.checking {
                self.busy_label.clone()
            } else {
                CHECK_UPDATES_LABEL.to_string()
            },
            padding: (11.0, 5.0),
            primary: false,
            enabled: !self.checking,
        });
        vec![Card {
            icon: GLYPH_REFRESH,
            title: "Application updates",
            description: "Updates install automatically and restart the app.",
            min_height: 105.0,
            split: Split::Options(250.0),
            fill: None,
            options: Node::Column {
                gap: 10.0,
                align: HAlign::Start,
                children,
            },
        }]
    }

    fn number_node(&self, id: Id, enabled: bool) -> Node<'static, Id> {
        let (text, caret) = match &self.edit {
            Some((editing, edit)) if *editing == id => (
                edit.text().to_string(),
                Some(Caret {
                    index: edit.caret(),
                    all_selected: edit.all_selected(),
                    visible: self.caret_visible,
                }),
            ),
            _ => (self.number(id).to_string(), None),
        };
        Node::Number {
            id,
            text,
            caret,
            enabled,
        }
    }

    /// The value of a numeric field.
    fn number(&self, id: Id) -> i32 {
        match id {
            Id::Critical => self.draft.critical_threshold,
            Id::Low => self.draft.low_threshold,
            Id::ReminderHours => self.draft.reset_expiry_lead_hours,
            Id::AutoUseMinutes => self.draft.auto_redeem_lead_minutes,
            Id::RunningRefresh => self.draft.refresh_seconds,
            Id::IdleRefresh => self.draft.idle_refresh_seconds,
            Id::PreviewRemaining => self.preview_percent,
            _ => 0,
        }
    }

    /// Sets a numeric field, then adjusts the fields that depend on it so
    /// the pairs stay valid.
    fn set_number(&mut self, id: Id, value: i32) {
        let Some((min, max)) = id.range() else {
            return;
        };
        let value = value.clamp(min, max);
        let draft = &mut self.draft;
        match id {
            Id::Critical => {
                draft.critical_threshold = value;
                draft.low_threshold = draft.low_threshold.max(value);
            }
            Id::Low => {
                draft.low_threshold = value;
                draft.critical_threshold = draft.critical_threshold.min(value);
            }
            Id::ReminderHours => draft.reset_expiry_lead_hours = value,
            Id::AutoUseMinutes => draft.auto_redeem_lead_minutes = value,
            Id::RunningRefresh => {
                draft.refresh_seconds = value;
                draft.keep_idle_refresh_valid();
            }
            Id::IdleRefresh => {
                draft.idle_refresh_seconds = value;
                draft.keep_idle_refresh_valid();
            }
            Id::PreviewRemaining => self.preview_percent = value,
            _ => {}
        }
    }

    /// Makes the typed text the field's value. Text that is not a number
    /// leaves the value as it was.
    fn commit_edit(&mut self) {
        if let Some((id, edit)) = self.edit.take() {
            if let Some(value) = id.range().and_then(|(min, max)| edit.value(min, max)) {
                self.set_number(id, value);
            }
        }
    }

    fn begin_edit(&mut self, id: Id, select_all: bool) {
        self.edit = Some((id, NumberEdit::new(self.number(id), select_all)));
        self.caret_visible = true;
    }

    /// Changes a numeric field by `delta`, starting from what is typed in it.
    fn step(&mut self, id: Id, delta: i32) {
        let Some((min, max)) = id.range() else {
            return;
        };
        let typed = self
            .edit
            .as_ref()
            .filter(|(editing, _)| *editing == id)
            .and_then(|(_, edit)| edit.value(min, max));
        let value = typed.unwrap_or_else(|| self.number(id));
        self.set_number(id, value.saturating_add(delta));
        self.begin_edit(id, true);
    }

    /// Keeps "the focused numeric field is the one being edited" true after
    /// the focus moved.
    fn sync_edit_with_focus(&mut self, select_all: bool) {
        let focused = self.interaction.focused();
        if self.edit.as_ref().map(|(id, _)| *id) != focused {
            self.commit_edit();
        }
        if let Some(id) = focused.filter(|id| id.range().is_some()) {
            if self.edit.is_none() {
                self.begin_edit(id, select_all);
            }
        }
    }

    fn press_number(&mut self, g: &Graphics, id: Id, x: f32, y: f32, double_click: bool) {
        let Some(field) = self.interaction.target(id).map(|field| field.bounds) else {
            return;
        };
        let spin = self
            .style
            .as_ref()
            .and_then(|style| controls::number_spin_at(style, field, x, y));
        if self
            .edit
            .as_ref()
            .is_some_and(|(editing, _)| *editing != id)
        {
            self.commit_edit();
        }
        if let Some(direction) = spin {
            self.step(id, direction);
            return;
        }
        if self.edit.is_none() {
            self.begin_edit(id, false);
        }
        if let (Some((_, edit)), Some(style)) = (&mut self.edit, &self.style) {
            if double_click {
                edit.select_all();
            } else {
                let caret = controls::number_caret_at(g, style, field, edit.text(), x);
                edit.set_caret(caret);
            }
        }
        self.caret_visible = true;
    }

    /// Handles a key while a numeric field is being edited. Returns false
    /// for keys the field does not use.
    fn edit_key(&mut self, key: Key) -> bool {
        let Some((id, edit)) = &mut self.edit else {
            return false;
        };
        let id = *id;
        match key {
            Key::Left => edit.move_caret(CaretMove::Left),
            Key::Right => edit.move_caret(CaretMove::Right),
            Key::Home => edit.move_caret(CaretMove::Home),
            Key::End => edit.move_caret(CaretMove::End),
            Key::Backspace => edit.backspace(),
            Key::Delete => edit.delete(),
            Key::SelectAll => edit.select_all(),
            Key::Up => self.step(id, 1),
            Key::Down => self.step(id, -1),
            Key::PageUp => self.step(id, 10),
            Key::PageDown => self.step(id, -10),
            Key::Enter => {
                self.commit_edit();
                self.begin_edit(id, true);
            }
            Key::Space => {}
            Key::Tab | Key::Escape => return false,
        }
        true
    }

    fn dropdown_key(&mut self, open: OpenDropdown, key: Key, shift: bool) {
        let last = dropdown_items(open.owner).len().saturating_sub(1);
        let highlight = |index: usize| {
            Some(OpenDropdown {
                highlighted: index,
                ..open
            })
        };
        match key {
            Key::Up => self.dropdown = highlight(open.highlighted.saturating_sub(1)),
            Key::Down => self.dropdown = highlight((open.highlighted + 1).min(last)),
            Key::Home | Key::PageUp => self.dropdown = highlight(0),
            Key::End | Key::PageDown => self.dropdown = highlight(last),
            Key::Enter | Key::Space => {
                self.set_dropdown_index(open.owner, open.highlighted);
                self.dropdown = None;
            }
            Key::Escape => self.dropdown = None,
            Key::Tab => {
                self.dropdown = None;
                self.interaction.focus_next(shift);
                self.sync_edit_with_focus(true);
                self.reveal_focus();
            }
            Key::Left | Key::Right | Key::Backspace | Key::Delete | Key::SelectAll => {}
        }
    }

    /// Handles an arrow key outside numeric fields and open dropdowns.
    fn arrow(&mut self, focused: Option<Id>, forward: bool, vertical: bool) {
        let moved = |index: usize, count: usize| {
            if forward {
                (index + 1).min(count.saturating_sub(1))
            } else {
                index.saturating_sub(1)
            }
        };
        let group = focused.and_then(|id| {
            [&METRIC_GROUP[..], &FORECAST_GROUP[..]]
                .into_iter()
                .find(|group| group.contains(&id))
                .map(|group| (group, id))
        });
        match focused {
            Some(Id::Navigation(page)) if vertical => {
                let index = Page::ALL
                    .iter()
                    .position(|other| *other == page)
                    .unwrap_or(0);
                let page = Page::ALL[moved(index, Page::ALL.len())];
                self.select_page(page);
                self.interaction.set_focus(Some(Id::Navigation(page)), true);
            }
            Some(id @ (Id::QuietHours | Id::Theme)) if vertical => {
                let count = dropdown_items(id).len();
                self.set_dropdown_index(id, moved(self.dropdown_index(id), count));
            }
            _ => match group {
                // Arrows move the choice within a group, and the focus with it.
                Some((group, id)) => {
                    let index = group.iter().position(|other| *other == id).unwrap_or(0);
                    let next = group[moved(index, group.len())];
                    self.activate(next);
                    self.interaction.set_focus(Some(next), true);
                }
                None if vertical => {
                    let step = self.px(SCROLL_STEP);
                    self.scroll.scroll_by(if forward { step } else { -step });
                }
                None => {}
            },
        }
    }

    /// Does what clicking a control, or pressing Space on it, does.
    fn activate(&mut self, id: Id) -> Option<Command> {
        let draft = &mut self.draft;
        match id {
            Id::Navigation(page) => self.select_page(page),
            Id::MetricWeekly => draft.icon_metric = IconMetric::Weekly,
            Id::MetricFiveHour => draft.icon_metric = IconMetric::FiveHour,
            Id::MetricAuto => draft.icon_metric = IconMetric::Auto,
            Id::ThresholdAlerts => draft.threshold_notifications = !draft.threshold_notifications,
            Id::ReminderEnabled => draft.reset_expiry_reminders = !draft.reset_expiry_reminders,
            Id::AutoUseEnabled => {
                draft.auto_redeem_reset_credits = !draft.auto_redeem_reset_credits;
            }
            Id::ForecastLearned => draft.forecast_method = ForecastMethod::LearnedRoutine,
            Id::ForecastRecent => draft.forecast_method = ForecastMethod::RecentPace,
            Id::ShowProjection => draft.show_projected_usage = !draft.show_projected_usage,
            Id::HighlightQuiet => draft.highlight_quiet_hours = !draft.highlight_quiet_hours,
            Id::StartWithWindows => draft.start_with_windows = !draft.start_with_windows,
            Id::TrayBackground => draft.show_tray_box = !draft.show_tray_box,
            Id::ShowUpdatedTime => draft.show_last_updated = !draft.show_last_updated,
            Id::ShowResetTimes => draft.show_reset_times = !draft.show_reset_times,
            Id::ColorUsageBars => draft.color_bars = !draft.color_bars,
            Id::TrayBoxAuto => draft.use_custom_tray_box_color = false,
            Id::QuietHours | Id::Theme => {
                self.dropdown = match self.dropdown {
                    Some(open) if open.owner == id => None,
                    _ => Some(OpenDropdown {
                        owner: id,
                        highlighted: self.dropdown_index(id),
                    }),
                };
            }
            Id::DropdownItem(index) => {
                if let Some(open) = self.dropdown.take() {
                    self.set_dropdown_index(open.owner, index);
                }
            }
            Id::RestoreDefaults => self.restore_defaults(),
            Id::TrayBoxColor => return Some(Command::PickColor(ColorTarget::TrayBox)),
            Id::TrayTextColor => return Some(Command::PickColor(ColorTarget::TrayText)),
            Id::CheckUpdates => return Some(Command::CheckUpdates),
            Id::Cancel => return Some(Command::Close),
            Id::Save => return Some(Command::Save),
            // Numeric fields react to the press itself, and the scroll bar
            // to dragging.
            Id::Critical
            | Id::Low
            | Id::ReminderHours
            | Id::AutoUseMinutes
            | Id::RunningRefresh
            | Id::IdleRefresh
            | Id::PreviewRemaining
            | Id::Scrollbar => {}
        }
        None
    }

    fn select_page(&mut self, page: Page) {
        if page != self.page {
            self.commit_edit();
            self.dropdown = None;
            self.page = page;
            self.scroll.scroll_to(0.0);
        }
    }

    /// Puts every control back to its default without saving anything.
    fn restore_defaults(&mut self) {
        self.edit = None;
        self.dropdown = None;
        self.draft = Draft::from_settings(&Settings::default());
        self.preview_percent = DEFAULT_PREVIEW_PERCENT;
    }

    fn is_open(&self, id: Id) -> bool {
        self.dropdown.is_some_and(|open| open.owner == id)
    }

    fn dropdown_index(&self, id: Id) -> usize {
        match id {
            Id::QuietHours => usize::from(self.draft.quiet_hours_mode == QuietHoursMode::Off),
            Id::Theme => THEMES
                .iter()
                .position(|theme| *theme == self.draft.theme)
                .unwrap_or(0),
            _ => 0,
        }
    }

    fn set_dropdown_index(&mut self, id: Id, index: usize) {
        match id {
            Id::QuietHours => {
                self.draft.quiet_hours_mode = if index == 1 {
                    QuietHoursMode::Off
                } else {
                    QuietHoursMode::Automatic
                };
            }
            Id::Theme => {
                if let Some(theme) = THEMES.get(index) {
                    self.draft.theme = *theme;
                }
            }
            _ => {}
        }
    }

    /// The tray box color the preview shows: the fixed color, or the color
    /// for the previewed percentage under the thresholds being edited.
    fn effective_tray_background(&self) -> u32 {
        let draft = &self.draft;
        if draft.use_custom_tray_box_color {
            draft.tray_box_color
        } else {
            threshold_color(
                self.preview_percent,
                draft.critical_threshold,
                draft.low_threshold,
            )
        }
    }

    /// The contrast note under the preview, and whether it is a warning.
    fn contrast_guidance(&self, background: u32) -> (String, bool) {
        if !self.draft.show_tray_box {
            return ("Text must contrast with the taskbar.".to_string(), false);
        }
        let ratio = contrast_ratio(self.draft.tray_text_color, background);
        if ratio >= MIN_CONTRAST {
            return (format!("Contrast {ratio:.2}:1"), false);
        }
        let black = contrast_ratio(0xFF00_0000, background);
        let white = contrast_ratio(0xFFFF_FFFF, background);
        let better = if black >= white {
            0xFF00_0000
        } else {
            0xFFFF_FFFF
        };
        (
            format!("Low contrast {ratio:.2}:1. Try {}.", format_color(better)),
            true,
        )
    }

    /// Scrolls the focused control into view if it is in the scrolled area.
    fn reveal_focus(&mut self) {
        let Some(bounds) = self
            .interaction
            .focused()
            .filter(|id| id.scrolls())
            .and_then(|id| self.interaction.target(id))
            .map(|target| target.bounds)
        else {
            return;
        };
        let margin = self.px(REVEAL_MARGIN);
        let origin = self.viewport.y - self.scroll.offset();
        self.scroll.reveal(
            bounds.y - origin - margin,
            bounds.bottom() - origin + margin,
        );
    }

    /// A length at 100% scale in device pixels at the scale last laid out.
    fn px(&self, value: f32) -> f32 {
        self.style.as_ref().map_or(value, |style| style.px(value))
    }
}

fn dropdown_items(id: Id) -> &'static [&'static str] {
    match id {
        Id::QuietHours => &QUIET_HOURS_ITEMS,
        Id::Theme => &THEME_ITEMS,
        _ => &[],
    }
}

/// The shade of the sidebar, a step away from the page behind the cards.
fn sidebar_color(palette: &Palette) -> u32 {
    if palette.dark {
        mix(palette.surface, 0xFF00_0000, 0.12)
    } else {
        mix(palette.surface, palette.border, 0.16)
    }
}

/// The color for the window's title bar, matching the sidebar under it, or
/// `None` to keep the system's own title bar as the light theme does.
pub fn caption_color(dark: bool) -> Option<u32> {
    dark.then(|| sidebar_color(&Palette::new(true)))
}

fn navigation_item(
    g: &Graphics,
    style: &Style,
    rect: Rect,
    page: Page,
    selected: bool,
    visual: Visual,
) {
    let palette = &style.palette;
    let radius = style.px(7.0);
    if selected {
        g.fill_round_rect(palette.selected, rect, radius);
        let bar = Rect::new(rect.x, rect.y, style.px(5.0), rect.h);
        g.fill_round_rect(palette.primary_edge, bar, style.px(2.0));
    } else if visual.hovered || visual.pressed {
        g.fill_round_rect(palette.hover, rect, radius);
    }
    let icon_box = Rect::new(rect.x + style.px(18.0), rect.y, style.px(28.0), rect.h);
    controls::icon(
        g,
        style,
        page.icon(),
        IconSize::Navigation,
        palette.text,
        icon_box,
    );
    let text = Rect::new(icon_box.right() + style.px(10.0), rect.y, 0.0, rect.h);
    g.text(
        page.title(),
        &style.fonts.navigation,
        palette.text,
        text,
        Align::Near,
        Align::Center,
    );
    if visual.focused {
        controls::focus_ring(g, style, rect, radius);
    }
}

/// The reminder under the forecast cards.
fn forecast_note() -> Node<'static, Id> {
    Node::Row {
        gap: 10.0,
        children: vec![
            Node::Custom {
                size: (22.0, 22.0),
                paint: Box::new(|g, style, rect| {
                    let color = style.palette.muted;
                    controls::icon(g, style, GLYPH_INFO, IconSize::Small, color, rect);
                }),
            },
            Node::label("These are estimates, not guarantees."),
        ],
    }
}

/// The WCAG contrast ratio of two colors, from 1 (none) to 21.
fn contrast_ratio(first: u32, second: u32) -> f64 {
    let (first, second) = (relative_luminance(first), relative_luminance(second));
    (first.max(second) + 0.05) / (first.min(second) + 0.05)
}

fn relative_luminance(argb: u32) -> f64 {
    let channel = |shift: u32| {
        let value = f64::from((argb >> shift) & 0xFF) / 255.0;
        if value <= 0.03928 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::Target;
    use crate::gfx::tests::{sample_path, Canvas};
    use crate::gfx::GdiPlus;

    /// Default settings with a few values changed.
    fn settings(change: impl FnOnce(&mut Settings)) -> Settings {
        let mut settings = Settings::default();
        change(&mut settings);
        settings
    }

    /// A view with a surface to lay it out on. The fields are in the order
    /// they must be dropped in: fonts before GDI+ shuts down.
    struct Harness {
        view: View,
        canvas: Canvas,
        scale: f32,
        size: (f32, f32),
        _gdiplus: GdiPlus,
    }

    impl Harness {
        fn new() -> Self {
            Self::with(&Settings::default(), 1.0, CLIENT_SIZE)
        }

        fn with(settings: &Settings, scale: f32, size: (f32, f32)) -> Self {
            let gdiplus = GdiPlus::start().unwrap();
            let mut harness = Self {
                view: View::new(settings, true),
                canvas: Canvas::new(8, 8),
                scale,
                size: ((size.0 * scale).round(), (size.1 * scale).round()),
                _gdiplus: gdiplus,
            };
            harness.layout();
            harness
        }

        fn layout(&mut self) {
            let g = self.canvas.graphics();
            self.view
                .render(&g, self.scale, self.size.0, self.size.1, false);
        }

        fn target(&self, id: Id) -> Option<Target<Id>> {
            self.view.interaction.target(id).copied()
        }

        fn enabled(&self, id: Id) -> bool {
            self.target(id).is_some_and(|target| target.enabled)
        }

        fn click_at(&mut self, x: f32, y: f32) -> Option<Command> {
            let g = self.canvas.graphics();
            self.view.pointer_pressed(&g, x, y, false);
            let command = self.view.pointer_released(x, y);
            self.layout();
            command
        }

        fn click(&mut self, id: Id) -> Option<Command> {
            let bounds = self.target(id).expect("control is on the page").bounds;
            // Left of center, clear of a numeric field's spin strip.
            self.click_at(bounds.x + bounds.w * 0.3, bounds.y + bounds.h / 2.0)
        }

        fn key(&mut self, key: Key) -> Option<Command> {
            let command = self.view.key(key, false);
            self.layout();
            command
        }

        fn shift_tab(&mut self) {
            self.view.key(Key::Tab, true);
            self.layout();
        }

        fn type_text(&mut self, text: &str) {
            for character in text.chars() {
                self.view.character(character);
            }
            self.layout();
        }

        /// Replaces the number in the field that is being edited.
        fn retype(&mut self, text: &str) {
            self.key(Key::SelectAll);
            self.type_text(text);
        }

        fn open(&mut self, page: Page) {
            self.click(Id::Navigation(page));
        }

        fn focused(&self) -> Option<Id> {
            self.view.interaction.focused()
        }

        /// The controls of the page itself, without navigation and footer.
        fn page_controls(&self) -> Vec<Id> {
            self.view
                .interaction
                .targets()
                .iter()
                .map(|target| target.id)
                .filter(|id| id.scrolls())
                .collect()
        }
    }

    #[test]
    fn the_window_opens_on_usage_with_nothing_to_save() {
        let mut harness = Harness::new();
        assert_eq!(harness.view.page, Page::Usage);
        assert!(!harness.view.has_unsaved_changes());
        assert_eq!(harness.focused(), Some(Id::Navigation(Page::Usage)));

        // Navigation shows exactly the selected page's controls.
        let expected: [(Page, &[Id], usize); 6] = [
            (
                Page::General,
                &[Id::RunningRefresh, Id::IdleRefresh, Id::StartWithWindows],
                3,
            ),
            (
                Page::Usage,
                &[
                    Id::MetricWeekly,
                    Id::MetricFiveHour,
                    Id::MetricAuto,
                    Id::ThresholdAlerts,
                    Id::Critical,
                    Id::Low,
                    Id::ReminderEnabled,
                    Id::ReminderHours,
                    Id::AutoUseEnabled,
                    Id::AutoUseMinutes,
                ],
                4,
            ),
            (
                Page::Forecast,
                &[
                    Id::ForecastLearned,
                    Id::ForecastRecent,
                    Id::QuietHours,
                    Id::ShowProjection,
                    Id::HighlightQuiet,
                ],
                3,
            ),
            (
                Page::Appearance,
                &[
                    Id::Theme,
                    Id::TrayBackground,
                    Id::TrayBoxColor,
                    Id::TrayBoxAuto,
                    Id::TrayTextColor,
                    Id::PreviewRemaining,
                ],
                4,
            ),
            (
                Page::Popup,
                &[Id::ShowUpdatedTime, Id::ShowResetTimes, Id::ColorUsageBars],
                3,
            ),
            (Page::Updates, &[Id::CheckUpdates], 1),
        ];
        for (page, controls, cards) in expected {
            harness.open(page);
            assert_eq!(harness.view.page, page);
            assert_eq!(harness.page_controls(), controls, "{page:?}");
            assert_eq!(harness.view.cards().len(), cards, "{page:?}");
        }
        // Looking around changes nothing that needs saving.
        assert!(!harness.view.has_unsaved_changes());
    }

    #[test]
    fn dependent_controls_follow_what_they_depend_on() {
        let mut harness = Harness::new();
        // Thresholds, reminder lead time and auto-use lead time.
        assert!(!harness.enabled(Id::Critical) && !harness.enabled(Id::Low));
        harness.click(Id::ThresholdAlerts);
        assert!(harness.enabled(Id::Critical) && harness.enabled(Id::Low));
        assert!(harness.enabled(Id::ReminderHours));
        harness.click(Id::ReminderEnabled);
        assert!(!harness.enabled(Id::ReminderHours));
        assert!(!harness.enabled(Id::AutoUseMinutes));
        harness.click(Id::AutoUseEnabled);
        assert!(harness.enabled(Id::AutoUseMinutes));

        // Quiet hours need the learned forecast; highlighting them also needs
        // the projection and the learned quiet-hour profile.
        harness.open(Page::Forecast);
        harness.click(Id::HighlightQuiet);
        assert!(harness.view.draft.highlight_quiet_hours);
        harness.click(Id::ForecastRecent);
        assert!(!harness.enabled(Id::QuietHours) && !harness.enabled(Id::HighlightQuiet));
        harness.click(Id::ForecastLearned);
        assert!(harness.enabled(Id::QuietHours) && harness.enabled(Id::HighlightQuiet));
        harness.click(Id::QuietHours);
        harness.click(Id::DropdownItem(1));
        assert_eq!(harness.view.draft.quiet_hours_mode, QuietHoursMode::Off);
        assert!(harness.enabled(Id::QuietHours) && !harness.enabled(Id::HighlightQuiet));
        harness.click(Id::QuietHours);
        harness.click(Id::DropdownItem(0));
        assert!(harness.enabled(Id::HighlightQuiet));
        harness.click(Id::ShowProjection);
        assert!(!harness.enabled(Id::HighlightQuiet));

        // Tray background colors need a background; "Auto" needs a fixed color.
        harness.open(Page::Appearance);
        assert!(harness.enabled(Id::TrayBoxColor) && !harness.enabled(Id::TrayBoxAuto));
        harness.view.begin_color_choice(ColorTarget::TrayBox);
        harness
            .view
            .finish_color_choice(ColorTarget::TrayBox, Some(0x0012_3456));
        harness.layout();
        assert!(harness.enabled(Id::TrayBoxAuto));
        assert_eq!(harness.view.draft.tray_box_color, 0xFF12_3456);
        harness.click(Id::TrayBackground);
        assert!(!harness.enabled(Id::TrayBoxColor) && !harness.enabled(Id::TrayBoxAuto));
        harness.click(Id::TrayBackground);
        harness.click(Id::TrayBoxAuto);
        assert!(!harness.view.draft.use_custom_tray_box_color);
        assert!(!harness.enabled(Id::TrayBoxAuto));
        // The color is remembered for the next time a fixed color is wanted.
        assert_eq!(harness.view.draft.tray_box_color, 0xFF12_3456);

        // The update button is unavailable while a check is running.
        harness.open(Page::Updates);
        assert!(harness.enabled(Id::CheckUpdates));
        harness.view.set_status(Status {
            checking: Some(true),
            ..Status::default()
        });
        harness.layout();
        assert!(!harness.enabled(Id::CheckUpdates));
    }

    #[test]
    fn paired_numbers_keep_each_other_valid() {
        let mut harness = Harness::new();
        let thresholds = |harness: &Harness| {
            let draft = &harness.view.draft;
            (draft.critical_threshold, draft.low_threshold)
        };
        harness.click(Id::ThresholdAlerts);
        harness.click(Id::Critical);
        harness.retype("45");
        harness.key(Key::Enter);
        assert_eq!(thresholds(&harness), (45, 45));
        harness.click(Id::Low);
        harness.retype("10");
        harness.key(Key::Tab);
        assert_eq!(thresholds(&harness), (10, 10));

        // An idle interval is off or at least the running interval.
        harness.open(Page::General);
        harness.click(Id::IdleRefresh);
        harness.retype("60");
        harness.key(Key::Enter);
        assert_eq!(harness.view.draft.idle_refresh_seconds, 300);
        harness.click(Id::RunningRefresh);
        harness.retype("900");
        harness.key(Key::Enter);
        assert_eq!(harness.view.draft.idle_refresh_seconds, 900);
        harness.click(Id::IdleRefresh);
        harness.retype("0");
        harness.key(Key::Enter);
        assert_eq!(harness.view.draft.idle_refresh_seconds, 0);
        harness.click(Id::RunningRefresh);
        harness.retype("1200");
        harness.key(Key::Enter);
        assert_eq!(harness.view.draft.refresh_seconds, 1_200);
        assert_eq!(harness.view.draft.idle_refresh_seconds, 0);
    }

    #[test]
    fn typed_numbers_are_committed_clamped_or_abandoned() {
        let mut harness = Harness::new();
        let hours = |harness: &Harness| harness.view.draft.reset_expiry_lead_hours;
        harness.click(Id::ReminderHours);
        assert!(harness.view.is_editing());
        harness.retype("x1-8 ");
        harness.key(Key::Enter);
        assert_eq!(hours(&harness), 18);

        // Out of range is clamped when the focus leaves the field.
        harness.retype("500");
        assert_eq!(hours(&harness), 18);
        harness.click(Id::AutoUseEnabled);
        assert_eq!(hours(&harness), 72);
        assert!(!harness.view.is_editing());

        // Blank text leaves the value alone.
        harness.click(Id::ReminderHours);
        harness.key(Key::SelectAll);
        harness.key(Key::Backspace);
        harness.key(Key::Tab);
        assert_eq!(hours(&harness), 72);

        // Arrow and paging keys step from what is typed.
        harness.click(Id::ReminderHours);
        harness.key(Key::Down);
        assert_eq!(hours(&harness), 71);
        harness.retype("30");
        harness.key(Key::Up);
        assert_eq!(hours(&harness), 31);
        harness.key(Key::PageDown);
        assert_eq!(hours(&harness), 21);
        for _ in 0..6 {
            harness.key(Key::PageUp);
        }
        assert_eq!(hours(&harness), 72);

        // So do the two halves of the spin strip.
        let field = harness.target(Id::ReminderHours).unwrap().bounds;
        harness.click_at(field.right() - 8.0, field.bottom() - 6.0);
        assert_eq!(hours(&harness), 71);
        harness.click_at(field.right() - 8.0, field.y + 6.0);
        harness.click_at(field.right() - 8.0, field.y + 6.0);
        assert_eq!(hours(&harness), 72);

        // Leaving the window commits as well.
        harness.retype("5");
        harness.view.focus_lost();
        assert_eq!(hours(&harness), 5);
        assert!(!harness.view.is_editing());
        harness.view.focus_gained();
        assert!(harness.view.is_editing());
    }

    #[test]
    fn unsaved_changes_cover_edits_typing_and_color_picking() {
        let mut harness = Harness::new();
        assert!(!harness.view.has_unsaved_changes());

        // The preview percentage is not a setting.
        harness.open(Page::Appearance);
        harness.click(Id::PreviewRemaining);
        harness.retype("17");
        assert!(!harness.view.has_unsaved_changes());
        harness.key(Key::Enter);
        assert_eq!(harness.view.preview_percent, 17);
        assert!(!harness.view.has_unsaved_changes());

        // A number still being typed counts, until it says the old value again.
        harness.open(Page::Usage);
        harness.click(Id::ReminderHours);
        assert!(!harness.view.has_unsaved_changes());
        harness.retype("18");
        assert!(harness.view.has_unsaved_changes());
        harness.retype("24");
        assert!(!harness.view.has_unsaved_changes());

        // An edit counts until it is reverted.
        harness.open(Page::Popup);
        harness.click(Id::ShowResetTimes);
        assert!(harness.view.has_unsaved_changes());
        harness.click(Id::ShowResetTimes);
        assert!(!harness.view.has_unsaved_changes());

        // An open color picker counts; canceling it leaves no change.
        let initial = harness.view.begin_color_choice(ColorTarget::TrayText);
        assert_eq!(initial, 0xFFFF_FFFF);
        assert!(harness.view.has_unsaved_changes());
        harness
            .view
            .finish_color_choice(ColorTarget::TrayText, None);
        assert!(!harness.view.has_unsaved_changes());
        harness.view.begin_color_choice(ColorTarget::TrayText);
        harness
            .view
            .finish_color_choice(ColorTarget::TrayText, Some(0x00FF_00FF));
        assert!(harness.view.has_unsaved_changes());
        assert_eq!(harness.view.draft.tray_text_color, 0xFFFF_00FF);

        // Saving makes the current values the baseline.
        harness.view.mark_saved();
        assert!(!harness.view.has_unsaved_changes());
    }

    #[test]
    fn tab_visits_the_enabled_controls_in_reading_order() {
        let mut harness = Harness::new();
        let order = [
            Id::MetricWeekly,
            Id::ThresholdAlerts,
            Id::ReminderEnabled,
            Id::ReminderHours,
            Id::AutoUseEnabled,
            Id::RestoreDefaults,
            Id::Cancel,
            Id::Save,
            Id::Navigation(Page::Usage),
        ];
        for expected in order {
            harness.key(Key::Tab);
            assert_eq!(harness.focused(), Some(expected));
            // A numeric field is ready for typing as soon as it has the focus.
            assert_eq!(harness.view.is_editing(), expected == Id::ReminderHours);
        }
        for expected in order.iter().rev().skip(1) {
            harness.shift_tab();
            assert_eq!(harness.focused(), Some(*expected));
        }

        // Enabling the alerts adds their fields to the order; the chosen
        // radio button stands for its group.
        harness.click(Id::MetricAuto);
        harness.click(Id::ThresholdAlerts);
        for expected in [Id::Critical, Id::Low, Id::ReminderEnabled] {
            harness.key(Key::Tab);
            assert_eq!(harness.focused(), Some(expected));
        }
        for _ in 0..4 {
            harness.shift_tab();
        }
        assert_eq!(harness.focused(), Some(Id::MetricAuto));
    }

    #[test]
    fn arrow_keys_move_within_lists_groups_and_dropdowns() {
        let mut harness = Harness::new();
        // The sidebar: Up and Down change the page and keep the focus.
        harness.key(Key::Down);
        assert_eq!(harness.view.page, Page::Forecast);
        assert_eq!(harness.focused(), Some(Id::Navigation(Page::Forecast)));
        for _ in 0..3 {
            harness.key(Key::Up);
        }
        assert_eq!(harness.view.page, Page::General);
        harness.key(Key::Down);
        assert_eq!(harness.view.page, Page::Usage);

        // Radio buttons: arrows move the choice and stop at the ends.
        harness.key(Key::Tab);
        assert_eq!(harness.focused(), Some(Id::MetricWeekly));
        harness.key(Key::Right);
        assert_eq!(harness.view.draft.icon_metric, IconMetric::FiveHour);
        harness.key(Key::Down);
        harness.key(Key::Down);
        assert_eq!(harness.view.draft.icon_metric, IconMetric::Auto);
        assert_eq!(harness.focused(), Some(Id::MetricAuto));
        harness.key(Key::Left);
        assert_eq!(harness.view.draft.icon_metric, IconMetric::FiveHour);

        // Segments behave the same way.
        harness.open(Page::Forecast);
        harness.key(Key::Tab);
        assert_eq!(harness.focused(), Some(Id::ForecastLearned));
        harness.key(Key::Right);
        assert_eq!(
            harness.view.draft.forecast_method,
            ForecastMethod::RecentPace
        );
        harness.key(Key::Left);
        assert_eq!(
            harness.view.draft.forecast_method,
            ForecastMethod::LearnedRoutine
        );

        // A closed dropdown changes with Up and Down; an open one highlights
        // with them and takes the highlighted item on Enter.
        let quiet = |harness: &Harness| harness.view.draft.quiet_hours_mode;
        harness.key(Key::Tab);
        assert_eq!(harness.focused(), Some(Id::QuietHours));
        harness.key(Key::Down);
        assert_eq!(quiet(&harness), QuietHoursMode::Off);
        harness.key(Key::Up);
        assert_eq!(quiet(&harness), QuietHoursMode::Automatic);
        harness.key(Key::Space);
        assert!(harness.target(Id::DropdownItem(1)).is_some());
        harness.key(Key::Down);
        assert_eq!(quiet(&harness), QuietHoursMode::Automatic);
        assert_eq!(harness.key(Key::Enter), None);
        assert_eq!(quiet(&harness), QuietHoursMode::Off);
        assert!(harness.target(Id::DropdownItem(1)).is_none());

        // Escape closes an open list before it closes the window.
        assert_eq!(harness.key(Key::Enter), None);
        assert!(harness.target(Id::DropdownItem(0)).is_some());
        harness.key(Key::Up);
        assert_eq!(harness.key(Key::Escape), None);
        assert!(harness.target(Id::DropdownItem(0)).is_none());
        assert_eq!(quiet(&harness), QuietHoursMode::Off);
        assert_eq!(harness.key(Key::Escape), Some(Command::Close));

        // The theme dropdown follows the keyboard too, and the window with it.
        harness.open(Page::Appearance);
        harness.key(Key::Tab);
        assert_eq!(harness.focused(), Some(Id::Theme));
        harness.key(Key::Down);
        assert_eq!(harness.view.draft.theme, ThemeChoice::Dark);
        harness.key(Key::Down);
        harness.key(Key::Down);
        assert_eq!(harness.view.draft.theme, ThemeChoice::Light);
        assert!(!harness.view.dark());
    }

    #[test]
    fn space_and_enter_activate_and_enter_otherwise_saves() {
        let mut harness = Harness::new();
        // Space toggles the focused control.
        harness.key(Key::Tab);
        harness.key(Key::Tab);
        assert_eq!(harness.focused(), Some(Id::ThresholdAlerts));
        assert_eq!(harness.key(Key::Space), None);
        assert!(harness.view.draft.threshold_notifications);
        // Enter on a control that is not a button is the default button.
        assert_eq!(harness.key(Key::Enter), Some(Command::Save));
        assert!(harness.view.draft.threshold_notifications);

        // Enter in a numeric field only commits the number.
        harness.key(Key::Tab);
        assert_eq!(harness.focused(), Some(Id::Critical));
        harness.type_text("7");
        assert_eq!(harness.key(Key::Enter), None);
        assert_eq!(harness.view.draft.critical_threshold, 7);
        assert_eq!(harness.key(Key::Space), None);

        // Buttons are pressed by both keys.
        harness.click(Id::Cancel);
        assert_eq!(harness.focused(), Some(Id::Cancel));
        assert_eq!(harness.key(Key::Enter), Some(Command::Close));
        assert_eq!(harness.key(Key::Space), Some(Command::Close));
        harness.key(Key::Tab);
        assert_eq!(harness.key(Key::Space), Some(Command::Save));
        harness.open(Page::Updates);
        harness.key(Key::Tab);
        assert_eq!(harness.key(Key::Enter), Some(Command::CheckUpdates));
        harness.open(Page::Appearance);
        assert_eq!(
            harness.click(Id::TrayTextColor),
            Some(Command::PickColor(ColorTarget::TrayText))
        );
        assert_eq!(
            harness.key(Key::Enter),
            Some(Command::PickColor(ColorTarget::TrayText))
        );
        assert_eq!(
            harness.click(Id::TrayBoxColor),
            Some(Command::PickColor(ColorTarget::TrayBox))
        );
        assert_eq!(harness.click(Id::Save), Some(Command::Save));
    }

    #[test]
    fn a_press_outside_an_open_dropdown_only_closes_it() {
        let mut harness = Harness::new();
        harness.open(Page::Appearance);
        harness.click(Id::Theme);
        assert!(harness.target(Id::DropdownItem(2)).is_some());
        // The swatch under the pointer is not pressed by the dismissing click.
        assert_eq!(harness.click(Id::TrayTextColor), None);
        assert!(harness.target(Id::DropdownItem(2)).is_none());
        assert_eq!(harness.focused(), Some(Id::Theme));

        // Clicking the dropdown again closes its list; picking an item
        // chooses it. The dropdown keeps the focus throughout.
        harness.click(Id::Theme);
        harness.click(Id::Theme);
        assert!(harness.target(Id::DropdownItem(0)).is_none());
        harness.click(Id::Theme);
        harness.click(Id::DropdownItem(2));
        assert_eq!(harness.view.draft.theme, ThemeChoice::Light);
        assert_eq!(harness.focused(), Some(Id::Theme));

        // Scrolling dismisses the list too.
        harness.click(Id::Theme);
        harness.view.wheel(-1.0);
        harness.layout();
        assert!(harness.target(Id::DropdownItem(0)).is_none());
    }

    #[test]
    fn restoring_defaults_changes_the_controls_but_saves_nothing() {
        let stored = settings(|settings| {
            settings.icon_metric = IconMetric::Auto;
            settings.low_threshold = 35;
            settings.theme = ThemeChoice::Light;
            settings.start_with_windows = true;
        });
        let mut harness = Harness::with(&stored, 1.0, CLIENT_SIZE);
        harness.open(Page::Appearance);
        harness.click(Id::PreviewRemaining);
        harness.retype("7");
        assert_eq!(harness.click(Id::RestoreDefaults), None);
        assert_eq!(
            harness.view.draft,
            Draft::from_settings(&Settings::default())
        );
        assert_eq!(harness.view.preview_percent, DEFAULT_PREVIEW_PERCENT);
        // The saved values are untouched, so the restore itself is unsaved.
        assert!(harness.view.has_unsaved_changes());
        assert_eq!(harness.view.saved, Draft::from_settings(&stored));
    }

    #[test]
    fn saving_applies_every_edit_and_nothing_else() {
        let stored = settings(|settings| {
            settings.auto_redeem_credit_id = Some("credit".to_string());
            settings.auto_redeem_idempotency_key = Some("key".to_string());
            settings.auto_redeem_last_attempt_ms = Some(1_793_300_371_325);
            settings.show_additional_limits = false;
        });
        let mut harness = Harness::with(&stored, 1.0, CLIENT_SIZE);
        harness.click(Id::MetricAuto);
        harness.click(Id::ThresholdAlerts);
        harness.click(Id::Critical);
        harness.retype("80");
        harness.click(Id::ReminderEnabled);
        harness.open(Page::Forecast);
        harness.click(Id::ForecastRecent);
        harness.click(Id::ShowProjection);
        harness.open(Page::General);
        harness.click(Id::StartWithWindows);
        harness.click(Id::RunningRefresh);
        harness.retype("45");
        harness.open(Page::Appearance);
        harness.click(Id::Theme);
        harness.click(Id::DropdownItem(1));
        harness
            .view
            .finish_color_choice(ColorTarget::TrayBox, Some(0xFF10_2030));
        harness
            .view
            .finish_color_choice(ColorTarget::TrayText, Some(0xFF00_0000));
        harness.open(Page::Popup);
        harness.click(Id::ShowUpdatedTime);
        harness.click(Id::ColorUsageBars);

        let mut candidate = stored.clone();
        harness.view.apply_to(&mut candidate);
        let mut expected = stored.clone();
        expected.icon_metric = IconMetric::Auto;
        expected.threshold_notifications = true;
        expected.critical_threshold = 80;
        expected.low_threshold = 80;
        expected.reset_expiry_reminders = false;
        expected.forecast_method = ForecastMethod::RecentPace;
        expected.show_projected_usage = false;
        expected.start_with_windows = true;
        expected.refresh_seconds = 45;
        expected.theme = ThemeChoice::Dark;
        expected.tray_box_color = Some(0xFF10_2030);
        expected.saved_tray_box_color = 0xFF10_2030;
        expected.tray_text_color = 0xFF00_0000;
        expected.show_last_updated = false;
        expected.color_bars = false;
        // Everything the window does not edit is carried over as it was.
        assert_eq!(candidate, expected);
        harness.view.mark_saved();
        assert!(!harness.view.has_unsaved_changes());

        // A draft round-trips through settings unchanged.
        assert_eq!(Draft::from_settings(&candidate), harness.view.draft);
    }

    #[test]
    fn a_number_being_typed_is_saved_with_the_rest() {
        let mut harness = Harness::new();
        harness.click(Id::ReminderHours);
        harness.retype("36");
        let mut candidate = Settings::default();
        harness.view.apply_to(&mut candidate);
        assert_eq!(candidate.reset_expiry_lead_hours, 36);
    }

    #[test]
    fn drafts_bring_stored_numbers_into_their_fields_ranges() {
        let draft = Draft::from_settings(&settings(|settings| {
            settings.critical_threshold = 60;
            settings.low_threshold = 40;
            settings.refresh_seconds = 100_000;
            settings.idle_refresh_seconds = 20;
            settings.reset_expiry_lead_hours = 0;
            settings.auto_redeem_lead_minutes = 500;
        }));
        assert_eq!((draft.critical_threshold, draft.low_threshold), (60, 60));
        assert_eq!(draft.refresh_seconds, 3_600);
        assert_eq!(draft.idle_refresh_seconds, 3_600);
        assert_eq!(draft.reset_expiry_lead_hours, 1);
        assert_eq!(draft.auto_redeem_lead_minutes, 120);
    }

    #[test]
    fn the_tray_status_fills_the_updates_page() {
        let status = Status {
            version: Some("1.4.2".to_string()),
            update_status: Some("Version 1.5.0 available".to_string()),
            checking: Some(true),
            busy_text: Some("Installing...".to_string()),
            startup_enabled: Some(true),
        };
        let json = status.to_json();
        assert_eq!(
            json,
            concat!(
                r#"{"busyText":"Installing...","checking":true,"startupEnabled":true,"#,
                r#""updateStatus":"Version 1.5.0 available","version":"1.4.2"}"#
            )
        );
        assert_eq!(Status::from_json(json.as_bytes()), Some(status.clone()));
        assert_eq!(Status::default().to_json(), "{}");
        assert_eq!(Status::from_json(b"{}"), Some(Status::default()));
        assert_eq!(
            Status::from_json(br#"{"checking":"yes","version":7,"later":[1]}"#),
            Some(Status::default())
        );
        assert_eq!(Status::from_json(b"[]"), None);
        assert_eq!(Status::from_json(b"not json"), None);

        let mut harness = Harness::new();
        assert_eq!(harness.view.version, version::display(&version::current()));
        harness.view.set_status(status);
        assert_eq!(harness.view.version, "1.4.2");
        assert_eq!(harness.view.update_status, "Version 1.5.0 available");
        assert!(harness.view.checking);
        assert_eq!(harness.view.busy_label, "Installing...");

        // A later status only changes what it mentions.
        harness.view.set_status(Status {
            checking: Some(true),
            ..Status::default()
        });
        assert_eq!(harness.view.busy_label, "Checking...");
        harness.view.set_status(Status {
            checking: Some(false),
            update_status: Some("Up to date: 1.4.2".to_string()),
            ..Status::default()
        });
        assert!(!harness.view.checking);
        assert_eq!(harness.view.version, "1.4.2");
        assert_eq!(harness.view.update_status, "Up to date: 1.4.2");
    }

    #[test]
    fn the_startup_switch_starts_from_what_the_tray_reports() {
        let startup = |enabled: bool| Status {
            startup_enabled: Some(enabled),
            ..Status::default()
        };
        // The stored wish says off, Windows says on: the switch shows on and
        // that is not an unsaved change.
        let mut harness = Harness::new();
        harness.view.set_status(startup(true));
        assert!(harness.view.draft.start_with_windows);
        assert!(!harness.view.has_unsaved_changes());
        // Later reports do not move a switch the user may be about to change.
        harness.view.set_status(startup(false));
        assert!(harness.view.draft.start_with_windows);
        assert!(!harness.view.has_unsaved_changes());

        // A switch changed before the first report keeps the user's choice.
        let mut harness = Harness::new();
        harness.open(Page::General);
        harness.click(Id::StartWithWindows);
        harness.view.set_status(startup(false));
        assert!(harness.view.draft.start_with_windows);
        assert!(harness.view.has_unsaved_changes());
        let mut harness = Harness::new();
        harness.open(Page::General);
        harness.click(Id::StartWithWindows);
        harness.view.set_status(startup(true));
        assert!(harness.view.draft.start_with_windows);
        assert!(!harness.view.has_unsaved_changes());
    }

    #[test]
    fn contrast_guidance_uses_the_existing_wording() {
        assert!((contrast_ratio(0xFF00_0000, 0xFFFF_FFFF) - 21.0).abs() < 1e-9);
        assert!((contrast_ratio(0xFF12_3456, 0xFF12_3456) - 1.0).abs() < 1e-9);

        let mut harness = Harness::new();
        harness.view.draft.use_custom_tray_box_color = true;
        harness.view.draft.tray_box_color = 0xFF00_0000;
        let background = harness.view.effective_tray_background();
        assert_eq!(
            harness.view.contrast_guidance(background),
            ("Contrast 21.00:1".to_string(), false)
        );
        // White on the low-usage yellow is hard to read; black is suggested.
        harness.view.draft.use_custom_tray_box_color = false;
        harness.view.preview_percent = 20;
        let background = harness.view.effective_tray_background();
        assert_eq!(background, 0xFFF0_D200);
        let (text, warning) = harness.view.contrast_guidance(background);
        assert!(warning);
        assert!(text.starts_with("Low contrast 1."), "{text}");
        assert!(text.ends_with(":1. Try #000000."), "{text}");
        // Dark text on a dark box gets white suggested.
        harness.view.draft.tray_text_color = 0xFF30_3030;
        let (text, warning) = harness.view.contrast_guidance(0xFF10_1010);
        assert!(warning && text.ends_with("Try #FFFFFF."), "{text}");
        // Without a box only the taskbar matters.
        harness.view.draft.show_tray_box = false;
        assert_eq!(
            harness.view.contrast_guidance(0xFF10_1010),
            ("Text must contrast with the taskbar.".to_string(), false)
        );

        // The preview follows the thresholds being edited.
        harness.view.draft.critical_threshold = 30;
        harness.view.draft.low_threshold = 60;
        harness.view.preview_percent = 30;
        assert_eq!(harness.view.effective_tray_background(), 0xFFDC_2828);
        harness.view.preview_percent = 60;
        assert_eq!(harness.view.effective_tray_background(), 0xFFF0_D200);
        harness.view.preview_percent = 61;
        assert_eq!(harness.view.effective_tray_background(), 0xFF00_78D7);
    }

    #[test]
    fn a_short_window_scrolls_its_cards() {
        let mut harness = Harness::with(&Settings::default(), 1.0, (1200.0, 430.0));
        let viewport = harness.view.viewport;
        assert!(harness.view.scroll.is_scrollable());
        assert!(harness.target(Id::Scrollbar).is_some());
        // The last card starts out of view and cannot be clicked there.
        let checkbox = harness.target(Id::AutoUseEnabled).unwrap();
        assert!(checkbox.bounds.y > viewport.bottom());
        assert_eq!(checkbox.visible, None);

        // Tab brings each control into view as it takes the focus.
        while harness.focused() != Some(Id::AutoUseEnabled) {
            harness.key(Key::Tab);
        }
        let checkbox = harness.target(Id::AutoUseEnabled).unwrap();
        assert!(harness.view.scroll.offset() > 0.0);
        assert!(checkbox.bounds.y >= viewport.y && checkbox.bounds.bottom() <= viewport.bottom());
        assert_eq!(checkbox.visible, Some(checkbox.bounds));
        harness.key(Key::Space);
        assert!(harness.view.draft.auto_redeem_reset_credits);

        // The wheel and the paging keys scroll too.
        let offset = |harness: &Harness| harness.view.scroll.offset();
        let end = harness.view.scroll.max_offset();
        assert!(harness.view.wheel(100.0));
        assert_eq!(offset(&harness), 0.0);
        assert!(!harness.view.wheel(1.0));
        assert!(harness.view.wheel(-1.0));
        assert_eq!(offset(&harness), SCROLL_STEP);
        harness.layout();
        for _ in 0..3 {
            harness.shift_tab();
        }
        assert_eq!(harness.focused(), Some(Id::ThresholdAlerts));
        harness.key(Key::End);
        assert_eq!(offset(&harness), end);
        harness.key(Key::Home);
        assert_eq!(offset(&harness), 0.0);
        harness.key(Key::PageDown);
        assert!(offset(&harness) > 0.0);
        harness.key(Key::Down);
        harness.key(Key::Home);
        assert_eq!(offset(&harness), 0.0);

        // Dragging the thumb scrolls until the button is released.
        let thumb = harness.target(Id::Scrollbar).unwrap().bounds;
        let (x, y) = (thumb.x + thumb.w / 2.0, thumb.y + thumb.h / 2.0);
        let g = harness.canvas.graphics();
        harness.view.pointer_pressed(&g, x, y, false);
        assert!(harness.view.pointer_moved(Some((x, y + 10_000.0))));
        assert_eq!(offset(&harness), end);
        assert_eq!(harness.view.pointer_released(x, y + 10_000.0), None);
        harness.view.pointer_moved(Some((x, y)));
        assert_eq!(offset(&harness), end);
        assert_eq!(harness.focused(), Some(Id::ThresholdAlerts));

        // Changing page starts again from the top.
        harness.layout();
        harness.open(Page::Popup);
        assert_eq!(offset(&harness), 0.0);
    }

    #[test]
    fn controls_stay_inside_the_window_and_apart_at_every_scale() {
        let stored = settings(|settings| {
            settings.threshold_notifications = true;
            settings.auto_redeem_reset_credits = true;
            settings.set_tray_box_color(true, 0xFF12_3456);
        });
        for scale in [1.0, 1.25, 1.5, 2.0] {
            // The size the window opens at, and about the smallest it allows.
            for size in [CLIENT_SIZE, (964.0, 611.0)] {
                let mut harness = Harness::with(&stored, scale, size);
                harness.view.set_status(Status {
                    update_status: Some(
                        "Automatic updates paused after a failed update. Check updates to retry."
                            .to_string(),
                    ),
                    ..Status::default()
                });
                for page in Page::ALL {
                    harness.open(page);
                    let context = format!("{page:?} at {scale} in {size:?}");
                    let (width, height) = harness.size;
                    let cards = harness.view.viewport;
                    let targets = harness.view.interaction.targets();
                    for (index, target) in targets.iter().enumerate() {
                        let bounds = target.bounds;
                        assert!(bounds.w > 0.0 && bounds.h > 0.0, "{context}: {target:?}");
                        if target.id.scrolls() {
                            // Inside the cards, which may scroll vertically.
                            assert!(
                                bounds.x > cards.x && bounds.right() < cards.right(),
                                "{context}: {target:?}"
                            );
                        } else {
                            assert!(
                                bounds.x >= 0.0
                                    && bounds.y >= 0.0
                                    && bounds.right() <= width
                                    && bounds.bottom() <= height,
                                "{context}: {target:?}"
                            );
                        }
                        // What can be seen of one control never covers another.
                        for other in &targets[index + 1..] {
                            let shared = target
                                .visible
                                .zip(other.visible)
                                .and_then(|(first, second)| first.intersection(&second));
                            assert_eq!(shared, None, "{context}: {target:?} and {other:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn measuring_and_painting_agree_on_layout() {
        let _gdiplus = GdiPlus::start().unwrap();
        let canvas = Canvas::new(1200, 732);
        let g = canvas.graphics();
        for page in Page::ALL {
            let mut view = View::new(&Settings::default(), true);
            view.page = page;
            view.render(&g, 1.0, 1200.0, 732.0, false);
            let measured = view.interaction.targets().to_vec();
            view.render(&g, 1.0, 1200.0, 732.0, true);
            assert_eq!(view.interaction.targets(), measured, "{page:?}");
            assert!(!measured.is_empty());
        }
    }

    #[test]
    fn the_theme_being_edited_decides_how_the_window_is_drawn() {
        let _gdiplus = GdiPlus::start().unwrap();
        let canvas = Canvas::new(300, 200);
        let g = canvas.graphics();
        let corner = |view: &mut View| {
            view.render(&g, 1.0, 300.0, 200.0, true);
            canvas.pixel(299, 0)
        };
        let (dark, light) = (Palette::new(true).surface, Palette::new(false).surface);

        let mut view = View::new(&Settings::default(), true);
        assert_eq!(corner(&mut view), dark);
        view.set_system_dark(false);
        assert_eq!(corner(&mut view), light);
        view.draft.theme = ThemeChoice::Dark;
        assert_eq!(corner(&mut view), dark);
        view.set_system_dark(true);
        view.draft.theme = ThemeChoice::Light;
        assert_eq!(corner(&mut view), light);

        assert_eq!(caption_color(false), None);
        assert_eq!(
            caption_color(true),
            Some(sidebar_color(&Palette::new(true)))
        );
    }

    /// Writes the window in both themes and at several scales for visual
    /// review. Run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "writes sample images to target/samples"]
    fn write_settings_samples() {
        let _gdiplus = GdiPlus::start().unwrap();
        let probe = Canvas::new(8, 8);
        let save = |name: &str, view: &mut View, scale: f32, size: (f32, f32)| {
            let (width, height) = ((size.0 * scale).round(), (size.1 * scale).round());
            let canvas = Canvas::new(width as i32, height as i32);
            let g = canvas.graphics();
            view.render(&g, scale, width, height, false);
            view.render(&g, scale, width, height, true);
            canvas.save_png(&sample_path(&format!("settings-{name}.png")));
        };
        let view_for = |page: Page, dark: bool| {
            let stored = settings(|settings| {
                settings.theme = if dark {
                    ThemeChoice::Dark
                } else {
                    ThemeChoice::Light
                };
            });
            let mut view = View::new(&stored, dark);
            view.select_page(page);
            view.interaction
                .set_focus(Some(Id::Navigation(page)), false);
            view.set_status(Status {
                version: Some("1.4.2".to_string()),
                update_status: Some("Up to date: 1.4.2".to_string()),
                ..Status::default()
            });
            view.render(&probe.graphics(), 1.0, CLIENT_SIZE.0, CLIENT_SIZE.1, false);
            view
        };
        let center = |view: &View, id: Id| {
            let bounds = view.interaction.target(id).map(|target| target.bounds);
            bounds.map(|bounds| (bounds.x + bounds.w / 2.0, bounds.y + bounds.h / 2.0))
        };

        for (page, name) in [
            (Page::General, "general"),
            (Page::Usage, "usage"),
            (Page::Forecast, "forecast"),
            (Page::Appearance, "appearance"),
            (Page::Popup, "popup"),
            (Page::Updates, "updates"),
        ] {
            for (theme, dark) in [("dark", true), ("light", false)] {
                for (suffix, scale) in [("100", 1.0), ("150", 1.5)] {
                    let mut view = view_for(page, dark);
                    let name = format!("{name}-{theme}-{suffix}");
                    save(&name, &mut view, scale, CLIENT_SIZE);
                }
            }
            for (suffix, scale) in [("125", 1.25), ("200", 2.0)] {
                let mut view = view_for(page, true);
                save(
                    &format!("{name}-dark-{suffix}"),
                    &mut view,
                    scale,
                    CLIENT_SIZE,
                );
            }
        }

        // States: hover, pressed, keyboard focus, editing, disabled, open list.
        for (theme, dark) in [("dark", true), ("light", false)] {
            let mut view = view_for(Page::Usage, dark);
            view.activate(Id::ThresholdAlerts);
            view.activate(Id::MetricAuto);
            view.interaction.set_focus(Some(Id::Low), false);
            view.sync_edit_with_focus(true);
            view.character('3');
            view.character('5');
            view.pointer_moved(center(&view, Id::AutoUseEnabled));
            save(
                &format!("state-editing-{theme}"),
                &mut view,
                1.0,
                CLIENT_SIZE,
            );

            let mut view = view_for(Page::Usage, dark);
            view.key(Key::Tab, false);
            view.key(Key::Tab, false);
            view.pointer_moved(center(&view, Id::Save));
            save(&format!("state-focus-{theme}"), &mut view, 1.0, CLIENT_SIZE);

            let mut view = view_for(Page::Usage, dark);
            view.key(Key::Tab, false);
            view.key(Key::Tab, false);
            view.key(Key::Tab, false);
            view.key(Key::Tab, false);
            view.pointer_moved(center(&view, Id::Navigation(Page::Popup)));
            save(
                &format!("state-selected-{theme}"),
                &mut view,
                1.0,
                CLIENT_SIZE,
            );

            let mut view = view_for(Page::Forecast, dark);
            view.activate(Id::QuietHours);
            view.key(Key::Down, false);
            view.interaction.set_focus(Some(Id::QuietHours), true);
            save(
                &format!("state-dropdown-{theme}"),
                &mut view,
                1.0,
                CLIENT_SIZE,
            );

            let mut view = view_for(Page::Forecast, dark);
            view.activate(Id::ForecastRecent);
            view.interaction.set_focus(Some(Id::ForecastRecent), true);
            save(
                &format!("state-disabled-{theme}"),
                &mut view,
                1.0,
                CLIENT_SIZE,
            );

            let mut view = view_for(Page::Appearance, dark);
            view.finish_color_choice(ColorTarget::TrayBox, Some(0xFFF0_D200));
            view.interaction.set_focus(Some(Id::TrayBoxAuto), true);
            if let Some((x, y)) = center(&view, Id::Cancel) {
                view.pointer_pressed(&probe.graphics(), x, y, false);
            }
            save(
                &format!("state-contrast-{theme}"),
                &mut view,
                1.0,
                CLIENT_SIZE,
            );

            let mut view = view_for(Page::Updates, dark);
            view.set_status(Status {
                update_status: Some(
                    "Automatic updates paused after a failed update. Check updates to retry."
                        .to_string(),
                ),
                checking: Some(true),
                busy_text: Some("Installing...".to_string()),
                ..Status::default()
            });
            save(
                &format!("state-updating-{theme}"),
                &mut view,
                1.0,
                CLIENT_SIZE,
            );
        }

        // The smallest window, and one too short for its cards.
        let small = (964.0, 611.0);
        let mut view = view_for(Page::Usage, true);
        save("small-usage-dark-100", &mut view, 1.0, small);
        let mut view = view_for(Page::Appearance, false);
        save("small-appearance-light-150", &mut view, 1.5, small);
        let mut view = view_for(Page::Forecast, true);
        save("small-forecast-dark-200", &mut view, 2.0, small);
        let mut view = view_for(Page::Usage, true);
        view.render(&probe.graphics(), 2.0, 1_920.0, 940.0, false);
        view.wheel(-2.0);
        save("short-usage-dark-200", &mut view, 2.0, (960.0, 470.0));
    }
}
