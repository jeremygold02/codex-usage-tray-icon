//! The Settings window process. The tray starts it when Settings is chosen;
//! it edits the settings file it was given and exits when it is closed, so
//! the window, its fonts and its drawing surface cost nothing while closed.
//!
//! Everything shown and every reaction to input lives in `settings_view.rs`.
//! This file is the Win32 side: the window, turning messages into view
//! events, carrying out the view's commands, and talking to the tray.
//!
//! As in the popup, the state is only borrowed for short, non-reentrant
//! sections. Anything that can dispatch messages back into the window
//! procedure (moving or destroying the window, capturing the mouse, message
//! boxes, the color picker) runs outside those borrows.

use std::cell::RefCell;
use std::ffi::{c_void, OsString};
use std::mem::{size_of, zeroed};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use windows_sys::w;
use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
    EndPaint, GetDC, GetMonitorInfoW, InvalidateRect, MonitorFromPoint, MonitorFromWindow,
    ReleaseDC, ScreenToClient, SelectObject, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    PAINTSTRUCT, SRCCOPY,
};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::Dialogs::{
    ChooseColorW, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW,
};
use windows_sys::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowW,
    GetCaretBlinkTime, GetClientRect, GetCursorPos, GetMessageW, IsIconic, KillTimer, LoadCursorW,
    LoadIconW, MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassExW, ReplyMessage,
    SendMessageTimeoutW, SetCursor, SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow,
    TranslateMessage, CS_DBLCLKS, HTCLIENT, IDC_ARROW, IDC_HAND, IDC_IBEAM, MB_ICONWARNING, MB_OK,
    MINMAXINFO, MSG, SMTO_ABORTIFHUNG, SWP_NOACTIVATE, SWP_NOZORDER, SW_RESTORE, SW_SHOWNORMAL,
    WHEEL_DELTA, WINDOW_STYLE, WM_CAPTURECHANGED, WM_CHAR, WM_COPYDATA, WM_DESTROY, WM_DPICHANGED,
    WM_ERASEBKGND, WM_GETMINMAXINFO, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_SETCURSOR, WM_SETFOCUS,
    WM_SETTINGCHANGE, WM_SIZE, WM_SYSCOLORCHANGE, WM_THEMECHANGED, WM_TIMER, WNDCLASSEXW,
    WS_CAPTION, WS_OVERLAPPED, WS_SYSMENU, WS_THICKFRAME,
};

use crate::controls::Cursor;
use crate::gfx::{GdiPlus, Graphics};
use crate::ipc::{
    MAX_SETTINGS_STATUS_BYTES, SETTINGS_ARGUMENT, SETTINGS_EVENT_CHECK_UPDATES,
    SETTINGS_EVENT_CLOSED, SETTINGS_EVENT_DIRTY, SETTINGS_EVENT_READY, SETTINGS_EVENT_SAVED,
    SETTINGS_STATUS_DATA, WM_SETTINGS_EVENT,
};
use crate::settings::{system_uses_dark_theme, Settings};
use crate::settings_view::{
    caption_color, ColorTarget, Command, Key, Status, View, CLIENT_SIZE, MIN_WINDOW_SIZE,
};
use crate::win::{wide, APP_ICON_RESOURCE};

const CLASS_NAME: *const u16 = w!("UsageTrayRs.Settings");
const TITLE: *const u16 = w!("Codex Usage Tray Settings");
const MESSAGE_TITLE: &str = "Codex Usage Settings";
/// Resizable, in the taskbar, and without minimize or maximize buttons.
const STYLE: WINDOW_STYLE = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_THICKFRAME;

const WM_MOUSELEAVE: u32 = 0x02A3;
const VK_BACK: usize = 0x08;
const VK_TAB: usize = 0x09;
const VK_RETURN: usize = 0x0D;
const VK_SHIFT: i32 = 0x10;
const VK_CONTROL: i32 = 0x11;
const VK_ESCAPE: usize = 0x1B;
const VK_SPACE: usize = 0x20;
const VK_PRIOR: usize = 0x21;
const VK_NEXT: usize = 0x22;
const VK_END: usize = 0x23;
const VK_HOME: usize = 0x24;
const VK_LEFT: usize = 0x25;
const VK_UP: usize = 0x26;
const VK_RIGHT: usize = 0x27;
const VK_DOWN: usize = 0x28;
const VK_DELETE: usize = 0x2E;
const VK_A: usize = 0x41;
const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
const DWMWA_CAPTION_COLOR: u32 = 35;
const DWMWA_TEXT_COLOR: u32 = 36;
const DWMWA_COLOR_DEFAULT: u32 = 0xFFFF_FFFF;
const WHITE: COLORREF = 0x00FF_FFFF;

const TIMER_CARET: usize = 1;
/// Used when the system reports no usable blink time.
const DEFAULT_CARET_BLINK_MILLISECONDS: u32 = 530;
const NO_CARET_BLINK: u32 = u32::MAX;
const STATUS_SEND_TIMEOUT_MILLISECONDS: u32 = 1_000;

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Whom the window reports to and which file it edits.
pub struct Launch {
    tray: HWND,
    path: PathBuf,
}

impl Launch {
    /// Parses `<tray hwnd> <settings path>`. A tray handle of zero runs the
    /// window on its own, with nobody to report to.
    pub fn parse(arguments: &[String]) -> Option<Self> {
        let [tray, path] = arguments else {
            return None;
        };
        if path.is_empty() {
            return None;
        }
        Some(Self {
            tray: tray.parse::<usize>().ok()? as HWND,
            path: PathBuf::from(path),
        })
    }
}

struct State {
    tray: HWND,
    path: PathBuf,
    view: View,
    scale: f32,
    /// Whether the title bar was last themed dark, if it has been themed.
    title_bar_dark: Option<bool>,
    /// What the tray was last told about unsaved edits.
    reported_dirty: bool,
    caret_timer: bool,
    /// The color picker's custom colors, kept between uses.
    custom_colors: [COLORREF; 16],
}

/// Runs `action` with the window state, or returns `None` if the state is
/// already borrowed further up the stack or the window is not set up.
fn with_state<R>(action: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|cell| {
        cell.try_borrow_mut()
            .ok()
            .and_then(|mut state| state.as_mut().map(action))
    })
}

/// The command-line arguments that start the Settings window for `tray`,
/// editing the settings file at `path`.
pub fn launch_arguments(tray: HWND, path: &Path) -> [OsString; 3] {
    [
        SETTINGS_ARGUMENT.into(),
        (tray as usize).to_string().into(),
        path.as_os_str().to_os_string(),
    ]
}

/// Sends the tray's status to a Settings window that reported ready.
/// Returns true when the window took it. Call it outside any state borrow:
/// the send waits for the window, and messages can arrive meanwhile.
pub fn send_status(tray: HWND, window: HWND, status: &Status) -> bool {
    let json = status.to_json();
    let data = COPYDATASTRUCT {
        dwData: SETTINGS_STATUS_DATA,
        cbData: json.len() as u32,
        lpData: json.as_ptr() as *mut c_void,
    };
    let mut reply = 0usize;
    // SAFETY: data and json outlive the synchronous send; the timeout keeps a
    // stuck window from blocking the caller.
    let sent = unsafe {
        SendMessageTimeoutW(
            window,
            WM_COPYDATA,
            tray as WPARAM,
            &data as *const COPYDATASTRUCT as LPARAM,
            SMTO_ABORTIFHUNG,
            STATUS_SEND_TIMEOUT_MILLISECONDS,
            &mut reply,
        )
    };
    sent != 0 && reply == 1
}

/// Brings a Settings window to the front, restoring it if it is minimized.
pub fn bring_to_front(window: HWND) {
    // SAFETY: these calls accept any window handle, including a stale one.
    unsafe {
        if IsIconic(window) != 0 {
            ShowWindow(window, SW_RESTORE);
        }
        SetForegroundWindow(window);
    }
}

/// Creates the Settings window and runs until it is closed.
pub fn run(launch: Launch) {
    // SAFETY: looking a window up by a static class name has no preconditions.
    let existing = unsafe { FindWindowW(CLASS_NAME, null()) };
    if !existing.is_null() {
        // One Settings window at a time, however this process was started.
        bring_to_front(existing);
        return;
    }
    let Some(_gdiplus) = GdiPlus::start() else {
        return;
    };
    let settings = Settings::load_from(&launch.path);
    let system_dark = system_uses_dark_theme();
    // SAFETY: standard window-class registration and message loop; every
    // pointer passed refers to a live local or a static string.
    unsafe {
        let instance = GetModuleHandleW(null());
        let mut class: WNDCLASSEXW = zeroed();
        class.cbSize = size_of::<WNDCLASSEXW>() as u32;
        // Double clicks select the whole number in a numeric field.
        class.style = CS_DBLCLKS;
        class.lpfnWndProc = Some(window_procedure);
        class.hInstance = instance;
        class.hIcon = LoadIconW(instance, APP_ICON_RESOURCE);
        class.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
        class.lpszClassName = CLASS_NAME;
        if RegisterClassExW(&class) == 0 {
            return;
        }

        let (dpi, bounds) = initial_placement();
        let hwnd = CreateWindowExW(
            0,
            CLASS_NAME,
            TITLE,
            STYLE,
            bounds.left,
            bounds.top,
            bounds.right - bounds.left,
            bounds.bottom - bounds.top,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            return;
        }
        let tray = launch.tray;
        // The window knows its DPI best; the monitor's is the fallback.
        let dpi = match GetDpiForWindow(hwnd) {
            0 => dpi,
            actual => actual,
        };
        STATE.with(|cell| {
            *cell.borrow_mut() = Some(State {
                tray,
                path: launch.path,
                view: View::new(&settings, system_dark),
                scale: scale_for_dpi(dpi),
                title_bar_dark: None,
                reported_dirty: false,
                caret_timer: false,
                custom_colors: [WHITE; 16],
            });
        });
        refresh(hwnd);
        ShowWindow(hwnd, SW_SHOWNORMAL);
        SetForegroundWindow(hwnd);
        notify_tray(SETTINGS_EVENT_READY, hwnd as LPARAM);

        let mut message: MSG = zeroed();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    // The view's fonts must be released before GDI+ shuts down.
    drop(STATE.with(|cell| cell.borrow_mut().take()));
}

fn scale_for_dpi(dpi: u32) -> f32 {
    dpi.max(48) as f32 / 96.0
}

/// The DPI of the monitor the pointer is on, and the window rectangle that
/// centers the window in that monitor's work area.
unsafe fn initial_placement() -> (u32, RECT) {
    let mut cursor = POINT { x: 0, y: 0 };
    GetCursorPos(&mut cursor);
    let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
    let (mut dpi, mut dpi_y) = (96u32, 96u32);
    GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut dpi_y);
    let scale = scale_for_dpi(dpi);
    let mut frame = RECT {
        left: 0,
        top: 0,
        right: (CLIENT_SIZE.0 * scale).round() as i32,
        bottom: (CLIENT_SIZE.1 * scale).round() as i32,
    };
    AdjustWindowRectExForDpi(&mut frame, STYLE, 0, 0, dpi);
    let size = (frame.right - frame.left, frame.bottom - frame.top);
    let work = work_area(monitor).unwrap_or(RECT {
        left: 0,
        top: 0,
        right: size.0,
        bottom: size.1,
    });
    (dpi, centered(work, size))
}

unsafe fn work_area(monitor: HMONITOR) -> Option<RECT> {
    let mut info: MONITORINFO = zeroed();
    info.cbSize = size_of::<MONITORINFO>() as u32;
    (GetMonitorInfoW(monitor, &mut info) != 0).then_some(info.rcWork)
}

/// A rectangle of `size`, shrunk if needed to fit `work`, centered in it.
fn centered(work: RECT, size: (i32, i32)) -> RECT {
    let width = size.0.min(work.right - work.left);
    let height = size.1.min(work.bottom - work.top);
    let left = work.left + (work.right - work.left - width) / 2;
    let top = work.top + (work.bottom - work.top - height) / 2;
    RECT {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

/// The smallest window size at `scale`, never larger than the work area so
/// the window always fits the screen; its content scrolls instead.
fn minimum_size(scale: f32, work: Option<RECT>) -> (i32, i32) {
    let wanted = (
        (MIN_WINDOW_SIZE.0 * scale).round() as i32,
        (MIN_WINDOW_SIZE.1 * scale).round() as i32,
    );
    match work {
        Some(work) => (
            wanted.0.min(work.right - work.left),
            wanted.1.min(work.bottom - work.top),
        ),
        None => wanted,
    }
}

/// Swaps between 0xAARRGGBB and the 0x00BBGGRR layout used by GDI and DWM.
fn colorref(argb: u32) -> COLORREF {
    ((argb & 0xFF) << 16) | (argb & 0xFF00) | ((argb >> 16) & 0xFF)
}

fn argb(color: COLORREF) -> u32 {
    0xFF00_0000 | colorref(color)
}

fn key_from(virtual_key: usize, control: bool) -> Option<Key> {
    Some(match virtual_key {
        VK_TAB => Key::Tab,
        VK_RETURN => Key::Enter,
        VK_ESCAPE => Key::Escape,
        VK_SPACE => Key::Space,
        VK_LEFT => Key::Left,
        VK_RIGHT => Key::Right,
        VK_UP => Key::Up,
        VK_DOWN => Key::Down,
        VK_HOME => Key::Home,
        VK_END => Key::End,
        VK_PRIOR => Key::PageUp,
        VK_NEXT => Key::PageDown,
        VK_BACK => Key::Backspace,
        VK_DELETE => Key::Delete,
        VK_A if control => Key::SelectAll,
        _ => return None,
    })
}

fn point_from(lparam: LPARAM) -> (f32, f32) {
    let x = (lparam & 0xFFFF) as i16;
    let y = ((lparam >> 16) & 0xFFFF) as i16;
    (f32::from(x), f32::from(y))
}

unsafe extern "system" fn window_procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_SIZE => {
            refresh(hwnd);
            0
        }
        WM_GETMINMAXINFO => {
            // SAFETY: for this message lparam points to a MINMAXINFO.
            let info = &mut *(lparam as *mut MINMAXINFO);
            let scale = with_state(|state| state.scale)
                .unwrap_or_else(|| scale_for_dpi(GetDpiForWindow(hwnd)));
            let work = work_area(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST));
            let (width, height) = minimum_size(scale, work);
            info.ptMinTrackSize = POINT {
                x: width,
                y: height,
            };
            0
        }
        WM_DPICHANGED => {
            let dpi = ((wparam >> 16) & 0xFFFF) as u32;
            with_state(|state| state.scale = scale_for_dpi(dpi));
            // SAFETY: for this message lparam points to the suggested RECT.
            let suggested = *(lparam as *const RECT);
            SetWindowPos(
                hwnd,
                null_mut(),
                suggested.left,
                suggested.top,
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            refresh(hwnd);
            0
        }
        WM_MOUSEMOVE => {
            let point = point_from(lparam);
            let changed = with_state(|state| state.view.pointer_moved(Some(point)));
            let mut track = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            TrackMouseEvent(&mut track);
            if changed == Some(true) {
                refresh(hwnd);
            }
            0
        }
        WM_MOUSELEAVE => {
            if with_state(|state| state.view.pointer_moved(None)) == Some(true) {
                refresh(hwnd);
            }
            0
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            let (x, y) = point_from(lparam);
            // Keep receiving the mouse while a button is held, so a drag or
            // a click that ends outside the window is still seen through.
            SetCapture(hwnd);
            let double_click = message == WM_LBUTTONDBLCLK;
            with_measuring_graphics(hwnd, |g| {
                with_state(|state| state.view.pointer_pressed(g, x, y, double_click));
            });
            refresh(hwnd);
            0
        }
        WM_LBUTTONUP => {
            let (x, y) = point_from(lparam);
            let command = with_state(|state| state.view.pointer_released(x, y)).flatten();
            ReleaseCapture();
            refresh(hwnd);
            run_command(hwnd, command);
            0
        }
        WM_CAPTURECHANGED => {
            with_state(|state| state.view.capture_lost());
            refresh(hwnd);
            0
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam >> 16) & 0xFFFF) as i16;
            let notches = f32::from(delta) / WHEEL_DELTA as f32;
            if with_state(|state| state.view.wheel(notches)) == Some(true) {
                refresh(hwnd);
            }
            0
        }
        WM_KEYDOWN => {
            let control = GetKeyState(VK_CONTROL) < 0;
            let shift = GetKeyState(VK_SHIFT) < 0;
            if let Some(key) = key_from(wparam, control) {
                let command = with_state(|state| state.view.key(key, shift)).flatten();
                refresh(hwnd);
                run_command(hwnd, command);
            }
            0
        }
        WM_CHAR => {
            if let Some(character) = u32::try_from(wparam).ok().and_then(char::from_u32) {
                with_state(|state| state.view.character(character));
                refresh(hwnd);
            }
            0
        }
        WM_SETCURSOR if (lparam & 0xFFFF) as u32 == HTCLIENT => {
            let mut point = POINT { x: 0, y: 0 };
            GetCursorPos(&mut point);
            ScreenToClient(hwnd, &mut point);
            let cursor = with_state(|state| state.view.cursor_at(point.x as f32, point.y as f32));
            let name = match cursor {
                Some(Cursor::Hand) => IDC_HAND,
                Some(Cursor::Text) => IDC_IBEAM,
                Some(Cursor::Arrow) | None => IDC_ARROW,
            };
            SetCursor(LoadCursorW(null_mut(), name));
            1
        }
        WM_SETFOCUS => {
            with_state(|state| state.view.focus_gained());
            refresh(hwnd);
            0
        }
        WM_KILLFOCUS => {
            with_state(|state| state.view.focus_lost());
            refresh(hwnd);
            0
        }
        WM_TIMER => {
            if wparam == TIMER_CARET {
                with_state(|state| state.view.blink_caret());
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_SETTINGCHANGE | WM_SYSCOLORCHANGE | WM_THEMECHANGED => {
            // The Windows app theme may have changed under a window that
            // follows it.
            let system_dark = system_uses_dark_theme();
            with_state(|state| state.view.set_system_dark(system_dark));
            refresh(hwnd);
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        WM_COPYDATA => LRESULT::from(receive_status(hwnd, wparam, lparam)),
        WM_DESTROY => {
            notify_tray(SETTINGS_EVENT_CLOSED, 0);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

/// Posts an event to the tray, if this window was started by one.
fn notify_tray(event: usize, lparam: LPARAM) {
    let tray = with_state(|state| state.tray).unwrap_or(null_mut());
    // A null handle would post to this thread's own queue instead.
    if !tray.is_null() {
        // SAFETY: posting to a window handle is safe even if the window is gone.
        unsafe {
            PostMessageW(tray, WM_SETTINGS_EVENT, event, lparam);
        }
    }
}

/// Runs `action` with a surface that measures text the way painting does.
unsafe fn with_measuring_graphics(hwnd: HWND, action: impl FnOnce(&Graphics)) {
    let dc = GetDC(hwnd);
    if dc.is_null() {
        return;
    }
    if let Some(g) = Graphics::from_hdc(dc) {
        action(&g);
    }
    ReleaseDC(hwnd, dc);
}

/// Brings everything derived from the view in line with it after it may have
/// changed: control positions, the title bar, the caret timer, what the tray
/// knows about unsaved edits, and what is on screen.
unsafe fn refresh(hwnd: HWND) {
    let mut client: RECT = zeroed();
    GetClientRect(hwnd, &mut client);
    let (width, height) = (client.right as f32, client.bottom as f32);
    with_measuring_graphics(hwnd, |g| {
        with_state(|state| state.view.render(g, state.scale, width, height, false));
    });

    struct Changes {
        title_bar: Option<bool>,
        dirty: Option<bool>,
        caret_timer: Option<bool>,
    }
    let Some(changes) = with_state(|state| {
        let changed = |known: &mut bool, now: bool| {
            let changed = *known != now;
            *known = now;
            changed.then_some(now)
        };
        let dark = state.view.dark();
        let title_bar = (state.title_bar_dark != Some(dark)).then_some(dark);
        state.title_bar_dark = Some(dark);
        Changes {
            title_bar,
            dirty: changed(&mut state.reported_dirty, state.view.has_unsaved_changes()),
            caret_timer: changed(&mut state.caret_timer, state.view.is_editing()),
        }
    }) else {
        return;
    };
    if let Some(dark) = changes.title_bar {
        apply_title_bar(hwnd, dark);
    }
    if let Some(dirty) = changes.dirty {
        notify_tray(SETTINGS_EVENT_DIRTY, LPARAM::from(dirty));
    }
    match changes.caret_timer {
        Some(true) => {
            let blink = match GetCaretBlinkTime() {
                0 => DEFAULT_CARET_BLINK_MILLISECONDS,
                blink => blink,
            };
            // The user may have turned blinking off, which leaves the caret on.
            if blink != NO_CARET_BLINK {
                SetTimer(hwnd, TIMER_CARET, blink, None);
            }
        }
        Some(false) => {
            KillTimer(hwnd, TIMER_CARET);
        }
        None => {}
    }
    InvalidateRect(hwnd, null(), 0);
}

/// Colors the title bar to match the window where the system supports it.
unsafe fn apply_title_bar(hwnd: HWND, dark: bool) {
    let set = |attribute: u32, value: u32| {
        // SAFETY: the value outlives the call and its size is passed with it.
        // Systems that do not know an attribute reject it without effect.
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                attribute,
                (&value as *const u32).cast(),
                size_of::<u32>() as u32,
            );
        }
    };
    set(DWMWA_USE_IMMERSIVE_DARK_MODE, u32::from(dark));
    set(
        DWMWA_CAPTION_COLOR,
        caption_color(dark).map_or(DWMWA_COLOR_DEFAULT, colorref),
    );
    set(
        DWMWA_TEXT_COLOR,
        if dark { WHITE } else { DWMWA_COLOR_DEFAULT },
    );
}

unsafe fn paint(hwnd: HWND) {
    let mut paint: PAINTSTRUCT = zeroed();
    let target = BeginPaint(hwnd, &mut paint);
    let mut client: RECT = zeroed();
    GetClientRect(hwnd, &mut client);
    let (width, height) = (client.right, client.bottom);
    if width > 0 && height > 0 {
        // Draw to an off-screen bitmap first so the window never flickers.
        let buffer = CreateCompatibleDC(target);
        let bitmap = CreateCompatibleBitmap(target, width, height);
        let previous = SelectObject(buffer, bitmap);
        if let Some(g) = Graphics::from_hdc(buffer) {
            with_state(|state| {
                state
                    .view
                    .render(&g, state.scale, width as f32, height as f32, true);
            });
        }
        BitBlt(target, 0, 0, width, height, buffer, 0, 0, SRCCOPY);
        SelectObject(buffer, previous);
        DeleteObject(bitmap);
        DeleteDC(buffer);
    }
    EndPaint(hwnd, &paint);
}

unsafe fn run_command(hwnd: HWND, command: Option<Command>) {
    match command {
        Some(Command::Save) => save(hwnd),
        Some(Command::Close) => {
            DestroyWindow(hwnd);
        }
        Some(Command::PickColor(target)) => pick_color(hwnd, target),
        Some(Command::CheckUpdates) => notify_tray(SETTINGS_EVENT_CHECK_UPDATES, 0),
        None => {}
    }
}

/// Writes the edited values to the settings file and closes the window, or
/// explains why they could not be written and leaves it open.
unsafe fn save(hwnd: HWND) {
    let Some(path) = with_state(|state| state.path.clone()) else {
        return;
    };
    // Start from the file as it is now: the tray may have recorded values
    // this window does not edit since the window opened.
    let mut candidate = Settings::load_from(&path);
    if let Some(message) = candidate.save_blocked_message() {
        show_warning(hwnd, &message);
        return;
    }
    with_state(|state| state.view.apply_to(&mut candidate));
    match candidate.save_to(&path) {
        Ok(()) => {
            with_state(|state| state.view.mark_saved());
            notify_tray(SETTINGS_EVENT_SAVED, 0);
            DestroyWindow(hwnd);
        }
        Err(error) => {
            show_warning(hwnd, &format!("Could not save settings:\r\n\r\n{error}"));
            refresh(hwnd);
        }
    }
}

unsafe fn show_warning(hwnd: HWND, message: &str) {
    let (message, title) = (wide(message), wide(MESSAGE_TITLE));
    MessageBoxW(
        hwnd,
        message.as_ptr(),
        title.as_ptr(),
        MB_OK | MB_ICONWARNING,
    );
}

/// Lets the user choose a tray color with the system color picker.
unsafe fn pick_color(hwnd: HWND, target: ColorTarget) {
    let Some((initial, mut custom_colors)) =
        with_state(|state| (state.view.begin_color_choice(target), state.custom_colors))
    else {
        return;
    };
    // An open picker counts as an unsaved edit; let the tray know.
    refresh(hwnd);
    let mut choice: CHOOSECOLORW = zeroed();
    choice.lStructSize = size_of::<CHOOSECOLORW>() as u32;
    choice.hwndOwner = hwnd;
    choice.rgbResult = colorref(initial);
    choice.lpCustColors = custom_colors.as_mut_ptr();
    choice.Flags = CC_FULLOPEN | CC_RGBINIT;
    // SAFETY: the structure and the sixteen custom colors it points to
    // outlive the modal call.
    let chosen = (ChooseColorW(&mut choice) != 0).then(|| argb(choice.rgbResult));
    with_state(|state| {
        state.custom_colors = custom_colors;
        state.view.finish_color_choice(target, chosen);
    });
    refresh(hwnd);
}

/// Accepts a status from the tray and shows it.
unsafe fn receive_status(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) -> bool {
    let data = lparam as *const COPYDATASTRUCT;
    if data.is_null() {
        return false;
    }
    let data = &*data;
    if data.dwData != SETTINGS_STATUS_DATA
        || data.lpData.is_null()
        || data.cbData > MAX_SETTINGS_STATUS_BYTES
    {
        return false;
    }
    // Only the tray that started this window may speak for the tray.
    let tray = with_state(|state| state.tray).unwrap_or(null_mut());
    if !tray.is_null() && wparam as HWND != tray {
        return false;
    }
    // SAFETY: for WM_COPYDATA the system guarantees lpData points to cbData readable bytes.
    let bytes = std::slice::from_raw_parts(data.lpData as *const u8, data.cbData as usize);
    let Some(status) = Status::from_json(bytes) else {
        return false;
    };
    // The data is copied; let the tray continue before doing any window work.
    ReplyMessage(1);
    with_state(|state| state.view.set_status(status));
    refresh(hwnd);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_arguments_carry_the_tray_window_and_settings_path() {
        let path = Path::new(r"C:\Users\Some One\AppData\Roaming\CodexUsageTrayRs\settings.json");
        let arguments = launch_arguments(1312usize as HWND, path);
        assert_eq!(arguments[0], SETTINGS_ARGUMENT);
        let rest: Vec<String> = arguments[1..]
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        let launch = Launch::parse(&rest).unwrap();
        assert_eq!(launch.tray as usize, 1312);
        assert_eq!(launch.path, path);

        // Without a tray the window still runs.
        let alone = Launch::parse(&["0".to_string(), "settings.json".to_string()]).unwrap();
        assert!(alone.tray.is_null());
        assert!(Launch::parse(&rest[..1]).is_none());
        assert!(Launch::parse(&["x".to_string(), "settings.json".to_string()]).is_none());
        assert!(Launch::parse(&["0".to_string(), String::new()]).is_none());
        assert!(Launch::parse(&["0".to_string(), "a".to_string(), "b".to_string()]).is_none());
    }

    #[test]
    fn colors_convert_to_and_from_colorref_byte_order() {
        assert_eq!(colorref(0xFF48_515B), 0x005B_5148);
        assert_eq!(argb(0x005B_5148), 0xFF48_515B);
        assert_eq!(argb(colorref(0xFF00_78D7)), 0xFF00_78D7);
    }

    #[test]
    fn keys_map_to_what_the_view_understands() {
        assert_eq!(key_from(VK_TAB, false), Some(Key::Tab));
        assert_eq!(key_from(VK_RETURN, false), Some(Key::Enter));
        assert_eq!(key_from(VK_ESCAPE, true), Some(Key::Escape));
        assert_eq!(key_from(VK_SPACE, false), Some(Key::Space));
        assert_eq!(key_from(VK_PRIOR, false), Some(Key::PageUp));
        assert_eq!(key_from(VK_NEXT, false), Some(Key::PageDown));
        assert_eq!(key_from(VK_DELETE, false), Some(Key::Delete));
        assert_eq!(key_from(VK_BACK, false), Some(Key::Backspace));
        assert_eq!(key_from(VK_A, true), Some(Key::SelectAll));
        assert_eq!(key_from(VK_A, false), None);
        assert_eq!(key_from(0x70, false), None);
    }

    #[test]
    fn the_window_is_centered_and_never_larger_than_the_work_area() {
        let work = RECT {
            left: 100,
            top: 50,
            right: 2020,
            bottom: 1090,
        };
        let placed = centered(work, (1216, 771));
        assert_eq!(
            (placed.left, placed.top, placed.right, placed.bottom),
            (452, 184, 1668, 955)
        );
        // At 200% the wanted size exceeds this screen: fill the work area.
        let placed = centered(work, (2416, 1503));
        assert_eq!(
            (placed.left, placed.top, placed.right, placed.bottom),
            (100, 50, 2020, 1090)
        );

        assert_eq!(minimum_size(1.0, Some(work)), (980, 650));
        assert_eq!(minimum_size(1.5, Some(work)), (1470, 975));
        assert_eq!(minimum_size(2.0, Some(work)), (1920, 1040));
        assert_eq!(minimum_size(2.0, None), (1960, 1300));
    }

    #[test]
    fn mouse_positions_are_signed() {
        let lparam = ((-3i16 as u16 as isize) << 16) | (250i16 as u16 as isize);
        assert_eq!(point_from(lparam), (250.0, -3.0));
        assert_eq!(scale_for_dpi(144), 1.5);
    }
}
