//! Restore planning: saved sessions → terminal launches.
//!
//! [`plan_steps`] is a pure function of (terminal, OS, sessions, resolved programs);
//! [`launch_plan`] resolves the programs and checks folders, then calls it.
//!
//! - **Window grouping**: a session with a `window_title` starts a window; a session without one
//!   is a further tab of the window before it (the encoding written by capture).
//! - **Syntax** follows the program actually launched: an entry the user re-pointed at another
//!   known terminal (or that an import mapped to one) gets that terminal's options.
//! - **Tabs**: Windows Terminal, GNOME Terminal, Xfce Terminal and MATE Terminal open a whole
//!   window with its tabs from one command line (one launch per window). Konsole, Terminator and
//!   Ptyxis open the window, then add each further tab with a follow-up launch handled by the
//!   running instance (`expected_windows: 0`); if no running instance picks it up (the launches
//!   ran too close together, or Konsole is not set to run all windows in one process), the tab
//!   opens as a window of its own, so no session is lost. Tilix is not asked for tabs: its tab
//!   action fails outright unless Tilix is already running. Every other terminal opens one window
//!   per session.
//! - **Missing folders**: the window still opens, in the terminal's default folder, and the
//!   session's startup command is *not* run (it was approved for that folder).
//! - **Startup commands** run only when `startup_command_approved` is true; unapproved commands
//!   are discarded by [`prepare_sessions`] and never reach the planner. So are commands that
//!   contain control characters other than line breaks, or invisible text-direction overrides:
//!   what runs must be exactly what the user read when approving it. The command then hands over
//!   to an interactive shell: `$SHELL` on Linux and macOS; on Windows PowerShell, or Command
//!   Prompt for sessions whose profile is Command Prompt. Not supported (the folder still
//!   opens): QTerminal, Deepin Terminal, Warp, terminals Reframe does not know, Terminology when
//!   `$SHELL` is neither a POSIX shell nor fish, and macOS Terminal / iTerm2 sessions whose folder
//!   name contains a character some shell reads specially even inside quotes (see
//!   [`quote_for_any_shell`]).

use std::path::{Path, PathBuf};

use reframe_platform::{
    LaunchRequest, LaunchTarget, LocateQuery, OsKind, PackageInfo, PackageKind,
};

use super::capture::{dir_label, usable_launcher};
use super::escape::{
    CMD_STARTUP_VARIABLE, CmdStartup, applescript_string, cmd_command_line, cmd_startup,
    posix_join, quote_for_any_shell, wt_escape,
};
use super::kind::{TerminalKind, kind_for_target, locate_query_for};
use super::shell::{
    ShellFamily, family_of_name, normalize_name, powershell_startup_args, startup_argv,
};
use crate::adapters::generic::launch_target;
use crate::adapters::{LaunchContext, LaunchStep};
use crate::model::{AppState, ApplicationEntry};

/// Most sessions restored per entry (protects against oversized or hostile workspaces).
pub const MAX_RESTORED_SESSIONS: usize = 50;

/// Longest profile name passed on to a terminal.
const MAX_PROFILE_LEN: usize = 200;

const OSASCRIPT: &str = "/usr/bin/osascript";
const OPEN: &str = "/usr/bin/open";

/// A session ready to be planned: folder verified, startup command approved.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlannedSession {
    /// Existing folder, or `None` for the terminal's default.
    pub directory: Option<PathBuf>,
    pub profile: Option<String>,
    /// An approved, non-empty startup command. Never set for unapproved commands.
    pub command: Option<String>,
    /// Starts a new window; otherwise the session is another tab of the previous window.
    pub new_window: bool,
}

/// Characters that make text read differently from what it is: bidirectional marks, overrides
/// and isolates, and Unicode line/paragraph separators.
fn is_deceptive(c: char) -> bool {
    matches!(
        c,
        '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{2028}'
            | '\u{2029}'
    )
}

/// An approved startup command as it will run: line endings normalised to `\n`, surrounding
/// whitespace trimmed. `None` when nothing is left, or when the command contains a control
/// character other than a line break (a tab triggers completion when typed into a shell, escape
/// sequences can hide text) or a deceptive character (see [`is_deceptive`]).
pub fn usable_command(raw: &str) -> Option<String> {
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    let command = normalized.trim();
    let clean = command
        .chars()
        .all(|c| c == '\n' || !(c.is_control() || is_deceptive(c)));
    (clean && !command.is_empty()).then(|| command.to_string())
}

/// A profile name that is safe to pass as an option value: not empty, no control or deceptive
/// characters, not too long, and not starting with `-` (it could be taken for an option when
/// passed as a separate argument, e.g. `wt.exe -p <profile>`).
pub fn usable_profile(raw: &str) -> Option<String> {
    let profile = raw.trim();
    let clean = !profile.is_empty()
        && !profile.starts_with('-')
        && profile.chars().count() <= MAX_PROFILE_LEN
        && !profile.chars().any(|c| c.is_control() || is_deceptive(c));
    clean.then(|| profile.to_string())
}

/// Turn saved state into plannable sessions. Unapproved startup commands are dropped here;
/// sessions whose folder no longer exists keep their window but lose the folder and the command.
/// Folders must be absolute and valid Unicode (launch arguments are text; a lossy conversion
/// could name a different folder). Without saved sessions one window opens in `fallback_dir`
/// (the entry's working directory).
pub fn prepare_sessions(
    state: &AppState,
    fallback_dir: Option<&Path>,
    dir_exists: impl Fn(&Path) -> bool,
) -> Vec<PlannedSession> {
    let usable_dir = |dir: &Path| dir.is_absolute() && dir.to_str().is_some() && dir_exists(dir);
    let sessions = match state {
        AppState::Terminal { sessions } if !sessions.is_empty() => sessions,
        _ => {
            return vec![PlannedSession {
                directory: fallback_dir
                    .filter(|d| usable_dir(d))
                    .map(Path::to_path_buf),
                new_window: true,
                ..Default::default()
            }];
        }
    };
    sessions
        .iter()
        .take(MAX_RESTORED_SESSIONS)
        .enumerate()
        .map(|(i, session)| {
            let saved = session
                .working_directory
                .as_deref()
                .filter(|d| !d.as_os_str().is_empty());
            let directory = saved.filter(|d| usable_dir(d)).map(Path::to_path_buf);
            let folder_missing = saved.is_some() && directory.is_none();
            let command = session
                .startup_command
                .as_deref()
                .filter(|_| session.startup_command_approved && !folder_missing)
                .and_then(usable_command);
            PlannedSession {
                directory,
                profile: session.profile.as_deref().and_then(usable_profile),
                command,
                new_window: i == 0 || session.window_title.is_some(),
            }
        })
        .collect()
}

/// Shell executables for classic console sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleShells {
    pub cmd: PathBuf,
    pub powershell: PathBuf,
    pub pwsh: PathBuf,
}

impl Default for ConsoleShells {
    fn default() -> Self {
        Self {
            cmd: "cmd.exe".into(),
            powershell: "powershell.exe".into(),
            pwsh: "pwsh.exe".into(),
        }
    }
}

/// Everything the planner needs besides the sessions, resolved by the caller.
#[derive(Debug, Clone)]
pub struct PlanEnv {
    pub os: OsKind,
    /// The program that opens the terminal's windows (launcher, package or helper).
    pub target: LaunchTarget,
    /// Arguments the user configured on the entry; passed first to executable launchers.
    pub user_args: Vec<String>,
    /// Shell that runs approved startup commands in Unix terminals (`$SHELL`).
    pub posix_shell: PathBuf,
    /// PowerShell that runs approved startup commands on Windows (Windows Terminal tabs,
    /// Alacritty, WezTerm).
    pub wt_powershell: PathBuf,
    pub console: ConsoleShells,
    /// The user's home folder: macOS terminals open it when a session has no folder (opening
    /// the bare application would only activate an already running instance).
    pub home: Option<PathBuf>,
}

fn group_windows(sessions: &[PlannedSession]) -> Vec<Vec<&PlannedSession>> {
    let mut groups: Vec<Vec<&PlannedSession>> = Vec::new();
    for session in sessions {
        match groups.last_mut() {
            Some(group) if !session.new_window => group.push(session),
            _ => groups.push(vec![session]),
        }
    }
    groups
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn title_hint(session: Option<&PlannedSession>) -> Option<String> {
    session
        .and_then(|s| s.directory.as_deref())
        .map(dir_label)
        .filter(|label| !label.is_empty())
}

fn executable(path: &str) -> LaunchTarget {
    LaunchTarget::Executable { path: path.into() }
}

fn step(
    target: LaunchTarget,
    args: Vec<String>,
    working_directory: Option<PathBuf>,
    expected_windows: u32,
    hint: Option<String>,
) -> LaunchStep {
    LaunchStep {
        request: LaunchRequest {
            target,
            args,
            working_directory,
            env: Vec::new(),
        },
        expected_windows,
        title_hint: hint,
    }
}

/// Arguments that run an approved command in the user's shell and then keep it interactive:
/// `$SHELL` on Unix (a login shell on macOS), PowerShell on Windows (Alacritty, WezTerm).
fn startup(env: &PlanEnv, command: &str) -> Vec<String> {
    if env.os == OsKind::Windows {
        return powershell_startup_args(&env.wt_powershell, command);
    }
    startup_argv(&env.posix_shell, command, env.os == OsKind::Macos)
}

/// Whether text quoted with [`posix_join`] means the same to `shell` as to Reframe: POSIX shells
/// and fish (the quoting is tested against both).
fn reads_posix_quoting(shell: &Path) -> bool {
    matches!(
        shell
            .file_name()
            .and_then(|n| family_of_name(&n.to_string_lossy())),
        Some(ShellFamily::Posix | ShellFamily::Fish)
    )
}

/// Launch steps for `sessions` (see the module docs). `kind` is `None` for a terminal Reframe
/// has no dedicated knowledge of: its windows open with the folder as working directory.
pub fn plan_steps(
    kind: Option<TerminalKind>,
    env: &PlanEnv,
    sessions: &[PlannedSession],
) -> Vec<LaunchStep> {
    use TerminalKind as K;
    let windows = group_windows(sessions);
    match kind {
        Some(K::WindowsTerminal) => windows.iter().map(|w| windows_terminal(env, w)).collect(),
        Some(K::WindowsConsole) => sessions.iter().map(|s| windows_console(env, s)).collect(),
        Some(K::GnomeTerminal) => windows
            .iter()
            .flat_map(|w| gnome_terminal(env, w))
            .collect(),
        Some(K::Xfce4Terminal) => windows
            .iter()
            .map(|w| tabbed_command_line(env, w, &XFCE))
            .collect(),
        Some(K::MateTerminal) => windows
            .iter()
            .map(|w| tabbed_command_line(env, w, &MATE))
            .collect(),
        Some(k @ (K::Konsole | K::Terminator | K::Ptyxis)) => windows
            .iter()
            .flat_map(|w| follow_up_tabs(env, k, w))
            .collect(),
        Some(k @ (K::MacTerminal | K::ITerm2 | K::Warp)) => {
            sessions.iter().map(|s| mac_open(env, k, s)).collect()
        }
        Some(K::Ghostty) if env.os == OsKind::Macos => {
            sessions.iter().map(|s| ghostty_macos(env, s)).collect()
        }
        Some(k) => sessions.iter().map(|s| one_window(env, k, s)).collect(),
        None => sessions
            .iter()
            .map(|s| {
                step(
                    env.target.clone(),
                    env.user_args.clone(),
                    s.directory.clone(),
                    1,
                    title_hint(Some(s)),
                )
            })
            .collect(),
    }
}

/// The program and arguments that run an approved command in a Windows Terminal tab: Command
/// Prompt for the Command Prompt profile, Windows PowerShell for that profile, otherwise
/// PowerShell (7 when installed).
fn windows_terminal_command(env: &PlanEnv, profile: Option<&str>, command: &str) -> Vec<String> {
    match profile.and_then(console_shell_name) {
        Some("cmd") => vec![
            path_arg(&env.console.cmd),
            "/K".to_string(),
            cmd_command_line(command),
        ],
        Some("powershell") => powershell_startup_args(&env.console.powershell, command),
        _ => powershell_startup_args(&env.wt_powershell, command),
    }
}

/// `wt.exe -w new new-tab -d <dir> [-p <profile>] [<command>] ; new-tab ...`
///
/// Every value is escaped for wt's `;` splitter. wt re-joins a tab's trailing arguments into its
/// command line by wrapping those that contain a space in plain quotes, without escaping embedded
/// quotes or backslashes (`AppCommandlineArgs::_getNewTerminalArgs`). PowerShell therefore gets
/// the command as `-EncodedCommand` (Base64: nothing to re-quote), and Command Prompt gets
/// `cmd.exe /K <text>`, whose `/K` strips exactly the quotes wt adds, so `cmd.exe` receives the
/// text verbatim, embedded quotes included.
fn windows_terminal(env: &PlanEnv, window: &[&PlannedSession]) -> LaunchStep {
    let mut args = env.user_args.clone();
    args.extend(["-w".to_string(), "new".to_string()]);
    for (i, session) in window.iter().enumerate() {
        if i > 0 {
            args.push(";".into());
        }
        args.push("new-tab".into());
        if let Some(dir) = &session.directory {
            args.extend(["-d".to_string(), wt_escape(&path_arg(dir))]);
        }
        if let Some(profile) = &session.profile {
            args.extend(["-p".to_string(), wt_escape(profile)]);
        }
        if let Some(command) = &session.command {
            let program = windows_terminal_command(env, session.profile.as_deref(), command);
            args.extend(program.iter().map(|a| wt_escape(a)));
        }
    }
    step(
        env.target.clone(),
        args,
        None,
        1,
        title_hint(window.first().copied()),
    )
}

/// The console shell a profile names (profiles of console sessions record their shell, with
/// Windows Terminal's default profile names).
fn console_shell_name(profile: &str) -> Option<&'static str> {
    match profile.trim().to_lowercase().as_str() {
        "command prompt" | "cmd" | "cmd.exe" => Some("cmd"),
        "windows powershell" | "powershell.exe" => Some("powershell"),
        "powershell" | "powershell 7" | "pwsh" | "pwsh.exe" => Some("pwsh"),
        _ => None,
    }
}

/// Classic console: start the shell itself in the folder; Windows gives it a console window.
fn windows_console(env: &PlanEnv, session: &PlannedSession) -> LaunchStep {
    let entry_shell = match &env.target {
        LaunchTarget::Executable { path } => Some(path.clone()),
        _ => None,
    };
    let wanted = session.profile.as_deref().and_then(console_shell_name);
    let shell = match (wanted, &entry_shell) {
        (Some(name), Some(path)) if normalize_name(&path_arg(path)) == name => path.clone(),
        (Some("cmd"), _) => env.console.cmd.clone(),
        (Some("powershell"), _) => env.console.powershell.clone(),
        (Some(_), _) => env.console.pwsh.clone(),
        (None, Some(path)) => path.clone(),
        (None, None) => env.console.cmd.clone(),
    };
    let mut args = if entry_shell.as_ref() == Some(&shell) {
        env.user_args.clone()
    } else {
        Vec::new()
    };
    let mut vars = Vec::new();
    if let Some(command) = &session.command {
        let family = shell
            .file_name()
            .and_then(|n| family_of_name(&n.to_string_lossy()));
        match family {
            Some(ShellFamily::Cmd) => match cmd_startup(command) {
                CmdStartup::Argument(text) => args.extend(["/K".to_string(), text]),
                CmdStartup::Environment(text) => {
                    args.extend(["/K".to_string(), format!("%{CMD_STARTUP_VARIABLE}%")]);
                    vars.push((CMD_STARTUP_VARIABLE.to_string(), text));
                }
            },
            Some(ShellFamily::PowerShell) => {
                args.extend(powershell_startup_args(&shell, command).into_iter().skip(1));
            }
            _ => args.extend(startup_argv(&shell, command, false).into_iter().skip(1)),
        }
    }
    LaunchStep {
        request: LaunchRequest {
            target: LaunchTarget::Executable { path: shell },
            args,
            working_directory: session.directory.clone(),
            env: vars,
        },
        expected_windows: 1,
        title_hint: title_hint(Some(session)),
    }
}

/// `gnome-terminal --window --working-directory=<dir> [--profile=<p>] --tab ...`
///
/// A session with an approved command opens in its own window
/// (`--window ... -- <shell> -c ...`): gnome-terminal's `--` command applies to a single
/// terminal, and the per-tab `-e` option is deprecated.
fn gnome_terminal(env: &PlanEnv, window: &[&PlannedSession]) -> Vec<LaunchStep> {
    let terminal_options = |session: &PlannedSession, args: &mut Vec<String>| {
        if let Some(dir) = &session.directory {
            args.push(format!("--working-directory={}", path_arg(dir)));
        }
        if let Some(profile) = &session.profile {
            args.push(format!("--profile={profile}"));
        }
    };
    let (plain, with_command): (Vec<&PlannedSession>, Vec<&PlannedSession>) =
        window.iter().copied().partition(|s| s.command.is_none());
    let mut steps = Vec::new();
    if !plain.is_empty() {
        let mut args = env.user_args.clone();
        for (i, session) in plain.iter().enumerate() {
            args.push(if i == 0 { "--window" } else { "--tab" }.to_string());
            terminal_options(session, &mut args);
        }
        steps.push(step(
            env.target.clone(),
            args,
            None,
            1,
            title_hint(plain.first().copied()),
        ));
    }
    for session in with_command {
        let mut args = env.user_args.clone();
        args.push("--window".into());
        terminal_options(session, &mut args);
        if let Some(command) = &session.command {
            args.push("--".into());
            args.extend(startup(env, command));
        }
        steps.push(step(
            env.target.clone(),
            args,
            None,
            1,
            title_hint(Some(session)),
        ));
    }
    steps
}

/// Option spelling of terminals that take `--window ... --tab ...` with per-tab options.
struct TabFlags {
    window: &'static str,
    tab: &'static str,
    window_with_profile: Option<&'static str>,
    tab_with_profile: Option<&'static str>,
}

const XFCE: TabFlags = TabFlags {
    window: "--window",
    tab: "--tab",
    window_with_profile: None,
    tab_with_profile: None,
};

const MATE: TabFlags = TabFlags {
    window: "--window",
    tab: "--tab",
    window_with_profile: Some("--window-with-profile="),
    tab_with_profile: Some("--tab-with-profile="),
};

/// Xfce Terminal / MATE Terminal: one command line per window, approved commands per tab via
/// `--command=<string>` (split with `g_shell_parse_argv` and run directly, hence POSIX-quoted).
fn tabbed_command_line(env: &PlanEnv, window: &[&PlannedSession], flags: &TabFlags) -> LaunchStep {
    let mut args = env.user_args.clone();
    for (i, session) in window.iter().enumerate() {
        let (plain, with_profile) = if i == 0 {
            (flags.window, flags.window_with_profile)
        } else {
            (flags.tab, flags.tab_with_profile)
        };
        match (with_profile, &session.profile) {
            (Some(prefix), Some(profile)) => args.push(format!("{prefix}{profile}")),
            _ => args.push(plain.to_string()),
        }
        if let Some(dir) = &session.directory {
            args.push(format!("--working-directory={}", path_arg(dir)));
        }
        if let Some(command) = &session.command {
            args.push(format!("--command={}", posix_join(&startup(env, command))));
        }
    }
    step(
        env.target.clone(),
        args,
        None,
        1,
        title_hint(window.first().copied()),
    )
}

/// Konsole, Terminator, Ptyxis: the first session opens the window, further sessions are added
/// as tabs by the running instance (or open their own window when there is none).
fn follow_up_tabs(
    env: &PlanEnv,
    kind: TerminalKind,
    window: &[&PlannedSession],
) -> Vec<LaunchStep> {
    window
        .iter()
        .enumerate()
        .map(|(i, session)| {
            let first = i == 0;
            let dir = session.directory.as_deref().map(path_arg);
            let command = session.command.as_deref().map(|c| startup(env, c));
            let mut args = env.user_args.clone();
            match kind {
                TerminalKind::Konsole => {
                    if !first {
                        args.push("--new-tab".into());
                    }
                    if let Some(profile) = &session.profile {
                        args.extend(["--profile".to_string(), profile.clone()]);
                    }
                    if let Some(dir) = dir {
                        args.extend(["--workdir".to_string(), dir]);
                    }
                    if let Some(command) = command {
                        args.push("-e".into());
                        args.extend(command);
                    }
                }
                TerminalKind::Terminator => {
                    if !first {
                        args.push("--new-tab".into());
                    }
                    if let Some(dir) = dir {
                        args.push(format!("--working-directory={dir}"));
                    }
                    if let Some(profile) = &session.profile {
                        args.push(format!("--profile={profile}"));
                    }
                    if let Some(command) = command {
                        args.push("-x".into());
                        args.extend(command);
                    }
                }
                _ => {
                    // Ptyxis.
                    args.push(if first { "--new-window" } else { "--tab" }.into());
                    if let Some(dir) = dir {
                        args.push(format!("--working-directory={dir}"));
                    }
                    if let Some(command) = command {
                        args.push("--".into());
                        args.extend(command);
                    }
                }
            }
            step(
                env.target.clone(),
                args,
                None,
                u32::from(first),
                title_hint(Some(session)),
            )
        })
        .collect()
}

/// Terminals that open one window per launch.
fn one_window(env: &PlanEnv, kind: TerminalKind, session: &PlannedSession) -> LaunchStep {
    use TerminalKind as K;
    let dir = session.directory.as_deref().map(path_arg);
    let command = session.command.as_deref().map(|c| startup(env, c));
    let mut args = env.user_args.clone();
    let mut working_directory = None;
    let mut vars = Vec::new();
    match kind {
        K::GnomeConsole => {
            args.extend(dir.map(|d| format!("--working-directory={d}")));
            if let Some(command) = command {
                args.push("--".into());
                args.extend(command);
            }
        }
        // `--command=<string>`: split with `g_shell_parse_argv` and run directly.
        K::LxTerminal => {
            args.extend(dir.map(|d| format!("--working-directory={d}")));
            args.extend(command.map(|c| format!("--command={}", posix_join(&c))));
        }
        K::Tilix => {
            args.extend(dir.map(|d| format!("--working-directory={d}")));
            args.extend(session.profile.as_ref().map(|p| format!("--profile={p}")));
            args.extend(command.map(|c| format!("--command={}", posix_join(&c))));
        }
        // `-e` takes the rest of the command line, joins it with spaces and runs the result with
        // `$SHELL -c`: one argument, quoted for a shell whose syntax the quoting is proven for,
        // with SHELL pinned to that shell (Terminology falls back to the passwd entry otherwise).
        K::Terminology => {
            args.extend(dir.map(|d| format!("--current-directory={d}")));
            if let Some(command) = command.filter(|_| reads_posix_quoting(&env.posix_shell)) {
                args.extend(["-e".to_string(), posix_join(&command)]);
                vars.push(("SHELL".to_string(), path_arg(&env.posix_shell)));
            }
        }
        // No reliable way to pass a command line: the folder opens, the command is not run.
        K::QTerminal => {
            if let Some(dir) = dir {
                args.extend(["--workdir".to_string(), dir]);
            }
        }
        K::DeepinTerminal => {
            if let Some(dir) = dir {
                args.extend(["--work-directory".to_string(), dir]);
            }
        }
        K::Alacritty => {
            if let Some(dir) = dir {
                args.extend(["--working-directory".to_string(), dir]);
            }
            if let Some(command) = command {
                args.push("-e".into());
                args.extend(command);
            }
        }
        K::Kitty => {
            if let Some(dir) = dir {
                args.extend(["--directory".to_string(), dir]);
            }
            args.extend(command.into_iter().flatten());
        }
        K::WezTerm => {
            args.push("start".into());
            if let Some(dir) = dir {
                args.extend(["--cwd".to_string(), dir]);
            }
            if let Some(command) = command {
                args.push("--".into());
                args.extend(command);
            }
        }
        K::Foot => {
            args.extend(dir.map(|d| format!("--working-directory={d}")));
            args.extend(command.into_iter().flatten());
        }
        K::Ghostty => {
            args.extend(dir.map(|d| format!("--working-directory={d}")));
            if let Some(command) = command {
                args.push("-e".into());
                args.extend(command);
            }
        }
        K::Xterm | K::Urxvt => {
            working_directory = session.directory.clone();
            if let Some(command) = command {
                args.push("-e".into());
                args.extend(command);
            }
        }
        _ => working_directory = session.directory.clone(),
    }
    LaunchStep {
        request: LaunchRequest {
            target: env.target.clone(),
            args,
            working_directory,
            env: vars,
        },
        expected_windows: 1,
        title_hint: title_hint(Some(session)),
    }
}

/// macOS Terminal, iTerm2, Warp: opening a folder with the application opens a new window there.
/// An approved command or a profile needs AppleScript (Terminal, iTerm2; the user grants
/// Automation permission on first use). When that is not possible (Warp has no scripting
/// interface; a folder name some shell would misread, see [`applescript_step`]), only the folder
/// is restored.
fn mac_open(env: &PlanEnv, kind: TerminalKind, session: &PlannedSession) -> LaunchStep {
    let scripted =
        kind != TerminalKind::Warp && (session.command.is_some() || session.profile.is_some());
    if scripted && let Some(step) = applescript_step(kind, session) {
        return step;
    }
    let folder = session.directory.clone().or_else(|| env.home.clone());
    step(
        env.target.clone(),
        folder.map(|f| vec![path_arg(&f)]).unwrap_or_default(),
        None,
        1,
        title_hint(Some(session)),
    )
}

/// `osascript -e 'tell application id "<bundle>"' -e ...`: a new window that types
/// `cd '<dir>' && <command>` into the interactive shell (Terminal's `do script`, iTerm2's
/// `write text`). The user's login shell could be zsh, bash, fish, tcsh, nu or PowerShell, so
/// the folder is typed only if it reads the same in all of them ([`quote_for_any_shell`]);
/// otherwise `None`. `&&` keeps the command from running anywhere but in its folder (shells
/// without `&&`, such as nu, reject the line). The whole text is escaped as an AppleScript
/// string. If Terminal was not running it may also open its default window.
fn applescript_step(kind: TerminalKind, session: &PlannedSession) -> Option<LaunchStep> {
    let folder = match &session.directory {
        Some(dir) => Some(quote_for_any_shell(dir.to_str()?)?),
        None => None,
    };
    let typed = match (folder, &session.command) {
        (Some(folder), Some(command)) => format!("cd {folder} && {command}"),
        (Some(folder), None) => format!("cd {folder}"),
        (None, Some(command)) => command.clone(),
        (None, None) => String::new(),
    };
    let bundle = kind.bundle_id()?;
    let mut lines = vec![
        format!("tell application id {}", applescript_string(bundle)),
        "activate".to_string(),
    ];
    if kind == TerminalKind::ITerm2 {
        lines.push(match &session.profile {
            Some(profile) => format!(
                "set newWindow to (create window with profile {})",
                applescript_string(profile)
            ),
            None => "set newWindow to (create window with default profile)".to_string(),
        });
        if !typed.is_empty() {
            lines.push(format!(
                "tell current session of newWindow to write text {}",
                applescript_string(&typed)
            ));
        }
    } else {
        lines.push(format!(
            "set newTab to do script {}",
            applescript_string(&typed)
        ));
        if let Some(profile) = &session.profile {
            lines.push(format!(
                "set current settings of newTab to settings set {}",
                applescript_string(profile)
            ));
        }
    }
    lines.push("end tell".to_string());
    let args = lines
        .into_iter()
        .flat_map(|line| ["-e".to_string(), line])
        .collect();
    Some(step(
        executable(OSASCRIPT),
        args,
        None,
        1,
        title_hint(Some(session)),
    ))
}

/// Ghostty on macOS is started through Launch Services as its documentation prescribes:
/// `open -n -b com.mitchellh.ghostty --args --working-directory=<dir> [-e ...]`.
fn ghostty_macos(env: &PlanEnv, session: &PlannedSession) -> LaunchStep {
    let bundle = TerminalKind::Ghostty
        .bundle_id()
        .unwrap_or("com.mitchellh.ghostty");
    let mut ghostty_args = env.user_args.clone();
    if let Some(dir) = &session.directory {
        ghostty_args.push(format!("--working-directory={}", path_arg(dir)));
    }
    if let Some(command) = &session.command {
        ghostty_args.push("-e".into());
        ghostty_args.extend(startup(env, command));
    }
    let mut args = vec!["-n".to_string(), "-b".to_string(), bundle.to_string()];
    if !ghostty_args.is_empty() {
        args.push("--args".into());
        args.extend(ghostty_args);
    }
    step(executable(OPEN), args, None, 1, title_hint(Some(session)))
}

// ---- Resolution (I/O) ---------------------------------------------------------------------

/// `$SHELL` when it names an existing file, or the platform's default shell.
fn posix_shell(os: OsKind) -> PathBuf {
    std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.is_file())
        .unwrap_or_else(|| {
            PathBuf::from(if os == OsKind::Macos {
                "/bin/zsh"
            } else {
                "/bin/sh"
            })
        })
}

/// Console shells and the PowerShell used in Windows Terminal tabs (PowerShell 7 when installed,
/// else Windows PowerShell, which every supported Windows version ships).
fn windows_shells(ctx: &LaunchContext<'_>) -> (ConsoleShells, PathBuf) {
    let system32 = std::env::var_os("SystemRoot").map(|root| PathBuf::from(root).join("System32"));
    let cmd = std::env::var_os("ComSpec")
        .map(PathBuf::from)
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("cmd.exe"))
        })
        .or_else(|| system32.as_ref().map(|s| s.join("cmd.exe")))
        .unwrap_or_else(|| PathBuf::from("cmd.exe"));
    let powershell = system32
        .as_ref()
        .map(|s| {
            s.join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe")
        })
        .unwrap_or_else(|| PathBuf::from("powershell.exe"));
    let pwsh = ctx
        .platform
        .locate_application(&LocateQuery {
            executable_names: vec!["pwsh.exe".into(), "pwsh".into()],
            ..Default::default()
        })
        .into_iter()
        .find_map(|target| match target {
            LaunchTarget::Executable { path } => Some(path),
            _ => None,
        });
    let wt_powershell = pwsh.clone().unwrap_or_else(|| powershell.clone());
    (
        ConsoleShells {
            cmd,
            powershell,
            pwsh: pwsh.unwrap_or_else(|| PathBuf::from("pwsh.exe")),
        },
        wt_powershell,
    )
}

/// The executable inside a macOS bundle for terminals driven by command-line flags.
fn bundle_executable(kind: TerminalKind, package: &PackageInfo) -> Option<PathBuf> {
    let bundle = package.path.as_deref()?;
    kind.launcher_names()
        .iter()
        .map(|name| bundle.join("Contents").join("MacOS").join(name))
        .find(|candidate| candidate.is_file())
}

fn locate_launcher(ctx: &LaunchContext<'_>, kind: TerminalKind) -> Option<LaunchTarget> {
    ctx.platform
        .locate_application(&locate_query_for(kind, ctx.info.os))
        .into_iter()
        .find_map(|target| match &target {
            LaunchTarget::Executable { path } if usable_launcher(kind, path) => Some(target),
            LaunchTarget::Package { package } => match package.kind {
                PackageKind::Flatpak | PackageKind::Snap => Some(target),
                PackageKind::MacBundle => {
                    bundle_executable(kind, package).map(|path| LaunchTarget::Executable { path })
                }
                _ => None,
            },
            _ => None,
        })
}

/// The program that opens windows of `kind` on this machine.
fn resolve_target(
    ctx: &LaunchContext<'_>,
    kind: Option<TerminalKind>,
    entry: &ApplicationEntry,
    console: &ConsoleShells,
) -> Option<LaunchTarget> {
    use TerminalKind as K;
    let os = ctx.info.os;
    let Some(kind) = kind else {
        return launch_target(&entry.launch);
    };
    let launch = &entry.launch;
    match kind {
        K::WindowsConsole => Some(LaunchTarget::Executable {
            path: launch
                .executable
                .clone()
                .unwrap_or_else(|| console.cmd.clone()),
        }),
        K::MacTerminal | K::ITerm2 | K::Warp => {
            let package = launch
                .package
                .clone()
                .filter(|p| p.kind == PackageKind::MacBundle)
                .or_else(|| {
                    kind.bundle_id().map(|id| PackageInfo {
                        kind: PackageKind::MacBundle,
                        id: id.to_string(),
                        path: None,
                    })
                })?;
            Some(LaunchTarget::Package { package })
        }
        K::Ghostty if os == OsKind::Macos => Some(executable(OPEN)),
        _ => {
            if let Some(package) = &launch.package
                && matches!(package.kind, PackageKind::Flatpak | PackageKind::Snap)
            {
                return Some(LaunchTarget::Package {
                    package: package.clone(),
                });
            }
            if let Some(path) = launch
                .executable
                .as_deref()
                .filter(|p| usable_launcher(kind, p))
            {
                return Some(LaunchTarget::Executable {
                    path: path.to_path_buf(),
                });
            }
            if let Some(path) = launch
                .package
                .as_ref()
                .filter(|p| p.kind == PackageKind::MacBundle)
                .and_then(|p| bundle_executable(kind, p))
            {
                return Some(LaunchTarget::Executable { path });
            }
            Some(locate_launcher(ctx, kind).unwrap_or_else(|| {
                // Not found: rely on PATH at launch time (a clear launch error if missing).
                let name = kind.launcher_names().first().copied().unwrap_or("");
                let file = if os == OsKind::Windows {
                    format!("{name}.exe")
                } else {
                    name.to_string()
                };
                LaunchTarget::Executable { path: file.into() }
            }))
        }
    }
}

/// Launch steps that restore a terminal entry (see the module docs).
pub fn launch_plan(
    ctx: &LaunchContext<'_>,
    entry: &ApplicationEntry,
) -> Result<Vec<LaunchStep>, String> {
    let os = ctx.info.os;
    let kind = TerminalKind::from_app_id(&entry.app_id);
    let (console, wt_powershell) = if os == OsKind::Windows {
        windows_shells(ctx)
    } else {
        (ConsoleShells::default(), PathBuf::from("powershell.exe"))
    };
    let target = resolve_target(ctx, kind, entry, &console)
        .ok_or_else(|| format!("No executable is configured for {}.", entry.display_name))?;
    // The program actually launched decides the command-line syntax: the user may have pointed
    // the entry at another terminal (or an import mapped it to one).
    let kind = kind_for_target(&target).filter(|k| k.runs_on(os)).or(kind);
    let sessions = prepare_sessions(
        &entry.state,
        entry.launch.working_directory.as_deref(),
        Path::is_dir,
    );
    let env = PlanEnv {
        os,
        target,
        user_args: entry.launch.arguments.clone(),
        posix_shell: posix_shell(os),
        wt_powershell,
        console,
        home: dirs::home_dir(),
    };
    Ok(plan_steps(kind, &env, &sessions))
}
