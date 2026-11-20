//! Tests of the Win32 implementation against the real OS (or Wine).
//!
//! Tests that only query the OS run by default. Tests that start programs and create, move and
//! close real windows need a desktop session to themselves and run only with
//! `REFRAME_WINE_TESTS=1` (meant for Wine under Xvfb or a disposable Windows session); they run
//! one at a time.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, IDI_APPLICATION, LoadIconW,
    MSG, PM_REMOVE, PeekMessageW, RegisterClassW, SW_SHOWNOACTIVATE, ShowWindow, TranslateMessage,
    UnregisterClassW, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW,
};

use super::ffi::{hwnd, pcwstr, raw_hwnd, wide};
use super::logic::filter::{WS_EX_APPWINDOW, WS_EX_TOOLWINDOW, format_window_id, parse_window_id};
use super::*;

// ---------------------------------------------------------------------------------- helpers

fn desktop_tests_enabled() -> bool {
    std::env::var("REFRAME_WINE_TESTS").is_ok_and(|v| v == "1")
}

/// Wine's ntdll exports `wine_get_version`; Windows' does not. Only used to relax assertions
/// about documented Wine gaps, never by the implementation.
fn running_under_wine() -> bool {
    // SAFETY: plain module and export lookups.
    unsafe {
        GetModuleHandleW(windows::core::w!("ntdll.dll")).is_ok_and(|ntdll| {
            GetProcAddress(ntdll, windows::core::s!("wine_get_version")).is_some()
        })
    }
}

macro_rules! require_desktop {
    () => {
        if !desktop_tests_enabled() {
            eprintln!("skipped: set REFRAME_WINE_TESTS=1 to run desktop integration tests");
            return;
        }
    };
}

/// Desktop tests share one screen and the foreground window: run them one at a time.
static DESKTOP: Mutex<()> = Mutex::new(());

fn desktop() -> MutexGuard<'static, ()> {
    DESKTOP
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn wait_for<T>(timeout: Duration, mut probe: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(value) = probe() {
            return Some(value);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn windows_dir() -> PathBuf {
    std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("windir"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
}

fn notepad() -> PathBuf {
    windows_dir().join("System32").join("notepad.exe")
}

/// `%SystemRoot%\explorer.exe` carries a version resource on Windows and Wine alike (Wine's
/// notepad.exe has none).
fn explorer() -> PathBuf {
    windows_dir().join("explorer.exe")
}

fn decode_png(png_bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    let mut reader = decoder.read_info().expect("valid png");
    let mut buf = vec![0; reader.output_buffer_size().expect("buffer size")];
    let info = reader.next_frame(&mut buf).expect("png frame");
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

fn assert_visible_icon(png_bytes: &[u8], size: u32) {
    let (width, height, rgba) = decode_png(png_bytes);
    assert_eq!((width, height), (size, size));
    assert_eq!(rgba.len(), (size * size * 4) as usize);
    assert!(
        rgba.as_chunks::<4>().0.iter().any(|&[_, _, _, a]| a > 0),
        "icon is blank"
    );
}

fn placement(bounds: Rect, state: WindowState) -> WindowPlacement {
    WindowPlacement { bounds, state }
}

fn open_request(target: &str) -> LaunchRequest {
    LaunchRequest {
        target: LaunchTarget::Open {
            target: target.into(),
        },
        args: Vec::new(),
        working_directory: None,
        env: Vec::new(),
    }
}

/// Politely closes a window when a test ends, even if it failed half-way.
struct CloseOnDrop<'a> {
    platform: &'a WindowsPlatform,
    id: WindowId,
}

impl Drop for CloseOnDrop<'_> {
    fn drop(&mut self) {
        let _ = self.platform.close_window(&self.id);
    }
}

fn window_gone(platform: &WindowsPlatform, id: &WindowId) -> Option<()> {
    let windows = platform.windows().ok()?;
    windows.iter().all(|w| &w.id != id).then_some(())
}

// ------------------------------------------------------------------- OS queries (always run)

#[test]
fn info_reports_win32_with_full_capabilities() {
    let info = WindowsPlatform::new().info();
    assert_eq!(info.os, OsKind::Windows);
    assert_eq!(info.display_server, DisplayServer::Win32);
    assert!(!info.os_version.trim().is_empty());
    assert_eq!(info.desktop_environment, None);
    let c = &info.capabilities;
    for capability in [
        &c.list_windows,
        &c.read_window_geometry,
        &c.move_windows,
        &c.change_window_state,
        &c.close_windows,
        &c.focus_windows,
        &c.list_monitors,
        &c.process_details,
        &c.application_icons,
    ] {
        assert_eq!(capability.level, SupportLevel::Full);
    }
    assert!(
        c.move_windows
            .note
            .as_deref()
            .is_some_and(|n| n.contains("administrator"))
    );
    assert!(
        c.close_windows
            .note
            .as_deref()
            .is_some_and(|n| n.contains("administrator"))
    );
    assert!(
        c.list_windows
            .note
            .as_deref()
            .is_some_and(|n| n.contains("virtual desktop"))
    );
}

#[test]
fn no_permissions_are_needed() {
    let platform = WindowsPlatform::new();
    assert!(platform.permissions().is_empty());
    for kind in [
        PermissionKind::Accessibility,
        PermissionKind::ScreenRecording,
        PermissionKind::Automation,
    ] {
        assert!(
            platform
                .request_permission(kind)
                .unwrap_err()
                .is_unsupported()
        );
    }
}

#[test]
fn monitors_are_consistent() {
    let monitors = WindowsPlatform::new().monitors().expect("monitors");
    assert!(!monitors.is_empty());
    assert_eq!(monitors.iter().filter(|m| m.is_primary).count(), 1);
    let primary = &monitors[0];
    assert!(primary.is_primary, "primary monitor is listed first");
    assert_eq!((primary.bounds.x, primary.bounds.y), (0, 0));
    let mut ids = HashSet::new();
    for monitor in &monitors {
        assert!(!monitor.bounds.is_empty(), "{monitor:?}");
        assert!(!monitor.work_area.is_empty(), "{monitor:?}");
        assert_eq!(
            monitor.bounds.intersection(&monitor.work_area),
            Some(monitor.work_area),
            "work area inside the monitor: {monitor:?}"
        );
        assert!(monitor.scale_factor >= 1.0 && monitor.scale_factor <= 5.0);
        assert!(!monitor.name.trim().is_empty());
        assert!(
            ids.insert(monitor.id.clone()),
            "duplicate id {}",
            monitor.id
        );
    }
}

#[test]
fn window_listing_satisfies_the_contract() {
    let windows = WindowsPlatform::new().windows().expect("windows");
    let mut ids = HashSet::new();
    for (index, window) in windows.iter().enumerate() {
        assert_eq!(window.z_order as usize, index);
        assert!(ids.insert(window.id.clone()), "duplicate {}", window.id);
        assert!(parse_window_id(window.id.as_str()).is_some());
        assert!(!window.title.trim().is_empty());
        assert!(!window.bounds.is_empty(), "{window:?}");
        assert_eq!(window.desktop, None);
    }
    assert!(windows.iter().filter(|w| w.is_focused).count() <= 1);
}

#[test]
fn processes_include_this_process_without_package() {
    let processes = WindowsPlatform::new().processes().expect("processes");
    let me = processes
        .iter()
        .find(|p| p.pid == std::process::id())
        .expect("own process listed");
    assert!(me.executable_path.is_some());
    assert_eq!(me.package, None, "test binaries are not packaged");
    assert_eq!(process::package_app_id(0), None);
}

#[test]
fn executable_paths_of_processes() {
    let me = process::executable_path(std::process::id()).expect("own executable");
    let expected = std::env::current_exe().expect("current exe");
    assert_eq!(
        logic::locate::path_key(&me),
        logic::locate::path_key(&expected),
        "{me:?} vs {expected:?}"
    );
    assert_eq!(process::executable_path(0), None);
    // Process ids are multiples of 4 on Windows; this one is never assigned.
    assert_eq!(process::executable_path(u32::MAX - 2), None);
}

#[test]
fn executables_get_arguments_environment_and_working_directory() {
    let dir = std::env::temp_dir().join(format!("reframe launch test {}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let output = dir.join("output.txt");
    let _ = std::fs::remove_file(&output);
    let cmd = windows_dir().join("System32").join("cmd.exe");
    let launched = WindowsPlatform::new()
        .launch(&LaunchRequest {
            target: LaunchTarget::Executable { path: cmd.clone() },
            args: vec![
                "/d".into(),
                "/c".into(),
                "echo %REFRAME_LAUNCH_TEST%> output.txt".into(),
            ],
            working_directory: Some(dir.clone()),
            env: vec![("REFRAME_LAUNCH_TEST".into(), "launched by reframe".into())],
        })
        .expect("launch cmd.exe");
    assert!(launched.pid.is_some_and(|pid| pid != 0));
    let text = wait_for(Duration::from_secs(20), || {
        std::fs::read_to_string(&output)
            .ok()
            .filter(|text| text.contains("launched by reframe"))
    });
    let _ = std::fs::remove_dir_all(&dir);
    assert!(text.is_some(), "cmd.exe did not write {output:?}");

    let missing_directory = WindowsPlatform::new()
        .launch(
            &LaunchRequest::executable(cmd)
                .with_working_directory(Some(PathBuf::from(r"C:\definitely\missing"))),
        )
        .unwrap_err();
    assert!(
        matches!(&missing_directory, PlatformError::LaunchFailed { message, .. }
            if message.contains("working directory")),
        "{missing_directory:?}"
    );
}

#[test]
fn product_names_come_from_version_resources() {
    let name = process::product_name(&explorer());
    assert!(
        name.as_deref().is_some_and(|n| !n.trim().is_empty()),
        "{name:?}"
    );
    // Test binaries carry no version resource; missing files have none either.
    let me = std::env::current_exe().expect("current exe");
    assert_eq!(process::product_name(&me), None);
    assert_eq!(
        process::product_name(Path::new(r"C:\definitely\missing.exe")),
        None
    );
}

#[test]
fn locate_finds_programs_on_path_without_duplicates() {
    let platform = WindowsPlatform::new();
    let query = LocateQuery {
        executable_names: vec!["cmd".into(), "CMD.EXE".into()],
        ..Default::default()
    };
    let targets = platform.locate_application(&query);
    match targets.first() {
        Some(LaunchTarget::Executable { path }) => assert!(
            path.file_name()
                .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("cmd.exe")),
            "{path:?}"
        ),
        other => panic!("cmd.exe not found first: {other:?}"),
    }
    let labels: HashSet<String> = targets.iter().map(|t| t.label().to_lowercase()).collect();
    assert_eq!(labels.len(), targets.len(), "duplicates in {targets:?}");

    let missing = LocateQuery {
        executable_names: vec!["reframe-no-such-program-4711".into()],
        display_names: vec!["Reframe No Such Program 4711".into()],
        ..Default::default()
    };
    assert!(platform.locate_application(&missing).is_empty());
    assert!(
        platform
            .locate_application(&LocateQuery::default())
            .is_empty()
    );
}

#[test]
fn missing_app_paths_entries_are_ignored() {
    let env = |name: &str| std::env::var(name).ok();
    assert!(locate::app_path_entries("reframe-no-such-program-4711.exe", &env).is_empty());
}

#[test]
fn executable_icons_are_png_at_the_requested_size() {
    let platform = WindowsPlatform::new();
    for size in [16, 32, 48] {
        let png_bytes = platform
            .application_icon(&IconRequest {
                executable_path: Some(notepad()),
                size,
                ..Default::default()
            })
            .expect("icon lookup succeeds")
            .expect("notepad has an icon");
        assert_visible_icon(&png_bytes, size);
    }
    // Size 0 means "default".
    let default_size = platform
        .application_icon(&IconRequest {
            executable_path: Some(notepad()),
            size: 0,
            ..Default::default()
        })
        .expect("icon lookup succeeds")
        .expect("icon");
    assert_visible_icon(&default_size, super::logic::icon::DEFAULT_ICON_SIZE);
}

#[test]
fn absent_icons_are_none_not_errors() {
    let platform = WindowsPlatform::new();
    let missing_file = IconRequest {
        executable_path: Some(PathBuf::from(r"C:\definitely\missing.exe")),
        size: 32,
        ..Default::default()
    };
    assert_eq!(platform.application_icon(&missing_file), Ok(None));
    let unknown_package = IconRequest {
        package: Some(PackageInfo {
            kind: PackageKind::WindowsPackage,
            id: "Reframe.Test.Missing_0000000000000!App".into(),
            path: None,
        }),
        size: 32,
        ..Default::default()
    };
    assert_eq!(platform.application_icon(&unknown_package), Ok(None));
    let gone_window = IconRequest {
        window: Some(WindowId::new("0x7ffffff0")),
        size: 32,
        ..Default::default()
    };
    assert_eq!(platform.application_icon(&gone_window), Ok(None));
    assert_eq!(platform.application_icon(&IconRequest::default()), Ok(None));
}

#[test]
fn operations_on_missing_windows_are_not_found() {
    let platform = WindowsPlatform::new();
    let not_found =
        |result: PlatformResult<()>| matches!(result, Err(PlatformError::NotFound { .. }));
    for id in ["0x7ffffff0", "not a window", ""] {
        let id = WindowId::new(id);
        assert!(not_found(platform.close_window(&id)), "{id}");
        assert!(not_found(platform.focus_window(&id)), "{id}");
        assert!(
            not_found(platform.set_window_placement(
                &id,
                &placement(Rect::new(0, 0, 640, 480), WindowState::Normal)
            )),
            "{id}"
        );
    }
}

#[test]
fn empty_placements_are_rejected() {
    let err = WindowsPlatform::new()
        .set_window_placement(
            &WindowId::new("0x7ffffff0"),
            &placement(Rect::new(10, 10, 0, 300), WindowState::Normal),
        )
        .unwrap_err();
    assert!(matches!(err, PlatformError::Os { .. }), "{err:?}");
}

#[test]
fn launch_failures_are_reported() {
    let platform = WindowsPlatform::new();
    let launch_failed = |result: PlatformResult<LaunchedProcess>| {
        matches!(result, Err(PlatformError::LaunchFailed { .. }))
    };
    assert!(launch_failed(platform.launch(&LaunchRequest::executable(
        r"C:\definitely\missing.exe"
    ))));
    assert!(launch_failed(platform.launch(&open_request("   "))));
    assert!(launch_failed(platform.launch(&open_request(
        r"C:\definitely\missing\reframe-test-file.txt"
    ))));
    let bad_directory = platform
        .launch(&LaunchRequest {
            working_directory: Some(PathBuf::from(r"C:\definitely\missing")),
            ..open_request(r"C:\")
        })
        .unwrap_err();
    assert!(
        bad_directory.to_string().contains("working directory"),
        "{bad_directory}"
    );

    let package = |kind: PackageKind, id: &str| LaunchRequest {
        target: LaunchTarget::Package {
            package: PackageInfo {
                kind,
                id: id.into(),
                path: None,
            },
        },
        args: Vec::new(),
        working_directory: None,
        env: Vec::new(),
    };
    assert!(
        platform
            .launch(&package(PackageKind::Flatpak, "org.example.App"))
            .unwrap_err()
            .is_unsupported()
    );
    assert!(launch_failed(
        platform.launch(&package(PackageKind::WindowsPackage, " "))
    ));
    // Arguments cannot be passed through the shell fallback, so a failed activation is an error.
    let with_args = LaunchRequest {
        args: vec!["--flag".into()],
        ..package(
            PackageKind::WindowsPackage,
            "Reframe.Test.Missing_0000000000000!App",
        )
    };
    assert!(launch_failed(platform.launch(&with_args)));
}

#[test]
fn activating_unknown_apps_fails_cleanly() {
    assert!(launch::activate_application("Reframe.Test.Missing_0000000000000!App", "").is_err());
}

// ------------------------------------------------------- desktop tests (REFRAME_WINE_TESTS=1)

#[test]
fn notepad_lifecycle() {
    require_desktop!();
    let _desktop = desktop();
    let platform = WindowsPlatform::new();
    let launched = platform
        .launch(&LaunchRequest::executable(notepad()))
        .expect("launch notepad");
    let pid = launched.pid.expect("executables report their pid");
    let find = || platform.windows().ok()?.into_iter().find(|w| w.pid == pid);
    let window = wait_for(Duration::from_secs(30), find).expect("notepad window appears");
    let _cleanup = CloseOnDrop {
        platform: &platform,
        id: window.id.clone(),
    };
    assert!(
        window.title.to_lowercase().contains("notepad"),
        "{window:?}"
    );
    assert_eq!(window.class_name.as_deref(), Some("Notepad"));
    assert!(window.monitor_id.is_some(), "{window:?}");

    // The process is listed with its executable and the product name of that executable
    // (Notepad on Windows; Wine's notepad.exe has no version resource).
    let processes = platform.processes().expect("processes");
    let listed = processes
        .iter()
        .find(|p| p.pid == pid)
        .expect("notepad process");
    let executable = listed.executable_path.as_deref().expect("executable path");
    assert_eq!(
        listed.product_name,
        process::product_name(executable),
        "{listed:?}"
    );
    assert_eq!(listed.package, None);

    // Move and resize.
    let target = Rect::new(160, 140, 640, 420);
    platform
        .set_window_placement(&window.id, &placement(target, WindowState::Normal))
        .expect("move");
    let moved = wait_for(Duration::from_secs(10), || {
        find().filter(|w| w.bounds == target && w.state == WindowState::Normal)
    })
    .unwrap_or_else(|| panic!("window not moved to {target:?}: {:?}", find()));
    assert_eq!(moved.normal_bounds, Some(target));

    // Wine gap: neither GetWindowPlacement nor SetWindowPlacement supports other processes'
    // windows. Placement therefore takes the ShowWindowAsync/SetWindowPos fallback here, and the
    // reported restored frame is the current window rectangle, so the restored-frame checks of
    // maximized and minimized windows only hold on Windows. Under Wine they run for this process's
    // own windows in `own_windows_follow_the_filtering_contract_and_placement`.
    let restored_frame_reliable = !running_under_wine();

    // Maximize: the window fills its monitor's work area and keeps its restored frame. The
    // window thread sets the maximized style before it resizes the window, so the state alone
    // can be observed a moment before the geometry.
    platform
        .set_window_placement(&window.id, &placement(target, WindowState::Maximized))
        .expect("maximize");
    let monitors = platform.monitors().expect("monitors");
    let fills_its_monitor = |w: &WindowInfo| {
        monitors
            .iter()
            .find(|m| Some(&m.id) == w.monitor_id.as_ref())
            .is_some_and(|m| {
                m.work_area.area() * 9 <= w.bounds.intersection_area(&m.work_area) * 10
            })
    };
    let maximized = wait_for(Duration::from_secs(10), || {
        find().filter(|w| w.state == WindowState::Maximized && fills_its_monitor(w))
    })
    .unwrap_or_else(|| panic!("window not maximized: {:?} on {monitors:?}", find()));
    if restored_frame_reliable {
        assert_eq!(maximized.normal_bounds, Some(target));
    }

    // Minimize: reported with the restored frame.
    platform
        .set_window_placement(&window.id, &placement(target, WindowState::Minimized))
        .expect("minimize");
    wait_for(Duration::from_secs(10), || {
        find().filter(|w| {
            w.state == WindowState::Minimized
                && (!restored_frame_reliable
                    || (w.bounds == target && w.normal_bounds == Some(target)))
        })
    })
    .unwrap_or_else(|| panic!("window not minimized with frame {target:?}: {:?}", find()));

    // Restore, then focus.
    let restored_target = Rect::new(220, 180, 600, 400);
    platform
        .set_window_placement(&window.id, &placement(restored_target, WindowState::Normal))
        .expect("restore");
    wait_for(Duration::from_secs(10), || {
        find().filter(|w| w.state == WindowState::Normal && w.bounds == restored_target)
    })
    .unwrap_or_else(|| panic!("window not restored: {:?}", find()));
    platform.focus_window(&window.id).expect("focus");
    wait_for(Duration::from_secs(5), || find().filter(|w| w.is_focused))
        .unwrap_or_else(|| panic!("window not focused: {:?}", find()));

    // Icon from the window (under Wine, which cannot read other processes' window or class
    // icons, it comes from the window's executable).
    let icon = platform
        .application_icon(&IconRequest {
            window: Some(window.id.clone()),
            size: 32,
            ..Default::default()
        })
        .expect("icon lookup succeeds")
        .expect("notepad window icon");
    assert_visible_icon(&icon, 32);

    // Close gracefully and confirm it is gone.
    platform.close_window(&window.id).expect("close");
    wait_for(Duration::from_secs(15), || {
        window_gone(&platform, &window.id)
    })
    .expect("notepad window closed");
}

/// Windows created by this process on a helper thread that pumps their messages.
struct TestWindows {
    handles: HashMap<&'static str, isize>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

struct WindowSpec {
    label: &'static str,
    title: &'static str,
    style: u32,
    ex_style: u32,
    owner: Option<&'static str>,
    bounds: Rect,
    show: bool,
}

const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const WS_POPUP: u32 = 0x8000_0000;

unsafe extern "system" fn test_window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: forwards the message unchanged; DefWindowProc closes windows on WM_CLOSE.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

impl TestWindows {
    fn create(specs: Vec<WindowSpec>) -> Self {
        static CLASS_COUNTER: AtomicUsize = AtomicUsize::new(0);
        let class_name = format!(
            "ReframeTestWindow{}_{}",
            std::process::id(),
            CLASS_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let (sender, receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            let class = wide(&class_name).expect("class name");
            // SAFETY: standard window class registration and creation on this thread; every
            // window is destroyed and the class unregistered before the thread ends.
            unsafe {
                let instance = HINSTANCE::from(GetModuleHandleW(None).expect("module handle"));
                let window_class = WNDCLASSW {
                    lpfnWndProc: Some(test_window_proc),
                    hInstance: instance,
                    hIcon: LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
                    lpszClassName: pcwstr(&class),
                    ..Default::default()
                };
                assert_ne!(RegisterClassW(&window_class), 0, "RegisterClassW");
                let mut handles: HashMap<&'static str, isize> = HashMap::new();
                for spec in specs {
                    let title = wide(spec.title).expect("title");
                    let owner = spec
                        .owner
                        .and_then(|label| handles.get(label))
                        .map(|&h| hwnd(h));
                    let created = CreateWindowExW(
                        WINDOW_EX_STYLE(spec.ex_style),
                        pcwstr(&class),
                        pcwstr(&title),
                        WINDOW_STYLE(spec.style),
                        spec.bounds.x,
                        spec.bounds.y,
                        spec.bounds.width,
                        spec.bounds.height,
                        owner,
                        None,
                        Some(instance),
                        None,
                    )
                    .expect("CreateWindowExW");
                    if spec.show {
                        let _ = ShowWindow(created, SW_SHOWNOACTIVATE);
                    }
                    handles.insert(spec.label, raw_hwnd(created));
                }
                let _ = sender.send(handles.clone());
                let mut message = MSG::default();
                while !stop_flag.load(Ordering::SeqCst) {
                    while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                for handle in handles.values() {
                    let _ = DestroyWindow(hwnd(*handle));
                }
                let _ = UnregisterClassW(pcwstr(&class), Some(instance));
            }
        });
        let handles = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("test windows created");
        Self {
            handles,
            stop,
            thread: Some(thread),
        }
    }

    fn id(&self, label: &str) -> WindowId {
        WindowId::new(format_window_id(self.handles[label]))
    }
}

impl Drop for TestWindows {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[test]
fn own_windows_follow_the_filtering_contract_and_placement() {
    require_desktop!();
    let _desktop = desktop();
    let spec = |label, title, style, ex_style, owner, show| WindowSpec {
        label,
        title,
        style,
        ex_style,
        owner,
        bounds: Rect::new(300, 250, 420, 300),
        show,
    };
    let windows = TestWindows::create(vec![
        spec(
            "normal",
            "Reframe test: normal",
            WS_OVERLAPPEDWINDOW,
            0,
            None,
            true,
        ),
        spec(
            "tool",
            "Reframe test: tool",
            WS_OVERLAPPEDWINDOW,
            WS_EX_TOOLWINDOW,
            None,
            true,
        ),
        spec(
            "tool_app",
            "Reframe test: tool app",
            WS_OVERLAPPEDWINDOW,
            WS_EX_TOOLWINDOW | WS_EX_APPWINDOW,
            None,
            true,
        ),
        spec(
            "owned",
            "Reframe test: owned",
            WS_OVERLAPPEDWINDOW,
            0,
            Some("normal"),
            true,
        ),
        spec("untitled", "", WS_OVERLAPPEDWINDOW, 0, None, true),
        spec(
            "hidden",
            "Reframe test: hidden",
            WS_OVERLAPPEDWINDOW,
            0,
            None,
            false,
        ),
        WindowSpec {
            bounds: Rect::new(300, 250, 0, 0),
            ..spec("zero", "Reframe test: zero", WS_POPUP, 0, None, true)
        },
        spec("popup", "Reframe test: popup", WS_POPUP, 0, None, true),
    ]);
    let platform = WindowsPlatform::new();
    let listed = |label: &str| -> Option<WindowInfo> {
        let id = windows.id(label);
        platform.windows().ok()?.into_iter().find(|w| w.id == id)
    };

    let normal =
        wait_for(Duration::from_secs(10), || listed("normal")).expect("normal window listed");
    assert_eq!(normal.pid, std::process::id());
    assert_eq!(normal.title, "Reframe test: normal");
    assert!(
        normal
            .class_name
            .as_deref()
            .is_some_and(|c| c.starts_with("ReframeTestWindow")),
        "{normal:?}"
    );
    assert!(
        listed("tool_app").is_some(),
        "WS_EX_APPWINDOW tool windows are listed"
    );
    assert!(
        listed("popup").is_some(),
        "titled unowned popups are listed"
    );
    for label in ["tool", "owned", "untitled", "hidden", "zero"] {
        assert!(listed(label).is_none(), "{label} window must not be listed");
    }

    // Focusing an owned window credits its listed owner.
    platform
        .focus_window(&windows.id("owned"))
        .expect("focus owned window");
    wait_for(Duration::from_secs(5), || {
        listed("normal").filter(|w| w.is_focused)
    })
    .expect("owner reported as focused");

    // Placement round trip, including the states.
    let id = windows.id("normal");
    let target = Rect::new(120, 90, 500, 360);
    platform
        .set_window_placement(&id, &placement(target, WindowState::Normal))
        .expect("move own window");
    wait_for(Duration::from_secs(10), || {
        listed("normal").filter(|w| w.bounds == target && w.normal_bounds == Some(target))
    })
    .unwrap_or_else(|| panic!("own window not moved: {:?}", listed("normal")));
    // Wine gap (9.0): its SetWindowPlacement first moves a maximized window to the clamped
    // `ptMaxPosition` (the screen origin for the default -1,-1), and Wine treats moving a
    // maximized window as restoring it there, so minimizing straight from the maximized state
    // records a restored frame at the origin. Windows keeps the restored frame; the transition is
    // still exercised under Wine, only that one check is skipped.
    let mut previous = WindowState::Normal;
    for state in [
        WindowState::Maximized,
        WindowState::Minimized,
        WindowState::Normal,
        WindowState::Minimized,
        WindowState::Normal,
    ] {
        platform
            .set_window_placement(&id, &placement(target, state))
            .expect("change state");
        let wine_gap = running_under_wine()
            && previous == WindowState::Maximized
            && state == WindowState::Minimized;
        // The restored frame is kept in every state, and minimized windows report it as their
        // bounds. Geometry is part of the wait: a window's state can be observed a moment
        // before its window thread has moved it.
        let expected = |w: &WindowInfo| {
            w.state == state
                && (wine_gap
                    || (w.normal_bounds == Some(target)
                        && (state == WindowState::Maximized || w.bounds == target)))
        };
        wait_for(Duration::from_secs(10), || {
            listed("normal").filter(|w| expected(w))
        })
        .unwrap_or_else(|| {
            panic!(
                "own window not {state:?} with restored frame {target:?} after {previous:?}: {:?}",
                listed("normal")
            )
        });
        previous = state;
    }
    // Fullscreen requests maximize.
    platform
        .set_window_placement(&id, &placement(target, WindowState::Fullscreen))
        .expect("fullscreen request");
    wait_for(Duration::from_secs(10), || {
        listed("normal").filter(|w| w.state == WindowState::Maximized)
    })
    .expect("fullscreen request maximizes");

    // The class icon is used when the window has none of its own.
    let icon = platform
        .application_icon(&IconRequest {
            window: Some(id.clone()),
            size: 24,
            ..Default::default()
        })
        .expect("icon lookup succeeds")
        .expect("class icon");
    assert_visible_icon(&icon, 24);

    // Closing the owner closes it (and its owned window).
    platform.close_window(&id).expect("close own window");
    wait_for(Duration::from_secs(10), || window_gone(&platform, &id)).expect("own window closed");
}

#[test]
fn shell_open_passes_arguments_to_the_handler() {
    require_desktop!();
    let _desktop = desktop();
    let platform = WindowsPlatform::new();
    let dir = std::env::temp_dir().join(format!("reframe shell open {}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("reframe-shell-test.txt");
    std::fs::write(&file, "Opened by the Reframe test suite.\r\n").expect("write file");

    let before: HashSet<WindowId> = platform
        .windows()
        .expect("windows")
        .into_iter()
        .map(|w| w.id)
        .collect();
    // Programs are opened through the `exefile` association, which exists on every system
    // (a fresh Wine prefix has no `.txt` association). The file path contains spaces, so this
    // also checks argument quoting.
    let launched = platform
        .launch(&LaunchRequest {
            args: vec![file.to_string_lossy().into_owned()],
            working_directory: Some(dir.clone()),
            ..open_request(&notepad().to_string_lossy())
        })
        .expect("open notepad with the file");
    assert_eq!(
        launched.pid, None,
        "the shell does not report the handler's pid"
    );
    let window = wait_for(Duration::from_secs(30), || {
        platform
            .windows()
            .ok()?
            .into_iter()
            .find(|w| !before.contains(&w.id) && w.title.contains("reframe-shell-test"))
    })
    .expect("default editor shows the file");
    let _cleanup = CloseOnDrop {
        platform: &platform,
        id: window.id.clone(),
    };
    platform.close_window(&window.id).expect("close editor");
    wait_for(Duration::from_secs(15), || {
        window_gone(&platform, &window.id)
    })
    .expect("editor closed");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn app_paths_registry_is_consulted() {
    require_desktop!();
    // Wine registers Internet Explorer under App Paths (Windows does as well).
    let env = |name: &str| std::env::var(name).ok();
    let entries = locate::app_path_entries("iexplore.exe", &env);
    let registered = entries.first().expect("App Paths entry for iexplore.exe");
    assert!(registered.is_file());
    let targets = WindowsPlatform::new().locate_application(&LocateQuery {
        executable_names: vec!["iexplore".into()],
        ..Default::default()
    });
    let hits = targets
        .iter()
        .filter(|t| {
            t.label()
                .eq_ignore_ascii_case(&registered.to_string_lossy())
        })
        .count();
    assert_eq!(hits, 1, "{targets:?}");
}
