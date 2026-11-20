//! End-to-end tests on a real X11 desktop: arrange real windows → capture through the full
//! service (discovery, adapters, database) → close them → restore → verify the windows came back
//! where they were.
//!
//! Run with `scripts/e2e-x11.sh` (starts Xvfb + openbox). Skipped unless REFRAME_E2E_X11=1.
#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reframe_core::ReframeService;
use reframe_core::database::Database;
use reframe_core::model::*;
use reframe_core::restoration::RestoreTiming;
use reframe_platform::{LaunchRequest, Platform, Rect, WindowId, WindowInfo, WindowPlacement, WindowState};

fn enabled() -> bool {
    std::env::var("REFRAME_E2E_X11").as_deref() == Ok("1") && std::env::var_os("DISPLAY").is_some()
}

fn wait_for<T>(timeout: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let start = Instant::now();
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if start.elapsed() > timeout {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn near(a: Rect, b: Rect, tolerance: i32) -> bool {
    (a.x - b.x).abs() <= tolerance
        && (a.y - b.y).abs() <= tolerance
        && (a.width - b.width).abs() <= tolerance
        && (a.height - b.height).abs() <= tolerance
}

fn which(name: &str) -> PathBuf {
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .map(|dir| PathBuf::from(dir).join(name))
        .find(|p| p.is_file())
        .unwrap_or_else(|| panic!("{name} is required for the end-to-end tests"))
}

fn service(platform: Arc<dyn Platform>) -> ReframeService {
    ReframeService::new(platform, Database::open_in_memory().unwrap(), "e2e").with_timing(RestoreTiming {
        poll_interval: Duration::from_millis(100),
        settle: Duration::from_millis(800),
        close_wait: Duration::from_secs(5),
    })
}

/// Tests share one X display; run them one at a time.
static DISPLAY_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn restores_a_single_window() {
    round_trip(&[("reframe-e2e-solo", Rect::new(220, 140, 700, 460))]);
}

#[test]
fn restores_two_terminal_windows() {
    round_trip(&[
        ("reframe-e2e-alpha", Rect::new(60, 80, 640, 420)),
        ("reframe-e2e-beta", Rect::new(760, 160, 560, 380)),
    ]);
}

fn round_trip(specs: &[(&str, Rect)]) {
    if !enabled() {
        eprintln!("skipped: set REFRAME_E2E_X11=1 and DISPLAY (see scripts/e2e-x11.sh)");
        return;
    }
    let _guard = DISPLAY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let platform = reframe_platform::native();
    let xterm = which("xterm");

    // 1. Real windows with fixed titles (allowTitleOps off so the shell cannot rename them).
    for (title, _) in specs {
        platform
            .launch(&LaunchRequest::executable(&xterm).with_args(["-T", title, "-xrm", "XTerm*allowTitleOps: false"]))
            .expect("launch xterm");
    }
    let originals: Vec<WindowInfo> = wait_for(Duration::from_secs(15), || {
        let ws = platform.windows().ok()?;
        specs.iter().map(|(t, _)| ws.iter().find(|w| w.title == *t).cloned()).collect()
    })
    .expect("all xterm windows appear");

    // 2. Arrange them at known places.
    for (w, (_, rect)) in originals.iter().zip(specs) {
        platform.set_window_placement(&w.id, &WindowPlacement { bounds: *rect, state: WindowState::Normal }).unwrap();
    }
    wait_for(Duration::from_secs(5), || {
        let ws = platform.windows().ok()?;
        specs
            .iter()
            .all(|(t, rect)| ws.iter().any(|w| w.title == *t && near(w.bounds, *rect, 8)))
            .then_some(())
    })
    .expect("windows moved to the arranged positions");

    // 3. Capture exactly these windows through the real service.
    let svc = service(platform.clone());
    let snapshot = svc.scan_desktop().expect("scan desktop");
    let app = snapshot
        .applications
        .iter()
        .find(|a| a.windows.iter().any(|w| w.title == specs[0].0))
        .expect("xterm is listed in the snapshot")
        .clone();
    let window_ids: Vec<WindowId> = originals.iter().map(|w| w.id.clone()).collect();
    let captured = svc
        .capture_workspace(CaptureRequest {
            name: "End-to-end".into(),
            description: String::new(),
            icon: "🧪".into(),
            color: None,
            selections: vec![CaptureSelection { app_key: app.key.clone(), window_ids: window_ids.clone() }],
        })
        .expect("capture workspace");
    let ws = captured.workspace;
    assert_eq!(ws.applications.len(), 1, "one application captured: {ws:#?}");
    let entry = &ws.applications[0];
    assert_eq!(entry.windows.len(), specs.len(), "all windows captured: {entry:#?}");
    for saved in &entry.windows {
        assert!(specs.iter().any(|(_, r)| near(saved.bounds, *r, 8)), "saved geometry {saved:?}");
    }
    assert!(svc.list_workspaces().unwrap().iter().any(|s| s.id == ws.id), "workspace persisted");

    // 4. Close the originals gracefully and wait until they are gone.
    for id in &window_ids {
        platform.close_window(id).expect("close window");
    }
    wait_for(Duration::from_secs(10), || {
        let ws = platform.windows().ok()?;
        (!ws.iter().any(|w| window_ids.contains(&w.id))).then_some(())
    })
    .expect("original windows closed");
    let before: Vec<WindowId> = platform.windows().unwrap().into_iter().map(|w| w.id).collect();

    // 5. Restore and collect progress events.
    let events: Arc<Mutex<Vec<RestoreEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_events = Arc::clone(&events);
    svc.start_restore(
        RestoreRequest { workspace_id: ws.id, mode: SwitchMode::Merge, entry_ids: None, close_window_ids: vec![] },
        Arc::new(move |e| sink_events.lock().unwrap().push(e)),
    )
    .expect("start restore");
    let run = wait_for(Duration::from_secs(90), || {
        events.lock().unwrap().iter().find_map(|e| match e {
            RestoreEvent::RunFinished { run } => Some(run.clone()),
            _ => None,
        })
    })
    .expect("restore finished");
    {
        let events = events.lock().unwrap();
        assert!(matches!(events.first(), Some(RestoreEvent::RunStarted { .. })), "RunStarted comes first");
        assert!(matches!(events.last(), Some(RestoreEvent::RunFinished { .. })), "RunFinished comes last");
    }
    eprintln!("restore run: {:?} {:#?}", run.status, run.entries);

    // 6. New windows exist, placed where they were saved.
    let restored: Vec<WindowInfo> = platform
        .windows()
        .unwrap()
        .into_iter()
        .filter(|w| !before.contains(&w.id) && w.class_name.as_deref().is_some_and(|c| c.eq_ignore_ascii_case("xterm")))
        .collect();
    for w in &restored {
        eprintln!("restored window {:?} {:?} {:?}", w.title, w.bounds, w.state);
    }
    let all_back = restored.len() == specs.len()
        && specs.iter().all(|(_, target)| restored.iter().any(|w| near(w.bounds, *target, 12)));

    // Clean up before asserting so a failure doesn't leave windows behind for the next test.
    for w in &restored {
        let _ = platform.close_window(&w.id);
    }
    assert_eq!(run.status, RunStatus::Completed, "run: {run:#?}");
    assert!(all_back, "every window was restored at its saved position: {restored:#?}");
    assert_eq!(svc.active_workspace().unwrap(), Some(ws.id));
    assert!(svc.get_workspace(ws.id).unwrap().last_restored_at.is_some());
}
