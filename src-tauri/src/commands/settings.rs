use reframe_core::model::{ActivityEntry, AppSettings, StartupContext};
use tauri::{AppHandle, State};
use tauri_plugin_autostart::ManagerExt;
use uuid::Uuid;

use super::blocking;
use crate::error::{CommandError, CommandResult};
use crate::state::AppState;

#[tauri::command]
pub async fn get_activity(
    state: State<'_, AppState>,
    limit: u32,
) -> CommandResult<Vec<ActivityEntry>> {
    let svc = state.service.clone();
    blocking(move || svc.activity(limit)).await
}

/// Settings, with `launchAtLogin` reflecting the real OS autostart state.
#[tauri::command]
pub async fn get_settings(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<AppSettings> {
    let svc = state.service.clone();
    let mut settings = blocking(move || svc.settings()).await?;
    if let Ok(enabled) = app.autolaunch().is_enabled() {
        settings.startup.launch_at_login = enabled;
    }
    Ok(settings)
}

/// Save settings and register/unregister the OS autostart entry.
#[tauri::command]
pub async fn update_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: AppSettings,
) -> CommandResult<AppSettings> {
    let svc = state.service.clone();
    let mut saved = blocking(move || svc.update_settings(settings)).await?;
    let autolaunch = app.autolaunch();
    let currently = autolaunch.is_enabled().unwrap_or(false);
    if saved.startup.launch_at_login != currently {
        let result = if saved.startup.launch_at_login {
            autolaunch.enable()
        } else {
            autolaunch.disable()
        };
        if let Err(e) = result {
            return Err(CommandError {
                kind: "platform".into(),
                message: format!(
                    "Settings were saved, but the login item could not be changed: {e}"
                ),
                details: Vec::new(),
            });
        }
    }
    if let Ok(enabled) = autolaunch.is_enabled() {
        saved.startup.launch_at_login = enabled;
    }
    Ok(saved)
}

#[tauri::command]
pub async fn get_active_workspace(state: State<'_, AppState>) -> CommandResult<Option<Uuid>> {
    let svc = state.service.clone();
    blocking(move || svc.active_workspace()).await
}

#[tauri::command]
pub fn get_startup_context(state: State<'_, AppState>) -> StartupContext {
    StartupContext {
        launched_at_login: state.launched_at_login,
        pending_restore: state.pending_restore.lock().unwrap().clone(),
    }
}

/// The UI handled (accepted or declined) the startup restore prompt.
#[tauri::command]
pub fn dismiss_startup_restore(state: State<'_, AppState>) {
    *state.pending_restore.lock().unwrap() = None;
}
