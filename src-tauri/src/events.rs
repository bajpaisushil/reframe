//! Bridges engine progress events to the webview.

use std::sync::Arc;

use reframe_core::model::RestoreEvent;
use reframe_core::restoration::EventSink;
use tauri::{AppHandle, Emitter};

/// Tauri event carrying [`RestoreEvent`] payloads.
pub const RESTORE_EVENT: &str = "reframe://restore";

pub fn restore_sink(app: &AppHandle) -> Arc<EventSink> {
    let app = app.clone();
    Arc::new(move |event: RestoreEvent| {
        if let Err(e) = app.emit(RESTORE_EVENT, &event) {
            log::warn!("could not emit restore event: {e}");
        }
    })
}
