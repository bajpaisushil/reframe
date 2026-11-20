//! Capture and manual-entry tests against the in-memory platform.

use std::path::{Path, PathBuf};

use reframe_platform::mock::MockPlatform;
use reframe_platform::{
    LaunchTarget, OsKind, PackageInfo, PackageKind, Platform, Rect, WindowId, WindowState,
};

use super::*;
use crate::discovery::test_support::*;
use crate::model::{EntryOptions, MAX_WINDOWS_PER_APP};

fn select(key: &str, windows: &[&WindowId]) -> CaptureSelection {
    CaptureSelection {
        app_key: key.to_string(),
        window_ids: windows.iter().map(|id| (*id).clone()).collect(),
    }
}

fn capture_with(
    p: &MockPlatform,
    adapters: &AdapterRegistry,
    selections: &[CaptureSelection],
    options: &DiscoveryOptions,
) -> CaptureResult {
    let state = discovery::gather(p);
    capture_entries(&state, p, adapters, selections, options).expect("capture succeeds")
}

fn capture(p: &MockPlatform, selections: &[CaptureSelection]) -> CaptureResult {
    capture_with(p, &generic_only(), selections, &options())
}

/// Key of the application owning `pid` in a fresh scan.
fn key_of(p: &MockPlatform, pid: u32) -> String {
    scan(p, &generic_only(), &options())
        .applications
        .into_iter()
        .find(|a| a.pids.contains(&pid))
        .map(|a| a.key)
        .expect("application is listed")
}

/// A mock desktop that reports Linux whatever the host OS (keys and rules depend on the OS).
fn linux() -> MockPlatform {
    let p = MockPlatform::new();
    set_os(&p, OsKind::Linux);
    p
}

fn titles(entry: &ApplicationEntry) -> Vec<&str> {
    entry.windows.iter().map(|w| w.title.as_str()).collect()
}

#[test]
fn captures_every_window_of_a_selected_application() {
    let p = MockPlatform::new();
    let pid = p.add_process("gimp", Some("/usr/bin/gimp".into()));
    p.add_window(pid, "Image 1", Some("Gimp"), Rect::new(10, 20, 800, 600));
    window(&p, pid, "Image 2");

    let result = capture(&p, &[select(&key_of(&p, pid), &[])]);
    assert_eq!(result.entries.len(), 1);
    let entry = &result.entries[0];
    assert_eq!(entry.app_id, "exe:gimp");
    assert_eq!(entry.display_name, "Gimp");
    assert_eq!(entry.adapter_id, "generic");
    assert!(entry.enabled);
    assert!(entry.depends_on.is_empty());
    assert_eq!(entry.options, EntryOptions::default());
    assert_eq!(entry.state, AppState::None);
    assert_eq!(
        entry.launch.executable,
        Some(PathBuf::from("/usr/bin/gimp"))
    );
    assert!(entry.launch.arguments.is_empty());
    assert_eq!(titles(entry), vec!["Image 2", "Image 1"]);
    let first = &entry.windows[1];
    assert_eq!(first.class_name.as_deref(), Some("Gimp"));
    assert_eq!(first.bounds, Rect::new(10, 20, 800, 600));
    assert_eq!(first.state, WindowState::Normal);
    assert_eq!(first.monitor_id.as_deref(), Some("MOCK-1"));
    assert_eq!(result.monitors, p.monitors().unwrap());
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
}

#[test]
fn only_selected_windows_and_their_processes_reach_the_adapter() {
    let p = MockPlatform::new();
    let main = p.add_process("code", Some("/usr/share/code/code".into()));
    let second = p.add_process("code", Some("/usr/share/code/code".into()));
    window(&p, main, "a — reframe");
    let b = window(&p, second, "b — notes");
    window(&p, main, "c — other");
    let adapter = TestAdapter::new("code");
    let adapters = registry(vec![adapter.clone()]);
    let key = key_of(&p, main);

    let result = capture_with(&p, &adapters, &[select(&key, &[&b])], &options());
    let entry = &result.entries[0];
    assert_eq!(entry.adapter_id, "test");
    assert_eq!(entry.app_id, "test-code");
    assert_eq!(entry.display_name, "Test App");
    assert_eq!(entry.state, test_state());
    assert_eq!(titles(entry), vec!["b — notes"]);
    assert_eq!(
        entry.launch.executable,
        Some(PathBuf::from("/usr/share/code/code"))
    );

    let seen = adapter.seen();
    assert_eq!(
        seen.len(),
        1,
        "capture runs the adapter once per application"
    );
    assert_eq!(seen[0].windows, vec![b]);
    assert_eq!(seen[0].pids, vec![second]);
    assert_eq!(seen[0].all_processes, 2);
}

#[test]
fn browser_tab_capture_setting_reaches_the_adapter() {
    let p = MockPlatform::new();
    let pid = p.add_process("code", Some("/usr/share/code/code".into()));
    window(&p, pid, "main.rs");
    let adapter = TestAdapter::new("code");
    let adapters = registry(vec![adapter.clone()]);
    let key = key_of(&p, pid);
    for allowed in [true, false] {
        let opts = DiscoveryOptions {
            browser_tab_capture: allowed,
            ..options()
        };
        capture_with(&p, &adapters, &[select(&key, &[])], &opts);
        assert_eq!(adapter.seen().last().unwrap().browser_tab_capture, allowed);
    }
}

#[test]
fn closed_windows_and_applications_are_reported() {
    let p = MockPlatform::new();
    let gimp = p.add_process("gimp", Some("/usr/bin/gimp".into()));
    let keep = window(&p, gimp, "keep");
    let gone = window(&p, gimp, "gone");
    let xterm = p.add_process("xterm", Some("/usr/bin/xterm".into()));
    let shell = window(&p, xterm, "shell");
    let gimp_key = key_of(&p, gimp);
    let xterm_key = key_of(&p, xterm);

    p.close_window(&gone).unwrap();
    p.close_window(&shell).unwrap(); // xterm exits with its last window

    let result = capture(
        &p,
        &[select(&gimp_key, &[&keep, &gone]), select(&xterm_key, &[])],
    );
    assert_eq!(result.entries.len(), 1);
    assert_eq!(titles(&result.entries[0]), vec!["keep"]);
    assert_eq!(
        result.warnings,
        vec![
            "Gimp: 1 window closed since the scan.".to_string(),
            "Xterm is no longer running.".to_string(),
        ]
    );
}

#[test]
fn an_application_whose_selected_windows_all_closed_is_still_captured() {
    let p = MockPlatform::new();
    let gimp = p.add_process("gimp", Some("/usr/bin/gimp".into()));
    let a = window(&p, gimp, "a");
    let b = window(&p, gimp, "b");
    window(&p, gimp, "c");
    let key = key_of(&p, gimp);
    p.close_window(&a).unwrap();
    p.close_window(&b).unwrap();

    let result = capture(&p, &[select(&key, &[&a, &b])]);
    assert_eq!(result.entries.len(), 1);
    assert!(result.entries[0].windows.is_empty());
    assert_eq!(
        result.entries[0].launch.executable,
        Some(PathBuf::from("/usr/bin/gimp"))
    );
    assert_eq!(
        result.warnings,
        vec![
            "Gimp: 2 windows closed since the scan; only the application itself was saved."
                .to_string()
        ]
    );
}

#[test]
fn selecting_an_application_twice_creates_one_entry() {
    let p = MockPlatform::new();
    let gimp = p.add_process("gimp", Some("/usr/bin/gimp".into()));
    let a = window(&p, gimp, "a");
    let b = window(&p, gimp, "b");
    window(&p, gimp, "c");
    let xterm = p.add_process("xterm", Some("/usr/bin/xterm".into()));
    window(&p, xterm, "shell");
    let gimp_key = key_of(&p, gimp);
    let xterm_key = key_of(&p, xterm);

    let result = capture(
        &p,
        &[
            select(&gimp_key, &[&a]),
            select(&xterm_key, &[]),
            select(&gimp_key, &[&b, &a, &a]),
        ],
    );
    let apps: Vec<&str> = result.entries.iter().map(|e| e.app_id.as_str()).collect();
    assert_eq!(apps, vec!["exe:gimp", "exe:xterm"]);
    assert_eq!(titles(&result.entries[0]), vec!["b", "a"]);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);

    // An empty window list means every window, also when combined with a subset.
    let result = capture(&p, &[select(&gimp_key, &[&a]), select(&gimp_key, &[])]);
    assert_eq!(result.entries.len(), 1);
    assert_eq!(titles(&result.entries[0]), vec!["c", "b", "a"]);
}

#[test]
fn entries_follow_the_selection_order() {
    let p = MockPlatform::new();
    let xterm = p.add_process("xterm", Some("/usr/bin/xterm".into()));
    window(&p, xterm, "shell");
    let gimp = p.add_process("gimp", Some("/usr/bin/gimp".into()));
    window(&p, gimp, "image"); // frontmost
    let result = capture(
        &p,
        &[
            select(&key_of(&p, xterm), &[]),
            select(&key_of(&p, gimp), &[]),
        ],
    );
    let apps: Vec<&str> = result.entries.iter().map(|e| e.app_id.as_str()).collect();
    assert_eq!(apps, vec!["exe:xterm", "exe:gimp"]);
}

#[test]
fn adapter_warnings_are_prefixed_with_the_application_name() {
    let p = MockPlatform::new();
    let code = p.add_process("code", Some("/usr/share/code/code".into()));
    window(&p, code, "main.rs");
    let notes = p.add_process("notes", None);
    window(&p, notes, "todo");
    let adapters = registry(vec![TestAdapter::with_warning(
        "code",
        "Could not read the session file.",
    )]);

    let result = capture_with(
        &p,
        &adapters,
        &[
            select(&key_of(&p, code), &[]),
            select(&key_of(&p, notes), &[]),
        ],
        &options(),
    );
    assert_eq!(
        result.warnings,
        vec![
            "Test App: Could not read the session file.".to_string(),
            // Already starts with the name: not prefixed twice.
            "Notes: the executable path is not accessible; choose it in the workspace before restoring."
                .to_string(),
        ]
    );
}

#[test]
fn a_panicking_capture_falls_back_to_a_generic_entry() {
    let p = MockPlatform::new();
    let code = p.add_process("code", Some("/usr/share/code/code".into()));
    window(&p, code, "main.rs");
    let adapters = registry(vec![PanicAdapter::new(PanicAt::Capture, "code")]);

    let result = capture_with(&p, &adapters, &[select(&key_of(&p, code), &[])], &options());
    let entry = &result.entries[0];
    assert_eq!(entry.adapter_id, "generic");
    assert_eq!(entry.app_id, "exe:code");
    assert_eq!(entry.display_name, "Code");
    assert_eq!(entry.state, AppState::None);
    assert_eq!(
        entry.launch.executable,
        Some(PathBuf::from("/usr/share/code/code"))
    );
    assert_eq!(titles(entry), vec!["main.rs"]);
    assert_eq!(
        result.warnings,
        vec![
            "The \"Panicky\" integration failed for Panicky App; basic support was used instead."
                .to_string()
        ]
    );
}

#[test]
fn a_panicking_identify_does_not_stop_capture() {
    let p = MockPlatform::new();
    let code = p.add_process("code", Some("/usr/share/code/code".into()));
    window(&p, code, "main.rs");
    let adapters = registry(vec![PanicAdapter::new(PanicAt::Identify, "code")]);
    let result = capture_with(&p, &adapters, &[select(&key_of(&p, code), &[])], &options());
    assert_eq!(result.entries.len(), 1);
    assert_eq!(result.entries[0].adapter_id, "generic");
    assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
}

#[test]
fn saved_bounds_are_the_restore_bounds() {
    let p = MockPlatform::new();
    let pid = p.add_process("gimp", Some("/usr/bin/gimp".into()));
    let maximized = window(&p, pid, "maximized");
    let plain = window(&p, pid, "plain");
    edit_window(&p, &maximized, |w| {
        w.state = WindowState::Maximized;
        w.bounds = Rect::new(0, 0, 1920, 1040);
        w.normal_bounds = Some(Rect::new(200, 150, 1000, 700));
    });
    edit_window(&p, &plain, |w| {
        w.bounds = Rect::new(5, 6, 640, 480);
        w.normal_bounds = None;
    });

    let result = capture(&p, &[select(&key_of(&p, pid), &[])]);
    let windows = &result.entries[0].windows;
    assert_eq!(windows[0].title, "plain");
    assert_eq!(windows[0].bounds, Rect::new(5, 6, 640, 480));
    assert_eq!(windows[1].bounds, Rect::new(200, 150, 1000, 700));
    assert_eq!(windows[1].state, WindowState::Maximized);
}

#[test]
fn empty_selections_are_rejected() {
    let p = MockPlatform::new();
    let state = discovery::gather(&p);
    let err = capture_entries(&state, &p, &generic_only(), &[], &options()).unwrap_err();
    assert!(matches!(err, CoreError::Validation(_)), "{err}");
}

#[test]
fn script_hosted_entries_are_named_after_the_script() {
    let p = linux();
    let meld = p.add_process_info(ProcessInfo {
        command_line: vec!["/usr/bin/python3".into(), "--version-flag".into()],
        ..process(0, "meld", Some("/usr/bin/python3.12"))
    });
    let first = window(&p, meld, "first");
    let second = window(&p, meld, "second");
    let key = key_of(&p, meld);
    p.close_window(&first).unwrap();

    let result = capture(&p, &[select(&key, &[&first, &second])]);
    let entry = &result.entries[0];
    assert_eq!(entry.display_name, "Meld");
    assert_eq!(entry.app_id, "exe:python3");
    assert_eq!(
        entry.launch.executable,
        Some(PathBuf::from("/usr/bin/python3.12"))
    );
    // Every message uses the presented name.
    assert_eq!(
        result.warnings,
        vec!["Meld: 1 window closed since the scan.".to_string()]
    );
}

#[test]
fn at_most_the_window_limit_is_saved() {
    let p = MockPlatform::new();
    let pid = p.add_process("browser", Some("/usr/bin/browser".into()));
    let total = MAX_WINDOWS_PER_APP + 2;
    for i in 0..total {
        window(&p, pid, &format!("w{i}"));
    }
    let result = capture(&p, &[select(&key_of(&p, pid), &[])]);
    let entry = &result.entries[0];
    assert_eq!(entry.windows.len(), MAX_WINDOWS_PER_APP);
    let newest = format!("w{}", total - 1);
    assert_eq!(
        entry.windows[0].title, newest,
        "the frontmost windows are kept"
    );
    assert_eq!(
        result.warnings,
        vec![format!(
            "Browser has {total} windows; only the {MAX_WINDOWS_PER_APP} frontmost were saved."
        )]
    );
}

#[test]
fn windows_without_a_process_are_captured_without_an_executable() {
    let p = linux();
    p.add_window(0, "Clock", Some("XClock"), rect(300, 300));
    let result = capture(&p, &[select("class:XClock", &[])]);
    let entry = &result.entries[0];
    assert_eq!(entry.app_id, "exe:xclock");
    assert_eq!(entry.launch, LaunchConfig::default());
    assert_eq!(titles(entry), vec!["Clock"]);
    assert_eq!(
        result.warnings,
        vec!["Xclock: the executable path is not accessible; choose it in the workspace before restoring.".to_string()]
    );
}

#[test]
fn excluded_and_system_applications_can_be_captured_explicitly() {
    let p = MockPlatform::new();
    set_os(&p, OsKind::Linux);
    let shell = p.add_process("gnome-shell", Some("/usr/bin/gnome-shell".into()));
    window(&p, shell, "gnome-shell");
    let slack = p.add_process("slack", Some("/usr/lib/slack/slack".into()));
    window(&p, slack, "Slack");
    let opts = DiscoveryOptions {
        exclusions: vec!["slack".into()],
        ..options()
    };
    let snap = scan(&p, &generic_only(), &opts);
    assert!(snap.applications.iter().all(|a| !a.suggested));

    let selections: Vec<CaptureSelection> = snap
        .applications
        .iter()
        .map(|a| select(&a.key, &[]))
        .collect();
    let result = capture_with(&p, &generic_only(), &selections, &opts);
    assert_eq!(result.entries.len(), 2);
}

#[test]
fn reframe_itself_is_never_captured() {
    let p = MockPlatform::new();
    let me = p.add_process("reframe", Some("/usr/bin/reframe".into()));
    window(&p, me, "Reframe");
    let opts = DiscoveryOptions {
        self_pid: me,
        ..options()
    };
    let result = capture_with(
        &p,
        &generic_only(),
        &[select("exe:/usr/bin/reframe", &[])],
        &opts,
    );
    assert!(result.entries.is_empty());
}

#[test]
fn applications_restored_by_reframe_can_be_captured() {
    let p = linux();
    let me = p.add_process("reframe", Some("/usr/bin/reframe".into()));
    window(&p, me, "Reframe");
    // Started by a restore: detached, but still a child of Reframe.
    let gedit = p.add_process_info(ProcessInfo {
        parent_pid: Some(me),
        ..process(0, "gedit", Some("/usr/bin/gedit"))
    });
    window(&p, gedit, "notes.txt");
    let opts = DiscoveryOptions {
        self_pid: me,
        ..options()
    };
    let result = capture_with(
        &p,
        &generic_only(),
        &[
            select("exe:/usr/bin/gedit", &[]),
            select("exe:/usr/bin/reframe", &[]),
        ],
        &opts,
    );
    let apps: Vec<&str> = result.entries.iter().map(|e| e.app_id.as_str()).collect();
    assert_eq!(apps, vec!["exe:gedit"]);
}

#[test]
fn platform_warnings_of_the_fresh_scan_are_included() {
    let p = linux();
    let pid = p.add_process("gimp", Some("/usr/bin/gimp".into()));
    window(&p, pid, "GIMP");
    let mut state = discovery::gather(&p);
    state.warnings.push("Monitors: not available".into());
    let result = capture_entries(
        &state,
        &p,
        &generic_only(),
        &[select("exe:/usr/bin/gimp", &[])],
        &options(),
    )
    .unwrap();
    assert_eq!(result.entries.len(), 1);
    assert_eq!(result.warnings, vec!["Monitors: not available".to_string()]);
}

// ---------------------------------------------------------------------------------- manual

fn manual(p: &MockPlatform, target: LaunchTarget) -> CoreResult<ApplicationEntry> {
    manual_entry(p, &generic_only(), target)
}

fn executable(path: &Path) -> LaunchTarget {
    LaunchTarget::Executable {
        path: path.to_path_buf(),
    }
}

fn validation_message(result: CoreResult<ApplicationEntry>) -> String {
    match result {
        Err(CoreError::Validation(messages)) => messages.join(" "),
        other => panic!("expected a validation error, got {other:?}"),
    }
}

/// A file with the executable bit set.
fn program(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

/// `<dir>/<name>.app` with an Info.plist.
fn app_bundle(dir: &Path, name: &str, info_plist: &[u8]) -> PathBuf {
    let bundle = dir.join(format!("{name}.app"));
    let contents = bundle.join("Contents");
    std::fs::create_dir_all(contents.join("MacOS")).unwrap();
    std::fs::write(contents.join("Info.plist"), info_plist).unwrap();
    program(&contents.join("MacOS"), name);
    bundle
}

const EXAMPLE_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleExecutable</key>
	<string>Example</string>
	<key>CFBundleIdentifier</key>
	<string>com.example.Example</string>
</dict>
</plist>
"#;

#[test]
fn manual_executable_entry_launches_the_program() {
    let dir = tempfile::tempdir().unwrap();
    let exe = program(dir.path(), "gimp-2.10");
    let entry = manual(&linux(), executable(&exe)).unwrap();
    assert_eq!(entry.app_id, "exe:gimp-2");
    assert_eq!(entry.display_name, "Gimp 2");
    assert_eq!(entry.adapter_id, "generic");
    assert_eq!(
        entry.launch,
        LaunchConfig {
            executable: Some(exe),
            ..LaunchConfig::default()
        }
    );
    assert_eq!(entry.state, AppState::None);
    assert!(entry.windows.is_empty());
    assert!(entry.enabled);
}

#[test]
#[cfg(unix)] // real executable files with Unix semantics
fn manual_entries_are_identified_by_adapters() {
    let dir = tempfile::tempdir().unwrap();
    let exe = program(dir.path(), "code");
    let entry = manual_entry(
        &linux(),
        &registry(vec![TestAdapter::new("code")]),
        executable(&exe),
    )
    .unwrap();
    assert_eq!(entry.adapter_id, "test");
    assert_eq!(entry.app_id, "test-code");
    assert_eq!(entry.launch.executable, Some(exe));
}

#[test]
fn a_panicking_adapter_does_not_break_manual_entries() {
    let dir = tempfile::tempdir().unwrap();
    let exe = program(dir.path(), "code");
    for at in [PanicAt::Identify, PanicAt::Id] {
        let entry = manual_entry(
            &linux(),
            &registry(vec![PanicAdapter::new(at, "code")]),
            executable(&exe),
        )
        .unwrap();
        assert_eq!(entry.adapter_id, "generic", "{at:?}");
        assert_eq!(entry.app_id, "exe:code", "{at:?}");
    }
}

#[test]
fn missing_executables_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let message = validation_message(manual(&linux(), executable(&dir.path().join("missing"))));
    assert!(message.contains("was not found"), "{message}");
}

#[test]
fn documents_are_rejected() {
    let message = validation_message(manual(
        &MockPlatform::new(),
        LaunchTarget::Open {
            target: "/home/me/report.pdf".into(),
        },
    ));
    assert_eq!(message, "Choose an application, not a document.");
}

#[cfg(unix)]
#[test]
fn folders_shortcuts_and_plain_files_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let p = linux();

    let message = validation_message(manual(&p, executable(dir.path())));
    assert!(message.contains("is a folder"), "{message}");

    let message = validation_message(manual(&p, executable(Path::new("bin/tool"))));
    assert!(message.contains("not a full path"), "{message}");

    let notes = dir.path().join("notes.txt");
    std::fs::write(&notes, "hello").unwrap();
    let message = validation_message(manual(&p, executable(&notes)));
    assert!(message.contains("not marked as executable"), "{message}");

    let shortcut = dir.path().join("firefox.desktop");
    std::fs::write(&shortcut, "[Desktop Entry]\n").unwrap();
    let message = validation_message(manual(&p, executable(&shortcut)));
    assert!(message.contains("desktop shortcut"), "{message}");

    let message = validation_message(manual(&p, executable(Path::new(""))));
    assert_eq!(message, "Choose an application.");
}

#[test]
fn windows_accepts_programs_only() {
    let dir = tempfile::tempdir().unwrap();
    let p = MockPlatform::new();
    set_os(&p, OsKind::Windows);
    let report = dir.path().join("report.pdf");
    std::fs::write(&report, "%PDF").unwrap();
    let message = validation_message(manual(&p, executable(&report)));
    assert!(message.contains("Choose an .exe file"), "{message}");

    // No executable bit on Windows: the extension decides.
    let tool = dir.path().join("tool.exe");
    std::fs::write(&tool, "MZ").unwrap();
    let entry = manual(&p, executable(&tool)).unwrap();
    assert_eq!(entry.launch.executable, Some(tool));
}

#[test]
fn app_bundles_become_mac_packages() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = app_bundle(dir.path(), "Example", EXAMPLE_PLIST.as_bytes());
    let p = MockPlatform::new();
    set_os(&p, OsKind::Macos);

    let entry = manual(&p, executable(&bundle)).unwrap();
    assert_eq!(
        entry.launch,
        LaunchConfig {
            package: Some(PackageInfo {
                kind: PackageKind::MacBundle,
                id: "com.example.Example".into(),
                path: Some(bundle),
            }),
            ..LaunchConfig::default()
        }
    );
    assert_eq!(entry.app_id, "pkg:com.example.example");
    assert_eq!(entry.display_name, "Example");
}

#[test]
fn bundles_with_a_binary_info_plist_keep_their_path() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = app_bundle(dir.path(), "Binary", b"bplist00\xd1\x01\x02binary");
    let p = MockPlatform::new();
    set_os(&p, OsKind::Macos);

    let entry = manual(&p, executable(&bundle)).unwrap();
    assert_eq!(entry.launch.executable, Some(bundle));
    assert_eq!(entry.launch.package, None);
    assert_eq!(entry.app_id, "exe:binary");
    assert_eq!(entry.display_name, "Binary");
}

#[test]
fn app_bundles_are_rejected_on_other_systems() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = app_bundle(dir.path(), "Example", EXAMPLE_PLIST.as_bytes());
    let p = linux();
    let message = validation_message(manual(&p, executable(&bundle)));
    assert!(message.contains("macOS application"), "{message}");
}

#[test]
fn package_targets_are_validated_for_this_system() {
    let p = linux();
    let package = |kind, id: &str, path: Option<PathBuf>| LaunchTarget::Package {
        package: PackageInfo {
            kind,
            id: id.into(),
            path,
        },
    };

    let entry = manual(&p, package(PackageKind::Flatpak, " org.gimp.GIMP ", None)).unwrap();
    assert_eq!(entry.app_id, "pkg:org.gimp.gimp");
    assert_eq!(entry.display_name, "GIMP");
    assert_eq!(
        entry.launch.package,
        Some(PackageInfo {
            kind: PackageKind::Flatpak,
            id: "org.gimp.GIMP".into(),
            path: None,
        })
    );
    assert_eq!(entry.launch.executable, None);

    let message = validation_message(manual(
        &p,
        package(
            PackageKind::WindowsPackage,
            "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App",
            None,
        ),
    ));
    assert!(
        message.contains("cannot be started on this system"),
        "{message}"
    );

    let message = validation_message(manual(&p, package(PackageKind::Snap, "  ", None)));
    assert!(message.contains("id is missing"), "{message}");

    let message = validation_message(manual(&p, package(PackageKind::Flatpak, "org.x y", None)));
    assert!(message.contains("not a valid application id"), "{message}");

    // Ids are passed to `flatpak run` / `snap run`: they must not read as options.
    for (kind, id) in [
        (PackageKind::Flatpak, "--command=sh"),
        (PackageKind::Snap, "-x"),
    ] {
        let message = validation_message(manual(&p, package(kind, id, None)));
        assert!(message.contains("not a valid application id"), "{message}");
    }

    let message = validation_message(manual(&p, package(PackageKind::AppImage, "", None)));
    assert!(message.contains(".AppImage"), "{message}");

    let message = validation_message(manual(
        &p,
        package(
            PackageKind::AppImage,
            "",
            Some("/definitely/not/here/App.AppImage".into()),
        ),
    ));
    assert!(message.contains("was not found"), "{message}");
}

#[cfg(unix)]
#[test]
fn appimages_need_an_executable_file() {
    let dir = tempfile::tempdir().unwrap();
    let p = linux();
    let image = program(dir.path(), "Obsidian-1.5.3.AppImage");
    let entry = manual(
        &p,
        LaunchTarget::Package {
            package: PackageInfo {
                kind: PackageKind::AppImage,
                id: String::new(),
                path: Some(image.clone()),
            },
        },
    )
    .unwrap();
    assert_eq!(entry.display_name, "Obsidian");
    assert_eq!(entry.app_id, "pkg:obsidian");
    assert_eq!(
        entry.launch.package,
        Some(PackageInfo {
            kind: PackageKind::AppImage,
            id: "Obsidian".into(),
            path: Some(image.clone()),
        })
    );

    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&image, std::fs::Permissions::from_mode(0o644)).unwrap();
    let message = validation_message(manual(
        &p,
        LaunchTarget::Package {
            package: PackageInfo {
                kind: PackageKind::AppImage,
                id: String::new(),
                path: Some(image),
            },
        },
    ));
    assert!(message.contains("not marked as executable"), "{message}");
}

#[test]
fn mac_bundle_packages_read_a_missing_id_from_the_bundle() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = app_bundle(dir.path(), "Example", EXAMPLE_PLIST.as_bytes());
    let p = MockPlatform::new();
    set_os(&p, OsKind::Macos);
    let entry = manual(
        &p,
        LaunchTarget::Package {
            package: PackageInfo {
                kind: PackageKind::MacBundle,
                id: String::new(),
                path: Some(bundle.clone()),
            },
        },
    )
    .unwrap();
    assert_eq!(
        entry.launch.package.as_ref().map(|p| p.id.as_str()),
        Some("com.example.Example")
    );
    assert_eq!(entry.display_name, "Example");

    let binary = app_bundle(dir.path(), "Binary", b"bplist00");
    let message = validation_message(manual(
        &p,
        LaunchTarget::Package {
            package: PackageInfo {
                kind: PackageKind::MacBundle,
                id: String::new(),
                path: Some(binary),
            },
        },
    ));
    assert!(message.contains("bundle identifier"), "{message}");
}
