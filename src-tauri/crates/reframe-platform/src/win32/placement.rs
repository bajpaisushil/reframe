//! Moving, closing and focusing windows.
//!
//! Limitations (Windows design, not fixable from a normal process):
//! - Windows of applications running as administrator cannot be moved, closed or focused from a
//!   non-elevated Reframe (User Interface Privilege Isolation): such calls report
//!   `PermissionDenied`.
//! - Fullscreen is an application-controlled mode; [`set_window_placement`] maximizes instead.
//! - Placement is requested asynchronously (`WPF_ASYNCWINDOWPLACEMENT`, `SWP_ASYNCWINDOWPOS`,
//!   `ShowWindowAsync`) so a hung application cannot block Reframe; callers re-read
//!   [`crate::Platform::windows`] to confirm the result.

use std::mem::size_of;

use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{INPUT, INPUT_MOUSE, SendInput};
use windows::Win32::UI::WindowsAndMessaging::{
    IsHungAppWindow, IsIconic, IsWindow, PostMessageW, SHOW_WINDOW_CMD, SW_RESTORE,
    SW_SHOWMAXIMIZED, SW_SHOWMINNOACTIVE, SW_SHOWNORMAL, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE,
    SWP_NOZORDER, SetForegroundWindow, SetWindowPlacement, SetWindowPos, ShowWindowAsync,
    WINDOWPLACEMENT, WM_CLOSE, WPF_ASYNCWINDOWPLACEMENT, WPF_RESTORETOMAXIMIZED,
};

use super::ffi::{clear_last_error, failure_message, resolve_window, win32_code};
use super::logic::errors::{window_error, window_not_found};
use super::logic::filter::{WS_CHILD, WS_EX_TOOLWINDOW};
use super::logic::geometry::{
    edges, inflate, monitor_for_rect, placement_for_visible_bounds, sanitize,
};
use super::logic::placement::{FallbackStep, fallback_steps, placement_passes, should_fall_back};
use super::monitor::MonitorSnapshot;
use super::window::{
    InsetsCache, frame_bounds, normal_insets, state_and_monitor, styles, window_placement,
    window_rect,
};
use crate::{PlatformError, PlatformResult, Rect, WindowId, WindowPlacement, WindowState};

const MOVE: &str = "Moving a window";
const CLOSE: &str = "Closing a window";
const FOCUS: &str = "Focusing a window";

fn os_error(operation: &str, id: &WindowId, error: &windows::core::Error) -> PlatformError {
    window_error(
        operation,
        id.as_str(),
        win32_code(error),
        &failure_message(error),
    )
}

/// Apply `placement` to a window: `bounds` is the visible restored frame (as reported by
/// [`crate::Platform::windows`]); it is converted to the outer, workspace-relative
/// `rcNormalPosition` and applied together with the show state by `SetWindowPlacement`, so
/// maximized/minimized windows land on the right monitor. `Fullscreen` maximizes.
///
/// If `SetWindowPlacement` is rejected without a specific reason, the same result is reached with
/// `ShowWindowAsync` and `SetWindowPos` (see [`super::logic::placement`]).
pub fn set_window_placement(
    id: &WindowId,
    placement: &WindowPlacement,
    monitors: &MonitorSnapshot,
    cache: &mut InsetsCache,
) -> PlatformResult<()> {
    let requested = sanitize(placement.bounds);
    if requested.is_empty() {
        return Err(PlatformError::os(
            MOVE,
            format!(
                "the requested size {}x{} is empty",
                placement.bounds.width, placement.bounds.height
            ),
        ));
    }
    let window = resolve_window(id)?;
    let current = window_placement(window).map_err(|e| os_error(MOVE, id, &e))?;
    let (style, ex_style) = styles(window);
    if style & WS_CHILD != 0 {
        return Err(PlatformError::os(
            MOVE,
            format!("{id} is a child window, not a top-level window"),
        ));
    }
    let outer = window_rect(window).ok_or_else(|| window_not_found(id.as_str()))?;
    let frame = frame_bounds(window);
    let (state, current_monitor) = state_and_monitor(window, style, outer, monitors);
    let insets = normal_insets(window, state, style, outer, frame, cache);
    let geometries = monitors.geometries();
    let normal_position = placement_for_visible_bounds(
        requested,
        ex_style & WS_EX_TOOLWINDOW == 0,
        &geometries,
        insets,
    );
    let (left, top, right, bottom) = edges(normal_position);
    let show = match placement.state {
        WindowState::Normal => SW_SHOWNORMAL,
        WindowState::Maximized | WindowState::Fullscreen => SW_SHOWMAXIMIZED,
        WindowState::Minimized => SW_SHOWMINNOACTIVE,
    };
    let wp = WINDOWPLACEMENT {
        length: size_of::<WINDOWPLACEMENT>() as u32,
        flags: WPF_ASYNCWINDOWPLACEMENT,
        showCmd: show.0 as u32,
        ptMinPosition: current.ptMinPosition,
        ptMaxPosition: current.ptMaxPosition,
        rcNormalPosition: RECT {
            left,
            top,
            right,
            bottom,
        },
    };
    if let Err(error) = apply_placement(window, &wp) {
        if !should_fall_back(win32_code(&error)) {
            return Err(os_error(MOVE, id, &error));
        }
        log::debug!(
            "SetWindowPlacement rejected for {id} ({}); using SetWindowPos/ShowWindowAsync",
            failure_message(&error)
        );
        let steps = fallback_steps(state, placement.state);
        return run_fallback(window, inflate(requested, insets), &steps)
            .map_err(|e| os_error(MOVE, id, &e));
    }

    // Some transitions need the placement applied more than once (see `placement_passes`).
    let target_scale = monitor_for_rect(&geometries, requested)
        .and_then(|index| monitors.records.get(index))
        .map(|m| m.info.scale_factor);
    let current_scale = current_monitor.map(|m| m.info.scale_factor);
    let dpi_changes =
        target_scale.is_some() && current_scale.is_some() && target_scale != current_scale;
    let restores_maximized = current.flags.0 & WPF_RESTORETOMAXIMIZED.0 != 0;
    for _ in 1..placement_passes(state, restores_maximized, placement.state, dpi_changes) {
        apply_placement(window, &wp).map_err(|e| os_error(MOVE, id, &e))?;
    }
    Ok(())
}

fn apply_placement(window: HWND, wp: &WINDOWPLACEMENT) -> windows::core::Result<()> {
    clear_last_error();
    // SAFETY: fully initialized WINDOWPLACEMENT with its length set.
    unsafe { SetWindowPlacement(window, wp) }
}

fn show_async(window: HWND, command: SHOW_WINDOW_CMD) -> windows::core::Result<()> {
    clear_last_error();
    // SAFETY: posts the request to the window's thread; never blocks on it.
    if unsafe { ShowWindowAsync(window, command) }.as_bool() {
        Ok(())
    } else {
        Err(windows::core::Error::from_thread())
    }
}

/// Execute the fallback steps; `outer` is the outer restored frame in screen coordinates.
fn run_fallback(window: HWND, outer: Rect, steps: &[FallbackStep]) -> windows::core::Result<()> {
    for step in steps {
        match step {
            FallbackStep::Restore => show_async(window, SW_RESTORE)?,
            FallbackStep::Move => {
                clear_last_error();
                // SAFETY: plain asynchronous request on a valid handle.
                unsafe {
                    SetWindowPos(
                        window,
                        None,
                        outer.x,
                        outer.y,
                        outer.width,
                        outer.height,
                        SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
                    )
                }?;
            }
            FallbackStep::Maximize => show_async(window, SW_SHOWMAXIMIZED)?,
            FallbackStep::Minimize => show_async(window, SW_SHOWMINNOACTIVE)?,
        }
    }
    Ok(())
}

/// Politely ask a window to close (`WM_CLOSE`). The application may prompt to save work or
/// refuse; nothing is ever terminated.
pub fn close_window(id: &WindowId) -> PlatformResult<()> {
    let window = resolve_window(id)?;
    clear_last_error();
    // SAFETY: posting a message never blocks on the target thread.
    unsafe { PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)) }
        .map_err(|e| os_error(CLOSE, id, &e))
}

/// Restore a minimized window and bring it to the foreground.
///
/// Windows only lets the process that received the last input event change the foreground
/// window ("focus-stealing prevention"). If the first attempt is refused, Reframe sends an empty
/// mouse input event (no movement, no buttons, so nothing visible happens) to become that
/// process and retries once, the same technique PowerToys uses. If Windows still refuses, it
/// flashes the window's taskbar button and an error explains that.
pub fn focus_window(id: &WindowId) -> PlatformResult<()> {
    let window = resolve_window(id)?;
    // SAFETY: plain query.
    if unsafe { IsHungAppWindow(window) }.as_bool() {
        return Err(PlatformError::os(
            FOCUS,
            "the application is not responding",
        ));
    }
    // SAFETY: plain query.
    if unsafe { IsIconic(window) }.as_bool() {
        // Asynchronous, so an application that is busy (but not yet considered hung) cannot
        // block Reframe; the window thread restores it before handling the activation.
        show_async(window, SW_RESTORE).map_err(|e| os_error(FOCUS, id, &e))?;
    }
    if try_set_foreground(window) {
        return Ok(());
    }
    let input = INPUT {
        r#type: INPUT_MOUSE,
        ..Default::default()
    };
    // SAFETY: one fully initialized INPUT with its size.
    unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
    if try_set_foreground(window) {
        return Ok(());
    }
    // SAFETY: plain query.
    if !unsafe { IsWindow(Some(window)) }.as_bool() {
        return Err(window_not_found(id.as_str()));
    }
    Err(PlatformError::os(
        FOCUS,
        "Windows did not allow Reframe to bring the window to the foreground (focus-stealing \
         prevention); its taskbar button flashes instead",
    ))
}

fn try_set_foreground(window: HWND) -> bool {
    // SAFETY: plain call on a valid handle.
    unsafe { SetForegroundWindow(window) }.as_bool()
}
