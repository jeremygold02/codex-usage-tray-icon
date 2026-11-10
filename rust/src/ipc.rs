//! Messages exchanged between the tray process and the on-demand popup process.
//!
//! The tray starts `<exe> --popup <tray hwnd> <left> <top> <right> <bottom>`,
//! where the rectangle is the tray icon's screen position. The popup posts
//! `WM_POPUP_EVENT` to the tray window; the tray answers `EVENT_READY`, and
//! every later state change, with a `WM_COPYDATA` carrying a JSON `PopupView`.

use windows_sys::Win32::UI::WindowsAndMessaging::WM_APP;

pub const POPUP_ARGUMENT: &str = "--popup";

/// Posted by the popup to the tray window. `wparam` is one of the `EVENT_*`
/// values; for `EVENT_READY`, `lparam` is the popup's window handle.
pub const WM_POPUP_EVENT: u32 = WM_APP + 3;
pub const EVENT_READY: usize = 1;
pub const EVENT_REFRESH: usize = 2;
pub const EVENT_SWITCH_PROVIDER: usize = 3;
pub const EVENT_CLOSED: usize = 4;
pub const EVENT_OPEN_HISTORY: usize = 5;
pub const EVENT_OPEN_SETTINGS: usize = 6;

/// `COPYDATASTRUCT::dwData` tag identifying a `PopupView` payload.
pub const VIEW_DATA: usize = 0x5554_5256;
pub const MAX_VIEW_BYTES: u32 = 65_536;

// The Settings window is a third kind of process, started by the tray as
// `<exe> --settings <tray hwnd> <settings path>`. It reads and writes the
// settings file itself and posts `WM_SETTINGS_EVENT` to the tray window; the
// tray answers `SETTINGS_EVENT_READY`, and any later change of what it
// reports, with a `WM_COPYDATA` carrying a JSON `settings_view::Status`.

/// The first command-line argument of the Settings window process.
pub const SETTINGS_ARGUMENT: &str = "--settings";

/// Posted by the Settings window to the tray window. `wparam` is one of the
/// `SETTINGS_EVENT_*` values.
pub const WM_SETTINGS_EVENT: u32 = WM_APP + 0x20;
/// The window exists; `lparam` is its window handle.
pub const SETTINGS_EVENT_READY: usize = 1;
/// The settings file was rewritten; the tray should load it again.
pub const SETTINGS_EVENT_SAVED: usize = 2;
/// The user asked for an update check.
pub const SETTINGS_EVENT_CHECK_UPDATES: usize = 3;
/// The window was closed.
pub const SETTINGS_EVENT_CLOSED: usize = 4;
/// The window started or stopped having unsaved edits; `lparam` is 1 while
/// it has some. The tray uses it to decide whether a restart may interrupt.
pub const SETTINGS_EVENT_DIRTY: usize = 5;

/// `COPYDATASTRUCT::dwData` tag identifying a `settings_view::Status` payload.
pub const SETTINGS_STATUS_DATA: usize = 0x5554_5353;
/// The largest status payload the Settings window accepts.
pub const MAX_SETTINGS_STATUS_BYTES: u32 = 16_384;

// A usage history chart is a fourth kind of process, started by the tray as
// `<exe> --history <tray hwnd> <provider>`, at most one per provider. It never
// opens the history files: it posts `WM_HISTORY_EVENT` to the tray window, and
// the tray answers `HISTORY_EVENT_READY`, and every later reading or settings
// change, with a `WM_COPYDATA` carrying a JSON `chart_data::ChartData`.

/// The first command-line argument of a history chart process.
pub const HISTORY_ARGUMENT: &str = "--history";

/// Posted by a history chart to the tray window. `wparam` combines one of the
/// `HISTORY_EVENT_*` values with the chart's provider (see
/// [`history_event`]); `lparam` is the chart's window handle.
pub const WM_HISTORY_EVENT: u32 = WM_APP + 0x30;
/// The window exists and wants its data.
pub const HISTORY_EVENT_READY: usize = 1;
/// The window was closed.
pub const HISTORY_EVENT_CLOSED: usize = 2;

/// The provider code of the Codex chart in a `WM_HISTORY_EVENT`.
pub const HISTORY_PROVIDER_CODEX: usize = 0;
/// The provider code of the Claude chart in a `WM_HISTORY_EVENT`.
pub const HISTORY_PROVIDER_CLAUDE: usize = 1;

const HISTORY_EVENT_MASK: usize = 0xFF;
const HISTORY_PROVIDER_SHIFT: u32 = 8;

/// The `wparam` of a `WM_HISTORY_EVENT` for `event` from the chart of the
/// provider with code `provider`.
pub const fn history_event(event: usize, provider: usize) -> usize {
    (event & HISTORY_EVENT_MASK) | (provider << HISTORY_PROVIDER_SHIFT)
}

/// Splits the `wparam` of a `WM_HISTORY_EVENT` into its event and provider code.
pub const fn split_history_event(wparam: usize) -> (usize, usize) {
    (
        wparam & HISTORY_EVENT_MASK,
        wparam >> HISTORY_PROVIDER_SHIFT,
    )
}

/// `COPYDATASTRUCT::dwData` tag identifying a `chart_data::ChartData` payload.
pub const HISTORY_DATA: usize = 0x5554_4844;
/// The largest chart payload a history chart accepts. `chart_data` caps what
/// it sends so that even a history filled to its sample limit with readings
/// that all differ stays below this (see `chart_data::MAX_POINTS`).
pub const MAX_HISTORY_DATA_BYTES: u32 = 6 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_events_carry_their_provider() {
        for provider in [HISTORY_PROVIDER_CODEX, HISTORY_PROVIDER_CLAUDE] {
            for event in [HISTORY_EVENT_READY, HISTORY_EVENT_CLOSED] {
                assert_eq!(
                    split_history_event(history_event(event, provider)),
                    (event, provider)
                );
            }
        }
        assert_eq!(history_event(HISTORY_EVENT_CLOSED, 1), 0x102);
    }

    #[test]
    fn cross_process_messages_and_payload_tags_are_distinct() {
        // The tray keeps `WM_APP + 1`, `+ 2` and `+ 20..=24` for itself.
        let private = [1, 2, 20, 21, 22, 23, 24].map(|offset| WM_APP + offset);
        let shared = [WM_POPUP_EVENT, WM_SETTINGS_EVENT, WM_HISTORY_EVENT];
        for (index, message) in shared.iter().enumerate() {
            assert!(!private.contains(message));
            assert!(!shared[index + 1..].contains(message));
        }
        let tags = [VIEW_DATA, SETTINGS_STATUS_DATA, HISTORY_DATA];
        for (index, tag) in tags.iter().enumerate() {
            assert!(!tags[index + 1..].contains(tag));
        }
    }
}
