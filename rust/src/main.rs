//! Rust prototype of Codex Usage Tray.
//!
//! One executable runs the long-lived tray and the short-lived popup,
//! Settings, and provider-specific history window processes. The tray owns
//! usage state and starts each window only when needed.

#![windows_subsystem = "windows"]

mod chart_data;
mod chart_view;
mod chart_window;
mod claude;
mod codex;
mod controls;
mod forecast;
mod gfx;
mod history;
mod history_file;
mod history_view;
mod http;
mod importer;
mod ipc;
mod localtime;
mod model;
mod paths;
mod pattern;
mod popup;
mod render;
mod resets;
mod settings;
mod settings_view;
mod settings_window;
mod startup;
mod timefmt;
mod tray;
mod update;
mod version;
mod view;
mod win;

use std::ptr::{null, null_mut};

use windows_sys::w;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONWARNING, MB_OK};

/// Asks a running tray process to exit instead of starting one.
const EXIT_ARGUMENT: &str = "--exit";

fn main() {
    // SAFETY: the call takes a constant and has no other preconditions.
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let arguments: Vec<String> = std::env::args().skip(1).collect();
    // Claude Code runs this as its status-line command on every response, so
    // it is handled before anything that creates windows or takes a lock.
    if arguments
        .iter()
        .any(|argument| argument.eq_ignore_ascii_case(claude::STATUS_LINE_ARGUMENT))
    {
        claude::handle_status_line();
        return;
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == ipc::POPUP_ARGUMENT)
    {
        if let Some(launch) = popup::Launch::parse(&arguments[1..]) {
            popup::run(launch);
        }
        return;
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == ipc::SETTINGS_ARGUMENT)
    {
        if let Some(launch) = settings_window::Launch::parse(&arguments[1..]) {
            settings_window::run(launch);
        }
        return;
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == ipc::HISTORY_ARGUMENT)
    {
        if let Some(launch) = chart_window::Launch::parse(&arguments[1..]) {
            chart_window::run(launch);
        }
        return;
    }
    if arguments
        .iter()
        .any(|argument| argument.eq_ignore_ascii_case(EXIT_ARGUMENT))
    {
        tray::request_exit();
        return;
    }

    let startup = std::env::current_exe().map_or(update::Startup::Normal, |executable| {
        update::startup(&arguments, &executable)
    });
    // The lock is distinct from the C# app's so both can run side by side.
    // SAFETY: the mutex name is a static string.
    let mutex = unsafe { CreateMutexW(null(), 1, w!("UsageTrayRs.SingleInstance")) };
    // SAFETY: reads the error the call above left on this thread.
    if mutex.is_null() || unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        // Someone started the new version by hand during an update's
        // hand-over. It is running, so the update still counts as installed.
        if let update::Startup::Updated(confirmation) = startup {
            confirmation.confirm();
        }
        return;
    }

    if let Some(staged) = tray::run(startup) {
        // The version started next has to be able to take the lock.
        // SAFETY: the handle came from CreateMutexW and is closed once.
        unsafe {
            CloseHandle(mutex);
        }
        let outcome = update::apply_and_restart(staged, &update::SystemHost);
        if matches!(outcome, update::ApplyOutcome::RecoveryFailed(_)) {
            // No copy of the app is left running to report this.
            let title = win::wide(update::NOTIFICATION_TITLE);
            let text = win::wide(update::RECOVERY_FAILED_NOTIFICATION);
            // SAFETY: both strings are null-terminated and outlive the call.
            unsafe {
                MessageBoxW(
                    null_mut(),
                    text.as_ptr(),
                    title.as_ptr(),
                    MB_OK | MB_ICONWARNING,
                );
            }
        }
    }
}
