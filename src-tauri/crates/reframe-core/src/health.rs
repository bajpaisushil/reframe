//! Workspace restore-readiness report ("Missing applications", "Supported restoration
//! features", "Estimated restoration status").

use reframe_platform::Platform;

use crate::adapters::AdapterRegistry;
use crate::model::{DesktopState, RestoreEstimate, Workspace, WorkspaceHealth};

/// PLACEHOLDER until the health implementation lands.
pub fn workspace_health(
    workspace: &Workspace,
    _state: &DesktopState,
    _platform: &dyn Platform,
    _adapters: &AdapterRegistry,
) -> WorkspaceHealth {
    WorkspaceHealth {
        workspace_id: workspace.id,
        entries: Vec::new(),
        monitors: Vec::new(),
        dependency_issues: Vec::new(),
        platform_notes: vec!["Restore readiness checks are not implemented yet.".to_string()],
        estimate: RestoreEstimate::default(),
    }
}
