//! End-to-end: a restore started through `ReframeService` runs the engine on a background
//! thread, streams its events, places windows and persists the finished run.

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use reframe_core::ReframeService;
use reframe_core::adapters::AdapterRegistry;
use reframe_core::database::Database;
use reframe_core::model::{
    ApplicationEntry, EntryStatus, NewWorkspace, RestoreEvent, RestoreRequest, RunStatus,
    SwitchMode, WindowSnapshot,
};
use reframe_core::restoration::{EventSink, RestoreTiming};
use reframe_platform::mock::{LaunchRule, MockPlatform, MockWindowSpec};
use reframe_platform::{Platform, Rect, WindowState};

#[test]
fn restore_started_through_the_service() {
    let platform = MockPlatform::new();
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("app-service-test");
    std::fs::write(&exe, "#!/bin/sh\n").unwrap();
    platform.add_rule(
        LaunchRule::new("app-service-test")
            .executable(&exe)
            .window(MockWindowSpec::titled("Service test")),
    );
    let svc = ReframeService::with_adapters(
        Arc::new(platform.clone()),
        Arc::new(AdapterRegistry::with_adapters(Vec::new())),
        Database::open_in_memory().unwrap(),
        "test",
    )
    .with_timing(RestoreTiming {
        poll_interval: Duration::from_millis(5),
        settle: Duration::from_millis(50),
        close_wait: Duration::from_millis(200),
    });

    let mut ws = svc
        .create_workspace(NewWorkspace {
            name: "Service".into(),
            description: String::new(),
            icon: "🧪".into(),
            color: None,
        })
        .unwrap();
    let mut entry = ApplicationEntry::new("exe:app-service-test", "Service test", "generic");
    entry.launch.executable = Some(exe.clone());
    let saved = Rect::new(10, 20, 640, 480);
    entry.windows = vec![WindowSnapshot {
        title: "Service test".into(),
        class_name: None,
        bounds: saved,
        state: WindowState::Normal,
        monitor_id: Some("MOCK-1".into()),
    }];
    ws.applications = vec![entry.clone()];
    ws.restore.launch_delay_ms = 0;
    let ws = svc.update_workspace(ws).unwrap();

    let (tx, rx) = mpsc::channel();
    let sink: Arc<EventSink> = Arc::new(move |event: RestoreEvent| {
        let _ = tx.send(event);
    });
    let run_id = svc
        .start_restore(
            RestoreRequest {
                workspace_id: ws.id,
                mode: SwitchMode::Merge,
                entry_ids: None,
                close_window_ids: Vec::new(),
            },
            sink,
        )
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut first = None;
    let finished = loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(RestoreEvent::RunFinished { run }) => break run,
            Ok(event) => {
                first.get_or_insert(event);
            }
            Err(error) => panic!("the restore did not finish: {error}"),
        }
    };
    assert!(matches!(first, Some(RestoreEvent::RunStarted { .. })));
    assert_eq!(finished.id, run_id);
    assert_eq!(finished.status, RunStatus::Completed);
    assert_eq!(finished.entries[0].status, EntryStatus::Completed);
    assert_eq!(finished.entries[0].windows_restored, 1);

    // The service thread persists the run and unregisters it right after RunFinished.
    while !svc.active_runs().is_empty() {
        assert!(Instant::now() < deadline, "the run was never unregistered");
        std::thread::sleep(Duration::from_millis(5));
    }
    let stored = svc.get_run(run_id).unwrap();
    assert_eq!(stored.status, RunStatus::Completed);
    assert_eq!(stored.entries, finished.entries);
    assert_eq!(svc.restore_history(Some(ws.id), 10).unwrap()[0].id, run_id);
    assert_eq!(svc.active_workspace().unwrap(), Some(ws.id));
    let window = platform
        .windows()
        .unwrap()
        .into_iter()
        .find(|w| w.title == "Service test")
        .unwrap();
    assert_eq!(window.bounds, saved);
}
