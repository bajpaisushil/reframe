//! A read-only [`Platform`] view of a [`DesktopState`], used to give adapters a
//! [`LaunchContext`](crate::adapters::LaunchContext) while planning a switch.
//!
//! Switch planning is pure, so every operation with a side effect (launching, moving,
//! closing, focusing windows, permission prompts) returns [`PlatformError::Unsupported`].
//! Applications are located through `PATH` only — the platform-specific lookups (`.desktop`
//! files, registry App Paths, `/Applications`) need the real platform, so an adapter that
//! depends on them may report a launch problem here that a real restore would not hit.

use reframe_platform::common::locate;
use reframe_platform::{
    IconRequest, LaunchRequest, LaunchTarget, LaunchedProcess, LocateQuery, MonitorInfo,
    PermissionKind, PermissionStatus, Platform, PlatformError, PlatformInfo, PlatformResult,
    ProcessInfo, WindowId, WindowInfo, WindowPlacement,
};

use crate::model::DesktopState;

const REASON: &str = "switch planning never changes the desktop";

pub(super) struct SnapshotPlatform<'a> {
    state: &'a DesktopState,
    info: &'a PlatformInfo,
}

impl<'a> SnapshotPlatform<'a> {
    pub(super) fn new(state: &'a DesktopState, info: &'a PlatformInfo) -> Self {
        Self { state, info }
    }
}

impl Platform for SnapshotPlatform<'_> {
    fn info(&self) -> PlatformInfo {
        self.info.clone()
    }

    fn monitors(&self) -> PlatformResult<Vec<MonitorInfo>> {
        Ok(self.state.monitors.clone())
    }

    fn windows(&self) -> PlatformResult<Vec<WindowInfo>> {
        Ok(self.state.windows.clone())
    }

    fn processes(&self) -> PlatformResult<Vec<ProcessInfo>> {
        Ok(self.state.processes.clone())
    }

    fn launch(&self, _request: &LaunchRequest) -> PlatformResult<LaunchedProcess> {
        Err(PlatformError::unsupported("Launching applications", REASON))
    }

    fn set_window_placement(&self, _: &WindowId, _: &WindowPlacement) -> PlatformResult<()> {
        Err(PlatformError::unsupported("Moving windows", REASON))
    }

    fn close_window(&self, _: &WindowId) -> PlatformResult<()> {
        Err(PlatformError::unsupported("Closing windows", REASON))
    }

    fn focus_window(&self, _: &WindowId) -> PlatformResult<()> {
        Err(PlatformError::unsupported("Focusing windows", REASON))
    }

    fn application_icon(&self, _: &IconRequest) -> PlatformResult<Option<Vec<u8>>> {
        Ok(None)
    }

    fn locate_application(&self, query: &LocateQuery) -> Vec<LaunchTarget> {
        locate::find_on_path(query)
            .into_iter()
            .map(|path| LaunchTarget::Executable { path })
            .collect()
    }

    fn permissions(&self) -> Vec<PermissionStatus> {
        Vec::new()
    }

    fn request_permission(&self, _: PermissionKind) -> PlatformResult<()> {
        Err(PlatformError::unsupported("Requesting permissions", REASON))
    }
}
