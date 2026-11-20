//! Portable JSON export format and the import mapping flow.
//!
//! An export separates the *logical* workspace (what to restore) from the *machine profile*
//! (where things live on the exporting computer). Importing on another computer resolves the
//! machine-specific parts again, asking the user when something cannot be found.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use reframe_platform::{LaunchTarget, MonitorInfo, OsKind, PackageInfo};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use super::state::AppState;
use super::workspace::{EntryOptions, RestoreConfiguration, WindowSnapshot};

pub const EXPORT_FORMAT: &str = "reframe.workspace";
pub const EXPORT_VERSION: u32 = 1;
/// Imports larger than this are rejected before parsing.
pub const MAX_IMPORT_BYTES: usize = 5 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WorkspaceExport {
    /// Always [`EXPORT_FORMAT`].
    pub format: String,
    pub version: u32,
    pub exported_at: DateTime<Utc>,
    pub app_version: String,
    pub workspace: PortableWorkspace,
    pub machine: MachineProfile,
}

/// Machine-independent workspace description.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PortableWorkspace {
    pub name: String,
    pub description: String,
    pub icon: String,
    pub color: Option<String>,
    pub applications: Vec<PortableApplication>,
    pub restore: RestoreConfiguration,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PortableApplication {
    pub id: Uuid,
    pub app_id: String,
    pub display_name: String,
    pub adapter_id: String,
    pub enabled: bool,
    pub arguments: Vec<String>,
    pub depends_on: Vec<Uuid>,
    /// Sanitized: no credentials, tokens or secret-looking URL parameters.
    pub state: AppState,
    pub windows: Vec<WindowSnapshot>,
    pub options: EntryOptions,
}

/// Where things lived on the exporting machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MachineProfile {
    pub os: OsKind,
    pub monitors: Vec<MonitorInfo>,
    pub applications: Vec<MachineApplication>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MachineApplication {
    pub entry_id: Uuid,
    pub executable: Option<PathBuf>,
    pub package: Option<PackageInfo>,
    pub working_directory: Option<PathBuf>,
}

/// How an application in an import was resolved on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum AppResolution {
    /// The exported executable/package exists here (same OS, same path).
    Exported { target: LaunchTarget },
    /// Found by searching this machine.
    Found { target: LaunchTarget },
    /// The user must pick the executable or skip the application.
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportApplication {
    pub entry_id: Uuid,
    pub app_id: String,
    pub display_name: String,
    pub adapter_id: String,
    pub resolution: AppResolution,
    /// Other candidates found on this machine.
    pub alternatives: Vec<LaunchTarget>,
    /// Paths referenced by the app state that do not exist here.
    pub missing_paths: Vec<PathBuf>,
}

/// Shown on the import mapping screen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportPreview {
    pub file_path: String,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub source_os: OsKind,
    pub exported_at: DateTime<Utc>,
    pub applications: Vec<ImportApplication>,
    /// Suggested folder remappings, e.g. `C:\Users\a\code` → `/home/a/code`.
    pub suggested_path_mappings: Vec<PathMapping>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PathMapping {
    pub from_prefix: String,
    pub to_prefix: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppMapping {
    pub entry_id: Uuid,
    /// User-chosen target; `None` keeps the automatic resolution.
    pub target: Option<LaunchTarget>,
    /// Leave this application out of the imported workspace.
    pub skip: bool,
}

/// The user's decisions from the mapping screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportResolution {
    pub file_path: String,
    pub name: Option<String>,
    pub app_mappings: Vec<AppMapping>,
    pub path_mappings: Vec<PathMapping>,
}
