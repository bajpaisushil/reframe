//! Visual Studio Code, VS Code Insiders and VSCodium: reopens the project folders and
//! `.code-workspace` files shown in the captured windows.
//!
//! # Capture
//!
//! VS Code has no API that tells another process which folder each window shows, so the
//! adapter combines information that is available locally, all of it read without
//! modification:
//!
//! 1. **Window titles** (`title` module): the default `window.title` contains the folder or
//!    workspace name ("main.rs - reframe - Visual Studio Code"). Parsing is best effort.
//! 2. **VS Code's own state** (`storage` module) in its user-data directory
//!    (`%APPDATA%\Code`, `~/Library/Application Support/Code`, `~/.config/Code`, Flatpak and
//!    portable locations): the "Open Recent" list in `state.vscdb` and the window state in
//!    `storage.json` turn the name into a full path. The most recently used location that
//!    still exists wins (`resolve` module).
//! 3. **The main process command line** (`cmdline` module): the folders it was started with
//!    (as extra candidates) and `--user-data-dir`. Command lines are never stored.
//!
//! Each resolved window becomes a [`ProjectRef`]. The launch configuration is only the main
//! process executable / package.
//!
//! # Restore
//!
//! One `--new-window <path>` launch per project that still exists — on macOS through the
//! application bundle (the platform runs `open -n -b <bundle id> --args ...`). VS Code
//! forwards each launch to its running instance. Without projects VS Code is started
//! plainly and reopens its own last session. Open editors, terminals and unsaved changes are
//! restored by VS Code itself (hot exit, `window.restoreWindows`), not by Reframe.
//!
//! # Limitations
//!
//! - A custom `window.title` that hides the folder name prevents matching; such windows get
//!   a warning and are left to VS Code's own session restore.
//! - Remote windows (SSH, WSL, containers, Codespaces, tunnels) and virtual workspaces can't
//!   be reopened locally and are skipped with a warning, as are Extension Development Host
//!   windows and unsaved ("Untitled") multi-root workspaces.
//! - When two different folders share a name, the most recently opened existing one is
//!   assigned first.
//! - A title segment equal to one of the user's profile names is taken to be
//!   `${profileName}`, so a folder named exactly like a profile is not recognised in a window
//!   that uses another profile and has an editor open (the title can't tell the two apart).
//! - Windows that show no folder (empty windows, loose files) are not reopened.
//! - Instances started with a custom `--user-data-dir` are captured, but restored into the
//!   default instance because command-line arguments are not stored.

mod cmdline;
mod paths;
mod resolve;
mod storage;
mod title;
mod uri;
mod variant;

pub use variant::Variant;

use std::path::{Path, PathBuf};

use reframe_platform::{
    LaunchRequest, LaunchTarget, LocateQuery, OsKind, PackageInfo, PackageKind, PlatformInfo,
    ProcessInfo, SupportLevel, WindowInfo,
};

use super::generic;
use super::{
    AppAdapter, AppIdentity, CaptureContext, CaptureInput, CaptureOutput, InstanceModel,
    LaunchContext, LaunchStep,
};
use crate::model::{
    AdapterCapability, AppState, ApplicationEntry, CapabilityLevel, LaunchConfig, ProjectRef,
    StateDetail,
};
use resolve::{Match, Resolver};
use storage::RecentKind;

/// Environment variable that replaces the user-data directory lookup with a fixed list of
/// directories (separated like `PATH`). Only read when set; meant for diagnostics.
pub const USER_DATA_ENV: &str = "REFRAME_VSCODE_USER_DATA";

const NEW_WINDOW: &str = "--new-window";

#[derive(Debug, Default)]
pub struct VsCodeAdapter {
    /// Fixed user-data directories; `None` = find them for each capture.
    user_data_dirs: Option<Vec<PathBuf>>,
}

impl VsCodeAdapter {
    pub fn new() -> Self {
        Self {
            user_data_dirs: None,
        }
    }

    /// An adapter that reads VS Code's state only from `dirs` (user-data directories such as
    /// `~/.config/Code`) instead of locating them. Used by tests.
    pub fn with_user_data_dirs(dirs: Vec<PathBuf>) -> Self {
        Self {
            user_data_dirs: Some(dirs),
        }
    }

    /// User-data directories to read for the instances in `processes` (each process with its
    /// command-line hints), most likely first.
    fn user_data_dirs_for(
        &self,
        variant: Variant,
        os: OsKind,
        processes: &[(&ProcessInfo, cmdline::CommandLineHints)],
    ) -> Vec<PathBuf> {
        if let Some(fixed) = &self.user_data_dirs {
            return fixed.clone();
        }
        if let Some(value) = std::env::var_os(USER_DATA_ENV).filter(|v| !v.is_empty()) {
            return std::env::split_paths(&value)
                .filter(|p| !p.as_os_str().is_empty())
                .collect();
        }
        let env_dir = |key: &str| {
            std::env::var_os(key)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let (config_dir, home) = (dirs::config_dir(), dirs::home_dir());
        let (vscode_portable, vscode_appdata) =
            (env_dir("VSCODE_PORTABLE"), env_dir("VSCODE_APPDATA"));
        let mut found: Vec<PathBuf> = Vec::new();
        for (process, hints) in processes {
            let inputs = paths::UserDataInputs {
                cli_user_data_dir: hints.user_data_dir.clone(),
                executable: process.executable_path.as_deref(),
                package: process.package.as_ref(),
                config_dir: config_dir.clone(),
                home: home.clone(),
                vscode_portable: vscode_portable.clone(),
                vscode_appdata: vscode_appdata.clone(),
            };
            for dir in paths::user_data_dirs(variant, os, &inputs) {
                if !found.contains(&dir) {
                    found.push(dir);
                }
            }
        }
        found
    }

    /// One project per window whose folder / workspace could be determined; problems become
    /// warnings. `processes` own the windows (several when more than one instance runs, e.g.
    /// with a separate `--user-data-dir`), the main process first.
    fn capture_projects(
        &self,
        variant: Variant,
        os: OsKind,
        processes: &[&ProcessInfo],
        windows: &[&WindowInfo],
        warnings: &mut Vec<String>,
    ) -> Vec<ProjectRef> {
        let parsed: Vec<(&ProcessInfo, cmdline::CommandLineHints)> = processes
            .iter()
            .map(|p| {
                let hints = cmdline::parse(&p.command_line, p.working_directory.as_deref(), os);
                (*p, hints)
            })
            .collect();
        let user_data = self.user_data_dirs_for(variant, os, &parsed);
        let state = storage::load(&user_data, os);
        let hinted: Vec<storage::Recent> = parsed
            .iter()
            .flat_map(|(_, hints)| hints.paths.iter().cloned())
            .collect();
        let candidates = resolve::merge(
            &[
                &state.recently_opened,
                &hinted,
                &state.windows,
                &state.legacy,
            ],
            os,
        );
        let mut app_names = vec![variant.display_name()];
        app_names.extend(
            Variant::ALL
                .into_iter()
                .filter(|v| *v != variant)
                .map(Variant::display_name),
        );
        let home = dirs::home_dir();
        let exists = |path: &Path, kind: RecentKind| match kind {
            RecentKind::Folder => path.is_dir(),
            RecentKind::Workspace | RecentKind::File => path.is_file(),
        };
        let resolver = Resolver {
            candidates: &candidates,
            os,
            home: home.as_deref(),
            exists: &exists,
        };

        let mut projects: Vec<ProjectRef> = Vec::new();
        let mut taken: Vec<PathBuf> = Vec::new();
        let mut unresolved: Vec<Unresolved> = Vec::new();
        for window in windows {
            let shown = window.title.trim();
            let info = title::parse(shown, &app_names, &state.profile_names);
            if info.extension_host {
                warnings.push(format!(
                    "'{shown}' is an Extension Development Host window (started by an extension debug session), so it was skipped."
                ));
            } else if let Some(remote) = &info.remote {
                warnings.push(format!(
                    "'{shown}' is a remote window ({remote}); remote folders can't be reopened locally, so it was skipped."
                ));
            } else if info.untitled_workspace {
                warnings.push(format!(
                    "'{shown}' shows an untitled workspace; save it as a .code-workspace file so it can be reopened."
                ));
            } else if info.empty_window {
                unresolved.push(Unresolved::NoFolder(shown.to_string()));
            } else if shown.is_empty() {
                unresolved.push(Unresolved::NoTitle);
            } else {
                match resolver.resolve(&info, &taken) {
                    Match::Found(found) => {
                        taken.push(found.path.clone());
                        projects.push(ProjectRef {
                            path: found.path,
                            is_workspace_file: found.is_workspace_file,
                            window_title: Some(window.title.clone()),
                        });
                    }
                    Match::SameAsOther(path) => log::debug!(
                        "VS Code window {shown:?} shows {}, like another captured window",
                        path.display()
                    ),
                    Match::NotFound => unresolved.push(Unresolved::Unknown(shown.to_string())),
                }
            }
        }

        let short = variant.short_name();
        let fallback = if projects.is_empty() {
            format!("{short} will reopen its last session instead.")
        } else {
            "Reframe won't reopen that window.".to_string()
        };
        let mut unknown = false;
        for item in unresolved {
            match item {
                Unresolved::Unknown(shown) => {
                    unknown = true;
                    warnings.push(format!(
                        "Couldn't tell which folder '{shown}' shows; {fallback}"
                    ));
                }
                Unresolved::NoTitle => {
                    unknown = true;
                    warnings.push(format!(
                        "Couldn't tell which folder a {short} window shows because its title isn't readable; {fallback}"
                    ));
                }
                // Without any project the plain launch already covers an empty window.
                Unresolved::NoFolder(shown) if !projects.is_empty() => warnings.push(format!(
                    "'{shown}' has no folder open, so Reframe won't reopen that window."
                )),
                Unresolved::NoFolder(_) => {}
            }
        }
        if unknown {
            if !state.problems.is_empty() {
                warnings.push(format!(
                    "Some of {short}'s state couldn't be read: {}",
                    state.problems.join("; ")
                ));
            } else if !state.found_any {
                warnings.push(format!(
                    "{short}'s list of recently opened folders wasn't found, so folder names couldn't be matched to paths."
                ));
            }
        }
        projects
    }

    /// What to start: the bundle on macOS, otherwise the configured package / executable
    /// (looked up again when the executable has moved).
    fn launch_target(
        &self,
        ctx: &LaunchContext<'_>,
        entry: &ApplicationEntry,
        variant: Variant,
    ) -> Result<LaunchTarget, String> {
        if ctx.info.os == OsKind::Macos {
            if let Some(package) = entry
                .launch
                .package
                .as_ref()
                .filter(|p| p.kind == PackageKind::MacBundle)
            {
                return Ok(LaunchTarget::Package {
                    package: package.clone(),
                });
            }
            if let Some(bundle) = entry
                .launch
                .executable
                .as_deref()
                .and_then(Path::to_str)
                .and_then(paths::mac_bundle_path)
            {
                return Ok(LaunchTarget::Package {
                    package: PackageInfo {
                        kind: PackageKind::MacBundle,
                        id: variant.bundle_id().to_string(),
                        path: Some(PathBuf::from(bundle)),
                    },
                });
            }
        }
        match generic::launch_target(&entry.launch) {
            Some(LaunchTarget::Executable { path }) if !path.exists() => ctx
                .platform
                .locate_application(&variant::locate_query(variant, ctx.info.os))
                .into_iter()
                .next()
                .ok_or_else(|| {
                    format!(
                        "{} is no longer installed at {}. Reinstall it or choose where it is installed in the workspace.",
                        entry.display_name,
                        path.display()
                    )
                }),
            Some(target) => Ok(target),
            None => Err(format!(
                "No executable is configured for {}. Choose where it is installed in the workspace before restoring.",
                entry.display_name
            )),
        }
    }
}

/// A window whose folder could not be captured.
enum Unresolved {
    /// The title names something that matches no known folder or workspace.
    Unknown(String),
    /// The window has no title to parse.
    NoTitle,
    /// The title is only the application name: no folder is open.
    NoFolder(String),
}

impl AppAdapter for VsCodeAdapter {
    fn id(&self) -> &'static str {
        "vscode"
    }

    fn name(&self) -> &'static str {
        "Visual Studio Code"
    }

    fn identify(&self, process: &ProcessInfo, windows: &[&WindowInfo]) -> Option<AppIdentity> {
        variant::identify(process, windows, OsKind::current()).map(Variant::identity)
    }

    fn capability(&self, info: &PlatformInfo) -> AdapterCapability {
        let placement = info.capabilities.move_windows.level != SupportLevel::Unsupported;
        AdapterCapability {
            level: CapabilityLevel::Partial,
            features: Vec::new(),
        }
        .feature(
            "Reopen project folders and workspaces",
            true,
            Some("Matched from window titles and VS Code's list of recently opened folders."),
        )
        .feature(
            "Open editors, terminals and unsaved changes",
            true,
            Some("Restored by VS Code itself (hot exit / window.restoreWindows), not by Reframe"),
        )
        .feature(
            "Remote (SSH/WSL/containers) windows",
            false,
            Some("Remote folders can't be reopened locally; these windows are skipped."),
        )
        .feature(
            "Window position and size",
            placement,
            info.capabilities
                .move_windows
                .note
                .as_deref()
                .filter(|_| !placement),
        )
    }

    fn capture(&self, ctx: &CaptureContext<'_>, input: &CaptureInput<'_>) -> CaptureOutput {
        let os = ctx.info.os;
        let Some(main) = input.main_process() else {
            return CaptureOutput {
                warnings: vec![format!(
                    "{}: no process information available.",
                    input.identity.display_name
                )],
                ..Default::default()
            };
        };
        let variant = Variant::from_app_id(&input.identity.app_id)
            .or_else(|| variant::identify(main, &input.windows, os))
            .unwrap_or(Variant::Stable);
        let mut warnings = Vec::new();
        if main.executable_path.is_none() && main.package.is_none() {
            warnings.push(format!(
                "{}: the executable path is not accessible; choose it in the workspace before restoring.",
                input.identity.display_name
            ));
        }
        let projects = if input.windows.is_empty() {
            Vec::new()
        } else {
            self.capture_projects(variant, os, &input.processes, &input.windows, &mut warnings)
        };
        CaptureOutput {
            state: if projects.is_empty() {
                AppState::None
            } else {
                AppState::Projects { projects }
            },
            launch: LaunchConfig {
                executable: main.executable_path.clone(),
                package: main.package.clone(),
                arguments: Vec::new(),
                working_directory: None,
            },
            warnings,
        }
    }

    fn launch_plan(
        &self,
        ctx: &LaunchContext<'_>,
        entry: &ApplicationEntry,
    ) -> Result<Vec<LaunchStep>, String> {
        let variant = Variant::from_app_id(&entry.app_id).unwrap_or(Variant::Stable);
        let target = self.launch_target(ctx, entry, variant)?;
        let step = |args: Vec<String>, title_hint: Option<String>| LaunchStep {
            request: LaunchRequest {
                target: target.clone(),
                args,
                working_directory: entry.launch.working_directory.clone(),
                env: Vec::new(),
            },
            expected_windows: 1,
            title_hint,
        };
        let projects = projects_to_open(&entry.state);
        if projects.is_empty() {
            return Ok(vec![step(entry.launch.arguments.clone(), None)]);
        }
        Ok(projects
            .into_iter()
            .map(|(path, title_hint)| {
                let mut args = Vec::with_capacity(entry.launch.arguments.len() + 2);
                args.push(NEW_WINDOW.to_string());
                args.extend(entry.launch.arguments.iter().cloned());
                args.push(path);
                step(args, Some(title_hint))
            })
            .collect())
    }

    fn instance_model(&self) -> InstanceModel {
        InstanceModel::SingleInstance
    }

    fn describe(&self, state: &AppState) -> Vec<StateDetail> {
        let AppState::Projects { projects } = state else {
            return Vec::new();
        };
        if projects.is_empty() {
            return Vec::new();
        }
        let names: Vec<String> = projects
            .iter()
            .map(|p| {
                if p.is_workspace_file {
                    format!("{} (workspace)", project_name(p))
                } else {
                    project_name(p)
                }
            })
            .collect();
        vec![StateDetail::new(
            if projects.len() == 1 {
                "Project"
            } else {
                "Projects"
            },
            names.join(", "),
        )]
    }

    fn locate_query(&self, app_id: &str) -> LocateQuery {
        let variant = Variant::from_app_id(app_id).unwrap_or(Variant::Stable);
        variant::locate_query(variant, OsKind::current())
    }
}

/// Saved projects that can be opened — they still exist, are absolute and valid UTF-8 —
/// without duplicates, as `(path argument, title hint)`.
fn projects_to_open(state: &AppState) -> Vec<(String, String)> {
    let AppState::Projects { projects } = state else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = Vec::new();
    for project in projects {
        let path = &project.path;
        let exists = if project.is_workspace_file {
            path.is_file()
        } else {
            path.is_dir()
        };
        if !exists || !path.is_absolute() {
            log::info!("Not reopening {}: it no longer exists", path.display());
            continue;
        }
        let Some(argument) = path.to_str() else {
            log::warn!(
                "Not reopening {}: the path is not valid Unicode",
                path.display()
            );
            continue;
        };
        if out.iter().any(|(a, _)| a == argument) {
            continue;
        }
        out.push((argument.to_string(), project_name(project)));
    }
    out
}

/// The name VS Code shows for a project: the folder name, or the workspace file name
/// without `.code-workspace`.
fn project_name(project: &ProjectRef) -> String {
    let kind = if project.is_workspace_file {
        RecentKind::Workspace
    } else {
        RecentKind::Folder
    };
    resolve::location_label(&project.path, kind, OsKind::current())
}

#[cfg(test)]
mod tests {
    use super::*;
    use reframe_platform::mock::MockPlatform;
    use reframe_platform::{Capability, Platform, Rect, WindowId, WindowState};
    use rusqlite::Connection;

    fn uri(path: &Path) -> String {
        url::Url::from_file_path(path)
            .expect("absolute path")
            .to_string()
    }

    fn main_process(exe: &str, command_line: &[&str]) -> ProcessInfo {
        ProcessInfo {
            pid: 4242,
            parent_pid: Some(1),
            name: "code".into(),
            product_name: None,
            executable_path: Some(exe.into()),
            command_line: command_line.iter().map(|s| s.to_string()).collect(),
            working_directory: None,
            start_time: None,
            package: None,
        }
    }

    fn window(id: &str, title: &str) -> WindowInfo {
        WindowInfo {
            id: WindowId::new(id),
            pid: 4242,
            title: title.into(),
            class_name: Some("Code".into()),
            app_id_hint: None,
            bounds: Rect::new(0, 0, 1200, 800),
            normal_bounds: None,
            state: WindowState::Normal,
            monitor_id: None,
            desktop: None,
            is_focused: false,
            z_order: 0,
        }
    }

    /// A VS Code user-data directory with the given recently-opened entries and storage.json.
    fn user_data_dir(
        root: &Path,
        entries: &[serde_json::Value],
        storage: serde_json::Value,
    ) -> PathBuf {
        let dir = root.join("Code");
        let global = dir.join("User").join("globalStorage");
        std::fs::create_dir_all(&global).unwrap();
        let conn = Connection::open(global.join("state.vscdb")).unwrap();
        conn.execute_batch(
            "CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
            rusqlite::params![
                storage::RECENTLY_OPENED_KEY,
                serde_json::json!({ "entries": entries }).to_string()
            ],
        )
        .unwrap();
        std::fs::write(global.join("storage.json"), storage.to_string()).unwrap();
        dir
    }

    fn capture(
        adapter: &VsCodeAdapter,
        platform: &MockPlatform,
        identity: &AppIdentity,
        main: &ProcessInfo,
        windows: &[WindowInfo],
    ) -> CaptureOutput {
        capture_with(adapter, platform, identity, &[main], windows)
    }

    fn capture_with(
        adapter: &VsCodeAdapter,
        platform: &MockPlatform,
        identity: &AppIdentity,
        processes: &[&ProcessInfo],
        windows: &[WindowInfo],
    ) -> CaptureOutput {
        let info = platform.info();
        let ctx = CaptureContext {
            platform,
            info: &info,
            browser_tab_capture: false,
        };
        let all: Vec<ProcessInfo> = processes.iter().map(|p| (*p).clone()).collect();
        let input = CaptureInput {
            identity,
            processes: processes.to_vec(),
            windows: windows.iter().collect(),
            all_processes: &all,
        };
        adapter.capture(&ctx, &input)
    }

    fn plan(platform: &MockPlatform, entry: &ApplicationEntry) -> Result<Vec<LaunchStep>, String> {
        let info = platform.info();
        let ctx = LaunchContext {
            platform,
            info: &info,
        };
        VsCodeAdapter::new().launch_plan(&ctx, entry)
    }

    fn project(path: &Path, is_workspace_file: bool) -> ProjectRef {
        ProjectRef {
            path: path.to_path_buf(),
            is_workspace_file,
            window_title: None,
        }
    }

    fn set_os(platform: &MockPlatform, os: OsKind) {
        let mut info = platform.info();
        info.os = os;
        platform.set_info(info);
    }

    #[test]
    fn adapter_identity_and_instance_model() {
        let adapter = VsCodeAdapter::new();
        assert_eq!(adapter.id(), "vscode");
        assert_eq!(adapter.name(), "Visual Studio Code");
        assert_eq!(adapter.instance_model(), InstanceModel::SingleInstance);
        let exe = match OsKind::current() {
            OsKind::Windows => r"C:\Program Files\Microsoft VS Code\Code.exe",
            OsKind::Macos => "/Applications/Visual Studio Code.app/Contents/MacOS/Electron",
            _ => "/usr/share/code/code",
        };
        assert_eq!(
            adapter.identify(&main_process(exe, &[]), &[]),
            Some(AppIdentity {
                app_id: "vscode".into(),
                display_name: "Visual Studio Code".into()
            })
        );
        assert_eq!(
            adapter.identify(&main_process("/usr/bin/gedit", &[]), &[]),
            None
        );
    }

    #[test]
    fn capture_resolves_windows_to_folders_and_workspaces() {
        let tmp = tempfile::tempdir().unwrap();
        let reframe = tmp.path().join("code").join("reframe");
        let api = tmp.path().join("work").join("api");
        let old_api = tmp.path().join("old").join("api"); // deleted since
        let notes = tmp.path().join("notes");
        for dir in [&reframe, &api, &notes] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let workspace = tmp.path().join("platform.code-workspace");
        std::fs::write(&workspace, "{\"folders\": []}").unwrap();
        let user_data = user_data_dir(
            tmp.path(),
            &[
                serde_json::json!({ "folderUri": uri(&old_api) }),
                serde_json::json!({ "folderUri": uri(&api) }),
                serde_json::json!({ "workspace": { "id": "1", "configPath": uri(&workspace) } }),
                serde_json::json!({ "folderUri": "vscode-remote://ssh-remote%2Bbox/srv/app" }),
            ],
            serde_json::json!({
                "windowsState": { "lastActiveWindow": { "folder": uri(&reframe) } },
                "userDataProfiles": [{ "location": "-1", "name": "Work" }]
            }),
        );
        let adapter = VsCodeAdapter::with_user_data_dirs(vec![user_data]);
        let platform = MockPlatform::new();
        let main = main_process(
            "/usr/share/code/code",
            &[
                "/usr/share/code/code",
                "--api-token=hunter2",
                notes.to_str().unwrap(),
            ],
        );
        let windows = vec![
            window("1", "main.rs - reframe - Visual Studio Code"),
            window("2", "\u{25CF} lib.rs - api - Work - Visual Studio Code"),
            window("3", "platform (Workspace) - Visual Studio Code"),
            window("4", "notes - Visual Studio Code"),
            window("5", "README.md - app [SSH: box] - Visual Studio Code"),
            window("6", "Untitled-1 - Visual Studio Code"),
            window("7", "Visual Studio Code"),
        ];
        let out = capture(
            &adapter,
            &platform,
            &Variant::Stable.identity(),
            &main,
            &windows,
        );
        let titled = |path: &Path, ws: bool, title: &str| ProjectRef {
            path: path.to_path_buf(),
            is_workspace_file: ws,
            window_title: Some(title.to_string()),
        };
        assert_eq!(
            out.state,
            AppState::Projects {
                projects: vec![
                    titled(&reframe, false, &windows[0].title),
                    titled(&api, false, &windows[1].title),
                    titled(&workspace, true, &windows[2].title),
                    // Resolved through the command line hint.
                    titled(&notes, false, &windows[3].title),
                ]
            }
        );
        // Raw command lines are never stored.
        assert_eq!(
            out.launch,
            LaunchConfig {
                executable: Some("/usr/share/code/code".into()),
                package: None,
                arguments: Vec::new(),
                working_directory: None,
            }
        );
        assert_eq!(out.warnings.len(), 3, "{:?}", out.warnings);
        assert!(out.warnings[0].contains("remote window (SSH: box)"));
        assert_eq!(
            out.warnings[1],
            "Couldn't tell which folder 'Untitled-1 - Visual Studio Code' shows; Reframe won't reopen that window."
        );
        assert_eq!(
            out.warnings[2],
            "'Visual Studio Code' has no folder open, so Reframe won't reopen that window."
        );
        assert!(!out.warnings.iter().any(|w| w.contains("hunter2")));
    }

    #[test]
    fn hints_come_from_every_instance_owning_the_windows() {
        let tmp = tempfile::tempdir().unwrap();
        let scratch = tmp.path().join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();
        let user_data = user_data_dir(tmp.path(), &[], serde_json::json!({}));
        let adapter = VsCodeAdapter::with_user_data_dirs(vec![user_data]);
        let platform = MockPlatform::new();
        let main = main_process("/usr/share/code/code", &["/usr/share/code/code"]);
        // A second instance with its own data directory, started with `code .` in `scratch`.
        let mut second = main_process(
            "/usr/share/code/code",
            &[
                "/usr/share/code/code",
                "--user-data-dir",
                "/tmp/other-profile",
                ".",
            ],
        );
        second.pid = 4343;
        second.working_directory = Some(scratch.clone());
        let windows = vec![window("1", "scratch - Visual Studio Code")];
        let out = capture_with(
            &adapter,
            &platform,
            &Variant::Stable.identity(),
            &[&main, &second],
            &windows,
        );
        assert_eq!(
            out.state,
            AppState::Projects {
                projects: vec![ProjectRef {
                    path: scratch,
                    is_workspace_file: false,
                    window_title: Some("scratch - Visual Studio Code".into()),
                }]
            }
        );
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        assert!(out.launch.arguments.is_empty());
    }

    #[test]
    fn unresolvable_windows_fall_back_to_vs_code_session_restore() {
        let tmp = tempfile::tempdir().unwrap();
        let user_data = user_data_dir(tmp.path(), &[], serde_json::json!({}));
        let adapter = VsCodeAdapter::with_user_data_dirs(vec![user_data]);
        let platform = MockPlatform::new();
        let main = main_process("/usr/share/code/code", &["/usr/share/code/code"]);
        let windows = vec![
            window("1", "main.rs - mystery - Visual Studio Code"),
            window(
                "2",
                "[Extension Development Host] index.ts - sample - Visual Studio Code",
            ),
            window("3", "Untitled (Workspace) - Visual Studio Code"),
            window("4", ""),
        ];
        let out = capture(
            &adapter,
            &platform,
            &Variant::Stable.identity(),
            &main,
            &windows,
        );
        assert_eq!(out.state, AppState::None);
        assert_eq!(out.warnings.len(), 4, "{:?}", out.warnings);
        assert!(out.warnings[0].contains("Extension Development Host"));
        assert!(out.warnings[1].contains("untitled workspace"));
        assert_eq!(
            out.warnings[2],
            "Couldn't tell which folder 'main.rs - mystery - Visual Studio Code' shows; VS Code will reopen its last session instead."
        );
        assert!(out.warnings[3].contains("title isn't readable"));
    }

    #[test]
    fn capture_reports_unreadable_state_and_missing_process() {
        let tmp = tempfile::tempdir().unwrap();
        let user_data = tmp.path().join("VSCodium");
        std::fs::create_dir_all(user_data.join("User").join("globalStorage")).unwrap();
        std::fs::write(
            user_data
                .join("User")
                .join("globalStorage")
                .join("state.vscdb"),
            "this is not a database",
        )
        .unwrap();
        let adapter = VsCodeAdapter::with_user_data_dirs(vec![user_data]);
        let platform = MockPlatform::new();
        let mut main = main_process("/usr/share/codium/codium", &[]);
        main.executable_path = None;
        let windows = vec![window("1", "x - lost - VSCodium")];
        let out = capture(
            &adapter,
            &platform,
            &Variant::Codium.identity(),
            &main,
            &windows,
        );
        assert_eq!(out.state, AppState::None);
        assert!(out.warnings[0].contains("executable path is not accessible"));
        assert!(out.warnings[1].ends_with("VSCodium will reopen its last session instead."));
        assert!(out.warnings[2].starts_with("Some of VSCodium's state couldn't be read"));

        let empty = VsCodeAdapter::with_user_data_dirs(vec![tmp.path().join("nothing")]);
        let out = capture(
            &empty,
            &platform,
            &Variant::Codium.identity(),
            &main_process("/usr/share/codium/codium", &[]),
            &windows,
        );
        assert!(out.warnings.last().unwrap().contains("wasn't found"));

        // No process at all.
        let info = platform.info();
        let ctx = CaptureContext {
            platform: &platform,
            info: &info,
            browser_tab_capture: false,
        };
        let identity = Variant::Stable.identity();
        let input = CaptureInput {
            identity: &identity,
            processes: Vec::new(),
            windows: Vec::new(),
            all_processes: &[],
        };
        let out = VsCodeAdapter::new().capture(&ctx, &input);
        assert_eq!(out.state, AppState::None);
        assert_eq!(out.warnings.len(), 1);
    }

    #[test]
    fn launch_plan_opens_each_existing_project_in_a_new_window() {
        let tmp = tempfile::tempdir().unwrap();
        let reframe = tmp.path().join("reframe");
        std::fs::create_dir_all(&reframe).unwrap();
        let workspace = tmp.path().join("platform.code-workspace");
        std::fs::write(&workspace, "{}").unwrap();
        let executable = std::env::current_exe().unwrap();

        let mut entry = ApplicationEntry::new("vscode", "Visual Studio Code", "vscode");
        entry.launch.executable = Some(executable.clone());
        entry.launch.arguments = vec!["--profile".into(), "Work".into()];
        entry.state = AppState::Projects {
            projects: vec![
                project(&reframe, false),
                project(&tmp.path().join("deleted"), false),
                project(&workspace, true),
                project(&reframe, false),
                // A folder saved as a workspace file (or vice versa) is not opened.
                project(&reframe, true),
            ],
        };
        let platform = MockPlatform::new();
        let steps = plan(&platform, &entry).unwrap();
        assert_eq!(steps.len(), 2);
        let target = LaunchTarget::Executable { path: executable };
        assert_eq!(steps[0].request.target, target);
        assert_eq!(
            steps[0].request.args,
            vec![
                "--new-window".to_string(),
                "--profile".into(),
                "Work".into(),
                reframe.to_str().unwrap().into()
            ]
        );
        assert_eq!(steps[0].title_hint.as_deref(), Some("reframe"));
        assert_eq!(steps[0].expected_windows, 1);
        assert_eq!(
            steps[1].request.args.last().map(String::as_str),
            workspace.to_str()
        );
        assert_eq!(steps[1].title_hint.as_deref(), Some("platform"));
        assert!(steps.iter().all(|s| s.request.env.is_empty()));
    }

    #[test]
    fn launch_plan_without_projects_starts_vs_code_plainly() {
        let mut entry =
            ApplicationEntry::new("vscode-insiders", "Visual Studio Code - Insiders", "vscode");
        entry.launch.executable = Some(std::env::current_exe().unwrap());
        let platform = MockPlatform::new();
        for state in [
            AppState::None,
            AppState::Projects {
                projects: Vec::new(),
            },
            AppState::Projects {
                projects: vec![project(Path::new("/definitely/missing/folder"), false)],
            },
        ] {
            entry.state = state;
            let steps = plan(&platform, &entry).unwrap();
            assert_eq!(steps.len(), 1);
            assert!(steps[0].request.args.is_empty());
            assert_eq!(steps[0].title_hint, None);
            assert_eq!(steps[0].expected_windows, 1);
        }
    }

    #[test]
    fn launch_plan_uses_the_bundle_on_macos() {
        let tmp = tempfile::tempdir().unwrap();
        let reframe = tmp.path().join("reframe");
        std::fs::create_dir_all(&reframe).unwrap();
        let platform = MockPlatform::new();
        set_os(&platform, OsKind::Macos);

        let bundle = PackageInfo {
            kind: PackageKind::MacBundle,
            id: "com.microsoft.VSCode".into(),
            path: Some("/Applications/Visual Studio Code.app".into()),
        };
        let mut entry = ApplicationEntry::new("vscode", "Visual Studio Code", "vscode");
        entry.launch.package = Some(bundle.clone());
        entry.launch.executable =
            Some("/Applications/Visual Studio Code.app/Contents/MacOS/Electron".into());
        entry.state = AppState::Projects {
            projects: vec![project(&reframe, false)],
        };
        let steps = plan(&platform, &entry).unwrap();
        assert_eq!(
            steps[0].request.target,
            LaunchTarget::Package {
                package: bundle.clone()
            }
        );
        assert_eq!(
            steps[0].request.args,
            vec!["--new-window".to_string(), reframe.to_str().unwrap().into()]
        );

        // Only the executable is known: launch its bundle anyway.
        entry.launch.package = None;
        entry.app_id = "vscodium".into();
        entry.launch.executable =
            Some("/Users/me/Apps/VSCodium.app/Contents/MacOS/Electron".into());
        let steps = plan(&platform, &entry).unwrap();
        assert_eq!(
            steps[0].request.target,
            LaunchTarget::Package {
                package: PackageInfo {
                    kind: PackageKind::MacBundle,
                    id: "com.vscodium".into(),
                    path: Some("/Users/me/Apps/VSCodium.app".into()),
                }
            }
        );
    }

    #[test]
    fn launch_plan_errors_and_relocation() {
        let platform = MockPlatform::new();
        let mut entry = ApplicationEntry::new("vscode", "Visual Studio Code", "vscode");
        let err = plan(&platform, &entry).unwrap_err();
        assert!(
            err.contains("No executable is configured for Visual Studio Code"),
            "{err}"
        );

        // The configured executable is gone and VS Code can't be found.
        entry.launch.executable = Some("/definitely/missing/code".into());
        let err = plan(&platform, &entry).unwrap_err();
        assert!(
            err.contains("no longer installed at /definitely/missing/code"),
            "{err}"
        );

        // ...but it is installed elsewhere.
        let relocated = LaunchTarget::Executable {
            path: std::env::current_exe().unwrap(),
        };
        let name =
            variant::locate_query(Variant::Stable, OsKind::current()).executable_names[0].clone();
        platform.install(&name, relocated.clone());
        let steps = plan(&platform, &entry).unwrap();
        assert_eq!(steps[0].request.target, relocated);

        // Flatpak / Snap packages launch through the package.
        let flatpak = PackageInfo {
            kind: PackageKind::Flatpak,
            id: "com.visualstudio.code".into(),
            path: None,
        };
        entry.launch.package = Some(flatpak.clone());
        let steps = plan(&platform, &entry).unwrap();
        assert_eq!(
            steps[0].request.target,
            LaunchTarget::Package { package: flatpak }
        );
    }

    #[test]
    fn capability_reflects_the_platform() {
        let platform = MockPlatform::new();
        let mut info = platform.info();
        let adapter = VsCodeAdapter::new();
        let cap = adapter.capability(&info);
        assert_eq!(cap.level, CapabilityLevel::Partial);
        let feature = |cap: &AdapterCapability, name: &str| {
            cap.features
                .iter()
                .find(|f| f.name == name)
                .cloned()
                .unwrap_or_else(|| panic!("missing feature {name}"))
        };
        assert!(feature(&cap, "Reopen project folders and workspaces").supported);
        let editors = feature(&cap, "Open editors, terminals and unsaved changes");
        assert!(editors.supported);
        assert_eq!(
            editors.note.as_deref(),
            Some("Restored by VS Code itself (hot exit / window.restoreWindows), not by Reframe")
        );
        assert!(!feature(&cap, "Remote (SSH/WSL/containers) windows").supported);
        let placement = feature(&cap, "Window position and size");
        assert!(placement.supported && placement.note.is_none());

        info.capabilities.move_windows = Capability::unsupported("Wayland does not allow it.");
        let placement = feature(&adapter.capability(&info), "Window position and size");
        assert!(!placement.supported);
        assert_eq!(
            placement.note.as_deref(),
            Some("Wayland does not allow it.")
        );
    }

    #[test]
    fn describe_lists_project_names() {
        let adapter = VsCodeAdapter::new();
        assert!(adapter.describe(&AppState::None).is_empty());
        assert!(
            adapter
                .describe(&AppState::Projects {
                    projects: Vec::new()
                })
                .is_empty()
        );
        let one = AppState::Projects {
            projects: vec![project(Path::new("/home/me/reframe"), false)],
        };
        assert_eq!(
            adapter.describe(&one),
            vec![StateDetail::new("Project", "reframe")]
        );
        let many = AppState::Projects {
            projects: vec![
                project(Path::new("/home/me/reframe"), false),
                project(Path::new("/home/me/platform.code-workspace"), true),
                project(Path::new("/srv/api"), false),
            ],
        };
        assert_eq!(
            adapter.describe(&many),
            vec![StateDetail::new(
                "Projects",
                "reframe, platform (workspace), api"
            )]
        );
    }

    #[test]
    fn locate_query_per_app_id() {
        let adapter = VsCodeAdapter::new();
        let stable = adapter.locate_query("vscode");
        assert_eq!(stable.bundle_ids, vec!["com.microsoft.VSCode".to_string()]);
        assert!(stable.desktop_ids.contains(&"code.desktop".to_string()));
        assert_eq!(stable.display_names, vec!["Visual Studio Code".to_string()]);
        let insiders = adapter.locate_query("vscode-insiders");
        assert_eq!(
            insiders.bundle_ids,
            vec!["com.microsoft.VSCodeInsiders".to_string()]
        );
        let codium = adapter.locate_query("vscodium");
        assert!(codium.desktop_ids.contains(&"codium.desktop".to_string()));
        // Unknown ids fall back to the stable release.
        assert_eq!(adapter.locate_query("something-else"), stable);
    }
}
