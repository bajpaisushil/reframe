//! Application state managed by Tauri.

use std::sync::{Arc, Mutex};

use reframe_core::ReframeService;
use reframe_core::model::PendingRestore;

pub struct AppState {
    pub service: Arc<ReframeService>,
    /// Started by the OS at login (`--autostart`).
    pub launched_at_login: bool,
    /// Startup restore offered to / started for the user, until the UI dismisses it.
    pub pending_restore: Mutex<Option<PendingRestore>>,
}

impl AppState {
    pub fn new(service: ReframeService, launched_at_login: bool) -> Self {
        Self {
            service: Arc::new(service),
            launched_at_login,
            pending_restore: Mutex::new(None),
        }
    }
}
