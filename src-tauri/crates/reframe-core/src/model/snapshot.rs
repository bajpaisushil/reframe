//! Desktop discovery results ("Explore") and capture requests.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use reframe_platform::{MonitorInfo, PackageInfo, PlatformInfo, ProcessInfo, WindowId, WindowInfo};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Raw data read from the platform once per scan. Errors become `warnings` so a partially
/// supported platform still produces a useful result.
#[derive(Debug, Clone)]
pub struct DesktopState {
    pub info: PlatformInfo,
    pub monitors: Vec<MonitorInfo>,
    pub windows: Vec<WindowInfo>,
    pub processes: Vec<ProcessInfo>,
    pub warnings: Vec<String>,
}

/// How completely an adapter can restore an application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CapabilityLevel {
    /// Application state (projects, tabs, folders...) and windows are restored.
    Full,
    /// Some state is restored; limits are listed in the features.
    Partial,
    /// Reframe can start the application and place its windows, nothing more.
    LaunchOnly,
    /// Reframe cannot restore this application on this system.
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CapabilityFeature {
    /// e.g. "Reopen project folders", "Restore tabs", "Window positions".
    pub name: String,
    pub supported: bool,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AdapterCapability {
    pub level: CapabilityLevel,
    pub features: Vec<CapabilityFeature>,
}

impl AdapterCapability {
    pub fn launch_only() -> Self {
        Self {
            level: CapabilityLevel::LaunchOnly,
            features: Vec::new(),
        }
    }

    pub fn feature(mut self, name: &str, supported: bool, note: Option<&str>) -> Self {
        self.features.push(CapabilityFeature {
            name: name.to_string(),
            supported,
            note: note.map(str::to_string),
        });
        self
    }
}

/// How a discovered application is classified for capture suggestions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum AppCategory {
    /// A user-facing application: suggested for capture.
    Application,
    /// Small helper/utility windows (settings panels, tray popups...): shown, not suggested.
    Utility,
    /// OS shell / desktop components: hidden from suggestions.
    System,
}

/// A short "label: value" fact shown in the UI ("Project: reframe", "Tabs: 12").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StateDetail {
    pub label: String,
    pub value: String,
}

impl StateDetail {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
        }
    }
}

/// One application with visible windows, as shown in Explore.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiscoveredApplication {
    /// Grouping key, stable for the lifetime of the snapshot (used in capture selections).
    pub key: String,
    pub app_id: String,
    pub display_name: String,
    pub adapter_id: String,
    pub capability: AdapterCapability,
    pub category: AppCategory,
    pub executable_path: Option<PathBuf>,
    pub package: Option<PackageInfo>,
    /// Processes owning the windows (main process first).
    pub pids: Vec<u32>,
    pub windows: Vec<WindowInfo>,
    /// Pre-selected in the capture UI.
    pub suggested: bool,
    /// Matches the user's exclusion list.
    pub excluded: bool,
    /// Adapter preview of restorable state.
    pub details: Vec<StateDetail>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DesktopSnapshot {
    pub captured_at: DateTime<Utc>,
    pub platform: PlatformInfo,
    pub monitors: Vec<MonitorInfo>,
    /// Applications with at least one visible window, most recently used first.
    pub applications: Vec<DiscoveredApplication>,
    /// Processes without visible windows (services, daemons, helpers) — counted, not listed.
    pub background_process_count: u32,
    pub warnings: Vec<String>,
}

/// Which windows of one discovered application to include.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CaptureSelection {
    /// `DiscoveredApplication::key`.
    pub app_key: String,
    /// Subset of the application's windows; empty means all of them.
    pub window_ids: Vec<WindowId>,
}

/// Capture the current desktop into a new workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CaptureRequest {
    pub name: String,
    pub description: String,
    pub icon: String,
    pub color: Option<String>,
    pub selections: Vec<CaptureSelection>,
}

/// Result of capturing applications.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CaptureResult {
    pub entries: Vec<super::workspace::ApplicationEntry>,
    pub monitors: Vec<MonitorInfo>,
    pub warnings: Vec<String>,
}

/// A freshly captured workspace plus non-fatal capture warnings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CapturedWorkspace {
    pub workspace: super::workspace::Workspace,
    pub warnings: Vec<String>,
}
