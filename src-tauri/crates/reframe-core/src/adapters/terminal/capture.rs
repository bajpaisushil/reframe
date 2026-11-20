//! Capturing terminal windows: one [`TerminalSession`] per tab with its working directory, in
//! window order.
//!
//! Sessions are mapped to windows through the process that owns each window:
//! - A process with exactly one window (xterm, Alacritty, kitty, foot... and a server-style
//!   terminal that currently shows one window) owns all its sessions: they are that window's tabs.
//! - A process with several windows (GNOME Terminal's `gnome-terminal-server`, Konsole, Windows
//!   Terminal, Xfce Terminal...) gives no reliable tab→window mapping. Each window gets the session
//!   whose folder appears in the window title (most shells put the folder there), otherwise the
//!   oldest unassigned one; remaining sessions follow the window opened most recently before them.
//!   This is a heuristic and is reported as a warning.
//!
//! Encoding of windows in the saved state: the first session of every window carries the
//! window's title in `window_title`; further tabs of the same window have `window_title: None`.
//!
//! Never stored: command lines, shell history, running programs, scrollback. Startup commands are
//! never captured (`startup_command: None`, `startup_command_approved: false`).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use reframe_platform::{LaunchTarget, PackageInfo, PackageKind, ProcessInfo, WindowInfo};

use super::kind::{TerminalKind, is_console_host, is_console_shell, locate_query_for};
use super::sessions::{FoundSession, ProcessIndex, scan_root};
use super::shell::{base_name, normalize_name};
use crate::adapters::{CaptureContext, CaptureInput, CaptureOutput};
use crate::model::{AppState, LaunchConfig, TerminalSession};

/// Most sessions saved per terminal application.
pub const MAX_SESSIONS: usize = 30;

/// Inputs of [`capture_sessions`] that come from outside the capture input.
pub struct CaptureEnv<'a> {
    pub kind: Option<TerminalKind>,
    pub display_name: &'a str,
    /// Windows each owner process has on screen, selected or not (from the platform). Missing
    /// entries mean only the selected windows are known.
    pub total_windows: &'a HashMap<u32, usize>,
    pub home: Option<&'a Path>,
}

/// Result of [`capture_sessions`].
#[derive(Debug, Default)]
pub struct CapturedSessions {
    pub sessions: Vec<TerminalSession>,
    pub warnings: Vec<String>,
    /// Windows consoles: pid of the shell that owns the first console window.
    pub console_shell: Option<u32>,
}

/// Profile recorded for a classic console session: the shell it runs, using Windows Terminal's
/// default profile names.
pub fn console_profile(shell: &str) -> Option<&'static str> {
    match shell {
        "cmd" => Some("Command Prompt"),
        "powershell" => Some("Windows PowerShell"),
        "pwsh" => Some("PowerShell"),
        _ => None,
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Final path component for display and title matching (`/` or `\` separated).
pub fn dir_label(dir: &Path) -> String {
    base_name(&dir.to_string_lossy()).to_string()
}

/// How strongly `title` refers to `dir`: 3 = full path or `~/...` form, 1 = folder name.
fn title_score(title: &str, dir: Option<&Path>, home: Option<&Path>) -> u32 {
    let Some(dir) = dir else {
        return 0;
    };
    let title = title.to_lowercase();
    let full = dir.to_string_lossy().to_lowercase();
    if full.len() > 1 && title.contains(&full) {
        return 3;
    }
    if let Some(rel) = home.and_then(|h| dir.strip_prefix(h).ok())
        && !rel.as_os_str().is_empty()
    {
        let tilde = format!("~/{}", rel.to_string_lossy()).to_lowercase();
        if title.contains(&tilde) {
            return 3;
        }
    }
    let label = dir_label(dir).to_lowercase();
    if label.chars().count() >= 2 && title.contains(&label) {
        return 1;
    }
    0
}

/// Distribute one owner process's sessions over its selected windows (see the module docs).
///
/// `complete` is false when the process has windows that were not selected: then each selected
/// window keeps only its best session and the rest are returned as dropped.
pub fn assign_to_windows(
    titles: &[&str],
    mut sessions: Vec<FoundSession>,
    complete: bool,
    home: Option<&Path>,
) -> (Vec<Vec<FoundSession>>, usize) {
    sessions.sort_by_key(|s| s.order);
    if titles.is_empty() {
        return (Vec::new(), sessions.len());
    }
    if titles.len() == 1 && complete {
        return (vec![sessions], 0);
    }
    let mut groups: Vec<Vec<FoundSession>> = vec![Vec::new(); titles.len()];
    let mut remaining: Vec<Option<FoundSession>> = sessions.into_iter().map(Some).collect();

    for (w, title) in titles.iter().enumerate() {
        let best = remaining
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let s = s.as_ref()?;
                let score = title_score(title, s.working_directory.as_deref(), home);
                (score > 0).then_some((score, std::cmp::Reverse(i)))
            })
            .max()
            .map(|(_, std::cmp::Reverse(i))| i);
        if let Some(session) = best.and_then(|i| remaining[i].take()) {
            groups[w].push(session);
        }
    }
    for group in &mut groups {
        if group.is_empty()
            && let Some(session) = remaining.iter_mut().find_map(Option::take)
        {
            group.push(session);
        }
    }

    let leftovers: Vec<FoundSession> = remaining.into_iter().flatten().collect();
    if !complete {
        return (groups, leftovers.len());
    }
    for session in leftovers {
        let leads = groups
            .iter()
            .enumerate()
            .filter_map(|(w, g)| g.first().map(|lead| (w, lead.order)));
        let target = leads
            .clone()
            .filter(|(_, order)| *order <= session.order)
            .max_by_key(|(_, order)| *order)
            .or_else(|| leads.min_by_key(|(_, order)| *order))
            .map(|(w, _)| w);
        if let Some(w) = target {
            groups[w].push(session);
        }
    }
    for group in &mut groups {
        if group.len() > 2 {
            group[1..].sort_by_key(|s| s.order);
        }
    }
    (groups, 0)
}

/// Owner processes of the captured windows, in window order.
fn owners(input: &CaptureInput<'_>) -> Vec<u32> {
    let mut seen = HashSet::new();
    let from_windows: Vec<u32> = input
        .windows
        .iter()
        .map(|w| w.pid)
        .filter(|pid| seen.insert(*pid))
        .collect();
    if !from_windows.is_empty() {
        return from_windows;
    }
    input
        .processes
        .iter()
        .map(|p| p.pid)
        .filter(|pid| seen.insert(*pid))
        .collect()
}

/// Processes that run a terminal's sessions without being its children: iTerm2's
/// `iTermServer-*` and WezTerm's `wezterm-mux-server` detach from the GUI process.
fn detached_roots(kind: Option<TerminalKind>, all: &[ProcessInfo]) -> Vec<u32> {
    let matches: fn(&str) -> bool = match kind {
        Some(TerminalKind::ITerm2) => |n| n.starts_with("itermserver"),
        Some(TerminalKind::WezTerm) => |n| n.starts_with("wezterm-mux-ser"),
        _ => return Vec::new(),
    };
    all.iter()
        .filter(|p| matches(&normalize_name(&p.name)))
        .map(|p| p.pid)
        .collect()
}

/// Shell roots of one owner process: `(pid, the pid is itself the tab's shell)`.
fn roots_for_owner(
    kind: Option<TerminalKind>,
    index: &ProcessIndex<'_>,
    owner: u32,
) -> (Vec<(u32, bool)>, Option<u32>) {
    if kind != Some(TerminalKind::WindowsConsole) {
        return (vec![(owner, false)], None);
    }
    let Some(process) = index.get(owner) else {
        return (Vec::new(), None);
    };
    let name = normalize_name(&process.name);
    if is_console_shell(&name) {
        return (vec![(owner, true)], Some(owner));
    }
    if is_console_host(&name) {
        // conhost.exe is a child of the shell that created the console.
        if let Some(parent) = process.parent_pid.and_then(|pid| index.get(pid))
            && is_console_shell(&normalize_name(&parent.name))
        {
            return (vec![(parent.pid, true)], Some(parent.pid));
        }
    }
    (Vec::new(), None)
}

/// Find every tab session of the captured windows and arrange them per window.
pub fn capture_sessions(env: &CaptureEnv<'_>, input: &CaptureInput<'_>) -> CapturedSessions {
    let name = env.display_name;
    let index = ProcessIndex::new(input.all_processes, &input.processes);
    let owner_pids = owners(input);
    let mut claimed: HashSet<u32> = HashSet::new();
    let mut console_shell = None;
    let mut per_owner: HashMap<u32, Vec<FoundSession>> = HashMap::new();
    let mut extra_roots = detached_roots(env.kind, input.all_processes);
    // Whether any process was visible below the terminals at all (tells "details are hidden"
    // apart from "the windows run something other than a shell").
    let mut saw_children = false;

    for (i, &owner) in owner_pids.iter().enumerate() {
        let (mut roots, shell) = roots_for_owner(env.kind, &index, owner);
        console_shell = console_shell.or(shell);
        if i == 0 {
            roots.extend(extra_roots.drain(..).map(|pid| (pid, false)));
        }
        let mut found = Vec::new();
        for (root, include_root) in roots {
            let scan = scan_root(input.all_processes, &index, root, include_root);
            saw_children |= scan.descendant_count > 0 || !scan.sessions.is_empty();
            found.extend(
                scan.sessions
                    .into_iter()
                    .filter(|s| claimed.insert(s.shell_pid)),
            );
        }
        per_owner.insert(owner, found);
    }

    let mut warnings = Vec::new();
    let mut ambiguous = false;
    let mut dropped = 0usize;
    let mut by_window: HashMap<&str, Vec<FoundSession>> = HashMap::new();
    let mut unwindowed: Vec<FoundSession> = Vec::new();
    for owner in &owner_pids {
        let sessions = per_owner.remove(owner).unwrap_or_default();
        let windows: Vec<&WindowInfo> = input
            .windows
            .iter()
            .copied()
            .filter(|w| w.pid == *owner)
            .collect();
        if windows.is_empty() {
            unwindowed.extend(sessions);
            continue;
        }
        let total = env
            .total_windows
            .get(owner)
            .copied()
            .unwrap_or(0)
            .max(windows.len());
        let complete = total == windows.len();
        if (windows.len() > 1 || !complete) && sessions.len() > 1 {
            ambiguous = true;
        }
        let titles: Vec<&str> = windows.iter().map(|w| w.title.as_str()).collect();
        let (groups, lost) = assign_to_windows(&titles, sessions, complete, env.home);
        dropped += lost;
        for (window, group) in windows.iter().zip(groups) {
            by_window.insert(window.id.as_str(), group);
        }
    }

    // Arrange per window in the order the windows were captured (front to back).
    let profile_for = |s: &FoundSession| -> Option<String> {
        if env.kind == Some(TerminalKind::WindowsConsole) {
            console_profile(&s.shell).map(str::to_string)
        } else {
            None
        }
    };
    let mut windows_out: Vec<Vec<TerminalSession>> = Vec::new();
    let mut without_shell = 0usize;
    let mut unreadable = 0usize;
    for window in &input.windows {
        let group = by_window.remove(window.id.as_str()).unwrap_or_default();
        let mut sessions: Vec<TerminalSession> = group
            .iter()
            .map(|s| {
                if s.working_directory.is_none() {
                    unreadable += 1;
                }
                TerminalSession {
                    working_directory: s.working_directory.clone(),
                    profile: profile_for(s),
                    startup_command: None,
                    startup_command_approved: false,
                    window_title: None,
                }
            })
            .collect();
        if sessions.is_empty() {
            without_shell += 1;
            sessions.push(TerminalSession {
                working_directory: None,
                profile: None,
                startup_command: None,
                startup_command_approved: false,
                window_title: None,
            });
        }
        sessions[0].window_title = Some(window.title.clone());
        windows_out.push(sessions);
    }
    if !unwindowed.is_empty() {
        // No window information (e.g. the platform cannot list windows): keep the sessions as
        // one group; terminals without tabs reopen each of them in its own window anyway.
        windows_out.push(
            unwindowed
                .iter()
                .map(|s| {
                    if s.working_directory.is_none() {
                        unreadable += 1;
                    }
                    TerminalSession {
                        working_directory: s.working_directory.clone(),
                        profile: profile_for(s),
                        startup_command: None,
                        startup_command_approved: false,
                        window_title: None,
                    }
                })
                .collect(),
        );
    }

    // Cap: keep the first session of as many windows as possible, then their tabs.
    let total: usize = windows_out.iter().map(Vec::len).sum();
    if total > MAX_SESSIONS {
        let mut budget = MAX_SESSIONS;
        let mut keep = vec![0usize; windows_out.len()];
        for (k, group) in keep.iter_mut().zip(&windows_out) {
            if budget > 0 && !group.is_empty() {
                *k = 1;
                budget -= 1;
            }
        }
        for (k, group) in keep.iter_mut().zip(&windows_out) {
            if *k == 1 {
                let extra = (group.len() - 1).min(budget);
                *k += extra;
                budget -= extra;
            }
        }
        for (group, k) in windows_out.iter_mut().zip(keep) {
            group.truncate(k);
        }
        windows_out.retain(|g| !g.is_empty());
        warnings.push(format!(
            "{name}: only the first {MAX_SESSIONS} of {total} sessions were saved."
        ));
    }

    let window_count = input.windows.len();
    if window_count > 0 && without_shell == window_count && !saw_children {
        warnings.push(format!(
            "{name}: the programs running in its windows could not be read (process details may \
             be restricted); its windows will reopen in the default folder."
        ));
    } else if without_shell > 0 {
        warnings.push(format!(
            "{name}: no shell was found in {} (for example tmux, screen or ssh running \
             directly); they will reopen in the default folder.",
            plural(without_shell, "window", "windows")
        ));
    }
    if unreadable > 0 {
        warnings.push(format!(
            "{name}: the folder of {} could not be read (for example a shell running as another \
             user or as administrator); they will reopen in the default folder.",
            plural(unreadable, "session", "sessions")
        ));
    }
    if ambiguous {
        warnings.push(format!(
            "{name} shows several windows from one process, so tabs cannot be matched to windows \
             reliably; sessions were assigned by window title and the order they were opened."
        ));
    }
    if dropped > 0 {
        warnings.push(format!(
            "{name}: {} could not be matched to the selected windows (they may belong to windows \
             that were not selected) and {} left out.",
            plural(dropped, "session", "sessions"),
            if dropped == 1 { "was" } else { "were" }
        ));
    }

    CapturedSessions {
        sessions: windows_out.into_iter().flatten().collect(),
        warnings,
        console_shell,
    }
}

/// Whether `path` can be used to open new windows of `kind`. Known server processes
/// (`gnome-terminal-server`, `urxvtd`, `WindowsTerminal.exe`) and script interpreters (Terminator
/// runs under Python) cannot; executables Reframe does not know (a custom build chosen by the
/// user) are trusted.
pub fn usable_launcher(kind: TerminalKind, path: &Path) -> bool {
    let Some(file) = path.file_name() else {
        return false;
    };
    let name = normalize_name(&file.to_string_lossy());
    if kind.launcher_names().contains(&name.as_str()) {
        return true;
    }
    let server = kind.process_names_include(&name);
    !(server || name.starts_with("python"))
}

/// Launch configuration for a captured terminal. Arguments are never captured.
fn launch_config(
    ctx: &CaptureContext<'_>,
    kind: Option<TerminalKind>,
    main: &ProcessInfo,
    console_shell: Option<&ProcessInfo>,
) -> LaunchConfig {
    let mut config = LaunchConfig {
        executable: main.executable_path.clone(),
        package: main.package.clone(),
        arguments: Vec::new(),
        working_directory: None,
    };
    let Some(kind) = kind else {
        return config;
    };
    match kind {
        TerminalKind::WindowsConsole => {
            // The console's shell is the launcher; a console host is never launched directly
            // (restoring then falls back to cmd.exe).
            let main_is_shell = is_console_shell(&normalize_name(&main.name));
            config.executable = console_shell
                .and_then(|p| p.executable_path.clone())
                .or_else(|| config.executable.take().filter(|_| main_is_shell));
            config.package = None;
        }
        TerminalKind::MacTerminal | TerminalKind::ITerm2 | TerminalKind::Warp => {
            let is_bundle = config
                .package
                .as_ref()
                .is_some_and(|p| p.kind == PackageKind::MacBundle);
            if !is_bundle && let Some(bundle) = kind.bundle_id() {
                config.package = Some(PackageInfo {
                    kind: PackageKind::MacBundle,
                    id: bundle.to_string(),
                    path: None,
                });
            }
        }
        _ => {
            let usable = config
                .executable
                .as_deref()
                .is_some_and(|exe| usable_launcher(kind, exe));
            if !usable {
                let located = ctx
                    .platform
                    .locate_application(&locate_query_for(kind, ctx.info.os))
                    .into_iter()
                    .find_map(|target| match target {
                        LaunchTarget::Executable { path } => Some(path),
                        _ => None,
                    });
                config.executable = located;
            }
        }
    }
    config
}

/// Capture a terminal application (see the module docs).
pub fn capture(ctx: &CaptureContext<'_>, input: &CaptureInput<'_>) -> CaptureOutput {
    let name = input.identity.display_name.as_str();
    let Some(main) = input.main_process() else {
        return CaptureOutput {
            warnings: vec![format!("{name}: no process information available.")],
            ..Default::default()
        };
    };
    let kind = TerminalKind::from_app_id(&input.identity.app_id)
        .or_else(|| super::kind::identify_kind(ctx.info.os, main, &input.windows));

    // Count every window of the owner processes, selected or not, to know whether the selection
    // covers all of a multi-window process's tabs.
    let mut total_windows: HashMap<u32, usize> = HashMap::new();
    if let Ok(all_windows) = ctx.platform.windows() {
        for window in all_windows {
            *total_windows.entry(window.pid).or_default() += 1;
        }
    }
    let home = dirs::home_dir();
    let env = CaptureEnv {
        kind,
        display_name: name,
        total_windows: &total_windows,
        home: home.as_deref(),
    };
    let captured = capture_sessions(&env, input);

    let index = ProcessIndex::new(input.all_processes, &input.processes);
    let console_shell = captured.console_shell.and_then(|pid| index.get(pid));
    let launch = launch_config(ctx, kind, main, console_shell);
    let mut warnings = captured.warnings;
    if launch.executable.is_none() && launch.package.is_none() {
        warnings.push(format!(
            "{name}: the program that opens new windows was not found; choose it in the \
             workspace before restoring."
        ));
    }
    CaptureOutput {
        state: if captured.sessions.is_empty() {
            AppState::None
        } else {
            AppState::Terminal {
                sessions: captured.sessions,
            }
        },
        launch,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::adapters::AppIdentity;
    use crate::adapters::terminal::sessions::tests::{abs, p};
    use reframe_platform::mock::MockPlatform;
    use reframe_platform::{Platform, Rect, WindowId, WindowState};

    fn window(id: &str, pid: u32, title: &str) -> WindowInfo {
        WindowInfo {
            id: WindowId::new(id),
            pid,
            title: title.into(),
            class_name: None,
            app_id_hint: None,
            bounds: Rect::new(0, 0, 800, 600),
            normal_bounds: None,
            state: WindowState::Normal,
            monitor_id: None,
            desktop: None,
            is_focused: false,
            z_order: 0,
        }
    }

    struct Fixture {
        identity: AppIdentity,
        all: Vec<ProcessInfo>,
        windows: Vec<WindowInfo>,
        total: HashMap<u32, usize>,
    }

    impl Fixture {
        fn new(kind: TerminalKind, all: Vec<ProcessInfo>, windows: Vec<WindowInfo>) -> Self {
            Self {
                identity: AppIdentity {
                    app_id: kind.app_id().into(),
                    display_name: kind.display_name().into(),
                },
                all,
                windows,
                total: HashMap::new(),
            }
        }

        fn input(&self) -> CaptureInput<'_> {
            let mut owner_pids: Vec<u32> = self.windows.iter().map(|w| w.pid).collect();
            owner_pids.dedup();
            CaptureInput {
                identity: &self.identity,
                processes: owner_pids
                    .iter()
                    .filter_map(|pid| self.all.iter().find(|p| p.pid == *pid))
                    .collect(),
                windows: self.windows.iter().collect(),
                all_processes: &self.all,
            }
        }

        fn run(&self) -> CapturedSessions {
            let env = CaptureEnv {
                kind: TerminalKind::from_app_id(&self.identity.app_id),
                display_name: &self.identity.display_name,
                total_windows: &self.total,
                home: Some(Path::new("/home/u")),
            };
            capture_sessions(&env, &self.input())
        }
    }

    fn summary(sessions: &[TerminalSession]) -> Vec<(Option<String>, Option<PathBuf>)> {
        sessions
            .iter()
            .map(|s| (s.window_title.clone(), s.working_directory.clone()))
            .collect()
    }

    #[test]
    fn single_window_processes_map_by_owner() {
        // Two xterm windows, one process each; the second has a nested shell.
        let all = vec![
            p(100, 1, "xterm", &[], None),
            p(101, 100, "bash", &["bash"], Some("/home/u/a")),
            p(200, 1, "xterm", &[], None),
            p(201, 200, "bash", &["bash"], Some("/home/u")),
            p(202, 201, "zsh", &["zsh"], Some("/home/u/b")),
        ];
        let fx = Fixture::new(
            TerminalKind::Xterm,
            all,
            vec![window("w2", 200, "xterm"), window("w1", 100, "xterm")],
        );
        let out = fx.run();
        assert_eq!(
            summary(&out.sessions),
            vec![
                (Some("xterm".into()), Some(abs("home/u/b"))),
                (Some("xterm".into()), Some(abs("home/u/a"))),
            ]
        );
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    }

    #[test]
    fn tabs_of_a_single_window_follow_their_window() {
        let all = vec![
            p(100, 1, "kitty", &[], None),
            p(101, 100, "bash", &["bash"], Some("/home/u/one")),
            p(102, 100, "bash", &["bash"], Some("/home/u/two")),
            p(103, 100, "bash", &["bash"], Some("/home/u/three")),
        ];
        let fx = Fixture::new(TerminalKind::Kitty, all, vec![window("w", 100, "two")]);
        let out = fx.run();
        assert_eq!(
            summary(&out.sessions),
            vec![
                (Some("two".into()), Some(abs("home/u/one"))),
                (None, Some(abs("home/u/two"))),
                (None, Some(abs("home/u/three"))),
            ]
        );
    }

    #[test]
    fn multi_window_server_uses_titles_then_creation_order() {
        // gnome-terminal-server with two windows and four tabs.
        let all = vec![
            p(100, 1, "gnome-terminal-", &[], None),
            p(110, 100, "bash", &["bash"], Some("/home/u/api")),
            p(120, 100, "bash", &["bash"], Some("/home/u/web")),
            p(130, 100, "bash", &["bash"], Some("/home/u/web/src")),
            p(140, 100, "bash", &["bash"], Some("/home/u/docs")),
        ];
        let fx = Fixture::new(
            TerminalKind::GnomeTerminal,
            all,
            vec![
                // Front window shows the web tab; the back window shows docs.
                window("front", 100, "u@host: ~/web"),
                window("back", 100, "u@host: ~/docs"),
            ],
        );
        let out = fx.run();
        assert_eq!(
            summary(&out.sessions),
            vec![
                (Some("u@host: ~/web".into()), Some(abs("home/u/web"))),
                // Older than every lead: joins the window with the oldest lead (web).
                (None, Some(abs("home/u/api"))),
                // Opened after the web tab and before the docs tab: same window as web.
                (None, Some(abs("home/u/web/src"))),
                (Some("u@host: ~/docs".into()), Some(abs("home/u/docs"))),
            ]
        );
        assert!(
            out.warnings
                .iter()
                .any(|w| w.contains("cannot be matched to windows reliably")),
            "{:?}",
            out.warnings
        );
    }

    #[test]
    fn partial_selection_of_a_server_keeps_one_session_per_window() {
        let all = vec![
            p(100, 1, "konsole", &[], None),
            p(110, 100, "bash", &["bash"], Some("/home/u/alpha")),
            p(120, 100, "bash", &["bash"], Some("/home/u/beta")),
            p(130, 100, "bash", &["bash"], Some("/home/u/gamma")),
        ];
        let mut fx = Fixture::new(
            TerminalKind::Konsole,
            all,
            vec![window("w", 100, "beta : bash — Konsole")],
        );
        // The process shows three windows; only one was selected.
        fx.total.insert(100, 3);
        let out = fx.run();
        assert_eq!(
            summary(&out.sessions),
            vec![(
                Some("beta : bash — Konsole".into()),
                Some(abs("home/u/beta"))
            )]
        );
        assert!(
            out.warnings.iter().any(|w| w.contains("2 sessions")),
            "{:?}",
            out.warnings
        );
    }

    #[test]
    fn windows_without_readable_shells_get_a_session_without_folder() {
        let all = vec![
            p(100, 1, "alacritty", &[], None),
            p(200, 1, "alacritty", &[], None),
        ];
        let fx = Fixture::new(
            TerminalKind::Alacritty,
            all,
            vec![window("a", 100, "one"), window("b", 200, "two")],
        );
        let out = fx.run();
        assert_eq!(
            summary(&out.sessions),
            vec![(Some("one".into()), None), (Some("two".into()), None)]
        );
        assert_eq!(out.warnings.len(), 1);
        assert!(
            out.warnings[0].contains("could not be read"),
            "{:?}",
            out.warnings
        );

        // tmux instead of a shell (its server runs outside the terminal).
        let all = vec![
            p(100, 1, "alacritty", &[], None),
            p(
                101,
                100,
                "tmux: client",
                &["tmux", "attach"],
                Some("/home/u"),
            ),
        ];
        let fx = Fixture::new(TerminalKind::Alacritty, all, vec![window("a", 100, "tmux")]);
        let out = fx.run();
        assert_eq!(summary(&out.sessions), vec![(Some("tmux".into()), None)]);
        assert_eq!(out.warnings.len(), 1);
        assert!(
            out.warnings[0].contains("no shell was found in 1 window"),
            "{:?}",
            out.warnings
        );

        // Shell visible but its folder is not.
        let all = vec![
            p(100, 1, "alacritty", &[], None),
            p(101, 100, "bash", &["bash"], None),
        ];
        let fx = Fixture::new(TerminalKind::Alacritty, all, vec![window("a", 100, "one")]);
        let out = fx.run();
        assert_eq!(summary(&out.sessions), vec![(Some("one".into()), None)]);
        assert!(out.warnings[0].contains("folder of 1 session"));
    }

    #[test]
    fn more_windows_than_sessions_keeps_every_window() {
        let all = vec![
            p(100, 1, "WindowsTerminal.exe", &[], None),
            p(101, 100, "pwsh.exe", &["pwsh.exe"], Some("/Users/u/x")),
        ];
        let fx = Fixture::new(
            TerminalKind::WindowsTerminal,
            all,
            vec![window("a", 100, "PowerShell"), window("b", 100, "Ubuntu")],
        );
        let out = fx.run();
        assert_eq!(
            summary(&out.sessions),
            vec![
                (Some("PowerShell".into()), Some(abs("Users/u/x"))),
                (Some("Ubuntu".into()), None),
            ]
        );
        // Windows Terminal profiles are not detectable.
        assert!(out.sessions.iter().all(|s| s.profile.is_none()));
    }

    #[test]
    fn windows_console_owned_by_shell_or_console_host() {
        let all = vec![
            // Owned by the shell (the usual Win32 report), with a nested PowerShell.
            p(200, 1, "cmd.exe", &["cmd.exe"], Some("/Users/u")),
            p(
                201,
                200,
                "powershell.exe",
                &["powershell.exe"],
                Some("/Users/u/ps"),
            ),
            // Owned by conhost.exe, whose parent is the shell.
            p(300, 1, "pwsh.exe", &["pwsh.exe"], Some("/Users/u/seven")),
            p(301, 300, "conhost.exe", &["conhost.exe", "0x4"], None),
        ];
        let fx = Fixture::new(
            TerminalKind::WindowsConsole,
            all,
            vec![
                window("a", 200, "Command Prompt - powershell"),
                window("b", 301, "PowerShell 7"),
            ],
        );
        let out = fx.run();
        assert_eq!(
            summary(&out.sessions),
            vec![
                (
                    Some("Command Prompt - powershell".into()),
                    Some(abs("Users/u/ps"))
                ),
                (Some("PowerShell 7".into()), Some(abs("Users/u/seven"))),
            ]
        );
        assert_eq!(
            out.sessions[0].profile.as_deref(),
            Some("Windows PowerShell")
        );
        assert_eq!(out.sessions[1].profile.as_deref(), Some("PowerShell"));
        assert_eq!(out.console_shell, Some(200));
    }

    #[test]
    fn capture_never_stores_commands() {
        let secrets = [
            "export API_TOKEN=hunter2",
            "postgres://admin:s3cret@db/prod",
            "ssh deploy@prod.example.com",
            "--password=letmein",
        ];
        let all = vec![
            p(
                100,
                1,
                "gnome-terminal-",
                &["/usr/libexec/gnome-terminal-server"],
                None,
            ),
            p(
                101,
                100,
                "bash",
                &["bash", "--rcfile", secrets[0]],
                Some("/home/u/a"),
            ),
            p(102, 101, "psql", &["psql", secrets[1]], Some("/home/u/a")),
            p(103, 100, "bash", &["bash"], Some("/home/u/b")),
            p(
                104,
                103,
                "ssh",
                &["ssh", secrets[2], secrets[3]],
                Some("/home/u/b"),
            ),
            p(105, 100, "sh", &["sh", "-c", secrets[0]], Some("/home/u/c")),
        ];
        let fx = Fixture::new(
            TerminalKind::GnomeTerminal,
            all,
            vec![window("w", 100, "Terminal")],
        );
        let out = fx.run();
        assert_eq!(out.sessions.len(), 3);
        for session in &out.sessions {
            assert_eq!(session.startup_command, None);
            assert!(!session.startup_command_approved);
        }
        let saved = serde_json::to_string(&out.sessions).unwrap() + &out.warnings.join("\n");
        for secret in secrets {
            assert!(!saved.contains(secret), "{secret} leaked into {saved}");
        }
        for program in ["psql", "ssh", "--rcfile"] {
            assert!(!saved.contains(program), "{program} leaked into {saved}");
        }
    }

    #[test]
    fn caps_sessions_keeping_every_window_first() {
        let mut all = vec![
            p(100, 1, "konsole", &[], None),
            p(200, 1, "konsole", &[], None),
        ];
        for i in 0..40 {
            let dir = format!("/s/{i}");
            all.push(p(1000 + i, 100, "bash", &["bash"], Some(dir.as_str())));
        }
        all.push(p(3000, 200, "bash", &["bash"], Some("/other")));
        let fx = Fixture::new(
            TerminalKind::Konsole,
            all,
            vec![window("a", 100, "many"), window("b", 200, "single")],
        );
        let out = fx.run();
        assert_eq!(out.sessions.len(), MAX_SESSIONS);
        // The second window survived the cap.
        assert!(
            out.sessions
                .iter()
                .any(|s| s.working_directory == Some(abs("other")))
        );
        assert!(out.warnings.iter().any(|w| w.contains("first 30 of 41")));
    }

    #[test]
    fn title_scores() {
        let home = Some(Path::new("/home/u"));
        let dir = Path::new("/home/u/code/reframe");
        assert_eq!(title_score("u@h: /home/u/code/reframe", Some(dir), home), 3);
        assert_eq!(title_score("u@h: ~/code/reframe", Some(dir), home), 3);
        assert_eq!(title_score("reframe — -zsh — 80×24", Some(dir), home), 1);
        assert_eq!(title_score("Terminal", Some(dir), home), 0);
        assert_eq!(title_score("Terminal", None, home), 0);
    }

    #[test]
    fn full_capture_through_the_adapter_entry_point() {
        let platform = MockPlatform::new();
        let info = platform.info();
        platform.install(
            "gnome-terminal",
            LaunchTarget::Executable {
                path: "/usr/bin/gnome-terminal".into(),
            },
        );
        let ctx = CaptureContext {
            platform: &platform,
            info: &info,
            browser_tab_capture: false,
        };
        let mut server = p(100, 1, "gnome-terminal-", &[], None);
        server.executable_path = Some("/usr/libexec/gnome-terminal-server".into());
        let all = vec![server, p(101, 100, "bash", &["bash"], Some("/home/u/x"))];
        let fx = Fixture::new(
            TerminalKind::GnomeTerminal,
            all,
            vec![window("w", 100, "Terminal")],
        );
        let out = capture(&ctx, &fx.input());
        // The launcher, not the server, is saved; no arguments are ever captured.
        assert_eq!(
            out.launch.executable.as_deref(),
            Some(Path::new("/usr/bin/gnome-terminal"))
        );
        assert!(out.launch.arguments.is_empty());
        assert_eq!(
            out.state,
            AppState::Terminal {
                sessions: vec![TerminalSession {
                    working_directory: Some(abs("home/u/x")),
                    profile: None,
                    startup_command: None,
                    startup_command_approved: false,
                    window_title: Some("Terminal".into()),
                }]
            }
        );

        // Launcher not installed: warn instead of saving the server binary.
        let bare = MockPlatform::new();
        let ctx = CaptureContext {
            platform: &bare,
            info: &info,
            browser_tab_capture: false,
        };
        let out = capture(&ctx, &fx.input());
        assert_eq!(out.launch.executable, None);
        assert!(out.warnings.iter().any(|w| w.contains("was not found")));
    }

    #[test]
    fn launcher_usability() {
        use TerminalKind::*;
        assert!(usable_launcher(Kitty, Path::new("/usr/bin/kitty")));
        assert!(usable_launcher(
            Kitty,
            Path::new("/Applications/kitty.app/Contents/MacOS/kitty")
        ));
        assert!(!usable_launcher(
            GnomeTerminal,
            Path::new("/usr/libexec/gnome-terminal-server")
        ));
        assert!(!usable_launcher(
            Terminator,
            Path::new("/usr/bin/python3.12")
        ));
        assert!(!usable_launcher(Urxvt, Path::new("/usr/bin/urxvtd")));
        assert!(usable_launcher(WezTerm, Path::new("/usr/bin/wezterm-gui")));
        assert!(usable_launcher(
            Alacritty,
            Path::new("/opt/my-alacritty-build")
        ));
    }
}
