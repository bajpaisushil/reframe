//! Workspace restore-readiness report ("Missing applications", "Supported restoration
//! features", "Estimated restoration status").

use std::collections::HashSet;
use std::path::PathBuf;

use reframe_platform::{Capability, LaunchTarget, Platform, SupportLevel};

use crate::adapters::{AdapterRegistry, LaunchContext, LaunchStep};
use crate::model::{
    ApplicationEntry, CapabilityLevel, DesktopState, EntryHealth, RestoreEstimate, Workspace,
    WorkspaceHealth,
};
use crate::portability::is_network_path;
use crate::restoration::{layout, planner, reuse, validate};

/// How ready `workspace` is to be restored on this machine, given a fresh desktop `state`.
///
/// Per entry (disabled ones included, so the page can show them):
/// - `available`: the adapter can build a launch plan, its programs exist and its working
///   folder exists — i.e. a restore would not fail validation.
/// - `launch_target`: what would be launched when available. When the configured program is
///   missing, the platform is asked to locate the application
///   ([`Platform::locate_application`] with the adapter's `locate_query`); the location it
///   finds is reported here (with an issue suggesting to use it) and `available` stays false
///   until the user edits the entry. `None` when nothing launchable is known.
/// - `missing_paths`: saved files/folders ([`AppState::paths`](crate::model::AppState::paths))
///   and the working folder that no longer exist.
/// - `capability`, `details`: from the adapter; `running`: everything the entry opens is open
///   already, so a Merge/Replace switch would reuse it without launching anything (the same
///   decision as the restore: a project or folder counts when a window shows its name, a
///   generic application when it has any window).
///
/// The estimate counts *enabled* entries by what a restore could achieve: `unavailable` when
/// the entry is not available or the adapter cannot restore it here, otherwise by the
/// adapter's capability level — a `Full` entry with missing saved paths counts as `partial`.
/// It does not depend on the switch mode, so an application that is running from a different
/// location than configured still counts as unavailable.
pub fn workspace_health(
    workspace: &Workspace,
    state: &DesktopState,
    platform: &dyn Platform,
    adapters: &AdapterRegistry,
) -> WorkspaceHealth {
    let ctx = LaunchContext {
        platform,
        info: &state.info,
    };
    // Enabled entries share running windows the way a restore hands them out.
    let mut taken = HashSet::new();
    let entries: Vec<EntryHealth> = workspace
        .applications
        .iter()
        .map(|entry| {
            let plan = adapters.get(&entry.adapter_id).launch_plan(&ctx, entry);
            let mut own = HashSet::new();
            let taken = if entry.enabled { &mut taken } else { &mut own };
            let running = reuse::decide_entry(entry, &plan, state, adapters, taken)
                .is_some_and(|r| r.complete());
            entry_health(entry, &plan, running, state, platform, adapters)
        })
        .collect();
    let estimate = estimate(workspace, &entries);
    WorkspaceHealth {
        workspace_id: workspace.id,
        entries,
        monitors: layout::map_monitors(&workspace.monitors, &state.monitors),
        dependency_issues: planner::dependency_issues(workspace),
        platform_notes: platform_notes(workspace, state),
        estimate,
    }
}

fn entry_health(
    entry: &ApplicationEntry,
    plan: &Result<Vec<LaunchStep>, String>,
    running: bool,
    state: &DesktopState,
    platform: &dyn Platform,
    adapters: &AdapterRegistry,
) -> EntryHealth {
    let name = entry.display_name.as_str();
    let adapter = adapters.get(&entry.adapter_id);
    let capability = adapter.capability(&state.info);
    let mut issues = Vec::new();
    let mut launch_target = None;
    let mut target_ok = false;
    // The program itself is missing or unknown (not just a file or folder it should open).
    let mut program_missing = true;
    let mut folder_ok = true;

    match plan {
        Ok(steps) if steps.is_empty() => issues.push(format!(
            "Reframe doesn't know how to start {name} - edit the application to choose its location"
        )),
        Ok(steps) => {
            match steps.iter().find_map(|s| {
                validate::check_target(&s.request.target, name)
                    .err()
                    .map(|problem| (&s.request.target, problem))
            }) {
                Some((target, problem)) => {
                    program_missing = !matches!(target, LaunchTarget::Open { .. });
                    issues.push(problem);
                }
                None => {
                    target_ok = true;
                    launch_target = Some(steps[0].request.target.clone());
                }
            }
            if let Some(problem) = steps
                .iter()
                .find_map(|s| validate::check_working_directory(&s.request, name).err())
            {
                folder_ok = false;
                issues.push(problem);
            }
        }
        Err(problem) => issues.push(problem.clone()),
    }
    if !target_ok && program_missing {
        let found = platform
            .locate_application(&adapter.locate_query(&entry.app_id))
            .into_iter()
            .find(|target| validate::check_target(target, name).is_ok());
        if let Some(found) = found {
            issues.push(format!(
                "{name} was found at {} - edit the application to use it",
                found.label()
            ));
            launch_target = Some(found);
        }
    }

    // Network locations (\\server\share, /net/...) are never probed: on Windows merely
    // checking one sends the user's credentials to that server, and imported workspaces are
    // untrusted. They are reported as unchecked instead.
    let mut missing_paths: Vec<PathBuf> = Vec::new();
    let mut network_paths = 0usize;
    for path in entry.state.paths() {
        if path.as_os_str().is_empty() || missing_paths.contains(&path) {
            continue;
        }
        if is_network_path(&path) {
            network_paths += 1;
        } else if !path.exists() {
            missing_paths.push(path);
        }
    }
    if network_paths > 0 {
        issues.push(format!(
            "{network_paths} saved location{} on another computer {} not checked",
            if network_paths == 1 { "" } else { "s" },
            if network_paths == 1 { "was" } else { "were" }
        ));
    }
    match missing_paths.as_slice() {
        [] => {}
        [path] => issues.push(format!("{} no longer exists", path.display())),
        paths => issues.push(format!(
            "{} saved files or folders no longer exist",
            paths.len()
        )),
    }
    if let Some(dir) = &entry.launch.working_directory
        && !is_network_path(dir)
        && !dir.is_dir()
        && !missing_paths.contains(dir)
    {
        missing_paths.push(dir.clone());
    }
    if capability.level == CapabilityLevel::Unsupported {
        issues.push(format!("Reframe can't restore {name} on this system"));
    }

    EntryHealth {
        entry_id: entry.id,
        available: target_ok && folder_ok,
        launch_target,
        missing_paths,
        running,
        details: adapter.describe(&entry.state),
        capability,
        issues,
    }
}

fn estimate(workspace: &Workspace, entries: &[EntryHealth]) -> RestoreEstimate {
    let mut estimate = RestoreEstimate::default();
    for (entry, health) in workspace.applications.iter().zip(entries) {
        if !entry.enabled {
            continue;
        }
        match health.capability.level {
            _ if !health.available => estimate.unavailable += 1,
            CapabilityLevel::Unsupported => estimate.unavailable += 1,
            CapabilityLevel::Full if health.missing_paths.is_empty() => estimate.full += 1,
            CapabilityLevel::Full | CapabilityLevel::Partial => estimate.partial += 1,
            CapabilityLevel::LaunchOnly => estimate.launch_only += 1,
        }
    }
    estimate
}

/// Platform limitations relevant to this workspace.
fn platform_notes(workspace: &Workspace, state: &DesktopState) -> Vec<String> {
    let caps = &state.info.capabilities;
    let mut notes = Vec::new();
    notes.extend(limitation("Detecting open windows", &caps.list_windows));
    let wants_positions = workspace.restore.restore_window_positions
        && workspace
            .applications
            .iter()
            .any(|e| e.enabled && e.options.restore_window_positions && !e.windows.is_empty());
    if wants_positions {
        notes.extend(limitation("Restoring window positions", &caps.move_windows));
        if !workspace.monitors.is_empty() && state.monitors.is_empty() {
            notes.push(
                "Connected displays could not be detected, so windows keep their saved coordinates."
                    .to_string(),
            );
        }
    }
    notes
}

fn limitation(what: &str, capability: &Capability) -> Option<String> {
    let detail = capability
        .note
        .as_deref()
        .map(|n| format!(": {}", n.trim_end_matches('.')))
        .unwrap_or_default();
    match capability.level {
        SupportLevel::Full => None,
        SupportLevel::Partial => Some(format!("{what} is limited on this system{detail}")),
        SupportLevel::Unsupported => Some(format!("{what} isn't available on this system{detail}")),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::adapters::generic::GenericAdapter;
    use crate::adapters::{
        AppAdapter, AppIdentity, CaptureContext, CaptureInput, CaptureOutput, LaunchStep,
    };
    use crate::model::{AdapterCapability, AppState, ProjectRef, StateDetail, WindowSnapshot};
    use reframe_platform::mock::MockPlatform;
    use reframe_platform::{
        LaunchTarget, LocateQuery, MonitorInfo, PlatformInfo, ProcessInfo, Rect, WindowInfo,
        WindowState,
    };

    /// A "full support" project adapter that launches like the generic adapter.
    struct ProjectsAdapter;

    impl AppAdapter for ProjectsAdapter {
        fn id(&self) -> &'static str {
            "projects-test"
        }
        fn name(&self) -> &'static str {
            "Projects (test)"
        }
        fn identify(&self, _: &ProcessInfo, _: &[&WindowInfo]) -> Option<AppIdentity> {
            None
        }
        fn capability(&self, _: &PlatformInfo) -> AdapterCapability {
            AdapterCapability {
                level: CapabilityLevel::Full,
                features: Vec::new(),
            }
            .feature("Reopen projects", true, None)
        }
        fn capture(&self, _: &CaptureContext<'_>, _: &CaptureInput<'_>) -> CaptureOutput {
            CaptureOutput::default()
        }
        fn launch_plan(
            &self,
            ctx: &LaunchContext<'_>,
            entry: &ApplicationEntry,
        ) -> Result<Vec<LaunchStep>, String> {
            GenericAdapter::new().launch_plan(ctx, entry)
        }
        fn describe(&self, state: &AppState) -> Vec<StateDetail> {
            match state {
                AppState::Projects { projects } => {
                    vec![StateDetail::new("Projects", projects.len().to_string())]
                }
                _ => Vec::new(),
            }
        }
        fn locate_query(&self, app_id: &str) -> LocateQuery {
            GenericAdapter::new().locate_query(app_id)
        }
    }

    fn adapters() -> AdapterRegistry {
        AdapterRegistry::with_adapters(vec![Arc::new(ProjectsAdapter)])
    }

    fn state(platform: &MockPlatform) -> DesktopState {
        DesktopState {
            info: platform.info(),
            monitors: platform.monitors().unwrap(),
            windows: platform.windows().unwrap(),
            processes: platform.processes().unwrap(),
            warnings: Vec::new(),
        }
    }

    fn snapshot() -> WindowSnapshot {
        WindowSnapshot {
            title: "w".into(),
            class_name: None,
            bounds: Rect::new(0, 0, 400, 300),
            state: WindowState::Normal,
            monitor_id: Some("MOCK-1".into()),
        }
    }

    #[test]
    fn network_locations_are_never_probed_and_reported_as_unchecked() {
        let platform = MockPlatform::new();
        let dir = tempfile::tempdir().unwrap();
        let editor_exe = dir.path().join("editor");
        std::fs::write(&editor_exe, "#!/bin/sh\n").unwrap();
        let mut editor = ApplicationEntry::new("exe:editor", "Editor", "projects-test");
        editor.launch.executable = Some(editor_exe);
        editor.launch.working_directory = Some(PathBuf::from(r"\\attacker\share\work"));
        editor.state = AppState::Projects {
            projects: vec![
                ProjectRef {
                    path: PathBuf::from(r"\\attacker\share\project"),
                    is_workspace_file: false,
                    window_title: None,
                },
                ProjectRef {
                    path: PathBuf::from("//attacker/share/other"),
                    is_workspace_file: false,
                    window_title: None,
                },
            ],
        };
        let mut ws = Workspace::new("Imported", "📦");
        ws.applications = vec![editor];
        let health = workspace_health(&ws, &state(&platform), &platform, &adapters());
        let entry = &health.entries[0];
        assert!(
            entry.missing_paths.is_empty(),
            "network paths are not reported missing: {entry:?}"
        );
        assert!(
            entry
                .issues
                .iter()
                .any(|i| i.contains("2 saved locations on another computer were not checked")),
            "{:?}",
            entry.issues
        );
    }

    #[test]
    fn reports_availability_paths_running_and_estimate() {
        let platform = MockPlatform::new();
        let dir = tempfile::tempdir().unwrap();
        let editor_exe = dir.path().join("editor");
        std::fs::write(&editor_exe, "#!/bin/sh\n").unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let gone_project = dir.path().join("gone-project");

        // Running full-support editor with one missing project folder.
        let mut editor = ApplicationEntry::new("exe:editor", "Editor", "projects-test");
        editor.launch.executable = Some(editor_exe.clone());
        editor.state = AppState::Projects {
            projects: vec![
                ProjectRef {
                    path: project.clone(),
                    is_workspace_file: false,
                    window_title: None,
                },
                ProjectRef {
                    path: gone_project.clone(),
                    is_workspace_file: false,
                    window_title: None,
                },
            ],
        };
        editor.windows.push(snapshot());
        let pid = platform.add_process("editor", Some(editor_exe.clone()));
        platform.add_window(pid, "project — Editor", None, Rect::new(0, 0, 400, 300));

        // Moved application: the configured path is gone, but it is installed elsewhere.
        let relocated = dir.path().join("viewer-new");
        std::fs::write(&relocated, "#!/bin/sh\n").unwrap();
        platform.install(
            "viewer",
            LaunchTarget::Executable {
                path: relocated.clone(),
            },
        );
        let mut viewer = ApplicationEntry::new("exe:viewer", "Viewer", "generic");
        viewer.launch.executable = Some(dir.path().join("viewer"));

        // Not installed anywhere, and a disabled entry that is ignored by the estimate.
        let mut missing = ApplicationEntry::new("exe:missing", "Missing", "generic");
        missing.launch.executable = Some(dir.path().join("missing"));
        let mut disabled = missing.clone();
        disabled.id = uuid::Uuid::new_v4();
        disabled.enabled = false;

        let mut ws = Workspace::new("Dev", "💻");
        ws.applications = vec![editor.clone(), viewer, missing, disabled];
        ws.monitors = vec![
            platform.monitors().unwrap()[0].clone(),
            MonitorInfo {
                id: "GONE".into(),
                name: "Old display".into(),
                bounds: Rect::new(1920, 0, 1920, 1080),
                work_area: Rect::new(1920, 0, 1920, 1040),
                scale_factor: 1.0,
                is_primary: false,
            },
        ];

        let health = workspace_health(&ws, &state(&platform), &platform, &adapters());
        assert_eq!(health.workspace_id, ws.id);
        assert_eq!(health.entries.len(), 4);

        let e = &health.entries[0];
        assert!(e.available && e.running);
        assert_eq!(
            e.launch_target,
            Some(LaunchTarget::Executable { path: editor_exe })
        );
        assert_eq!(e.missing_paths, vec![gone_project.clone()]);
        assert_eq!(
            e.issues,
            vec![format!("{} no longer exists", gone_project.display())]
        );
        assert_eq!(e.capability.level, CapabilityLevel::Full);
        assert_eq!(e.details, vec![StateDetail::new("Projects", "2")]);

        let v = &health.entries[1];
        assert!(!v.available && !v.running);
        assert_eq!(
            v.launch_target,
            Some(LaunchTarget::Executable {
                path: relocated.clone()
            })
        );
        assert_eq!(v.issues.len(), 2, "{:?}", v.issues);
        assert!(v.issues[0].starts_with("Viewer isn't installed at"));
        assert_eq!(
            v.issues[1],
            format!(
                "Viewer was found at {} - edit the application to use it",
                relocated.display()
            )
        );

        let m = &health.entries[2];
        assert!(!m.available);
        assert_eq!(m.launch_target, None);
        assert_eq!(m.capability.level, CapabilityLevel::LaunchOnly);

        assert_eq!(
            health.estimate,
            RestoreEstimate {
                full: 0,
                partial: 1,
                launch_only: 0,
                unavailable: 2
            }
        );
        let mappings: Vec<(&str, Option<&str>, bool)> = health
            .monitors
            .iter()
            .map(|m| (m.saved_id.as_str(), m.mapped_id.as_deref(), m.exact))
            .collect();
        assert_eq!(
            mappings,
            vec![("MOCK-1", Some("MOCK-1"), true), ("GONE", None, false)]
        );
        assert!(health.dependency_issues.is_empty());
        assert!(
            health.platform_notes.is_empty(),
            "{:?}",
            health.platform_notes
        );
    }

    #[test]
    fn full_entry_without_problems_counts_as_full() {
        let platform = MockPlatform::new();
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("ide");
        std::fs::write(&exe, "").unwrap();
        let mut ide = ApplicationEntry::new("exe:ide", "IDE", "projects-test");
        ide.launch.executable = Some(exe);
        ide.launch.working_directory = Some(dir.path().to_path_buf());
        let mut ws = Workspace::new("Dev", "💻");
        ws.applications = vec![ide];
        let health = workspace_health(&ws, &state(&platform), &platform, &adapters());
        assert!(health.entries[0].available);
        assert!(health.entries[0].issues.is_empty());
        assert_eq!(health.estimate.full, 1);
    }

    #[test]
    fn missing_working_folder_makes_an_entry_unavailable() {
        let platform = MockPlatform::new();
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("shell");
        std::fs::write(&exe, "").unwrap();
        let folder = dir.path().join("gone");
        let mut shell = ApplicationEntry::new("exe:shell", "Shell", "generic");
        shell.launch.executable = Some(exe);
        shell.launch.working_directory = Some(folder.clone());
        let mut ws = Workspace::new("Dev", "💻");
        ws.applications = vec![shell];
        let health = workspace_health(&ws, &state(&platform), &platform, &adapters());
        let e = &health.entries[0];
        assert!(!e.available);
        assert_eq!(e.missing_paths, vec![folder]);
        assert_eq!(e.issues.len(), 1);
        assert!(e.issues[0].contains("working folder"));
        assert_eq!(health.estimate.unavailable, 1);
    }

    /// Opens each saved folder through the OS handler (one `Open` step per folder).
    struct FoldersAdapter;

    impl AppAdapter for FoldersAdapter {
        fn id(&self) -> &'static str {
            "folders-test"
        }
        fn name(&self) -> &'static str {
            "Folders (test)"
        }
        fn identify(&self, _: &ProcessInfo, _: &[&WindowInfo]) -> Option<AppIdentity> {
            None
        }
        fn capability(&self, _: &PlatformInfo) -> AdapterCapability {
            AdapterCapability::launch_only()
        }
        fn capture(&self, _: &CaptureContext<'_>, _: &CaptureInput<'_>) -> CaptureOutput {
            CaptureOutput::default()
        }
        fn launch_plan(
            &self,
            _: &LaunchContext<'_>,
            entry: &ApplicationEntry,
        ) -> Result<Vec<LaunchStep>, String> {
            Ok(entry
                .state
                .paths()
                .into_iter()
                .map(|path| LaunchStep {
                    request: reframe_platform::LaunchRequest {
                        target: LaunchTarget::Open {
                            target: path.display().to_string(),
                        },
                        args: Vec::new(),
                        working_directory: None,
                        env: Vec::new(),
                    },
                    expected_windows: 1,
                    title_hint: None,
                })
                .collect())
        }
        fn describe(&self, _: &AppState) -> Vec<StateDetail> {
            Vec::new()
        }
        fn locate_query(&self, _: &str) -> LocateQuery {
            LocateQuery {
                executable_names: vec!["files".into()],
                ..Default::default()
            }
        }
    }

    #[test]
    fn a_missing_folder_is_not_blamed_on_the_program() {
        let platform = MockPlatform::new();
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("files");
        std::fs::write(&program, "").unwrap();
        platform.install("files", LaunchTarget::Executable { path: program });
        let kept = dir.path().join("kept");
        std::fs::create_dir(&kept).unwrap();
        let gone = dir.path().join("gone");
        let mut files = ApplicationEntry::new("files", "Files", "folders-test");
        files.state = AppState::Folders {
            folders: [&kept, &gone]
                .into_iter()
                .map(|p| crate::model::FolderRef {
                    path: p.clone(),
                    window_title: None,
                })
                .collect(),
        };
        let mut ws = Workspace::new("Dev", "💻");
        ws.applications = vec![files];
        let adapters = AdapterRegistry::with_adapters(vec![Arc::new(FoldersAdapter)]);
        let health = workspace_health(&ws, &state(&platform), &platform, &adapters);
        let e = &health.entries[0];
        assert!(!e.available);
        assert_eq!(e.missing_paths, vec![gone.clone()]);
        assert!(
            e.issues.iter().all(|i| !i.contains("was found at")),
            "the program is fine, only a folder is missing: {:?}",
            e.issues
        );
        assert_eq!(e.launch_target, None);
        assert!(e.issues[0].starts_with(&format!("{} no longer exists", gone.display())));
    }

    #[test]
    fn dependency_issues_and_platform_notes() {
        let platform = MockPlatform::new();
        let mut info = platform.info();
        info.capabilities.list_windows = Capability::partial("Only X11 applications are visible.");
        info.capabilities.move_windows = Capability::unsupported("Wayland does not allow it.");
        platform.set_info(info);
        let mut a = ApplicationEntry::new("exe:a", "Terminal", "generic");
        let mut b = ApplicationEntry::new("exe:b", "VS Code", "generic");
        a.depends_on.push(b.id);
        b.depends_on.push(a.id);
        b.windows.push(snapshot());
        let mut ws = Workspace::new("Dev", "💻");
        ws.applications = vec![a, b];
        ws.monitors = platform.monitors().unwrap();
        let health = workspace_health(&ws, &state(&platform), &platform, &adapters());
        assert_eq!(
            health.dependency_issues,
            vec!["Circular dependency: Terminal -> VS Code -> Terminal".to_string()]
        );
        assert_eq!(
            health.platform_notes,
            vec![
                "Detecting open windows is limited on this system: Only X11 applications are visible"
                    .to_string(),
                "Restoring window positions isn't available on this system: Wayland does not allow it"
                    .to_string(),
            ]
        );

        // Displays that cannot be detected are explained.
        let mut blind = state(&platform);
        blind.monitors.clear();
        let health = workspace_health(&ws, &blind, &platform, &adapters());
        assert!(
            health
                .platform_notes
                .iter()
                .any(|n| n.starts_with("Connected displays could not be detected"))
        );
    }
}
