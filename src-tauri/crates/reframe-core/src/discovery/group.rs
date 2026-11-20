//! Grouping visible windows into logical applications.
//!
//! Every window gets a *grouping key* from its owning process:
//!
//! 1. `pkg:<package id>` (lower-cased) when the process belongs to a package (macOS bundle,
//!    Windows packaged app, Flatpak, Snap, AppImage);
//! 2. otherwise `exe:<executable path>` (case- and separator-insensitive on Windows; Linux's
//!    " (deleted)" marker for replaced binaries is ignored);
//! 3. otherwise `proc:<process name>` (case-insensitive on Windows);
//! 4. windows whose owner is unknown (pid 0, or a pid missing from the process list) are keyed by
//!    window class: `class:<class>` (then `pid:<pid>` / `window:<id>` when there is no class).
//!
//! All windows with the same key form one application, which covers multi-process applications
//! (browsers) and several instances of one program. Keys are opaque to the UI: they are only
//! handed back in capture selections, and stay stable while the application keeps running.
//!
//! Refinement: script interpreters and program hosts (Python, GJS, Perl, Ruby, Wine...) run many
//! unrelated applications from one executable. Their processes are keyed
//! `script:<process name>|<executable path>` when the process name differs from the interpreter's
//! (on Linux the process name of a script started through its `#!` line is the script's name).
//! When the name is the interpreter's own (e.g. Windows `python.exe`) such applications still
//! share one group — a documented limitation.

use std::collections::{HashMap, HashSet};

use reframe_platform::common::process::descendants;
use reframe_platform::{OsKind, ProcessInfo, WindowInfo};

use super::DiscoveryOptions;
use super::names;
use crate::adapters::AppIdentity;
use crate::model::DesktopState;
use crate::util;

/// Who owns a group's windows.
#[derive(Debug)]
enum Owner<'a> {
    /// Known processes; `main` owns the frontmost window.
    Known {
        main: &'a ProcessInfo,
        others: Vec<&'a ProcessInfo>,
    },
    /// The owning process is unknown; adapters are given this stand-in built from the window.
    Unknown(ProcessInfo),
}

/// The windows of one logical application.
#[derive(Debug)]
pub(crate) struct AppGroup<'a> {
    /// Grouping key (see module docs), stable while the application runs.
    pub key: String,
    /// Front-to-back.
    pub windows: Vec<&'a WindowInfo>,
    /// Distinct non-zero owner pids (even when the process itself is not listed), owner of the
    /// frontmost window first.
    pub pids: Vec<u32>,
    owner: Owner<'a>,
}

impl<'a> AppGroup<'a> {
    /// The process adapters treat as the main process: the owner of the frontmost window, or a
    /// stand-in when the owner is unknown.
    pub fn main_process(&self) -> &ProcessInfo {
        match &self.owner {
            Owner::Known { main, .. } => main,
            Owner::Unknown(stand_in) => stand_in,
        }
    }

    /// The real main process (`None` when the owner is unknown).
    pub fn known_main_process(&self) -> Option<&'a ProcessInfo> {
        match &self.owner {
            Owner::Known { main, .. } => Some(*main),
            Owner::Unknown(_) => None,
        }
    }

    /// Owning processes, main first (the stand-in when the owner is unknown).
    pub fn processes(&self) -> Vec<&ProcessInfo> {
        match &self.owner {
            Owner::Known { main, others } => std::iter::once(*main)
                .chain(others.iter().copied())
                .collect(),
            Owner::Unknown(stand_in) => vec![stand_in],
        }
    }

    /// Processes owning `windows` (a subset of this group's windows, front-to-back), owner of the
    /// first window first. Falls back to the group's main process so adapters always get one.
    pub fn processes_for(&self, windows: &[&WindowInfo]) -> Vec<&ProcessInfo> {
        let mut out: Vec<&ProcessInfo> = Vec::new();
        if let Owner::Known { main, others } = &self.owner {
            for window in windows {
                let owner = std::iter::once(*main)
                    .chain(others.iter().copied())
                    .find(|p| p.pid == window.pid);
                if let Some(process) = owner
                    && !out.iter().any(|p| p.pid == process.pid)
                {
                    out.push(process);
                }
            }
        }
        if out.is_empty() {
            out.push(self.main_process());
        }
        out
    }

    /// The identity to present for this application. The generic adapter names a
    /// script-hosted application after its interpreter ("Python3"); the script's name is shown
    /// instead, unless the platform reported a product name for the process. A blank display
    /// name is replaced by one derived from the grouping key.
    pub fn presented_identity(&self, mut identity: AppIdentity, generic: bool) -> AppIdentity {
        if identity.display_name.trim().is_empty() {
            identity.display_name = key_label(&self.key);
        }
        let has_product_name = self
            .main_process()
            .product_name
            .as_deref()
            .is_some_and(|name| !name.trim().is_empty());
        if generic
            && !has_product_name
            && let Some((name, _)) = self
                .key
                .strip_prefix("script:")
                .and_then(|rest| rest.split_once('|'))
        {
            let name = util::prettify_name(name);
            if !name.is_empty() {
                identity.display_name = name;
            }
        }
        identity
    }
}

/// Pids whose windows are never listed: Reframe itself and every process it started (helper
/// processes such as the web view). `self_pid == 0` means "unknown" and hides nothing; pid 0
/// (windows without a known owner) is never treated as Reframe's.
pub(crate) fn own_pids(processes: &[ProcessInfo], self_pid: u32) -> HashSet<u32> {
    if self_pid == 0 {
        return HashSet::new();
    }
    std::iter::once(self_pid)
        .chain(descendants(processes, self_pid))
        .filter(|&pid| pid != 0)
        .collect()
}

/// Group the visible windows of `state` into applications, ordered by their frontmost window,
/// with windows front-to-back inside each application. Reframe's own windows are left out.
pub(crate) fn group_windows<'a>(
    state: &'a DesktopState,
    options: &DiscoveryOptions,
) -> Vec<AppGroup<'a>> {
    let os = state.info.os;
    let mut by_pid: HashMap<u32, &ProcessInfo> = HashMap::new();
    for process in &state.processes {
        by_pid.entry(process.pid).or_insert(process);
    }
    let hidden = own_pids(&state.processes, options.self_pid);

    let mut ordered: Vec<(usize, &WindowInfo)> = state
        .windows
        .iter()
        .enumerate()
        .filter(|(_, w)| !hidden.contains(&w.pid))
        .collect();
    // z_order first; the platform's front-to-back order breaks ties.
    ordered.sort_by_key(|(index, w)| (w.z_order, *index));

    struct Builder<'a> {
        key: String,
        windows: Vec<&'a WindowInfo>,
        pids: Vec<u32>,
        known: Vec<&'a ProcessInfo>,
    }
    let mut builders: Vec<Builder<'a>> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for (_, window) in ordered {
        let process = if window.pid == 0 {
            None
        } else {
            by_pid.get(&window.pid).copied()
        };
        let key = match process {
            Some(process) => process_key(process, os),
            None => orphan_window_key(window, os),
        };
        let slot = *index.entry(key).or_insert_with_key(|key| {
            builders.push(Builder {
                key: key.clone(),
                windows: Vec::new(),
                pids: Vec::new(),
                known: Vec::new(),
            });
            builders.len() - 1
        });
        let builder = &mut builders[slot];
        builder.windows.push(window);
        if window.pid != 0 && !builder.pids.contains(&window.pid) {
            builder.pids.push(window.pid);
        }
        if let Some(process) = process
            && !builder.known.iter().any(|p| p.pid == process.pid)
        {
            builder.known.push(process);
        }
    }

    builders
        .into_iter()
        .filter_map(|b| {
            let owner = match b.known.split_first() {
                Some((main, others)) => Owner::Known {
                    main,
                    others: others.to_vec(),
                },
                None => Owner::Unknown(stand_in_process(b.windows.first()?)),
            };
            Some(AppGroup {
                key: b.key,
                windows: b.windows,
                pids: b.pids,
                owner,
            })
        })
        .collect()
}

/// Grouping key of a window owned by a known process (see module docs).
pub(crate) fn process_key(process: &ProcessInfo, os: OsKind) -> String {
    if let Some(package) = &process.package {
        let id = package.id.trim();
        if !id.is_empty() {
            return format!("pkg:{}", id.to_lowercase());
        }
    }
    if let Some(exe) = process.executable_path.as_deref() {
        let path = names::exe_path_key(exe, os);
        if !path.is_empty() {
            let stem = names::exe_stem(exe, os);
            if names::is_script_host(&stem) {
                let name = names::process_name(&process.name);
                if !name.is_empty() && name != stem && !names::is_script_host(&name) {
                    return format!("script:{name}|{path}");
                }
            }
            return format!("exe:{path}");
        }
    }
    let name = names::fold_case(process.name.trim(), os);
    if name.is_empty() {
        format!("pid:{}", process.pid)
    } else {
        format!("proc:{name}")
    }
}

/// Grouping key of a window whose owner process is unknown.
fn orphan_window_key(window: &WindowInfo, os: OsKind) -> String {
    if let Some(class) = non_empty(window.class_name.as_deref()) {
        return format!("class:{}", names::fold_case(class, os));
    }
    if window.pid != 0 {
        format!("pid:{}", window.pid)
    } else {
        format!("window:{}", window.id)
    }
}

/// Stand-in process for windows without a known owner, so adapters can still identify the
/// application by its window class.
fn stand_in_process(window: &WindowInfo) -> ProcessInfo {
    let name = non_empty(window.class_name.as_deref())
        .or_else(|| non_empty(window.app_id_hint.as_deref()))
        .unwrap_or("unknown")
        .to_string();
    ProcessInfo {
        pid: window.pid,
        parent_pid: None,
        name,
        product_name: None,
        executable_path: None,
        command_line: Vec::new(),
        working_directory: None,
        start_time: None,
        package: None,
    }
}

fn non_empty(text: Option<&str>) -> Option<&str> {
    text.map(str::trim).filter(|t| !t.is_empty())
}

/// Human-readable application name recovered from a grouping key, for messages about
/// applications that are gone ("GIMP is no longer running").
pub(crate) fn key_label(key: &str) -> String {
    let fallback = || "An application".to_string();
    let Some((kind, value)) = key.split_once(':') else {
        return fallback();
    };
    let label = match kind {
        "exe" => {
            let file = value.rsplit(['/', '\\']).next().unwrap_or(value);
            util::prettify_name(names::strip_extension(file))
        }
        "script" => util::prettify_name(value.split_once('|').map_or(value, |(name, _)| name)),
        "proc" => util::prettify_name(value),
        "pkg" | "class" => value.trim().to_string(),
        _ => String::new(),
    };
    if label.trim().is_empty() {
        fallback()
    } else {
        label
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reframe_platform::{
        DisplayServer, PackageInfo, PackageKind, PlatformCapabilities, PlatformInfo, Rect,
        WindowId, WindowState,
    };

    fn state(processes: Vec<ProcessInfo>, windows: Vec<WindowInfo>) -> DesktopState {
        DesktopState {
            info: PlatformInfo {
                os: OsKind::Linux,
                os_version: "test".into(),
                display_server: DisplayServer::X11,
                desktop_environment: None,
                capabilities: PlatformCapabilities::all_unsupported("test"),
            },
            monitors: Vec::new(),
            windows,
            processes,
            warnings: Vec::new(),
        }
    }

    fn window(id: &str, pid: u32, z_order: u32) -> WindowInfo {
        WindowInfo {
            id: WindowId::new(id),
            pid,
            title: id.into(),
            class_name: None,
            app_id_hint: None,
            bounds: Rect::new(0, 0, 800, 600),
            normal_bounds: None,
            state: WindowState::Normal,
            monitor_id: None,
            desktop: None,
            is_focused: false,
            z_order,
        }
    }

    #[test]
    fn presented_identity_names_scripts_and_fills_blank_names() {
        let desktop = state(
            vec![process(7, "meld", Some("/usr/bin/python3.12"))],
            vec![window("w", 7, 0)],
        );
        let groups = group_windows(&desktop, &DiscoveryOptions::default());
        assert_eq!(groups.len(), 1);
        let group = &groups[0];
        let identity = |name: &str| AppIdentity {
            app_id: "exe:python3".into(),
            display_name: name.into(),
        };
        assert_eq!(
            group.presented_identity(identity("Python3"), true),
            identity("Meld")
        );
        // Dedicated adapters keep their names.
        assert_eq!(
            group.presented_identity(identity("Python3"), false),
            identity("Python3")
        );
        assert_eq!(
            group.presented_identity(identity("  "), false),
            identity("Meld")
        );
    }

    #[test]
    fn processes_for_a_window_subset() {
        let desktop = state(
            vec![
                process(1, "app", Some("/usr/bin/app")),
                process(2, "app", Some("/usr/bin/app")),
            ],
            vec![window("a", 1, 0), window("b", 2, 1), window("c", 1, 2)],
        );
        let groups = group_windows(&desktop, &DiscoveryOptions::default());
        let group = &groups[0];
        let pids = |windows: &[&WindowInfo]| -> Vec<u32> {
            group.processes_for(windows).iter().map(|p| p.pid).collect()
        };
        assert_eq!(pids(&[group.windows[1]]), vec![2]);
        assert_eq!(pids(&group.windows), vec![1, 2]);
        // No windows left: the main process still represents the application.
        assert_eq!(pids(&[]), vec![1]);
        assert_eq!(
            group.processes().iter().map(|p| p.pid).collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    fn process(pid: u32, name: &str, exe: Option<&str>) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid: None,
            name: name.into(),
            product_name: None,
            executable_path: exe.map(Into::into),
            command_line: Vec::new(),
            working_directory: None,
            start_time: None,
            package: None,
        }
    }

    #[test]
    fn keys_prefer_package_then_executable_then_name() {
        let mut p = process(1, "gimp", Some("/usr/bin/gimp"));
        assert_eq!(process_key(&p, OsKind::Linux), "exe:/usr/bin/gimp");
        p.package = Some(PackageInfo {
            kind: PackageKind::Flatpak,
            id: "org.gimp.GIMP".into(),
            path: None,
        });
        assert_eq!(process_key(&p, OsKind::Linux), "pkg:org.gimp.gimp");
        let p = process(2, "Notes", None);
        assert_eq!(process_key(&p, OsKind::Linux), "proc:Notes");
        assert_eq!(process_key(&p, OsKind::Windows), "proc:notes");
        let p = process(3, " ", None);
        assert_eq!(process_key(&p, OsKind::Linux), "pid:3");
    }

    #[test]
    fn script_hosted_processes_are_told_apart_by_name() {
        let meld = process(1, "meld", Some("/usr/bin/python3.12"));
        let terminator = process(2, "terminator", Some("/usr/bin/python3.12"));
        let plain = process(3, "python3", Some("/usr/bin/python3.12"));
        assert_eq!(
            process_key(&meld, OsKind::Linux),
            "script:meld|/usr/bin/python3.12"
        );
        assert_ne!(
            process_key(&meld, OsKind::Linux),
            process_key(&terminator, OsKind::Linux)
        );
        assert_eq!(
            process_key(&plain, OsKind::Linux),
            "exe:/usr/bin/python3.12"
        );
    }

    #[test]
    fn labels_from_keys() {
        assert_eq!(
            key_label("exe:/usr/bin/gnome-calculator"),
            "Gnome Calculator"
        );
        assert_eq!(key_label(r"exe:c:\program files\app\app.exe"), "App");
        assert_eq!(key_label("script:meld|/usr/bin/python3"), "Meld");
        assert_eq!(key_label("proc:xterm"), "Xterm");
        assert_eq!(key_label("pkg:org.gimp.gimp"), "org.gimp.gimp");
        assert_eq!(key_label("class:XClock"), "XClock");
        assert_eq!(key_label("window:0x1234"), "An application");
        assert_eq!(key_label("garbage"), "An application");
        assert_eq!(key_label("exe:"), "An application");
    }

    #[test]
    fn own_pids_include_descendants_and_ignore_zero() {
        let mut child = process(11, "WebKitWebProcess", None);
        child.parent_pid = Some(10);
        let mut grandchild = process(12, "helper", None);
        grandchild.parent_pid = Some(11);
        let mut orphan = process(13, "init-child", None);
        orphan.parent_pid = Some(0);
        let processes = vec![process(10, "reframe", None), child, grandchild, orphan];
        let own = own_pids(&processes, 10);
        assert_eq!(own, HashSet::from([10, 11, 12]));
        assert!(own_pids(&processes, 0).is_empty());
    }
}
