//! Recent activity feed.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ActivityKind {
    Created,
    Captured,
    Updated,
    Deleted,
    Restored,
    RestorePartial,
    RestoreFailed,
    Imported,
    Exported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ActivityEntry {
    pub id: Uuid,
    pub kind: ActivityKind,
    pub workspace_id: Option<Uuid>,
    pub workspace_name: String,
    pub workspace_icon: String,
    pub message: String,
    pub created_at: DateTime<Utc>,
}

impl ActivityEntry {
    pub fn new(
        kind: ActivityKind,
        workspace: Option<(&Uuid, &str, &str)>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind,
            workspace_id: workspace.map(|w| *w.0),
            workspace_name: workspace.map(|w| w.1.to_string()).unwrap_or_default(),
            workspace_icon: workspace.map(|w| w.2.to_string()).unwrap_or_default(),
            message: message.into(),
            created_at: Utc::now(),
        }
    }
}
