//! Workspace switching (Replace / Merge / Additive) planning.

use reframe_platform::PlatformInfo;

use crate::adapters::AdapterRegistry;
use crate::model::{DesktopState, SwitchMode, SwitchPlan, Workspace};

/// Compute what switching to `target` would do. Pure: nothing is launched or closed.
/// - Additive: every enabled entry is launched.
/// - Merge: entries satisfied by running windows are reused; the rest launch.
/// - Replace: like Merge, plus close candidates = running windows of applications not in
///   `target`; `default_selected` only for apps that belonged to `previous`. System windows
///   and Reframe itself are never candidates.
///
/// PLACEHOLDER until the switching implementation lands.
pub fn plan_switch(
    _state: &DesktopState,
    _adapters: &AdapterRegistry,
    _info: &PlatformInfo,
    previous: Option<&Workspace>,
    target: &Workspace,
    mode: SwitchMode,
) -> SwitchPlan {
    SwitchPlan {
        workspace_id: target.id,
        mode,
        previous_workspace_id: previous.map(|p| p.id),
        launch: Vec::new(),
        reuse: Vec::new(),
        close: Vec::new(),
        warnings: vec!["Switch planning is not implemented yet.".to_string()],
    }
}
