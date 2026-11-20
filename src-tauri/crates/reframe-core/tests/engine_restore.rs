//! Integration tests for the restoration engine (`restoration::execute`) against the mock
//! platform: scheduling, dependencies, retries, timeouts, window matching and placement,
//! Merge/Replace/Additive behaviour, cancellation and event-stream invariants.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reframe_core::adapters::{
    AdapterRegistry, AppAdapter, AppIdentity, CaptureContext, CaptureInput, CaptureOutput,
    InstanceModel, LaunchContext, LaunchStep,
};
use reframe_core::model::{
    AdapterCapability, AppState, ApplicationEntry, CloseStatus, EntryProgress, EntryStatus,
    ProjectRef, RestoreEvent, RestoreRun, RunStatus, StateDetail, SwitchMode, SwitchPlan,
    WindowSnapshot, Workspace,
};
use reframe_core::restoration::orchestrator::{
    ALREADY_RUNNING, ALREADY_RUNNING_ARRANGED, CANCELLED, CANCELLED_AFTER_START, STILL_OPEN,
};
use reframe_core::restoration::{RestoreEnv, RestoreOptions, RestoreTiming, execute};
use reframe_platform::mock::{LaunchRule, MockPlatform, MockWindowSpec};
use reframe_platform::{
    Capability, IconRequest, LaunchRequest, LaunchTarget, LaunchedProcess, LocateQuery,
    MonitorInfo, PermissionKind, PermissionStatus, Platform, PlatformError, PlatformInfo,
    PlatformResult, ProcessInfo, Rect, WindowId, WindowInfo, WindowPlacement, WindowState,
};
use uuid::Uuid;

// ---------------------------------------------------------------------------- fixtures

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn timing() -> RestoreTiming {
    RestoreTiming {
        poll_interval: ms(5),
        settle: ms(60),
        close_wait: ms(300),
    }
}

/// Test adapter: identifies processes by name, launches project folders through the OS
/// "open" handler (one step per project, titled after the folder) or the executable.
struct TestAdapter {
    id: &'static str,
    process_name: &'static str,
    app_id: &'static str,
    single_instance: bool,
    expected_windows: u32,
}

const EDITOR: TestAdapter = TestAdapter {
    id: "editor-test",
    process_name: "editor",
    app_id: "editor",
    single_instance: true,
    expected_windows: 1,
};

const DAEMON: TestAdapter = TestAdapter {
    id: "daemon-test",
    process_name: "daemon",
    app_id: "daemon",
    single_instance: false,
    expected_windows: 0,
};

impl AppAdapter for TestAdapter {
    fn id(&self) -> &'static str {
        self.id
    }
    fn name(&self) -> &'static str {
        "Test adapter"
    }
    fn identify(&self, process: &ProcessInfo, _: &[&WindowInfo]) -> Option<AppIdentity> {
        (process.name == self.process_name).then(|| AppIdentity {
            app_id: self.app_id.to_string(),
            display_name: self.process_name.to_string(),
        })
    }
    fn capability(&self, _: &PlatformInfo) -> AdapterCapability {
        AdapterCapability::launch_only()
    }
    fn capture(&self, _: &CaptureContext<'_>, _: &CaptureInput<'_>) -> CaptureOutput {
        CaptureOutput::default()
    }
    fn launch_plan(
        &self,
        _: &LaunchContext<'_>,
        entry: &ApplicationEntry,
    ) -> Result<Vec<LaunchStep>, String> {
        let exe =
            entry.launch.executable.clone().ok_or_else(|| {
                format!("No executable is configured for {}.", entry.display_name)
            })?;
        match &entry.state {
            AppState::Projects { projects } if !projects.is_empty() => Ok(projects
                .iter()
                .map(|p| LaunchStep {
                    request: LaunchRequest {
                        target: LaunchTarget::Open {
                            target: p.path.display().to_string(),
                        },
                        args: Vec::new(),
                        working_directory: None,
                        env: Vec::new(),
                    },
                    expected_windows: self.expected_windows,
                    title_hint: p.path.file_name().map(|n| n.to_string_lossy().into_owned()),
                })
                .collect()),
            _ => Ok(vec![LaunchStep {
                request: LaunchRequest::executable(exe),
                expected_windows: self.expected_windows,
                title_hint: None,
            }]),
        }
    }
    fn instance_model(&self) -> InstanceModel {
        if self.single_instance {
            InstanceModel::SingleInstance
        } else {
            InstanceModel::MultiProcess
        }
    }
    fn describe(&self, _: &AppState) -> Vec<StateDetail> {
        Vec::new()
    }
    fn locate_query(&self, _: &str) -> LocateQuery {
        LocateQuery::default()
    }
}

/// Wraps the mock platform to inject failures it cannot simulate itself.
#[derive(Clone, Default)]
struct Faulty {
    inner: MockPlatform,
    fail_close: bool,
    /// The platform reports that it cannot list windows (and listing fails).
    fail_windows: bool,
    /// Listing windows fails although the platform claims it can.
    break_windows: bool,
    fail_moves: bool,
}

impl Platform for Faulty {
    fn info(&self) -> PlatformInfo {
        let mut info = self.inner.info();
        if self.fail_windows {
            info.capabilities.list_windows = Capability::unsupported("No window access here.");
        }
        info
    }
    fn monitors(&self) -> PlatformResult<Vec<MonitorInfo>> {
        self.inner.monitors()
    }
    fn windows(&self) -> PlatformResult<Vec<WindowInfo>> {
        if self.fail_windows {
            return Err(PlatformError::unsupported(
                "Listing windows",
                "no window access",
            ));
        }
        if self.break_windows {
            return Err(PlatformError::os(
                "Listing windows",
                "the display connection was lost",
            ));
        }
        self.inner.windows()
    }
    fn processes(&self) -> PlatformResult<Vec<ProcessInfo>> {
        self.inner.processes()
    }
    fn launch(&self, request: &LaunchRequest) -> PlatformResult<LaunchedProcess> {
        self.inner.launch(request)
    }
    fn set_window_placement(&self, w: &WindowId, p: &WindowPlacement) -> PlatformResult<()> {
        if self.fail_moves {
            return Err(PlatformError::os("Moving the window", "denied by the test"));
        }
        self.inner.set_window_placement(w, p)
    }
    fn close_window(&self, w: &WindowId) -> PlatformResult<()> {
        if self.fail_close {
            return Err(PlatformError::unsupported(
                "Closing windows",
                "not in this test",
            ));
        }
        self.inner.close_window(w)
    }
    fn focus_window(&self, w: &WindowId) -> PlatformResult<()> {
        self.inner.focus_window(w)
    }
    fn application_icon(&self, r: &IconRequest) -> PlatformResult<Option<Vec<u8>>> {
        self.inner.application_icon(r)
    }
    fn locate_application(&self, q: &LocateQuery) -> Vec<LaunchTarget> {
        self.inner.locate_application(q)
    }
    fn permissions(&self) -> Vec<PermissionStatus> {
        self.inner.permissions()
    }
    fn request_permission(&self, k: PermissionKind) -> PlatformResult<()> {
        self.inner.request_permission(k)
    }
}

struct Harness {
    platform: MockPlatform,
    dir: tempfile::TempDir,
}

struct Outcome {
    run: RestoreRun,
    events: Vec<(Instant, RestoreEvent)>,
    elapsed: Duration,
}

impl Harness {
    fn new() -> Self {
        Self {
            platform: MockPlatform::new(),
            dir: tempfile::tempdir().unwrap(),
        }
    }

    /// A fake installed executable (a real file, so existence checks pass).
    fn exe(&self, name: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        if !path.exists() {
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
        }
        path
    }

    /// A generic-adapter entry for the fake executable `name`.
    fn app(&self, name: &str) -> ApplicationEntry {
        let mut entry = ApplicationEntry::new(format!("exe:{name}"), name, "generic");
        entry.launch.executable = Some(self.exe(name));
        entry
    }

    /// Launch rule for `name` opening one window per title.
    fn rule(&self, name: &str, titles: &[&str]) -> LaunchRule {
        titles.iter().fold(
            LaunchRule::new(name).executable(self.exe(name)),
            |rule, title| rule.window(MockWindowSpec::titled(*title)),
        )
    }

    /// Start `name` as an already-running application with one window.
    fn running(&self, name: &str, title: &str, bounds: Rect) -> WindowId {
        let pid = self.platform.add_process(name, Some(self.exe(name)));
        self.platform.add_window(pid, title, None, bounds)
    }

    fn run(&self, ws: &Workspace, mode: SwitchMode) -> Outcome {
        self.run_with(
            Arc::new(self.platform.clone()),
            ws,
            options(mode),
            Arc::new(AtomicBool::new(false)),
            |_, _| {},
        )
    }

    fn run_with(
        &self,
        platform: Arc<dyn Platform>,
        ws: &Workspace,
        options: RestoreOptions,
        cancel: Arc<AtomicBool>,
        hook: impl Fn(&RestoreEvent, &AtomicBool) + Send + Sync + 'static,
    ) -> Outcome {
        let env = RestoreEnv {
            platform,
            adapters: Arc::new(adapters()),
            timing: timing(),
        };
        let events: Arc<Mutex<Vec<(Instant, RestoreEvent)>>> = Arc::default();
        let sink = {
            let events = Arc::clone(&events);
            let cancel = Arc::clone(&cancel);
            move |event: RestoreEvent| {
                hook(&event, &cancel);
                events.lock().unwrap().push((Instant::now(), event));
            }
        };
        let started = Instant::now();
        let run = execute(&env, ws, options, &cancel, &sink);
        let elapsed = started.elapsed();
        let events = std::mem::take(&mut *events.lock().unwrap());
        check_event_stream(&events, &run);
        Outcome {
            run,
            events,
            elapsed,
        }
    }

    fn window_titled(&self, title: &str) -> WindowInfo {
        self.platform
            .windows()
            .unwrap()
            .into_iter()
            .find(|w| w.title == title)
            .unwrap_or_else(|| panic!("no window titled {title:?}"))
    }
}

impl Outcome {
    fn entry(&self, id: Uuid) -> &EntryProgress {
        self.run
            .entries
            .iter()
            .find(|e| e.entry_id == id)
            .expect("entry in run")
    }

    fn updates(&self, id: Uuid) -> Vec<(Instant, &EntryProgress)> {
        self.events
            .iter()
            .filter_map(|(t, e)| match e {
                RestoreEvent::EntryUpdated { entry, .. } if entry.entry_id == id => {
                    Some((*t, entry))
                }
                _ => None,
            })
            .collect()
    }

    /// When `id` first reached `status`.
    fn reached(&self, id: Uuid, status: EntryStatus) -> Instant {
        self.updates(id)
            .into_iter()
            .find(|(_, e)| e.status == status)
            .map(|(t, _)| t)
            .unwrap_or_else(|| panic!("entry never reached {status:?}"))
    }

    fn finished_at(&self, id: Uuid) -> Instant {
        self.updates(id)
            .into_iter()
            .find(|(_, e)| e.status.is_terminal())
            .map(|(t, _)| t)
            .expect("entry finished")
    }

    fn run_started(&self) -> Instant {
        self.events[0].0
    }
}

fn adapters() -> AdapterRegistry {
    AdapterRegistry::with_adapters(vec![Arc::new(EDITOR), Arc::new(DAEMON)])
}

/// What the switch planner proposes for the harness desktop as it is now.
fn switch_plan(h: &Harness, ws: &Workspace, mode: SwitchMode) -> SwitchPlan {
    let state = reframe_core::discovery::gather(&h.platform);
    reframe_core::switching::plan_switch(&state, &adapters(), &state.info, None, ws, mode)
}

/// Whether the health report says the entry would be reused without launching anything.
fn health_says_running(h: &Harness, ws: &Workspace, id: Uuid) -> bool {
    let state = reframe_core::discovery::gather(&h.platform);
    let health = reframe_core::health::workspace_health(ws, &state, &h.platform, &adapters());
    health
        .entries
        .iter()
        .find(|e| e.entry_id == id)
        .expect("entry health")
        .running
}

fn options(mode: SwitchMode) -> RestoreOptions {
    RestoreOptions {
        run_id: Uuid::new_v4(),
        mode,
        entry_ids: None,
        close_window_ids: Vec::new(),
    }
}

fn workspace(entries: Vec<ApplicationEntry>) -> Workspace {
    let mut ws = Workspace::new("Test", "🧪");
    ws.restore.launch_delay_ms = 0;
    ws.restore.timeout_ms = 2_000;
    ws.restore.retry_count = 0;
    ws.restore.max_parallel = 3;
    ws.applications = entries;
    ws
}

fn snapshot(title: &str, bounds: Rect, monitor: Option<&str>) -> WindowSnapshot {
    WindowSnapshot {
        title: title.to_string(),
        class_name: None,
        bounds,
        state: WindowState::Normal,
        monitor_id: monitor.map(str::to_string),
    }
}

fn stage(status: EntryStatus) -> u8 {
    match status {
        EntryStatus::Pending => 0,
        EntryStatus::Validating => 1,
        EntryStatus::Launching => 2,
        EntryStatus::Restoring => 3,
        _ => 4,
    }
}

/// Invariants every restore must satisfy.
fn check_event_stream(events: &[(Instant, RestoreEvent)], run: &RestoreRun) {
    let RestoreEvent::RunStarted { run: started } = &events.first().expect("events").1 else {
        panic!("the first event must be RunStarted");
    };
    let Some((_, RestoreEvent::RunFinished { run: finished })) = events.last() else {
        panic!("the last event must be RunFinished");
    };
    assert_eq!(finished, run, "RunFinished carries the returned run");
    let count = |f: fn(&RestoreEvent) -> bool| events.iter().filter(|(_, e)| f(e)).count();
    assert_eq!(count(|e| matches!(e, RestoreEvent::RunStarted { .. })), 1);
    assert_eq!(count(|e| matches!(e, RestoreEvent::RunFinished { .. })), 1);
    assert_eq!(started.id, run.id);
    assert_eq!(started.status, RunStatus::Running);
    for entry in &started.entries {
        assert!(
            entry.status == EntryStatus::Pending
                || (entry.status == EntryStatus::Skipped
                    && entry.message.as_deref() == Some("Disabled")),
            "RunStarted entries are pending: {entry:?}"
        );
    }
    let mut last: HashMap<Uuid, EntryProgress> = started
        .entries
        .iter()
        .map(|e| (e.entry_id, e.clone()))
        .collect();
    for (_, event) in events {
        match event {
            RestoreEvent::EntryUpdated { run_id, entry } => {
                assert_eq!(*run_id, run.id);
                let previous = last.get(&entry.entry_id).expect("update for a known entry");
                assert!(
                    !previous.status.is_terminal(),
                    "finished entries never change"
                );
                assert!(
                    stage(entry.status) >= stage(previous.status),
                    "status went backwards: {:?} -> {:?}",
                    previous.status,
                    entry.status
                );
                assert_ne!(previous, entry, "EntryUpdated without a change");
                assert!(entry.attempt >= previous.attempt, "attempts only increase");
                last.insert(entry.entry_id, entry.clone());
            }
            RestoreEvent::WindowCloseUpdated { run_id, .. } => assert_eq!(*run_id, run.id),
            _ => {}
        }
    }
    assert_ne!(run.status, RunStatus::Running);
    assert!(run.finished_at.is_some());
    assert_eq!(run.entries.len(), started.entries.len());
    for entry in &run.entries {
        assert!(entry.status.is_terminal(), "unfinished entry {entry:?}");
        assert_eq!(&last[&entry.entry_id], entry, "events and final run agree");
        if matches!(
            entry.status,
            EntryStatus::Failed | EntryStatus::Skipped | EntryStatus::PartiallyCompleted
        ) {
            assert!(entry.message.is_some(), "{entry:?} needs an explanation");
        }
        if entry.status != EntryStatus::Skipped {
            assert!(entry.started_at.is_some() && entry.finished_at.is_some());
        }
    }
}

// ---------------------------------------------------------------------------- scheduling

#[test]
fn independent_apps_launch_in_parallel() {
    let h = Harness::new();
    h.platform
        .add_rule(h.rule("app-alpha", &["Alpha"]).delay(ms(400)));
    h.platform
        .add_rule(h.rule("app-beta", &["Beta"]).delay(ms(400)));
    let a = h.app("app-alpha");
    let b = h.app("app-beta");
    let out = h.run(&workspace(vec![a.clone(), b.clone()]), SwitchMode::Merge);

    assert_eq!(out.run.status, RunStatus::Completed);
    let (a_start, a_end) = (
        out.reached(a.id, EntryStatus::Launching),
        out.finished_at(a.id),
    );
    let (b_start, b_end) = (
        out.reached(b.id, EntryStatus::Launching),
        out.finished_at(b.id),
    );
    assert!(
        a_start < b_end && b_start < a_end,
        "independent applications should restore at the same time"
    );
    for id in [a.id, b.id] {
        let e = out.entry(id);
        assert_eq!(e.status, EntryStatus::Completed);
        assert_eq!(
            (e.windows_restored, e.windows_expected, e.attempt),
            (1, 1, 1)
        );
        assert_eq!(e.message, None);
    }
}

#[test]
fn max_parallel_limits_concurrency() {
    let h = Harness::new();
    h.platform
        .add_rule(h.rule("app-alpha", &["Alpha"]).delay(ms(80)));
    h.platform
        .add_rule(h.rule("app-beta", &["Beta"]).delay(ms(80)));
    let a = h.app("app-alpha");
    let b = h.app("app-beta");
    let mut ws = workspace(vec![a.clone(), b.clone()]);
    ws.restore.max_parallel = 1;
    let out = h.run(&ws, SwitchMode::Merge);
    assert_eq!(out.run.status, RunStatus::Completed);
    assert!(out.reached(b.id, EntryStatus::Validating) >= out.finished_at(a.id));
}

#[test]
fn dependencies_start_after_their_dependency_and_its_delay() {
    let h = Harness::new();
    h.platform
        .add_rule(h.rule("app-db", &["Database"]).delay(ms(50)));
    h.platform.add_rule(h.rule("app-api", &["API"]));
    let mut db = h.app("app-db");
    db.options.delay_after_launch_ms = Some(150);
    let mut api = h.app("app-api");
    api.depends_on = vec![db.id];
    // The dependent is listed first: ordering comes from the dependency, not the list.
    let out = h.run(&workspace(vec![api.clone(), db.clone()]), SwitchMode::Merge);

    assert_eq!(out.run.status, RunStatus::Completed);
    assert!(out.reached(api.id, EntryStatus::Validating) >= out.finished_at(db.id) + ms(150));
    let launched: Vec<PathBuf> = h
        .platform
        .launches()
        .into_iter()
        .map(|l| match l.target {
            LaunchTarget::Executable { path } => path,
            other => panic!("unexpected target {other:?}"),
        })
        .collect();
    assert_eq!(launched, vec![h.exe("app-db"), h.exe("app-api")]);
    assert!(
        out.updates(api.id)
            .iter()
            .any(|(_, e)| e.status == EntryStatus::Pending
                && e.message.as_deref() == Some("Waiting on app-db")),
        "a waiting dependent explains what it waits for"
    );
}

#[test]
fn launch_starts_are_staggered_in_workspace_order() {
    let h = Harness::new();
    let names = ["app-one", "app-two", "app-three"];
    for name in names {
        h.platform.add_rule(h.rule(name, &[name]));
    }
    let entries: Vec<ApplicationEntry> = names.iter().map(|n| h.app(n)).collect();
    let mut ws = workspace(entries.clone());
    ws.restore.launch_delay_ms = 100;
    let out = h.run(&ws, SwitchMode::Merge);

    assert_eq!(out.run.status, RunStatus::Completed);
    let start = out.run_started();
    for (i, entry) in entries.iter().enumerate() {
        let launching = out.reached(entry.id, EntryStatus::Launching);
        assert!(
            launching >= start + ms(100) * i as u32,
            "launch {i} started too early"
        );
    }
    let order: Vec<String> = h
        .platform
        .launches()
        .iter()
        .map(|l| l.target.label())
        .collect();
    let expected: Vec<String> = names
        .iter()
        .map(|n| h.exe(n).display().to_string())
        .collect();
    assert_eq!(order, expected);
}

// ---------------------------------------------------------------------------- dependencies

#[test]
fn invalid_dependency_graph_fails_before_anything_happens() {
    let h = Harness::new();
    let open = h.running("app-chat", "Chat", Rect::new(0, 0, 300, 300));
    let mut a = h.app("A");
    let mut b = h.app("B");
    let c = h.app("C");
    a.depends_on.push(b.id);
    b.depends_on.push(a.id);
    let mut opts = options(SwitchMode::Replace);
    opts.close_window_ids = vec![open.clone()];
    let out = h.run_with(
        Arc::new(h.platform.clone()),
        &workspace(vec![a.clone(), b.clone(), c.clone()]),
        opts,
        Arc::new(AtomicBool::new(false)),
        |_, _| {},
    );

    assert_eq!(out.run.status, RunStatus::Failed);
    assert_eq!(
        out.run.messages,
        vec!["Circular dependency: A -> B -> A".to_string()]
    );
    for id in [a.id, b.id] {
        assert_eq!(out.entry(id).status, EntryStatus::Skipped);
        assert_eq!(
            out.entry(id).message.as_deref(),
            Some("Skipped because of a circular dependency")
        );
    }
    assert_eq!(out.entry(c.id).status, EntryStatus::Skipped);
    assert!(
        out.entry(c.id)
            .message
            .as_deref()
            .unwrap()
            .contains("dependencies are invalid")
    );
    assert!(h.platform.launches().is_empty());
    assert!(
        h.platform.close_requests().is_empty(),
        "nothing is closed for an invalid plan"
    );
    assert!(h.platform.window(&open).is_some());
}

#[test]
fn failed_dependency_skips_dependents_but_not_unrelated_apps() {
    let h = Harness::new();
    h.platform.add_rule(h.rule("app-web", &["Web"]));
    let mut gone = ApplicationEntry::new("exe:app-gone", "app-gone", "generic");
    let missing = h.dir.path().join("app-gone");
    gone.launch.executable = Some(missing.clone());
    let mut dependent = h.app("app-client");
    dependent.depends_on = vec![gone.id];
    let unrelated = h.app("app-web");
    let out = h.run(
        &workspace(vec![gone.clone(), dependent.clone(), unrelated.clone()]),
        SwitchMode::Merge,
    );

    assert_eq!(out.run.status, RunStatus::PartiallyCompleted);
    let failed = out.entry(gone.id);
    assert_eq!(failed.status, EntryStatus::Failed);
    assert_eq!(
        failed.message.as_deref(),
        Some(
            format!(
                "app-gone isn't installed at {} - edit the application to choose its location",
                missing.display()
            )
            .as_str()
        )
    );
    assert_eq!(failed.attempt, 0, "validation failures never launch");
    let skipped = out.entry(dependent.id);
    assert_eq!(skipped.status, EntryStatus::Skipped);
    assert_eq!(
        skipped.message.as_deref(),
        Some("Skipped because app-gone did not start (required dependency)")
    );
    assert_eq!(out.entry(unrelated.id).status, EntryStatus::Completed);
    let mut retryable = out.run.retryable_entries();
    retryable.sort();
    let mut expected = vec![gone.id, dependent.id];
    expected.sort();
    assert_eq!(retryable, expected);
    assert_eq!(h.platform.launches().len(), 1);
}

#[test]
fn dependencies_outside_the_run_count_as_satisfied() {
    let h = Harness::new();
    for name in ["app-a", "app-c", "app-d"] {
        h.platform.add_rule(h.rule(name, &[name]));
    }
    let a = h.app("app-a");
    let mut b = h.app("app-b");
    b.enabled = false;
    let c = h.app("app-c");
    let mut d = h.app("app-d");
    d.depends_on = vec![c.id];
    let ws = workspace(vec![a.clone(), b.clone(), c.clone(), d.clone()]);
    let mut opts = options(SwitchMode::Merge);
    opts.entry_ids = Some(vec![a.id, b.id, d.id]);
    let out = h.run_with(
        Arc::new(h.platform.clone()),
        &ws,
        opts,
        Arc::new(AtomicBool::new(false)),
        |_, _| {},
    );

    let ids: Vec<Uuid> = out.run.entries.iter().map(|e| e.entry_id).collect();
    assert_eq!(
        ids,
        vec![a.id, b.id, d.id],
        "only the selected entries are part of the run"
    );
    assert_eq!(out.entry(b.id).status, EntryStatus::Skipped);
    assert_eq!(out.entry(b.id).message.as_deref(), Some("Disabled"));
    assert_eq!(out.entry(a.id).status, EntryStatus::Completed);
    assert_eq!(out.entry(d.id).status, EntryStatus::Completed);
    assert_eq!(out.run.status, RunStatus::Completed);
    // Independent entries start together, so compare without order.
    let mut launched: Vec<String> = h
        .platform
        .launches()
        .iter()
        .map(|l| l.target.label())
        .collect();
    launched.sort();
    let mut expected = vec![
        h.exe("app-a").display().to_string(),
        h.exe("app-d").display().to_string(),
    ];
    expected.sort();
    assert_eq!(
        launched, expected,
        "app-c is not selected, so it is not launched"
    );
}

// ---------------------------------------------------------------------------- launching

#[test]
fn launch_failures_are_retried() {
    let h = Harness::new();
    h.platform
        .add_rule(h.rule("app-flaky", &["Flaky"]).fail_times(2));
    let flaky = h.app("app-flaky");
    let mut ws = workspace(vec![flaky.clone()]);
    ws.restore.retry_count = 2;
    let out = h.run(&ws, SwitchMode::Merge);

    let e = out.entry(flaky.id);
    assert_eq!(e.status, EntryStatus::Completed);
    assert_eq!(e.attempt, 3);
    assert_eq!(h.platform.launches().len(), 3);
    let attempts: Vec<u32> = out
        .updates(flaky.id)
        .iter()
        .map(|(_, e)| e.attempt)
        .collect();
    for n in 1..=3 {
        assert!(attempts.contains(&n), "attempt {n} reported: {attempts:?}");
    }
}

#[test]
fn exhausted_retries_fail_with_the_platform_error() {
    let h = Harness::new();
    h.platform
        .add_rule(h.rule("app-broken", &["Broken"]).fail_times(10));
    h.platform
        .add_rule(h.rule("app-once", &["Once"]).fail_times(10));
    let broken = h.app("app-broken");
    let mut once = h.app("app-once");
    once.options.retry_count = Some(0);
    let mut ws = workspace(vec![broken.clone(), once.clone()]);
    ws.restore.retry_count = 1;
    let out = h.run(&ws, SwitchMode::Merge);

    assert_eq!(out.run.status, RunStatus::Failed);
    let e = out.entry(broken.id);
    assert_eq!(e.status, EntryStatus::Failed);
    assert_eq!(e.attempt, 2);
    assert_eq!(
        e.message.as_deref(),
        Some(
            format!(
                "Could not launch {}: simulated launch failure (tried 2 times)",
                h.exe("app-broken").display()
            )
            .as_str()
        )
    );
    let once_progress = out.entry(once.id);
    assert_eq!(
        (once_progress.status, once_progress.attempt),
        (EntryStatus::Failed, 1)
    );
    assert!(!once_progress.message.as_deref().unwrap().contains("tried"));
    let count = |name: &str| {
        h.platform
            .launches()
            .iter()
            .filter(|l| l.target.label().ends_with(name))
            .count()
    };
    assert_eq!((count("app-broken"), count("app-once")), (2, 1));
}

#[test]
fn no_window_within_the_timeout_is_partial() {
    let h = Harness::new();
    // No launch rule: the mock process starts but never opens a window.
    let silent = h.app("app-silent");
    let mut quick = h.app("app-quick");
    quick.options.timeout_ms = Some(100);
    let mut ws = workspace(vec![silent.clone(), quick.clone()]);
    ws.restore.timeout_ms = 200;
    let out = h.run(&ws, SwitchMode::Merge);

    assert_eq!(out.run.status, RunStatus::PartiallyCompleted);
    let e = out.entry(silent.id);
    assert_eq!(e.status, EntryStatus::PartiallyCompleted);
    assert_eq!(
        e.message.as_deref(),
        Some("Started, but no window appeared within 0.2 s")
    );
    assert_eq!((e.windows_restored, e.windows_expected), (0, 1));
    assert_eq!(
        out.entry(quick.id).message.as_deref(),
        Some("Started, but no window appeared within 0.1 s")
    );
}

#[test]
fn apps_that_open_no_window_complete_without_waiting() {
    let h = Harness::new();
    let mut daemon = ApplicationEntry::new("daemon", "Sync daemon", "daemon-test");
    daemon.launch.executable = Some(h.exe("app-daemon"));
    let mut ws = workspace(vec![daemon.clone()]);
    ws.restore.timeout_ms = 5_000;
    let out = h.run(&ws, SwitchMode::Merge);

    let e = out.entry(daemon.id);
    assert_eq!(e.status, EntryStatus::Completed);
    assert_eq!(e.windows_expected, 0);
    assert!(
        out.elapsed < ms(2_000),
        "no waiting for windows: {:?}",
        out.elapsed
    );
}

#[test]
fn missing_working_folder_fails_validation() {
    let h = Harness::new();
    let mut tool = h.app("app-tool");
    let folder = h.dir.path().join("no-such-folder");
    tool.launch.working_directory = Some(folder.clone());
    let out = h.run(&workspace(vec![tool.clone()]), SwitchMode::Merge);

    let e = out.entry(tool.id);
    assert_eq!(e.status, EntryStatus::Failed);
    let message = e.message.as_deref().unwrap();
    assert!(message.contains("working folder"), "{message}");
    assert!(message.contains(&folder.display().to_string()), "{message}");
    assert!(h.platform.launches().is_empty());
}

#[test]
fn adapter_launch_plan_errors_fail_the_entry() {
    let h = Harness::new();
    let unconfigured = ApplicationEntry::new("exe:mystery", "Mystery", "generic");
    let out = h.run(&workspace(vec![unconfigured.clone()]), SwitchMode::Merge);
    let e = out.entry(unconfigured.id);
    assert_eq!(e.status, EntryStatus::Failed);
    assert_eq!(
        e.message.as_deref(),
        Some("No executable is configured for Mystery.")
    );
    assert_eq!(out.run.status, RunStatus::Failed);
}

// ---------------------------------------------------------------------------- windows

#[test]
fn single_instance_windows_may_belong_to_an_already_running_process() {
    let h = Harness::new();
    let editor_pid = h.platform.add_process("editor", Some(h.exe("editor-app")));
    let old = h
        .platform
        .add_window(editor_pid, "old — Editor", None, Rect::new(0, 0, 500, 400));
    h.platform.add_rule(
        h.rule("editor-app", &["new — Editor"])
            .single_instance(editor_pid)
            .delay(ms(30)),
    );
    let mut editor = ApplicationEntry::new("editor", "Editor", "editor-test");
    editor.launch.executable = Some(h.exe("editor-app"));
    let saved = Rect::new(300, 200, 900, 700);
    editor.windows = vec![snapshot("new — Editor", saved, Some("MOCK-1"))];
    // Additive: the running editor is not reused, a new window is requested from it.
    let out = h.run(&workspace(vec![editor.clone()]), SwitchMode::Additive);

    let e = out.entry(editor.id);
    assert_eq!(e.status, EntryStatus::Completed, "{:?}", e.message);
    assert_eq!(e.windows_restored, 1);
    let new = h.window_titled("new — Editor");
    assert_eq!(new.pid, editor_pid, "owned by the already-running process");
    assert_eq!(new.bounds, saved);
    let placed: Vec<WindowId> = h
        .platform
        .placements()
        .into_iter()
        .map(|(w, _)| w)
        .collect();
    assert_eq!(placed, vec![new.id]);
    assert_eq!(
        h.platform.window(&old).unwrap().bounds,
        Rect::new(0, 0, 500, 400)
    );
}

#[test]
fn windows_are_matched_to_snapshots_by_title() {
    let h = Harness::new();
    h.platform
        .add_rule(h.rule("app-mail", &["Inbox — Mail", "Calendar — Mail"]));
    let mut mail = h.app("app-mail");
    let calendar_at = Rect::new(50, 60, 700, 500);
    let inbox_at = Rect::new(900, 100, 800, 700);
    mail.windows = vec![
        snapshot("Calendar — Mail", calendar_at, Some("MOCK-1")),
        snapshot("Inbox — Mail", inbox_at, Some("MOCK-1")),
    ];
    let out = h.run(&workspace(vec![mail.clone()]), SwitchMode::Merge);

    let e = out.entry(mail.id);
    assert_eq!(e.status, EntryStatus::Completed);
    assert_eq!((e.windows_restored, e.windows_expected), (2, 2));
    assert_eq!(h.window_titled("Calendar — Mail").bounds, calendar_at);
    assert_eq!(h.window_titled("Inbox — Mail").bounds, inbox_at);
}

#[test]
fn multi_step_single_instance_launch_places_each_project_window() {
    let h = Harness::new();
    let editor_pid = h.platform.add_process("editor", Some(h.exe("editor-app")));
    let alpha = h.dir.path().join("proj-alpha");
    let beta = h.dir.path().join("proj-beta");
    std::fs::create_dir_all(&alpha).unwrap();
    std::fs::create_dir_all(&beta).unwrap();
    // The project windows appear in the opposite order of the launches.
    h.platform.add_rule(
        LaunchRule::new("proj-alpha")
            .window(MockWindowSpec::titled("proj-alpha — Editor"))
            .single_instance(editor_pid)
            .delay(ms(60)),
    );
    h.platform.add_rule(
        LaunchRule::new("proj-beta")
            .window(MockWindowSpec::titled("proj-beta — Editor"))
            .single_instance(editor_pid),
    );
    let mut editor = ApplicationEntry::new("editor", "Editor", "editor-test");
    editor.launch.executable = Some(h.exe("editor-app"));
    editor.state = AppState::Projects {
        projects: [&alpha, &beta]
            .into_iter()
            .map(|p| ProjectRef {
                path: p.clone(),
                is_workspace_file: false,
                window_title: None,
            })
            .collect(),
    };
    let alpha_at = Rect::new(0, 0, 960, 1040);
    let beta_at = Rect::new(960, 0, 960, 1040);
    editor.windows = vec![
        snapshot("main.rs — proj-beta — Editor", beta_at, Some("MOCK-1")),
        snapshot("notes — proj-alpha — Editor", alpha_at, Some("MOCK-1")),
    ];
    // Merge: a running process without windows does not count as "already running".
    let out = h.run(&workspace(vec![editor.clone()]), SwitchMode::Merge);

    let e = out.entry(editor.id);
    assert_eq!(e.status, EntryStatus::Completed, "{:?}", e.message);
    assert_eq!((e.windows_restored, e.windows_expected), (2, 2));
    assert_eq!(h.platform.launches().len(), 2);
    assert_eq!(h.window_titled("proj-alpha — Editor").bounds, alpha_at);
    assert_eq!(h.window_titled("proj-beta — Editor").bounds, beta_at);
}

#[test]
fn every_window_gets_a_saved_position_when_titles_changed_after_relaunch() {
    let h = Harness::new();
    h.platform
        .add_rule(h.rule("app-sketch", &["Untitled 1", "Untitled 2"]));
    let mut sketch = h.app("app-sketch");
    let first_at = Rect::new(0, 0, 700, 500);
    let second_at = Rect::new(800, 300, 700, 500);
    sketch.windows = vec![
        snapshot("logo.svg - Sketch", first_at, None),
        snapshot("banner.svg - Sketch", second_at, None),
    ];
    let out = h.run(&workspace(vec![sketch.clone()]), SwitchMode::Merge);

    let e = out.entry(sketch.id);
    assert_eq!(e.status, EntryStatus::Completed, "{:?}", e.message);
    assert_eq!((e.windows_restored, e.windows_expected), (2, 2));
    let mut placed: Vec<Rect> = h
        .platform
        .placements()
        .iter()
        .map(|(_, p)| p.bounds)
        .collect();
    placed.sort_by_key(|r| r.x);
    assert_eq!(
        placed,
        vec![first_at, second_at],
        "no title matches, yet both are placed"
    );
}

#[test]
fn a_failed_step_does_not_hold_up_the_rest_of_the_entry() {
    let h = Harness::new();
    let editor_pid = h.platform.add_process("editor", Some(h.exe("editor-app")));
    let good = h.dir.path().join("proj-good");
    let bad = h.dir.path().join("proj-bad");
    std::fs::create_dir_all(&good).unwrap();
    std::fs::create_dir_all(&bad).unwrap();
    h.platform.add_rule(
        LaunchRule::new("proj-good")
            .window(MockWindowSpec::titled("proj-good — Editor"))
            .single_instance(editor_pid),
    );
    h.platform.add_rule(
        LaunchRule::new("proj-bad")
            .window(MockWindowSpec::titled("proj-bad — Editor"))
            .fail_times(10),
    );
    let mut editor = ApplicationEntry::new("editor", "Editor", "editor-test");
    editor.launch.executable = Some(h.exe("editor-app"));
    editor.state = AppState::Projects {
        projects: [&good, &bad]
            .into_iter()
            .map(|p| ProjectRef {
                path: p.clone(),
                is_workspace_file: false,
                window_title: None,
            })
            .collect(),
    };
    let mut ws = workspace(vec![editor.clone()]);
    ws.restore.timeout_ms = 10_000;
    let out = h.run(&ws, SwitchMode::Merge);

    let e = out.entry(editor.id);
    assert_eq!(e.status, EntryStatus::PartiallyCompleted);
    assert_eq!(
        e.message.as_deref(),
        Some(
            format!(
                "1 of 2 windows reopened; 1 of 2 launches failed: Could not launch {}: simulated launch failure",
                bad.display()
            )
            .as_str()
        )
    );
    assert_eq!((e.windows_restored, e.windows_expected), (1, 2));
    assert!(
        out.elapsed < ms(5_000),
        "no waiting for the window of a step that never launched: {:?}",
        out.elapsed
    );
}

#[test]
fn windows_are_remapped_from_two_monitors_to_one() {
    let h = Harness::new();
    let current = h.platform.monitors().unwrap();
    h.platform
        .add_rule(h.rule("app-docs", &["Left doc", "Right doc"]));
    let mut docs = h.app("app-docs");
    let left_at = Rect::new(100, 100, 800, 600);
    let mut right = snapshot(
        "Right doc",
        Rect::new(1920 + 200, 100, 800, 600),
        Some("MOCK-2"),
    );
    right.state = WindowState::Maximized;
    docs.windows = vec![snapshot("Left doc", left_at, Some("MOCK-1")), right];
    let mut ws = workspace(vec![docs.clone()]);
    ws.monitors = vec![
        current[0].clone(),
        MonitorInfo {
            id: "MOCK-2".into(),
            name: "Mock Display 2".into(),
            bounds: Rect::new(1920, 0, 1920, 1080),
            work_area: Rect::new(1920, 0, 1920, 1040),
            scale_factor: 1.0,
            is_primary: false,
        },
    ];
    let out = h.run(&ws, SwitchMode::Merge);

    assert_eq!(out.entry(docs.id).status, EntryStatus::Completed);
    let placements: HashMap<WindowId, WindowPlacement> =
        h.platform.placements().into_iter().collect();
    let left = h.window_titled("Left doc");
    let right = h.window_titled("Right doc");
    assert_eq!(
        placements[&left.id].bounds, left_at,
        "unchanged monitor, unchanged bounds"
    );
    assert_eq!(
        placements[&right.id],
        WindowPlacement {
            bounds: Rect::new(200, 100, 800, 600),
            state: WindowState::Maximized
        },
        "moved from the missing monitor into the primary work area"
    );
    assert_eq!(right.monitor_id.as_deref(), Some("MOCK-1"));
    assert_eq!(
        right.bounds, current[0].work_area,
        "maximized on the remaining monitor"
    );
}

#[test]
fn missing_windows_are_reported_precisely() {
    let h = Harness::new();
    h.platform
        .add_rule(h.rule("app-paint", &["Image 1", "Image 2"]));
    let mut paint = h.app("app-paint");
    paint.windows = (1..=3)
        .map(|i| snapshot(&format!("Image {i}"), Rect::new(10 * i, 10, 400, 300), None))
        .collect();
    let out = h.run(&workspace(vec![paint.clone()]), SwitchMode::Merge);

    let e = out.entry(paint.id);
    assert_eq!(e.status, EntryStatus::PartiallyCompleted);
    assert_eq!(e.message.as_deref(), Some("2 of 3 windows reopened"));
    assert_eq!((e.windows_restored, e.windows_expected), (2, 3));
    assert_eq!(out.run.status, RunStatus::PartiallyCompleted);
}

#[test]
fn entries_of_the_same_app_never_share_windows() {
    let h = Harness::new();
    h.platform
        .add_rule(h.rule("app-term", &["Terminal"]).delay(ms(20)));
    let mut first = h.app("app-term");
    first.windows = vec![snapshot("Terminal", Rect::new(0, 0, 600, 400), None)];
    let mut second = h.app("app-term");
    second.id = Uuid::new_v4();
    second.windows = vec![snapshot("Terminal", Rect::new(700, 0, 600, 400), None)];
    let out = h.run(
        &workspace(vec![first.clone(), second.clone()]),
        SwitchMode::Additive,
    );

    assert_eq!(out.run.status, RunStatus::Completed);
    let placements = h.platform.placements();
    assert_eq!(placements.len(), 2);
    assert_ne!(
        placements[0].0, placements[1].0,
        "each window is claimed once"
    );
    let mut bounds: Vec<Rect> = h
        .platform
        .windows()
        .unwrap()
        .iter()
        .map(|w| w.bounds)
        .collect();
    bounds.sort_by_key(|r| r.x);
    assert_eq!(
        bounds,
        vec![Rect::new(0, 0, 600, 400), Rect::new(700, 0, 600, 400)]
    );
}

#[test]
fn placement_errors_make_an_entry_partial() {
    let h = Harness::new();
    h.platform.add_rule(h.rule("app-view", &["Viewer"]));
    let mut view = h.app("app-view");
    view.windows = vec![snapshot("Viewer", Rect::new(10, 10, 500, 500), None)];
    let faulty = Faulty {
        inner: h.platform.clone(),
        fail_moves: true,
        ..Default::default()
    };
    let out = h.run_with(
        Arc::new(faulty),
        &workspace(vec![view.clone()]),
        options(SwitchMode::Merge),
        Arc::new(AtomicBool::new(false)),
        |_, _| {},
    );

    let e = out.entry(view.id);
    assert_eq!(e.status, EntryStatus::PartiallyCompleted);
    assert_eq!(
        e.message.as_deref(),
        Some("1 window could not be moved: Moving the window failed: denied by the test")
    );
    assert_eq!((e.windows_restored, e.windows_expected), (0, 1));
}

#[test]
fn positions_are_not_touched_when_disabled_or_unsupported() {
    let h = Harness::new();
    h.platform.add_rule(h.rule("app-calc", &["Calculator"]));
    let mut calc = h.app("app-calc");
    calc.windows = vec![snapshot("Calculator", Rect::new(10, 10, 300, 400), None)];
    calc.options.restore_window_positions = false;
    let out = h.run(&workspace(vec![calc.clone()]), SwitchMode::Merge);
    assert_eq!(out.entry(calc.id).status, EntryStatus::Completed);
    assert_eq!(out.entry(calc.id).windows_restored, 1);
    assert!(h.platform.placements().is_empty());

    // The platform cannot move windows: the run explains why positions were not restored.
    let h = Harness::new();
    let mut info = h.platform.info();
    info.capabilities.move_windows = Capability::unsupported("Wayland does not allow it.");
    h.platform.set_info(info);
    h.platform.add_rule(h.rule("app-calc", &["Calculator"]));
    let mut calc = h.app("app-calc");
    calc.windows = vec![snapshot("Calculator", Rect::new(10, 10, 300, 400), None)];
    let out = h.run(&workspace(vec![calc.clone()]), SwitchMode::Merge);
    assert_eq!(out.entry(calc.id).status, EntryStatus::Completed);
    assert!(h.platform.placements().is_empty());
    assert_eq!(
        out.run.messages,
        vec!["Window positions were not restored: Wayland does not allow it.".to_string()]
    );
}

#[test]
fn window_listing_unavailable_still_launches() {
    let h = Harness::new();
    h.platform.add_rule(h.rule("app-notes", &["Notes"]));
    let mut notes = h.app("app-notes");
    notes.windows = vec![snapshot("Notes", Rect::new(0, 0, 300, 300), None)];
    let faulty = Faulty {
        inner: h.platform.clone(),
        fail_windows: true,
        ..Default::default()
    };
    let out = h.run_with(
        Arc::new(faulty),
        &workspace(vec![notes.clone()]),
        options(SwitchMode::Merge),
        Arc::new(AtomicBool::new(false)),
        |_, _| {},
    );

    let e = out.entry(notes.id);
    assert_eq!(e.status, EntryStatus::Completed);
    assert_eq!(
        e.message.as_deref(),
        Some("Started - Reframe could not see its windows")
    );
    assert_eq!(e.windows_expected, 0);
    assert_eq!(h.platform.launches().len(), 1);
    assert!(
        out.run
            .messages
            .iter()
            .any(|m| m.starts_with("Reframe can't see open windows on this system")),
        "{:?}",
        out.run.messages
    );
    assert!(
        out.run
            .messages
            .iter()
            .any(|m| m.starts_with("Reframe could not wait for windows")),
        "{:?}",
        out.run.messages
    );
}

#[test]
fn window_listing_failing_at_runtime_is_explained() {
    let h = Harness::new();
    h.platform.add_rule(h.rule("app-notes", &["Notes"]));
    let notes = h.app("app-notes");
    let faulty = Faulty {
        inner: h.platform.clone(),
        break_windows: true,
        ..Default::default()
    };
    let out = h.run_with(
        Arc::new(faulty),
        &workspace(vec![notes.clone()]),
        options(SwitchMode::Additive),
        Arc::new(AtomicBool::new(false)),
        |_, _| {},
    );

    let e = out.entry(notes.id);
    assert_eq!(e.status, EntryStatus::Completed);
    assert_eq!(
        e.message.as_deref(),
        Some("Started - Reframe could not see its windows")
    );
    assert_eq!(e.windows_expected, 0);
    assert_eq!(h.platform.launches().len(), 1);
    assert_eq!(
        out.run.messages,
        vec![
            "Reframe could not wait for windows or arrange them: Listing windows failed: the display connection was lost"
                .to_string()
        ]
    );
}

// ---------------------------------------------------------------------------- modes

#[test]
fn merge_reuses_running_apps_without_launching_them() {
    let h = Harness::new();
    let todo = h.running("app-notes", "todo — Notes", Rect::new(0, 0, 300, 300));
    h.platform.add_rule(h.rule("app-mail", &["Inbox — Mail"]));
    let mut notes = h.app("app-notes");
    let saved = Rect::new(500, 400, 640, 480);
    notes.windows = vec![snapshot("todo — Notes", saved, Some("MOCK-1"))];
    let mail = h.app("app-mail");
    let out = h.run(
        &workspace(vec![notes.clone(), mail.clone()]),
        SwitchMode::Merge,
    );

    assert_eq!(out.run.status, RunStatus::Completed);
    let e = out.entry(notes.id);
    assert_eq!(e.status, EntryStatus::Completed);
    assert_eq!(e.message.as_deref(), Some(ALREADY_RUNNING_ARRANGED));
    assert_eq!(e.windows_restored, 1);
    assert_eq!(h.platform.window(&todo).unwrap().bounds, saved);
    let launched: Vec<String> = h
        .platform
        .launches()
        .iter()
        .map(|l| l.target.label())
        .collect();
    assert_eq!(launched, vec![h.exe("app-mail").display().to_string()]);
    assert_eq!(
        out.updates(notes.id).len(),
        1,
        "reused entries complete in one step"
    );

    // Without saved positions to apply the entry is simply "Already running".
    let h = Harness::new();
    h.running("app-notes", "todo — Notes", Rect::new(0, 0, 300, 300));
    let mut notes = h.app("app-notes");
    notes.windows = vec![snapshot("todo — Notes", saved, None)];
    notes.options.restore_window_positions = false;
    let out = h.run(&workspace(vec![notes.clone()]), SwitchMode::Merge);
    assert_eq!(
        out.entry(notes.id).message.as_deref(),
        Some(ALREADY_RUNNING)
    );
    assert!(h.platform.placements().is_empty() && h.platform.launches().is_empty());
}

/// An editor entry with one project folder per name (one launch step each, hinted with the
/// folder name), plus the already-running single-instance editor process that owns its
/// windows. Each project opens one window titled "<name> — Editor" when launched.
fn editor_with_projects(h: &Harness, names: &[&str]) -> (ApplicationEntry, u32) {
    let editor_pid = h.platform.add_process("editor", Some(h.exe("editor-app")));
    let mut projects = Vec::new();
    for name in names {
        let path = h.dir.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        h.platform.add_rule(
            LaunchRule::new(*name)
                .window(MockWindowSpec::titled(format!("{name} — Editor")))
                .single_instance(editor_pid),
        );
        projects.push(ProjectRef {
            path,
            is_workspace_file: false,
            window_title: None,
        });
    }
    let mut editor = ApplicationEntry::new("editor", "Editor", "editor-test");
    editor.launch.executable = Some(h.exe("editor-app"));
    editor.state = AppState::Projects { projects };
    (editor, editor_pid)
}

fn launched_labels(h: &Harness) -> Vec<String> {
    h.platform
        .launches()
        .iter()
        .map(|l| l.target.label())
        .collect()
}

#[test]
fn merge_launches_only_the_projects_that_are_not_open() {
    let h = Harness::new();
    let (mut editor, editor_pid) = editor_with_projects(&h, &["proj-alpha", "proj-beta"]);
    // proj-alpha is already open (with a file title around the folder name); proj-beta is not.
    let alpha_window = h.platform.add_window(
        editor_pid,
        "main.rs — proj-alpha — Editor",
        None,
        Rect::new(0, 0, 400, 300),
    );
    let alpha_at = Rect::new(0, 0, 960, 1040);
    let beta_at = Rect::new(960, 0, 960, 1040);
    editor.windows = vec![
        snapshot("main.rs — proj-alpha — Editor", alpha_at, Some("MOCK-1")),
        snapshot("lib.rs — proj-beta — Editor", beta_at, Some("MOCK-1")),
    ];
    let ws = workspace(vec![editor.clone()]);

    let plan = switch_plan(&h, &ws, SwitchMode::Merge);
    assert!(plan.reuse.is_empty(), "{:?}", plan.reuse);
    assert_eq!(plan.launch.len(), 1);
    assert_eq!(plan.launch[0].reason, "1 of 2 projects not open");
    assert!(!health_says_running(&h, &ws, editor.id));

    let out = h.run(&ws, SwitchMode::Merge);
    let beta = h.dir.path().join("proj-beta").display().to_string();
    assert_eq!(
        launched_labels(&h),
        vec![beta],
        "only the missing project opens"
    );
    let e = out.entry(editor.id);
    assert_eq!(e.status, EntryStatus::Completed, "{:?}", e.message);
    assert_eq!(
        e.message.as_deref(),
        Some("Opened 1 of 2 projects; 1 was already open")
    );
    assert_eq!((e.windows_restored, e.windows_expected), (2, 2));
    assert_eq!(h.platform.window(&alpha_window).unwrap().bounds, alpha_at);
    assert_eq!(h.window_titled("proj-beta — Editor").bounds, beta_at);
}

#[test]
fn merge_reuses_an_entry_whose_projects_are_all_open() {
    let h = Harness::new();
    let (mut editor, editor_pid) = editor_with_projects(&h, &["proj-alpha", "proj-beta"]);
    let beta_window = h.platform.add_window(
        editor_pid,
        "proj-beta — Editor",
        None,
        Rect::new(0, 0, 400, 300),
    );
    let alpha_window = h.platform.add_window(
        editor_pid,
        "x.rs — PROJ-ALPHA — Editor",
        None,
        Rect::new(0, 0, 400, 300),
    );
    let alpha_at = Rect::new(0, 0, 960, 1040);
    let beta_at = Rect::new(960, 0, 960, 1040);
    editor.windows = vec![
        snapshot("proj-alpha — Editor", alpha_at, Some("MOCK-1")),
        snapshot("proj-beta — Editor", beta_at, Some("MOCK-1")),
    ];
    let ws = workspace(vec![editor.clone()]);

    let plan = switch_plan(&h, &ws, SwitchMode::Merge);
    assert!(plan.launch.is_empty(), "{:?}", plan.launch);
    assert_eq!(plan.reuse.len(), 1);
    assert_eq!(
        plan.reuse[0].window_ids,
        vec![alpha_window.clone(), beta_window.clone()]
    );
    assert!(health_says_running(&h, &ws, editor.id));

    let out = h.run(&ws, SwitchMode::Merge);
    assert!(h.platform.launches().is_empty());
    let e = out.entry(editor.id);
    assert_eq!(e.status, EntryStatus::Completed);
    assert_eq!(e.message.as_deref(), Some(ALREADY_RUNNING_ARRANGED));
    assert_eq!(e.windows_restored, 2);
    assert_eq!(h.platform.window(&alpha_window).unwrap().bounds, alpha_at);
    assert_eq!(h.platform.window(&beta_window).unwrap().bounds, beta_at);
}

#[test]
fn an_editor_open_on_other_projects_does_not_count_as_restored() {
    let h = Harness::new();
    let (editor, editor_pid) = editor_with_projects(&h, &["proj-alpha", "proj-beta"]);
    // Another project, and one whose name merely contains "proj-alpha".
    let started_at = Rect::new(0, 0, 400, 300);
    let gamma = h
        .platform
        .add_window(editor_pid, "proj-gamma — Editor", None, started_at);
    let similar = h
        .platform
        .add_window(editor_pid, "proj-alpha-v2 — Editor", None, started_at);
    let ws = workspace(vec![editor.clone()]);

    let plan = switch_plan(&h, &ws, SwitchMode::Merge);
    assert!(plan.reuse.is_empty(), "{:?}", plan.reuse);
    assert_eq!(plan.launch[0].reason, "2 of 2 projects not open");
    assert!(!health_says_running(&h, &ws, editor.id));

    let out = h.run(&ws, SwitchMode::Merge);
    assert_eq!(h.platform.launches().len(), 2, "both projects are opened");
    let e = out.entry(editor.id);
    assert_eq!(e.status, EntryStatus::Completed, "{:?}", e.message);
    assert_eq!(e.message, None);
    assert_eq!((e.windows_restored, e.windows_expected), (2, 2));
    for other in [gamma, similar] {
        assert_eq!(h.platform.window(&other).unwrap().bounds, started_at);
    }
}

#[test]
fn additive_launches_every_project_even_when_open() {
    let h = Harness::new();
    let (editor, editor_pid) = editor_with_projects(&h, &["proj-alpha", "proj-beta"]);
    h.platform.add_window(
        editor_pid,
        "proj-alpha — Editor",
        None,
        Rect::new(0, 0, 400, 300),
    );
    let ws = workspace(vec![editor.clone()]);

    let plan = switch_plan(&h, &ws, SwitchMode::Additive);
    assert!(plan.reuse.is_empty());
    assert!(plan.launch[0].reason.starts_with("Additive mode"));

    let out = h.run(&ws, SwitchMode::Additive);
    assert_eq!(h.platform.launches().len(), 2);
    let e = out.entry(editor.id);
    assert_eq!(e.status, EntryStatus::Completed, "{:?}", e.message);
    assert_eq!(e.message, None);
    assert_eq!((e.windows_restored, e.windows_expected), (2, 2));
}

#[test]
fn generic_apps_are_still_reused_per_application() {
    let h = Harness::new();
    // The window shows a different document than the saved one: still the same application.
    let window = h.running("app-notes", "Untitled — Notes", Rect::new(0, 0, 300, 300));
    let mut notes = h.app("app-notes");
    let saved = Rect::new(500, 400, 640, 480);
    notes.windows = vec![snapshot("todo — Notes", saved, None)];
    let ws = workspace(vec![notes.clone()]);

    let plan = switch_plan(&h, &ws, SwitchMode::Merge);
    assert!(plan.launch.is_empty());
    assert_eq!(plan.reuse[0].window_ids, vec![window.clone()]);
    assert!(health_says_running(&h, &ws, notes.id));

    let out = h.run(&ws, SwitchMode::Merge);
    assert!(h.platform.launches().is_empty());
    assert_eq!(
        out.entry(notes.id).message.as_deref(),
        Some(ALREADY_RUNNING_ARRANGED)
    );
    assert_eq!(h.platform.window(&window).unwrap().bounds, saved);
}

#[test]
fn a_partly_open_entry_arranges_its_open_windows_when_the_rest_cannot_start() {
    let h = Harness::new();
    let (mut editor, editor_pid) = editor_with_projects(&h, &["proj-alpha", "proj-beta"]);
    let alpha_window = h.platform.add_window(
        editor_pid,
        "proj-alpha — Editor",
        None,
        Rect::new(0, 0, 400, 300),
    );
    let beta = h.dir.path().join("proj-beta");
    std::fs::remove_dir(&beta).unwrap();
    let alpha_at = Rect::new(100, 100, 800, 600);
    editor.windows = vec![
        snapshot("proj-alpha — Editor", alpha_at, Some("MOCK-1")),
        snapshot(
            "proj-beta — Editor",
            Rect::new(900, 100, 800, 600),
            Some("MOCK-1"),
        ),
    ];
    let mut ws = workspace(vec![editor.clone()]);
    ws.restore.timeout_ms = 10_000;
    let out = h.run(&ws, SwitchMode::Merge);

    assert!(h.platform.launches().is_empty());
    let e = out.entry(editor.id);
    assert_eq!(e.status, EntryStatus::PartiallyCompleted);
    assert_eq!(
        e.message.as_deref(),
        Some(
            format!(
                "1 of 2 windows reopened; 1 of 1 launches failed: {} no longer exists, so Editor cannot open it - edit the application; 1 of 2 projects was already open",
                beta.display()
            )
            .as_str()
        )
    );
    assert_eq!((e.windows_restored, e.windows_expected), (1, 2));
    assert_eq!(h.platform.window(&alpha_window).unwrap().bounds, alpha_at);
    assert!(
        out.elapsed < ms(5_000),
        "nothing to wait for: {:?}",
        out.elapsed
    );
}

#[test]
fn a_running_window_is_reused_by_one_entry_only() {
    let h = Harness::new();
    let open = h.running("app-term", "Terminal", Rect::new(0, 0, 300, 300));
    h.platform.add_rule(h.rule("app-term", &["Terminal 2"]));
    let first_at = Rect::new(0, 0, 600, 400);
    let second_at = Rect::new(700, 0, 600, 400);
    let mut first = h.app("app-term");
    first.windows = vec![snapshot("Terminal", first_at, None)];
    let mut second = h.app("app-term");
    second.id = Uuid::new_v4();
    second.windows = vec![snapshot("Terminal", second_at, None)];
    let ws = workspace(vec![first.clone(), second.clone()]);

    let plan = switch_plan(&h, &ws, SwitchMode::Merge);
    assert_eq!(plan.reuse.len(), 1);
    assert_eq!(plan.reuse[0].entry_id, first.id);
    assert_eq!(plan.reuse[0].window_ids, vec![open.clone()]);
    assert_eq!(plan.launch.len(), 1);
    assert_eq!(plan.launch[0].entry_id, second.id);
    assert_eq!(plan.launch[0].reason, "Not running");
    assert!(health_says_running(&h, &ws, first.id));
    assert!(!health_says_running(&h, &ws, second.id));

    let out = h.run(&ws, SwitchMode::Merge);
    assert_eq!(h.platform.launches().len(), 1);
    assert_eq!(
        out.entry(first.id).message.as_deref(),
        Some(ALREADY_RUNNING_ARRANGED)
    );
    assert_eq!(out.entry(second.id).status, EntryStatus::Completed);
    assert_eq!(h.platform.window(&open).unwrap().bounds, first_at);
    assert_eq!(h.window_titled("Terminal 2").bounds, second_at);
}

#[test]
fn reused_windows_that_cannot_be_moved_make_the_entry_partial() {
    let h = Harness::new();
    h.running("app-notes", "todo — Notes", Rect::new(0, 0, 300, 300));
    let mut notes = h.app("app-notes");
    notes.windows = vec![snapshot(
        "todo — Notes",
        Rect::new(500, 400, 640, 480),
        None,
    )];
    let faulty = Faulty {
        inner: h.platform.clone(),
        fail_moves: true,
        ..Default::default()
    };
    let out = h.run_with(
        Arc::new(faulty),
        &workspace(vec![notes.clone()]),
        options(SwitchMode::Merge),
        Arc::new(AtomicBool::new(false)),
        |_, _| {},
    );

    assert!(h.platform.launches().is_empty());
    let e = out.entry(notes.id);
    assert_eq!(e.status, EntryStatus::PartiallyCompleted);
    assert_eq!(
        e.message.as_deref(),
        Some(
            "Already running - its windows could not be moved: Moving the window failed: denied by the test"
        )
    );
    assert_eq!(out.run.status, RunStatus::PartiallyCompleted);
}

#[test]
fn reused_dependencies_let_dependents_start_immediately() {
    let h = Harness::new();
    h.running("app-db", "Database", Rect::new(0, 0, 300, 300));
    h.platform.add_rule(h.rule("app-api", &["API"]));
    let mut db = h.app("app-db");
    db.options.delay_after_launch_ms = Some(5_000);
    let mut api = h.app("app-api");
    api.depends_on = vec![db.id];
    let out = h.run(&workspace(vec![db.clone(), api.clone()]), SwitchMode::Merge);
    assert_eq!(out.run.status, RunStatus::Completed);
    assert_eq!(out.entry(db.id).message.as_deref(), Some(ALREADY_RUNNING));
    assert!(
        out.elapsed < ms(4_000),
        "an already-running dependency needs no delay"
    );
}

#[test]
fn additive_launches_even_when_running() {
    let h = Harness::new();
    let old = h.running("app-notes", "todo — Notes", Rect::new(0, 0, 300, 300));
    h.platform
        .add_rule(h.rule("app-notes", &["Untitled — Notes"]));
    let mut notes = h.app("app-notes");
    let saved = Rect::new(400, 300, 500, 400);
    notes.windows = vec![snapshot("todo — Notes", saved, None)];
    let out = h.run(&workspace(vec![notes.clone()]), SwitchMode::Additive);

    assert_eq!(out.entry(notes.id).status, EntryStatus::Completed);
    assert_eq!(h.platform.launches().len(), 1);
    let new = h.window_titled("Untitled — Notes");
    assert_eq!(new.bounds, saved, "the new window gets the saved position");
    assert_eq!(
        h.platform.window(&old).unwrap().bounds,
        Rect::new(0, 0, 300, 300)
    );
}

#[test]
fn replace_closes_confirmed_windows_and_reports_refused_ones() {
    let h = Harness::new();
    let writer = h.running("app-writer", "report* — Writer", Rect::new(0, 0, 300, 300));
    let chat = h.running("app-chat", "General — Chat", Rect::new(0, 0, 300, 300));
    let own_pid = h.platform.add_process_info(ProcessInfo {
        pid: std::process::id(),
        parent_pid: None,
        name: "reframe".into(),
        product_name: None,
        executable_path: None,
        command_line: Vec::new(),
        working_directory: None,
        start_time: None,
        package: None,
    });
    let own = h
        .platform
        .add_window(own_pid, "Reframe", None, Rect::new(0, 0, 10, 10));
    h.platform.refuse_close("*");
    h.platform.add_rule(h.rule("app-term", &["Terminal"]));
    let term = h.app("app-term");
    let mut opts = options(SwitchMode::Replace);
    opts.close_window_ids = vec![
        writer.clone(),
        chat.clone(),
        WindowId::new("mock-does-not-exist"),
        own.clone(),
        chat.clone(),
    ];
    let out = h.run_with(
        Arc::new(h.platform.clone()),
        &workspace(vec![term.clone()]),
        opts,
        Arc::new(AtomicBool::new(false)),
        |_, _| {},
    );

    assert_eq!(out.run.status, RunStatus::Completed);
    assert_eq!(out.entry(term.id).status, EntryStatus::Completed);
    let outcomes: Vec<(&WindowId, CloseStatus, Option<&str>)> = out
        .run
        .closed_windows
        .iter()
        .map(|c| (&c.window_id, c.status, c.message.as_deref()))
        .collect();
    assert_eq!(
        outcomes,
        vec![
            (&writer, CloseStatus::StillOpen, Some(STILL_OPEN)),
            (&chat, CloseStatus::Closed, None),
        ]
    );
    let chat_outcome = &out.run.closed_windows[1];
    assert_eq!(chat_outcome.title, "General — Chat");
    assert_eq!(chat_outcome.app_name, "App Chat");
    assert_eq!(
        h.platform.close_requests(),
        vec![writer.clone(), chat.clone()]
    );
    assert!(h.platform.window(&writer).is_some(), "never force-closed");
    assert!(
        h.platform.window(&own).is_some(),
        "Reframe never closes itself"
    );

    let close_events = |id: &WindowId| -> Vec<CloseStatus> {
        out.events
            .iter()
            .filter_map(|(_, e)| match e {
                RestoreEvent::WindowCloseUpdated { outcome, .. } if &outcome.window_id == id => {
                    Some(outcome.status)
                }
                _ => None,
            })
            .collect()
    };
    assert_eq!(
        close_events(&chat),
        vec![CloseStatus::Requested, CloseStatus::Closed]
    );
    assert_eq!(
        close_events(&writer),
        vec![CloseStatus::Requested, CloseStatus::StillOpen]
    );
    let last_close = out
        .events
        .iter()
        .filter(|(_, e)| matches!(e, RestoreEvent::WindowCloseUpdated { .. }))
        .map(|(t, _)| *t)
        .max()
        .unwrap();
    assert!(
        out.reached(term.id, EntryStatus::Validating) >= last_close,
        "the restore continues after the closing phase"
    );
}

#[test]
fn close_errors_are_reported_as_failed() {
    let h = Harness::new();
    let chat = h.running("app-chat", "General — Chat", Rect::new(0, 0, 300, 300));
    h.platform.add_rule(h.rule("app-term", &["Terminal"]));
    let term = h.app("app-term");
    let faulty = Faulty {
        inner: h.platform.clone(),
        fail_close: true,
        ..Default::default()
    };
    let mut opts = options(SwitchMode::Replace);
    opts.close_window_ids = vec![chat.clone()];
    let out = h.run_with(
        Arc::new(faulty),
        &workspace(vec![term.clone()]),
        opts,
        Arc::new(AtomicBool::new(false)),
        |_, _| {},
    );

    assert_eq!(out.run.closed_windows.len(), 1);
    let outcome = &out.run.closed_windows[0];
    assert_eq!(outcome.status, CloseStatus::Failed);
    assert_eq!(
        outcome.message.as_deref(),
        Some("Closing windows is not supported here: not in this test")
    );
    assert_eq!(out.entry(term.id).status, EntryStatus::Completed);
}

// ---------------------------------------------------------------------------- cancellation

#[test]
fn cancellation_skips_remaining_entries() {
    let h = Harness::new();
    h.platform
        .add_rule(h.rule("app-slow", &["Slow"]).delay(ms(3_000)));
    h.platform.add_rule(h.rule("app-other", &["Other"]));
    let slow = h.app("app-slow");
    let mut dependent = h.app("app-dependent");
    dependent.depends_on = vec![slow.id];
    let other = h.app("app-other");
    let mut ws = workspace(vec![slow.clone(), dependent.clone(), other.clone()]);
    ws.restore.max_parallel = 1;
    ws.restore.timeout_ms = 10_000;
    let slow_id = slow.id;
    let out = h.run_with(
        Arc::new(h.platform.clone()),
        &ws,
        options(SwitchMode::Merge),
        Arc::new(AtomicBool::new(false)),
        move |event, cancel| {
            if let RestoreEvent::EntryUpdated { entry, .. } = event
                && entry.entry_id == slow_id
                && entry.status == EntryStatus::Restoring
            {
                cancel.store(true, Ordering::SeqCst);
            }
        },
    );

    assert_eq!(out.run.status, RunStatus::Cancelled);
    assert_eq!(out.entry(slow.id).status, EntryStatus::PartiallyCompleted);
    assert_eq!(
        out.entry(slow.id).message.as_deref(),
        Some(CANCELLED_AFTER_START)
    );
    for id in [dependent.id, other.id] {
        assert_eq!(out.entry(id).status, EntryStatus::Skipped);
        assert_eq!(out.entry(id).message.as_deref(), Some(CANCELLED));
    }
    assert_eq!(h.platform.launches().len(), 1);
    assert!(
        out.elapsed < ms(2_500),
        "cancellation is prompt: {:?}",
        out.elapsed
    );
    assert_eq!(out.run.retryable_entries().len(), 3);
}

#[test]
fn cancelled_before_start_changes_nothing() {
    let h = Harness::new();
    let chat = h.running("app-chat", "General — Chat", Rect::new(0, 0, 300, 300));
    h.platform.add_rule(h.rule("app-term", &["Terminal"]));
    let term = h.app("app-term");
    let mut disabled = h.app("app-off");
    disabled.enabled = false;
    let mut opts = options(SwitchMode::Replace);
    opts.close_window_ids = vec![chat.clone()];
    let out = h.run_with(
        Arc::new(h.platform.clone()),
        &workspace(vec![term.clone(), disabled.clone()]),
        opts,
        Arc::new(AtomicBool::new(true)),
        |_, _| {},
    );

    assert_eq!(out.run.status, RunStatus::Cancelled);
    assert_eq!(out.entry(term.id).status, EntryStatus::Skipped);
    assert_eq!(out.entry(term.id).message.as_deref(), Some(CANCELLED));
    assert_eq!(out.entry(disabled.id).message.as_deref(), Some("Disabled"));
    assert!(h.platform.launches().is_empty());
    assert!(h.platform.close_requests().is_empty());
    assert!(out.run.closed_windows.is_empty());
}

// ---------------------------------------------------------------------------- edge cases

#[test]
fn nothing_selected_completes_with_an_explanation() {
    let h = Harness::new();
    let mut off = h.app("app-off");
    off.enabled = false;
    let out = h.run(&workspace(vec![off.clone()]), SwitchMode::Merge);
    assert_eq!(out.run.status, RunStatus::Completed);
    assert_eq!(out.entry(off.id).message.as_deref(), Some("Disabled"));
    assert!(out.run.messages[0].starts_with("Nothing to restore"));
    assert!(h.platform.launches().is_empty());
}

#[test]
fn corrupt_settings_cannot_stall_or_crash_a_restore() {
    let h = Harness::new();
    h.platform.add_rule(h.rule("app-ok", &["Fine"]));
    let mut ok = h.app("app-ok");
    ok.windows = vec![snapshot(
        "Fine",
        Rect::new(i32::MAX, i32::MIN, -5, i32::MAX),
        Some("?"),
    )];
    let mut ws = workspace(vec![ok.clone()]);
    ws.restore.max_parallel = 0;
    ws.restore.retry_count = u32::MAX;
    ws.monitors = vec![MonitorInfo {
        id: "?".into(),
        name: "Broken".into(),
        bounds: Rect::new(i32::MIN, i32::MIN, i32::MAX, i32::MAX),
        work_area: Rect::new(0, 0, 0, 0),
        scale_factor: f64::NAN,
        is_primary: true,
    }];
    let out = h.run(&ws, SwitchMode::Merge);
    assert_eq!(out.entry(ok.id).status, EntryStatus::Completed);
    let placed = h.platform.placements()[0].1.bounds;
    let screen = h.platform.monitors().unwrap()[0].work_area;
    assert!(
        placed.x >= screen.x
            && placed.y >= screen.y
            && placed.right() <= screen.right()
            && placed.bottom() <= screen.bottom(),
        "{placed:?} is on screen"
    );
}
