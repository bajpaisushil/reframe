//! File managers (File Explorer, Finder, Nautilus, Dolphin, Thunar, Nemo, Caja, PCManFM,
//! PCManFM-Qt): reopen the folders that were open, one window per folder.
//!
//! How folders are captured depends on the platform:
//!
//! | OS | Source | Limits |
//! |----|--------|--------|
//! | Windows | Explorer's shell window list (COM `ShellWindows` → window handle + location URL), matched to the selected `CabinetWClass` windows by HWND; full-path titles as a fallback ([`explorer`]) | Special locations (This PC, Recycle Bin, libraries, search results) are skipped; Windows 11 tabs reopen as separate windows |
//! | macOS | `osascript`: Finder's window names and target folders, matched by name ([`finder`]) | Needs the Automation permission for Finder; only the front tab of each window |
//! | Linux | Window titles: full paths, or folder names looked up in the home folder, XDG user folders and mounted volumes ([`linux`]) | Ambiguous or unknown names are skipped with a warning; only the visible tab |
//!
//! Restoring opens every folder that still exists in a new window: `explorer.exe <folder>`,
//! `open -a Finder <folder>`, or the captured Linux file manager with its new-window flag
//! (`nautilus --new-window`, `dolphin --new-window`, `pcmanfm --new-win`...), falling back to
//! the desktop's default handler (`xdg-open`) when no file manager executable is known. View
//! settings, selection and navigation history are never restored.

mod explorer;
mod finder;
mod linux;
pub(in crate::adapters) mod support;

use std::path::{Path, PathBuf};

use reframe_platform::{
    LaunchRequest, LaunchTarget, LocateQuery, OsKind, PlatformInfo, ProcessInfo, WindowInfo,
};

use self::support::KnownApp;
use super::generic::{self, GenericAdapter};
use super::{
    AppAdapter, AppIdentity, CaptureContext, CaptureInput, CaptureOutput, InstanceModel,
    LaunchContext, LaunchStep,
};
use crate::model::{
    AdapterCapability, AppState, ApplicationEntry, CapabilityLevel, FolderRef, StateDetail,
};

/// At most this many folders are captured or reopened per application.
const MAX_FOLDERS: usize = 50;

/// macOS `open`, used to show folders in Finder.
const MACOS_OPEN: &str = "/usr/bin/open";

const APPS: &[KnownApp] = &[
    KnownApp::new("explorer", "File Explorer")
        .aliases(&["Windows Explorer"])
        .executables(&["explorer"])
        .locate_names(&["explorer.exe"]),
    KnownApp::new("finder", "Finder")
        .executable_paths(&["Finder.app/Contents/MacOS/Finder"])
        .bundle_ids(&["com.apple.finder"]),
    KnownApp::new("nautilus", "Files")
        .aliases(&["Nautilus", "GNOME Files"])
        .executables(&["nautilus"])
        .locate_names(&["nautilus"])
        .flatpak_ids(&["org.gnome.Nautilus"])
        .window_classes(&["org.gnome.Nautilus", "nautilus"])
        .desktop_ids(&["org.gnome.Nautilus.desktop", "nautilus.desktop"]),
    KnownApp::new("dolphin", "Dolphin")
        .executables(&["dolphin"])
        .locate_names(&["dolphin"])
        .flatpak_ids(&["org.kde.dolphin"])
        .window_classes(&["dolphin", "org.kde.dolphin"])
        .desktop_ids(&["org.kde.dolphin.desktop", "dolphin.desktop"]),
    KnownApp::new("thunar", "Thunar")
        .executables(&["thunar"])
        .locate_names(&["thunar", "Thunar"])
        .flatpak_ids(&["org.xfce.Thunar"])
        .window_classes(&["thunar", "org.xfce.Thunar"])
        .desktop_ids(&[
            "thunar.desktop",
            "Thunar.desktop",
            "org.xfce.Thunar.desktop",
        ]),
    KnownApp::new("nemo", "Nemo")
        .executables(&["nemo"])
        .locate_names(&["nemo"])
        .window_classes(&["nemo"])
        .desktop_ids(&["nemo.desktop"]),
    KnownApp::new("caja", "Caja")
        .executables(&["caja"])
        .locate_names(&["caja"])
        .window_classes(&["caja"])
        .desktop_ids(&["caja-browser.desktop", "caja.desktop"]),
    KnownApp::new("pcmanfm", "PCManFM")
        .executables(&["pcmanfm"])
        .locate_names(&["pcmanfm"])
        .window_classes(&["pcmanfm"])
        .desktop_ids(&["pcmanfm.desktop"]),
    KnownApp::new("pcmanfm-qt", "PCManFM-Qt")
        .executables(&["pcmanfm-qt"])
        .locate_names(&["pcmanfm-qt"])
        .window_classes(&["pcmanfm-qt"])
        .desktop_ids(&["pcmanfm-qt.desktop"]),
];

/// Flags that make a Linux file manager open the folder in a new window rather than a tab of
/// an existing one (Nemo, Thunar and Caja open a new window by default).
fn new_window_flags(app_id: &str) -> &'static [&'static str] {
    match app_id {
        "nautilus" | "dolphin" | "pcmanfm-qt" => &["--new-window"],
        "pcmanfm" => &["--new-win"],
        _ => &[],
    }
}

#[derive(Debug)]
pub struct FileManagerAdapter {
    generic: GenericAdapter,
    /// Where folder names in Linux window titles are looked up.
    search: linux::FolderSearch,
}

impl FileManagerAdapter {
    pub fn new() -> Self {
        Self {
            generic: GenericAdapter::new(),
            search: linux::FolderSearch::from_environment(),
        }
    }

    #[cfg(test)]
    fn with_search(search: linux::FolderSearch) -> Self {
        Self {
            generic: GenericAdapter::new(),
            search,
        }
    }
}

impl Default for FileManagerAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl AppAdapter for FileManagerAdapter {
    fn id(&self) -> &'static str {
        "file-manager"
    }

    fn name(&self) -> &'static str {
        "File managers"
    }

    fn identify(&self, process: &ProcessInfo, windows: &[&WindowInfo]) -> Option<AppIdentity> {
        let app = support::find_app(APPS, process, windows)?;
        // explorer.exe also owns the taskbar, the desktop and other shell UI: only a process
        // with Explorer folder windows is a file manager.
        if app.id == "explorer" && !windows.iter().any(|w| explorer::is_folder_window(w)) {
            return None;
        }
        Some(app.identity())
    }

    fn capability(&self, info: &PlatformInfo) -> AdapterCapability {
        let (level, folders_note, tabs): (CapabilityLevel, &str, (&str, bool, &str)) = match info.os
        {
            OsKind::Windows => (
                CapabilityLevel::Full,
                "Read from File Explorer's list of open windows. Special locations such as This PC \
                 or the Recycle Bin are skipped.",
                ("Explorer tabs", true, "Each tab reopens as its own window."),
            ),
            OsKind::Macos => (
                CapabilityLevel::Partial,
                "Reframe asks Finder for its windows; macOS asks once for permission to control \
                 Finder (Automation).",
                (
                    "Finder tabs",
                    false,
                    "Only the front tab of each window is captured.",
                ),
            ),
            OsKind::Linux => (
                CapabilityLevel::Partial,
                "Recognised from window titles. Show the full path in the file manager's title \
                 bar for reliable results; otherwise folder names are looked up in your home \
                 folder, XDG user folders and mounted volumes and skipped when ambiguous.",
                (
                    "File manager tabs",
                    false,
                    "Only the visible tab of each window is captured.",
                ),
            ),
            OsKind::Other => {
                return support::with_window_placement(
                    AdapterCapability::launch_only()
                        .feature("Launch application", true, None)
                        .feature(
                            "Reopen folders",
                            false,
                            Some("Not supported on this platform."),
                        ),
                    info,
                );
            }
        };
        let capability = AdapterCapability {
            level,
            features: Vec::new(),
        }
        .feature("Launch application", true, None)
        .feature("Reopen folders", true, Some(folders_note))
        .feature(tabs.0, tabs.1, Some(tabs.2));
        support::with_window_placement(capability, info).feature(
            "View settings, selection and history",
            false,
            Some("Restored by the file manager itself, if at all."),
        )
    }

    fn capture(&self, _ctx: &CaptureContext<'_>, input: &CaptureInput<'_>) -> CaptureOutput {
        let (launch, mut warnings) = support::launch_config(input);
        let (mut folders, folder_warnings) = match input.identity.app_id.as_str() {
            "explorer" => {
                let windows: Vec<&WindowInfo> = input
                    .windows
                    .iter()
                    .copied()
                    .filter(|w| explorer::is_folder_window(w))
                    .collect();
                explorer::capture(&windows)
            }
            "finder" => finder::capture(&input.windows),
            _ => linux::capture(&input.windows, &self.search),
        };
        let name = &input.identity.display_name;
        warnings.extend(folder_warnings.into_iter().map(|w| format!("{name}: {w}")));
        if folders.len() > MAX_FOLDERS {
            warnings.push(format!(
                "{name}: only the first {MAX_FOLDERS} of {} folders were captured.",
                folders.len()
            ));
            folders.truncate(MAX_FOLDERS);
        }
        CaptureOutput {
            state: if folders.is_empty() {
                AppState::None
            } else {
                AppState::Folders { folders }
            },
            launch,
            warnings,
        }
    }

    fn launch_plan(
        &self,
        ctx: &LaunchContext<'_>,
        entry: &ApplicationEntry,
    ) -> Result<Vec<LaunchStep>, String> {
        let folders: Vec<&FolderRef> = match &entry.state {
            AppState::Folders { folders } => folders
                .iter()
                .filter(|f| f.path.is_absolute() && f.path.is_dir())
                .take(MAX_FOLDERS)
                .collect(),
            _ => Vec::new(),
        };
        if folders.is_empty() {
            // Nothing left to reopen: start the file manager on its own.
            return Ok(vec![LaunchStep {
                request: folder_request(ctx.info.os, entry, None)?,
                expected_windows: 1,
                title_hint: None,
            }]);
        }
        folders
            .into_iter()
            .map(|folder| {
                Ok(LaunchStep {
                    request: folder_request(ctx.info.os, entry, Some(&folder.path))?,
                    expected_windows: 1,
                    title_hint: Some(support::display_name(&folder.path)),
                })
            })
            .collect()
    }

    fn instance_model(&self) -> InstanceModel {
        // explorer.exe, Finder, Nautilus, Dolphin... hand new windows to a running instance.
        InstanceModel::SingleInstance
    }

    fn describe(&self, state: &AppState) -> Vec<StateDetail> {
        match state {
            AppState::Folders { folders } if !folders.is_empty() => vec![StateDetail::new(
                if folders.len() == 1 {
                    "Folder"
                } else {
                    "Folders"
                },
                support::summarize(folders.iter().map(|f| support::display_name(&f.path)), 4),
            )],
            _ => self.generic.describe(state),
        }
    }

    fn locate_query(&self, app_id: &str) -> LocateQuery {
        support::app_by_id(APPS, app_id)
            .map(KnownApp::locate_query)
            .unwrap_or_else(|| self.generic.locate_query(app_id))
    }
}

/// How to open `folder` (or just the file manager) on `os`.
fn folder_request(
    os: OsKind,
    entry: &ApplicationEntry,
    folder: Option<&Path>,
) -> Result<LaunchRequest, String> {
    let user_args = entry.launch.arguments.iter().cloned();
    let request = |target: LaunchTarget, args: Vec<String>| LaunchRequest {
        // A configured working directory belongs to the file manager's own executable, not to
        // the shell / `open` helpers used otherwise.
        working_directory: match &target {
            LaunchTarget::Open { .. } => None,
            LaunchTarget::Executable { path } if path == Path::new(MACOS_OPEN) => None,
            _ => entry.launch.working_directory.clone(),
        },
        target,
        args,
        env: Vec::new(),
    };
    match os {
        OsKind::Windows => {
            let explorer = entry
                .launch
                .executable
                .clone()
                .filter(|exe| support::executable_stem(exe) == "explorer")
                .unwrap_or_else(|| PathBuf::from("explorer.exe"));
            Ok(match folder {
                // explorer.exe treats commas in its command line as switch separators; let the
                // shell open such folders instead.
                Some(folder) if folder.to_string_lossy().contains(',') => request(
                    LaunchTarget::Open {
                        target: folder.display().to_string(),
                    },
                    Vec::new(),
                ),
                Some(folder) => request(
                    LaunchTarget::Executable { path: explorer },
                    user_args
                        .chain(std::iter::once(folder.display().to_string()))
                        .collect(),
                ),
                None => request(
                    LaunchTarget::Executable { path: explorer },
                    user_args.collect(),
                ),
            })
        }
        OsKind::Macos => {
            // Always Finder (the captured application), even if another app handles folders.
            let mut args = vec!["-a".to_string(), "Finder".to_string()];
            args.extend(folder.map(|f| f.display().to_string()));
            Ok(request(
                LaunchTarget::Executable {
                    path: PathBuf::from(MACOS_OPEN),
                },
                args,
            ))
        }
        OsKind::Linux | OsKind::Other => match (generic::launch_target(&entry.launch), folder) {
            (Some(target), Some(folder)) => Ok(request(
                target,
                user_args
                    .chain(
                        new_window_flags(&entry.app_id)
                            .iter()
                            .map(|f| f.to_string()),
                    )
                    .chain(std::iter::once(folder.display().to_string()))
                    .collect(),
            )),
            (Some(target), None) => Ok(request(target, user_args.collect())),
            // No file manager executable known: the desktop's default folder handler.
            (None, Some(folder)) => Ok(request(
                LaunchTarget::Open {
                    target: folder.display().to_string(),
                },
                Vec::new(),
            )),
            (None, None) => Err(format!(
                "No executable is configured for {}.",
                entry.display_name
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::support::testing::{platform, process, window};
    use super::*;
    use reframe_platform::{PackageInfo, PackageKind, Platform};

    fn identify(p: &ProcessInfo, windows: &[&WindowInfo]) -> Option<String> {
        FileManagerAdapter::new()
            .identify(p, windows)
            .map(|id| id.app_id)
    }

    #[test]
    fn identifies_linux_file_managers() {
        let w = window("1", 1, "Downloads", None);
        assert_eq!(
            identify(&process(1, "nautilus", Some("/usr/bin/nautilus")), &[&w]).as_deref(),
            Some("nautilus")
        );
        assert_eq!(
            identify(
                &process(1, "pcmanfm-qt", Some("/usr/bin/pcmanfm-qt")),
                &[&w]
            )
            .as_deref(),
            Some("pcmanfm-qt")
        );
        let mut flatpak = process(1, "dolphin", Some("/app/bin/dolphin"));
        flatpak.package = Some(PackageInfo {
            kind: PackageKind::Flatpak,
            id: "org.kde.dolphin".into(),
            path: None,
        });
        assert_eq!(identify(&flatpak, &[&w]).as_deref(), Some("dolphin"));
        let by_class = window("1", 1, "Downloads", Some("Thunar"));
        assert_eq!(
            identify(&process(1, "unknown", None), &[&by_class]).as_deref(),
            Some("thunar")
        );
        assert_eq!(
            identify(
                &process(1, "nemo-desktop", Some("/usr/bin/nemo-desktop")),
                &[]
            ),
            None
        );
        assert_eq!(
            identify(&process(1, "gedit", Some("/usr/bin/gedit")), &[&w]),
            None
        );
    }

    #[test]
    fn explorer_needs_a_folder_window() {
        let explorer = process(1, "explorer.exe", Some(r"C:\Windows\explorer.exe"));
        let taskbar = window("0x10", 1, "", Some("Shell_TrayWnd"));
        let folder = window("0x20", 1, "Downloads", Some("CabinetWClass"));
        assert_eq!(identify(&explorer, &[&taskbar]), None);
        assert_eq!(identify(&explorer, &[]), None);
        let id = FileManagerAdapter::new()
            .identify(&explorer, &[&taskbar, &folder])
            .unwrap();
        assert_eq!(id.app_id, "explorer");
        assert_eq!(id.display_name, "File Explorer");
    }

    #[test]
    fn identifies_finder_but_not_other_finders() {
        let mut finder = process(1, "Finder", None);
        finder.package = Some(PackageInfo {
            kind: PackageKind::MacBundle,
            id: "com.apple.finder".into(),
            path: None,
        });
        assert_eq!(identify(&finder, &[]).as_deref(), Some("finder"));
        let by_path = process(
            1,
            "Finder",
            Some("/System/Library/CoreServices/Finder.app/Contents/MacOS/Finder"),
        );
        assert_eq!(identify(&by_path, &[]).as_deref(), Some("finder"));
        assert_eq!(
            identify(&process(1, "finder", Some("/usr/bin/finder")), &[]),
            None
        );
    }

    #[test]
    fn capability_depends_on_the_os() {
        let adapter = FileManagerAdapter::new();
        for (os, level) in [
            (OsKind::Windows, CapabilityLevel::Full),
            (OsKind::Macos, CapabilityLevel::Partial),
            (OsKind::Linux, CapabilityLevel::Partial),
            (OsKind::Other, CapabilityLevel::LaunchOnly),
        ] {
            let (_platform, info) = platform(os);
            let capability = adapter.capability(&info);
            assert_eq!(capability.level, level, "{os:?}");
            assert!(
                capability
                    .features
                    .iter()
                    .any(|f| f.name == "Reopen folders")
            );
            assert!(
                capability
                    .features
                    .iter()
                    .any(|f| f.name == "Window position and size")
            );
            assert_eq!(adapter.capability_for_app("nautilus", &info), capability);
        }
    }

    struct Folders {
        _dir: tempfile::TempDir,
        a: PathBuf,
        b: PathBuf,
        comma: PathBuf,
        missing: PathBuf,
    }

    fn folders() -> Folders {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("alpha");
        let b = dir.path().join("beta");
        let comma = dir.path().join("x,y");
        for d in [&a, &b, &comma] {
            std::fs::create_dir(d).unwrap();
        }
        let missing = dir.path().join("gone");
        Folders {
            _dir: dir,
            a,
            b,
            comma,
            missing,
        }
    }

    fn entry(app_id: &str, folders: &[&Path]) -> ApplicationEntry {
        let mut entry = ApplicationEntry::new(app_id, app_id, "file-manager");
        entry.state = AppState::Folders {
            folders: folders
                .iter()
                .map(|p| FolderRef {
                    path: p.to_path_buf(),
                    window_title: None,
                })
                .collect(),
        };
        entry
    }

    fn plan(os: OsKind, entry: &ApplicationEntry) -> Result<Vec<LaunchStep>, String> {
        let (platform, info) = platform(os);
        let ctx = LaunchContext {
            platform: &platform,
            info: &info,
        };
        FileManagerAdapter::new().launch_plan(&ctx, entry)
    }

    #[test]
    fn windows_plan_uses_explorer_per_existing_folder() {
        let f = folders();
        let mut e = entry("explorer", &[&f.a, &f.missing, &f.comma]);
        e.launch.executable = Some(PathBuf::from(r"C:\Windows\explorer.exe"));
        let steps = plan(OsKind::Windows, &e).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(
            steps[0].request.target,
            LaunchTarget::Executable {
                path: PathBuf::from(r"C:\Windows\explorer.exe")
            }
        );
        assert_eq!(steps[0].request.args, vec![f.a.display().to_string()]);
        assert_eq!(steps[0].title_hint.as_deref(), Some("alpha"));
        assert_eq!(steps[0].expected_windows, 1);
        assert_eq!(
            steps[1].request.target,
            LaunchTarget::Open {
                target: f.comma.display().to_string()
            }
        );
        assert!(steps[1].request.args.is_empty());

        // Without a captured executable, explorer.exe is found by the OS.
        let steps = plan(OsKind::Windows, &entry("explorer", &[&f.b])).unwrap();
        assert_eq!(
            steps[0].request.target,
            LaunchTarget::Executable {
                path: PathBuf::from("explorer.exe")
            }
        );
    }

    #[test]
    fn macos_plan_opens_folders_in_finder() {
        let f = folders();
        let steps = plan(OsKind::Macos, &entry("finder", &[&f.a, &f.b])).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(
            steps[1].request.target,
            LaunchTarget::Executable {
                path: PathBuf::from("/usr/bin/open")
            }
        );
        assert_eq!(
            steps[1].request.args,
            vec!["-a".to_string(), "Finder".into(), f.b.display().to_string()]
        );
        assert_eq!(steps[1].title_hint.as_deref(), Some("beta"));
    }

    #[test]
    fn working_directory_only_applies_to_the_file_manager_itself() {
        let f = folders();
        let mut e = entry("nautilus", &[&f.a]);
        e.launch.working_directory = Some(f.b.clone());
        // No executable: xdg-open, without the working directory.
        assert_eq!(
            plan(OsKind::Linux, &e).unwrap()[0]
                .request
                .working_directory,
            None
        );
        assert_eq!(
            plan(OsKind::Macos, &e).unwrap()[0]
                .request
                .working_directory,
            None
        );
        e.launch.executable = Some(PathBuf::from("/usr/bin/nautilus"));
        assert_eq!(
            plan(OsKind::Linux, &e).unwrap()[0]
                .request
                .working_directory,
            Some(f.b.clone())
        );
    }

    #[test]
    fn linux_plan_uses_the_file_manager_with_its_new_window_flag() {
        let f = folders();
        for (app, flags) in [
            ("nautilus", vec!["--new-window"]),
            ("dolphin", vec!["--new-window"]),
            ("pcmanfm", vec!["--new-win"]),
            ("pcmanfm-qt", vec!["--new-window"]),
            ("thunar", vec![]),
            ("nemo", vec![]),
            ("caja", vec![]),
        ] {
            let mut e = entry(app, &[&f.a]);
            e.launch.executable = Some(PathBuf::from(format!("/usr/bin/{app}")));
            let steps = plan(OsKind::Linux, &e).unwrap();
            let mut expected: Vec<String> = flags.iter().map(|s| s.to_string()).collect();
            expected.push(f.a.display().to_string());
            assert_eq!(steps[0].request.args, expected, "{app}");
        }

        // Flatpak file manager: the package target carries the arguments.
        let mut e = entry("dolphin", &[&f.a]);
        let package = PackageInfo {
            kind: PackageKind::Flatpak,
            id: "org.kde.dolphin".into(),
            path: None,
        };
        e.launch.package = Some(package.clone());
        let steps = plan(OsKind::Linux, &e).unwrap();
        assert_eq!(steps[0].request.target, LaunchTarget::Package { package });

        // No executable known: xdg-open each folder.
        let steps = plan(OsKind::Linux, &entry("nautilus", &[&f.a, &f.b])).unwrap();
        assert_eq!(
            steps[1].request.target,
            LaunchTarget::Open {
                target: f.b.display().to_string()
            }
        );
    }

    #[test]
    fn plain_launch_when_no_folder_is_left() {
        let f = folders();
        let mut e = entry("nautilus", &[&f.missing]);
        assert!(plan(OsKind::Linux, &e).is_err());
        e.launch.executable = Some(PathBuf::from("/usr/bin/nautilus"));
        e.launch.arguments = vec!["--user-flag".into()];
        let steps = plan(OsKind::Linux, &e).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].request.args, vec!["--user-flag".to_string()]);
        assert_eq!(steps[0].title_hint, None);

        let steps = plan(OsKind::Windows, &entry("explorer", &[])).unwrap();
        assert!(steps[0].request.args.is_empty());
        let steps = plan(OsKind::Macos, &entry("finder", &[])).unwrap();
        assert_eq!(
            steps[0].request.args,
            vec!["-a".to_string(), "Finder".into()]
        );
    }

    #[test]
    fn capture_resolves_linux_titles_end_to_end() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("Projects/reframe")).unwrap();
        let adapter = FileManagerAdapter::with_search(linux::FolderSearch {
            home: Some(home.path().to_path_buf()),
            ..Default::default()
        });
        let (platform, info) = platform(OsKind::Linux);
        let ctx = CaptureContext {
            platform: &platform,
            info: &info,
            browser_tab_capture: false,
        };
        let p = process(5, "dolphin", Some("/usr/bin/dolphin"));
        let w1 = window("1", 5, "reframe — Dolphin", Some("dolphin"));
        let w2 = window("2", 5, "Nowhere — Dolphin", Some("dolphin"));
        let identity = adapter.identify(&p, &[&w1, &w2]).unwrap();
        let all = vec![p.clone()];
        let input = CaptureInput {
            identity: &identity,
            processes: vec![&p],
            windows: vec![&w1, &w2],
            all_processes: &all,
        };
        let out = adapter.capture(&ctx, &input);
        assert_eq!(
            out.state,
            AppState::Folders {
                folders: vec![FolderRef {
                    path: std::fs::canonicalize(home.path().join("Projects/reframe")).unwrap(),
                    window_title: Some("reframe — Dolphin".into()),
                }]
            }
        );
        assert_eq!(
            out.launch.executable,
            Some(PathBuf::from("/usr/bin/dolphin"))
        );
        assert!(out.launch.arguments.is_empty());
        assert_eq!(out.warnings.len(), 1);
        assert!(out.warnings[0].starts_with("Dolphin: could not find the folder \"Nowhere\""));
    }

    #[test]
    fn capture_without_windows_or_process_degrades_gracefully() {
        let adapter = FileManagerAdapter::new();
        let (platform, info) = platform(OsKind::Linux);
        let ctx = CaptureContext {
            platform: &platform,
            info: &info,
            browser_tab_capture: false,
        };
        let identity = AppIdentity {
            app_id: "nautilus".into(),
            display_name: "Files".into(),
        };
        let input = CaptureInput {
            identity: &identity,
            processes: Vec::new(),
            windows: Vec::new(),
            all_processes: &[],
        };
        let out = adapter.capture(&ctx, &input);
        assert_eq!(out.state, AppState::None);
        assert_eq!(
            out.warnings,
            vec!["Files: no process information available.".to_string()]
        );
        assert_eq!(platform.processes().unwrap().len(), 0);
    }

    #[test]
    fn describes_folders() {
        let adapter = FileManagerAdapter::new();
        let state = AppState::Folders {
            folders: ["/home/a/Downloads", r"C:\Users\me\Work", "/"]
                .iter()
                .map(|p| FolderRef {
                    path: PathBuf::from(p),
                    window_title: None,
                })
                .collect(),
        };
        assert_eq!(
            adapter.describe(&state),
            vec![StateDetail::new("Folders", "Downloads, Work, /")]
        );
        assert!(adapter.describe(&AppState::None).is_empty());
    }

    #[test]
    fn locate_queries_per_app() {
        let adapter = FileManagerAdapter::new();
        let q = adapter.locate_query("nautilus");
        assert_eq!(q.executable_names, vec!["nautilus".to_string()]);
        assert!(
            q.desktop_ids
                .contains(&"org.gnome.Nautilus.desktop".to_string())
        );
        assert_eq!(
            adapter.locate_query("finder").bundle_ids,
            vec!["com.apple.finder".to_string()]
        );
        assert_eq!(
            adapter.locate_query("explorer").executable_names,
            vec!["explorer.exe".to_string()]
        );
        // Unknown ids fall back to the generic rules.
        assert_eq!(
            adapter.locate_query("exe:foo").executable_names[0],
            "foo".to_string()
        );
        assert_eq!(adapter.instance_model(), InstanceModel::SingleInstance);
    }
}
