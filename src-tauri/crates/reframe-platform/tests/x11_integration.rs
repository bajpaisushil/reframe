//! Integration tests for `LinuxPlatform` against a real X server.
//!
//! Run them with `tests/run-x11-tests.sh`, which starts `Xvfb :91` (2560x1080) with two RandR
//! monitors (`LEFT` 1280x1080+0+0, primary, and `RIGHT` 1280x1080+1280+0), `Xft.dpi: 120`
//! and openbox, then runs this file with `REFRAME_X11_TESTS=1`. Without that variable every
//! test is skipped with a message. `REFRAME_X11_SECONDARY_DISPLAY` (e.g. `:92`) enables the
//! test that starts, kills and restarts its own bare X server (no window manager).
//!
//! The script also re-runs two tests in simulated Wayland sessions
//! (`REFRAME_SESSION_MODE=xwayland`: `XDG_SESSION_TYPE=wayland` with the X display reachable;
//! `REFRAME_SESSION_MODE=wayland`: `XDG_SESSION_TYPE=wayland` without `DISPLAY`); in a plain
//! run those two skip.
//!
//! Tests run one at a time (they share the display and the input focus).

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use reframe_platform::linux::LinuxPlatform;
use reframe_platform::*;

static SERIAL: Mutex<()> = Mutex::new(());
const WINDOW_TIMEOUT: Duration = Duration::from_secs(15);
/// xterm resizes in character cells (WM_NORMAL_HINTS increments); allow one cell or so.
const SIZE_TOLERANCE: i32 = 16;

fn session_mode() -> String {
    std::env::var("REFRAME_SESSION_MODE").unwrap_or_else(|_| "x11".to_string())
}

/// Gate for the X11 tests (`REFRAME_X11_TESTS=1`, plain X11 session mode).
fn enabled(test: &str) -> Option<MutexGuard<'static, ()>> {
    enabled_in(test, "x11")
}

fn enabled_in(test: &str, mode: &str) -> Option<MutexGuard<'static, ()>> {
    if std::env::var("REFRAME_X11_TESTS").as_deref() != Ok("1") {
        eprintln!(
            "skipping {test}: X11 integration tests need REFRAME_X11_TESTS=1 \
             (run crates/reframe-platform/tests/run-x11-tests.sh)"
        );
        return None;
    }
    if session_mode() != mode {
        eprintln!("skipping {test}: only runs with REFRAME_SESSION_MODE={mode}");
        return None;
    }
    Some(SERIAL.lock().unwrap_or_else(|e| e.into_inner()))
}

fn wait_until<T>(timeout: Duration, mut probe: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(value) = probe() {
            return Some(value);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn unique_title(tag: &str) -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    format!(
        "reframe-it-{}-{tag}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

fn find_window(platform: &LinuxPlatform, title: &str) -> Option<WindowInfo> {
    platform
        .windows()
        .ok()?
        .into_iter()
        .find(|w| w.title == title)
}

/// Launch `xterm -T <title> -e sleep 600` through the platform and wait for its window.
fn launch_xterm(platform: &LinuxPlatform, tag: &str, env: &[(&str, &str)]) -> (u32, WindowInfo) {
    let title = unique_title(tag);
    let mut request =
        LaunchRequest::executable("xterm").with_args(["-T", &title, "-e", "sleep", "600"]);
    request.env = env
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let launched = platform.launch(&request).expect("launch xterm");
    let pid = launched.pid.expect("xterm pid");
    let window = wait_until(WINDOW_TIMEOUT, || find_window(platform, &title))
        .unwrap_or_else(|| panic!("window {title:?} did not appear"));
    (pid, window)
}

fn process_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
        && std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .map(|s| !s.contains(") Z "))
            .unwrap_or(false)
}

/// Close a window politely and wait for it (and its process) to go away.
fn close_and_wait(platform: &LinuxPlatform, window: &WindowInfo, pid: u32) {
    platform.close_window(&window.id).expect("close_window");
    let gone = wait_until(WINDOW_TIMEOUT, || {
        let windows = platform.windows().ok()?;
        (!windows.iter().any(|w| w.id == window.id)).then_some(())
    });
    assert!(gone.is_some(), "window {} did not close", window.title);
    assert!(
        wait_until(WINDOW_TIMEOUT, || (!process_alive(pid)).then_some(())).is_some(),
        "xterm {pid} did not exit after its window closed"
    );
}

fn command_output(program: &str, args: &[&str], display: &str) -> String {
    let out = Command::new(program)
        .args(args)
        .env("DISPLAY", display)
        .output()
        .unwrap_or_else(|e| panic!("run {program}: {e}"));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn display() -> String {
    std::env::var("DISPLAY").expect("DISPLAY must be set by run-x11-tests.sh")
}

/// Outer frame bounds computed independently with `xwininfo` and `xprop`.
fn xwininfo_outer(id: &WindowId) -> Rect {
    let info = command_output("xwininfo", &["-id", id.as_str()], &display());
    let field = |label: &str| -> i32 {
        info.lines()
            .find_map(|l| l.trim().strip_prefix(label))
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or_else(|| panic!("xwininfo output lacks {label}: {info}"))
    };
    let (x, y) = (
        field("Absolute upper-left X:"),
        field("Absolute upper-left Y:"),
    );
    let (w, h) = (field("Width:"), field("Height:"));
    let border = field("Border width:");
    let extents = command_output(
        "xprop",
        &["-id", id.as_str(), "_NET_FRAME_EXTENTS"],
        &display(),
    );
    let e: Vec<i32> = extents
        .split('=')
        .nth(1)
        .map(|v| v.split(',').filter_map(|n| n.trim().parse().ok()).collect())
        .unwrap_or_default();
    let (l, r, t, b) = match e[..] {
        [l, r, t, b] => (l, r, t, b),
        _ => (0, 0, 0, 0),
    };
    Rect::new(x - l, y - t, w + 2 * border + l + r, h + 2 * border + t + b)
}

fn assert_close_size(actual: Rect, expected: Rect) {
    assert!(
        (actual.width - expected.width).abs() <= SIZE_TOLERANCE
            && (actual.height - expected.height).abs() <= SIZE_TOLERANCE,
        "size {}x{} not within {SIZE_TOLERANCE}px of {}x{}",
        actual.width,
        actual.height,
        expected.width,
        expected.height
    );
}

fn place(platform: &LinuxPlatform, id: &WindowId, bounds: Rect, state: WindowState) {
    platform
        .set_window_placement(id, &WindowPlacement { bounds, state })
        .unwrap_or_else(|e| panic!("set_window_placement {bounds:?} {state:?}: {e}"));
}

/// Wait until the window satisfies `check`, returning its last state.
fn wait_for_window(
    platform: &LinuxPlatform,
    id: &WindowId,
    what: &str,
    check: impl Fn(&WindowInfo) -> bool,
) -> WindowInfo {
    let mut last = None;
    let found = wait_until(WINDOW_TIMEOUT, || {
        let w = platform.windows().ok()?.into_iter().find(|w| &w.id == id)?;
        let ok = check(&w);
        last = Some(w.clone());
        ok.then_some(w)
    });
    found.unwrap_or_else(|| panic!("window never became {what}; last seen: {last:#?}"))
}

#[test]
fn info_reports_x11_session() {
    let Some(_guard) = enabled("info_reports_x11_session") else {
        return;
    };
    let platform = LinuxPlatform::new();
    let info = platform.info();
    eprintln!("info: {info:?}");
    assert_eq!(info.os, OsKind::Linux);
    assert_eq!(info.display_server, DisplayServer::X11);
    assert_eq!(info.desktop_environment.as_deref(), Some("Openbox"));
    let caps = &info.capabilities;
    for cap in [
        &caps.list_windows,
        &caps.read_window_geometry,
        &caps.move_windows,
        &caps.change_window_state,
        &caps.close_windows,
        &caps.focus_windows,
        &caps.list_monitors,
    ] {
        assert_eq!(cap.level, SupportLevel::Full, "{cap:?}");
    }
    assert!(platform.permissions().is_empty());
}

#[test]
fn monitors_match_randr_layout() {
    let Some(_guard) = enabled("monitors_match_randr_layout") else {
        return;
    };
    let monitors = LinuxPlatform::new().monitors().expect("monitors");
    eprintln!("monitors: {monitors:?}");
    assert_eq!(monitors.len(), 2, "{monitors:#?}");
    let left = monitors.iter().find(|m| m.id == "LEFT").expect("LEFT");
    let right = monitors.iter().find(|m| m.id == "RIGHT").expect("RIGHT");
    assert_eq!(left.bounds, Rect::new(0, 0, 1280, 1080));
    assert_eq!(right.bounds, Rect::new(1280, 0, 1280, 1080));
    assert!(left.is_primary);
    assert!(!right.is_primary);
    assert_eq!(left.name, "LEFT");
    // No panels: the work area is the whole monitor.
    assert_eq!(left.work_area, left.bounds);
    assert_eq!(right.work_area, right.bounds);
    // run-x11-tests.sh sets Xft.dpi: 120.
    assert_eq!(left.scale_factor, 1.25);
    assert_eq!(right.scale_factor, 1.25);
}

#[test]
fn xterm_window_lifecycle() {
    let Some(_guard) = enabled("xterm_window_lifecycle") else {
        return;
    };
    let platform = LinuxPlatform::new();
    let (pid, window) = launch_xterm(&platform, "life", &[]);
    // Let the window manager finish placing the new window before comparing geometry.
    std::thread::sleep(Duration::from_millis(200));
    let window = find_window(&platform, &window.title).expect("xterm still listed");
    eprintln!("launched xterm {pid}: {window:?}");

    // Identity and geometry as reported by windows().
    assert_eq!(window.pid, pid, "pid of {window:#?}");
    assert_eq!(window.class_name.as_deref(), Some("XTerm"));
    assert_eq!(window.app_id_hint.as_deref(), Some("xterm"));
    assert_eq!(window.state, WindowState::Normal);
    assert_eq!(window.normal_bounds, Some(window.bounds));
    assert_eq!(window.desktop, Some(0));
    assert!(window.id.as_str().starts_with("0x"));
    assert_eq!(
        window.bounds,
        xwininfo_outer(&window.id),
        "outer frame bounds"
    );
    assert!(window.monitor_id.is_some());

    // Move + resize on the left monitor.
    let target = Rect::new(100, 120, 640, 400);
    place(&platform, &window.id, target, WindowState::Normal);
    let moved = wait_for_window(&platform, &window.id, "moved", |w| {
        w.bounds.x == target.x && w.bounds.y == target.y
    });
    eprintln!("moved to {target:?}: {:?}", moved.bounds);
    assert_close_size(moved.bounds, target);
    assert_eq!(moved.bounds, xwininfo_outer(&window.id));
    assert_eq!(moved.monitor_id.as_deref(), Some("LEFT"));
    assert_eq!(moved.state, WindowState::Normal);

    // Move to the right monitor.
    let right = Rect::new(1400, 100, 700, 450);
    place(&platform, &window.id, right, WindowState::Normal);
    let on_right = wait_for_window(&platform, &window.id, "on RIGHT", |w| {
        w.bounds.x == right.x && w.bounds.y == right.y
    });
    assert_eq!(on_right.monitor_id.as_deref(), Some("RIGHT"));
    assert_close_size(on_right.bounds, right);

    // Maximize: fills (most of) the right monitor, no normal bounds.
    place(&platform, &window.id, right, WindowState::Maximized);
    let maximized = wait_for_window(&platform, &window.id, "maximized", |w| {
        w.state == WindowState::Maximized
    });
    eprintln!(
        "maximized: {:?} on {:?}",
        maximized.bounds, maximized.monitor_id
    );
    assert_eq!(maximized.monitor_id.as_deref(), Some("RIGHT"));
    assert_eq!(maximized.normal_bounds, None);
    let monitor = Rect::new(1280, 0, 1280, 1080);
    assert!(
        monitor.intersection_area(&maximized.bounds) * 10 >= monitor.area() * 9,
        "maximized bounds {:?} should cover the RIGHT monitor",
        maximized.bounds
    );

    // Minimize: still listed, reported as minimized.
    place(&platform, &window.id, right, WindowState::Minimized);
    let minimized = wait_for_window(&platform, &window.id, "minimized", |w| {
        w.state == WindowState::Minimized
    });
    assert_eq!(minimized.normal_bounds, None);

    // Restore to normal on the left monitor.
    let restored_target = Rect::new(200, 150, 700, 450);
    place(&platform, &window.id, restored_target, WindowState::Normal);
    let restored = wait_for_window(&platform, &window.id, "restored", |w| {
        w.state == WindowState::Normal
            && w.bounds.x == restored_target.x
            && w.bounds.y == restored_target.y
    });
    assert_close_size(restored.bounds, restored_target);
    assert_eq!(restored.monitor_id.as_deref(), Some("LEFT"));
    assert_eq!(restored.normal_bounds, Some(restored.bounds));

    // Full screen and back.
    place(
        &platform,
        &window.id,
        restored_target,
        WindowState::Fullscreen,
    );
    let fullscreen = wait_for_window(&platform, &window.id, "fullscreen", |w| {
        w.state == WindowState::Fullscreen
    });
    eprintln!("fullscreen: {:?}", fullscreen.bounds);
    assert_eq!(fullscreen.bounds, Rect::new(0, 0, 1280, 1080));
    place(&platform, &window.id, restored_target, WindowState::Normal);
    wait_for_window(&platform, &window.id, "normal again", |w| {
        w.state == WindowState::Normal && w.bounds.x == restored_target.x
    });

    // Focus: a second xterm takes the focus, then focus the first one again.
    let (pid2, second) = launch_xterm(&platform, "focus", &[]);
    platform.focus_window(&second.id).expect("focus second");
    wait_for_window(&platform, &second.id, "focused", |w| {
        w.is_focused && w.z_order == 0
    });
    platform.focus_window(&window.id).expect("focus first");
    let focused = wait_for_window(&platform, &window.id, "focused again", |w| {
        w.is_focused && w.z_order == 0
    });
    assert!(focused.is_focused);
    let windows = platform.windows().unwrap();
    eprintln!(
        "after focus: {:?}",
        windows
            .iter()
            .map(|w| (&w.title, w.z_order, w.is_focused))
            .collect::<Vec<_>>()
    );
    let other = windows.iter().find(|w| w.id == second.id).unwrap();
    assert!(!other.is_focused);
    assert!(other.z_order > focused.z_order);
    assert_eq!(windows.iter().filter(|w| w.is_focused).count(), 1);
    // Focusing a minimized window restores it.
    place(&platform, &second.id, second.bounds, WindowState::Minimized);
    wait_for_window(&platform, &second.id, "minimized", |w| {
        w.state == WindowState::Minimized
    });
    platform.focus_window(&second.id).expect("focus minimized");
    wait_for_window(&platform, &second.id, "restored by focus", |w| {
        w.state == WindowState::Normal && w.is_focused
    });

    // Graceful close: windows and processes go away.
    close_and_wait(&platform, &window, pid);
    close_and_wait(&platform, &second, pid2);

    // Operations on a window that no longer exists report NotFound.
    let gone = platform.set_window_placement(
        &window.id,
        &WindowPlacement {
            bounds: target,
            state: WindowState::Normal,
        },
    );
    assert!(
        matches!(gone, Err(PlatformError::NotFound { .. })),
        "{gone:?}"
    );
    assert!(matches!(
        platform.close_window(&window.id),
        Err(PlatformError::NotFound { .. })
    ));
    assert!(matches!(
        platform.focus_window(&window.id),
        Err(PlatformError::NotFound { .. })
    ));
}

/// Send an EWMH client message about `window` to the root window of `$DISPLAY`.
fn ewmh_message(message: &str, window: u32, data: [u32; 5]) {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{ClientMessageEvent, ConnectionExt as _, EventMask};
    let x = TestWindows::new();
    let root = x.conn.setup().roots[0].root;
    let event = ClientMessageEvent::new(32, window, x.atom(message), data);
    x.conn
        .send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        )
        .unwrap()
        .check()
        .unwrap();
}

fn current_desktop() -> Option<u32> {
    command_output("xprop", &["-root", "_NET_CURRENT_DESKTOP"], &display())
        .split('=')
        .nth(1)
        .and_then(|v| v.trim().parse().ok())
}

/// Switches back to the first desktop when dropped, so a failing test leaves no trace.
struct BackToFirstDesktop;

impl Drop for BackToFirstDesktop {
    fn drop(&mut self) {
        ewmh_message("_NET_CURRENT_DESKTOP", 0, [0, 0, 0, 0, 0]);
        let _ = wait_until(WINDOW_TIMEOUT, || {
            (current_desktop() == Some(0)).then_some(())
        });
    }
}

/// xterm stores its `-T` title only as `COMPOUND_TEXT` (no `_NET_WM_NAME`); in a UTF-8 locale
/// Xlib writes characters outside Latin-1, such as an em dash, as a UTF-8 segment.
#[test]
fn compound_text_titles_with_non_latin1_characters() {
    let Some(_guard) = enabled("compound_text_titles_with_non_latin1_characters") else {
        return;
    };
    let platform = LinuxPlatform::new();
    // `launch_xterm` finds the window by its exact title, so a misdecoded title fails here.
    let (pid, window) = launch_xterm(&platform, "Grüße — ok", &[("LC_ALL", "C.UTF-8")]);
    assert!(window.title.contains("Grüße — ok"), "{:?}", window.title);
    close_and_wait(&platform, &window, pid);
}

/// Window managers put windows on other desktops into ICCCM IconicState (openbox, mutter):
/// they are not minimized, and arranging them must not switch the user's desktop.
#[test]
fn windows_on_other_desktops() {
    let Some(_guard) = enabled("windows_on_other_desktops") else {
        return;
    };
    let _reset = BackToFirstDesktop;
    let platform = LinuxPlatform::new();
    let (pid, window) = launch_xterm(&platform, "desktop", &[]);
    let id = u32::from_str_radix(window.id.as_str().trim_start_matches("0x"), 16).unwrap();
    assert_eq!(current_desktop(), Some(0));
    // openbox activates a new window shortly after mapping it, which also brings it to the
    // current desktop: wait for that, and repeat the request if it was undone anyway.
    wait_for_window(&platform, &window.id, "focused", |w| w.is_focused);
    let elsewhere = wait_until(WINDOW_TIMEOUT, || {
        ewmh_message("_NET_WM_DESKTOP", id, [1, 2, 0, 0, 0]);
        wait_until(Duration::from_millis(500), || {
            let windows = platform.windows().ok()?;
            windows
                .into_iter()
                .find(|w| w.id == window.id && w.desktop == Some(1))
        })
    })
    .expect("window moved to desktop 1");
    assert_eq!(elsewhere.state, WindowState::Normal, "{elsewhere:#?}");
    assert_eq!(elsewhere.normal_bounds, Some(elsewhere.bounds));

    // Moving it keeps it (and the user) where they are.
    let target = Rect::new(220, 140, 600, 380);
    place(&platform, &window.id, target, WindowState::Normal);
    let moved = wait_for_window(&platform, &window.id, "moved on desktop 1", |w| {
        w.bounds.x == target.x && w.bounds.y == target.y
    });
    assert_eq!(moved.desktop, Some(1));
    assert_eq!(moved.state, WindowState::Normal);
    assert_eq!(current_desktop(), Some(0), "the current desktop changed");

    // A minimized window stays minimized on another desktop (openbox ignores requests to
    // minimize windows on other desktops, so minimize it here first).
    ewmh_message("_NET_WM_DESKTOP", id, [0, 2, 0, 0, 0]);
    wait_for_window(&platform, &window.id, "back on desktop 0", |w| {
        w.desktop == Some(0)
    });
    place(&platform, &window.id, target, WindowState::Minimized);
    wait_for_window(&platform, &window.id, "minimized", |w| {
        w.state == WindowState::Minimized
    });
    ewmh_message("_NET_WM_DESKTOP", id, [1, 2, 0, 0, 0]);
    let minimized = wait_for_window(&platform, &window.id, "moved to desktop 1", |w| {
        w.desktop == Some(1)
    });
    assert_eq!(minimized.state, WindowState::Minimized, "{minimized:#?}");
    assert_eq!(minimized.normal_bounds, None);
    assert_eq!(current_desktop(), Some(0), "the current desktop changed");

    close_and_wait(&platform, &window, pid);
}

#[test]
fn icons_processes_and_locate() {
    let Some(_guard) = enabled("icons_processes_and_locate") else {
        return;
    };
    let platform = LinuxPlatform::new();
    let (pid, window) = launch_xterm(&platform, "icon", &[]);

    // _NET_WM_ICON is converted to PNG (set a 2x2 icon: red, green, blue, translucent white).
    let set = Command::new("xprop")
        .args([
            "-id",
            window.id.as_str(),
            "-f",
            "_NET_WM_ICON",
            "32c",
            "-set",
            "_NET_WM_ICON",
            "2,2,4294901760,4278255360,4278190335,2164260863",
        ])
        .env("DISPLAY", display())
        .status()
        .expect("xprop");
    assert!(set.success());
    let png = platform
        .application_icon(&IconRequest {
            window: Some(window.id.clone()),
            size: 32,
            ..Default::default()
        })
        .expect("icon")
        .expect("window icon");
    assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
    let decoder = png::Decoder::new(std::io::Cursor::new(&png));
    let mut reader = decoder.read_info().expect("valid PNG");
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut pixels).unwrap();
    assert_eq!((frame.width, frame.height), (2, 2));
    assert_eq!(
        &pixels[..16],
        &[
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 128
        ]
    );

    // Icon from the executable (desktop entry / hicolor theme), when the system has one.
    let xterm_path = which_xterm();
    let theme_icon = Path::new("/usr/share/icons/hicolor/48x48/apps/xterm.png");
    let from_exe = platform
        .application_icon(&IconRequest {
            executable_path: Some(xterm_path.clone()),
            size: 48,
            ..Default::default()
        })
        .expect("icon lookup");
    if theme_icon.exists() {
        let png = from_exe.expect("xterm icon from the hicolor theme");
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
    }

    // processes(): the xterm process is there, with its desktop-entry name when installed.
    let processes = platform.processes().expect("processes");
    let xterm = processes
        .iter()
        .find(|p| p.pid == pid)
        .expect("xterm process");
    assert_eq!(xterm.name, "xterm");
    assert!(xterm.command_line.iter().any(|a| a == &window.title));
    if Path::new("/usr/share/applications/debian-xterm.desktop").exists() {
        assert_eq!(xterm.product_name.as_deref(), Some("XTerm"));
    }
    assert_eq!(xterm.package, None);

    // locate_application finds xterm on PATH and through its desktop entry.
    let by_name = platform.locate_application(&LocateQuery {
        executable_names: vec!["xterm".into()],
        ..Default::default()
    });
    assert!(
        by_name.iter().any(
            |t| matches!(t, LaunchTarget::Executable { path } if same_file(path, &xterm_path))
        ),
        "{by_name:?}"
    );
    let by_display = platform.locate_application(&LocateQuery {
        display_names: vec!["XTerm".into()],
        ..Default::default()
    });
    if Path::new("/usr/share/applications/debian-xterm.desktop").exists() {
        assert!(
            matches!(by_display.first(), Some(LaunchTarget::Executable { path }) if same_file(path, &xterm_path)),
            "{by_display:?}"
        );
        let by_id = platform.locate_application(&LocateQuery {
            desktop_ids: vec!["debian-xterm.desktop".into()],
            ..Default::default()
        });
        assert!(
            matches!(by_id.first(), Some(LaunchTarget::Executable { path }) if same_file(path, &xterm_path)),
            "{by_id:?}"
        );
    }
    // No duplicates even when every hint matches the same program.
    let all = platform.locate_application(&LocateQuery {
        executable_names: vec!["xterm".into()],
        desktop_ids: vec!["debian-xterm.desktop".into()],
        display_names: vec!["XTerm".into()],
        ..Default::default()
    });
    let xterms = all
        .iter()
        .filter(|t| matches!(t, LaunchTarget::Executable { path } if same_file(path, &xterm_path)))
        .count();
    assert_eq!(xterms, 1, "{all:?}");
    assert!(
        platform
            .locate_application(&LocateQuery {
                executable_names: vec!["reframe-definitely-not-installed".into()],
                ..Default::default()
            })
            .is_empty()
    );

    close_and_wait(&platform, &window, pid);
}

/// Raw X11 test windows created directly with x11rb, destroyed on drop.
struct TestWindows {
    conn: x11rb::rust_connection::RustConnection,
    windows: Vec<u32>,
}

impl TestWindows {
    fn new() -> Self {
        Self::on(None)
    }

    /// Test windows on `display` (`None`: `$DISPLAY`).
    fn on(display: Option<&str>) -> Self {
        let (conn, _) = x11rb::connect(display).expect("connect to the test X server");
        Self {
            conn,
            windows: Vec::new(),
        }
    }

    fn atom(&self, name: &str) -> u32 {
        use x11rb::protocol::xproto::ConnectionExt as _;
        self.conn
            .intern_atom(false, name.as_bytes())
            .unwrap()
            .reply()
            .unwrap()
            .atom
    }

    /// Create a top-level window titled `title`; `setup` sets properties before mapping.
    fn create(
        &mut self,
        title: &str,
        size: (u16, u16),
        override_redirect: bool,
        map: bool,
        setup: impl FnOnce(&Self, u32),
    ) -> u32 {
        use x11rb::connection::Connection as _;
        use x11rb::protocol::xproto::{
            AtomEnum, ConnectionExt as _, CreateWindowAux, PropMode, WindowClass,
        };
        use x11rb::wrapper::ConnectionExt as _;
        let screen = &self.conn.setup().roots[0];
        let (root, white) = (screen.root, screen.white_pixel);
        let id = self.conn.generate_id().unwrap();
        self.conn
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                id,
                root,
                50,
                50,
                size.0,
                size.1,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new()
                    .background_pixel(white)
                    .override_redirect(u32::from(override_redirect)),
            )
            .unwrap();
        self.conn
            .change_property8(
                PropMode::REPLACE,
                id,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                title.as_bytes(),
            )
            .unwrap();
        self.conn
            .change_property8(
                PropMode::REPLACE,
                id,
                AtomEnum::WM_CLASS,
                AtomEnum::STRING,
                b"reframe-test\0ReframeTest\0",
            )
            .unwrap();
        setup(self, id);
        if map {
            self.conn.map_window(id).unwrap();
        }
        self.conn.flush().unwrap();
        self.windows.push(id);
        id
    }

    fn set_atoms(&self, window: u32, property: &str, values: &[&str]) {
        use x11rb::protocol::xproto::{AtomEnum, PropMode};
        use x11rb::wrapper::ConnectionExt as _;
        let atoms: Vec<u32> = values.iter().map(|v| self.atom(v)).collect();
        self.conn
            .change_property32(
                PropMode::REPLACE,
                window,
                self.atom(property),
                AtomEnum::ATOM,
                &atoms,
            )
            .unwrap();
    }
}

impl Drop for TestWindows {
    fn drop(&mut self) {
        use x11rb::connection::Connection as _;
        use x11rb::protocol::xproto::ConnectionExt as _;
        for &w in &self.windows {
            let _ = self.conn.destroy_window(w);
        }
        let _ = self.conn.flush();
    }
}

#[test]
fn filters_non_application_windows() {
    let Some(_guard) = enabled("filters_non_application_windows") else {
        return;
    };
    use x11rb::protocol::xproto::{AtomEnum, PropMode};
    use x11rb::wrapper::ConnectionExt as _;
    let platform = LinuxPlatform::new();
    let tag = unique_title("filter");
    let title = |kind: &str| format!("{tag}-{kind}");
    let mut x = TestWindows::new();

    let unicode_title = format!("{tag} – ünïcode ✓");
    let normal = x.create(&title("normal"), (300, 200), false, true, |x, w| {
        let utf8 = x.atom("UTF8_STRING");
        x.conn
            .change_property8(
                PropMode::REPLACE,
                w,
                x.atom("_NET_WM_NAME"),
                utf8,
                unicode_title.as_bytes(),
            )
            .unwrap();
        x.conn
            .change_property8(
                PropMode::REPLACE,
                w,
                x.atom("_GTK_APPLICATION_ID"),
                utf8,
                b"org.reframe.Test",
            )
            .unwrap();
    });
    x.create(&title("dialog"), (200, 100), false, true, |x, w| {
        x.set_atoms(w, "_NET_WM_WINDOW_TYPE", &["_NET_WM_WINDOW_TYPE_DIALOG"]);
    });
    // A 30px panel at the top of the LEFT monitor.
    x.create(&title("dock"), (1280, 30), false, true, |x, w| {
        x.set_atoms(w, "_NET_WM_WINDOW_TYPE", &["_NET_WM_WINDOW_TYPE_DOCK"]);
        x.conn
            .change_property32(
                PropMode::REPLACE,
                w,
                x.atom("_NET_WM_STRUT_PARTIAL"),
                AtomEnum::CARDINAL,
                &[0, 0, 30, 0, 0, 0, 0, 0, 0, 1279, 0, 0],
            )
            .unwrap();
    });
    x.create(&title("utility"), (200, 100), false, true, |x, w| {
        x.set_atoms(w, "_NET_WM_WINDOW_TYPE", &["_NET_WM_WINDOW_TYPE_UTILITY"]);
    });
    x.create(
        &title("vendor-then-splash"),
        (200, 100),
        false,
        true,
        |x, w| {
            x.set_atoms(
                w,
                "_NET_WM_WINDOW_TYPE",
                &["_REFRAME_UNKNOWN_TYPE", "_NET_WM_WINDOW_TYPE_SPLASH"],
            );
        },
    );
    x.create(&title("skip-taskbar"), (200, 100), false, true, |x, w| {
        x.set_atoms(w, "_NET_WM_STATE", &["_NET_WM_STATE_SKIP_TASKBAR"]);
    });
    x.create(&title("override"), (200, 100), true, true, |_, _| {});
    x.create(&title("unmapped"), (200, 100), false, false, |_, _| {});
    x.create(&title("tiny"), (1, 1), false, true, |_, _| {});

    let ours = || -> Vec<WindowInfo> {
        platform
            .windows()
            .unwrap_or_default()
            .into_iter()
            .filter(|w| w.title.starts_with(&tag))
            .collect()
    };
    wait_until(WINDOW_TIMEOUT, || (ours().len() >= 2).then_some(())).expect("test windows listed");
    // Give the window manager time to manage everything, then look again.
    std::thread::sleep(Duration::from_millis(300));
    let listed = ours();
    let mut titles: Vec<&str> = listed.iter().map(|w| w.title.as_str()).collect();
    titles.sort();
    let mut expected = vec![unicode_title.clone(), title("dialog")];
    expected.sort();
    assert_eq!(titles, expected, "only application windows are listed");

    let normal_info = listed
        .iter()
        .find(|w| w.title == unicode_title)
        .expect("normal window");
    assert_eq!(normal_info.id.as_str(), format!("{normal:#x}"));
    // No _NET_WM_PID: the pid comes from the X-Resource extension (this test process owns the
    // X connection that created the window).
    assert_eq!(normal_info.pid, std::process::id());
    assert_eq!(normal_info.class_name.as_deref(), Some("ReframeTest"));
    assert_eq!(normal_info.app_id_hint.as_deref(), Some("org.reframe.Test"));
    assert_eq!(normal_info.state, WindowState::Normal);

    // The dock's strut shrinks only the LEFT monitor's work area.
    let monitors = wait_until(WINDOW_TIMEOUT, || {
        let monitors = platform.monitors().ok()?;
        let left = monitors.iter().find(|m| m.id == "LEFT")?;
        (left.work_area != left.bounds).then_some(monitors)
    })
    .expect("work area reflects the panel strut");
    eprintln!("monitors with a panel: {monitors:?}");
    let left = monitors.iter().find(|m| m.id == "LEFT").unwrap();
    let right = monitors.iter().find(|m| m.id == "RIGHT").unwrap();
    assert_eq!(left.work_area, Rect::new(0, 30, 1280, 1050));
    assert_eq!(right.work_area, right.bounds);

    // Clean up and wait until the panel is gone for the following tests.
    drop(x);
    let restored = wait_until(WINDOW_TIMEOUT, || {
        let monitors = platform.monitors().ok()?;
        monitors
            .iter()
            .all(|m| m.work_area == m.bounds)
            .then_some(())
    });
    assert!(
        restored.is_some(),
        "work area restored after the panel closed"
    );
}

fn which_xterm() -> PathBuf {
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join("xterm"))
                .find(|p| p.is_file())
        })
        .expect("xterm on PATH")
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// An X server owned by a test; terminated (SIGTERM) when dropped.
struct OwnedXvfb {
    child: Option<Child>,
    display: String,
}

impl OwnedXvfb {
    fn start(display: &str) -> Self {
        Self::start_sized(display, "1600x900")
    }

    /// Start Xvfb with a `WIDTHxHEIGHT` screen (also the largest size RandR can set later).
    fn start_sized(display: &str, size: &str) -> Self {
        let number = display.trim_start_matches(':');
        let socket = PathBuf::from(format!("/tmp/.X11-unix/X{number}"));
        assert!(
            !socket.exists(),
            "{display} is already in use; refusing to touch another X server"
        );
        let child = Command::new("Xvfb")
            .args([
                display,
                "-screen",
                "0",
                &format!("{size}x24"),
                "-nolisten",
                "tcp",
                "-noreset",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start Xvfb");
        let server = Self {
            child: Some(child),
            display: display.to_string(),
        };
        let ready = wait_until(Duration::from_secs(10), || {
            Command::new("xdpyinfo")
                .env("DISPLAY", display)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .ok()
                .filter(|s| s.success())
        });
        assert!(ready.is_some(), "Xvfb {display} did not start");
        server
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = Command::new("kill")
                .args(["-TERM", &child.id().to_string()])
                .status();
            let stopped = wait_until(Duration::from_secs(10), || child.try_wait().ok().flatten());
            if stopped.is_none() {
                // Our own test server ignoring SIGTERM: do not leave it behind.
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        let number = self.display.trim_start_matches(':');
        let _ = wait_until(Duration::from_secs(5), || {
            (!Path::new(&format!("/tmp/.X11-unix/X{number}")).exists()).then_some(())
        });
    }
}

impl Drop for OwnedXvfb {
    fn drop(&mut self) {
        self.stop();
    }
}

#[test]
fn bare_x_server_and_reconnect() {
    let Some(_guard) = enabled("bare_x_server_and_reconnect") else {
        return;
    };
    let Ok(display) = std::env::var("REFRAME_X11_SECONDARY_DISPLAY") else {
        eprintln!("skipping bare_x_server_and_reconnect: REFRAME_X11_SECONDARY_DISPLAY not set");
        return;
    };
    let mut server = OwnedXvfb::start(&display);
    let platform = LinuxPlatform::for_display(display.clone());

    // No window manager at all: reduced capabilities, but everything works without panics.
    let info = platform.info();
    assert_eq!(info.display_server, DisplayServer::X11);
    assert_eq!(info.capabilities.list_windows.level, SupportLevel::Partial);
    assert_eq!(info.capabilities.close_windows.level, SupportLevel::Full);
    let monitors = platform.monitors().expect("monitors");
    eprintln!("bare server: {info:?}\n{monitors:?}");
    assert_eq!(monitors.len(), 1);
    assert_eq!(monitors[0].bounds, Rect::new(0, 0, 1600, 900));
    assert!(monitors[0].is_primary);
    assert_eq!(monitors[0].scale_factor, 1.0);

    // Applications launched through this platform go to its display.
    let (pid, window) = launch_xterm(&platform, "bare", &[]);
    assert_eq!(window.pid, pid);
    assert_eq!(window.class_name.as_deref(), Some("XTerm"));
    assert_eq!(window.monitor_id.as_deref(), Some(monitors[0].id.as_str()));

    // Without a window manager ConfigureWindow moves the window directly.
    let target = Rect::new(300, 200, 500, 300);
    place(&platform, &window.id, target, WindowState::Normal);
    let moved = wait_for_window(&platform, &window.id, "moved", |w| {
        w.bounds.x == target.x && w.bounds.y == target.y
    });
    assert_close_size(moved.bounds, target);

    // Maximizing needs a window manager: Unsupported, but the window was still moved.
    let target2 = Rect::new(40, 50, 500, 300);
    let err = platform
        .set_window_placement(
            &window.id,
            &WindowPlacement {
                bounds: target2,
                state: WindowState::Maximized,
            },
        )
        .unwrap_err();
    assert!(err.is_unsupported(), "{err:?}");
    wait_for_window(&platform, &window.id, "moved again", |w| {
        w.bounds.x == target2.x && w.bounds.y == target2.y
    });

    platform.focus_window(&window.id).expect("focus");
    wait_for_window(&platform, &window.id, "focused", |w| w.is_focused);

    // WM_DELETE_WINDOW goes straight to the client.
    close_and_wait(&platform, &window, pid);

    // The X server dies: errors, no panic.
    server.stop();
    let err = platform.windows().unwrap_err();
    eprintln!("X server gone: {err}");
    assert!(matches!(err, PlatformError::Os { .. }), "{err:?}");
    assert!(platform.monitors().is_err());
    let caps = platform.info().capabilities;
    assert_eq!(caps.list_windows.level, SupportLevel::Unsupported);

    // It comes back: the platform reconnects on its own.
    let server = OwnedXvfb::start(&display);
    let windows = platform
        .windows()
        .expect("windows after the X server restarted");
    assert!(windows.is_empty());
    assert_eq!(platform.monitors().expect("monitors").len(), 1);
    drop(server);
}

/// A helper process owned by a test (a window manager); terminated (SIGTERM) when dropped.
struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .args(["-TERM", &self.0.id().to_string()])
            .status();
        if wait_until(Duration::from_secs(10), || self.0.try_wait().ok().flatten()).is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn start_openbox(display: &str) -> OwnedChild {
    let child = Command::new("openbox")
        .arg("--sm-disable")
        .env("DISPLAY", display)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start openbox");
    let wm = OwnedChild(child);
    // openbox claims the screen (_NET_SUPPORTING_WM_CHECK) before it has finished starting;
    // windows mapped before _NET_CLIENT_LIST exists may never be managed.
    let ready = wait_until(Duration::from_secs(10), || {
        let root = command_output(
            "xprop",
            &["-root", "_NET_SUPPORTING_WM_CHECK", "_NET_CLIENT_LIST"],
            display,
        );
        (root.contains("_NET_SUPPORTING_WM_CHECK(WINDOW)")
            && root.contains("_NET_CLIENT_LIST(WINDOW)"))
        .then_some(())
    });
    assert!(ready.is_some(), "openbox did not start on {display}");
    wm
}

/// Run `xrandr` on `display`. Xvfb cannot re-enable its output after some screen size
/// changes and xrandr then reports an error; only the screen size and monitors matter here.
fn xrandr(display: &str, args: &[&str]) {
    let _ = Command::new("xrandr")
        .args(args)
        .env("DISPLAY", display)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Current root window size according to `xdpyinfo`.
fn screen_size(display: &str) -> (i32, i32) {
    let info = command_output("xdpyinfo", &[], display);
    info.lines()
        .find_map(|l| l.trim().strip_prefix("dimensions:"))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|dims| dims.split_once('x'))
        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        .unwrap_or_else(|| panic!("xdpyinfo output lacks the screen size: {info}"))
}

/// The screen grows after the platform connected (a monitor was plugged in): panel struts on
/// the right/bottom edges are measured from the *current* screen edges.
#[test]
fn work_area_follows_screen_resize() {
    let Some(_guard) = enabled("work_area_follows_screen_resize") else {
        return;
    };
    let Ok(display) = std::env::var("REFRAME_X11_SECONDARY_DISPLAY") else {
        eprintln!(
            "skipping work_area_follows_screen_resize: REFRAME_X11_SECONDARY_DISPLAY not set"
        );
        return;
    };
    use x11rb::protocol::xproto::{AtomEnum, PropMode};
    use x11rb::wrapper::ConnectionExt as _;
    let _server = OwnedXvfb::start_sized(&display, "2560x1080");
    xrandr(&display, &["--fb", "1920x1080"]);
    assert_eq!(screen_size(&display), (1920, 1080));
    let platform = LinuxPlatform::for_display(display.clone());
    let before = platform.monitors().expect("monitors before the resize");
    eprintln!("1920 pixels wide: {before:?}");

    xrandr(&display, &["--fb", "2560x1080"]);
    assert_eq!(screen_size(&display), (2560, 1080));
    xrandr(
        &display,
        &["--setmonitor", "LEFT", "1280/338x1080/286+0+0", "none"],
    );
    xrandr(
        &display,
        &["--setmonitor", "RIGHT", "1280/338x1080/286+1280+0", "none"],
    );
    let _wm = start_openbox(&display);

    // A 48px panel along the right edge of the screen, i.e. on the RIGHT monitor.
    let mut x = TestWindows::on(Some(&display));
    let panel = x.create("reframe-it-right-panel", (48, 1080), false, true, |x, w| {
        x.set_atoms(w, "_NET_WM_WINDOW_TYPE", &["_NET_WM_WINDOW_TYPE_DOCK"]);
        x.conn
            .change_property32(
                PropMode::REPLACE,
                w,
                x.atom("_NET_WM_STRUT_PARTIAL"),
                AtomEnum::CARDINAL,
                &[0, 48, 0, 0, 0, 0, 0, 1079, 0, 0, 0, 0],
            )
            .unwrap();
    });
    let managed = wait_until(WINDOW_TIMEOUT, || {
        command_output("xprop", &["-root", "_NET_CLIENT_LIST"], &display)
            .contains(&format!("{panel:#x}"))
            .then_some(())
    });
    assert!(managed.is_some(), "openbox did not manage the panel");
    let monitors = wait_until(WINDOW_TIMEOUT, || {
        let monitors = platform.monitors().ok()?;
        let right = monitors.iter().find(|m| m.id == "RIGHT")?;
        (right.work_area != right.bounds).then_some(monitors)
    })
    .expect("the panel strut reduces the RIGHT monitor's work area");
    eprintln!("2560 pixels wide with a right panel: {monitors:?}");
    let left = monitors.iter().find(|m| m.id == "LEFT").expect("LEFT");
    let right = monitors.iter().find(|m| m.id == "RIGHT").expect("RIGHT");
    assert_eq!(right.bounds, Rect::new(1280, 0, 1280, 1080));
    assert_eq!(right.work_area, Rect::new(1280, 0, 1232, 1080));
    assert_eq!(left.work_area, left.bounds);
}

const WAYLAND_REASON: &str = "Wayland compositors do not allow applications to inspect or \
                              arrange other applications' windows";

#[test]
fn session_xwayland() {
    let Some(_guard) = enabled_in("session_xwayland", "xwayland") else {
        return;
    };
    let platform = LinuxPlatform::new();
    let info = platform.info();
    eprintln!("simulated XWayland session: {info:?}");
    assert_eq!(info.display_server, DisplayServer::XWayland);
    let caps = &info.capabilities;
    for cap in [
        &caps.list_windows,
        &caps.read_window_geometry,
        &caps.move_windows,
        &caps.change_window_state,
        &caps.close_windows,
        &caps.focus_windows,
    ] {
        assert_eq!(cap.level, SupportLevel::Partial, "{cap:?}");
        assert!(cap.note.as_deref().unwrap_or("").contains("XWayland"));
    }
    assert_eq!(caps.list_monitors.level, SupportLevel::Partial);
    // X11 clients remain visible and controllable.
    let (pid, window) = launch_xterm(&platform, "xwayland", &[]);
    assert_eq!(window.pid, pid);
    let target = Rect::new(150, 160, 600, 400);
    place(&platform, &window.id, target, WindowState::Normal);
    wait_for_window(&platform, &window.id, "moved", |w| {
        w.bounds.x == target.x && w.bounds.y == target.y
    });
    assert_eq!(platform.monitors().expect("monitors").len(), 2);
    close_and_wait(&platform, &window, pid);
}

#[test]
fn session_wayland_only() {
    let Some(_guard) = enabled_in("session_wayland_only", "wayland") else {
        return;
    };
    assert!(
        std::env::var_os("DISPLAY").is_none(),
        "DISPLAY must be unset"
    );
    let platform = LinuxPlatform::new();
    let info = platform.info();
    eprintln!("simulated Wayland session: {info:?}");
    assert_eq!(info.display_server, DisplayServer::Wayland);
    let unsupported = |result: PlatformResult<()>| match result {
        Err(PlatformError::Unsupported { reason, .. }) => reason,
        other => panic!("expected Unsupported, got {other:?}"),
    };
    assert_eq!(unsupported(platform.windows().map(|_| ())), WAYLAND_REASON);
    let window = WindowId::new("0x400001");
    let placement = WindowPlacement {
        bounds: Rect::new(0, 0, 400, 300),
        state: WindowState::Normal,
    };
    assert_eq!(
        unsupported(platform.set_window_placement(&window, &placement)),
        WAYLAND_REASON
    );
    assert_eq!(unsupported(platform.close_window(&window)), WAYLAND_REASON);
    assert_eq!(unsupported(platform.focus_window(&window)), WAYLAND_REASON);
    assert!(platform.monitors().unwrap_err().is_unsupported());
    for cap in [
        &info.capabilities.list_windows,
        &info.capabilities.move_windows,
        &info.capabilities.close_windows,
        &info.capabilities.focus_windows,
    ] {
        assert_eq!(cap.level, SupportLevel::Unsupported);
    }
    // Launching, processes and application lookup still work.
    let launched = platform
        .launch(&LaunchRequest::executable("/bin/sh").with_args(["-c", "exit 0"]))
        .expect("launch");
    assert!(launched.pid.is_some());
    let me = std::process::id();
    assert!(platform.processes().unwrap().iter().any(|p| p.pid == me));
    let found = platform.locate_application(&LocateQuery {
        executable_names: vec!["xterm".into()],
        ..Default::default()
    });
    assert!(!found.is_empty());
}
