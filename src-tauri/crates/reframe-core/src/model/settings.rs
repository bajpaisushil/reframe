//! User settings and startup behaviour.

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use super::switching::SwitchMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum StartupAction {
    #[default]
    DoNothing,
    /// Restore `StartupSettings::workspace_id`.
    RestoreWorkspace,
    /// Restore whichever workspace was active last.
    RestoreLastActive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct StartupSettings {
    /// Start Reframe when the user logs in (native OS autostart entry).
    pub launch_at_login: bool,
    pub action: StartupAction,
    pub workspace_id: Option<Uuid>,
    /// Show a confirmation (with countdown) instead of restoring silently.
    pub ask_before_restoring: bool,
}

impl Default for StartupSettings {
    fn default() -> Self {
        Self {
            launch_at_login: false,
            action: StartupAction::DoNothing,
            workspace_id: None,
            ask_before_restoring: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct AppSettings {
    pub theme: ThemePreference,
    pub startup: StartupSettings,
    /// App ids or executable names never suggested for capture (case-insensitive).
    pub excluded_apps: Vec<String>,
    /// Read browser session files to capture open tabs (optional browser integration).
    pub browser_tab_capture: bool,
    pub default_switch_mode: SwitchMode,
    /// Seconds to wait for windows to close during a Replace switch before reporting them.
    pub close_wait_seconds: u32,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            startup: StartupSettings::default(),
            excluded_apps: Vec::new(),
            browser_tab_capture: true,
            default_switch_mode: SwitchMode::Merge,
            close_wait_seconds: 15,
        }
    }
}

impl AppSettings {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut problems = Vec::new();
        if self.startup.action == StartupAction::RestoreWorkspace
            && self.startup.workspace_id.is_none()
        {
            problems.push("Choose which workspace to restore at startup.".to_string());
        }
        if !(1..=120).contains(&self.close_wait_seconds) {
            problems.push("Close wait must be between 1 and 120 seconds.".to_string());
        }
        if self.excluded_apps.len() > 500 {
            problems.push("Too many excluded applications.".to_string());
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems)
        }
    }
}

/// Information the UI needs right after launch to handle startup restoration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StartupContext {
    /// Reframe was started by the OS at login (autostart).
    pub launched_at_login: bool,
    /// A workspace restore that should be offered/started now.
    pub pending_restore: Option<PendingRestore>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PendingRestore {
    pub workspace_id: Uuid,
    pub workspace_name: String,
    pub workspace_icon: String,
    /// When true the UI asks first; otherwise the restore has already been started.
    pub ask: bool,
    pub run_id: Option<Uuid>,
}
