//! Restore runs, per-application progress and the events streamed to the UI.

use chrono::{DateTime, Utc};
use reframe_platform::WindowId;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use super::switching::SwitchMode;

/// Lifecycle of one application during a restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum EntryStatus {
    Pending,
    /// Checking executable, paths and dependencies.
    Validating,
    Launching,
    /// Waiting for windows / applying window placement and adapter state.
    Restoring,
    Completed,
    /// Launched, but some windows or state could not be restored.
    PartiallyCompleted,
    Failed,
    /// Not attempted (disabled, already running, dependency failed, cancelled).
    Skipped,
}

impl EntryStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::PartiallyCompleted | Self::Failed | Self::Skipped
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EntryProgress {
    pub entry_id: Uuid,
    pub display_name: String,
    pub app_id: String,
    pub status: EntryStatus,
    /// Human-readable explanation of the current status (always set for Failed/Skipped/Partial).
    pub message: Option<String>,
    /// 1-based launch attempt number (0 before the first attempt).
    pub attempt: u32,
    pub windows_expected: u32,
    pub windows_restored: u32,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RunStatus {
    Running,
    Completed,
    PartiallyCompleted,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CloseStatus {
    /// Close request sent, waiting for the application.
    Requested,
    Closed,
    /// Still open after the wait (usually a "save changes?" prompt). Never force-closed.
    StillOpen,
    Failed,
}

/// A window Reframe asked to close during a Replace switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CloseOutcome {
    pub window_id: WindowId,
    pub title: String,
    pub app_name: String,
    pub status: CloseStatus,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RestoreRun {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub workspace_name: String,
    pub mode: SwitchMode,
    pub status: RunStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub entries: Vec<EntryProgress>,
    pub closed_windows: Vec<CloseOutcome>,
    /// Run-level problems (invalid dependency graph, platform limitations...).
    pub messages: Vec<String>,
}

impl RestoreRun {
    /// A run that has not started yet: every planned entry is Pending.
    pub fn pending(
        id: Uuid,
        workspace: &super::workspace::Workspace,
        mode: SwitchMode,
        entry_ids: Option<&[Uuid]>,
    ) -> Self {
        let entries = workspace
            .applications
            .iter()
            .filter(|e| entry_ids.is_none_or(|ids| ids.contains(&e.id)))
            .map(|e| EntryProgress {
                entry_id: e.id,
                display_name: e.display_name.clone(),
                app_id: e.app_id.clone(),
                status: if e.enabled {
                    EntryStatus::Pending
                } else {
                    EntryStatus::Skipped
                },
                message: (!e.enabled).then(|| "Disabled".to_string()),
                attempt: 0,
                windows_expected: e.windows.len() as u32,
                windows_restored: 0,
                started_at: None,
                finished_at: None,
            })
            .collect();
        Self {
            id,
            workspace_id: workspace.id,
            workspace_name: workspace.name.clone(),
            mode,
            status: RunStatus::Running,
            started_at: Utc::now(),
            finished_at: None,
            entries,
            closed_windows: Vec::new(),
            messages: Vec::new(),
        }
    }

    /// Entries that can be retried (failed, partial, or skipped because of a failure).
    pub fn retryable_entries(&self) -> Vec<Uuid> {
        self.entries
            .iter()
            .filter(|e| {
                matches!(
                    e.status,
                    EntryStatus::Failed | EntryStatus::PartiallyCompleted
                ) || (e.status == EntryStatus::Skipped
                    && e.message.as_deref().is_some_and(|m| {
                        m.starts_with("Waiting on")
                            || m.contains("dependency")
                            || m.contains("cancelled")
                    }))
            })
            .map(|e| e.entry_id)
            .collect()
    }

    /// Overall status derived from entry statuses.
    pub fn summarize(&self) -> RunStatus {
        let relevant: Vec<_> = self
            .entries
            .iter()
            .filter(|e| {
                e.status != EntryStatus::Skipped || e.message.as_deref() != Some("Disabled")
            })
            .collect();
        if relevant
            .iter()
            .all(|e| matches!(e.status, EntryStatus::Completed | EntryStatus::Skipped))
            && (relevant.iter().any(|e| e.status == EntryStatus::Completed) || relevant.is_empty())
        {
            return RunStatus::Completed;
        }
        if relevant.iter().any(|e| {
            matches!(
                e.status,
                EntryStatus::Completed | EntryStatus::PartiallyCompleted
            )
        }) {
            RunStatus::PartiallyCompleted
        } else {
            RunStatus::Failed
        }
    }
}

/// Events emitted while a restore runs (Tauri event `reframe://restore`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum RestoreEvent {
    RunStarted { run: RestoreRun },
    EntryUpdated { run_id: Uuid, entry: EntryProgress },
    WindowCloseUpdated { run_id: Uuid, outcome: CloseOutcome },
    RunFinished { run: RestoreRun },
}

/// Start a restore / switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RestoreRequest {
    pub workspace_id: Uuid,
    pub mode: SwitchMode,
    /// Restore only these entries (e.g. "retry failed"); `None` = every enabled entry.
    pub entry_ids: Option<Vec<Uuid>>,
    /// Windows the user confirmed may be closed (Replace mode). Never inferred.
    pub close_window_ids: Vec<WindowId>,
}
