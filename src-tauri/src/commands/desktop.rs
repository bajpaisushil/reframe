use reframe_core::model::{DesktopSnapshot, IconLookup};
use reframe_platform::{PermissionKind, PermissionStatus, PlatformInfo};
use tauri::State;

use super::blocking;
use crate::error::CommandResult;
use crate::state::AppState;

#[tauri::command]
pub async fn get_platform_info(state: State<'_, AppState>) -> CommandResult<PlatformInfo> {
    let svc = state.service.clone();
    blocking(move || Ok(svc.platform_info())).await
}

#[tauri::command]
pub async fn get_permissions(state: State<'_, AppState>) -> CommandResult<Vec<PermissionStatus>> {
    let svc = state.service.clone();
    blocking(move || Ok(svc.permissions())).await
}

#[tauri::command]
pub async fn request_permission(
    state: State<'_, AppState>,
    kind: PermissionKind,
) -> CommandResult<()> {
    let svc = state.service.clone();
    blocking(move || svc.request_permission(kind)).await
}

#[tauri::command]
pub async fn scan_desktop(state: State<'_, AppState>) -> CommandResult<DesktopSnapshot> {
    let svc = state.service.clone();
    blocking(move || svc.scan_desktop()).await
}

#[tauri::command]
pub async fn get_app_icon(
    state: State<'_, AppState>,
    lookup: IconLookup,
) -> CommandResult<Option<String>> {
    let svc = state.service.clone();
    blocking(move || svc.app_icon(&lookup)).await
}
