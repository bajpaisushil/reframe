//! Windows implementation: Win32 windows, DWM, monitors, packaged apps.
//!
//! Coordinates are physical pixels in the virtual-screen space (origin at the primary monitor's
//! top-left corner; other monitors may have negative coordinates). Every call that reads or
//! writes geometry runs with per-monitor DPI awareness on the calling thread, so this holds even
//! if the host process is not DPI aware. Window bounds describe the *visible* frame (DWM extended
//! frame bounds), not the larger `GetWindowRect` that includes the invisible resize border.
//!
//! Limitations (documented, not worked around):
//! - Windows of applications running as administrator cannot be moved, closed or focused unless
//!   Reframe is elevated too (User Interface Privilege Isolation); such calls fail with
//!   `PermissionDenied`.
//! - Only windows on the current virtual desktop are listed: Windows cloaks the others and there
//!   is no supported API to enumerate or address other desktops.
//! - Focus changes are subject to focus-stealing prevention (see [`placement::focus_window`]).
//! - Fullscreen is controlled by each application; restoring it maximizes the window.
//! - Optional APIs (per-monitor DPI, packaged-app identity, DPI-aware metrics) are resolved at
//!   run time, so older Windows builds and compatibility layers degrade instead of failing.
//! - Where an API is unavailable or rejected (older builds, remote sessions, compatibility layers
//!   such as Wine), Reframe falls back rather than failing: without cloaking information windows
//!   count as not cloaked, without DWM frame bounds the outer window rectangle is reported, without
//!   the display configuration GDI device names serve as monitor ids, a rejected
//!   `SetWindowPlacement` is replaced by `ShowWindowAsync` + `SetWindowPos`, and a window icon that
//!   cannot be read is taken from the window's executable.
//!
//! Structure: [`logic`] holds the platform-independent decisions (window filtering, coordinate
//! conversions, pixel conversion, ranking...) and is unit-tested on every OS; the other modules
//! wrap Win32 calls in small safe functions.

mod ffi;
mod icon;
mod launch;
mod locate;
mod logic;
mod monitor;
mod placement;
mod process;
mod window;

#[cfg(test)]
mod tests;

use std::sync::{Mutex, MutexGuard};

use crate::common::process as common_process;
use crate::*;

use self::ffi::{ComScope, DpiAwarenessScope};
use self::locate::LocateCache;
use self::monitor::MonitorSnapshot;
use self::process::ProcessCache;
use self::window::InsetsCache;

const UIPI_NOTE: &str = "Windows of applications running as administrator can only be \
     controlled when Reframe also runs as administrator.";

/// The Win32 [`Platform`]. Cheap to call repeatedly; caches are internal and thread-safe.
#[derive(Default)]
pub struct WindowsPlatform {
    insets: Mutex<InsetsCache>,
    processes: Mutex<ProcessCache>,
    locate: Mutex<LocateCache>,
}

impl WindowsPlatform {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Lock a cache, recovering from poisoning (a cache is always safe to reuse).
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Monitors for coordinate conversions; without them geometry is still reported, only monitor
/// ids and taskbar offsets are missing.
fn monitor_snapshot() -> MonitorSnapshot {
    MonitorSnapshot::capture().unwrap_or_else(|e| {
        log::warn!("monitor information unavailable: {e}");
        MonitorSnapshot::default()
    })
}

fn full(note: &str) -> Capability {
    Capability {
        level: SupportLevel::Full,
        note: Some(note.to_string()),
    }
}

impl Platform for WindowsPlatform {
    fn info(&self) -> PlatformInfo {
        PlatformInfo {
            os: OsKind::Windows,
            os_version: common_process::long_os_version(),
            display_server: DisplayServer::Win32,
            desktop_environment: None,
            capabilities: PlatformCapabilities {
                list_windows: full(
                    "Only windows on the current virtual desktop are listed; Windows hides the \
                     others from applications.",
                ),
                read_window_geometry: Capability::full(),
                move_windows: full(UIPI_NOTE),
                change_window_state: full(&format!(
                    "{UIPI_NOTE} Fullscreen is controlled by each application and is restored \
                     as maximized."
                )),
                close_windows: full(UIPI_NOTE),
                focus_windows: full(
                    "Windows may refuse to switch focus (focus-stealing prevention) and flash \
                     the taskbar button instead.",
                ),
                list_monitors: Capability::full(),
                process_details: full(
                    "Command lines and working directories of elevated or protected processes \
                     cannot be read.",
                ),
                application_icons: Capability::full(),
            },
        }
    }

    fn monitors(&self) -> PlatformResult<Vec<MonitorInfo>> {
        let _dpi = DpiAwarenessScope::per_monitor();
        Ok(MonitorSnapshot::capture()?.monitors())
    }

    fn windows(&self) -> PlatformResult<Vec<WindowInfo>> {
        let _dpi = DpiAwarenessScope::per_monitor();
        let _com = ComScope::apartment();
        let monitors = monitor_snapshot();
        window::list_windows(&monitors, &mut lock(&self.insets))
    }

    fn processes(&self) -> PlatformResult<Vec<ProcessInfo>> {
        let mut processes = common_process::list_processes()?;
        process::enrich(&mut processes, &mut lock(&self.processes));
        Ok(processes)
    }

    fn launch(&self, request: &LaunchRequest) -> PlatformResult<LaunchedProcess> {
        launch::launch(request)
    }

    fn set_window_placement(
        &self,
        window: &WindowId,
        placement: &WindowPlacement,
    ) -> PlatformResult<()> {
        let _dpi = DpiAwarenessScope::per_monitor();
        let monitors = monitor_snapshot();
        placement::set_window_placement(window, placement, &monitors, &mut lock(&self.insets))
    }

    fn close_window(&self, window: &WindowId) -> PlatformResult<()> {
        placement::close_window(window)
    }

    fn focus_window(&self, window: &WindowId) -> PlatformResult<()> {
        placement::focus_window(window)
    }

    fn application_icon(&self, request: &IconRequest) -> PlatformResult<Option<Vec<u8>>> {
        icon::application_icon(request)
    }

    fn locate_application(&self, query: &LocateQuery) -> Vec<LaunchTarget> {
        locate::locate(query, &mut lock(&self.locate))
    }

    fn permissions(&self) -> Vec<PermissionStatus> {
        Vec::new()
    }

    fn request_permission(&self, kind: PermissionKind) -> PlatformResult<()> {
        Err(PlatformError::unsupported(
            format!("Requesting the {kind:?} permission"),
            "Windows has no such permission; Reframe needs no extra permissions on Windows.",
        ))
    }
}
