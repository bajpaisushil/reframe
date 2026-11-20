//! Discovery tests against the in-memory platform.

use reframe_platform::common::UnsupportedPlatform;
use reframe_platform::mock::MockPlatform;
use reframe_platform::{
    Capability, OsKind, PackageInfo, PackageKind, Platform, ProcessInfo, WindowId,
};

use super::test_support::*;
use super::*;
use crate::model::{CapabilityLevel, StateDetail};

fn keys(snapshot: &DesktopSnapshot) -> Vec<&str> {
    snapshot
        .applications
        .iter()
        .map(|a| a.key.as_str())
        .collect()
}

fn ids(app: &DiscoveredApplication) -> Vec<WindowId> {
    app.windows.iter().map(|w| w.id.clone()).collect()
}

fn app<'a>(snapshot: &'a DesktopSnapshot, key: &str) -> &'a DiscoveredApplication {
    snapshot
        .applications
        .iter()
        .find(|a| a.key == key)
        .unwrap_or_else(|| panic!("no application {key:?} in {:?}", keys(snapshot)))
}

fn app_by_pid(snapshot: &DesktopSnapshot, pid: u32) -> &DiscoveredApplication {
    snapshot
        .applications
        .iter()
        .find(|a| a.pids.contains(&pid))
        .unwrap_or_else(|| panic!("no application for pid {pid} in {:?}", keys(snapshot)))
}

#[test]
fn all_windows_of_a_process_form_one_application() {
    let p = MockPlatform::new();
    let pid = p.add_process("gimp", Some("/usr/bin/gimp".into()));
    let first = window(&p, pid, "Image 1");
    let second = window(&p, pid, "Image 2");

    let snap = scan(&p, &generic_only(), &options());
    assert_eq!(keys(&snap), vec!["exe:/usr/bin/gimp"]);
    let gimp = &snap.applications[0];
    assert_eq!(gimp.app_id, "exe:gimp");
    assert_eq!(gimp.display_name, "Gimp");
    assert_eq!(gimp.adapter_id, "generic");
    assert_eq!(gimp.capability.level, CapabilityLevel::LaunchOnly);
    assert_eq!(gimp.category, AppCategory::Application);
    assert_eq!(gimp.executable_path, Some("/usr/bin/gimp".into()));
    assert_eq!(gimp.package, None);
    assert_eq!(gimp.pids, vec![pid]);
    assert_eq!(ids(gimp), vec![second, first], "front-to-back");
    assert!(gimp.suggested);
    assert!(!gimp.excluded);
    assert_eq!(snap.background_process_count, 0);
    assert_eq!(snap.monitors, p.monitors().unwrap());
    assert_eq!(snap.platform, p.info());
}

#[test]
fn processes_sharing_an_executable_form_one_application() {
    let p = MockPlatform::new();
    let exe = "/opt/google/chrome/chrome";
    let browser = p.add_process("chrome", Some(exe.into()));
    let second_instance = p.add_process("chrome", Some(exe.into()));
    let _renderer = p.add_process("chrome", Some(exe.into()));
    let a = window(&p, browser, "Inbox");
    let b = window(&p, second_instance, "Docs");
    let c = window(&p, browser, "News");

    let snap = scan(&p, &generic_only(), &options());
    assert_eq!(snap.applications.len(), 1);
    let chrome = &snap.applications[0];
    // The owner of the frontmost window ("News") is the main process.
    assert_eq!(chrome.pids, vec![browser, second_instance]);
    assert_eq!(ids(chrome), vec![c, b.clone(), a]);
    // The renderer owns no window.
    assert_eq!(snap.background_process_count, 1);

    // Focusing the second instance's window makes it the main process.
    edit_window(&p, &b, |w| w.z_order = 0);
    for w in p.state().windows.iter_mut().filter(|w| w.id != b) {
        w.z_order += 1;
    }
    let snap = scan(&p, &generic_only(), &options());
    assert_eq!(snap.applications[0].pids, vec![second_instance, browser]);
}

#[test]
fn a_package_groups_processes_with_different_executables() {
    let p = MockPlatform::new();
    let package = PackageInfo {
        kind: PackageKind::Flatpak,
        id: "org.example.Suite".into(),
        path: None,
    };
    let writer = p.add_process_info(ProcessInfo {
        package: Some(package.clone()),
        ..process(0, "writer", Some("/app/bin/writer"))
    });
    let calc = p.add_process_info(ProcessInfo {
        package: Some(package.clone()),
        ..process(0, "calc", Some("/app/bin/calc"))
    });
    window(&p, writer, "Letter");
    window(&p, calc, "Budget");

    let snap = scan(&p, &generic_only(), &options());
    assert_eq!(keys(&snap), vec!["pkg:org.example.suite"]);
    let suite = &snap.applications[0];
    assert_eq!(suite.app_id, "pkg:org.example.suite");
    assert_eq!(suite.package, Some(package));
    assert_eq!(suite.pids, vec![calc, writer]);
}

#[test]
fn windows_executable_paths_are_case_insensitive() {
    let p = MockPlatform::new();
    set_os(&p, OsKind::Windows);
    let a = p.add_process(
        "Code.exe",
        Some(r"C:\Program Files\Microsoft VS Code\Code.exe".into()),
    );
    let b = p.add_process(
        "code.exe",
        Some(r"c:\program files\microsoft vs code\CODE.EXE".into()),
    );
    window(&p, a, "a");
    window(&p, b, "b");

    let snap = scan(&p, &generic_only(), &options());
    assert_eq!(
        keys(&snap),
        vec![r"exe:c:\program files\microsoft vs code\code.exe"]
    );

    // Elsewhere paths are case-sensitive.
    set_os(&p, OsKind::Linux);
    assert_eq!(scan(&p, &generic_only(), &options()).applications.len(), 2);
}

#[test]
fn processes_without_an_executable_path_group_by_name() {
    let p = MockPlatform::new();
    let a = p.add_process("xterm", None);
    let b = p.add_process("xterm", None);
    let c = p.add_process("xclock", None);
    window(&p, a, "a");
    window(&p, b, "b");
    window(&p, c, "c");

    let snap = scan(&p, &generic_only(), &options());
    assert_eq!(keys(&snap), vec!["proc:xclock", "proc:xterm"]);
    assert_eq!(app(&snap, "proc:xterm").pids, vec![b, a]);
    assert_eq!(app(&snap, "proc:xterm").executable_path, None);
}

#[test]
fn windows_without_a_known_process_group_by_class() {
    let p = MockPlatform::new();
    set_os(&p, OsKind::Linux);
    let w1 = p.add_window(0, "Clock", Some("XClock"), rect(300, 300));
    // pid 4242 is not in the process list (exited, or another user's process).
    let w2 = p.add_window(4242, "Clock 2", Some("XClock"), rect(300, 300));
    let w3 = p.add_window(0, "Mystery", None, rect(300, 300));
    let w4 = p.add_window(777, "Stray", None, rect(300, 300));

    let snap = scan(&p, &generic_only(), &options());
    let clock = app(&snap, "class:XClock");
    assert_eq!(ids(clock), vec![w2, w1]);
    assert_eq!(clock.pids, vec![4242]);
    assert_eq!(clock.executable_path, None);
    assert_eq!(clock.app_id, "exe:xclock");
    assert_eq!(clock.display_name, "Xclock");
    assert_eq!(ids(app(&snap, &format!("window:{w3}"))), vec![w3]);
    assert_eq!(ids(app(&snap, "pid:777")), vec![w4]);
    assert_eq!(snap.applications.len(), 3);
}

#[test]
fn reframe_itself_is_not_listed() {
    let p = MockPlatform::new();
    let me = p.add_process("reframe", Some("/usr/bin/reframe".into()));
    let gimp = p.add_process("gimp", Some("/usr/bin/gimp".into()));
    window(&p, me, "Reframe");
    window(&p, me, "Reframe settings");
    window(&p, gimp, "GIMP");

    let opts = DiscoveryOptions {
        self_pid: me,
        ..options()
    };
    let snap = scan(&p, &generic_only(), &opts);
    assert_eq!(keys(&snap), vec!["exe:/usr/bin/gimp"]);

    // An unknown own pid (0) hides nothing, not even windows without an owner.
    p.add_window(0, "Clock", Some("XClock"), rect(300, 300));
    let snap = scan(&p, &generic_only(), &options());
    assert_eq!(snap.applications.len(), 3);
    let snap = scan(&p, &generic_only(), &opts);
    assert_eq!(keys(&snap), vec!["class:XClock", "exe:/usr/bin/gimp"]);
}

#[test]
fn applications_restored_by_reframe_are_listed() {
    // Regression: restores start applications detached (`spawn_detached` only changes the
    // process group), so they keep Reframe as their parent process while it runs. Only windows
    // owned by Reframe's own pid are hidden, never those of its descendants.
    let p = MockPlatform::new();
    set_os(&p, OsKind::Windows);
    let child = |parent: u32, name: &str, exe: &str| {
        p.add_process_info(ProcessInfo {
            parent_pid: Some(parent),
            ..process(0, name, Some(exe))
        })
    };
    let me = p.add_process(
        "reframe.exe",
        Some(r"C:\Program Files\Reframe\reframe.exe".into()),
    );
    let notepad = child(me, "Notepad.exe", r"C:\Windows\System32\notepad.exe");
    let terminal = child(
        me,
        "WindowsTerminal.exe",
        r"C:\Program Files\WindowsApps\Microsoft.WindowsTerminal\WindowsTerminal.exe",
    );
    let shell = child(
        terminal,
        "pwsh.exe",
        r"C:\Program Files\PowerShell\7\pwsh.exe",
    );
    window(&p, me, "Reframe");
    window(&p, notepad, "notes.txt - Notepad");
    window(&p, terminal, "Terminal");
    window(&p, shell, "Windows PowerShell");

    let opts = DiscoveryOptions {
        self_pid: me,
        ..options()
    };
    let snap = scan(&p, &generic_only(), &opts);
    let mut listed: Vec<u32> = snap
        .applications
        .iter()
        .flat_map(|a| a.pids.clone())
        .collect();
    listed.sort_unstable();
    assert_eq!(listed, vec![notepad, terminal, shell], "{:?}", keys(&snap));
    assert!(snap.applications.iter().all(|a| a.suggested));
}

#[test]
fn applications_follow_their_frontmost_window() {
    let p = MockPlatform::new();
    let editor = p.add_process("editor", Some("/usr/bin/editor".into()));
    let viewer = p.add_process("viewer", Some("/usr/bin/viewer".into()));
    let e1 = window(&p, editor, "e1");
    let v1 = window(&p, viewer, "v1");
    let e2 = window(&p, editor, "e2");

    let snap = scan(&p, &generic_only(), &options());
    assert_eq!(
        keys(&snap),
        vec!["exe:/usr/bin/editor", "exe:/usr/bin/viewer"]
    );
    assert_eq!(ids(&snap.applications[0]), vec![e2.clone(), e1.clone()]);

    edit_window(&p, &v1, |w| w.z_order = 0);
    edit_window(&p, &e2, |w| w.z_order = 1);
    edit_window(&p, &e1, |w| w.z_order = 2);
    let snap = scan(&p, &generic_only(), &options());
    assert_eq!(
        keys(&snap),
        vec!["exe:/usr/bin/viewer", "exe:/usr/bin/editor"]
    );
}

#[test]
fn stacking_order_wins_over_list_order() {
    let p = MockPlatform::new();
    let a = p.add_process("a", Some("/usr/bin/a".into()));
    let b = p.add_process("b", Some("/usr/bin/b".into()));
    window(&p, a, "a");
    window(&p, b, "b");
    let mut state = gather(&p);
    state.windows.reverse();
    let snap = build_snapshot(&state, &p, &generic_only(), &options());
    assert_eq!(keys(&snap), vec!["exe:/usr/bin/b", "exe:/usr/bin/a"]);
}

#[test]
fn shell_components_are_listed_but_not_suggested() {
    let p = MockPlatform::new();
    set_os(&p, OsKind::Linux);
    let shell = p.add_process("gnome-shell", Some("/usr/bin/gnome-shell".into()));
    let panel = p.add_process("xfce4-panel", Some("/usr/bin/xfce4-panel".into()));
    let gimp = p.add_process("gimp", Some("/usr/bin/gimp".into()));
    window(&p, shell, "gnome-shell");
    window(&p, panel, "Panel");
    window(&p, gimp, "GIMP");

    let snap = scan(&p, &generic_only(), &options());
    for pid in [shell, panel] {
        let component = app_by_pid(&snap, pid);
        assert_eq!(component.category, AppCategory::System);
        assert!(!component.suggested);
        assert!(!component.excluded);
    }
    assert!(app_by_pid(&snap, gimp).suggested);
}

#[test]
fn system_tables_follow_the_reported_os() {
    let p = MockPlatform::new();
    let dock = p.add_process(
        "Dock",
        Some("/System/Library/CoreServices/Dock.app/Contents/MacOS/Dock".into()),
    );
    let input = p.add_process(
        "TextInputHost.exe",
        Some(
            r"C:\Windows\SystemApps\MicrosoftWindows.Client.CBS_cw5n1h2txyewy\TextInputHost.exe"
                .into(),
        ),
    );
    window(&p, dock, "Dock");
    window(&p, input, "Emoji");

    set_os(&p, OsKind::Macos);
    let snap = scan(&p, &generic_only(), &options());
    assert_eq!(app_by_pid(&snap, dock).category, AppCategory::System);
    assert_eq!(app_by_pid(&snap, input).category, AppCategory::Application);

    set_os(&p, OsKind::Windows);
    let snap = scan(&p, &generic_only(), &options());
    assert_eq!(app_by_pid(&snap, dock).category, AppCategory::Application);
    assert_eq!(app_by_pid(&snap, input).category, AppCategory::System);
}

#[test]
fn helpers_and_tiny_windows_are_utilities() {
    let p = MockPlatform::new();
    set_os(&p, OsKind::Linux);
    let applet = p.add_process("nm-applet", Some("/usr/bin/nm-applet".into()));
    let eyes = p.add_process("xeyes", Some("/usr/bin/xeyes".into()));
    let editor = p.add_process("editor", Some("/usr/bin/editor".into()));
    p.add_window(applet, "Network", None, rect(400, 300));
    p.add_window(eyes, "xeyes", None, rect(150, 100));
    p.add_window(editor, "Toolbox", None, rect(150, 100));
    p.add_window(editor, "Document", None, rect(900, 700));

    let snap = scan(&p, &generic_only(), &options());
    for pid in [applet, eyes] {
        let helper = app_by_pid(&snap, pid);
        assert_eq!(helper.category, AppCategory::Utility);
        assert!(!helper.suggested);
    }
    let editor_app = app_by_pid(&snap, editor);
    assert_eq!(editor_app.category, AppCategory::Application);
    assert!(editor_app.suggested);
}

#[test]
fn exclusions_are_case_insensitive() {
    let p = MockPlatform::new();
    let slack = p.add_process("slack", Some("/usr/lib/slack/slack".into()));
    let gimp = p.add_process("gimp-2.10", Some("/usr/bin/gimp-2.10".into()));
    let notes = p.add_process("Notes", None);
    let xterm = p.add_process("xterm", Some("/usr/bin/xterm".into()));
    for (pid, title) in [
        (slack, "Slack"),
        (gimp, "GIMP"),
        (notes, "Notes"),
        (xterm, "xterm"),
    ] {
        window(&p, pid, title);
    }
    let opts = DiscoveryOptions {
        // app id, executable stem and process name
        exclusions: vec!["EXE:SLACK".into(), "Gimp-2".into(), " notes ".into()],
        ..options()
    };

    let snap = scan(&p, &generic_only(), &opts);
    for pid in [slack, gimp, notes] {
        let excluded = app_by_pid(&snap, pid);
        assert!(excluded.excluded, "{}", excluded.display_name);
        assert!(!excluded.suggested, "{}", excluded.display_name);
        assert_eq!(excluded.category, AppCategory::Application);
    }
    let kept = app_by_pid(&snap, xterm);
    assert!(!kept.excluded);
    assert!(kept.suggested);
}

#[test]
fn adapter_identity_capability_and_details_are_attached() {
    let p = MockPlatform::new();
    let code = p.add_process("code", Some("/usr/share/code/code".into()));
    let gimp = p.add_process("gimp", Some("/usr/bin/gimp".into()));
    let w = window(&p, code, "main.rs — reframe");
    window(&p, gimp, "GIMP");
    let adapter = TestAdapter::new("code");
    let adapters = registry(vec![adapter.clone()]);
    let opts = DiscoveryOptions {
        browser_tab_capture: true,
        ..options()
    };

    let snap = scan(&p, &adapters, &opts);
    let code_app = app_by_pid(&snap, code);
    assert_eq!(code_app.adapter_id, "test");
    assert_eq!(code_app.app_id, "test-code");
    assert_eq!(code_app.display_name, "Test App");
    assert_eq!(code_app.capability.level, CapabilityLevel::Full);
    assert_eq!(code_app.details, vec![StateDetail::new("Projects", "1")]);
    assert_eq!(app_by_pid(&snap, gimp).adapter_id, "generic");

    let seen = adapter.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].windows, vec![w]);
    assert_eq!(seen[0].pids, vec![code]);
    assert_eq!(seen[0].all_processes, 2);
    assert!(seen[0].browser_tab_capture);

    scan(&p, &adapters, &options());
    assert!(!adapter.seen()[1].browser_tab_capture);
}

#[test]
fn generic_preview_lists_documents_from_the_command_line() {
    let dir = tempfile::tempdir().unwrap();
    let doc = dir.path().join("notes.txt");
    std::fs::write(&doc, "x").unwrap();
    let p = MockPlatform::new();
    let pid = p.add_process_info(ProcessInfo {
        command_line: vec![
            "gedit".into(),
            "--token=secret".into(),
            doc.display().to_string(),
        ],
        ..process(0, "gedit", Some("/usr/bin/gedit"))
    });
    window(&p, pid, "notes.txt");

    let snap = scan(&p, &generic_only(), &options());
    assert_eq!(
        snap.applications[0].details,
        vec![StateDetail::new("Document", "notes.txt")]
    );
}

#[test]
fn a_panicking_identify_skips_only_that_adapter() {
    let p = MockPlatform::new();
    let code = p.add_process("code", Some("/usr/share/code/code".into()));
    let gimp = p.add_process("gimp", Some("/usr/bin/gimp".into()));
    window(&p, code, "main.rs");
    window(&p, gimp, "GIMP");
    let adapters = registry(vec![
        PanicAdapter::new(PanicAt::Identify, "code"),
        TestAdapter::new("code"),
    ]);

    let snap = scan(&p, &adapters, &options());
    assert_eq!(snap.applications.len(), 2);
    assert_eq!(app_by_pid(&snap, code).adapter_id, "test");
    assert_eq!(app_by_pid(&snap, gimp).adapter_id, "generic");
    let failures: Vec<&String> = snap
        .warnings
        .iter()
        .filter(|w| w.contains("\"Panicky\""))
        .collect();
    assert_eq!(failures.len(), 1, "{:?}", snap.warnings);
    // Applications in snapshot order (GIMP's window is in front).
    assert!(
        failures[0].contains("for Gimp and Code;"),
        "{}",
        failures[0]
    );
}

#[test]
fn adapters_failing_after_identification_fall_back_to_generic() {
    for at in [
        PanicAt::Id,
        PanicAt::Capability,
        PanicAt::Capture,
        PanicAt::Describe,
    ] {
        let p = MockPlatform::new();
        let code = p.add_process("code", Some("/usr/share/code/code".into()));
        window(&p, code, "main.rs");
        let adapters = registry(vec![PanicAdapter::new(at, "code")]);

        let snap = scan(&p, &adapters, &options());
        assert_eq!(snap.applications.len(), 1, "{at:?}");
        let fallback = &snap.applications[0];
        assert_eq!(fallback.adapter_id, "generic", "{at:?}");
        assert_eq!(fallback.app_id, "exe:code", "{at:?}");
        assert_eq!(fallback.display_name, "Code", "{at:?}");
        assert_eq!(
            fallback.capability.level,
            CapabilityLevel::LaunchOnly,
            "{at:?}"
        );
        assert!(fallback.details.is_empty(), "{at:?}");
        assert!(fallback.suggested, "{at:?}");
        assert_eq!(
            snap.warnings,
            vec![
                "The \"Panicky\" integration failed for Panicky App; basic support was used instead."
                    .to_string()
            ],
            "{at:?}"
        );
    }
}

#[test]
fn processes_without_listed_windows_are_background() {
    let p = MockPlatform::new();
    let gimp = p.add_process("gimp", Some("/usr/bin/gimp".into()));
    p.add_process("dbus-daemon", None);
    p.add_process("pipewire", None);
    let me = p.add_process("reframe", None);
    window(&p, gimp, "GIMP");
    window(&p, me, "Reframe");

    let opts = DiscoveryOptions {
        self_pid: me,
        ..options()
    };
    let snap = scan(&p, &generic_only(), &opts);
    assert_eq!(snap.applications.len(), 1);
    // dbus-daemon, pipewire and Reframe itself (its window is not listed).
    assert_eq!(snap.background_process_count, 3);
}

#[test]
fn platform_warnings_are_kept_and_partial_listing_is_explained() {
    let p = MockPlatform::new();
    let mut info = p.info();
    info.capabilities.list_windows =
        Capability::partial("Only X11 applications are visible in this Wayland session.");
    p.set_info(info);
    let mut state = gather(&p);
    state.warnings.push("Monitors: something went wrong".into());

    let snap = build_snapshot(&state, &p, &generic_only(), &options());
    assert_eq!(
        snap.warnings,
        vec![
            "Monitors: something went wrong".to_string(),
            "Some open windows may be missing: Only X11 applications are visible in this Wayland session."
                .to_string(),
        ]
    );
}

#[test]
fn unsupported_window_listing_is_explained_once() {
    let p = MockPlatform::new();
    let mut info = p.info();
    info.capabilities.list_windows = Capability::unsupported("No window manager integration.");
    p.set_info(info);

    let snap = scan(&p, &generic_only(), &options());
    assert_eq!(
        snap.warnings,
        vec![
            "Reframe cannot list open windows on this system: No window manager integration."
                .to_string()
        ]
    );

    // The platform already explained it while listing windows: not repeated.
    let mut state = gather(&p);
    state.warnings.push(
        "Windows: Listing windows is not supported here: No window manager integration.".into(),
    );
    let snap = build_snapshot(&state, &p, &generic_only(), &options());
    assert_eq!(snap.warnings.len(), 1, "{:?}", snap.warnings);
}

#[test]
fn a_platform_without_window_support_still_produces_a_snapshot() {
    let platform = UnsupportedPlatform::new();
    let state = gather(&platform);
    let snap = build_snapshot(&state, &platform, &AdapterRegistry::builtin(), &options());
    assert!(snap.applications.is_empty());
    assert_eq!(
        snap.background_process_count as usize,
        state.processes.len()
    );
    assert!(
        snap.warnings
            .iter()
            .any(|w| w.contains("no window-management integration")),
        "{:?}",
        snap.warnings
    );
}

#[test]
fn script_hosted_applications_are_separate() {
    let p = MockPlatform::new();
    set_os(&p, OsKind::Linux);
    let meld = p.add_process("meld", Some("/usr/bin/python3.12".into()));
    let terminator = p.add_process("terminator", Some("/usr/bin/python3.12".into()));
    let meld2 = p.add_process("meld", Some("/usr/bin/python3.12".into()));
    window(&p, meld, "Meld");
    window(&p, terminator, "Terminator");
    window(&p, meld2, "Meld 2");

    let snap = scan(&p, &generic_only(), &options());
    assert_eq!(
        keys(&snap),
        vec![
            "script:meld|/usr/bin/python3.12",
            "script:terminator|/usr/bin/python3.12"
        ]
    );
    assert_eq!(snap.applications[0].pids, vec![meld2, meld]);
    // Named after the script rather than the interpreter ("Python3"); the app id stays the
    // generic adapter's.
    let names: Vec<&str> = snap
        .applications
        .iter()
        .map(|a| a.display_name.as_str())
        .collect();
    assert_eq!(names, vec!["Meld", "Terminator"]);
    assert_eq!(snap.applications[0].app_id, "exe:python3");
}

#[test]
fn script_hosted_names_defer_to_product_names_and_adapters() {
    let p = MockPlatform::new();
    set_os(&p, OsKind::Linux);
    let meld = p.add_process_info(ProcessInfo {
        product_name: Some("Meld Diff Viewer".into()),
        ..process(0, "meld", Some("/usr/bin/python3.12"))
    });
    let tool = p.add_process("code", Some("/usr/bin/node".into()));
    window(&p, meld, "Meld");
    window(&p, tool, "Tool");

    let snap = scan(&p, &registry(vec![TestAdapter::new("code")]), &options());
    assert_eq!(app_by_pid(&snap, meld).display_name, "Meld Diff Viewer");
    let claimed = app_by_pid(&snap, tool);
    assert_eq!(claimed.key, "script:code|/usr/bin/node");
    assert_eq!(claimed.display_name, "Test App");
}

#[test]
fn an_empty_desktop_is_fine() {
    let p = MockPlatform::new();
    p.add_process("systemd", None);
    let snap = scan(&p, &AdapterRegistry::builtin(), &options());
    assert!(snap.applications.is_empty());
    assert_eq!(snap.background_process_count, 1);
    assert!(snap.warnings.is_empty());
}
