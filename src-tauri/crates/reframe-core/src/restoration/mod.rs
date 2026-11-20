//! Restoration engine: dependency planning, orchestrated launches, window matching and
//! placement, monitor remapping, progress reporting.

pub mod layout;
pub mod matcher;
pub mod orchestrator;
pub mod planner;
pub(crate) mod reuse;
pub(crate) mod validate;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use reframe_platform::{Platform, WindowId};
use uuid::Uuid;

use crate::adapters::AdapterRegistry;
use crate::model::{RestoreEvent, RestoreRun, SwitchMode, Workspace};

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
///
/// 1. Emit `RunStarted` first (every selected entry `Pending`, disabled ones
///    `Skipped "Disabled"`) and `RunFinished` last, after every worker thread has finished.
///    `EntryUpdated` is emitted on every status, attempt, window-count or message change;
///    statuses only move forward and a finished entry never changes again.
/// 2. Plan the dependency DAG ([`planner`]). An invalid graph fails the run with the planner's
///    messages and every planned entry is `Skipped` with a reason — *before* anything is
///    closed or launched. Dependencies on disabled or unselected entries count as satisfied.
/// 3. Replace mode: gracefully close the `close_window_ids` that still exist (never kill, never
///    Reframe's own windows), wait up to `timing.close_wait`, and report each window
///    (`WindowCloseUpdated`: Requested → Closed / StillOpen / Failed). Then restore as Merge.
/// 4. Merge/Replace: launch steps already satisfied by running windows of the entry's
///    application are not launched again: a step with a title hint (project, folder) when a
///    window shows it, a step without one when the application has any window. An entry whose
///    steps are all satisfied completes at once ("Already running - arranged windows") after
///    its windows are arranged per the saved snapshots (when positions are enabled); a partly
///    open entry launches only the rest ("Opened 1 of 2 projects; 1 was already open").
///    Additive: everything launches.
/// 5. Validate each entry when it becomes ready (adapter launch plan, programs and working
///    folders exist), then launch it on a worker thread: at most `max_parallel` entries at a
///    time, launch starts staggered by `launch_delay_ms`, an entry only after all its planned
///    dependencies completed (fully or partially) plus their `delay_after_launch_ms`, and
///    entries of the same application one after another. Failed launch steps are retried up
///    to `retry_count` times with a short backoff. Dependents of failed or skipped entries are
///    `Skipped`; unrelated entries continue.
/// 6. New windows are claimed through a shared claim set (concurrent entries never take the
///    same window) when they belong to the entry's application (launched process tree,
///    adapter identification, executable). The worker waits until the windows the launch
///    steps expect appeared — then up to `timing.settle` longer while saved windows are still
///    unmatched — or until the entry's timeout, matches them to the saved `WindowSnapshot`s
///    and applies their monitor-remapped placement. Missing windows or placement errors make
///    the entry `PartiallyCompleted` with a precise message; launch failures make it `Failed`.
/// 7. `cancel` is checked between steps and while waiting: entries not launched yet are
///    `Skipped "Restore cancelled"`, the run ends `Cancelled`. Otherwise the final status is
///    [`RestoreRun::summarize`].
///
/// Never panics (internal errors fail the remaining entries), never holds a lock while
/// sleeping and never blocks forever: every wait is bounded by a timeout, and user-provided
/// timeouts and delays are capped (see [`orchestrator`]).
pub fn execute(
    env: &RestoreEnv,
    workspace: &Workspace,
    options: RestoreOptions,
    cancel: &AtomicBool,
    emit: &EventSink,
) -> RestoreRun {
    orchestrator::run(env, workspace, options, cancel, emit)
}
