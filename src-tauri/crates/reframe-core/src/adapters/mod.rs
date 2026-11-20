//! Application adapters.
//!
//! Every supported application has an adapter implementing [`AppAdapter`]: it recognises the
//! application's processes, captures restorable state, builds launch steps and reports what
//! it can restore on the current platform. The engine only talks to this trait, so adding an
//! application means adding one file and one line in [`AdapterRegistry::builtin`].
//!
//! Unknown applications fall back to [`generic::GenericAdapter`] (launch + window placement).

pub mod chromium;
pub mod file_manager;
pub mod generic;
pub mod music;
pub mod pdf;
pub mod terminal;
pub mod vscode;

use std::sync::Arc;

use reframe_platform::{
    LaunchRequest, LocateQuery, Platform, PlatformInfo, ProcessInfo, WindowInfo,
};

use crate::model::{AdapterCapability, AppState, ApplicationEntry, LaunchConfig, StateDetail};

/// The logical application a process belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppIdentity {
    /// Machine-independent id ("vscode", "chrome", "edge", "exe:gimp").
    pub app_id: String,
    pub display_name: String,
}

/// Inputs available while capturing.
pub struct CaptureContext<'a> {
    pub platform: &'a dyn Platform,
    pub info: &'a PlatformInfo,
    /// The user allows reading browser session files (optional browser integration).
    pub browser_tab_capture: bool,
}

/// The application being captured.
pub struct CaptureInput<'a> {
    pub identity: &'a AppIdentity,
    /// Processes owning the selected windows; the first is the main process.
    pub processes: Vec<&'a ProcessInfo>,
    /// Selected windows of this application, front-to-back.
    pub windows: Vec<&'a WindowInfo>,
    /// Every process on the system (e.g. to find shells running inside a terminal).
    pub all_processes: &'a [ProcessInfo],
}

impl CaptureInput<'_> {
    pub fn main_process(&self) -> Option<&ProcessInfo> {
        self.processes.first().copied()
    }
}

/// Result of [`AppAdapter::capture`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CaptureOutput {
    pub state: AppState,
    pub launch: LaunchConfig,
    /// Non-fatal problems ("Could not read session file; tabs were not captured").
    pub warnings: Vec<String>,
}

/// Inputs available while planning a launch.
pub struct LaunchContext<'a> {
    pub platform: &'a dyn Platform,
    pub info: &'a PlatformInfo,
}

/// One process launch performed during restore.
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchStep {
    pub request: LaunchRequest,
    /// Windows this step is expected to open (0 when the app may legitimately open none).
    pub expected_windows: u32,
    /// Text expected in the resulting window title (project/folder/document name). Used to
    /// match new windows to saved positions.
    pub title_hint: Option<String>,
}

/// How an application's processes relate to its windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceModel {
    /// Each launch creates a new process that owns its windows.
    MultiProcess,
    /// Launches are forwarded to an already-running instance (VS Code, Chrome, Explorer):
    /// new windows may belong to a different pid than the one launched.
    SingleInstance,
}

pub trait AppAdapter: Send + Sync {
    /// Stable identifier stored in workspaces ("vscode", "chromium", "terminal", "generic").
    fn id(&self) -> &'static str;

    /// Human-readable adapter name.
    fn name(&self) -> &'static str;

    /// Return the logical application if this adapter handles `process`.
    fn identify(&self, process: &ProcessInfo, windows: &[&WindowInfo]) -> Option<AppIdentity>;

    /// What this adapter can restore on `info`'s platform.
    fn capability(&self, info: &PlatformInfo) -> AdapterCapability;

    /// What this adapter can restore for one of its applications (`app_id`, as returned by
    /// [`AppAdapter::identify`]) on `info`'s platform. Adapters whose applications differ (PDF
    /// readers with and without page support, Spotify vs. other music players) override this;
    /// the default is the adapter-wide [`AppAdapter::capability`].
    fn capability_for_app(&self, app_id: &str, info: &PlatformInfo) -> AdapterCapability {
        let _ = app_id;
        self.capability(info)
    }

    /// Extract restorable state. Must degrade gracefully: on failure return
    /// `AppState::None` (or partial state) plus a warning, never panic.
    fn capture(&self, ctx: &CaptureContext<'_>, input: &CaptureInput<'_>) -> CaptureOutput;

    /// Launch steps that restore `entry`. Returns a user-facing error when the entry cannot be
    /// launched (e.g. no executable configured).
    fn launch_plan(
        &self,
        ctx: &LaunchContext<'_>,
        entry: &ApplicationEntry,
    ) -> Result<Vec<LaunchStep>, String>;

    fn instance_model(&self) -> InstanceModel {
        InstanceModel::MultiProcess
    }

    /// Short summary of `state` for the UI.
    fn describe(&self, state: &AppState) -> Vec<StateDetail>;

    /// How to find this application on a machine (import mapping, availability checks).
    fn locate_query(&self, app_id: &str) -> LocateQuery;

    /// Remove sensitive data before export. The default sanitizes URLs and drops anything that
    /// looks like a credential.
    fn sanitize_for_export(&self, state: &AppState) -> AppState {
        crate::portability::sanitize::sanitize_state(state)
    }
}

/// All adapters, most specific first; the generic adapter is the fallback.
pub struct AdapterRegistry {
    adapters: Vec<Arc<dyn AppAdapter>>,
    generic: Arc<dyn AppAdapter>,
}

impl AdapterRegistry {
    pub fn builtin() -> Self {
        Self::with_adapters(vec![
            Arc::new(vscode::VsCodeAdapter::new()),
            Arc::new(chromium::ChromiumAdapter::new()),
            Arc::new(terminal::TerminalAdapter::new()),
            Arc::new(file_manager::FileManagerAdapter::new()),
            Arc::new(pdf::PdfAdapter::new()),
            Arc::new(music::MusicAdapter::new()),
        ])
    }

    pub fn with_adapters(adapters: Vec<Arc<dyn AppAdapter>>) -> Self {
        Self {
            adapters,
            generic: Arc::new(generic::GenericAdapter::new()),
        }
    }

    /// The adapter responsible for `process` and the logical application it belongs to.
    pub fn identify(
        &self,
        process: &ProcessInfo,
        windows: &[&WindowInfo],
    ) -> (Arc<dyn AppAdapter>, AppIdentity) {
        for adapter in &self.adapters {
            if let Some(identity) = adapter.identify(process, windows) {
                return (Arc::clone(adapter), identity);
            }
        }
        let identity = self
            .generic
            .identify(process, windows)
            .expect("the generic adapter identifies every process");
        (Arc::clone(&self.generic), identity)
    }

    /// Adapter by id; unknown ids (e.g. from a newer Reframe version) use the generic adapter.
    pub fn get(&self, id: &str) -> Arc<dyn AppAdapter> {
        self.adapters
            .iter()
            .find(|a| a.id() == id)
            .cloned()
            .unwrap_or_else(|| Arc::clone(&self.generic))
    }

    pub fn generic(&self) -> Arc<dyn AppAdapter> {
        Arc::clone(&self.generic)
    }

    pub fn all(&self) -> impl Iterator<Item = &Arc<dyn AppAdapter>> {
        self.adapters.iter().chain(std::iter::once(&self.generic))
    }
}

impl Default for AdapterRegistry {
    fn default() -> Self {
        Self::builtin()
    }
}
