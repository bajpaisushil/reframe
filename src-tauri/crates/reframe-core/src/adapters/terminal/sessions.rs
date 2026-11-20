//! Finding the shell sessions running inside a terminal.
//!
//! A terminal starts one shell per tab (directly, or through helpers such as `login`,
//! `ptyxis-agent` or `OpenConsole.exe`). The user may have started further shells inside it
//! (`bash` in `bash`, `sudo -s`, `nix-shell`...). For every tab Reframe keeps the *innermost*
//! interactive shell, the one the user is typing into, and reads its working directory.
//!
//! Only the chain of shells and session wrappers counts: a shell started by an editor, a build
//! tool or a GUI application launched from the terminal (VS Code's integrated terminal, `make`'s
//! `sh -c` children) is not a tab of this terminal.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use reframe_platform::ProcessInfo;
use reframe_platform::common::process::descendants;

use super::shell::{self, normalize_name};

/// Upper bound for parent-chain walks (protects against cycles in inconsistent process data).
const MAX_CHAIN: usize = 64;

/// Processes allowed between a terminal and its shells, or between nested shells: privilege and
/// session wrappers, and terminal helper processes.
fn is_wrapper(normalized: &str) -> bool {
    matches!(
        normalized,
        "sudo"
            | "su"
            | "doas"
            | "login"
            | "runuser"
            | "script"
            | "sg"
            | "newgrp"
            | "pkexec"
            | "nix-shell"
            | "ptyxis-agent"
            | "openconsole"
            | "conhost"
    ) || normalized.starts_with("itermserver")
}

fn is_pass_through(process: &ProcessInfo) -> bool {
    shell::process_shell(process).is_some() || is_wrapper(&normalize_name(&process.name))
}

/// Process lookup by pid.
pub struct ProcessIndex<'a> {
    by_pid: HashMap<u32, &'a ProcessInfo>,
}

impl<'a> ProcessIndex<'a> {
    /// Index `all`, plus `extra` processes missing from it. If a pid appears more than once
    /// (pid reuse while the list was read), the first entry wins.
    pub fn new(all: &'a [ProcessInfo], extra: &[&'a ProcessInfo]) -> Self {
        let mut by_pid: HashMap<u32, &'a ProcessInfo> = HashMap::with_capacity(all.len());
        for p in all.iter().chain(extra.iter().copied()) {
            by_pid.entry(p.pid).or_insert(p);
        }
        Self { by_pid }
    }

    pub fn get(&self, pid: u32) -> Option<&'a ProcessInfo> {
        self.by_pid.get(&pid).copied()
    }
}

/// One tab's shell session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundSession {
    /// The innermost interactive shell of the tab.
    pub shell_pid: u32,
    /// Its normalised name (`bash`, `pwsh`, `cmd`...).
    pub shell: String,
    pub working_directory: Option<PathBuf>,
    /// When the tab was opened: start time and pid of the tab's top-level shell.
    pub order: (u64, u32),
}

/// Sessions found below one terminal process.
#[derive(Debug, Default)]
pub struct RootScan {
    pub sessions: Vec<FoundSession>,
    /// Processes visible below the root (0 when it has no children or they cannot be seen).
    pub descendant_count: usize,
}

struct ShellNode<'a> {
    process: &'a ProcessInfo,
    name: String,
    interactive: bool,
    /// Ancestors between the shell and the root, closest first (the root itself is the last
    /// element when it is part of the session, i.e. a console shell).
    chain: Vec<&'a ProcessInfo>,
}

impl ShellNode<'_> {
    fn order(&self) -> (u64, u32) {
        (self.process.start_time.unwrap_or(0), self.process.pid)
    }
}

/// Ancestors of `pid` up to `root`, provided every process in between is a shell or a wrapper.
fn chain_to_root<'a>(
    index: &ProcessIndex<'a>,
    pid: u32,
    root: u32,
    include_root: bool,
) -> Option<Vec<&'a ProcessInfo>> {
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut current = pid;
    while current != root {
        if !seen.insert(current) || chain.len() > MAX_CHAIN {
            return None;
        }
        let parent = index.get(current)?.parent_pid?;
        if parent == current {
            return None;
        }
        let parent_process = index.get(parent)?;
        if parent == root {
            if include_root {
                chain.push(parent_process);
            }
            return Some(chain);
        }
        if !is_pass_through(parent_process) {
            return None;
        }
        chain.push(parent_process);
        current = parent;
    }
    Some(chain)
}

/// The working directory of `process`, if it can be read and makes sense. Folders whose name is
/// not valid Unicode are skipped: workspaces are stored as JSON text, and launch arguments are
/// text too (a lossy conversion could name a different folder).
pub fn readable_directory(process: &ProcessInfo) -> Option<PathBuf> {
    let dir = process.working_directory.as_ref()?;
    let text = dir.to_str()?;
    if text.is_empty() || !dir.is_absolute() {
        return None;
    }
    // Linux reports a deleted working directory as "<path> (deleted)".
    if let Some(stripped) = text.strip_suffix(" (deleted)")
        && !dir.exists()
    {
        return Some(PathBuf::from(stripped));
    }
    Some(dir.clone())
}

/// Find the tab sessions below `root`. With `include_root` the root itself is the tab's shell
/// (a classic console window owned by `cmd.exe`); otherwise the root is the terminal process.
pub fn scan_root(
    all: &[ProcessInfo],
    index: &ProcessIndex<'_>,
    root: u32,
    include_root: bool,
) -> RootScan {
    let below = descendants(all, root);
    let mut candidates: Vec<u32> = Vec::with_capacity(below.len() + 1);
    if include_root {
        candidates.push(root);
    }
    candidates.extend(below.iter().copied());

    let mut shells: Vec<ShellNode<'_>> = Vec::new();
    for pid in candidates {
        let Some(process) = index.get(pid) else {
            continue;
        };
        let Some((family, name)) = shell::process_shell(process) else {
            continue;
        };
        let Some(chain) = chain_to_root(index, pid, root, include_root) else {
            continue;
        };
        shells.push(ShellNode {
            process,
            name,
            interactive: shell::is_interactive(process, family),
            chain,
        });
    }

    let by_pid: HashMap<u32, usize> = shells
        .iter()
        .enumerate()
        .map(|(i, s)| (s.process.pid, i))
        .collect();
    let is_shell = |p: &ProcessInfo| by_pid.contains_key(&p.pid);

    // Group shells by their tab: the outermost shell on their chain.
    let mut tabs: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (i, node) in shells.iter().enumerate() {
        let top = node
            .chain
            .iter()
            .rev()
            .find(|p| is_shell(p))
            .map_or(node.process.pid, |p| p.pid);
        tabs.entry(top).or_default().push(i);
    }

    let mut sessions = Vec::new();
    for (top_pid, members) in tabs {
        let Some(&top_index) = by_pid.get(&top_pid) else {
            continue;
        };
        let top = &shells[top_index];
        let depth = |node: &ShellNode<'_>| node.chain.iter().filter(|p| is_shell(p)).count();
        // The innermost interactive shell; a tab without one (a terminal running `sh -c ...`,
        // e.g. a startup command still running) is represented by its top-level shell.
        let chosen = members
            .iter()
            .map(|&i| &shells[i])
            .filter(|node| node.interactive)
            .max_by_key(|node| (depth(node), node.order()))
            .unwrap_or(top);
        // Unreadable (e.g. a root shell from `sudo -s`): use the closest readable folder between
        // it and the tab's top-level shell, never above (helpers there have unrelated folders).
        let working_directory = if chosen.process.pid == top_pid {
            readable_directory(chosen.process)
        } else {
            std::iter::once(chosen.process)
                .chain(
                    chosen
                        .chain
                        .iter()
                        .copied()
                        .take_while(|p| p.pid != top_pid),
                )
                .chain(std::iter::once(top.process))
                .find_map(readable_directory)
        };
        sessions.push(FoundSession {
            shell_pid: chosen.process.pid,
            shell: chosen.name.clone(),
            working_directory,
            order: top.order(),
        });
    }
    sessions.sort_by_key(|s| s.order);
    RootScan {
        sessions,
        descendant_count: below.len(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Synthetic process started at time `pid` (so pids also give the creation order).
    pub(crate) fn p(
        pid: u32,
        parent: u32,
        name: &str,
        cmd: &[&str],
        cwd: Option<&str>,
    ) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid: (parent != 0).then_some(parent),
            name: name.into(),
            product_name: None,
            executable_path: None,
            command_line: cmd.iter().map(|s| s.to_string()).collect(),
            working_directory: cwd.map(abs),
            start_time: Some(u64::from(pid)),
            package: None,
        }
    }

    /// An absolute path on every OS (`/x` on Unix, `C:\x` on Windows).
    pub(crate) fn abs(path: &str) -> PathBuf {
        let root = if cfg!(windows) { "C:\\" } else { "/" };
        PathBuf::from(root).join(path.trim_start_matches('/'))
    }

    fn scan(all: &[ProcessInfo], root: u32, include_root: bool) -> RootScan {
        let index = ProcessIndex::new(all, &[]);
        scan_root(all, &index, root, include_root)
    }

    fn dirs(scan: &RootScan) -> Vec<Option<PathBuf>> {
        scan.sessions
            .iter()
            .map(|s| s.working_directory.clone())
            .collect()
    }

    #[test]
    fn one_session_per_tab_in_creation_order() {
        let all = vec![
            p(100, 1, "gnome-terminal-", &[], Some("/home/u")),
            p(120, 100, "bash", &["bash"], Some("/home/u/b")),
            p(110, 100, "zsh", &["zsh"], Some("/home/u/a")),
            p(130, 100, "fish", &["fish"], Some("/home/u/c")),
        ];
        let result = scan(&all, 100, false);
        assert_eq!(
            dirs(&result),
            vec![
                Some(abs("home/u/a")),
                Some(abs("home/u/b")),
                Some(abs("home/u/c"))
            ]
        );
        assert_eq!(result.sessions[0].shell, "zsh");
        assert_eq!(result.descendant_count, 3);
    }

    #[test]
    fn keeps_the_innermost_interactive_shell() {
        let all = vec![
            p(100, 1, "kitty", &[], None),
            p(101, 100, "bash", &["bash"], Some("/home/u")),
            p(102, 101, "zsh", &["zsh"], Some("/home/u/project")),
            p(103, 102, "fish", &["fish"], Some("/home/u/project/src")),
        ];
        let result = scan(&all, 100, false);
        assert_eq!(result.sessions.len(), 1);
        assert_eq!(result.sessions[0].shell_pid, 103);
        assert_eq!(
            result.sessions[0].working_directory,
            Some(abs("home/u/project/src"))
        );
        assert_eq!(result.sessions[0].order, (101, 101));
    }

    #[test]
    fn ignores_command_shells_and_shells_of_other_programs() {
        let all = vec![
            p(100, 1, "alacritty", &[], None),
            p(101, 100, "bash", &["bash"], Some("/work")),
            // vim's `:!ls` and make's recipe lines.
            p(102, 101, "vim", &["vim", "notes"], Some("/work")),
            p(103, 102, "sh", &["sh", "-c", "ls"], Some("/work")),
            p(104, 101, "make", &["make", "-j8"], Some("/work")),
            p(105, 104, "sh", &["sh", "-c", "cc a.c"], Some("/work/sub")),
            // A background `sh -c` job directly under the shell.
            p(106, 101, "sh", &["sh", "-c", "sleep 100"], Some("/tmp")),
            // VS Code started from the terminal, with its integrated terminal shell.
            p(107, 101, "code", &["code", "."], Some("/work")),
            p(108, 107, "bash", &["bash"], Some("/elsewhere")),
        ];
        let result = scan(&all, 100, false);
        assert_eq!(result.sessions.len(), 1);
        assert_eq!(result.sessions[0].shell_pid, 101);
        assert_eq!(result.sessions[0].working_directory, Some(abs("work")));
    }

    #[test]
    fn falls_back_to_the_enclosing_shell_folder_for_root_shells() {
        // `sudo -s`: the root shell's working directory cannot be read.
        let all = vec![
            p(100, 1, "konsole", &[], None),
            p(101, 100, "bash", &["bash"], Some("/home/u/srv")),
            p(102, 101, "sudo", &["sudo", "-s"], None),
            p(103, 102, "bash", &["bash"], None),
        ];
        let result = scan(&all, 100, false);
        assert_eq!(result.sessions.len(), 1);
        assert_eq!(result.sessions[0].shell_pid, 103);
        assert_eq!(
            result.sessions[0].working_directory,
            Some(abs("home/u/srv"))
        );
    }

    #[test]
    fn passes_through_terminal_helpers() {
        let all = vec![
            // macOS Terminal: login → -zsh.
            p(10, 1, "Terminal", &[], None),
            p(11, 10, "login", &["login", "-pf", "u"], None),
            p(12, 11, "zsh", &["-zsh"], Some("/Users/u/site")),
            // Ptyxis: ptyxis-agent → bash.
            p(20, 1, "ptyxis", &[], None),
            p(21, 20, "ptyxis-agent", &[], None),
            p(22, 21, "bash", &["bash"], Some("/home/u/p")),
        ];
        assert_eq!(
            dirs(&scan(&all, 10, false)),
            vec![Some(abs("Users/u/site"))]
        );
        assert_eq!(dirs(&scan(&all, 20, false)), vec![Some(abs("home/u/p"))]);
    }

    #[test]
    fn tab_running_a_command_is_kept_with_its_folder() {
        // A tab whose shell is still running `sh -c <startup command>`.
        let all = vec![
            p(100, 1, "xterm", &[], None),
            p(101, 100, "sh", &["sh", "-c", "npm run dev"], Some("/app")),
            p(102, 101, "node", &["node", "server.js"], Some("/app")),
        ];
        let result = scan(&all, 100, false);
        assert_eq!(result.sessions.len(), 1);
        assert_eq!(result.sessions[0].working_directory, Some(abs("app")));
    }

    #[test]
    fn console_shell_root_is_its_own_session() {
        let all = vec![
            p(200, 1, "cmd.exe", &["cmd.exe"], Some("/Users/u")),
            p(
                201,
                200,
                "powershell.exe",
                &["powershell.exe"],
                Some("/Users/u/x"),
            ),
            p(
                202,
                200,
                "conhost.exe",
                &["conhost.exe", "0xffffffff"],
                None,
            ),
        ];
        let result = scan(&all, 200, true);
        assert_eq!(result.sessions.len(), 1);
        assert_eq!(result.sessions[0].shell, "powershell");
        assert_eq!(result.sessions[0].working_directory, Some(abs("Users/u/x")));
        // Without a nested shell the console shell itself is the session.
        let all = vec![p(300, 1, "pwsh.exe", &["pwsh.exe"], Some("/Users/u/y"))];
        let result = scan(&all, 300, true);
        assert_eq!(result.sessions.len(), 1);
        assert_eq!(result.sessions[0].shell, "pwsh");
    }

    #[test]
    fn unreadable_and_missing_processes() {
        // Shell whose details are hidden: still a session, without a folder.
        let all = vec![p(100, 1, "foot", &[], None), p(101, 100, "bash", &[], None)];
        let result = scan(&all, 100, false);
        assert_eq!(dirs(&result), vec![None]);
        // No children visible at all.
        let all = vec![p(100, 1, "foot", &[], None)];
        let result = scan(&all, 100, false);
        assert!(result.sessions.is_empty());
        assert_eq!(result.descendant_count, 0);
        // A tmux client instead of a shell (the tmux server runs elsewhere).
        let all = vec![
            p(100, 1, "foot", &[], None),
            p(
                101,
                100,
                "tmux: client",
                &["tmux", "attach"],
                Some("/home/u"),
            ),
        ];
        assert!(scan(&all, 100, false).sessions.is_empty());
    }

    #[test]
    fn survives_cycles_duplicates_and_self_parents() {
        // Inconsistent data (pid reuse while the list was read): pid 101 appears twice, and the
        // entry that wins the index forms a parent loop with 102.
        let all = vec![
            p(101, 102, "bash", &["bash"], Some("/a")),
            p(102, 101, "bash", &["bash"], Some("/b")),
            p(101, 100, "bash", &["bash"], Some("/c")),
            p(100, 1, "xterm", &[], None),
            p(7, 7, "bash", &["bash"], None),
        ];
        // Terminates; the looping shells are not attributed to the terminal.
        assert!(scan(&all, 100, false).sessions.is_empty());
        assert!(scan(&all, 7, false).sessions.is_empty());
    }

    #[test]
    fn deleted_working_directory_suffix_is_stripped() {
        let process = p(
            1,
            0,
            "bash",
            &["bash"],
            Some("/definitely/missing/reframe-dir (deleted)"),
        );
        assert_eq!(
            readable_directory(&process),
            Some(abs("definitely/missing/reframe-dir"))
        );
        let mut relative = p(2, 0, "bash", &["bash"], None);
        relative.working_directory = Some(PathBuf::from("relative/dir"));
        assert_eq!(readable_directory(&relative), None);
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_working_directories_are_skipped() {
        use std::os::unix::ffi::OsStrExt;
        let mut process = p(1, 0, "bash", &["bash"], None);
        process.working_directory = Some(PathBuf::from(std::ffi::OsStr::from_bytes(
            b"/home/u/caf\xe9",
        )));
        assert_eq!(readable_directory(&process), None);
    }
}
