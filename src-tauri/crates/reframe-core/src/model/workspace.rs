//! Workspaces: the saved, restorable description of a working environment.

use std::collections::HashSet;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use reframe_platform::{MonitorInfo, PackageInfo, Rect, WindowState};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use super::state::AppState;
use super::switching::SwitchMode;

/// Version of the workspace document format (stored with every workspace and export).
pub const WORKSPACE_SCHEMA_VERSION: u32 = 1;

pub const MAX_NAME_LEN: usize = 80;
pub const MAX_DESCRIPTION_LEN: usize = 500;
pub const MAX_APPLICATIONS: usize = 100;
pub const MAX_WINDOWS_PER_APP: usize = 50;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Workspace {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    /// Emoji shown next to the workspace name.
    pub icon: String,
    /// Optional accent color (`#RRGGBB`).
    pub color: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_restored_at: Option<DateTime<Utc>>,
    /// Applications in launch order: among entries that do not depend on each other, a lower
    /// index launches first.
    pub applications: Vec<ApplicationEntry>,
    /// Monitor layout at capture time, used to remap windows onto a different display setup.
    pub monitors: Vec<MonitorInfo>,
    pub restore: RestoreConfiguration,
    pub schema_version: u32,
}

impl Workspace {
    /// A new, empty workspace.
    pub fn new(name: impl Into<String>, icon: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            description: String::new(),
            icon: icon.into(),
            color: None,
            created_at: now,
            updated_at: now,
            last_restored_at: None,
            applications: Vec::new(),
            monitors: Vec::new(),
            restore: RestoreConfiguration::default(),
            schema_version: WORKSPACE_SCHEMA_VERSION,
        }
    }

    pub fn entry(&self, id: Uuid) -> Option<&ApplicationEntry> {
        self.applications.iter().find(|e| e.id == id)
    }

    /// Structural validation (names, sizes, references). Dependency *cycles* are reported by
    /// the restoration planner, which also needs them for ordering.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut problems = Vec::new();
        let name = self.name.trim();
        if name.is_empty() {
            problems.push("Workspace name cannot be empty.".to_string());
        }
        if name.chars().count() > MAX_NAME_LEN {
            problems.push(format!(
                "Workspace name must be at most {MAX_NAME_LEN} characters."
            ));
        }
        if self.description.chars().count() > MAX_DESCRIPTION_LEN {
            problems.push(format!(
                "Description must be at most {MAX_DESCRIPTION_LEN} characters."
            ));
        }
        if self.icon.chars().count() > 16 {
            problems.push("Icon must be a single emoji.".to_string());
        }
        if let Some(color) = &self.color {
            let valid = color.len() == 7
                && color.starts_with('#')
                && color[1..].chars().all(|c| c.is_ascii_hexdigit());
            if !valid {
                problems.push(format!("Color {color:?} must look like #RRGGBB."));
            }
        }
        if self.applications.len() > MAX_APPLICATIONS {
            problems.push(format!(
                "A workspace can contain at most {MAX_APPLICATIONS} applications."
            ));
        }
        let ids: HashSet<Uuid> = self.applications.iter().map(|e| e.id).collect();
        if ids.len() != self.applications.len() {
            problems.push("Application entries must have unique ids.".to_string());
        }
        for entry in &self.applications {
            if entry.display_name.trim().is_empty() {
                problems.push("Every application needs a name.".to_string());
            }
            if entry.windows.len() > MAX_WINDOWS_PER_APP {
                problems.push(format!("{} has too many windows.", entry.display_name));
            }
            for dep in &entry.depends_on {
                if *dep == entry.id {
                    problems.push(format!("{} cannot depend on itself.", entry.display_name));
                } else if !ids.contains(dep) {
                    problems.push(format!(
                        "{} depends on an application that is not in this workspace.",
                        entry.display_name
                    ));
                }
            }
        }
        problems.extend(self.restore.validate());
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems)
        }
    }
}

/// One application inside a workspace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ApplicationEntry {
    /// Unique within the workspace; referenced by `depends_on`.
    pub id: Uuid,
    /// Logical, machine-independent application id ("vscode", "chrome", "exe:gimp").
    pub app_id: String,
    pub display_name: String,
    /// Adapter responsible for capture/restore ("vscode", "chromium", "generic", ...).
    pub adapter_id: String,
    /// Disabled entries are kept but skipped during restore.
    pub enabled: bool,
    /// Machine-specific launch information.
    pub launch: LaunchConfig,
    /// Entries that must be running before this one starts.
    pub depends_on: Vec<Uuid>,
    /// Adapter-specific restoration data (projects, tabs, folders, documents...).
    pub state: AppState,
    /// Saved window arrangement, one per window.
    pub windows: Vec<WindowSnapshot>,
    pub options: EntryOptions,
}

impl ApplicationEntry {
    pub fn new(
        app_id: impl Into<String>,
        display_name: impl Into<String>,
        adapter_id: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            app_id: app_id.into(),
            display_name: display_name.into(),
            adapter_id: adapter_id.into(),
            enabled: true,
            launch: LaunchConfig::default(),
            depends_on: Vec::new(),
            state: AppState::None,
            windows: Vec::new(),
            options: EntryOptions::default(),
        }
    }
}

/// How to start the application on *this* machine. Exports move this into the machine
/// profile so the logical workspace stays portable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct LaunchConfig {
    pub executable: Option<PathBuf>,
    /// macOS bundle, Windows packaged app, Flatpak/Snap.
    pub package: Option<PackageInfo>,
    /// Extra arguments explicitly configured by the user (or safe adapter-derived ones).
    /// Captured command lines are never stored blindly: they can contain secrets.
    pub arguments: Vec<String>,
    pub working_directory: Option<PathBuf>,
}

/// A saved window position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WindowSnapshot {
    /// Title at capture time; used to match restored windows to saved positions.
    pub title: String,
    pub class_name: Option<String>,
    /// Normal (restored) bounds in virtual-desktop coordinates.
    pub bounds: Rect,
    pub state: WindowState,
    /// Monitor the window was on (see `Workspace::monitors`).
    pub monitor_id: Option<String>,
}

/// Per-application overrides of the workspace restore configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct EntryOptions {
    /// Extra wait after this application is up before its dependents start.
    pub delay_after_launch_ms: Option<u32>,
    /// Override of the workspace window-wait timeout.
    pub timeout_ms: Option<u32>,
    /// Override of the workspace retry count.
    pub retry_count: Option<u32>,
    pub restore_window_positions: bool,
}

impl Default for EntryOptions {
    fn default() -> Self {
        Self {
            delay_after_launch_ms: None,
            timeout_ms: None,
            retry_count: None,
            restore_window_positions: true,
        }
    }
}

/// Workspace-wide restore behaviour.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct RestoreConfiguration {
    pub default_mode: SwitchMode,
    /// Pause between consecutive launches.
    pub launch_delay_ms: u32,
    /// How long to wait for an application's windows to appear.
    pub timeout_ms: u32,
    /// Additional launch attempts after a failure.
    pub retry_count: u32,
    pub restore_window_positions: bool,
    /// Maximum applications launching at the same time.
    pub max_parallel: u32,
}

impl Default for RestoreConfiguration {
    fn default() -> Self {
        Self {
            default_mode: SwitchMode::Merge,
            launch_delay_ms: 300,
            timeout_ms: 20_000,
            retry_count: 1,
            restore_window_positions: true,
            max_parallel: 3,
        }
    }
}

impl RestoreConfiguration {
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.launch_delay_ms > 60_000 {
            problems.push("Launch delay must be at most 60 seconds.".to_string());
        }
        if !(1_000..=300_000).contains(&self.timeout_ms) {
            problems.push("Timeout must be between 1 and 300 seconds.".to_string());
        }
        if self.retry_count > 5 {
            problems.push("At most 5 retries are allowed.".to_string());
        }
        if !(1..=8).contains(&self.max_parallel) {
            problems.push("Parallel launches must be between 1 and 8.".to_string());
        }
        problems
    }
}

/// Lightweight listing row for the dashboard and workspace list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WorkspaceSummary {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub color: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_restored_at: Option<DateTime<Utc>>,
    pub application_count: u32,
    /// Display names of the first few applications.
    pub application_names: Vec<String>,
    /// App ids + executables of the first few applications (for icons).
    pub application_icons: Vec<IconLookup>,
    pub is_active: bool,
}

impl WorkspaceSummary {
    pub fn from_workspace(ws: &Workspace, active: Option<Uuid>) -> Self {
        Self {
            id: ws.id,
            name: ws.name.clone(),
            description: ws.description.clone(),
            icon: ws.icon.clone(),
            color: ws.color.clone(),
            created_at: ws.created_at,
            updated_at: ws.updated_at,
            last_restored_at: ws.last_restored_at,
            application_count: ws.applications.len() as u32,
            application_names: ws
                .applications
                .iter()
                .take(8)
                .map(|e| e.display_name.clone())
                .collect(),
            application_icons: ws
                .applications
                .iter()
                .take(8)
                .map(IconLookup::for_entry)
                .collect(),
            is_active: active == Some(ws.id),
        }
    }
}

/// Input for creating an empty workspace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NewWorkspace {
    pub name: String,
    pub description: String,
    pub icon: String,
    pub color: Option<String>,
}

/// Identifies an application icon to fetch.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IconLookup {
    pub app_id: String,
    pub executable_path: Option<PathBuf>,
    pub package: Option<PackageInfo>,
}

impl IconLookup {
    pub fn for_entry(entry: &ApplicationEntry) -> Self {
        Self {
            app_id: entry.app_id.clone(),
            executable_path: entry.launch.executable.clone(),
            package: entry.launch.package.clone(),
        }
    }

    /// Cache key for the icon cache.
    pub fn cache_key(&self) -> String {
        if let Some(p) = &self.package {
            format!("pkg:{}", p.id)
        } else if let Some(exe) = &self.executable_path {
            format!("exe:{}", exe.display())
        } else {
            format!("app:{}", self.app_id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_catches_common_problems() {
        let mut ws = Workspace::new("  ", "🧪");
        let mut a = ApplicationEntry::new("x", "X", "generic");
        a.depends_on.push(a.id);
        let mut b = ApplicationEntry::new("y", "Y", "generic");
        b.depends_on.push(Uuid::new_v4());
        ws.applications = vec![a, b];
        ws.color = Some("red".into());
        let problems = ws.validate().unwrap_err();
        assert!(problems.iter().any(|p| p.contains("name cannot be empty")));
        assert!(problems.iter().any(|p| p.contains("depend on itself")));
        assert!(problems.iter().any(|p| p.contains("not in this workspace")));
        assert!(problems.iter().any(|p| p.contains("#RRGGBB")));
    }

    #[test]
    fn valid_workspace_passes() {
        let mut ws = Workspace::new("Development", "💻");
        let a = ApplicationEntry::new("vscode", "Visual Studio Code", "vscode");
        let mut b = ApplicationEntry::new("terminal", "Terminal", "terminal");
        b.depends_on.push(a.id);
        ws.applications = vec![a, b];
        assert!(ws.validate().is_ok());
    }

    #[test]
    fn old_documents_deserialize_with_defaults() {
        let json = r#"{"executable":"/usr/bin/code"}"#;
        let cfg: LaunchConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.arguments.is_empty());
        let opts: EntryOptions = serde_json::from_str("{}").unwrap();
        assert!(opts.restore_window_positions);
    }
}
