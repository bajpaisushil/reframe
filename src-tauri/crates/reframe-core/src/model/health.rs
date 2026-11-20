//! Restore readiness report shown on the workspace detail page.

use std::path::PathBuf;

use reframe_platform::LaunchTarget;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use super::snapshot::{AdapterCapability, StateDetail};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EntryHealth {
    pub entry_id: Uuid,
    /// The application can be launched on this machine.
    pub available: bool,
    pub launch_target: Option<LaunchTarget>,
    /// Paths in the saved state that no longer exist.
    pub missing_paths: Vec<PathBuf>,
    pub capability: AdapterCapability,
    /// Currently running (would be reused in Merge/Replace mode).
    pub running: bool,
    pub details: Vec<StateDetail>,
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MonitorMapping {
    pub saved_id: String,
    pub saved_name: String,
    /// Connected monitor the saved one maps to (`None` = falls back to the primary display).
    pub mapped_id: Option<String>,
    pub mapped_name: Option<String>,
    pub exact: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RestoreEstimate {
    pub full: u32,
    pub partial: u32,
    pub launch_only: u32,
    pub unavailable: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WorkspaceHealth {
    pub workspace_id: Uuid,
    pub entries: Vec<EntryHealth>,
    pub monitors: Vec<MonitorMapping>,
    /// Dependency problems (cycles, missing references).
    pub dependency_issues: Vec<String>,
    /// Platform limitations relevant to this workspace (e.g. Wayland window placement).
    pub platform_notes: Vec<String>,
    pub estimate: RestoreEstimate,
}
