//! The restore orchestrator behind [`super::execute`].
//!
//! # Threading model
//!
//! The calling thread is the *coordinator*: it plans, closes windows (Replace mode), arranges
//! the windows of applications that are already running (Merge/Replace), validates each entry
//! once its dependencies are up, and hands it to a scoped worker thread that launches it and
//! waits for its windows. Workers report back over a channel. Shared state — the run, the
//! window claim set, the launch-slot clock — lives behind mutexes that are only held for short
//! bookkeeping: never while sleeping, waiting, or calling the platform or the event sink.
//!
//! # Order of operations
//!
//! 1. `RunStarted` with every selected entry `Pending` (disabled ones `Skipped "Disabled"`).
//! 2. Plan the dependency graph. An invalid graph fails the run *before* anything is closed
//!    or launched.
//! 3. Replace mode: gracefully close the confirmed windows and report each one.
//! 4. Merge/Replace: launch steps that running windows already satisfy are not launched
//!    (see `restoration::reuse`): fully open entries are arranged and complete at once, partly
//!    open ones keep their open windows and launch only the rest.
//! 5. Launch the rest: dependencies first, independent entries in parallel.
//! 6. `RunFinished` — always the last event, after every worker has finished.
//!
//! # Limitations
//!
//! - "Already open" is decided from window titles for launch steps with a title hint (a
//!   project or folder name shown in the title) and per *application* for steps without one:
//!   a generic application counts as running as soon as it has any window, whatever document
//!   that window shows.
//! - New windows are attributed through their owning process (launched process tree, adapter
//!   identification, executable path). A window whose process the platform cannot report
//!   (e.g. an X11 window without `_NET_WM_PID`) is never claimed, so it is neither counted
//!   nor placed.
//! - Entries of the same application are restored one after another (not in parallel) so
//!   their windows can be told apart; a single-instance application opening windows for two
//!   entries at once could otherwise not be disambiguated.
//! - Waiting stops once the windows the launch steps expect have appeared, plus
//!   `RestoreTiming::settle` for further saved windows. Windows that take longer than that to
//!   appear (beyond the expected ones) are reported missing.
//! - Window placement is only as good as the platform's `set_window_placement`; failures are
//!   reported per entry and nothing is retried.
//! - User- and import-controlled timeouts, delays, retry counts and parallelism are capped
//!   (10 min window timeout, 60 s launch stagger, 10 min dependency delay, 10 retries, 16
//!   parallel launches) so corrupt data cannot stall a restore.

use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use chrono::Utc;
use reframe_platform::common::process::descendants;
use reframe_platform::{
    MonitorInfo, Platform, PlatformError, PlatformInfo, ProcessInfo, WindowId, WindowInfo,
    WindowPlacement,
};
use uuid::Uuid;

use super::planner::{self, ExecutionPlan, PlanIssue};
use super::{
    EventSink, RestoreEnv, RestoreOptions, RestoreTiming, layout, matcher, reuse, validate,
};
use crate::adapters::{AdapterRegistry, AppAdapter, InstanceModel, LaunchContext, LaunchStep};
use crate::discovery;
use crate::model::{
    ApplicationEntry, CloseOutcome, CloseStatus, EntryProgress, EntryStatus, RestoreEvent,
    RestoreRun, RunStatus, SwitchMode, Workspace,
};

/// Message for entries that were not started because the restore was cancelled.
pub const CANCELLED: &str = "Restore cancelled";
/// Message for entries that were launched, then cut short by a cancellation.
pub const CANCELLED_AFTER_START: &str = "Restore cancelled after the application started";
/// Message for windows that did not close during a Replace switch.
pub const STILL_OPEN: &str = "Still open - it may be asking to save changes";
/// Message for entries reused in Merge/Replace mode without arranging windows.
pub const ALREADY_RUNNING: &str = "Already running";
/// Message for entries reused in Merge/Replace mode whose windows were arranged.
pub const ALREADY_RUNNING_ARRANGED: &str = "Already running - arranged windows";

const INTERNAL_ERROR: &str = "Reframe ran into an internal error while restoring this application";

// Upper bounds for user/import-controlled values, so corrupt data cannot stall a restore.
const MAX_WINDOW_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_LAUNCH_DELAY: Duration = Duration::from_secs(60);
const MAX_DEPENDENCY_DELAY: Duration = Duration::from_secs(600);
const MAX_SETTLE: Duration = Duration::from_secs(60);
const MAX_CLOSE_WAIT: Duration = Duration::from_secs(600);
const MAX_RETRIES: u32 = 10;
const MAX_PARALLEL: usize = 16;
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(2);
const MIN_POLL: Duration = Duration::from_millis(1);
const MAX_POLL: Duration = Duration::from_secs(5);
/// Process-list refreshes attempted for a window whose owning process is unknown.
const PROCESS_LOOKUPS_PER_PID: u8 = 3;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Run a restore to completion. See [`super::execute`] for the contract.
pub(crate) fn run(
    env: &RestoreEnv,
    workspace: &Workspace,
    options: RestoreOptions,
    cancel: &AtomicBool,
    emit: &EventSink,
) -> RestoreRun {
    let pending = RestoreRun::pending(
        options.run_id,
        workspace,
        options.mode,
        options.entry_ids.as_deref(),
    );
    emit(RestoreEvent::RunStarted {
        run: pending.clone(),
    });
    let fallback = pending.clone();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let engine = Engine::new(env, workspace, &options, cancel, emit, pending);
        let forced = match catch_unwind(AssertUnwindSafe(|| engine.execute(&options))) {
            Ok(forced) => forced,
            Err(_) => {
                log::error!(
                    "restore {} panicked; finishing it as failed",
                    options.run_id
                );
                engine.add_message(
                    "Reframe ran into an internal error; the remaining applications were not restored.",
                );
                engine.abort_remaining(INTERNAL_ERROR);
                None
            }
        };
        engine.finish(forced)
    }));
    outcome.unwrap_or_else(|_| {
        // Last resort (a panic outside the guarded section): report a consistent, failed run.
        let mut run = fallback;
        let now = Utc::now();
        for entry in run.entries.iter_mut().filter(|e| !e.status.is_terminal()) {
            entry.status = EntryStatus::Failed;
            entry.message = Some(INTERNAL_ERROR.to_string());
            entry.finished_at = Some(now);
        }
        run.status = RunStatus::Failed;
        run.finished_at = Some(now);
        run.messages
            .push("Reframe ran into an internal error during the restore.".to_string());
        emit(RestoreEvent::RunFinished { run: run.clone() });
        run
    })
}

/// [`RestoreTiming`] with sane bounds.
#[derive(Debug, Clone, Copy)]
struct Timing {
    poll: Duration,
    settle: Duration,
    close_wait: Duration,
}

impl Timing {
    fn sanitized(t: &RestoreTiming) -> Self {
        Self {
            poll: t.poll_interval.clamp(MIN_POLL, MAX_POLL),
            settle: t.settle.min(MAX_SETTLE),
            close_wait: t.close_wait.min(MAX_CLOSE_WAIT),
        }
    }
}

/// Scheduling state of one planned entry (coordinator only).
#[derive(Debug, Clone, Copy)]
enum Slot {
    Waiting,
    Running,
    /// `ok`: Completed/PartiallyCompleted; dependents may start at `ready_at`.
    Done {
        ok: bool,
        ready_at: Instant,
    },
}

/// A validated entry, ready to launch.
struct Prepared {
    adapter: Arc<dyn AppAdapter>,
    /// The steps to launch.
    steps: Vec<LaunchStep>,
    /// Steps already satisfied by running windows (Merge/Replace).
    reused: Reused,
    /// Steps of a partly open entry that cannot be launched (validation errors).
    invalid: Vec<String>,
}

/// The already-open part of a partly open entry.
#[derive(Default)]
struct Reused {
    /// Running windows taken over for the entry (claimed during the reuse phase).
    windows: Vec<WindowId>,
    /// Satisfied launch steps, out of `total_steps`.
    steps: usize,
    total_steps: usize,
    /// Windows the satisfied steps account for.
    expected_windows: u32,
    /// Title hints of the satisfied steps (to match their windows to saved positions).
    hints: Vec<String>,
}

/// An entry whose launch steps are partly satisfied by running windows (Merge/Replace),
/// decided before anything is launched; only the unsatisfied steps are launched later.
struct PartialReuse {
    steps: Vec<LaunchStep>,
    satisfied: Vec<bool>,
    windows: Vec<WindowId>,
}

enum Launch {
    Started(Option<u32>),
    Failed(String),
    Cancelled,
}

#[derive(Debug, Default)]
struct Placement {
    placed: u32,
    errors: Vec<String>,
}

struct Engine<'a> {
    platform: &'a dyn Platform,
    adapters: &'a AdapterRegistry,
    workspace: &'a Workspace,
    info: PlatformInfo,
    timing: Timing,
    cancel: &'a AtomicBool,
    /// Set once cancellation actually cut the restore short.
    cancel_seen: AtomicBool,
    emit: &'a EventSink,
    run_id: Uuid,
    run: Mutex<RestoreRun>,
    progress_index: HashMap<Uuid, usize>,
    /// Windows taken by an entry; concurrent entries never take the same window.
    claimed: Mutex<HashSet<WindowId>>,
    /// Process ids returned by launches of multi-process applications, by entry.
    launched_by: Mutex<HashMap<u32, Uuid>>,
    /// Earliest time the next launch may start (`launch_delay_ms` stagger).
    next_launch: Mutex<Instant>,
    launch_delay: Duration,
    monitors: Vec<MonitorInfo>,
}

impl<'a> Engine<'a> {
    fn new(
        env: &'a RestoreEnv,
        workspace: &'a Workspace,
        options: &RestoreOptions,
        cancel: &'a AtomicBool,
        emit: &'a EventSink,
        run: RestoreRun,
    ) -> Self {
        let platform: &'a dyn Platform = &*env.platform;
        let monitors = platform.monitors().unwrap_or_else(|e| {
            log::info!("connected monitors unavailable, windows keep saved coordinates: {e}");
            Vec::new()
        });
        let mut progress_index = HashMap::new();
        for (i, entry) in run.entries.iter().enumerate() {
            progress_index.entry(entry.entry_id).or_insert(i);
        }
        Self {
            platform,
            adapters: &env.adapters,
            workspace,
            info: platform.info(),
            timing: Timing::sanitized(&env.timing),
            cancel,
            cancel_seen: AtomicBool::new(false),
            emit,
            run_id: options.run_id,
            run: Mutex::new(run),
            progress_index,
            claimed: Mutex::new(HashSet::new()),
            launched_by: Mutex::new(HashMap::new()),
            next_launch: Mutex::new(Instant::now()),
            launch_delay: Duration::from_millis(u64::from(workspace.restore.launch_delay_ms))
                .min(MAX_LAUNCH_DELAY),
            monitors,
        }
    }

    /// Everything between `RunStarted` and `RunFinished`. Returns a forced final status
    /// (invalid plan), or `None` to derive it from the entries.
    fn execute(&self, options: &RestoreOptions) -> Option<RunStatus> {
        let selected: Option<HashSet<Uuid>> = options
            .entry_ids
            .as_ref()
            .map(|ids| ids.iter().copied().collect());
        let plan = match planner::analyze(self.workspace, |e| {
            e.enabled && selected.as_ref().is_none_or(|s| s.contains(&e.id))
        }) {
            Ok(plan) => plan,
            Err(issues) => {
                self.reject_plan(&issues);
                return Some(RunStatus::Failed);
            }
        };
        if plan.order.is_empty() {
            self.add_message("Nothing to restore: no enabled application was selected.");
            return None;
        }
        if options.mode == SwitchMode::Replace
            && !options.close_window_ids.is_empty()
            && !self.is_cancelled()
        {
            self.close_windows(&options.close_window_ids);
        }
        let mut slots: HashMap<Uuid, Slot> = HashMap::new();
        let partial = if options.mode != SwitchMode::Additive && !self.is_cancelled() {
            self.reuse_running(&plan, &mut slots)
        } else {
            HashMap::new()
        };
        self.schedule(&plan, slots, partial);
        None
    }

    // ------------------------------------------------------------------ planning

    fn reject_plan(&self, issues: &[PlanIssue]) {
        for issue in issues {
            self.add_message(&issue.message);
        }
        let entries: Vec<(usize, Uuid)> = lock(&self.run)
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.status.is_terminal())
            .map(|(i, e)| (i, e.entry_id))
            .collect();
        for (index, id) in entries {
            let reason = issues
                .iter()
                .find(|issue| issue.entries.contains(&id))
                .map_or(planner::INVALID_PLAN_REASON, PlanIssue::skip_reason);
            self.update_at(index, |p| {
                p.status = EntryStatus::Skipped;
                p.message = Some(reason.to_string());
            });
        }
    }

    // ------------------------------------------------------------------ Replace: closing

    /// Gracefully close the windows the user confirmed, then wait for them to disappear.
    /// Nothing is ever force-closed; windows that stay open are reported.
    fn close_windows(&self, ids: &[WindowId]) {
        let mut unique: Vec<&WindowId> = Vec::new();
        for id in ids {
            if !unique.contains(&id) {
                unique.push(id);
            }
        }
        let windows = match self.platform.windows() {
            Ok(windows) => windows,
            Err(error) => {
                let message = sentence(&format!("could not check which windows are open: {error}"));
                for id in unique {
                    self.record_close(&CloseOutcome {
                        window_id: id.clone(),
                        title: String::new(),
                        app_name: String::new(),
                        status: CloseStatus::Failed,
                        message: Some(message.clone()),
                    });
                }
                return;
            }
        };
        let processes = self.platform.processes().unwrap_or_default();
        let self_pid = std::process::id();
        let mut waiting: Vec<CloseOutcome> = Vec::new();
        for id in unique {
            if self.is_cancelled() {
                break;
            }
            // Windows that are already gone need no request.
            let Some(window) = windows.iter().find(|w| &w.id == id) else {
                continue;
            };
            if window.pid == self_pid {
                log::warn!("not closing window {id}: it belongs to Reframe itself");
                continue;
            }
            let (_, app_name) =
                matcher::identify_window(window, &windows, &processes, self.adapters);
            let mut outcome = CloseOutcome {
                window_id: id.clone(),
                title: window.title.clone(),
                app_name,
                status: CloseStatus::Requested,
                message: None,
            };
            self.record_close(&outcome);
            match self.platform.close_window(id) {
                Ok(()) => waiting.push(outcome),
                Err(PlatformError::NotFound { .. }) => {
                    outcome.status = CloseStatus::Closed;
                    self.record_close(&outcome);
                }
                Err(error) => {
                    outcome.status = CloseStatus::Failed;
                    outcome.message = Some(sentence(&error.to_string()));
                    self.record_close(&outcome);
                }
            }
        }

        let deadline = Instant::now() + self.timing.close_wait;
        let mut list_error: Option<String> = None;
        while !waiting.is_empty() {
            match self.platform.windows() {
                Ok(current) => {
                    list_error = None;
                    let open: HashSet<&WindowId> = current.iter().map(|w| &w.id).collect();
                    waiting.retain_mut(|outcome| {
                        if open.contains(&outcome.window_id) {
                            return true;
                        }
                        outcome.status = CloseStatus::Closed;
                        self.record_close(outcome);
                        false
                    });
                }
                Err(error) => list_error = Some(error.to_string()),
            }
            let now = Instant::now();
            if waiting.is_empty()
                || now >= deadline
                || !self.sleep_until((now + self.timing.poll).min(deadline))
            {
                break;
            }
        }
        for mut outcome in waiting {
            match &list_error {
                Some(error) => {
                    outcome.status = CloseStatus::Failed;
                    outcome.message = Some(sentence(&format!(
                        "could not check whether it closed: {error}"
                    )));
                }
                None => {
                    outcome.status = CloseStatus::StillOpen;
                    outcome.message = Some(STILL_OPEN.to_string());
                }
            }
            self.record_close(&outcome);
        }
    }

    fn record_close(&self, outcome: &CloseOutcome) {
        {
            let mut run = lock(&self.run);
            match run
                .closed_windows
                .iter_mut()
                .find(|c| c.window_id == outcome.window_id)
            {
                Some(slot) => *slot = outcome.clone(),
                None => run.closed_windows.push(outcome.clone()),
            }
        }
        (self.emit)(RestoreEvent::WindowCloseUpdated {
            run_id: self.run_id,
            outcome: outcome.clone(),
        });
    }

    // ------------------------------------------------------------------ Merge/Replace: reuse

    /// Decide, before anything is launched, which launch steps running windows already
    /// satisfy (see `restoration::reuse`; the switch planner decides the same way). Entries
    /// whose steps are all satisfied are not launched: their windows are arranged according to
    /// the saved snapshots and the entry completes immediately. Partly open entries keep the
    /// windows they took over and are returned, so only their other steps are launched.
    fn reuse_running(
        &self,
        plan: &ExecutionPlan,
        slots: &mut HashMap<Uuid, Slot>,
    ) -> HashMap<Uuid, PartialReuse> {
        let mut partial = HashMap::new();
        let state = discovery::gather(self.platform);
        if state.windows.is_empty() && !self.info.capabilities.list_windows.is_available() {
            self.add_message(
                "Reframe can't see open windows on this system, so applications that are already running could not be reused and may have been started again.",
            );
            return partial;
        }
        let planned: HashSet<Uuid> = plan.order.iter().copied().collect();
        // Workspace order and one set of taken windows, exactly like the switch planner.
        let mut taken: HashSet<WindowId> = lock(&self.claimed).clone();
        for entry in self
            .workspace
            .applications
            .iter()
            .filter(|e| planned.contains(&e.id))
        {
            let steps = self.launch_steps(entry);
            let Some(decision) =
                reuse::decide_entry(entry, &steps, &state, self.adapters, &mut taken)
            else {
                continue;
            };
            if decision.satisfied_count() == 0 {
                continue;
            }
            lock(&self.claimed).extend(decision.windows.iter().cloned());
            if !decision.complete() {
                // Only possible with a launch plan of several steps.
                if let Ok(steps) = steps {
                    partial.insert(
                        entry.id,
                        PartialReuse {
                            steps,
                            satisfied: decision.satisfied,
                            windows: decision.windows,
                        },
                    );
                }
                continue;
            }
            let candidates: Vec<WindowInfo> = decision
                .windows
                .iter()
                .filter_map(|id| state.windows.iter().find(|w| &w.id == id))
                .cloned()
                .collect();
            let hints: Vec<String> = reuse::step_hints(&steps).into_iter().flatten().collect();
            let assignments =
                matcher::assign_windows_with_fallback(&entry.windows, &candidates, &hints);
            let placement = self.place_windows(entry, &assignments);
            let attempted = placement.placed + count_u32(placement.errors.len());
            let (status, message) = match placement.errors.first() {
                None if placement.placed > 0 => {
                    (EntryStatus::Completed, ALREADY_RUNNING_ARRANGED.to_string())
                }
                None => (EntryStatus::Completed, ALREADY_RUNNING.to_string()),
                Some(_) if placement.placed > 0 => (
                    EntryStatus::PartiallyCompleted,
                    format!(
                        "Already running - arranged {} of {attempted} windows",
                        placement.placed
                    ),
                ),
                Some(error) => (
                    EntryStatus::PartiallyCompleted,
                    format!("Already running - its windows could not be moved: {error}"),
                ),
            };
            let restored = if attempted > 0 {
                placement.placed
            } else {
                count_u32(assignments.len())
            }
            .min(count_u32(entry.windows.len()));
            self.finish_entry(entry.id, status, Some(message), restored);
            slots.insert(
                entry.id,
                Slot::Done {
                    ok: true,
                    ready_at: Instant::now(),
                },
            );
        }
        partial
    }

    /// The adapter's launch steps for `entry`.
    fn launch_steps(&self, entry: &ApplicationEntry) -> Result<Vec<LaunchStep>, String> {
        let ctx = LaunchContext {
            platform: self.platform,
            info: &self.info,
        };
        self.adapters
            .get(&entry.adapter_id)
            .launch_plan(&ctx, entry)
    }

    // ------------------------------------------------------------------ scheduling

    /// Launch every planned entry that is not done yet: at most `max_parallel` at a time, an
    /// entry only after its dependencies are up (plus their `delay_after_launch_ms`), entries
    /// of the same application one after another so their windows can be told apart. Partly
    /// open entries (`partial`) launch only their unsatisfied steps.
    fn schedule(
        &self,
        plan: &ExecutionPlan,
        mut slots: HashMap<Uuid, Slot>,
        mut partial: HashMap<Uuid, PartialReuse>,
    ) {
        for id in &plan.order {
            slots.entry(*id).or_insert(Slot::Waiting);
        }
        let max_parallel = usize::try_from(self.workspace.restore.max_parallel)
            .unwrap_or(MAX_PARALLEL)
            .clamp(1, MAX_PARALLEL);
        let (tx, rx) = mpsc::channel::<(Uuid, EntryStatus)>();
        std::thread::scope(|scope| {
            let mut running = 0usize;
            let mut running_apps: HashMap<String, usize> = HashMap::new();
            loop {
                let now = Instant::now();
                let cancelled = self.is_cancelled();
                let mut wake: Option<Instant> = None;
                for id in &plan.order {
                    if !matches!(slots.get(id), Some(Slot::Waiting)) {
                        continue;
                    }
                    let failed = Slot::Done {
                        ok: false,
                        ready_at: now,
                    };
                    let Some(entry) = self.workspace.entry(*id) else {
                        slots.insert(*id, failed);
                        continue;
                    };
                    if cancelled {
                        self.cancel_seen.store(true, Ordering::SeqCst);
                        self.finish_entry(*id, EntryStatus::Skipped, Some(CANCELLED.into()), 0);
                        slots.insert(*id, failed);
                        continue;
                    }
                    let deps = plan
                        .dependencies
                        .get(id)
                        .map(Vec::as_slice)
                        .unwrap_or_default();
                    if let Some(dep) = deps
                        .iter()
                        .find(|d| matches!(slots.get(*d), Some(Slot::Done { ok: false, .. })))
                    {
                        let message = format!(
                            "Skipped because {} did not start (required dependency)",
                            self.name_of(*dep)
                        );
                        self.finish_entry(*id, EntryStatus::Skipped, Some(message), 0);
                        slots.insert(*id, failed);
                        continue;
                    }
                    if let Some(dep) = deps
                        .iter()
                        .find(|d| !matches!(slots.get(*d), Some(Slot::Done { .. })))
                    {
                        self.set_waiting(*id, *dep);
                        continue;
                    }
                    let latest = deps
                        .iter()
                        .filter_map(|d| match slots.get(d) {
                            Some(Slot::Done { ready_at, .. }) => Some((*ready_at, *d)),
                            _ => None,
                        })
                        .max_by_key(|(t, _)| *t);
                    if let Some((ready_at, dep)) = latest
                        && ready_at > now
                    {
                        self.set_waiting(*id, dep);
                        wake = Some(wake.map_or(ready_at, |w| w.min(ready_at)));
                        continue;
                    }
                    let app_key = entry.app_id.to_lowercase();
                    if running >= max_parallel || running_apps.get(&app_key).is_some_and(|n| *n > 0)
                    {
                        continue;
                    }
                    let prepared = match self.prepare(entry, partial.remove(id)) {
                        Ok(prepared) => prepared,
                        Err(message) => {
                            self.finish_entry(*id, EntryStatus::Failed, Some(message), 0);
                            slots.insert(*id, failed);
                            continue;
                        }
                    };
                    let first_slot = self.reserve_launch_slot();
                    let tx = tx.clone();
                    let spawned = std::thread::Builder::new()
                        .name("reframe-restore-app".into())
                        .spawn_scoped(scope, move || {
                            let status = catch_unwind(AssertUnwindSafe(|| {
                                self.run_entry(entry, prepared, first_slot)
                            }))
                            .unwrap_or_else(|_| {
                                log::error!("restoring {} panicked", entry.display_name);
                                self.finish_entry(
                                    entry.id,
                                    EntryStatus::Failed,
                                    Some(INTERNAL_ERROR.into()),
                                    0,
                                )
                            });
                            // The coordinator outlives every worker; a send cannot fail.
                            let _ = tx.send((entry.id, status));
                        });
                    match spawned {
                        Ok(_) => {
                            slots.insert(*id, Slot::Running);
                            running += 1;
                            *running_apps.entry(app_key).or_default() += 1;
                        }
                        Err(error) => {
                            let message = format!(
                                "Could not start restoring {}: {error}",
                                entry.display_name
                            );
                            self.finish_entry(*id, EntryStatus::Failed, Some(message), 0);
                            slots.insert(*id, failed);
                        }
                    }
                }

                if running == 0 && slots.values().all(|s| matches!(s, Slot::Done { .. })) {
                    break;
                }
                let timeout = wake
                    .map_or(self.timing.poll, |w| {
                        w.saturating_duration_since(Instant::now())
                    })
                    .clamp(MIN_POLL, self.timing.poll);
                let mut finished: Vec<(Uuid, EntryStatus)> =
                    rx.recv_timeout(timeout).into_iter().collect();
                finished.extend(rx.try_iter());
                for (id, status) in finished {
                    running = running.saturating_sub(1);
                    if let Some(entry) = self.workspace.entry(id)
                        && let Some(n) = running_apps.get_mut(&entry.app_id.to_lowercase())
                    {
                        *n = n.saturating_sub(1);
                    }
                    let ok = matches!(
                        status,
                        EntryStatus::Completed | EntryStatus::PartiallyCompleted
                    );
                    let delay = if ok {
                        self.dependency_delay(id)
                    } else {
                        Duration::ZERO
                    };
                    slots.insert(
                        id,
                        Slot::Done {
                            ok,
                            ready_at: Instant::now() + delay,
                        },
                    );
                }
            }
        });
    }

    fn set_waiting(&self, id: Uuid, dependency: Uuid) {
        let message = format!("Waiting on {}", self.name_of(dependency));
        self.update(id, |p| p.message = Some(message));
    }

    fn name_of(&self, id: Uuid) -> String {
        self.workspace.entry(id).map_or_else(
            || "another application".to_string(),
            |e| e.display_name.clone(),
        )
    }

    fn dependency_delay(&self, id: Uuid) -> Duration {
        self.workspace
            .entry(id)
            .and_then(|e| e.options.delay_after_launch_ms)
            .map_or(Duration::ZERO, |ms| {
                Duration::from_millis(u64::from(ms)).min(MAX_DEPENDENCY_DELAY)
            })
    }

    /// Validating: build the launch plan and check that its programs and folders exist. A
    /// partly open entry keeps the steps decided in the reuse phase and launches only the
    /// unsatisfied ones; those that fail validation count as failed launches, so the windows
    /// that are already open are still arranged.
    fn prepare(
        &self,
        entry: &ApplicationEntry,
        partial: Option<PartialReuse>,
    ) -> Result<Prepared, String> {
        self.update(entry.id, |p| {
            p.status = EntryStatus::Validating;
            p.message = None;
        });
        let adapter = self.adapters.get(&entry.adapter_id);
        if let Some(partial) = partial {
            let mut reused = Reused {
                windows: partial.windows,
                total_steps: partial.steps.len(),
                ..Reused::default()
            };
            let mut steps = Vec::new();
            let mut invalid = Vec::new();
            for (step, satisfied) in partial.steps.into_iter().zip(partial.satisfied) {
                if satisfied {
                    reused.steps += 1;
                    reused.expected_windows = reused
                        .expected_windows
                        .saturating_add(step.expected_windows);
                    reused.hints.extend(step.title_hint);
                } else {
                    match validate::check_request(&step.request, &entry.display_name) {
                        Ok(()) => steps.push(step),
                        Err(message) => invalid.push(message),
                    }
                }
            }
            return Ok(Prepared {
                adapter,
                steps,
                reused,
                invalid,
            });
        }
        let steps = self.launch_steps(entry)?;
        if steps.is_empty() {
            return Err(format!(
                "Reframe doesn't know how to start {} - edit the application to choose its location",
                entry.display_name
            ));
        }
        for step in &steps {
            validate::check_request(&step.request, &entry.display_name)?;
        }
        Ok(Prepared {
            adapter,
            steps,
            reused: Reused::default(),
            invalid: Vec::new(),
        })
    }

    // ------------------------------------------------------------------ one entry (worker)

    fn run_entry(
        &self,
        entry: &ApplicationEntry,
        prepared: Prepared,
        first_slot: Instant,
    ) -> EntryStatus {
        let Prepared {
            adapter,
            steps,
            reused,
            invalid,
        } = prepared;
        let retries = entry
            .options
            .retry_count
            .unwrap_or(self.workspace.restore.retry_count)
            .min(MAX_RETRIES);
        let timeout = Duration::from_millis(u64::from(
            entry
                .options
                .timeout_ms
                .unwrap_or(self.workspace.restore.timeout_ms),
        ))
        .min(MAX_WINDOW_TIMEOUT);
        let step_windows = steps.iter().fold(reused.expected_windows, |sum, s| {
            sum.saturating_add(s.expected_windows)
        });
        let hints: Vec<String> = steps
            .iter()
            .filter_map(|s| s.title_hint.clone())
            .chain(reused.hints.iter().cloned())
            .collect();

        let single_instance = adapter.instance_model() == InstanceModel::SingleInstance;
        let mut tracker = Tracker::new(self, entry, single_instance);
        // Windows that were open already (partly open entry) are this entry's from the start.
        tracker.adopt(&reused.windows);
        // Why windows cannot be tracked for this entry, if they cannot.
        let mut untracked: Option<String> = (!self.info.capabilities.list_windows.is_available())
            .then(|| {
                self.info
                    .capabilities
                    .list_windows
                    .note
                    .clone()
                    .unwrap_or_else(|| {
                        "this system doesn't let Reframe see application windows".to_string()
                    })
            });
        let mut tracking = untracked.is_none();
        // Windows that must reappear for a full restore; 0 when the launch may legitimately
        // open none or when this system cannot report windows.
        let mut expected = if tracking && step_windows > 0 {
            step_windows.max(count_u32(entry.windows.len()))
        } else {
            0
        };
        self.update(entry.id, |p| p.windows_expected = expected);

        let mut launched = 0usize;
        // Windows the successfully launched steps should open (what is worth waiting for).
        let mut launched_windows = 0u32;
        // Steps of a partly open entry that failed validation count as failed launches.
        let attempted_steps = steps.len() + invalid.len();
        let mut failures: Vec<String> = invalid;
        let mut attempt = 0u32;
        for (index, step) in steps.iter().enumerate() {
            let slot = if index == 0 {
                first_slot
            } else {
                self.reserve_launch_slot()
            };
            if !self.sleep_until(slot) {
                return self.cancelled_entry(entry.id, launched > 0);
            }
            // Windows that exist right before the first launch are never this entry's.
            if index == 0
                && tracking
                && let Err(error) = tracker.take_baseline()
            {
                untracked = Some(sentence(&error));
                tracking = false;
                expected = 0;
                self.update(entry.id, |p| p.windows_expected = 0);
            }
            match self.launch_step(entry, step, retries, single_instance, &mut attempt) {
                Launch::Started(pid) => {
                    launched += 1;
                    launched_windows = launched_windows.saturating_add(step.expected_windows);
                    if let Some(pid) = pid.filter(|_| !single_instance) {
                        tracker.add_launched_pid(pid);
                    }
                }
                Launch::Failed(message) => failures.push(message),
                Launch::Cancelled => return self.cancelled_entry(entry.id, launched > 0),
            }
        }
        if launched == 0 && reused.steps == 0 {
            let message = failures
                .into_iter()
                .next()
                .unwrap_or_else(|| format!("{} could not be started", entry.display_name));
            return self.finish_entry(entry.id, EntryStatus::Failed, Some(message), 0);
        }
        self.update(entry.id, |p| {
            p.status = EntryStatus::Restoring;
            p.message = None;
        });

        let mut problems: Vec<String> = Vec::new();
        let mut restored = 0u32;
        if tracking {
            if launched > 0 {
                if !tracker.wait_for_windows(
                    launched_windows,
                    entry.windows.len(),
                    timeout,
                    expected,
                ) {
                    return self.cancelled_entry(entry.id, true);
                }
            } else {
                // Nothing was launched: only the windows that were open already are arranged.
                tracker.observe();
            }
            let candidates = tracker.claimed_windows();
            let found = count_u32(candidates.len());
            let found_new = count_u32(
                candidates
                    .iter()
                    .filter(|w| !tracker.adopted.contains(&w.id))
                    .count(),
            );
            let assignments =
                matcher::assign_windows_with_fallback(&entry.windows, &candidates, &hints);
            let placement = self.place_windows(entry, &assignments);
            let failed_moves = count_u32(placement.errors.len());
            restored = found.saturating_sub(failed_moves).min(expected);
            if launched_windows > 0 && found_new == 0 {
                problems.push(format!(
                    "Started, but no window appeared within {}",
                    format_duration(timeout)
                ));
            } else if found < expected {
                problems.push(format!("{found} of {expected} windows reopened"));
            }
            if let Some(error) = placement.errors.first() {
                problems.push(format!(
                    "{} could not be moved: {error}",
                    plural(failed_moves, "window", "windows")
                ));
            }
        } else {
            let reason = untracked.unwrap_or_default();
            self.add_message(&format!(
                "Reframe could not wait for windows or arrange them: {reason}"
            ));
        }
        if let Some(error) = failures.first() {
            problems.push(format!(
                "{} of {attempted_steps} launches failed: {error}",
                failures.len()
            ));
        }
        let total = reused.total_steps;
        let noun = reuse::step_noun(&entry.state, total);
        let verb = if reused.steps == 1 { "was" } else { "were" };
        if problems.is_empty() {
            let mut notes: Vec<String> = Vec::new();
            if reused.steps > 0 {
                notes.push(format!(
                    "Opened {launched} of {total} {noun}; {} {verb} already open",
                    reused.steps
                ));
            }
            if !tracking {
                notes.push("Started - Reframe could not see its windows".to_string());
            }
            let message = (!notes.is_empty()).then(|| notes.join("; "));
            self.finish_entry(entry.id, EntryStatus::Completed, message, restored)
        } else {
            if reused.steps > 0 {
                problems.push(format!(
                    "{} of {total} {noun} {verb} already open",
                    reused.steps
                ));
            }
            self.finish_entry(
                entry.id,
                EntryStatus::PartiallyCompleted,
                Some(problems.join("; ")),
                restored,
            )
        }
    }

    /// Launch one step, retrying failures up to `retries` more times with a short backoff.
    fn launch_step(
        &self,
        entry: &ApplicationEntry,
        step: &LaunchStep,
        retries: u32,
        single_instance: bool,
        attempt: &mut u32,
    ) -> Launch {
        let attempts = retries.saturating_add(1);
        let mut last_error: Option<PlatformError> = None;
        let mut tries = 0u32;
        for n in 1..=attempts {
            if n > 1 {
                if !self.sleep_until(Instant::now() + self.retry_backoff(n - 1)) {
                    return Launch::Cancelled;
                }
                let slot = self.reserve_launch_slot();
                if !self.sleep_until(slot) {
                    return Launch::Cancelled;
                }
            }
            if self.is_cancelled() {
                return Launch::Cancelled;
            }
            *attempt = (*attempt).max(n);
            let shown = *attempt;
            let note = last_error
                .as_ref()
                .map(|e| format!("Retrying after an error: {}", sentence(&e.to_string())));
            self.update(entry.id, |p| {
                p.status = EntryStatus::Launching;
                p.attempt = shown;
                p.message = note;
            });
            tries = n;
            match self.platform.launch(&step.request) {
                Ok(process) => {
                    if let Some(pid) = process.pid.filter(|_| !single_instance) {
                        lock(&self.launched_by).insert(pid, entry.id);
                    }
                    return Launch::Started(process.pid);
                }
                Err(error) => {
                    log::warn!(
                        "launching {} failed (attempt {n} of {attempts}): {error}",
                        entry.display_name
                    );
                    let retryable = !matches!(
                        error,
                        PlatformError::Unsupported { .. } | PlatformError::PermissionDenied { .. }
                    );
                    last_error = Some(error);
                    if !retryable {
                        break;
                    }
                }
            }
        }
        let message = last_error.map_or_else(
            || format!("{} could not be started", entry.display_name),
            |e| sentence(&e.to_string()),
        );
        Launch::Failed(if tries > 1 {
            format!("{message} (tried {tries} times)")
        } else {
            message
        })
    }

    fn retry_backoff(&self, failures: u32) -> Duration {
        self.timing
            .poll
            .saturating_mul(2)
            .saturating_mul(failures.max(1))
            .min(MAX_RETRY_BACKOFF)
    }

    fn cancelled_entry(&self, id: Uuid, launched: bool) -> EntryStatus {
        self.cancel_seen.store(true, Ordering::SeqCst);
        if launched {
            self.finish_entry(
                id,
                EntryStatus::PartiallyCompleted,
                Some(CANCELLED_AFTER_START.into()),
                0,
            )
        } else {
            self.finish_entry(id, EntryStatus::Skipped, Some(CANCELLED.into()), 0)
        }
    }

    // ------------------------------------------------------------------ windows

    fn positions_enabled(&self, entry: &ApplicationEntry) -> bool {
        self.workspace.restore.restore_window_positions && entry.options.restore_window_positions
    }

    /// Move each assigned window to its saved (monitor-remapped) position, when the user wants
    /// positions restored and the platform can move windows.
    fn place_windows(
        &self,
        entry: &ApplicationEntry,
        assignments: &[(usize, WindowId)],
    ) -> Placement {
        let mut result = Placement::default();
        if assignments.is_empty() || !self.positions_enabled(entry) {
            return result;
        }
        if !self.info.capabilities.move_windows.is_available() {
            let detail = self
                .info
                .capabilities
                .move_windows
                .note
                .clone()
                .unwrap_or_else(|| "this system doesn't let Reframe move windows".to_string());
            self.add_message(&format!("Window positions were not restored: {detail}"));
            return result;
        }
        for (index, window) in assignments {
            let Some(snapshot) = entry.windows.get(*index) else {
                continue;
            };
            let placement = WindowPlacement {
                bounds: layout::remap_bounds(snapshot, &self.workspace.monitors, &self.monitors),
                state: snapshot.state,
            };
            match self.platform.set_window_placement(window, &placement) {
                Ok(()) => result.placed += 1,
                Err(error) => {
                    log::warn!(
                        "placing window {window} of {} failed: {error}",
                        entry.display_name
                    );
                    result.errors.push(sentence(&error.to_string()));
                }
            }
        }
        result
    }

    fn claim(&self, window: &WindowId) -> bool {
        lock(&self.claimed).insert(window.clone())
    }

    fn launched_by(&self, pid: u32) -> Option<Uuid> {
        lock(&self.launched_by).get(&pid).copied()
    }

    // ------------------------------------------------------------------ timing helpers

    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// Sleep until `deadline` in poll-sized slices; false as soon as the restore is cancelled.
    fn sleep_until(&self, deadline: Instant) -> bool {
        loop {
            if self.is_cancelled() {
                return false;
            }
            let now = Instant::now();
            if now >= deadline {
                return true;
            }
            std::thread::sleep((deadline - now).min(self.timing.poll));
        }
    }

    /// Reserve the next launch start time, `launch_delay` after the previous one.
    fn reserve_launch_slot(&self) -> Instant {
        let mut next = lock(&self.next_launch);
        let slot = (*next).max(Instant::now());
        *next = slot + self.launch_delay;
        slot
    }

    // ------------------------------------------------------------------ progress & events

    /// Apply `change` to an entry's progress and emit `EntryUpdated` if anything changed.
    /// Statuses only move forward and terminal entries never change. Returns the entry's
    /// status afterwards (`None` for an unknown entry).
    fn update(&self, id: Uuid, change: impl FnOnce(&mut EntryProgress)) -> Option<EntryStatus> {
        let index = *self.progress_index.get(&id)?;
        self.update_at(index, change)
    }

    fn update_at(
        &self,
        index: usize,
        change: impl FnOnce(&mut EntryProgress),
    ) -> Option<EntryStatus> {
        let (status, event) = {
            let mut run = lock(&self.run);
            let entry = run.entries.get_mut(index)?;
            if entry.status.is_terminal() {
                return Some(entry.status);
            }
            let before = entry.clone();
            change(entry);
            if stage(entry.status) < stage(before.status) {
                entry.status = before.status;
            }
            if entry.status != before.status {
                let now = Utc::now();
                if entry.started_at.is_none()
                    && !matches!(entry.status, EntryStatus::Pending | EntryStatus::Skipped)
                {
                    entry.started_at = Some(now);
                }
                if entry.status.is_terminal() {
                    entry.finished_at = Some(now);
                }
            }
            if *entry == before {
                return Some(entry.status);
            }
            let event = RestoreEvent::EntryUpdated {
                run_id: self.run_id,
                entry: entry.clone(),
            };
            (entry.status, event)
        };
        (self.emit)(event);
        Some(status)
    }

    /// Move an entry to a final status; returns the status it actually ended with (an entry
    /// that already finished keeps its status).
    fn finish_entry(
        &self,
        id: Uuid,
        status: EntryStatus,
        message: Option<String>,
        restored: u32,
    ) -> EntryStatus {
        self.update(id, |p| {
            p.status = status;
            p.message = message;
            p.windows_restored = restored;
        })
        .unwrap_or(status)
    }

    fn add_message(&self, message: &str) {
        let mut run = lock(&self.run);
        if !run.messages.iter().any(|m| m == message) {
            run.messages.push(message.to_string());
        }
    }

    /// Fail every entry that has not finished (after an internal error).
    fn abort_remaining(&self, reason: &str) {
        let count = lock(&self.run).entries.len();
        for index in 0..count {
            self.update_at(index, |p| {
                p.status = EntryStatus::Failed;
                p.message = Some(reason.to_string());
            });
        }
    }

    /// Settle the final status, emit `RunFinished` and return the run.
    fn finish(&self, forced: Option<RunStatus>) -> RestoreRun {
        let cancelled = self.cancel_seen.load(Ordering::SeqCst);
        // Safety net: no entry may stay in progress once the run is over.
        let count = lock(&self.run).entries.len();
        for index in 0..count {
            self.update_at(index, |p| {
                p.status = EntryStatus::Skipped;
                p.message = Some(if cancelled { CANCELLED } else { "Not restored" }.to_string());
            });
        }
        let run = {
            let mut run = lock(&self.run);
            run.status = match forced {
                Some(status) => status,
                None if cancelled => RunStatus::Cancelled,
                None => run.summarize(),
            };
            run.finished_at = Some(Utc::now());
            run.clone()
        };
        (self.emit)(RestoreEvent::RunFinished { run: run.clone() });
        run
    }
}

/// Watches the window list for the windows of one launched entry and claims them.
struct Tracker<'e, 'a> {
    engine: &'e Engine<'a>,
    entry: &'e ApplicationEntry,
    single_instance: bool,
    /// Windows that existed before the first launch.
    baseline: HashSet<WindowId>,
    launched: HashSet<u32>,
    /// Launched processes and their descendants.
    family: HashSet<u32>,
    processes: HashMap<u32, ProcessInfo>,
    lookups: HashMap<u32, u8>,
    /// Per process: identified as this entry's application.
    verdicts: HashMap<u32, bool>,
    /// Claimed windows, in the order they appeared (taken-over windows first).
    mine: Vec<WindowId>,
    /// Windows that were open already and were taken over for this entry.
    adopted: HashSet<WindowId>,
    latest: Vec<WindowInfo>,
}

impl<'e, 'a> Tracker<'e, 'a> {
    fn new(engine: &'e Engine<'a>, entry: &'e ApplicationEntry, single_instance: bool) -> Self {
        Self {
            engine,
            entry,
            single_instance,
            baseline: HashSet::new(),
            launched: HashSet::new(),
            family: HashSet::new(),
            processes: HashMap::new(),
            lookups: HashMap::new(),
            verdicts: HashMap::new(),
            mine: Vec::new(),
            adopted: HashSet::new(),
            latest: Vec::new(),
        }
    }

    /// Record the windows that exist before launching; the error when they cannot be listed.
    fn take_baseline(&mut self) -> Result<(), String> {
        match self.engine.platform.windows() {
            Ok(windows) => {
                self.baseline = windows.iter().map(|w| w.id.clone()).collect();
                Ok(())
            }
            Err(error) => {
                log::warn!(
                    "cannot list windows, {} will not be tracked: {error}",
                    self.entry.display_name
                );
                Err(error.to_string())
            }
        }
    }

    fn add_launched_pid(&mut self, pid: u32) {
        self.launched.insert(pid);
        self.family.insert(pid);
    }

    /// Count windows taken over before launching (already claimed for this entry) as its own.
    fn adopt(&mut self, windows: &[WindowId]) {
        for id in windows {
            if self.adopted.insert(id.clone()) {
                self.mine.push(id.clone());
            }
        }
    }

    /// Refresh the window list without claiming new windows (nothing was launched).
    fn observe(&mut self) {
        match self.engine.platform.windows() {
            Ok(windows) => {
                let present: HashSet<&WindowId> = windows.iter().map(|w| &w.id).collect();
                self.mine.retain(|id| present.contains(id));
                self.latest = windows;
            }
            Err(error) => log::debug!("listing windows failed: {error}"),
        }
    }

    /// Wait until `wait_for` new windows appeared (windows taken over do not count) — then up
    /// to `settle` longer while saved windows are still unmatched — or until `timeout`.
    /// Returns false when cancelled.
    fn wait_for_windows(
        &mut self,
        wait_for: u32,
        snapshots: usize,
        timeout: Duration,
        expected: u32,
    ) -> bool {
        let wait_for = usize::try_from(wait_for).unwrap_or(usize::MAX);
        let deadline = Instant::now() + timeout;
        let mut reached_at: Option<Instant> = None;
        let mut reported = 0usize;
        loop {
            if self.engine.is_cancelled() {
                return false;
            }
            self.poll();
            let found = self.mine.len();
            if found != reported {
                reported = found;
                let shown = count_u32(found).min(expected);
                self.engine
                    .update(self.entry.id, |p| p.windows_restored = shown);
            }
            let found_new = self
                .mine
                .iter()
                .filter(|id| !self.adopted.contains(*id))
                .count();
            let now = Instant::now();
            let mut wake = (now + self.engine.timing.poll).min(deadline);
            if found_new >= wait_for {
                if found >= snapshots {
                    return true;
                }
                let settle_end = *reached_at.get_or_insert(now) + self.engine.timing.settle;
                if now >= settle_end {
                    return true;
                }
                wake = wake.min(settle_end);
            }
            if now >= deadline {
                return true;
            }
            if !self.engine.sleep_until(wake) {
                return false;
            }
        }
    }

    /// One look at the window list: forget claimed windows that closed, claim new ones that
    /// belong to this application.
    fn poll(&mut self) {
        let windows = match self.engine.platform.windows() {
            Ok(windows) => windows,
            Err(error) => {
                log::debug!("listing windows failed while waiting: {error}");
                return;
            }
        };
        {
            let present: HashSet<&WindowId> = windows.iter().map(|w| &w.id).collect();
            self.mine.retain(|id| present.contains(id));
        }
        // Platforms list windows front-to-back and new windows open on top: walk backwards
        // so windows are claimed in the order they appeared.
        let new: Vec<usize> = (0..windows.len())
            .rev()
            .filter(|&i| {
                !self.baseline.contains(&windows[i].id) && !self.mine.contains(&windows[i].id)
            })
            .collect();
        if new.iter().any(|&i| self.needs_lookup(windows[i].pid)) {
            self.refresh_processes();
        }
        for i in new {
            if self.belongs(&windows[i], &windows) && self.engine.claim(&windows[i].id) {
                self.mine.push(windows[i].id.clone());
            }
        }
        self.latest = windows;
    }

    fn needs_lookup(&self, pid: u32) -> bool {
        !self.family.contains(&pid)
            && !self.verdicts.contains_key(&pid)
            && !self.processes.contains_key(&pid)
            && self.lookups.get(&pid).copied().unwrap_or(0) < PROCESS_LOOKUPS_PER_PID
    }

    fn refresh_processes(&mut self) {
        match self.engine.platform.processes() {
            Ok(list) => {
                for root in &self.launched {
                    self.family.extend(descendants(&list, *root));
                }
                self.processes = list.into_iter().map(|p| (p.pid, p)).collect();
            }
            Err(error) => log::debug!("listing processes failed while waiting: {error}"),
        }
    }

    /// Whether a new window belongs to this entry: owned by a launched process (or one of its
    /// children), or by a process the adapters identify as this application. For
    /// multi-process applications a window of a process launched by *another* entry is never
    /// taken; single-instance applications may open windows in an already-running process.
    fn belongs(&mut self, window: &WindowInfo, windows: &[WindowInfo]) -> bool {
        if self.family.contains(&window.pid) {
            return true;
        }
        if !self.identified(window.pid, windows) {
            return false;
        }
        self.single_instance
            || self
                .engine
                .launched_by(window.pid)
                .is_none_or(|owner| owner == self.entry.id)
    }

    fn identified(&mut self, pid: u32, windows: &[WindowInfo]) -> bool {
        if let Some(&verdict) = self.verdicts.get(&pid) {
            return verdict;
        }
        match self.processes.get(&pid) {
            Some(process) => {
                let owned: Vec<&WindowInfo> = windows.iter().filter(|w| w.pid == pid).collect();
                let verdict = matcher::process_belongs_to_entry(
                    self.entry,
                    process,
                    &owned,
                    self.engine.adapters,
                );
                self.verdicts.insert(pid, verdict);
                verdict
            }
            None => {
                let lookups = self.lookups.entry(pid).or_insert(0);
                *lookups = lookups.saturating_add(1);
                if *lookups >= PROCESS_LOOKUPS_PER_PID {
                    self.verdicts.insert(pid, false);
                }
                false
            }
        }
    }

    /// Claimed windows that still exist, in the order they appeared.
    fn claimed_windows(&self) -> Vec<WindowInfo> {
        self.mine
            .iter()
            .filter_map(|id| self.latest.iter().find(|w| &w.id == id))
            .cloned()
            .collect()
    }
}

/// Position in the forward-only lifecycle.
fn stage(status: EntryStatus) -> u8 {
    match status {
        EntryStatus::Pending => 0,
        EntryStatus::Validating => 1,
        EntryStatus::Launching => 2,
        EntryStatus::Restoring => 3,
        EntryStatus::Completed
        | EntryStatus::PartiallyCompleted
        | EntryStatus::Failed
        | EntryStatus::Skipped => 4,
    }
}

fn count_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// `20 s`, `1.5 s`, `0.2 s`.
fn format_duration(d: Duration) -> String {
    let tenths = (d.as_millis() + 50) / 100;
    if tenths.is_multiple_of(10) {
        format!("{} s", tenths / 10)
    } else {
        format!("{}.{} s", tenths / 10, tenths % 10)
    }
}

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Capitalize the first letter of a platform message for display.
fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_are_human_readable() {
        assert_eq!(format_duration(Duration::from_secs(20)), "20 s");
        assert_eq!(format_duration(Duration::from_millis(1500)), "1.5 s");
        assert_eq!(format_duration(Duration::from_millis(200)), "0.2 s");
        assert_eq!(format_duration(Duration::ZERO), "0 s");
    }

    #[test]
    fn messages_are_capitalized() {
        assert_eq!(
            sentence("could not launch x: boom"),
            "Could not launch x: boom"
        );
        assert_eq!(sentence(""), "");
        assert_eq!(plural(1, "window", "windows"), "1 window");
        assert_eq!(plural(3, "window", "windows"), "3 windows");
    }

    #[test]
    fn lifecycle_stages_only_increase() {
        let order = [
            EntryStatus::Pending,
            EntryStatus::Validating,
            EntryStatus::Launching,
            EntryStatus::Restoring,
            EntryStatus::Completed,
        ];
        assert!(order.windows(2).all(|w| stage(w[0]) < stage(w[1])));
        assert_eq!(stage(EntryStatus::Skipped), stage(EntryStatus::Failed));
    }
}
