//! Restoration engine: dependency planning, orchestrated launches, window matching and
//! placement, monitor remapping, progress reporting.

pub mod layout;
pub mod matcher;
pub mod planner;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use chrono::Utc;
use reframe_platform::{Platform, WindowId};
use uuid::Uuid;

use crate::adapters::AdapterRegistry;
use crate::model::{RestoreEvent, RestoreRun, RunStatus, SwitchMode, Workspace};

/// Timing knobs (shortened in tests).
#[derive(Debug, Clone)]
pub struct RestoreTiming {
    /// How often to poll the window list while waiting for launched applications.
    pub poll_interval: Duration,
    /// Extra time to wait for additional windows after the expected ones appeared.
    pub settle: Duration,
    /// How long to wait for windows to close in Replace mode before reporting them.
    pub close_wait: Duration,
}

impl Default for RestoreTiming {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(250),
            settle: Duration::from_millis(1500),
            close_wait: Duration::from_secs(15),
        }
    }
}

pub struct RestoreEnv {
    pub platform: Arc<dyn Platform>,
    pub adapters: Arc<AdapterRegistry>,
    pub timing: RestoreTiming,
}

#[derive(Debug, Clone)]
pub struct RestoreOptions {
    pub run_id: Uuid,
    pub mode: SwitchMode,
    /// Restore only these entries; `None` = every enabled entry.
    pub entry_ids: Option<Vec<Uuid>>,
    /// Windows the user confirmed may be closed (Replace mode only). Never inferred.
    pub close_window_ids: Vec<WindowId>,
}

/// Receives progress events. Must be cheap; called from worker threads.
pub type EventSink = dyn Fn(RestoreEvent) + Send + Sync;

/// Run a restore to completion (blocking). Contract:
/// 1. Replace mode: gracefully close `close_window_ids` (never kill), wait up to
///    `timing.close_wait`, report windows that stayed open (`WindowCloseUpdated`).
/// 2. Plan the dependency DAG (`planner`); invalid graphs fail the run with messages.
/// 3. Merge/Replace: entries already satisfied by running windows are not relaunched; their
///    windows are repositioned. Additive: everything launches.
/// 4. Launch independent entries in parallel (≤ `max_parallel`, staggered by
///    `launch_delay_ms`), dependents after their dependencies finished; retries + timeouts
///    per entry; dependents of failed entries are Skipped; unrelated entries continue.
/// 5. Match new windows to saved `WindowSnapshot`s and apply placement (monitor-remapped).
/// 6. Emit `EntryUpdated` on every status change, honour `cancel`, return the final run.
///
/// PLACEHOLDER until the restoration engine lands.
pub fn execute(
    _env: &RestoreEnv,
    workspace: &Workspace,
    options: RestoreOptions,
    _cancel: &AtomicBool,
    emit: &EventSink,
) -> RestoreRun {
    let run = RestoreRun {
        id: options.run_id,
        workspace_id: workspace.id,
        workspace_name: workspace.name.clone(),
        mode: options.mode,
        status: RunStatus::Failed,
        started_at: Utc::now(),
        finished_at: Some(Utc::now()),
        entries: Vec::new(),
        closed_windows: Vec::new(),
        messages: vec!["The restoration engine is not implemented yet.".to_string()],
    };
    emit(RestoreEvent::RunFinished { run: run.clone() });
    run
}
