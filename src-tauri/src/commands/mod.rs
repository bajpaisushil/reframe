//! Tauri command layer: thin, typed wrappers around [`reframe_core::ReframeService`].
//! Potentially slow operations run on the blocking thread pool so the UI never freezes.

pub mod desktop;
pub mod portability;
pub mod restore;
pub mod settings;
pub mod workspaces;

use reframe_core::CoreResult;

use crate::error::{CommandError, CommandResult};

/// Run a service call on the blocking pool.
pub(crate) async fn blocking<T, F>(f: F) -> CommandResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> CoreResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| CommandError::internal(format!("background task failed: {e}")))?
        .map_err(Into::into)
}
