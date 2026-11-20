use reframe_core::model::{RestoreRequest, RestoreRun, SwitchMode, SwitchPlan};
use tauri::{AppHandle, State};
use uuid::Uuid;

use super::blocking;
use crate::error::CommandResult;
use crate::events::restore_sink;
use crate::state::AppState;

#[tauri::command]
pub async fn plan_switch(
    state: State<'_, AppState>,
    workspace_id: Uuid,
    mode: SwitchMode,
) -> CommandResult<SwitchPlan> {
    let svc = state.service.clone();
    blocking(move || svc.plan_switch(workspace_id, mode)).await
}

/// Starts a restore and returns its run id; progress arrives as `reframe://restore` events.
#[tauri::command]
pub async fn start_restore(
    app: AppHandle,
    state: State<'_, AppState>,
    request: RestoreRequest,
) -> CommandResult<Uuid> {
    let svc = state.service.clone();
    let sink = restore_sink(&app);
    blocking(move || svc.start_restore(request, sink)).await
}

#[tauri::command]
pub async fn cancel_restore(state: State<'_, AppState>, run_id: Uuid) -> CommandResult<()> {
    let svc = state.service.clone();
    blocking(move || svc.cancel_restore(run_id)).await
}

#[tauri::command]
pub async fn get_active_runs(state: State<'_, AppState>) -> CommandResult<Vec<RestoreRun>> {
    let svc = state.service.clone();
    blocking(move || Ok(svc.active_runs())).await
}

#[tauri::command]
pub async fn get_restore_run(
    state: State<'_, AppState>,
    run_id: Uuid,
) -> CommandResult<RestoreRun> {
    let svc = state.service.clone();
    blocking(move || svc.get_run(run_id)).await
}

#[tauri::command]
pub async fn list_restore_runs(
    state: State<'_, AppState>,
    workspace_id: Option<Uuid>,
    limit: u32,
) -> CommandResult<Vec<RestoreRun>> {
    let svc = state.service.clone();
    blocking(move || svc.restore_history(workspace_id, limit)).await
}
