//! Fallback for operating systems without a native implementation.

use crate::common::{launch, locate, process};
use crate::*;

/// Lists processes and launches executables; everything window-related is unsupported.
#[derive(Debug, Default)]
pub struct UnsupportedPlatform;

impl UnsupportedPlatform {
    pub fn new() -> Self {
        Self
    }
}

const REASON: &str = "Reframe has no window-management integration for this operating system.";

impl Platform for UnsupportedPlatform {
    fn info(&self) -> PlatformInfo {
        PlatformInfo {
            os: OsKind::current(),
            os_version: process::long_os_version(),
            display_server: DisplayServer::Unknown,
            desktop_environment: None,
            capabilities: PlatformCapabilities::all_unsupported(REASON),
        }
    }

    fn monitors(&self) -> PlatformResult<Vec<MonitorInfo>> {
        Err(PlatformError::unsupported("Listing monitors", REASON))
    }

    fn windows(&self) -> PlatformResult<Vec<WindowInfo>> {
        Err(PlatformError::unsupported("Listing windows", REASON))
    }

    fn processes(&self) -> PlatformResult<Vec<ProcessInfo>> {
        process::list_processes()
    }

    fn launch(&self, request: &LaunchRequest) -> PlatformResult<LaunchedProcess> {
        match &request.target {
            LaunchTarget::Executable { path } => {
                let pid = launch::spawn_detached(
                    path,
                    &request.args,
                    request.working_directory.as_deref(),
                    &request.env,
                )?;
                Ok(LaunchedProcess { pid: Some(pid) })
            }
            other => Err(PlatformError::unsupported(
                format!("Launching {}", other.label()),
                REASON,
            )),
        }
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

    fn request_permission(&self, kind: PermissionKind) -> PlatformResult<()> {
        Err(PlatformError::unsupported(
            format!("{kind:?} permission"),
            REASON,
        ))
    }
}
