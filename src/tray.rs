//! The tray process: tray icon, refresh scheduling and usage state.
//!
//! It stays small on purpose. The popup runs in a separate, short-lived
//! process (see `popup.rs`) that this process starts on a tray click and
//! feeds with ready-to-draw views, and GDI+ is only loaded while a new icon
//! is being rendered.
//!
//! All state lives in a thread-local `App` that is only borrowed for short,
//! non-reentrant sections. Calls that can dispatch messages back into the
//! window procedure (menus, cross-process sends) run outside those borrows.

use std::cell::RefCell;
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use windows_sys::core::PCWSTR;
use windows_sys::w;
use windows_sys::Win32::Foundation::{
    CloseHandle, HWND, INVALID_HANDLE_VALUE, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconGetRect, Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP,
    NIF_TIP, NIIF_INFO, NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NOTIFYICONDATAW, NOTIFYICONIDENTIFIER, NOTIFYICON_VERSION_4,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW, FindWindowW, GetCursorPos,
    GetMessageW, GetWindowThreadProcessId, KillTimer, PostMessageW, PostQuitMessage,
    RegisterClassExW, RegisterWindowMessageW, SendMessageTimeoutW, SetForegroundWindow, SetTimer,
    TrackPopupMenu, TranslateMessage, HICON, MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG,
    SMTO_ABORTIFHUNG, TPM_BOTTOMALIGN, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP,
    WM_CLOSE, WM_CONTEXTMENU, WM_COPYDATA, WM_DESTROY, WM_NULL, WM_TIMER, WNDCLASSEXW,
    WS_OVERLAPPED,
};

use crate::chart_data::{ChartData, ChartProvider};
use crate::chart_window;
use crate::claude::{ClaudeClient, ClaudeReading};
use crate::codex;
use crate::forecast::ForecastSettings;
use crate::gfx::{self, GdiPlus};
use crate::history::HistoryStore;
use crate::history_view::format_daily_usage_detail;
use crate::importer::{self, ScanOutcome, SystemStopwatch};
use crate::ipc::{
    split_history_event, EVENT_CLOSED, EVENT_OPEN_HISTORY, EVENT_OPEN_SETTINGS, EVENT_READY,
    EVENT_REFRESH, EVENT_SWITCH_PROVIDER, HISTORY_EVENT_CLOSED, HISTORY_EVENT_READY,
    POPUP_ARGUMENT, SETTINGS_EVENT_CHECK_UPDATES, SETTINGS_EVENT_CLOSED, SETTINGS_EVENT_DIRTY,
    SETTINGS_EVENT_READY, SETTINGS_EVENT_SAVED, VIEW_DATA, WM_HISTORY_EVENT, WM_POPUP_EVENT,
    WM_SETTINGS_EVENT,
};
use crate::localtime::{format_date, TimeZone};
use crate::model::{LimitWindow, RedemptionOutcome, Usage};
use crate::paths;
use crate::resets::redemption::{AttemptStatus, AutoRedeemAttempt, RedemptionPlan};
use crate::resets::{
    self, Notification, NotificationKind, ResetEngine, ResetSettings, Step, TimerChange,
};
use crate::settings::{self, IconMetric, Settings};
use crate::settings_view::Status;
use crate::settings_window;
use crate::startup;
use crate::timefmt::{format_clock, format_date_time, format_duration, now_unix};
use crate::update::{
    self, CheckOutcome, CheckRequest, Installation, SettingsWindow, StageOutcome, StagedUpdate,
    Startup, Trigger, UpdateError, UpdateInfo,
};
use crate::version;
use crate::view::{DailyBar, PopupView, ResetRow, Row};
use crate::win::from_wide;

const WM_TRAY: u32 = WM_APP + 1;
const WM_FETCH_DONE: u32 = WM_APP + 2;
// Messages posted to this window by its own worker threads. They start well
// above the cross-process messages defined in `ipc`.
/// A Claude read has started a network request.
const WM_CLAUDE_REQUEST: u32 = WM_APP + 20;
/// An update check finished; `lparam` owns a boxed check result.
const WM_UPDATE_CHECKED: u32 = WM_APP + 21;
/// An update download finished; `lparam` owns a boxed staging result.
const WM_UPDATE_STAGED: u32 = WM_APP + 22;
/// A reset redemption returned; `lparam` owns a boxed outcome.
const WM_REDEMPTION_DONE: u32 = WM_APP + 23;
/// A session-log scan finished; `lparam` owns its boxed outcome.
const WM_IMPORT_DONE: u32 = WM_APP + 24;
const NIN_SELECT: u32 = 0x0400;
const NIN_KEYSELECT: u32 = 0x0401;

const TRAY_ICON_ID: u32 = 1;
const TIMER_REFRESH: usize = 1;
const TIMER_ACTIVITY: usize = 2;
const TIMER_CLAUDE: usize = 3;
const TIMER_TOOLTIP: usize = 4;
/// One-shot timers armed by the reset engine.
const TIMER_REMINDER: usize = 5;
const TIMER_REDEMPTION: usize = 6;
const COMMAND_REFRESH: usize = 1;
const COMMAND_SHOW: usize = 2;
const COMMAND_EXIT: usize = 3;
const COMMAND_SETTINGS: usize = 4;
const COMMAND_HISTORY: usize = 5;
const COMMAND_CLAUDE_HISTORY: usize = 6;

const ACTIVITY_POLL_MILLISECONDS: u32 = 5_000;
const CLAUDE_POLL_MILLISECONDS: u32 = 30_000;
const TOOLTIP_POLL_MILLISECONDS: u32 = 60_000;
const VIEW_SEND_TIMEOUT_MILLISECONDS: u32 = 1_000;
/// A tray click this soon after the popup closed is the click that closed it.
const REOPEN_GUARD: Duration = Duration::from_millis(300);
const UNKNOWN_ICON_FILL: u32 = 0xFF40_4040;
const ERROR_ICON_FILL: u32 = 0xFFD2_2E2E;
const FIXED_ICON_INK: u32 = 0xFFFF_FFFF;
const PLAIN_BAR_DARK: u32 = 0xFF84_8484;
const PLAIN_BAR_LIGHT: u32 = 0xFF76_7676;

const TRAY_WINDOW_CLASS: PCWSTR = w!("UsageTrayRs.Tray");

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Provider {
    Codex,
    Claude,
}

/// Everything that determines the tray icon's pixels.
#[derive(Clone, Copy, PartialEq, Eq)]
enum IconSpec {
    Percent {
        percent: i32,
        fill: Option<u32>,
        ink: u32,
    },
    Unknown,
    Error,
}

/// What a worker thread is asked to fetch.
enum Fetch {
    Codex,
    Claude {
        client: Arc<Mutex<ClaudeClient>>,
        force: bool,
    },
}

enum FetchResult {
    Codex(Result<Usage, String>),
    Claude(Option<ClaudeReading>),
}

#[derive(Default)]
struct ProviderState {
    /// The last successful reading; kept when a later refresh fails.
    usage: Option<Usage>,
    /// Why the most recent refresh failed, if it did.
    error: Option<String>,
    refreshing: bool,
}

#[derive(Default)]
struct ChartProcess {
    process: Option<Child>,
    window: Option<HWND>,
}

impl ChartProcess {
    fn alive(&mut self) -> bool {
        let running = self
            .process
            .as_mut()
            .is_some_and(|process| matches!(process.try_wait(), Ok(None)));
        if !running {
            self.process = None;
            self.window = None;
        }
        running
    }
}

struct App {
    hwnd: HWND,
    icon: HICON,
    icon_spec: Option<IconSpec>,
    settings: Settings,
    codex: ProviderState,
    /// What the Claude client last returned; `None` keeps Claude hidden.
    claude: Option<ClaudeReading>,
    /// Owns Claude's refresh schedule. A worker thread holds the lock for a
    /// whole read, so this thread never locks it.
    claude_client: Arc<Mutex<ClaudeClient>>,
    /// True while a worker thread is inside the client.
    claude_read_running: bool,
    /// True once that read has started a network request.
    claude_refreshing: bool,
    codex_running: bool,
    codex_refresh_started: Option<Instant>,
    /// Set by the tray menu's "Refresh now": the refresh in flight should
    /// report its result in a notification.
    codex_announce: bool,
    /// Readings kept for forecasts and charts, one store per provider.
    codex_history: HistoryStore,
    claude_history: HistoryStore,
    importer: importer::Session,
    /// Decides which usage events deserve a notification.
    resets: ResetEngine,
    /// When update checks are due and what the last one found.
    updates: update::Session,
    /// The executable updates apply to, when its path is known.
    installation: Option<Installation>,
    /// A verified download the app is exiting to install.
    update_to_apply: Option<StagedUpdate>,
    selected: Provider,
    popup_process: Option<Child>,
    /// The popup's window, once its process has reported ready.
    popup_window: Option<HWND>,
    popup_closed_at: Option<Instant>,
    settings_process: Option<Child>,
    /// The Settings window, once its process has reported ready.
    settings_window: Option<HWND>,
    /// Whether the Settings window has edits that a restart would lose.
    settings_dirty: bool,
    codex_chart: ChartProcess,
    claude_chart: ChartProcess,
}

impl App {
    fn chart(&self, provider: ChartProvider) -> &ChartProcess {
        match provider {
            ChartProvider::Codex => &self.codex_chart,
            ChartProvider::Claude => &self.claude_chart,
        }
    }

    fn chart_mut(&mut self, provider: ChartProvider) -> &mut ChartProcess {
        match provider {
            ChartProvider::Codex => &mut self.codex_chart,
            ChartProvider::Claude => &mut self.claude_chart,
        }
    }

    fn shown_provider(&self) -> Provider {
        if self.selected == Provider::Claude && self.claude.is_some() {
            Provider::Claude
        } else {
            Provider::Codex
        }
    }

    /// Seconds between automatic Codex checks right now; zero pauses them.
    fn codex_interval_seconds(&self) -> u64 {
        if self.codex_running {
            self.settings.refresh_seconds
        } else {
            self.settings.idle_refresh_seconds
        }
    }

    fn codex_paused(&self) -> bool {
        self.codex_interval_seconds() == 0
    }

    fn dark(&self) -> bool {
        self.settings.theme.resolve_dark()
    }

    /// True while the Settings process is running. Forgets one that exited.
    fn settings_alive(&mut self) -> bool {
        let running = self
            .settings_process
            .as_mut()
            .is_some_and(|process| matches!(process.try_wait(), Ok(None)));
        if !running {
            self.settings_process = None;
            self.settings_window = None;
            self.settings_dirty = false;
        }
        running
    }

    /// The Settings window as far as restarting for an update is concerned.
    fn settings_state(&mut self) -> SettingsWindow {
        if self.settings_alive() {
            SettingsWindow::Open {
                unsaved_changes: self.settings_dirty,
            }
        } else {
            SettingsWindow::Closed
        }
    }

    /// What the Settings window shows that only the tray knows.
    fn settings_status(&self) -> Status {
        let busy = self.updates.busy_label();
        Status {
            version: Some(version::display(&version::current())),
            update_status: Some(self.updates.status().to_string()),
            checking: Some(busy.is_some()),
            busy_text: busy.map(str::to_string),
            startup_enabled: self
                .installation
                .as_ref()
                .map(|installation| startup::is_enabled(&installation.executable)),
        }
    }

    /// Writes the record of a redemption attempt to the settings file. The
    /// record is what stops a restart from spending a second reset, so the
    /// request is only sent once this has succeeded.
    fn store_redemption_attempt(&mut self, attempt: &AutoRedeemAttempt) -> Result<(), String> {
        let settings = &mut self.settings;
        settings.auto_redeem_credit_id = attempt.credit_id.clone();
        settings.auto_redeem_idempotency_key = attempt.idempotency_key.clone();
        settings.auto_redeem_attempt_status =
            attempt.status.map(|status| status.as_str().to_string());
        settings.auto_redeem_last_attempt_ms = attempt
            .last_attempt_utc
            .map(|seconds| seconds.saturating_mul(1_000));
        settings.save().map_err(|error| error.to_string())
    }

    fn icon_spec(&self) -> IconSpec {
        let settings = &self.settings;
        let window = self
            .codex
            .usage
            .as_ref()
            .and_then(|usage| usage.icon_window(settings.icon_metric));
        match window {
            Some((window, _)) => {
                let percent = window.remaining_percent();
                let fill = settings
                    .show_tray_box
                    .then(|| settings.box_color_for_percent(percent));
                IconSpec::Percent {
                    percent,
                    fill,
                    ink: settings.tray_text_color,
                }
            }
            None if self.codex.error.is_some() => IconSpec::Error,
            None => IconSpec::Unknown,
        }
    }

    fn status(&self, provider: Provider) -> Option<String> {
        match provider {
            Provider::Codex => {
                let state = &self.codex;
                match (&state.error, state.usage.is_some()) {
                    (Some(error), true) => Some(format!("{error} Showing last known usage.")),
                    (Some(error), false) => Some(error.clone()),
                    (None, true) => self.codex_paused().then(|| {
                        "Automatic refresh paused while Codex is not running.".to_string()
                    }),
                    (None, false) if self.codex_paused() => {
                        Some("Waiting for Codex to start...".to_string())
                    }
                    (None, false) => Some("Checking limits...".to_string()),
                }
            }
            Provider::Claude => self
                .claude
                .as_ref()
                .and_then(|reading| reading.status.clone()),
        }
    }

    fn tooltip(&self, now: i64) -> String {
        let mut lines = Vec::new();
        let describe = |window: &LimitWindow| match window.resets_at {
            Some(resets_at) => format!(
                "{}% (Resets in {})",
                window.remaining_percent(),
                format_duration(resets_at - now)
            ),
            None => format!("{}%", window.remaining_percent()),
        };
        match &self.codex.usage {
            Some(usage) => {
                if self.settings.icon_metric == IconMetric::Auto {
                    if let Some((_, weekly)) = usage.icon_window(IconMetric::Auto) {
                        let name = if weekly { "Weekly" } else { "5-hour" };
                        lines.push(format!("Tray: {name} (Auto)"));
                    }
                }
                lines.push("Codex Usage Remaining".to_string());
                if let Some(weekly) = &usage.weekly {
                    lines.push(format!("Weekly: {}", describe(weekly)));
                }
                if let Some(five_hour) = &usage.five_hour {
                    lines.push(format!("5h: {}", describe(five_hour)));
                }
            }
            None => {
                lines.push("Codex Usage Remaining".to_string());
                lines.push("Usage unavailable".to_string());
            }
        }
        lines.extend(self.status(Provider::Codex));
        lines.join("\n")
    }

    /// The popup's content for the provider currently shown.
    fn view(&self, now: i64) -> PopupView {
        let settings = &self.settings;
        let dark = self.dark();
        let provider = self.shown_provider();
        let (usage, history, refreshing, name, fresh) = match provider {
            Provider::Codex => (
                self.codex.usage.as_ref(),
                &self.codex_history,
                self.codex.refreshing,
                "Codex",
                self.codex.error.is_none() && !self.codex_paused(),
            ),
            Provider::Claude => (
                self.claude.as_ref().map(|reading| &reading.usage),
                &self.claude_history,
                self.claude_refreshing,
                "Claude",
                self.claude.as_ref().is_some_and(|reading| !reading.stale),
            ),
        };
        let forecast_settings = ForecastSettings {
            method: settings.forecast_method,
            quiet_hours: settings.quiet_hours_mode,
        };

        let mut rows = Vec::new();
        let mut resets = Vec::new();
        let mut reset_count = 0;
        if let Some(usage) = usage {
            for (window, weekly) in [(&usage.weekly, true), (&usage.five_hour, false)] {
                let Some(window) = window else {
                    continue;
                };
                let remaining = window.remaining_percent();
                rows.push(Row {
                    weekly,
                    remaining,
                    fill: if settings.color_bars {
                        settings.color_for_percent(remaining)
                    } else if dark {
                        PLAIN_BAR_DARK
                    } else {
                        PLAIN_BAR_LIGHT
                    },
                    reset: match window.resets_at {
                        _ if !settings.show_reset_times => String::new(),
                        Some(resets_at) => format!(
                            "Next reset: {} ({})",
                            format_date_time(resets_at, now),
                            format_duration(resets_at - now)
                        ),
                        None => "Next reset: ?".to_string(),
                    },
                    forecast: history
                        .forecast(weekly, usage, fresh, forecast_settings, now)
                        .text(now),
                });
            }
            let itemized = usage.resets.len();
            reset_count = usage.reset_count.unwrap_or(0).max(itemized);
            resets = usage
                .resets
                .iter()
                .enumerate()
                .map(|(index, credit)| ResetRow {
                    title: format!(
                        "{}. {}",
                        index + 1,
                        credit.title.as_deref().unwrap_or("Reset")
                    ),
                    expiration: match credit.expires_at {
                        Some(expires_at) => {
                            format!("Expires {}", format_date_time(expires_at, now))
                        }
                        None => "Expiration unavailable".to_string(),
                    },
                })
                .collect();
            // Codex can report more resets than it itemizes.
            let unitemized = reset_count - itemized;
            if unitemized > 0 {
                let noun = if unitemized == 1 { "reset" } else { "resets" };
                resets.push(ResetRow {
                    title: format!("{unitemized} additional {noun}"),
                    expiration: "Expiration unavailable".to_string(),
                });
            }
        }

        PopupView {
            dark,
            provider: name.to_string(),
            can_switch: self.claude.is_some(),
            updated: usage
                .filter(|_| settings.show_last_updated)
                .map(|usage| format!("Updated {}", format_clock(usage.updated_at))),
            refreshing,
            // The chart shows weekly allowance used per day, so it needs a
            // weekly limit to be meaningful.
            daily: match usage {
                Some(usage) if usage.weekly.is_some() => history
                    .daily_usage(now)
                    .iter()
                    .map(|day| DailyBar {
                        label: format_date(day.date, "ddd"),
                        used: day.used_percent,
                        detail: format_daily_usage_detail(day),
                    })
                    .collect(),
                _ => Vec::new(),
            },
            rows,
            status: self.status(provider),
            reset_count,
            resets,
        }
    }

    /// The settings the reset engine works from.
    fn reset_settings(&self) -> ResetSettings {
        ResetSettings {
            threshold_notifications: self.settings.threshold_notifications,
            critical_threshold: self.settings.critical_threshold,
            low_threshold: self.settings.low_threshold,
            reset_expiry_reminders: self.settings.reset_expiry_reminders,
            reset_expiry_lead_hours: self.settings.reset_expiry_lead_hours,
            auto_redeem_reset_credits: self.settings.auto_redeem_reset_credits,
            auto_redeem_lead_minutes: self.settings.auto_redeem_lead_minutes,
            auto_redeem_attempt: AutoRedeemAttempt {
                credit_id: self.settings.auto_redeem_credit_id.clone(),
                idempotency_key: self.settings.auto_redeem_idempotency_key.clone(),
                status: self
                    .settings
                    .auto_redeem_attempt_status
                    .as_deref()
                    .and_then(AttemptStatus::parse),
                last_attempt_utc: self
                    .settings
                    .auto_redeem_last_attempt_ms
                    .map(|milliseconds| milliseconds.div_euclid(1_000)),
            },
        }
    }

    /// True while the popup process is running. Forgets a process that exited.
    fn popup_alive(&mut self) -> bool {
        let running = self
            .popup_process
            .as_mut()
            .is_some_and(|process| matches!(process.try_wait(), Ok(None)));
        if !running {
            self.popup_process = None;
            self.popup_window = None;
        }
        running
    }
}

/// Opens one provider's saved readings from the data directory.
fn open_history(file: &str) -> HistoryStore {
    HistoryStore::open(paths::data_file(file), TimeZone::system(), now_unix())
}

/// Runs `action` with the app state, or returns `None` if the state is
/// already borrowed further up the stack or the app is shutting down.
fn with_app<R>(action: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| {
        cell.try_borrow_mut()
            .ok()
            .and_then(|mut app| app.as_mut().map(action))
    })
}

/// Asks a running tray process to exit cleanly, removing its icon.
pub fn request_exit() {
    // SAFETY: the class name is a static string; posting to a window that
    // has just gone away is harmless.
    unsafe {
        let window = FindWindowW(TRAY_WINDOW_CLASS, null());
        if !window.is_null() {
            PostMessageW(window, WM_CLOSE, 0, 0);
        }
    }
}

/// Creates the tray icon, then runs the message loop until exit. Returns a
/// downloaded update when the app exited in order to install it; the caller
/// applies it once this process no longer holds the single-instance lock.
pub fn run(startup: Startup) -> Option<StagedUpdate> {
    if let (Some(data), Some(legacy)) = (paths::data_directory(), paths::legacy_data_directory()) {
        // Without the directory the app still shows live usage; it only
        // cannot keep caches, so a failure here is not fatal.
        let _ = paths::prepare_data_directory(&data, &legacy);
    }
    // SAFETY: standard window-class registration and message loop; every
    // pointer passed refers to a live local or a static string.
    unsafe {
        let instance = GetModuleHandleW(null());
        let class_name = TRAY_WINDOW_CLASS;
        let mut class: WNDCLASSEXW = zeroed();
        class.cbSize = size_of::<WNDCLASSEXW>() as u32;
        class.lpfnWndProc = Some(window_procedure);
        class.hInstance = instance;
        class.lpszClassName = class_name;
        if RegisterClassExW(&class) == 0 {
            return None;
        }
        // A hidden top-level window (not message-only) so that the shell's
        // TaskbarCreated broadcast reaches it after an Explorer restart.
        let hwnd = CreateWindowExW(
            0,
            class_name,
            w!("Usage Tray"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            return None;
        }

        TASKBAR_CREATED.store(
            RegisterWindowMessageW(w!("TaskbarCreated")),
            Ordering::Relaxed,
        );
        APP.with(|cell| {
            *cell.borrow_mut() = Some(App {
                hwnd,
                icon: null_mut(),
                icon_spec: None,
                settings: Settings::load(),
                codex: ProviderState::default(),
                claude: None,
                claude_client: Arc::new(Mutex::new(ClaudeClient::default())),
                claude_read_running: false,
                claude_refreshing: false,
                codex_running: false,
                codex_refresh_started: None,
                codex_announce: false,
                codex_history: open_history(paths::CODEX_HISTORY_FILE),
                claude_history: open_history(paths::CLAUDE_HISTORY_FILE),
                importer: importer::Session::new(importer::default_codex_home()),
                resets: match reset_state_paths() {
                    Some((banked, reminders)) => ResetEngine::load(&banked, &reminders),
                    None => ResetEngine::default(),
                },
                updates: update::Session::new(&startup),
                installation: Installation::current(),
                update_to_apply: None,
                selected: Provider::Codex,
                popup_process: None,
                popup_window: None,
                popup_closed_at: None,
                settings_process: None,
                settings_window: None,
                settings_dirty: false,
                codex_chart: ChartProcess::default(),
                claude_chart: ChartProcess::default(),
            });
        });

        add_tray_icon();
        match startup {
            // The previous version is waiting to hear that this one runs.
            Startup::Updated(confirmation) => confirmation.confirm(),
            Startup::UpdateFailed(_) => show_notification(&Notification {
                title: update::NOTIFICATION_TITLE.to_string(),
                text: update::ROLLED_BACK_NOTIFICATION.to_string(),
                kind: NotificationKind::Warning,
            }),
            Startup::Normal => {}
        }
        SetTimer(hwnd, TIMER_ACTIVITY, ACTIVITY_POLL_MILLISECONDS, None);
        SetTimer(hwnd, TIMER_CLAUDE, CLAUDE_POLL_MILLISECONDS, None);
        SetTimer(hwnd, TIMER_TOOLTIP, TOOLTIP_POLL_MILLISECONDS, None);
        schedule_codex();
        // Expiring resets must be noticed even while automatic checks are
        // paused, so one reading is taken at startup regardless.
        let needs_reading = with_app(|app| {
            !app.codex.refreshing
                && app.codex.usage.is_none()
                && app.reset_settings().needs_startup_refresh()
        });
        if needs_reading == Some(true) {
            request_refresh(Provider::Codex, false);
        }
        request_refresh(Provider::Claude, false);
        request_update_check(Trigger::Automatic);

        let mut message: MSG = zeroed();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        let app = APP.with(|cell| cell.borrow_mut().take())?;
        if !app.icon.is_null() {
            DestroyIcon(app.icon);
        }
        app.update_to_apply
    }
}

unsafe extern "system" fn window_procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_TRAY => {
            match (lparam as u32) & 0xFFFF {
                NIN_SELECT | NIN_KEYSELECT => toggle_popup(),
                WM_CONTEXTMENU => show_menu(hwnd),
                _ => {}
            }
            0
        }
        WM_TIMER => {
            match wparam {
                TIMER_REFRESH => {
                    KillTimer(hwnd, TIMER_REFRESH);
                    schedule_codex();
                }
                TIMER_ACTIVITY => poll_codex_activity(),
                // The client decides whether a request is due; between polls
                // a read only re-reads the saved reading.
                TIMER_CLAUDE => request_refresh(Provider::Claude, false),
                TIMER_TOOLTIP => {
                    refresh_ui();
                    // Cheap unless the hourly check has come due.
                    request_update_check(Trigger::Automatic);
                }
                TIMER_REMINDER | TIMER_REDEMPTION => {
                    KillTimer(hwnd, wparam);
                    let now = now_unix();
                    let step = with_app(|app| {
                        let settings = app.reset_settings();
                        let refreshing = app.codex.refreshing;
                        if wparam == TIMER_REMINDER {
                            app.resets.reminder_timer_fired(&settings, refreshing, now)
                        } else {
                            app.resets
                                .redemption_timer_fired(&settings, refreshing, now)
                        }
                    });
                    if let Some(step) = step {
                        apply_step(step);
                    }
                }
                _ => {}
            }
            0
        }
        WM_CLAUDE_REQUEST => {
            with_app(|app| app.claude_refreshing = app.claude_read_running);
            refresh_ui();
            0
        }
        WM_FETCH_DONE => {
            // SAFETY: the pointer was produced by Box::into_raw in spawn_fetch
            // and is delivered exactly once.
            match *Box::from_raw(lparam as *mut FetchResult) {
                FetchResult::Codex(outcome) => finish_codex_refresh(outcome),
                FetchResult::Claude(reading) => {
                    with_app(|app| {
                        app.claude_read_running = false;
                        app.claude_refreshing = false;
                        // A reading shown again from the cache is no newer
                        // than the last one stored, and the store skips it.
                        if let Some(reading) = reading.as_ref().filter(|reading| !reading.stale) {
                            app.claude_history.observe(&reading.usage, now_unix());
                        }
                        app.claude = reading;
                    });
                    refresh_ui();
                }
            }
            0
        }
        WM_POPUP_EVENT => {
            handle_popup_event(wparam, lparam);
            0
        }
        WM_SETTINGS_EVENT => {
            handle_settings_event(wparam, lparam);
            0
        }
        WM_HISTORY_EVENT => {
            handle_history_event(wparam, lparam);
            0
        }
        WM_IMPORT_DONE => {
            // SAFETY: the pointer was produced by post_boxed with this type
            // and is delivered exactly once.
            let outcome = *Box::from_raw(lparam as *mut ScanOutcome);
            let changed = with_app(|app| {
                app.importer
                    .scan_finished(outcome, &mut app.codex_history, now_unix())
            });
            if changed.is_some_and(|count| count > 0) {
                refresh_ui();
            }
            0
        }
        WM_REDEMPTION_DONE => {
            // SAFETY: the pointer was produced by post_boxed with this type
            // and is delivered exactly once.
            finish_redemption(*Box::from_raw(lparam as *mut RedemptionOutcome));
            0
        }
        WM_UPDATE_CHECKED => {
            // SAFETY: the pointer was produced by post_boxed with this type
            // and is delivered exactly once.
            finish_update_check(*Box::from_raw(
                lparam as *mut Result<UpdateInfo, UpdateError>,
            ));
            0
        }
        WM_UPDATE_STAGED => {
            // SAFETY: as above, for the staging result.
            finish_update_stage(*Box::from_raw(
                lparam as *mut Result<StagedUpdate, UpdateError>,
            ));
            0
        }
        WM_DESTROY => {
            close_child_windows();
            let data = tray_data(hwnd);
            Shell_NotifyIconW(NIM_DELETE, &data);
            PostQuitMessage(0);
            0
        }
        _ if message != 0 && message == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            add_tray_icon();
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    // SAFETY: NOTIFYICONDATAW is plain data for which all-zero is valid.
    let mut data: NOTIFYICONDATAW = unsafe { zeroed() };
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = TRAY_ICON_ID;
    data
}

/// Gives `value` to the tray window's procedure through `message`. If the
/// window is gone the value is dropped here, which for a staged update also
/// deletes its download.
fn post_boxed<T>(window: usize, message: u32, value: T) {
    let pointer = Box::into_raw(Box::new(value));
    // SAFETY: ownership of the box moves to the window procedure when the
    // message is queued; if queuing fails it is reclaimed here.
    unsafe {
        if PostMessageW(window as HWND, message, 0, pointer as LPARAM) == 0 {
            drop(Box::from_raw(pointer));
        }
    }
}

/// Starts an update check on a worker thread if the session says one is due.
fn request_update_check(trigger: Trigger) {
    let start = with_app(|app| {
        let installation = app.installation.clone()?;
        let request = app.updates.request_check(trigger, now_unix());
        (request == CheckRequest::Start).then_some((app.hwnd as usize, installation))
    });
    if let Some((window, installation)) = start.flatten() {
        std::thread::spawn(move || {
            let result = update::check(&installation, &update::SystemHost);
            post_boxed(window, WM_UPDATE_CHECKED, result);
        });
    }
    send_settings_status();
}

/// Records what a check found and downloads the update if there is one.
fn finish_update_check(result: Result<UpdateInfo, UpdateError>) {
    let download = with_app(|app| match app.updates.check_finished(result).1 {
        CheckOutcome::Install(info) => {
            let installation = app.installation.clone()?;
            Some((app.hwnd as usize, info, installation))
        }
        // The other outcomes only change the status the session reports.
        _ => None,
    });
    if let Some((window, info, installation)) = download.flatten() {
        std::thread::spawn(move || {
            let result = update::stage(&info, &installation, &update::SystemHost);
            post_boxed(window, WM_UPDATE_STAGED, result);
        });
    }
    send_settings_status();
}

/// Records a finished download and, if it verified, exits to install it.
fn finish_update_stage(result: Result<StagedUpdate, UpdateError>) {
    match with_app(|app| app.updates.stage_finished(result)) {
        // A background failure has no window to report it in.
        Some((Trigger::Automatic, StageOutcome::Failed { .. })) => {
            show_notification(&Notification {
                title: update::NOTIFICATION_TITLE.to_string(),
                text: update::STAGE_FAILED_NOTIFICATION.to_string(),
                kind: NotificationKind::Warning,
            });
        }
        Some((_, StageOutcome::Ready)) => exit_to_install_update(),
        _ => {}
    }
    send_settings_status();
}

/// Closes the app so that `main` can swap in a downloaded update, unless the
/// Settings window is in the way: a background update waits for it to close,
/// a requested one only for unsaved edits.
fn exit_to_install_update() {
    let window = with_app(|app| {
        let settings = app.settings_state();
        let staged = app.updates.take_ready(settings)?;
        app.update_to_apply = Some(staged);
        Some(app.hwnd)
    });
    if let Some(hwnd) = window.flatten() {
        // SAFETY: hwnd is the tray window owned by this thread. WM_DESTROY
        // closes the child processes before the update is applied.
        unsafe {
            DestroyWindow(hwnd);
        }
    }
}

/// Stops the import and all processes this tray started, including ones still
/// creating their windows. Waiting here releases the executable before an
/// update tries to replace it.
fn close_child_windows() {
    let children = with_app(|app| {
        app.importer.cancel();
        [
            (app.popup_window.take(), app.popup_process.take()),
            (app.settings_window.take(), app.settings_process.take()),
            (
                app.codex_chart.window.take(),
                app.codex_chart.process.take(),
            ),
            (
                app.claude_chart.window.take(),
                app.claude_chart.process.take(),
            ),
        ]
    });
    let Some(children) = children else {
        return;
    };
    for (window, _) in &children {
        if let Some(window) = window {
            // SAFETY: posting to a stale or already closed window is harmless.
            unsafe { PostMessageW(*window, WM_CLOSE, 0, 0) };
        }
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    for (_, process) in children {
        let Some(mut process) = process else { continue };
        loop {
            match process.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ => {
                    // A process that never created a window, or failed to
                    // close, cannot keep the updater from replacing the app.
                    if process.kill().is_ok() {
                        let _ = process.wait();
                    }
                    break;
                }
            }
        }
    }
}

/// Starts the Settings window process, or brings the one already open to
/// the front.
fn open_settings() {
    let Some((hwnd, alive, window)) =
        with_app(|app| (app.hwnd, app.settings_alive(), app.settings_window))
    else {
        return;
    };
    if alive {
        // Still starting up when it has no window yet.
        if let Some(window) = window {
            settings_window::bring_to_front(window);
        }
        return;
    }
    let (Ok(executable), Some(path)) = (std::env::current_exe(), settings::default_path()) else {
        return;
    };
    let arguments = settings_window::launch_arguments(hwnd, &path);
    let Ok(process) = Command::new(executable).args(arguments).spawn() else {
        return;
    };
    // The click gave this process the right to take the foreground; pass it
    // on so the new window can come to the front.
    // SAFETY: the call only takes a process id.
    unsafe {
        AllowSetForegroundWindow(process.id());
    }
    with_app(|app| {
        app.settings_process = Some(process);
        app.settings_window = None;
        app.settings_dirty = false;
    });
}

/// Starts one history process per provider, or raises the existing window.
fn open_history_window(provider: ChartProvider) {
    let Some((hwnd, alive, window)) = with_app(|app| {
        let hwnd = app.hwnd;
        let chart = app.chart_mut(provider);
        (hwnd, chart.alive(), chart.window)
    }) else {
        return;
    };
    if alive {
        if let Some(window) = window {
            chart_window::bring_to_front(window);
        }
        return;
    }
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let Ok(process) = Command::new(executable)
        .args(chart_window::launch_arguments(hwnd, provider))
        .spawn()
    else {
        return;
    };
    // SAFETY: the call only takes the child process id.
    unsafe { AllowSetForegroundWindow(process.id()) };
    with_app(|app| {
        let chart = app.chart_mut(provider);
        chart.process = Some(process);
        chart.window = None;
    });
}

/// Builds a provider's chart under the state borrow, then sends it only after
/// the borrow is released because a synchronous send can dispatch messages.
fn send_history(provider: ChartProvider) {
    let now = now_unix();
    let Some((tray, window, data)) = with_app(|app| {
        let window = app.chart(provider).window?;
        let (store, usage, fresh) = match provider {
            ChartProvider::Codex => (
                &app.codex_history,
                app.codex.usage.as_ref(),
                app.codex.error.is_none() && !app.codex_paused(),
            ),
            ChartProvider::Claude => (
                &app.claude_history,
                app.claude.as_ref().map(|reading| &reading.usage),
                app.claude.as_ref().is_some_and(|reading| !reading.stale),
            ),
        };
        Some((
            app.hwnd,
            window,
            ChartData::build(store, usage, fresh, &app.settings, now),
        ))
    })
    .flatten() else {
        return;
    };
    chart_window::send_data(tray, window, &data);
}

fn handle_history_event(wparam: WPARAM, lparam: LPARAM) {
    let (event, code) = split_history_event(wparam);
    let Some(provider) = ChartProvider::from_code(code) else {
        return;
    };
    let window = lparam as HWND;
    match event {
        HISTORY_EVENT_READY => {
            let mut owner = 0u32;
            // SAFETY: querying a window's process id has no preconditions.
            unsafe { GetWindowThreadProcessId(window, &mut owner) };
            let accepted = with_app(|app| {
                let chart = app.chart_mut(provider);
                let accepted = owner != 0 && chart.process.as_ref().map(Child::id) == Some(owner);
                if accepted {
                    chart.window = Some(window);
                }
                accepted
            });
            if accepted == Some(true) {
                send_history(provider);
            }
        }
        HISTORY_EVENT_CLOSED => {
            with_app(|app| {
                let chart = app.chart_mut(provider);
                if chart.window == Some(window) {
                    chart.window = None;
                }
            });
        }
        _ => {}
    }
}

/// Sends the Settings window, if one is open, what the tray knows.
fn send_settings_status() {
    let Some((hwnd, window, status)) = with_app(|app| {
        app.settings_window
            .map(|window| (app.hwnd, window, app.settings_status()))
    })
    .flatten() else {
        return;
    };
    settings_window::send_status(hwnd, window, &status);
}

fn handle_settings_event(event: WPARAM, lparam: LPARAM) {
    match event {
        SETTINGS_EVENT_READY => {
            let window = lparam as HWND;
            let mut owner = 0u32;
            // SAFETY: querying the owner of a window handle has no preconditions.
            unsafe {
                GetWindowThreadProcessId(window, &mut owner);
            }
            // Only the Settings process this tray started may receive status.
            let accepted = with_app(|app| {
                let expected = app.settings_process.as_ref().map(Child::id);
                let accepted = owner != 0 && expected == Some(owner);
                if accepted {
                    app.settings_window = Some(window);
                }
                accepted
            });
            if accepted == Some(true) {
                send_settings_status();
            }
        }
        SETTINGS_EVENT_SAVED => {
            with_app(|app| app.settings_dirty = false);
            // Loads the file the window just wrote and applies it everywhere.
            schedule_codex();
            apply_startup_setting();
            send_settings_status();
            exit_to_install_update();
        }
        SETTINGS_EVENT_CHECK_UPDATES => request_update_check(Trigger::Manual),
        SETTINGS_EVENT_DIRTY => {
            with_app(|app| app.settings_dirty = lparam != 0);
            exit_to_install_update();
        }
        SETTINGS_EVENT_CLOSED => {
            with_app(|app| {
                app.settings_window = None;
                app.settings_dirty = false;
            });
            exit_to_install_update();
        }
        _ => {}
    }
}

/// Makes the sign-in entry match the saved "Start with Windows" setting.
fn apply_startup_setting() {
    let failure = with_app(|app| {
        let executable = &app.installation.as_ref()?.executable;
        let wanted = app.settings.start_with_windows;
        if startup::is_enabled(executable) == wanted {
            return None;
        }
        startup::set_enabled(executable, wanted).err()
    });
    if let Some(error) = failure.flatten() {
        show_notification(&Notification {
            title: "Codex Usage".to_string(),
            text: error.message().to_string(),
            kind: NotificationKind::Warning,
        });
    }
}

/// Records a redemption attempt and, only once that is saved, asks Codex to
/// use the reset on a worker thread.
fn start_redemption(plan: RedemptionPlan) {
    let saved = with_app(|app| {
        app.store_redemption_attempt(&plan.attempt)
            .map(|()| app.hwnd as usize)
    });
    match saved {
        Some(Ok(window)) => {
            std::thread::spawn(move || {
                let outcome = codex::consume_reset_credit(&plan.credit_id, &plan.idempotency_key);
                post_boxed(window, WM_REDEMPTION_DONE, outcome);
            });
        }
        Some(Err(error)) => {
            let now = now_unix();
            let step = with_app(|app| app.resets.redemption_not_saved(&error, now));
            if let Some(step) = step {
                apply_step(step);
            }
        }
        None => {}
    }
}

/// Saves what a redemption returned and lets the reset engine react to it.
fn finish_redemption(outcome: RedemptionOutcome) {
    let now = now_unix();
    let step = with_app(|app| {
        let save_error = app
            .resets
            .redemption_record(&outcome, now)
            .and_then(|record| app.store_redemption_attempt(&record).err());
        let settings = app.reset_settings();
        let App {
            resets: engine,
            codex,
            ..
        } = app;
        engine.redemption_finished(
            &outcome,
            save_error.as_deref(),
            codex.usage.as_ref(),
            &settings,
            now,
            &mut resets::redemption::new_idempotency_key,
        )
    });
    if let Some(step) = step {
        apply_step(step);
    }
}

/// The reset engine's two state files, when the data directory is known.
fn reset_state_paths() -> Option<(PathBuf, PathBuf)> {
    let data = paths::data_directory()?;
    Some((
        data.join(paths::BANKED_RESETS_FILE),
        data.join(paths::RESET_REMINDERS_FILE),
    ))
}

/// Records a finished Codex refresh, lets the reset engine react to it, and
/// schedules the next one.
fn finish_codex_refresh(outcome: Result<Usage, String>) {
    let now = now_unix();
    let succeeded = outcome.is_ok();
    let step = with_app(|app| {
        app.codex.refreshing = false;
        let announce = std::mem::take(&mut app.codex_announce);
        let settings = app.reset_settings();
        match outcome {
            Ok(usage) => {
                let step = app.resets.refresh_succeeded(
                    &usage,
                    &settings,
                    now,
                    announce,
                    &mut resets::redemption::new_idempotency_key,
                );
                app.codex_history.observe(&usage, now);
                app.codex.usage = Some(usage);
                app.codex.error = None;
                step
            }
            Err(message) => {
                let last_usage = app.codex.usage.as_ref();
                let step = app
                    .resets
                    .refresh_failed(&message, last_usage, &settings, now, announce);
                app.codex.error = Some(message);
                step
            }
        }
    });
    schedule_codex();
    if let Some(step) = step {
        apply_step(step);
    }
    if succeeded {
        let request = with_app(|app| {
            let usage = app.codex.usage.as_ref()?;
            app.importer
                .request_scan(&app.codex_history, usage, now)
                .map(|request| (app.hwnd as usize, request))
        })
        .flatten();
        if let Some((window, request)) = request {
            std::thread::spawn(move || {
                let outcome = importer::scan(request, &SystemStopwatch::start());
                post_boxed(window, WM_IMPORT_DONE, outcome);
            });
        }
    }
}

/// Carries out what the reset engine decided: notifications first, then its
/// saved state and timers, then any refresh it asked for.
fn apply_step(step: Step) {
    let now = now_unix();
    for notification in &step.notifications {
        show_notification(notification);
    }
    let Some(hwnd) = with_app(|app| {
        if let Some((banked, reminders)) = reset_state_paths() {
            app.resets.save(&step, &banked, &reminders);
        }
        app.hwnd
    }) else {
        return;
    };
    for (timer, change) in [
        (TIMER_REMINDER, step.reminder_timer),
        (TIMER_REDEMPTION, step.redemption_timer),
    ] {
        // SAFETY: hwnd is the tray window owned by this thread.
        unsafe {
            match change {
                TimerChange::Keep => {}
                TimerChange::Cancel => {
                    KillTimer(hwnd, timer);
                }
                TimerChange::FireAt(fire_at) => {
                    let delay = resets::timer_delay_milliseconds(fire_at, now);
                    SetTimer(hwnd, timer, delay, None);
                }
            }
        }
    }
    if let Some(plan) = step.redemption {
        start_redemption(plan);
    }
    if step.refresh {
        request_refresh(Provider::Codex, false);
    }
}

/// Shows a tray notification.
fn show_notification(notification: &Notification) {
    let Some(hwnd) = with_app(|app| app.hwnd) else {
        return;
    };
    let mut data = tray_data(hwnd);
    data.uFlags = NIF_INFO;
    copy_text(&mut data.szInfoTitle, &notification.title);
    copy_text(&mut data.szInfo, &notification.text);
    data.dwInfoFlags = match notification.kind {
        NotificationKind::Info => NIIF_INFO,
        NotificationKind::Warning => NIIF_WARNING,
    };
    // SAFETY: data is fully initialized and outlives the call.
    unsafe {
        Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

/// Copies as much of `text` as fits, leaving room for the terminator.
fn copy_text<const N: usize>(target: &mut [u16; N], text: &str) {
    let mut length = 0;
    for unit in text.encode_utf16().take(target.len() - 1) {
        target[length] = unit;
        length += 1;
    }
    // Do not end on the first half of a surrogate pair.
    if length > 0 && (0xD800..0xDC00).contains(&target[length - 1]) {
        length -= 1;
    }
    target[length] = 0;
}

/// Renders an icon, loading GDI+ only for the duration of the call.
fn render_icon(spec: IconSpec) -> Option<HICON> {
    let _gdiplus = GdiPlus::start()?;
    match spec {
        IconSpec::Percent { percent, fill, ink } => {
            gfx::render_text_icon(&percent.to_string(), fill, ink)
        }
        IconSpec::Unknown => gfx::render_text_icon("...", Some(UNKNOWN_ICON_FILL), FIXED_ICON_INK),
        IconSpec::Error => gfx::render_text_icon("!", Some(ERROR_ICON_FILL), FIXED_ICON_INK),
    }
}

/// Adds the tray icon, also after Explorer restarts and forgets it.
fn add_tray_icon() {
    let now = now_unix();
    let Some((hwnd, spec, tooltip)) = with_app(|app| (app.hwnd, app.icon_spec(), app.tooltip(now)))
    else {
        return;
    };
    let icon = render_icon(spec);
    let mut data = tray_data(hwnd);
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = icon.unwrap_or(null_mut());
    copy_text(&mut data.szTip, &tooltip);
    // SAFETY: data is fully initialized and outlives both calls.
    unsafe {
        Shell_NotifyIconW(NIM_ADD, &data);
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        Shell_NotifyIconW(NIM_SETVERSION, &data);
    }
    if let Some(icon) = icon {
        store_icon(spec, icon);
    }
}

fn store_icon(spec: IconSpec, icon: HICON) {
    let previous = with_app(|app| {
        app.icon_spec = Some(spec);
        std::mem::replace(&mut app.icon, icon)
    });
    if let Some(previous) = previous.filter(|previous| !previous.is_null()) {
        // SAFETY: the previous icon was created by render_icon and is no longer shown.
        unsafe {
            DestroyIcon(previous);
        }
    }
}

/// Brings the tray icon, tooltip and (if open) popup in line with the state.
fn refresh_ui() {
    let now = now_unix();
    let Some((hwnd, spec, tooltip)) = with_app(|app| {
        let spec = app.icon_spec();
        let changed = app.icon_spec != Some(spec);
        (app.hwnd, changed.then_some(spec), app.tooltip(now))
    }) else {
        return;
    };

    let rendered = spec.and_then(|spec| render_icon(spec).map(|icon| (spec, icon)));
    let mut data = tray_data(hwnd);
    data.uFlags = NIF_TIP | NIF_SHOWTIP;
    copy_text(&mut data.szTip, &tooltip);
    if let Some((_, icon)) = rendered {
        data.uFlags |= NIF_ICON;
        data.hIcon = icon;
    }
    // SAFETY: data is fully initialized and outlives the call.
    unsafe {
        Shell_NotifyIconW(NIM_MODIFY, &data);
    }
    if let Some((spec, icon)) = rendered {
        store_icon(spec, icon);
    }
    send_view();
    send_history(ChartProvider::Codex);
    send_history(ChartProvider::Claude);
}

/// Sends the current view to the popup process, if one is open.
fn send_view() {
    let now = now_unix();
    let Some((hwnd, popup, json)) = with_app(|app| {
        app.popup_window
            .map(|popup| (app.hwnd, popup, app.view(now).to_json()))
    })
    .flatten() else {
        return;
    };
    let data = COPYDATASTRUCT {
        dwData: VIEW_DATA,
        cbData: json.len() as u32,
        lpData: json.as_ptr() as *mut c_void,
    };
    let mut reply = 0usize;
    // SAFETY: data and json outlive the synchronous send; the timeout keeps a
    // stuck popup from blocking the tray.
    unsafe {
        SendMessageTimeoutW(
            popup,
            WM_COPYDATA,
            hwnd as WPARAM,
            &data as *const COPYDATASTRUCT as LPARAM,
            SMTO_ABORTIFHUNG,
            VIEW_SEND_TIMEOUT_MILLISECONDS,
            &mut reply,
        );
    }
}

fn handle_popup_event(event: WPARAM, lparam: LPARAM) {
    match event {
        EVENT_READY => {
            let popup = lparam as HWND;
            let mut owner = 0u32;
            // SAFETY: querying the owner of a window handle has no preconditions.
            unsafe {
                GetWindowThreadProcessId(popup, &mut owner);
            }
            // Only the popup process this tray started may receive views.
            let accepted = with_app(|app| {
                let expected = app.popup_process.as_ref().map(Child::id);
                let accepted = owner != 0 && expected == Some(owner);
                if accepted {
                    app.popup_window = Some(popup);
                }
                accepted
            });
            if accepted == Some(true) {
                send_view();
            }
        }
        EVENT_REFRESH => {
            if let Some(provider) = with_app(|app| app.shown_provider()) {
                request_refresh(provider, true);
            }
        }
        EVENT_SWITCH_PROVIDER => {
            with_app(|app| {
                app.selected = match app.shown_provider() {
                    Provider::Codex => Provider::Claude,
                    Provider::Claude => Provider::Codex,
                };
            });
            send_view();
        }
        EVENT_CLOSED => {
            with_app(|app| {
                app.popup_window = None;
                app.popup_closed_at = Some(Instant::now());
            });
        }
        EVENT_OPEN_SETTINGS => open_settings(),
        EVENT_OPEN_HISTORY => {
            if let Some(provider) = with_app(|app| app.shown_provider()) {
                open_history_window(match provider {
                    Provider::Codex => ChartProvider::Codex,
                    Provider::Claude => ChartProvider::Claude,
                });
            }
        }
        _ => {}
    }
}

/// A tray click opens the popup, or closes it if it is already open.
fn toggle_popup() {
    enum Action {
        Open,
        Close(HWND),
        Ignore,
    }
    let action = with_app(|app| {
        if app.popup_alive() {
            // Still starting up when it has no window yet.
            app.popup_window.map_or(Action::Ignore, Action::Close)
        } else if app
            .popup_closed_at
            .is_some_and(|closed| closed.elapsed() < REOPEN_GUARD)
        {
            Action::Ignore
        } else {
            Action::Open
        }
    });
    match action {
        Some(Action::Open) => open_popup(),
        Some(Action::Close(popup)) => {
            // SAFETY: posting to a window handle is safe even if the window is gone.
            unsafe {
                PostMessageW(popup, WM_CLOSE, 0, 0);
            }
        }
        _ => {}
    }
}

/// Starts the popup process beside the tray icon unless one is running.
fn open_popup() {
    let Some((hwnd, alive)) = with_app(|app| (app.hwnd, app.popup_alive())) else {
        return;
    };
    if alive {
        return;
    }
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    // SAFETY: hwnd is the tray window owned by this thread.
    let anchor = unsafe { tray_icon_rect(hwnd) };
    let arguments = [
        POPUP_ARGUMENT.to_string(),
        (hwnd as usize).to_string(),
        anchor.left.to_string(),
        anchor.top.to_string(),
        anchor.right.to_string(),
        anchor.bottom.to_string(),
    ];
    let Ok(process) = Command::new(executable).args(arguments).spawn() else {
        return;
    };
    // The click gave this process the right to take the foreground; pass it
    // on so the popup can activate itself and later notice losing focus.
    // SAFETY: the call only takes a process id.
    unsafe {
        AllowSetForegroundWindow(process.id());
    }
    with_app(|app| {
        app.popup_process = Some(process);
        app.popup_window = None;
    });
}

/// Screen rectangle of the tray icon, or the cursor position if the shell
/// cannot report one.
unsafe fn tray_icon_rect(hwnd: HWND) -> RECT {
    let mut identifier: NOTIFYICONIDENTIFIER = zeroed();
    identifier.cbSize = size_of::<NOTIFYICONIDENTIFIER>() as u32;
    identifier.hWnd = hwnd;
    identifier.uID = TRAY_ICON_ID;
    let mut rect: RECT = zeroed();
    if Shell_NotifyIconGetRect(&identifier, &mut rect) == 0 {
        return rect;
    }
    let mut cursor = POINT { x: 0, y: 0 };
    GetCursorPos(&mut cursor);
    RECT {
        left: cursor.x,
        top: cursor.y,
        right: cursor.x,
        bottom: cursor.y,
    }
}

unsafe fn show_menu(hwnd: HWND) {
    let (refreshing, claude_available) =
        with_app(|app| (app.codex.refreshing, app.claude.is_some())).unwrap_or_default();
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return;
    }
    let refresh_flags = if refreshing {
        MF_STRING | MF_GRAYED
    } else {
        MF_STRING
    };
    AppendMenuW(menu, refresh_flags, COMMAND_REFRESH, w!("Refresh now"));
    AppendMenuW(menu, MF_STRING, COMMAND_SHOW, w!("Show usage"));
    AppendMenuW(menu, MF_STRING, COMMAND_HISTORY, w!("Usage history"));
    if claude_available {
        AppendMenuW(
            menu,
            MF_STRING,
            COMMAND_CLAUDE_HISTORY,
            w!("Claude usage history"),
        );
    }
    AppendMenuW(menu, MF_STRING, COMMAND_SETTINGS, w!("Settings"));
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    AppendMenuW(menu, MF_STRING, COMMAND_EXIT, w!("Exit"));

    let mut cursor = POINT { x: 0, y: 0 };
    GetCursorPos(&mut cursor);
    // The menu only dismisses on an outside click if its owner is foreground.
    SetForegroundWindow(hwnd);
    let command = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
        cursor.x,
        cursor.y,
        0,
        hwnd,
        null(),
    );
    PostMessageW(hwnd, WM_NULL, 0, 0);
    DestroyMenu(menu);

    match command as usize {
        COMMAND_REFRESH => {
            with_app(|app| app.codex_announce = true);
            request_refresh(Provider::Codex, true);
            request_refresh(Provider::Claude, true);
        }
        COMMAND_SHOW => open_popup(),
        COMMAND_HISTORY => open_history_window(ChartProvider::Codex),
        COMMAND_CLAUDE_HISTORY => open_history_window(ChartProvider::Claude),
        COMMAND_SETTINGS => open_settings(),
        COMMAND_EXIT => {
            DestroyWindow(hwnd);
        }
        _ => {}
    }
}

/// Starts a background fetch unless one is already running. For Claude,
/// `manual` asks the client to skip its wait between polls; the client alone
/// decides whether a request is sent.
fn request_refresh(provider: Provider, manual: bool) {
    let target = with_app(|app| match provider {
        Provider::Codex => {
            // A reset being redeemed changes the limits, so no reading is
            // taken until it has finished.
            if app.codex.refreshing || !app.resets.begin_refresh() {
                return None;
            }
            app.codex.refreshing = true;
            app.codex_refresh_started = Some(Instant::now());
            Some((app.hwnd, Fetch::Codex))
        }
        Provider::Claude => {
            if app.claude_read_running {
                return None;
            }
            app.claude_read_running = true;
            let client = Arc::clone(&app.claude_client);
            Some((
                app.hwnd,
                Fetch::Claude {
                    client,
                    force: manual,
                },
            ))
        }
    });
    if let Some((hwnd, fetch)) = target.flatten() {
        if provider == Provider::Codex {
            // SAFETY: hwnd is the tray window owned by this thread.
            unsafe {
                KillTimer(hwnd, TIMER_REFRESH);
            }
        }
        spawn_fetch(hwnd, fetch);
    }
    refresh_ui();
}

fn spawn_fetch(hwnd: HWND, fetch: Fetch) {
    let target = hwnd as usize;
    std::thread::spawn(move || {
        let result = match fetch {
            Fetch::Codex => FetchResult::Codex(codex::fetch_usage()),
            Fetch::Claude { client, force } => {
                // The client's state stays consistent if a read panics, so a
                // poisoned lock is still usable.
                let mut client = client.lock().unwrap_or_else(PoisonError::into_inner);
                FetchResult::Claude(client.read(force, || {
                    // SAFETY: posting to a window handle is safe even if the window is gone.
                    unsafe {
                        PostMessageW(target as HWND, WM_CLAUDE_REQUEST, 0, 0);
                    }
                }))
            }
        };
        post_boxed(target, WM_FETCH_DONE, result);
    });
}

/// Re-reads the settings and decides when the next automatic Codex refresh
/// happens: now, after the rest of the interval, or not at all while paused.
fn schedule_codex() {
    enum Next {
        Nothing,
        RefreshNow,
        Wait(u32),
    }
    let settings = Settings::load();
    let running = is_codex_running();
    let Some((hwnd, next, settings_changed)) = with_app(|app| {
        let settings_changed = app.settings != settings;
        app.settings = settings;
        app.codex_running = running;
        let interval = Duration::from_secs(app.codex_interval_seconds());
        let next = if app.codex.refreshing || interval.is_zero() {
            Next::Nothing
        } else {
            match app.codex_refresh_started.map(|started| started.elapsed()) {
                Some(elapsed) if elapsed < interval => {
                    let remaining = (interval - elapsed).as_millis();
                    Next::Wait(remaining.clamp(1, u128::from(u32::MAX)) as u32)
                }
                _ => Next::RefreshNow,
            }
        };
        (app.hwnd, next, settings_changed)
    }) else {
        return;
    };
    match next {
        Next::Nothing => {
            // SAFETY: hwnd is the tray window owned by this thread.
            unsafe {
                KillTimer(hwnd, TIMER_REFRESH);
            }
            refresh_ui();
        }
        Next::RefreshNow => request_refresh(Provider::Codex, false),
        Next::Wait(milliseconds) => {
            // SAFETY: hwnd is the tray window owned by this thread.
            unsafe {
                SetTimer(hwnd, TIMER_REFRESH, milliseconds, None);
            }
            refresh_ui();
        }
    }
    if settings_changed {
        // New thresholds or reminder lead times change what is due.
        let now = now_unix();
        let step = with_app(|app| {
            let settings = app.reset_settings();
            let refreshing = app.codex.refreshing;
            let App { resets, codex, .. } = app;
            resets.settings_changed(codex.usage.as_ref(), &settings, refreshing, now)
        });
        if let Some(step) = step {
            apply_step(step);
        }
    }
}

fn poll_codex_activity() {
    let running = is_codex_running();
    if with_app(|app| app.codex_running != running) == Some(true) {
        schedule_codex();
    }
}

/// True when another process whose name starts with "codex" exists, using the
/// same rule as the existing tray app.
fn is_codex_running() -> bool {
    // SAFETY: the snapshot handle is closed before returning; the entry is
    // sized before the first call as the API requires.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return false;
        }
        let own_id = GetCurrentProcessId();
        let mut entry: PROCESSENTRY32W = zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut more = Process32FirstW(snapshot, &mut entry) != 0;
        while more && !found {
            found = entry.th32ProcessID != own_id
                && is_codex_process_name(&from_wide(&entry.szExeFile));
            more = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
        found
    }
}

fn is_codex_process_name(file_name: &str) -> bool {
    let lower = file_name.to_lowercase();
    let name = lower.strip_suffix(".exe").unwrap_or(&lower);
    name.starts_with("codex") && name != "codexusagetray"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_processes_are_recognized_by_name() {
        assert!(is_codex_process_name("codex.exe"));
        assert!(is_codex_process_name("Codex-Code-Mode-Host.exe"));
        assert!(!is_codex_process_name("CodexUsageTray.exe"));
        assert!(!is_codex_process_name("usage-tray-rs.exe"));
        assert!(!is_codex_process_name("explorer.exe"));
    }

    #[test]
    fn tooltips_are_truncated_without_splitting_characters() {
        let mut buffer = [0xFFFFu16; 128];
        copy_text(&mut buffer, &"a".repeat(126));
        assert_eq!(from_wide(&buffer).len(), 126);

        let mut buffer = [0xFFFFu16; 128];
        copy_text(&mut buffer, &format!("{}\u{1F600}", "a".repeat(126)));
        assert_eq!(from_wide(&buffer), "a".repeat(126));
    }
}
