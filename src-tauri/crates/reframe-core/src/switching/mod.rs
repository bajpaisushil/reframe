//! Workspace switching (Replace / Merge / Additive) planning.

mod snapshot;

use std::collections::HashSet;

use reframe_platform::{Capability, PlatformInfo, SupportLevel, WindowId};

use crate::adapters::{AdapterRegistry, LaunchContext};
use crate::model::{
    ApplicationEntry, CloseCandidate, DesktopState, PlannedLaunch, PlannedReuse, SwitchMode,
    SwitchPlan, Workspace,
};
use crate::restoration::{matcher, validate};

use self::snapshot::SnapshotPlatform;

/// Compute what switching to `target` would do. Pure: nothing is launched or closed.
///
/// - Additive: every enabled entry is launched.
/// - Merge: entries satisfied by running windows ([`matcher::running_windows_for_entry`]) are
///   reused (their windows will be arranged); the rest launch.
/// - Replace: like Merge, plus close candidates = titled running windows of applications that
///   are not in `target` (enabled or not). `default_selected` is true only for windows of
///   applications that belong to `previous`. Reframe's own windows (this process) and
///   untitled windows are never candidates; desktop/shell windows are already filtered out by
///   the platform's window list.
///
/// Warnings cover entries that would be launched but cannot be (missing program or working
/// folder, adapter launch-plan errors) and platform limitations: windows that cannot be
/// listed (nothing can be reused, closed or arranged), closed or moved. Adapters plan their
/// launches against a read-only view of `state` that locates applications through `PATH`
/// only, so an adapter relying on platform-specific lookups may report a problem here that a
/// real restore would not hit.
pub fn plan_switch(
    state: &DesktopState,
    adapters: &AdapterRegistry,
    info: &PlatformInfo,
    previous: Option<&Workspace>,
    target: &Workspace,
    mode: SwitchMode,
) -> SwitchPlan {
    let caps = &info.capabilities;
    let view = SnapshotPlatform::new(state, info);
    let ctx = LaunchContext {
        platform: &view,
        info,
    };
    let can_list = caps.list_windows.is_available();
    let reuse_allowed = mode != SwitchMode::Additive && can_list;
    let mut warnings = capability_warnings(info, target, mode);

    let mut launch = Vec::new();
    let mut reuse = Vec::new();
    for entry in target.applications.iter().filter(|e| e.enabled) {
        let running = if reuse_allowed {
            matcher::running_windows_for_entry(entry, state, adapters)
        } else {
            Vec::new()
        };
        if !running.is_empty() {
            reuse.push(PlannedReuse {
                entry_id: entry.id,
                display_name: entry.display_name.clone(),
                app_id: entry.app_id.clone(),
                window_ids: running,
            });
            continue;
        }
        let reason = match mode {
            SwitchMode::Additive => "Additive mode - starts even if it is already running",
            _ if !can_list => "Running applications can't be detected on this system",
            _ => "Not running",
        };
        launch.push(PlannedLaunch {
            entry_id: entry.id,
            display_name: entry.display_name.clone(),
            app_id: entry.app_id.clone(),
            reason: reason.to_string(),
        });
        if let Some(problem) = launch_problem(entry, adapters, &ctx) {
            warnings.push(problem);
        }
    }

    let close = if mode == SwitchMode::Replace && can_list && caps.close_windows.is_available() {
        close_candidates(state, adapters, previous, target)
    } else {
        Vec::new()
    };

    SwitchPlan {
        workspace_id: target.id,
        mode,
        previous_workspace_id: previous.map(|p| p.id),
        launch,
        reuse,
        close,
        warnings,
    }
}

/// Why `entry` could not be launched, if it could not.
fn launch_problem(
    entry: &ApplicationEntry,
    adapters: &AdapterRegistry,
    ctx: &LaunchContext<'_>,
) -> Option<String> {
    let name = &entry.display_name;
    match adapters.get(&entry.adapter_id).launch_plan(ctx, entry) {
        Err(message) if message.contains(name.as_str()) => Some(message),
        Err(message) => Some(format!("{name}: {message}")),
        Ok(steps) if steps.is_empty() => Some(format!(
            "Reframe doesn't know how to start {name} - edit the application to choose its location"
        )),
        Ok(steps) => steps
            .iter()
            .find_map(|step| validate::check_request(&step.request, name).err()),
    }
}

/// Running windows of applications that are not part of `target`.
fn close_candidates(
    state: &DesktopState,
    adapters: &AdapterRegistry,
    previous: Option<&Workspace>,
    target: &Workspace,
) -> Vec<CloseCandidate> {
    let windows_of = |ws: &Workspace| -> HashSet<WindowId> {
        ws.applications
            .iter()
            .flat_map(|e| matcher::running_windows_for_entry(e, state, adapters))
            .collect()
    };
    let in_target = windows_of(target);
    let in_previous = previous.map(windows_of).unwrap_or_default();
    let self_pid = std::process::id();
    state
        .windows
        .iter()
        .filter(|w| w.pid != self_pid && !w.title.trim().is_empty() && !in_target.contains(&w.id))
        .map(|w| {
            let (app_id, app_name) =
                matcher::identify_window(w, &state.windows, &state.processes, adapters);
            let default_selected = in_previous.contains(&w.id);
            let reason = match previous {
                Some(prev) if default_selected => {
                    format!("Part of {} but not of {}", prev.name, target.name)
                }
                _ => format!("Not part of {}", target.name),
            };
            CloseCandidate {
                window_id: w.id.clone(),
                pid: w.pid,
                title: w.title.clone(),
                app_id,
                app_name,
                default_selected,
                reason,
            }
        })
        .collect()
}

/// Platform limitations that affect this switch.
fn capability_warnings(info: &PlatformInfo, target: &Workspace, mode: SwitchMode) -> Vec<String> {
    let caps = &info.capabilities;
    let mut warnings = Vec::new();
    match caps.list_windows.level {
        SupportLevel::Unsupported => {
            let consequence = match mode {
                SwitchMode::Additive => "window positions won't be restored",
                SwitchMode::Merge => {
                    "running applications won't be reused and window positions won't be restored"
                }
                SwitchMode::Replace => {
                    "running applications won't be reused or closed and window positions won't be restored"
                }
            };
            warnings.push(format!(
                "Reframe can't see open windows on this system, so {consequence}{}",
                detail(&caps.list_windows)
            ));
        }
        SupportLevel::Partial => warnings.push(format!(
            "Some open windows may not be visible to Reframe{}",
            detail(&caps.list_windows)
        )),
        SupportLevel::Full => {}
    }
    if mode == SwitchMode::Replace && caps.list_windows.is_available() {
        match caps.close_windows.level {
            SupportLevel::Unsupported => warnings.push(format!(
                "Closing windows isn't available on this system{}",
                detail(&caps.close_windows)
            )),
            SupportLevel::Partial => warnings.push(format!(
                "Some applications may not close when asked{}",
                detail(&caps.close_windows)
            )),
            SupportLevel::Full => {}
        }
    }
    let wants_positions = target.restore.restore_window_positions
        && target
            .applications
            .iter()
            .any(|e| e.enabled && e.options.restore_window_positions && !e.windows.is_empty());
    if wants_positions && caps.list_windows.is_available() {
        match caps.move_windows.level {
            SupportLevel::Unsupported => warnings.push(format!(
                "Window positions can't be restored on this system{}",
                detail(&caps.move_windows)
            )),
            SupportLevel::Partial => warnings.push(format!(
                "Some window positions may not be restored{}",
                detail(&caps.move_windows)
            )),
            SupportLevel::Full => {}
        }
    }
    warnings
}

/// `": <note>"` when the capability carries an explanation.
fn detail(capability: &Capability) -> String {
    capability
        .note
        .as_deref()
        .map(|n| format!(": {}", n.trim_end_matches('.')))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reframe_platform::mock::MockPlatform;
    use reframe_platform::{Platform, ProcessInfo, Rect, WindowState};
    use std::path::PathBuf;

    use crate::model::WindowSnapshot;

    struct Fixture {
        platform: MockPlatform,
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                platform: MockPlatform::new(),
                dir: tempfile::tempdir().unwrap(),
            }
        }

        /// A fake installed executable.
        fn exe(&self, name: &str) -> PathBuf {
            let path = self.dir.path().join(name);
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
            path
        }

        fn entry(&self, name: &str) -> ApplicationEntry {
            let mut e = ApplicationEntry::new(format!("exe:{name}"), name, "generic");
            e.launch.executable = Some(self.exe(name));
            e
        }

        /// Start `name` with one window; returns the window id.
        fn running(&self, name: &str, title: &str) -> WindowId {
            let pid = self.platform.add_process(name, Some(self.exe(name)));
            self.platform
                .add_window(pid, title, None, Rect::new(0, 0, 400, 300))
        }

        fn state(&self) -> DesktopState {
            DesktopState {
                info: self.platform.info(),
                monitors: self.platform.monitors().unwrap(),
                windows: self.platform.windows().unwrap(),
                processes: self.platform.processes().unwrap(),
                warnings: Vec::new(),
            }
        }
    }

    fn adapters() -> AdapterRegistry {
        AdapterRegistry::with_adapters(Vec::new())
    }

    fn workspace(name: &str, entries: Vec<ApplicationEntry>) -> Workspace {
        let mut ws = Workspace::new(name, "🧪");
        ws.applications = entries;
        ws
    }

    fn plan(
        f: &Fixture,
        previous: Option<&Workspace>,
        target: &Workspace,
        mode: SwitchMode,
    ) -> SwitchPlan {
        let state = f.state();
        plan_switch(&state, &adapters(), &state.info, previous, target, mode)
    }

    fn launched(plan: &SwitchPlan) -> Vec<&str> {
        plan.launch
            .iter()
            .map(|l| l.display_name.as_str())
            .collect()
    }

    #[test]
    fn additive_launches_every_enabled_entry() {
        let f = Fixture::new();
        f.running("editor", "notes — Editor");
        let mut disabled = f.entry("music");
        disabled.enabled = false;
        let target = workspace("Dev", vec![f.entry("editor"), f.entry("browser"), disabled]);
        let p = plan(&f, None, &target, SwitchMode::Additive);
        assert_eq!(launched(&p), vec!["editor", "browser"]);
        assert!(
            p.launch
                .iter()
                .all(|l| l.reason.starts_with("Additive mode"))
        );
        assert!(p.reuse.is_empty() && p.close.is_empty());
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        assert_eq!(p.workspace_id, target.id);
        assert_eq!(p.mode, SwitchMode::Additive);
    }

    #[test]
    fn merge_reuses_running_entries_and_launches_the_rest() {
        let f = Fixture::new();
        let editor_window = f.running("editor", "notes — Editor");
        f.running("chat", "General — Chat");
        let target = workspace("Dev", vec![f.entry("editor"), f.entry("browser")]);
        let p = plan(&f, None, &target, SwitchMode::Merge);
        assert_eq!(launched(&p), vec!["browser"]);
        assert_eq!(p.launch[0].reason, "Not running");
        assert_eq!(p.reuse.len(), 1);
        assert_eq!(p.reuse[0].display_name, "editor");
        assert_eq!(p.reuse[0].window_ids, vec![editor_window]);
        assert!(p.close.is_empty(), "Merge never closes anything");
    }

    #[test]
    fn replace_proposes_windows_outside_the_target() {
        let f = Fixture::new();
        f.running("editor", "notes — Editor");
        let chat = f.running("chat", "General — Chat");
        let game = f.running("game", "Solitaire");
        let untitled_pid = f.platform.add_process("helper", None);
        f.platform
            .add_window(untitled_pid, "  ", None, Rect::new(0, 0, 10, 10));
        let me = f.platform.add_process_info(ProcessInfo {
            pid: std::process::id(),
            parent_pid: None,
            name: "reframe".into(),
            product_name: None,
            executable_path: None,
            command_line: vec![],
            working_directory: None,
            start_time: None,
            package: None,
        });
        f.platform
            .add_window(me, "Reframe", None, Rect::new(0, 0, 10, 10));
        let previous = workspace("Morning", vec![f.entry("chat"), f.entry("editor")]);
        let mut disabled_game = f.entry("game");
        disabled_game.enabled = false;
        let target = workspace("Deep Work", vec![f.entry("editor"), f.entry("browser")]);

        let p = plan(&f, Some(&previous), &target, SwitchMode::Replace);
        assert_eq!(p.previous_workspace_id, Some(previous.id));
        assert_eq!(launched(&p), vec!["browser"]);
        assert_eq!(p.reuse.len(), 1);
        let close: Vec<(&WindowId, bool, &str)> = p
            .close
            .iter()
            .map(|c| (&c.window_id, c.default_selected, c.reason.as_str()))
            .collect();
        assert_eq!(
            close,
            vec![
                (&game, false, "Not part of Deep Work"),
                (&chat, true, "Part of Morning but not of Deep Work"),
            ]
        );
        let chat_candidate = p.close.iter().find(|c| c.window_id == chat).unwrap();
        assert_eq!(chat_candidate.app_id, "exe:chat");
        assert_eq!(chat_candidate.app_name, "Chat");
        assert_eq!(chat_candidate.title, "General — Chat");

        // Applications of the target are kept even when their entry is disabled.
        let target = workspace("Games", vec![disabled_game]);
        let p = plan(&f, Some(&previous), &target, SwitchMode::Replace);
        assert!(p.close.iter().all(|c| c.window_id != game));
    }

    #[test]
    fn warnings_for_entries_that_cannot_launch() {
        let f = Fixture::new();
        let mut missing = ApplicationEntry::new("exe:gone", "Gone", "generic");
        missing.launch.executable = Some(f.dir.path().join("gone"));
        let unconfigured = ApplicationEntry::new("exe:nothing", "Nothing", "generic");
        let mut bad_folder = f.entry("tool");
        bad_folder.launch.working_directory = Some(f.dir.path().join("no-such-folder"));
        let target = workspace("Dev", vec![missing, unconfigured, bad_folder]);
        let p = plan(&f, None, &target, SwitchMode::Merge);
        assert_eq!(p.warnings.len(), 3, "{:?}", p.warnings);
        assert!(p.warnings[0].starts_with("Gone isn't installed at"));
        assert!(p.warnings[0].ends_with("edit the application to choose its location"));
        assert_eq!(p.warnings[1], "No executable is configured for Nothing.");
        assert!(p.warnings[2].contains("working folder"));
    }

    #[test]
    fn platform_limitations_are_warned_about() {
        let f = Fixture::new();
        f.running("chat", "General — Chat");
        let mut info = f.platform.info();
        info.capabilities.close_windows = Capability::unsupported("Not allowed here.");
        info.capabilities.move_windows = Capability::partial("Only X11 applications can be moved.");
        f.platform.set_info(info);
        let mut entry = f.entry("editor");
        entry.windows.push(WindowSnapshot {
            title: "notes".into(),
            class_name: None,
            bounds: Rect::new(0, 0, 400, 300),
            state: WindowState::Normal,
            monitor_id: None,
        });
        let target = workspace("Dev", vec![entry]);
        let p = plan(&f, None, &target, SwitchMode::Replace);
        assert!(p.close.is_empty(), "closing is unsupported");
        assert_eq!(
            p.warnings,
            vec![
                "Closing windows isn't available on this system: Not allowed here".to_string(),
                "Some window positions may not be restored: Only X11 applications can be moved"
                    .to_string(),
            ]
        );

        let mut info = f.platform.info();
        info.capabilities.list_windows = Capability::unsupported("Wayland hides other windows.");
        f.platform.set_info(info);
        let p = plan(&f, None, &target, SwitchMode::Merge);
        assert!(p.reuse.is_empty());
        assert_eq!(launched(&p), vec!["editor"]);
        assert_eq!(
            p.launch[0].reason,
            "Running applications can't be detected on this system"
        );
        assert_eq!(p.warnings.len(), 1, "{:?}", p.warnings);
        assert!(p.warnings[0].starts_with("Reframe can't see open windows on this system"));
        assert!(p.warnings[0].ends_with("Wayland hides other windows"));
    }
}
