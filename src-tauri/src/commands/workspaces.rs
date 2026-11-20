use reframe_core::model::{
    ApplicationEntry, CaptureRequest, CaptureResult, CaptureSelection, CapturedWorkspace,
    NewWorkspace, Workspace, WorkspaceHealth, WorkspaceSummary,
};
use reframe_platform::LaunchTarget;
use tauri::State;
use uuid::Uuid;

use super::blocking;
use crate::error::CommandResult;
use crate::state::AppState;

#[tauri::command]
pub async fn list_workspaces(state: State<'_, AppState>) -> CommandResult<Vec<WorkspaceSummary>> {
    let svc = state.service.clone();
    blocking(move || svc.list_workspaces()).await
}

#[tauri::command]
pub async fn get_workspace(state: State<'_, AppState>, id: Uuid) -> CommandResult<Workspace> {
    let svc = state.service.clone();
    blocking(move || svc.get_workspace(id)).await
}

#[tauri::command]
pub async fn create_workspace(
    state: State<'_, AppState>,
    input: NewWorkspace,
) -> CommandResult<Workspace> {
    let svc = state.service.clone();
    blocking(move || svc.create_workspace(input)).await
}

#[tauri::command]
pub async fn capture_workspace(
    state: State<'_, AppState>,
    request: CaptureRequest,
) -> CommandResult<CapturedWorkspace> {
    let svc = state.service.clone();
    blocking(move || svc.capture_workspace(request)).await
}

#[tauri::command]
pub async fn recapture_workspace(
    state: State<'_, AppState>,
    id: Uuid,
    selections: Vec<CaptureSelection>,
) -> CommandResult<CapturedWorkspace> {
    let svc = state.service.clone();
    blocking(move || svc.recapture_workspace(id, &selections)).await
}

#[tauri::command]
pub async fn capture_applications(
    state: State<'_, AppState>,
    selections: Vec<CaptureSelection>,
) -> CommandResult<CaptureResult> {
    let svc = state.service.clone();
    blocking(move || svc.capture_applications(&selections)).await
}

#[tauri::command]
pub async fn create_manual_entry(
    state: State<'_, AppState>,
    target: LaunchTarget,
) -> CommandResult<ApplicationEntry> {
    let svc = state.service.clone();
    blocking(move || svc.create_manual_entry(target)).await
}

#[tauri::command]
pub async fn update_workspace(
    state: State<'_, AppState>,
    workspace: Workspace,
) -> CommandResult<Workspace> {
    let svc = state.service.clone();
    blocking(move || svc.update_workspace(workspace)).await
}

#[tauri::command]
pub async fn delete_workspace(state: State<'_, AppState>, id: Uuid) -> CommandResult<()> {
    let svc = state.service.clone();
    blocking(move || svc.delete_workspace(id)).await
}

#[tauri::command]
pub async fn duplicate_workspace(state: State<'_, AppState>, id: Uuid) -> CommandResult<Workspace> {
    let svc = state.service.clone();
    blocking(move || svc.duplicate_workspace(id)).await
}

#[tauri::command]
pub async fn get_workspace_health(
    state: State<'_, AppState>,
    id: Uuid,
) -> CommandResult<WorkspaceHealth> {
    let svc = state.service.clone();
    blocking(move || svc.workspace_health(id)).await
}
