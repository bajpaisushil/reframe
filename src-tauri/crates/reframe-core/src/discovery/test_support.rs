//! Fixtures shared by the discovery and capture tests.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use reframe_platform::mock::MockPlatform;
use reframe_platform::{
    LocateQuery, OsKind, Platform, PlatformInfo, ProcessInfo, Rect, WindowId, WindowInfo,
};

use super::{DiscoveryOptions, build_snapshot, gather};
use crate::adapters::{
    AdapterRegistry, AppAdapter, AppIdentity, CaptureContext, CaptureInput, CaptureOutput,
    LaunchContext, LaunchStep,
};
use crate::model::{
    AdapterCapability, AppState, ApplicationEntry, CapabilityLevel, DesktopSnapshot, LaunchConfig,
    ProjectRef, StateDetail,
};

/// A registry with only the generic adapter (stable whatever the built-in adapters claim).
pub(crate) fn generic_only() -> AdapterRegistry {
    AdapterRegistry::with_adapters(Vec::new())
}

pub(crate) fn registry(adapters: Vec<Arc<dyn AppAdapter>>) -> AdapterRegistry {
    AdapterRegistry::with_adapters(adapters)
}

pub(crate) fn options() -> DiscoveryOptions {
    DiscoveryOptions::default()
}

pub(crate) fn scan(
    platform: &MockPlatform,
    adapters: &AdapterRegistry,
    options: &DiscoveryOptions,
) -> DesktopSnapshot {
    let state = gather(platform);
    build_snapshot(&state, platform, adapters, options)
}

pub(crate) fn process(pid: u32, name: &str, exe: Option<&str>) -> ProcessInfo {
    ProcessInfo {
        pid,
        parent_pid: None,
        name: name.to_string(),
        product_name: None,
        executable_path: exe.map(PathBuf::from),
        command_line: vec![name.to_string()],
        working_directory: None,
        start_time: None,
        package: None,
    }
}

pub(crate) fn rect(width: i32, height: i32) -> Rect {
    Rect::new(100, 100, width, height)
}

/// Add a window and return its id (new windows open on top in the mock).
pub(crate) fn window(platform: &MockPlatform, pid: u32, title: &str) -> WindowId {
    platform.add_window(pid, title, None, rect(800, 600))
}

/// Override the reported OS (keeps the mock's capabilities).
pub(crate) fn set_os(platform: &MockPlatform, os: OsKind) {
    let info = PlatformInfo {
        os,
        ..platform.info()
    };
    platform.set_info(info);
}

/// Edit a window in place.
pub(crate) fn edit_window(
    platform: &MockPlatform,
    id: &WindowId,
    edit: impl FnOnce(&mut WindowInfo),
) {
    let mut state = platform.state();
    if let Some(w) = state.windows.iter_mut().find(|w| &w.id == id) {
        edit(w);
    }
}

/// What a [`TestAdapter`] saw in one `capture` call.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Seen {
    pub app_id: String,
    pub windows: Vec<WindowId>,
    pub pids: Vec<u32>,
    pub all_processes: usize,
    pub browser_tab_capture: bool,
}

/// Claims processes with a given name, records its capture inputs and returns a project state.
pub(crate) struct TestAdapter {
    pub claims: &'static str,
    pub warnings: Vec<String>,
    pub seen: Mutex<Vec<Seen>>,
}

impl TestAdapter {
    pub(crate) fn new(claims: &'static str) -> Arc<Self> {
        Arc::new(Self {
            claims,
            warnings: Vec::new(),
            seen: Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn with_warning(claims: &'static str, warning: &str) -> Arc<Self> {
        Arc::new(Self {
            claims,
            warnings: vec![warning.to_string()],
            seen: Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

pub(crate) fn test_state() -> AppState {
    AppState::Projects {
        projects: vec![ProjectRef {
            path: "/home/me/code/reframe".into(),
            is_workspace_file: false,
            window_title: Some("reframe".into()),
        }],
    }
}

impl AppAdapter for TestAdapter {
    fn id(&self) -> &'static str {
        "test"
    }

    fn name(&self) -> &'static str {
        "Test integration"
    }

    fn identify(&self, process: &ProcessInfo, _windows: &[&WindowInfo]) -> Option<AppIdentity> {
        (process.name == self.claims).then(|| AppIdentity {
            app_id: format!("test-{}", self.claims),
            display_name: "Test App".to_string(),
        })
    }

    fn capability(&self, _info: &PlatformInfo) -> AdapterCapability {
        AdapterCapability {
            level: CapabilityLevel::Full,
            features: Vec::new(),
        }
        .feature("Reopen projects", true, None)
    }

    fn capture(&self, ctx: &CaptureContext<'_>, input: &CaptureInput<'_>) -> CaptureOutput {
        self.seen.lock().unwrap().push(Seen {
            app_id: input.identity.app_id.clone(),
            windows: input.windows.iter().map(|w| w.id.clone()).collect(),
            pids: input.processes.iter().map(|p| p.pid).collect(),
            all_processes: input.all_processes.len(),
            browser_tab_capture: ctx.browser_tab_capture,
        });
        CaptureOutput {
            state: test_state(),
            launch: LaunchConfig {
                executable: input.main_process().and_then(|p| p.executable_path.clone()),
                ..LaunchConfig::default()
            },
            warnings: self.warnings.clone(),
        }
    }

    fn launch_plan(
        &self,
        _ctx: &LaunchContext<'_>,
        _entry: &ApplicationEntry,
    ) -> Result<Vec<LaunchStep>, String> {
        Err("not used in these tests".to_string())
    }

    fn describe(&self, state: &AppState) -> Vec<StateDetail> {
        match state {
            AppState::Projects { projects } => {
                vec![StateDetail::new("Projects", projects.len().to_string())]
            }
            _ => Vec::new(),
        }
    }

    fn locate_query(&self, _app_id: &str) -> LocateQuery {
        LocateQuery::default()
    }
}

/// Where a [`PanicAdapter`] panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PanicAt {
    /// Panics while identifying *every* process.
    Identify,
    Id,
    Capability,
    Capture,
    Describe,
}

/// Claims processes with a given name (except with [`PanicAt::Identify`]) and panics at one
/// point.
pub(crate) struct PanicAdapter {
    pub at: PanicAt,
    pub claims: &'static str,
}

impl PanicAdapter {
    pub(crate) fn new(at: PanicAt, claims: &'static str) -> Arc<Self> {
        Arc::new(Self { at, claims })
    }

    fn maybe_panic(&self, at: PanicAt) {
        if self.at == at {
            panic!("simulated adapter failure in {at:?}");
        }
    }
}

impl AppAdapter for PanicAdapter {
    fn id(&self) -> &'static str {
        self.maybe_panic(PanicAt::Id);
        "panicky"
    }

    fn name(&self) -> &'static str {
        "Panicky"
    }

    fn identify(&self, process: &ProcessInfo, _windows: &[&WindowInfo]) -> Option<AppIdentity> {
        self.maybe_panic(PanicAt::Identify);
        (process.name == self.claims).then(|| AppIdentity {
            app_id: "panicky-app".to_string(),
            display_name: "Panicky App".to_string(),
        })
    }

    fn capability(&self, _info: &PlatformInfo) -> AdapterCapability {
        self.maybe_panic(PanicAt::Capability);
        AdapterCapability {
            level: CapabilityLevel::Full,
            features: Vec::new(),
        }
    }

    fn capture(&self, _ctx: &CaptureContext<'_>, _input: &CaptureInput<'_>) -> CaptureOutput {
        self.maybe_panic(PanicAt::Capture);
        CaptureOutput {
            state: test_state(),
            ..CaptureOutput::default()
        }
    }

    fn launch_plan(
        &self,
        _ctx: &LaunchContext<'_>,
        _entry: &ApplicationEntry,
    ) -> Result<Vec<LaunchStep>, String> {
        Err("not used in these tests".to_string())
    }

    fn describe(&self, _state: &AppState) -> Vec<StateDetail> {
        self.maybe_panic(PanicAt::Describe);
        vec![StateDetail::new("Panicky", "yes")]
    }

    fn locate_query(&self, _app_id: &str) -> LocateQuery {
        LocateQuery::default()
    }
}
