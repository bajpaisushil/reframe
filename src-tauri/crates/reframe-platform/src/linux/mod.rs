//! Linux implementation: X11/EWMH via x11rb, Wayland detection.
//!
//! PLACEHOLDER: delegates to [`UnsupportedPlatform`] until the native implementation lands.

use crate::common::UnsupportedPlatform;
use crate::*;

pub struct LinuxPlatform {
    fallback: UnsupportedPlatform,
}

impl LinuxPlatform {
    pub fn new() -> Self {
        Self {
            fallback: UnsupportedPlatform::new(),
        }
    }
}

impl Default for LinuxPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl Platform for LinuxPlatform {
    fn info(&self) -> PlatformInfo {
        self.fallback.info()
    }
    fn monitors(&self) -> PlatformResult<Vec<MonitorInfo>> {
        self.fallback.monitors()
    }
    fn windows(&self) -> PlatformResult<Vec<WindowInfo>> {
        self.fallback.windows()
    }
    fn processes(&self) -> PlatformResult<Vec<ProcessInfo>> {
        self.fallback.processes()
    }
    fn launch(&self, request: &LaunchRequest) -> PlatformResult<LaunchedProcess> {
        self.fallback.launch(request)
    }
    fn set_window_placement(
        &self,
        window: &WindowId,
        placement: &WindowPlacement,
    ) -> PlatformResult<()> {
        self.fallback.set_window_placement(window, placement)
    }
    fn close_window(&self, window: &WindowId) -> PlatformResult<()> {
        self.fallback.close_window(window)
    }
    fn focus_window(&self, window: &WindowId) -> PlatformResult<()> {
        self.fallback.focus_window(window)
    }
    fn application_icon(&self, request: &IconRequest) -> PlatformResult<Option<Vec<u8>>> {
        self.fallback.application_icon(request)
    }
    fn locate_application(&self, query: &LocateQuery) -> Vec<LaunchTarget> {
        self.fallback.locate_application(query)
    }
    fn permissions(&self) -> Vec<PermissionStatus> {
        self.fallback.permissions()
    }
    fn request_permission(&self, kind: PermissionKind) -> PlatformResult<()> {
        self.fallback.request_permission(kind)
    }
}
