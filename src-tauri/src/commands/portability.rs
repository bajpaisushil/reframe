use std::path::PathBuf;

use reframe_core::model::{ImportPreview, ImportResolution, Workspace};
use tauri::{AppHandle, State};
use uuid::Uuid;

use super::blocking;
use crate::error::CommandResult;
use crate::state::AppState;

/// Write the workspace export JSON to `path` (chosen by the user in a save dialog).
#[tauri::command]
pub async fn export_workspace(
    state: State<'_, AppState>,
    id: Uuid,
    path: PathBuf,
) -> CommandResult<()> {
    let svc = state.service.clone();
    blocking(move || svc.export_workspace(id, &path)).await
}

#[tauri::command]
pub async fn preview_import(
    state: State<'_, AppState>,
    path: PathBuf,
) -> CommandResult<ImportPreview> {
    let svc = state.service.clone();
    blocking(move || svc.preview_import(&path)).await
}

#[tauri::command]
pub async fn commit_import(
    app: AppHandle,
    state: State<'_, AppState>,
    resolution: ImportResolution,
) -> CommandResult<Workspace> {
    let svc = state.service.clone();
    let result = blocking(move || svc.commit_import(resolution)).await;
    crate::tray::refresh(&app);
    result
}
