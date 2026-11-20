//! # reframe-platform
//!
//! Platform abstraction layer. Everything that touches the operating system's window manager,
//! display configuration or process table goes through the [`Platform`] trait so the workspace
//! engine (`reframe-core`) stays OS-agnostic and testable.
//!
//! | OS      | Module    | Mechanism |
//! |---------|-----------|-----------|
//! | Windows | [`win32`] | Win32 window enumeration/placement, DWM, monitor APIs |
//! | macOS   | [`macos`] | AppKit (NSWorkspace), CoreGraphics window list, Accessibility (AX) |
//! | Linux   | [`linux`] | X11 + EWMH via `x11rb`; Wayland sessions report reduced capabilities |
//!
//! Rules for implementations:
//! - Operations that cannot be performed return [`PlatformError::Unsupported`] (or
//!   `PermissionDenied`) with a human-readable reason. Never panic, never fake success.
//! - [`Platform::close_window`] is always a *graceful* request (WM_CLOSE, `_NET_CLOSE_WINDOW`,
//!   AX close button). Nothing in Reframe force-kills processes.

pub mod common;
mod error;
mod types;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(any(target_os = "macos", test))] // pure macOS logic, unit-tested on every OS
#[path = "macos/logic.rs"]
mod macos_logic;
#[cfg(target_os = "windows")]
pub mod win32;

#[cfg(any(test, feature = "mock"))]
pub mod mock;

use std::sync::Arc;

pub use error::*;
pub use types::*;

/// The operating-system integration surface used by the workspace engine.
///
/// Implementations must be cheap to call repeatedly (the restore engine polls
/// [`Platform::windows`] while waiting for launched applications) and thread-safe.
pub trait Platform: Send + Sync {
    /// OS, display server and capability report.
    fn info(&self) -> PlatformInfo;

    /// Connected monitors.
    fn monitors(&self) -> PlatformResult<Vec<MonitorInfo>>;

    /// Visible top-level application windows, front-to-back (see [`WindowInfo`] docs for the
    /// filtering contract).
    fn windows(&self) -> PlatformResult<Vec<WindowInfo>>;

    /// All processes the current user can see.
    fn processes(&self) -> PlatformResult<Vec<ProcessInfo>>;

    /// Start an application detached from Reframe (it must keep running if Reframe exits).
    fn launch(&self, request: &LaunchRequest) -> PlatformResult<LaunchedProcess>;

    /// Move/resize a window and apply its state (normal/maximized/minimized/fullscreen).
    fn set_window_placement(
        &self,
        window: &WindowId,
        placement: &WindowPlacement,
    ) -> PlatformResult<()>;

    /// Politely ask a window to close. The application may show a "save changes?" prompt or
    /// refuse; callers must re-check [`Platform::windows`] to see whether it actually closed.
    fn close_window(&self, window: &WindowId) -> PlatformResult<()>;

    /// Bring a window to the foreground (best effort; some OSes restrict focus stealing).
    fn focus_window(&self, window: &WindowId) -> PlatformResult<()>;

    /// PNG-encoded application icon, if one can be found.
    fn application_icon(&self, request: &IconRequest) -> PlatformResult<Option<Vec<u8>>>;

    /// Find launchable targets for an installed application (PATH, registry App Paths,
    /// `/Applications`, `.desktop` files...). Most specific/likely match first.
    fn locate_application(&self, query: &LocateQuery) -> Vec<LaunchTarget>;

    /// Permissions relevant on this platform (empty where none are needed).
    fn permissions(&self) -> Vec<PermissionStatus>;

    /// Trigger the OS permission prompt or open the relevant settings page.
    fn request_permission(&self, kind: PermissionKind) -> PlatformResult<()>;
}

/// The platform implementation for the OS Reframe was compiled for.
pub fn native() -> Arc<dyn Platform> {
    #[cfg(target_os = "linux")]
    {
        Arc::new(linux::LinuxPlatform::new())
    }
    #[cfg(target_os = "windows")]
    {
        Arc::new(win32::WindowsPlatform::new())
    }
    #[cfg(target_os = "macos")]
    {
        Arc::new(macos::MacPlatform::new())
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        Arc::new(common::UnsupportedPlatform::new())
    }
}
