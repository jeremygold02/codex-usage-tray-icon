//! The on-demand history window. The tray owns its data and sends prepared
//! payloads; the window owns drawing and input. Calls that can dispatch
//! window messages run outside the short-lived state borrows.

use std::cell::RefCell;
use std::ffi::{c_void, OsString};
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};

use windows_sys::core::PCWSTR;
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
use windows_sys::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowW, GetClientRect,
    GetCursorPos, GetMessageW, IsIconic, LoadCursorW, LoadIconW, PostMessageW, PostQuitMessage,
    RegisterClassExW, ReplyMessage, SendMessageTimeoutW, SetCursor, SetForegroundWindow,
    SetWindowPos, ShowWindow, TranslateMessage, HTCLIENT, IDC_ARROW, IDC_HAND, IDC_IBEAM,
    IDI_APPLICATION, MINMAXINFO, MSG, SMTO_ABORTIFHUNG, SWP_NOACTIVATE, SWP_NOZORDER, SW_RESTORE,
    SW_SHOWNORMAL, WINDOW_STYLE, WM_CAPTURECHANGED, WM_COPYDATA, WM_DESTROY, WM_DPICHANGED,
    WM_ERASEBKGND, WM_GETMINMAXINFO, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_PAINT, WM_SETCURSOR, WM_SETFOCUS, WM_SETTINGCHANGE, WM_SIZE, WM_SYSCHAR,
    WM_SYSCOLORCHANGE, WM_THEMECHANGED, WNDCLASSEXW, WS_CAPTION, WS_OVERLAPPED, WS_SYSMENU,
    WS_THICKFRAME,
};

use crate::chart_data::{ChartData, ChartProvider};
use crate::chart_view::{caption_color, Command, Key, View, CLIENT_SIZE, MIN_WINDOW_SIZE};
use crate::controls::Cursor;
use crate::gfx::{GdiPlus, Graphics};
use crate::ipc::{
    history_event, HISTORY_ARGUMENT, HISTORY_DATA, HISTORY_EVENT_CLOSED, HISTORY_EVENT_READY,
    MAX_HISTORY_DATA_BYTES, WM_HISTORY_EVENT,
};
use crate::localtime::TimeZone;
use crate::settings::system_uses_dark_theme;
use crate::win::wide;

const STYLE: WINDOW_STYLE = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_THICKFRAME;
const WM_MOUSELEAVE: u32 = 0x02A3;
const VK_TAB: usize = 0x09;
const VK_SHIFT: i32 = 0x10;
const VK_ESCAPE: usize = 0x1B;
const VK_END: usize = 0x23;
const VK_HOME: usize = 0x24;
const VK_LEFT: usize = 0x25;
const VK_RIGHT: usize = 0x27;
const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
const DWMWA_CAPTION_COLOR: u32 = 35;
const DWMWA_TEXT_COLOR: u32 = 36;
const DWMWA_COLOR_DEFAULT: u32 = 0xFFFF_FFFF;
const WHITE: COLORREF = 0x00FF_FFFF;
const SEND_TIMEOUT_MILLISECONDS: u32 = 1_000;

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// The tray to report to and the provider this chart displays.
pub struct Launch {
    tray: HWND,
    provider: ChartProvider,
}

impl Launch {
    /// Parses `<tray hwnd> <provider>`.
    pub fn parse(arguments: &[String]) -> Option<Self> {
        let [tray, provider] = arguments else {
            return None;
        };
        Some(Self {
            tray: tray.parse::<usize>().ok()? as HWND,
            provider: ChartProvider::from_argument(provider)?,
        })
    }
}

struct State {
    tray: HWND,
    provider: ChartProvider,
    view: View,
    scale: f32,
    title_bar_dark: Option<bool>,
}

fn with_state<R>(action: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|cell| {
        cell.try_borrow_mut()
            .ok()
            .and_then(|mut state| state.as_mut().map(action))
    })
}

/// Arguments used by the tray to start a chart for one provider.
pub fn launch_arguments(tray: HWND, provider: ChartProvider) -> [OsString; 3] {
    [
        HISTORY_ARGUMENT.into(),
        (tray as usize).to_string().into(),
        provider.argument().into(),
    ]
}

/// Sends a payload to a ready window. Call outside any state borrow.
pub fn send_data(tray: HWND, window: HWND, data: &ChartData) -> bool {
    let json = data.to_json();
    if json.len() > MAX_HISTORY_DATA_BYTES as usize {
        return false;
    }
    let packet = COPYDATASTRUCT {
        dwData: HISTORY_DATA,
        cbData: json.len() as u32,
        lpData: json.as_ptr() as *mut c_void,
    };
    let mut reply = 0usize;
    // SAFETY: packet and JSON bytes live through the synchronous bounded send.
    let sent = unsafe {
        SendMessageTimeoutW(
            window,
            WM_COPYDATA,
            tray as WPARAM,
            &packet as *const COPYDATASTRUCT as LPARAM,
            SMTO_ABORTIFHUNG,
            SEND_TIMEOUT_MILLISECONDS,
            &mut reply,
        )
    };
    sent != 0 && reply == 1
}

/// Restores and raises an existing chart window.
pub fn bring_to_front(window: HWND) {
    // SAFETY: the functions accept window handles even if the window is gone.
    unsafe {
        if IsIconic(window) != 0 {
            ShowWindow(window, SW_RESTORE);
        }
        SetForegroundWindow(window);
    }
}

fn class_name(provider: ChartProvider) -> PCWSTR {
    match provider {
        ChartProvider::Codex => w!("UsageTrayRs.History.Codex"),
        ChartProvider::Claude => w!("UsageTrayRs.History.Claude"),
    }
}

/// Creates the history window and runs until it is closed.
pub fn run(launch: Launch) {
    let class_name = class_name(launch.provider);
    // SAFETY: class_name is a static null-terminated string.
    let existing = unsafe { FindWindowW(class_name, null()) };
    if !existing.is_null() {
        bring_to_front(existing);
        return;
    }
    let Some(_gdiplus) = GdiPlus::start() else {
        return;
    };
    let title = wide(&format!("{} Usage History", launch.provider.name()));
    // SAFETY: registration and creation receive live or static strings, and
    // the message loop uses a local MSG. State is installed after creation.
    unsafe {
        let instance = GetModuleHandleW(null());
        let mut class: WNDCLASSEXW = zeroed();
        class.cbSize = size_of::<WNDCLASSEXW>() as u32;
        class.lpfnWndProc = Some(window_procedure);
        class.hInstance = instance;
        class.hIcon = LoadIconW(null_mut(), IDI_APPLICATION);
        class.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
        class.lpszClassName = class_name;
        if RegisterClassExW(&class) == 0 {
            return;
        }
        let (dpi, bounds) = initial_placement();
        let hwnd = CreateWindowExW(
            0,
            class_name,
            title.as_ptr(),
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
        let dpi = match GetDpiForWindow(hwnd) {
            0 => dpi,
            actual => actual,
        };
        STATE.with(|cell| {
            *cell.borrow_mut() = Some(State {
                tray: launch.tray,
                provider: launch.provider,
                view: View::new(TimeZone::system(), system_uses_dark_theme()),
                scale: scale_for_dpi(dpi),
                title_bar_dark: None,
            });
        });
        refresh(hwnd);
        ShowWindow(hwnd, SW_SHOWNORMAL);
        SetForegroundWindow(hwnd);
        notify_tray(HISTORY_EVENT_READY, hwnd);
        let mut message: MSG = zeroed();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    // View resources must go away before GDI+ shuts down.
    drop(STATE.with(|cell| cell.borrow_mut().take()));
}

fn scale_for_dpi(dpi: u32) -> f32 {
    dpi.max(48) as f32 / 96.0
}

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

fn key_from(virtual_key: usize) -> Option<Key> {
    Some(match virtual_key {
        VK_TAB => Key::Tab,
        VK_ESCAPE => Key::Escape,
        VK_LEFT => Key::Left,
        VK_RIGHT => Key::Right,
        VK_HOME => Key::Home,
        VK_END => Key::End,
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
            // SAFETY: this message supplies a pointer to MINMAXINFO.
            let info = &mut *(lparam as *mut MINMAXINFO);
            let scale = with_state(|state| state.scale)
                .unwrap_or_else(|| scale_for_dpi(GetDpiForWindow(hwnd)));
            let work = work_area(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST));
            let (x, y) = minimum_size(scale, work);
            info.ptMinTrackSize = POINT { x, y };
            0
        }
        WM_DPICHANGED => {
            let dpi = ((wparam >> 16) & 0xFFFF) as u32;
            with_state(|state| state.scale = scale_for_dpi(dpi));
            // SAFETY: this message supplies the suggested window rectangle.
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
        WM_LBUTTONDOWN => {
            let (x, y) = point_from(lparam);
            SetCapture(hwnd);
            with_state(|state| state.view.pointer_pressed(x, y));
            refresh(hwnd);
            0
        }
        WM_LBUTTONUP => {
            let (x, y) = point_from(lparam);
            with_state(|state| state.view.pointer_released(x, y));
            ReleaseCapture();
            refresh(hwnd);
            0
        }
        WM_CAPTURECHANGED => {
            with_state(|state| state.view.capture_lost());
            refresh(hwnd);
            0
        }
        WM_KEYDOWN => {
            if let Some(key) = key_from(wparam) {
                let shift = GetKeyState(VK_SHIFT) < 0;
                let command = with_state(|state| state.view.key(key, shift)).flatten();
                refresh(hwnd);
                if command == Some(Command::Close) {
                    DestroyWindow(hwnd);
                }
            }
            0
        }
        WM_SYSCHAR => {
            let handled = u32::try_from(wparam)
                .ok()
                .and_then(char::from_u32)
                .and_then(|character| with_state(|state| state.view.access_key(character)))
                == Some(true);
            if handled {
                refresh(hwnd);
                0
            } else {
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
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
        WM_SETTINGCHANGE | WM_SYSCOLORCHANGE | WM_THEMECHANGED => {
            let dark = system_uses_dark_theme();
            with_state(|state| state.view.set_system_dark(dark));
            refresh(hwnd);
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        WM_COPYDATA => LRESULT::from(receive_data(hwnd, wparam, lparam)),
        WM_DESTROY => {
            notify_tray(HISTORY_EVENT_CLOSED, hwnd);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

fn notify_tray(event: usize, window: HWND) {
    let Some((tray, provider)) = with_state(|state| (state.tray, state.provider)) else {
        return;
    };
    if !tray.is_null() {
        // SAFETY: posting to a window handle is safe even if it has closed.
        unsafe {
            PostMessageW(
                tray,
                WM_HISTORY_EVENT,
                history_event(event, provider.code()),
                window as LPARAM,
            );
        }
    }
}

unsafe fn receive_data(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) -> bool {
    let packet = lparam as *const COPYDATASTRUCT;
    if packet.is_null() {
        return false;
    }
    let packet = &*packet;
    if packet.dwData != HISTORY_DATA
        || packet.lpData.is_null()
        || packet.cbData > MAX_HISTORY_DATA_BYTES
    {
        return false;
    }
    let Some(tray) = with_state(|state| state.tray) else {
        return false;
    };
    if tray.is_null() || wparam as HWND != tray {
        return false;
    }
    // SAFETY: WM_COPYDATA guarantees this many readable bytes for the call.
    let bytes = std::slice::from_raw_parts(packet.lpData as *const u8, packet.cbData as usize);
    let Some(data) = ChartData::from_json(bytes) else {
        return false;
    };
    // Parsing copied the payload. Release the sender before updating the UI.
    ReplyMessage(1);
    if with_state(|state| state.view.set_data(data)) == Some(true) {
        refresh(hwnd);
    }
    true
}

unsafe fn refresh(hwnd: HWND) {
    let mut client: RECT = zeroed();
    GetClientRect(hwnd, &mut client);
    let dc = GetDC(hwnd);
    if !dc.is_null() {
        if let Some(g) = Graphics::from_hdc(dc) {
            with_state(|state| {
                state.view.render(
                    &g,
                    state.scale,
                    client.right as f32,
                    client.bottom as f32,
                    false,
                )
            });
        }
        ReleaseDC(hwnd, dc);
    }
    let title_bar = with_state(|state| {
        let dark = state.view.dark();
        let changed = state.title_bar_dark != Some(dark);
        state.title_bar_dark = Some(dark);
        changed.then_some(dark)
    })
    .flatten();
    if let Some(dark) = title_bar {
        apply_title_bar(hwnd, dark);
    }
    InvalidateRect(hwnd, null(), 0);
}

fn colorref(argb: u32) -> COLORREF {
    ((argb & 0xFF) << 16) | (argb & 0xFF00) | ((argb >> 16) & 0xFF)
}

unsafe fn apply_title_bar(hwnd: HWND, dark: bool) {
    let set = |attribute: u32, value: u32| {
        // SAFETY: value outlives the call and its size is supplied.
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
    if !target.is_null() && width > 0 && height > 0 {
        let buffer = CreateCompatibleDC(target);
        if !buffer.is_null() {
            let bitmap = CreateCompatibleBitmap(target, width, height);
            if !bitmap.is_null() {
                let previous = SelectObject(buffer, bitmap);
                if let Some(g) = Graphics::from_hdc(buffer) {
                    with_state(|state| {
                        state
                            .view
                            .render(&g, state.scale, width as f32, height as f32, true)
                    });
                }
                BitBlt(target, 0, 0, width, height, buffer, 0, 0, SRCCOPY);
                SelectObject(buffer, previous);
                DeleteObject(bitmap);
            }
            DeleteDC(buffer);
        }
    }
    EndPaint(hwnd, &paint);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_arguments_round_trip_for_each_provider() {
        for provider in [ChartProvider::Codex, ChartProvider::Claude] {
            let arguments = launch_arguments(123usize as HWND, provider);
            let arguments: Vec<String> = arguments[1..]
                .iter()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect();
            let launch = Launch::parse(&arguments).unwrap();
            assert_eq!(launch.tray as usize, 123);
            assert_eq!(launch.provider, provider);
        }
        assert!(Launch::parse(&[]).is_none());
        assert!(Launch::parse(&["no hwnd".into(), "codex".into()]).is_none());
        assert!(Launch::parse(&["1".into(), "other".into()]).is_none());
        assert!(Launch::parse(&["1".into(), "codex".into(), "extra".into()]).is_none());
    }

    #[test]
    fn placement_fits_the_monitor_and_respects_dpi() {
        let work = RECT {
            left: -1000,
            top: 100,
            right: 0,
            bottom: 800,
        };
        let bounds = centered(work, (1500, 500));
        assert_eq!(
            (bounds.left, bounds.top, bounds.right, bounds.bottom),
            (-1000, 200, 0, 700)
        );
        assert_eq!(minimum_size(1.5, Some(work)), (1000, 700));
        assert_eq!(scale_for_dpi(144), 1.5);
    }
}
