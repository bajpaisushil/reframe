//! Startup restoration: when the OS starts Reframe at login (autostart entry created by
//! `tauri-plugin-autostart` with [`AUTOSTART_FLAG`]), offer or start the configured restore.

use std::time::Duration;

use reframe_core::model::{PendingRestore, RestoreRequest, SwitchMode};
use tauri::{AppHandle, Manager};

use crate::events::restore_sink;
use crate::state::AppState;

/// Argument the autostart entry passes to Reframe.
pub const AUTOSTART_FLAG: &str = "--autostart";

/// Give the desktop session a moment to settle (taskbar, network drives) before launching.
const LOGIN_SETTLE_DELAY: Duration = Duration::from_secs(4);

pub fn on_launch(app: &AppHandle) {
    let state = app.state::<AppState>();
    if !state.launched_at_login {
        return;
    }
    let service = state.service.clone();
    let target = match service.startup_restore_target() {
        Ok(Some(ws)) => ws,
        Ok(None) => return,
        Err(e) => {
            log::warn!("startup restore unavailable: {e}");
            return;
        }
    };
    let ask = service
        .settings()
        .map(|s| s.startup.ask_before_restoring)
        .unwrap_or(true);
    *state.pending_restore.lock().unwrap() = Some(PendingRestore {
        workspace_id: target.id,
        workspace_name: target.name.clone(),
        workspace_icon: target.icon.clone(),
        ask,
        run_id: None,
    });
    if ask {
        return; // the UI shows the prompt
    }

    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(LOGIN_SETTLE_DELAY);
        // Merge never closes anything, which is the only safe choice for an unattended restore.
        let request = RestoreRequest {
            workspace_id: target.id,
            mode: SwitchMode::Merge,
            entry_ids: None,
            close_window_ids: Vec::new(),
        };
        match service.start_restore(request, restore_sink(&app)) {
            Ok(run_id) => {
                let state = app.state::<AppState>();
                if let Some(pending) = state.pending_restore.lock().unwrap().as_mut() {
                    pending.run_id = Some(run_id);
                }
            }
            Err(e) => log::warn!("automatic startup restore failed to start: {e}"),
        }
    });
}
