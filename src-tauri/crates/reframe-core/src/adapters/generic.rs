//! Fallback adapter for any application without a dedicated integration: launches the
//! executable (plus documents it was opened with) and lets the engine restore window placement.

use reframe_platform::{
    LaunchRequest, LaunchTarget, LocateQuery, OsKind, PlatformInfo, ProcessInfo, SupportLevel,
    WindowInfo,
};

use super::{
    AppAdapter, AppIdentity, CaptureContext, CaptureInput, CaptureOutput, LaunchContext, LaunchStep,
};
use crate::model::{
    AdapterCapability, AppState, ApplicationEntry, CapabilityLevel, DocumentRef, LaunchConfig,
    StateDetail,
};
use crate::util;

/// Maximum number of document arguments captured from a command line.
const MAX_DOCUMENTS: usize = 20;

#[derive(Debug, Default)]
pub struct GenericAdapter;

impl GenericAdapter {
    pub fn new() -> Self {
        Self
    }

    /// Logical id for an unknown application: package id when packaged, else executable stem.
    pub fn app_id_for(process: &ProcessInfo) -> String {
        if let Some(package) = &process.package {
            return format!("pkg:{}", package.id.to_lowercase());
        }
        let stem = process
            .executable_path
            .as_deref()
            .map(util::exe_stem)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| util::normalized_process_name(&process.name));
        format!("exe:{stem}")
    }

    fn display_name_for(process: &ProcessInfo, windows: &[&WindowInfo]) -> String {
        if let Some(name) = process
            .product_name
            .as_deref()
            .filter(|n| !n.trim().is_empty())
        {
            return name.trim().to_string();
        }
        let raw = process
            .executable_path
            .as_deref()
            .map(util::exe_stem)
            .filter(|s| !s.is_empty())
            .or_else(|| windows.iter().find_map(|w| w.class_name.clone()))
            .unwrap_or_else(|| process.name.clone());
        util::prettify_name(&raw)
    }
}

impl AppAdapter for GenericAdapter {
    fn id(&self) -> &'static str {
        "generic"
    }

    fn name(&self) -> &'static str {
        "Any application"
    }

    fn identify(&self, process: &ProcessInfo, windows: &[&WindowInfo]) -> Option<AppIdentity> {
        Some(AppIdentity {
            app_id: Self::app_id_for(process),
            display_name: Self::display_name_for(process, windows),
        })
    }

    fn capability(&self, info: &PlatformInfo) -> AdapterCapability {
        let placement = info.capabilities.move_windows.level != SupportLevel::Unsupported;
        AdapterCapability {
            level: CapabilityLevel::LaunchOnly,
            features: Vec::new(),
        }
        .feature("Launch application", true, None)
        .feature(
            "Window position and size",
            placement,
            info.capabilities
                .move_windows
                .note
                .as_deref()
                .filter(|_| !placement),
        )
        .feature(
            "Reopen documents passed on the command line",
            true,
            Some("Only files that still exist."),
        )
        .feature(
            "Application-internal state",
            false,
            Some("Restored only if the application does it itself."),
        )
    }

    fn capture(&self, _ctx: &CaptureContext<'_>, input: &CaptureInput<'_>) -> CaptureOutput {
        let Some(main) = input.main_process() else {
            return CaptureOutput {
                warnings: vec![format!(
                    "{}: no process information available.",
                    input.identity.display_name
                )],
                ..Default::default()
            };
        };
        let documents: Vec<DocumentRef> =
            util::document_arguments(&main.command_line, MAX_DOCUMENTS)
                .into_iter()
                .filter(|p| p.is_file())
                .map(|path| DocumentRef { path, page: None })
                .collect();
        let mut warnings = Vec::new();
        if main.executable_path.is_none() && main.package.is_none() {
            warnings.push(format!(
                "{}: the executable path is not accessible; choose it in the workspace before restoring.",
                input.identity.display_name
            ));
        }
        CaptureOutput {
            state: if documents.is_empty() {
                AppState::None
            } else {
                AppState::Documents { documents }
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
        _ctx: &LaunchContext<'_>,
        entry: &ApplicationEntry,
    ) -> Result<Vec<LaunchStep>, String> {
        let target = launch_target(&entry.launch)
            .ok_or_else(|| format!("No executable is configured for {}.", entry.display_name))?;
        let mut args: Vec<String> = Vec::new();
        if let AppState::Documents { documents } = &entry.state {
            args.extend(
                documents
                    .iter()
                    .filter(|d| d.path.exists())
                    .map(|d| d.path.display().to_string()),
            );
        }
        args.extend(entry.launch.arguments.iter().cloned());
        Ok(vec![LaunchStep {
            request: LaunchRequest {
                target,
                args,
                working_directory: entry.launch.working_directory.clone(),
                env: Vec::new(),
            },
            expected_windows: 1,
            title_hint: None,
        }])
    }

    fn describe(&self, state: &AppState) -> Vec<StateDetail> {
        match state {
            AppState::Documents { documents } if !documents.is_empty() => vec![StateDetail::new(
                if documents.len() == 1 {
                    "Document"
                } else {
                    "Documents"
                },
                documents
                    .iter()
                    .map(|d| util::path_label(&d.path))
                    .collect::<Vec<_>>()
                    .join(", "),
            )],
            _ => Vec::new(),
        }
    }

    fn locate_query(&self, app_id: &str) -> LocateQuery {
        let mut query = LocateQuery::default();
        if let Some(stem) = app_id.strip_prefix("exe:") {
            query.executable_names.push(stem.to_string());
            if OsKind::current() == OsKind::Windows {
                query.executable_names.push(format!("{stem}.exe"));
            }
            query.display_names.push(util::prettify_name(stem));
        } else if let Some(id) = app_id.strip_prefix("pkg:") {
            query.bundle_ids.push(id.to_string());
            query.desktop_ids.push(format!("{id}.desktop"));
        }
        query
    }
}

/// The launch target configured for an entry: package first, then executable.
pub fn launch_target(launch: &LaunchConfig) -> Option<LaunchTarget> {
    if let Some(package) = &launch.package {
        return Some(LaunchTarget::Package {
            package: package.clone(),
        });
    }
    launch
        .executable
        .as_ref()
        .map(|path| LaunchTarget::Executable { path: path.clone() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use reframe_platform::mock::MockPlatform;
    use reframe_platform::{Platform, Rect, WindowId, WindowState};

    fn process(exe: &str, cmd: &[&str]) -> ProcessInfo {
        ProcessInfo {
            pid: 42,
            parent_pid: None,
            name: "gimp".into(),
            product_name: None,
            executable_path: Some(exe.into()),
            command_line: cmd.iter().map(|s| s.to_string()).collect(),
            working_directory: None,
            start_time: None,
            package: None,
        }
    }

    fn window() -> WindowInfo {
        WindowInfo {
            id: WindowId::new("1"),
            pid: 42,
            title: "GIMP".into(),
            class_name: Some("Gimp".into()),
            app_id_hint: None,
            bounds: Rect::new(0, 0, 100, 100),
            normal_bounds: None,
            state: WindowState::Normal,
            monitor_id: None,
            desktop: None,
            is_focused: false,
            z_order: 0,
        }
    }

    #[test]
    fn identifies_by_executable_stem() {
        let p = process("/usr/bin/gimp-2.10", &["gimp"]);
        let w = window();
        let id = GenericAdapter::new().identify(&p, &[&w]).unwrap();
        assert_eq!(id.app_id, "exe:gimp-2");
        assert_eq!(id.display_name, "Gimp 2");
    }

    #[test]
    fn capture_keeps_documents_and_never_raw_arguments() {
        let doc = std::env::temp_dir().join("reframe-generic-capture.txt");
        std::fs::write(&doc, "x").unwrap();
        let p = process(
            "/usr/bin/gedit",
            &["gedit", "--api-key=hunter2", doc.to_str().unwrap()],
        );
        let w = window();
        let platform = MockPlatform::new();
        let info = platform.info();
        let ctx = CaptureContext {
            platform: &platform,
            info: &info,
            browser_tab_capture: false,
            apple_events: false,
        };
        let identity = AppIdentity {
            app_id: "exe:gedit".into(),
            display_name: "Gedit".into(),
        };
        let all = vec![p.clone()];
        let input = CaptureInput {
            identity: &identity,
            processes: vec![&p],
            windows: vec![&w],
            all_processes: &all,
        };
        let out = GenericAdapter::new().capture(&ctx, &input);
        assert!(out.launch.arguments.is_empty());
        assert_eq!(
            out.state,
            AppState::Documents {
                documents: vec![DocumentRef {
                    path: doc.clone(),
                    page: None
                }]
            }
        );
        let _ = std::fs::remove_file(doc);
    }

    #[test]
    fn launch_plan_requires_a_target() {
        let platform = MockPlatform::new();
        let info = platform.info();
        let ctx = LaunchContext {
            platform: &platform,
            info: &info,
        };
        let mut entry = ApplicationEntry::new("exe:x", "X", "generic");
        assert!(GenericAdapter::new().launch_plan(&ctx, &entry).is_err());
        entry.launch.executable = Some("/usr/bin/x".into());
        entry.launch.arguments = vec!["--new-window".into()];
        let steps = GenericAdapter::new().launch_plan(&ctx, &entry).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].request.args, vec!["--new-window".to_string()]);
    }
}
