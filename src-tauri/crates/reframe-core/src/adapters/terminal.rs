//! Terminal emulators: windows, working directories, tabs, profiles and user-approved startup
//! commands.
//!
//! Supported terminals ([`TerminalKind`]): Windows Terminal and classic console windows (cmd,
//! Windows PowerShell, pwsh) on Windows; GNOME Terminal, GNOME Console, Ptyxis, Konsole, Xfce
//! Terminal, MATE Terminal, LXTerminal, QTerminal, Tilix, Terminator, foot, XTerm, rxvt-unicode,
//! Deepin Terminal and Terminology on Linux; Terminal, iTerm2 and Warp on macOS; Alacritty,
//! kitty, WezTerm and Ghostty wherever they run.
//!
//! **Capture** records one session per tab: the innermost interactive shell running in it (the
//! one the user is typing into) and that shell's working directory (`sessions`), arranged per
//! window (`capture`).
//!
//! **Restore** reopens every window, with tabs where the terminal can open them from the command
//! line, in the saved folders (`plan`).
//!
//! **Safety**: command lines, shell history, running programs and scrollback are never captured
//! or replayed. A startup command runs only when the user added it and approved it in Reframe
//! (`startup_command_approved`), only in the folder it was approved for, and only if it contains
//! no control characters (other than line breaks) or invisible text-direction overrides, so that
//! what runs is what the user read. Every value placed on a command line goes through the quoting
//! helpers in [`escape`]. Exports never carry an approval, and leave out startup commands and
//! window titles that look like they contain a credential.
//!
//! Limitations:
//! - Terminals that show several windows from one process (GNOME Terminal, Konsole, Windows
//!   Terminal, Xfce Terminal, Tilix...) do not reveal which tab belongs to which window. Reframe
//!   assigns tabs by window title and the order they were opened, and says so in a warning.
//! - Tabs reopen as tabs in Windows Terminal, GNOME Terminal, Xfce Terminal and MATE Terminal (one
//!   command line per window), and in Konsole, Terminator and Ptyxis (follow-up launches handled
//!   by the running instance; a tab opens as its own window if no instance takes it). Every other
//!   terminal, Tilix included, reopens each tab as a window.
//! - Working directories of shells owned by another user (`sudo -i`, `su`) or, on Windows, of
//!   terminals running as administrator cannot be read. A nested root shell uses the folder of the
//!   shell it was started from; otherwise the window reopens in the terminal's default folder.
//!   Folder names that are not valid Unicode are not saved.
//! - tmux, screen and zellij keep their shells in a server outside the terminal, and SSH sessions
//!   run remotely: such windows reopen in the default folder (with a warning). The same applies
//!   to Flatpak-sandboxed terminals that start shells on the host through `flatpak-spawn`, and to
//!   consoles that Windows handed over to Windows Terminal when it is the default terminal (their
//!   shell is not a child process of Windows Terminal).
//! - Profiles are not detected, except that a console window records its shell (cmd, Windows
//!   PowerShell or pwsh) as its profile. Profiles the user sets are passed to Windows Terminal,
//!   GNOME Terminal, MATE Terminal, Konsole, Terminator, Tilix, and through AppleScript to
//!   Terminal and iTerm2.
//! - Approved startup commands run in `$SHELL` on Linux and macOS, and in PowerShell on Windows
//!   (Command Prompt for sessions whose profile is Command Prompt). They are not run (the folder
//!   still opens) in QTerminal, Deepin Terminal, Warp and terminals Reframe does not know, in
//!   Terminology unless `$SHELL` is a POSIX shell or fish, and in Terminal / iTerm2 when the
//!   folder name contains `'`, `\`, `!`, a typographic quote or a control character (the command
//!   is typed into the login shell, and some shell would misread such a name).
//! - Terminal, iTerm2 and Warp open a new window when asked to open a folder; this relies on the
//!   platform passing a package launch's arguments as documents (`open -b <bundle> <folder>`).
//!   AppleScript (approved commands, profiles) needs the macOS Automation permission.

mod capture;
pub mod escape;
mod kind;
mod plan;
mod sessions;
mod shell;
#[cfg(test)]
mod tests;

use reframe_platform::{LocateQuery, OsKind, PlatformInfo, ProcessInfo, SupportLevel, WindowInfo};

pub use kind::TerminalKind;
use kind::{identify_kind, locate_query_for};

use super::generic::GenericAdapter;
use super::{
    AppAdapter, AppIdentity, CaptureContext, CaptureInput, CaptureOutput, InstanceModel,
    LaunchContext, LaunchStep,
};
use crate::model::{AdapterCapability, AppState, ApplicationEntry, CapabilityLevel, StateDetail};
use crate::util;

/// Folder names listed by [`TerminalAdapter::describe`] before "and N more".
const DESCRIBED_FOLDERS: usize = 5;

#[derive(Debug, Default)]
pub struct TerminalAdapter;

impl TerminalAdapter {
    pub fn new() -> Self {
        Self
    }
}

fn capability_for(info: &PlatformInfo) -> AdapterCapability {
    let details = &info.capabilities.process_details;
    let (folders_supported, folders_note) = if details.level == SupportLevel::Unsupported {
        (
            false,
            details.note.clone().unwrap_or_else(|| {
                "This system does not let Reframe read the folders of other programs.".to_string()
            }),
        )
    } else {
        let os_note = match info.os {
            OsKind::Windows => {
                "Read from the shell in each window; not available for terminals running as \
                 administrator."
            }
            _ => {
                "Read from the shell in each window; shells running as another user (sudo -i, su) \
                 reopen in the folder they were started from."
            }
        };
        let note = match (details.level, &details.note) {
            (SupportLevel::Partial, Some(platform_note)) => format!("{os_note} {platform_note}"),
            _ => os_note.to_string(),
        };
        (true, note)
    };
    let (tabs_supported, tabs_note) = match info.os {
        OsKind::Windows => (
            true,
            "Reopened as tabs in Windows Terminal; tabs cannot always be matched to the right \
             window.",
        ),
        OsKind::Macos => (false, "Each tab reopens in its own window."),
        _ => (
            true,
            "Reopened as tabs in GNOME Terminal, Xfce Terminal, MATE Terminal, Konsole, \
             Terminator and Ptyxis; other terminals open one window per tab. Tabs of terminals \
             that show several windows from one process cannot always be matched to the right \
             window.",
        ),
    };
    let profiles_note = match info.os {
        OsKind::Windows => {
            "Windows Terminal profiles you choose; console windows remember their shell."
        }
        OsKind::Macos => "Terminal and iTerm2 profiles you choose (applied with AppleScript).",
        _ => {
            "GNOME Terminal, MATE Terminal, Konsole, Terminator and Tilix profiles you choose; \
             not detected automatically."
        }
    };
    AdapterCapability {
        level: CapabilityLevel::Partial,
        features: Vec::new(),
    }
    .feature("Reopen terminal windows", true, None)
    .feature(
        "Working directories",
        folders_supported,
        Some(&folders_note),
    )
    .feature("Tabs", tabs_supported, Some(tabs_note))
    .feature("Profiles", true, Some(profiles_note))
    .feature(
        "Startup commands",
        true,
        Some("Only commands you add and approve"),
    )
    .feature(
        "Running programs, history and scrollback",
        false,
        Some("Never replayed for safety"),
    )
}

fn describe_state(state: &AppState) -> Vec<StateDetail> {
    let AppState::Terminal { sessions } = state else {
        return Vec::new();
    };
    if sessions.is_empty() {
        return Vec::new();
    }
    let mut details = vec![StateDetail::new("Sessions", sessions.len().to_string())];
    let mut folders: Vec<String> = Vec::new();
    for dir in sessions
        .iter()
        .filter_map(|s| s.working_directory.as_deref())
    {
        let label = util::path_label(dir);
        if !folders.contains(&label) {
            folders.push(label);
        }
    }
    if !folders.is_empty() {
        let value = if folders.len() > DESCRIBED_FOLDERS {
            format!(
                "{}, and {} more",
                folders[..DESCRIBED_FOLDERS].join(", "),
                folders.len() - DESCRIBED_FOLDERS
            )
        } else {
            folders.join(", ")
        };
        let label = if folders.len() == 1 {
            "Folder"
        } else {
            "Folders"
        };
        details.push(StateDetail::new(label, value));
    }
    let commands: Vec<bool> = sessions
        .iter()
        .filter(|s| {
            s.startup_command
                .as_deref()
                .is_some_and(|c| !c.trim().is_empty())
        })
        .map(|s| s.startup_command_approved)
        .collect();
    if !commands.is_empty() {
        let approved = commands.iter().filter(|a| **a).count();
        let waiting = commands.len() - approved;
        let value = match (approved, waiting) {
            (a, 0) => format!("{a} approved"),
            (0, w) => format!("{w} not approved"),
            (a, w) => format!("{a} approved, {w} not approved"),
        };
        details.push(StateDetail::new("Startup commands", value));
    }
    details
}

/// Whether `text` looks like it contains a credential (`export API_TOKEN=...`,
/// `mysql --password=...`, `https://user:pass@host`). Errs on the side of caution: such text is
/// left out of exports.
fn looks_like_credential(text: &str) -> bool {
    const MARKERS: &[&str] = &[
        "password",
        "passwd",
        "secret",
        "token",
        "apikey",
        "api_key",
        "api-key",
        "access_key",
        "access-key",
        "private_key",
        "private-key",
        "credential",
        "bearer",
        "authorization",
    ];
    let lower = text.to_lowercase();
    MARKERS.iter().any(|marker| lower.contains(marker)) || has_url_password(&lower)
}

/// A URL with a password in it: `scheme://user:password@host`.
fn has_url_password(text: &str) -> bool {
    text.match_indices("://").any(|(i, _)| {
        let authority = text[i + 3..]
            .split(|c: char| c == '/' || c.is_whitespace())
            .next()
            .unwrap_or_default();
        authority
            .rsplit_once('@')
            .is_some_and(|(user_info, _)| user_info.contains(':'))
    })
}

impl AppAdapter for TerminalAdapter {
    fn id(&self) -> &'static str {
        "terminal"
    }

    fn name(&self) -> &'static str {
        "Terminals"
    }

    fn identify(&self, process: &ProcessInfo, windows: &[&WindowInfo]) -> Option<AppIdentity> {
        identify_kind(OsKind::current(), process, windows).map(|kind| AppIdentity {
            app_id: kind.app_id().to_string(),
            display_name: kind.display_name().to_string(),
        })
    }

    fn capability(&self, info: &PlatformInfo) -> AdapterCapability {
        capability_for(info)
    }

    fn capture(&self, ctx: &CaptureContext<'_>, input: &CaptureInput<'_>) -> CaptureOutput {
        capture::capture(ctx, input)
    }

    fn launch_plan(
        &self,
        ctx: &LaunchContext<'_>,
        entry: &ApplicationEntry,
    ) -> Result<Vec<LaunchStep>, String> {
        plan::launch_plan(ctx, entry)
    }

    /// Always [`InstanceModel::SingleInstance`]: this method has no entry context, and many
    /// terminals forward a launch to an already running process that owns the new window
    /// (GNOME Terminal's `gnome-terminal-server`, Windows Terminal, Konsole, Xfce Terminal,
    /// Tilix, Terminator, GNOME Console, Ptyxis; `wt.exe` and `open` exit right away). Treating
    /// the one-process-per-window terminals (xterm, Alacritty, foot...) the same way is safe:
    /// their windows are then matched by application rather than by the launched pid.
    fn instance_model(&self) -> InstanceModel {
        InstanceModel::SingleInstance
    }

    fn describe(&self, state: &AppState) -> Vec<StateDetail> {
        describe_state(state)
    }

    fn locate_query(&self, app_id: &str) -> LocateQuery {
        match TerminalKind::from_app_id(app_id) {
            Some(kind) => locate_query_for(kind, OsKind::current()),
            None => GenericAdapter::new().locate_query(app_id),
        }
    }

    /// The shared sanitizer, then: every startup command loses its approval (an exported file
    /// never carries permission to run a command; imports reset it as well), and startup commands
    /// and window titles that look like they contain a credential are left out (a title marks
    /// the first tab of a window, so it is emptied rather than removed).
    fn sanitize_for_export(&self, state: &AppState) -> AppState {
        let mut clean = crate::portability::sanitize::sanitize_state(state);
        if let AppState::Terminal { sessions } = &mut clean {
            for session in sessions {
                session.startup_command_approved = false;
                if session
                    .startup_command
                    .as_deref()
                    .is_some_and(looks_like_credential)
                {
                    session.startup_command = None;
                }
                if let Some(title) = &mut session.window_title
                    && looks_like_credential(title)
                {
                    title.clear();
                }
            }
        }
        clean
    }
}
