//! The popup process. It is started by the tray process when the tray icon
//! is clicked, draws the view the tray sends it, and exits as soon as it is
//! dismissed, so its window, fonts and drawing surface cost no memory while
//! the popup is closed.

use std::cell::RefCell;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::time::Instant;

use windows_sys::w;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
    EndPaint, GetDC, GetMonitorInfoW, InvalidateRect, MonitorFromRect, ReleaseDC, SelectObject,
    MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT, SRCCOPY,
};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetCursorPos,
    GetMessageW, GetWindowRect, IsWindow, KillTimer, LoadCursorW, PostMessageW, PostQuitMessage,
    RegisterClassExW, ReplyMessage, SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow,
    TranslateMessage, HWND_TOPMOST, IDC_ARROW, MSG, SWP_NOACTIVATE, SW_SHOW, WM_ACTIVATE,
    WM_COPYDATA, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT,
    WM_TIMER, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::gfx::{GdiPlus, Graphics, Rect};
use crate::ipc::{
    EVENT_CLOSED, EVENT_OPEN_HISTORY, EVENT_OPEN_SETTINGS, EVENT_READY, EVENT_REFRESH,
    EVENT_SWITCH_PROVIDER, MAX_VIEW_BYTES, VIEW_DATA, WM_POPUP_EVENT,
};
use crate::render::{self, Hit, Theme};
use crate::view::PopupView;

const WM_MOUSELEAVE: u32 = 0x02A3;
const WA_INACTIVE: usize = 0;
const VK_LBUTTON: i32 = 0x01;
const VK_RBUTTON: i32 = 0x02;
const VK_ESCAPE: usize = 0x1B;
const VK_F5: usize = 0x74;
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWA_BORDER_COLOR: u32 = 34;
const DWMWCP_ROUND: u32 = 2;

const TIMER_STARTUP: usize = 1;
const TIMER_DISMISS: usize = 2;
const TIMER_SPIN: usize = 3;
/// Repaint interval and rotation period of the loading circle.
const SPIN_FRAME_MILLISECONDS: u32 = 16;
const SPIN_SECONDS: f32 = 0.9;
/// How long to wait for the tray's first view before giving up.
const STARTUP_TIMEOUT_MILLISECONDS: u32 = 3_000;
const DISMISS_POLL_MILLISECONDS: u32 = 100;
const POPUP_WIDTH: f32 = 520.0;
const POPUP_MARGIN: f32 = 8.0;
const FALLBACK_POPUP_HEIGHT: f32 = 200.0;

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Where the popup was asked to appear and whom it reports to.
pub struct Launch {
    tray: HWND,
    anchor: RECT,
}

impl Launch {
    /// Parses `<tray hwnd> <left> <top> <right> <bottom>`.
    pub fn parse(arguments: &[String]) -> Option<Self> {
        let [tray, left, top, right, bottom] = arguments else {
            return None;
        };
        let edge = |text: &String| text.parse::<i32>().ok();
        Some(Self {
            tray: tray.parse::<usize>().ok()? as HWND,
            anchor: RECT {
                left: edge(left)?,
                top: edge(top)?,
                right: edge(right)?,
                bottom: edge(bottom)?,
            },
        })
    }
}

struct State {
    tray: HWND,
    anchor: RECT,
    view: Option<PopupView>,
    scale: f32,
    shown: bool,
    closing: bool,
    /// When the loading circle started turning, while a refresh is running.
    spin_started: Option<Instant>,
    hover: Option<Hit>,
    hits: Vec<(Hit, Rect)>,
}

/// Runs `action` with the popup state, or returns `None` if the state is
/// already borrowed further up the stack.
fn with_state<R>(action: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|cell| {
        cell.try_borrow_mut()
            .ok()
            .and_then(|mut state| state.as_mut().map(action))
    })
}

/// Creates the hidden popup window, asks the tray for a view, and runs until
/// the popup is dismissed.
pub fn run(launch: Launch) {
    let Some(_gdiplus) = GdiPlus::start() else {
        return;
    };
    // SAFETY: standard window-class registration and message loop; every
    // pointer passed refers to a live local or a static string.
    unsafe {
        let instance = GetModuleHandleW(null());
        let class_name = w!("UsageTrayRs.Popup");
        let mut class: WNDCLASSEXW = zeroed();
        class.cbSize = size_of::<WNDCLASSEXW>() as u32;
        class.lpfnWndProc = Some(window_procedure);
        class.hInstance = instance;
        class.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
        class.lpszClassName = class_name;
        if RegisterClassExW(&class) == 0 {
            return;
        }
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class_name,
            w!("Usage"),
            WS_POPUP,
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
            return;
        }
        let corners = DWMWCP_ROUND;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&corners as *const u32).cast(),
            size_of::<u32>() as u32,
        );

        let tray = launch.tray;
        STATE.with(|cell| {
            *cell.borrow_mut() = Some(State {
                tray,
                anchor: launch.anchor,
                view: None,
                scale: 1.0,
                shown: false,
                closing: false,
                spin_started: None,
                hover: None,
                hits: Vec::new(),
            });
        });
        SetTimer(hwnd, TIMER_STARTUP, STARTUP_TIMEOUT_MILLISECONDS, None);
        if PostMessageW(tray, WM_POPUP_EVENT, EVENT_READY, hwnd as LPARAM) == 0 {
            return;
        }

        let mut message: MSG = zeroed();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

unsafe extern "system" fn window_procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_COPYDATA => LRESULT::from(receive_view(hwnd, lparam)),
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_MOUSEMOVE => {
            update_hover(hwnd, Some(point_from(lparam)));
            0
        }
        WM_MOUSELEAVE => {
            update_hover(hwnd, None);
            0
        }
        WM_LBUTTONUP => {
            let (x, y) = point_from(lparam);
            let hit = with_state(|state| {
                state
                    .hits
                    .iter()
                    .find(|(_, area)| area.contains(x, y))
                    .map(|(hit, _)| *hit)
            });
            match hit.flatten() {
                Some(Hit::Refresh) => notify_tray(EVENT_REFRESH),
                Some(Hit::Provider) => notify_tray(EVENT_SWITCH_PROVIDER),
                Some(Hit::History) => notify_tray(EVENT_OPEN_HISTORY),
                Some(Hit::Settings) => notify_tray(EVENT_OPEN_SETTINGS),
                // Days only respond to hovering.
                Some(Hit::Day(_)) | None => {}
            }
            0
        }
        WM_ACTIVATE if wparam & 0xFFFF == WA_INACTIVE => {
            if with_state(|state| state.shown) == Some(true) {
                close(hwnd);
            }
            0
        }
        WM_KEYDOWN => {
            match wparam {
                VK_ESCAPE => close(hwnd),
                VK_F5 => notify_tray(EVENT_REFRESH),
                _ => {}
            }
            0
        }
        WM_TIMER => {
            let shown = with_state(|state| state.shown) == Some(true);
            match wparam {
                TIMER_STARTUP if !shown => close(hwnd),
                TIMER_STARTUP => {
                    KillTimer(hwnd, TIMER_STARTUP);
                }
                TIMER_DISMISS if should_dismiss(hwnd) => close(hwnd),
                TIMER_SPIN => {
                    InvalidateRect(hwnd, null(), 0);
                }
                _ => {}
            }
            0
        }
        WM_DESTROY => {
            notify_tray(EVENT_CLOSED);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

fn point_from(lparam: LPARAM) -> (f32, f32) {
    let x = (lparam & 0xFFFF) as i16;
    let y = ((lparam >> 16) & 0xFFFF) as i16;
    (f32::from(x), f32::from(y))
}

/// Destroys the popup once. Destruction deactivates the window, which would
/// otherwise ask to close it again.
fn close(hwnd: HWND) {
    let first = with_state(|state| !std::mem::replace(&mut state.closing, true));
    if first == Some(true) {
        // SAFETY: hwnd is this thread's popup window.
        unsafe {
            DestroyWindow(hwnd);
        }
    }
}

fn notify_tray(event: usize) {
    if let Some(tray) = with_state(|state| state.tray) {
        // SAFETY: posting to a window handle is safe even if the window is gone.
        unsafe {
            PostMessageW(tray, WM_POPUP_EVENT, event, 0);
        }
    }
}

/// Accepts a view from the tray, then sizes, shows and repaints the popup.
unsafe fn receive_view(hwnd: HWND, lparam: LPARAM) -> bool {
    let data = lparam as *const COPYDATASTRUCT;
    if data.is_null() {
        return false;
    }
    let data = &*data;
    if data.dwData != VIEW_DATA || data.lpData.is_null() || data.cbData > MAX_VIEW_BYTES {
        return false;
    }
    // SAFETY: for WM_COPYDATA the system guarantees lpData points to cbData readable bytes.
    let bytes = std::slice::from_raw_parts(data.lpData as *const u8, data.cbData as usize);
    let Some(view) = PopupView::from_json(bytes) else {
        return false;
    };
    // The data is copied; let the tray continue before doing any window work.
    ReplyMessage(1);

    let refreshing = view.refreshing;
    let first = with_state(|state| {
        state.view = Some(view);
        // Keep an already turning circle in phase when another view arrives.
        state.spin_started = match (refreshing, state.spin_started) {
            (true, started) => started.or_else(|| Some(Instant::now())),
            (false, _) => None,
        };
        !std::mem::replace(&mut state.shown, true)
    });
    if refreshing {
        SetTimer(hwnd, TIMER_SPIN, SPIN_FRAME_MILLISECONDS, None);
    } else {
        KillTimer(hwnd, TIMER_SPIN);
    }
    place(hwnd);
    if first == Some(true) {
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        SetTimer(hwnd, TIMER_DISMISS, DISMISS_POLL_MILLISECONDS, None);
    }
    InvalidateRect(hwnd, null(), 0);
    true
}

/// True once the popup should close without having lost activation: the user
/// pressed a mouse button outside it, or the tray process has gone away.
unsafe fn should_dismiss(hwnd: HWND) -> bool {
    let tray = with_state(|state| state.tray).unwrap_or(null_mut());
    if IsWindow(tray) == 0 {
        return true;
    }
    let pressed = GetAsyncKeyState(VK_LBUTTON) < 0 || GetAsyncKeyState(VK_RBUTTON) < 0;
    if !pressed {
        return false;
    }
    let mut cursor = POINT { x: 0, y: 0 };
    let mut bounds: RECT = zeroed();
    if GetCursorPos(&mut cursor) == 0 || GetWindowRect(hwnd, &mut bounds) == 0 {
        return false;
    }
    let inside = cursor.x >= bounds.left
        && cursor.x < bounds.right
        && cursor.y >= bounds.top
        && cursor.y < bounds.bottom;
    !inside
}

/// Sizes the popup to its content and anchors it beside the tray icon.
unsafe fn place(hwnd: HWND) {
    let Some((anchor, Some(view))) = with_state(|state| (state.anchor, state.view.clone())) else {
        return;
    };
    let monitor = MonitorFromRect(&anchor, MONITOR_DEFAULTTONEAREST);
    let mut info: MONITORINFO = zeroed();
    info.cbSize = size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(monitor, &mut info) == 0 {
        return;
    }
    let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
    GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
    let scale = dpi_x.max(48) as f32 / 96.0;

    let work = info.rcWork;
    let margin = (POPUP_MARGIN * scale).round() as i32;
    let width = ((POPUP_WIDTH * scale).round() as i32).min(work.right - work.left - 2 * margin);
    let screen = GetDC(null_mut());
    let measured = Graphics::from_hdc(screen)
        .map(|g| render::render(&g, &view, scale, width as f32, None, 0.0, false).height);
    ReleaseDC(null_mut(), screen);
    let height = (measured.unwrap_or(FALLBACK_POPUP_HEIGHT * scale).ceil() as i32)
        .min(work.bottom - work.top - 2 * margin);

    let centered = (anchor.left + anchor.right) / 2 - width / 2;
    let x = centered
        .min(work.right - width - margin)
        .max(work.left + margin);
    // Prefer sitting above the icon; fall back to below it for a top taskbar.
    let above = anchor.top.min(work.bottom) - height - margin;
    let y = if above >= work.top + margin {
        above
    } else {
        (anchor.bottom.max(work.top) + margin).min(work.bottom - height - margin)
    };

    with_state(|state| state.scale = scale);
    let border = colorref(Theme::new(view.dark).border);
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_BORDER_COLOR,
        (&border as *const u32).cast(),
        size_of::<u32>() as u32,
    );
    SetWindowPos(hwnd, HWND_TOPMOST, x, y, width, height, SWP_NOACTIVATE);
}

/// Converts 0xAARRGGBB to the 0x00BBGGRR layout used by GDI and DWM.
fn colorref(argb: u32) -> u32 {
    ((argb & 0xFF) << 16) | (argb & 0xFF00) | ((argb >> 16) & 0xFF)
}

unsafe fn paint(hwnd: HWND) {
    let mut paint: PAINTSTRUCT = zeroed();
    let target = BeginPaint(hwnd, &mut paint);
    let mut client: RECT = zeroed();
    GetClientRect(hwnd, &mut client);
    let (width, height) = (client.right, client.bottom);
    if width > 0 && height > 0 {
        // Draw to an off-screen bitmap first so the popup never flickers.
        let buffer = CreateCompatibleDC(target);
        let bitmap = CreateCompatibleBitmap(target, width, height);
        let previous = SelectObject(buffer, bitmap);
        if let Some(g) = Graphics::from_hdc(buffer) {
            with_state(|state| {
                if let Some(view) = &state.view {
                    let turns = state.spin_started.map_or(0.0, |started| {
                        started.elapsed().as_secs_f32() / SPIN_SECONDS
                    });
                    let layout = render::render(
                        &g,
                        view,
                        state.scale,
                        width as f32,
                        state.hover,
                        turns.fract() * 360.0,
                        true,
                    );
                    state.hits = layout.hits;
                }
            });
        }
        BitBlt(target, 0, 0, width, height, buffer, 0, 0, SRCCOPY);
        SelectObject(buffer, previous);
        DeleteObject(bitmap);
        DeleteDC(buffer);
    }
    EndPaint(hwnd, &paint);
}

fn update_hover(hwnd: HWND, point: Option<(f32, f32)>) {
    let changed = with_state(|state| {
        let hover = point.and_then(|(x, y)| {
            state
                .hits
                .iter()
                .find(|(_, area)| area.contains(x, y))
                .map(|(hit, _)| *hit)
        });
        let changed = hover != state.hover;
        state.hover = hover;
        changed
    });
    // SAFETY: hwnd is the popup window currently handling a mouse message.
    unsafe {
        if point.is_some() {
            let mut track = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            TrackMouseEvent(&mut track);
        }
        if changed == Some(true) {
            InvalidateRect(hwnd, null(), 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_arguments_carry_the_tray_window_and_icon_position() {
        let arguments: Vec<String> = ["1312", "1800", "1040", "1824", "1080"]
            .map(String::from)
            .to_vec();
        let launch = Launch::parse(&arguments).unwrap();
        assert_eq!(launch.tray as usize, 1312);
        assert_eq!(
            (
                launch.anchor.left,
                launch.anchor.top,
                launch.anchor.right,
                launch.anchor.bottom
            ),
            (1800, 1040, 1824, 1080)
        );
        assert!(Launch::parse(&arguments[..4]).is_none());
        assert!(Launch::parse(&["x", "1", "2", "3", "4"].map(String::from)).is_none());
    }

    #[test]
    fn colors_convert_to_colorref_byte_order() {
        assert_eq!(colorref(0xFF48_515B), 0x005B_5148);
    }
}
