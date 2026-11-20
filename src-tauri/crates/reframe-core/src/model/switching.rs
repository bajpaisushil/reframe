//! Workspace switching modes and the plan shown to the user before switching.

use reframe_platform::WindowId;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SwitchMode {
    /// Close (with confirmation) apps that belong only to the previous workspace, keep shared
    /// apps, launch what is missing.
    Replace,
    /// Keep everything running, launch what is missing, apply window layout.
    #[default]
    Merge,
    /// Launch everything in the workspace without closing or reusing anything.
    Additive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlannedLaunch {
    pub entry_id: Uuid,
    pub display_name: String,
    pub app_id: String,
    /// Why it will be launched ("Not running", "Additive mode").
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlannedReuse {
    pub entry_id: Uuid,
    pub display_name: String,
    pub app_id: String,
    /// Running windows that satisfy the entry (they are repositioned, not relaunched).
    pub window_ids: Vec<WindowId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CloseCandidate {
    pub window_id: WindowId,
    pub pid: u32,
    pub title: String,
    pub app_id: String,
    pub app_name: String,
    /// Pre-checked in the confirmation dialog (true only for windows belonging exclusively to
    /// the previously active workspace).
    pub default_selected: bool,
    pub reason: String,
}

/// What a switch would do. Nothing is closed unless the user confirms the candidates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SwitchPlan {
    pub workspace_id: Uuid,
    pub mode: SwitchMode,
    pub previous_workspace_id: Option<Uuid>,
    pub launch: Vec<PlannedLaunch>,
    pub reuse: Vec<PlannedReuse>,
    /// Replace mode only.
    pub close: Vec<CloseCandidate>,
    /// Missing executables, unsupported platform operations, etc.
    pub warnings: Vec<String>,
}
