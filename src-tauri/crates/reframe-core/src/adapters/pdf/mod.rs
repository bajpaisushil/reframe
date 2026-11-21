//! PDF and e-book readers: reopen the open documents, at the saved page where the reader
//! exposes one.
//!
//! Documents come from the readers' command lines (existing `.pdf`, `.epub`, `.djvu`, `.xps`...
//! files), from window titles (see [`titles`]) and, for Preview and Skim, from AppleScript.
//! Bare file names in titles are resolved against the documents already known and the reader's
//! own history; unresolvable names are reported as warnings.
//!
//! | Reader | Page source | Reopened with |
//! |---|---|---|
//! | SumatraPDF | `SumatraPDF-settings.txt` `FileStates` ([`sumatra`]) | `-page N <file>` |
//! | Okular | `~/.local/share/okular/docdata/*.xml` ([`okular`]) | `--page N <file>` |
//! | Skim | AppleScript `index of current page` (shown only) | `open -b <bundle> <file>` |
//! | Preview | — | `open -b <bundle> <file>` |
//! | Acrobat, zathura | — (a page set in the workspace is used) | `/A page=N <file>`, `-P N <file>` |
//! | Evince, Atril, Xreader, Foxit | — | `<file>` |
//!
//! One launch step is planned per document that still exists; readers with tabs (SumatraPDF,
//! Acrobat, Foxit) open later documents as tabs, so those steps expect no new window.
//! Limitations: background tabs of tabbed readers are only known when they were passed on the
//! command line; documents opened from the command line and closed later inside the reader are
//! still restored; saved pages can lag behind the page on screen (see the reader modules).

mod okular;
mod scripting;
mod sumatra;
mod titles;

use std::path::{Path, PathBuf};

use reframe_platform::{
    LaunchRequest, LaunchTarget, LocateQuery, OsKind, PackageInfo, PackageKind, PlatformInfo,
    ProcessInfo, WindowInfo,
};

use super::file_manager::support::{self, KnownApp};
use super::generic::{self, GenericAdapter};
use super::{
    AppAdapter, AppIdentity, CaptureContext, CaptureInput, CaptureOutput, InstanceModel,
    LaunchContext, LaunchStep,
};
use crate::model::{
    AdapterCapability, AppState, ApplicationEntry, CapabilityLevel, DocumentRef, StateDetail,
};
use crate::util;

/// At most this many documents are captured or reopened per application.
const MAX_DOCUMENTS: usize = 50;
/// Larger page numbers are treated as invalid.
const MAX_PAGE: u32 = 1_000_000;
/// macOS `open`, which hands documents to an application as "open document" events.
const MACOS_OPEN: &str = "/usr/bin/open";

const APPS: &[KnownApp] = &[
    KnownApp::new("sumatrapdf", "SumatraPDF")
        .executables(&["sumatrapdf"])
        .executable_prefixes(&["sumatrapdf-"])
        .locate_names(&["SumatraPDF.exe", "sumatrapdf"])
        .window_classes(&["SUMATRA_PDF_FRAME"]),
    KnownApp::new("acrobat", "Adobe Acrobat")
        .aliases(&[
            "Adobe Acrobat Reader",
            "Adobe Acrobat Reader DC",
            "Adobe Acrobat DC",
        ])
        .executables(&["acrobat", "acrord32"])
        .locate_names(&["Acrobat.exe", "AcroRd32.exe"])
        .bundle_ids(&["com.adobe.Reader", "com.adobe.Acrobat.Pro"])
        .window_classes(&["AcrobatSDIWindow"]),
    KnownApp::new("evince", "Evince")
        .aliases(&["Document Viewer"])
        .executables(&["evince"])
        .locate_names(&["evince"])
        .flatpak_ids(&["org.gnome.Evince"])
        .window_classes(&["evince", "org.gnome.Evince"])
        .desktop_ids(&["org.gnome.Evince.desktop", "evince.desktop"]),
    KnownApp::new("okular", "Okular")
        .executables(&["okular"])
        .locate_names(&["okular", "okular.exe"])
        .flatpak_ids(&["org.kde.okular"])
        .snap_names(&["okular"])
        .window_classes(&["okular", "org.kde.okular"])
        .desktop_ids(&["org.kde.okular.desktop", "okular.desktop"]),
    KnownApp::new("zathura", "zathura")
        .executables(&["zathura"])
        .locate_names(&["zathura"])
        .flatpak_ids(&["org.pwmt.zathura"])
        .window_classes(&["zathura", "org.pwmt.zathura"])
        .desktop_ids(&["org.pwmt.zathura.desktop"]),
    KnownApp::new("atril", "Atril")
        .executables(&["atril"])
        .locate_names(&["atril"])
        .window_classes(&["atril"])
        .desktop_ids(&["atril.desktop"]),
    KnownApp::new("xreader", "Xreader")
        .executables(&["xreader"])
        .locate_names(&["xreader"])
        .window_classes(&["xreader"])
        .desktop_ids(&["xreader.desktop"]),
    KnownApp::new("preview", "Preview")
        .executable_paths(&["Preview.app/Contents/MacOS/Preview"])
        .bundle_ids(&["com.apple.Preview"]),
    KnownApp::new("skim", "Skim")
        .executable_paths(&["Skim.app/Contents/MacOS/Skim"])
        .bundle_ids(&["net.sourceforge.skim-app.skim"]),
    KnownApp::new("foxit", "Foxit PDF Reader")
        .aliases(&["Foxit Reader", "Foxit PDF Editor"])
        .executables(&[
            "foxitpdfreader",
            "foxitreader",
            "foxitpdfeditor",
            "foxitphantompdf",
        ])
        .locate_names(&[
            "FoxitPDFReader.exe",
            "FoxitReader.exe",
            "FoxitPDFEditor.exe",
            "FoxitReader",
        ]),
];

const DOCUMENTS_NOTE: &str = "Documents from the command line and window titles; file names in \
                              titles are looked up in the reader's history where available.";
const SCRIPTED_NOTE: &str = "Reframe asks the reader for its open documents; macOS asks once for \
                             permission to control it (Automation).";

/// Readers that open further documents as tabs of the existing window.
fn opens_documents_in_tabs(app_id: &str) -> bool {
    matches!(app_id, "sumatrapdf" | "acrobat" | "foxit")
}

/// Where readers keep their history (replaced in tests).
#[derive(Debug, Clone, Default)]
struct HistorySources {
    okular_docdata: Vec<PathBuf>,
    /// `%LOCALAPPDATA%` (parent of SumatraPDF's settings folder).
    local_app_data: Option<PathBuf>,
    home: Option<PathBuf>,
}

impl HistorySources {
    fn from_environment() -> Self {
        Self {
            okular_docdata: okular::docdata_dirs(),
            local_app_data: dirs::data_local_dir(),
            home: dirs::home_dir(),
        }
    }
}

#[derive(Debug)]
pub struct PdfAdapter {
    generic: GenericAdapter,
    sources: HistorySources,
}

impl PdfAdapter {
    pub fn new() -> Self {
        Self {
            generic: GenericAdapter::new(),
            sources: HistorySources::from_environment(),
        }
    }

    #[cfg(test)]
    fn with_sources(sources: HistorySources) -> Self {
        Self {
            generic: GenericAdapter::new(),
            sources,
        }
    }
}

impl Default for PdfAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl AppAdapter for PdfAdapter {
    fn id(&self) -> &'static str {
        "pdf"
    }

    fn name(&self) -> &'static str {
        "PDF readers"
    }

    fn identify(&self, process: &ProcessInfo, windows: &[&WindowInfo]) -> Option<AppIdentity> {
        support::find_app(APPS, process, windows).map(KnownApp::identity)
    }

    fn capability(&self, info: &PlatformInfo) -> AdapterCapability {
        let capability = AdapterCapability {
            level: CapabilityLevel::Partial,
            features: Vec::new(),
        }
        .feature("Launch application", true, None)
        .feature("Reopen documents", true, Some(DOCUMENTS_NOTE))
        .feature(
            "Restore page",
            true,
            Some("SumatraPDF and Okular. Other readers reopen documents without a saved page."),
        );
        support::with_window_placement(capability, info)
    }

    fn capability_for_app(&self, app_id: &str, info: &PlatformInfo) -> AdapterCapability {
        let (level, documents_note, page_supported, page_note) = match app_id {
            "sumatrapdf" => (
                CapabilityLevel::Full,
                DOCUMENTS_NOTE,
                true,
                "The page SumatraPDF last saved in its settings file (it saves when a document \
                 is closed or a setting changes).",
            ),
            "okular" => (
                CapabilityLevel::Full,
                DOCUMENTS_NOTE,
                true,
                "The page Okular last saved for the document (usually when it was closed).",
            ),
            "skim" => (
                CapabilityLevel::Partial,
                SCRIPTED_NOTE,
                false,
                "The current page is captured and shown, but Skim chooses the page when \
                 reopening (it remembers the last page itself).",
            ),
            "preview" => (
                CapabilityLevel::Partial,
                SCRIPTED_NOTE,
                false,
                "Preview does not expose the current page.",
            ),
            "acrobat" | "zathura" => (
                CapabilityLevel::Partial,
                DOCUMENTS_NOTE,
                false,
                "The reader does not expose its current page; a page entered in the workspace \
                 is opened.",
            ),
            _ => (
                CapabilityLevel::Partial,
                DOCUMENTS_NOTE,
                false,
                "This reader does not expose its current page.",
            ),
        };
        let capability = AdapterCapability {
            level,
            features: Vec::new(),
        }
        .feature("Launch application", true, None)
        .feature("Reopen documents", true, Some(documents_note))
        .feature("Restore page", page_supported, Some(page_note));
        support::with_window_placement(capability, info)
    }

    fn capture(&self, ctx: &CaptureContext<'_>, input: &CaptureInput<'_>) -> CaptureOutput {
        let (launch, mut warnings) = support::launch_config(input);
        let app_id = input.identity.app_id.as_str();
        let name = input.identity.display_name.as_str();

        // Documents passed on the command line, from every process: several readers run one
        // process per document.
        let mut command_line: Vec<PathBuf> = Vec::new();
        for process in &input.processes {
            for path in command_line_documents(&process.command_line) {
                if titles::has_document_extension(&path.to_string_lossy())
                    && path.is_file()
                    && !command_line.contains(&path)
                {
                    command_line.push(path);
                }
            }
        }

        let scripted = if ctx.apple_events && scripting::is_scriptable(app_id) {
            scripting::open_documents(app_id).unwrap_or_else(|error| {
                warnings.push(format!(
                    "{name}: could not ask for the open documents ({error}). If macOS asked, \
                     allow Reframe to control {name} in System Settings > Privacy & Security > \
                     Automation."
                ));
                Vec::new()
            })
        } else {
            Vec::new()
        };

        let history = History::load(app_id, input.main_process(), &self.sources);
        let known: Vec<PathBuf> = command_line
            .iter()
            .cloned()
            .chain(scripted.iter().map(|d| d.path.clone()))
            .collect();

        let window_titles: Vec<&str> = input.windows.iter().map(|w| w.title.as_str()).collect();
        let mut paths: Vec<PathBuf> = Vec::new();
        for title in &window_titles {
            let candidates = titles::document_candidates(title);
            let Some(shortest) = candidates.last() else {
                continue; // the title shows no file name (start page, document title...)
            };
            match resolve(&candidates, &known, &history, self.sources.home.as_deref()) {
                Some(path) => push_unique(&mut paths, path),
                None => warnings.push(format!(
                    "{name}: could not find the file \"{shortest}\" shown in a window; it was \
                     skipped."
                )),
            }
        }
        for document in scripting::select_for_windows(&scripted, &window_titles) {
            push_unique(&mut paths, document.path.clone());
        }
        for path in command_line {
            push_unique(&mut paths, path);
        }
        if paths.len() > MAX_DOCUMENTS {
            warnings.push(format!(
                "{name}: only the first {MAX_DOCUMENTS} of {} documents were captured.",
                paths.len()
            ));
            paths.truncate(MAX_DOCUMENTS);
        }

        let documents: Vec<DocumentRef> = paths
            .into_iter()
            .map(|path| {
                let page = scripted
                    .iter()
                    .find(|d| d.path == path)
                    .and_then(|d| d.page)
                    .or_else(|| history.page_for(&path))
                    .filter(|p| (1..=MAX_PAGE).contains(p));
                DocumentRef { path, page }
            })
            .collect();
        CaptureOutput {
            state: if documents.is_empty() {
                AppState::None
            } else {
                AppState::Documents { documents }
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
        let documents: Vec<&DocumentRef> = match &entry.state {
            AppState::Documents { documents } => documents
                .iter()
                .filter(|d| d.path.is_absolute() && d.path.is_file())
                .take(MAX_DOCUMENTS)
                .collect(),
            _ => Vec::new(),
        };
        let target = launch_target(ctx.info.os, entry)
            .ok_or_else(|| format!("No executable is configured for {}.", entry.display_name))?;
        // macOS readers get their documents through `open -b <bundle> <file>`: launching the
        // bundle with the file as an argument starts a new instance whose AppKit code ignores it.
        let bundle = match &target {
            LaunchTarget::Package { package }
                if package.kind == PackageKind::MacBundle && is_bundle_id(&package.id) =>
            {
                Some(package.id.as_str())
            }
            _ => None,
        };
        let request = |args: Vec<String>| LaunchRequest {
            target: target.clone(),
            args,
            working_directory: entry.launch.working_directory.clone(),
            env: Vec::new(),
        };
        if documents.is_empty() {
            // Nothing left to reopen: start the reader on its own.
            return Ok(vec![LaunchStep {
                request: request(entry.launch.arguments.clone()),
                expected_windows: 1,
                title_hint: None,
            }]);
        }
        let tabs = opens_documents_in_tabs(&entry.app_id);
        Ok(documents
            .into_iter()
            .enumerate()
            .map(|(index, document)| {
                let request = match bundle {
                    Some(bundle) => {
                        open_document_request(bundle, &document.path, &entry.launch.arguments)
                    }
                    None => {
                        let mut args = entry.launch.arguments.clone();
                        args.extend(document_args(&entry.app_id, &document.path, document.page));
                        request(args)
                    }
                };
                LaunchStep {
                    request,
                    expected_windows: if tabs && index > 0 { 0 } else { 1 },
                    title_hint: Some(support::display_name(&document.path)),
                }
            })
            .collect())
    }

    fn instance_model(&self) -> InstanceModel {
        // SumatraPDF, Acrobat, Foxit, Preview and Skim forward documents to a running instance.
        InstanceModel::SingleInstance
    }

    fn describe(&self, state: &AppState) -> Vec<StateDetail> {
        match state {
            AppState::Documents { documents } if !documents.is_empty() => {
                vec![StateDetail::new(
                    if documents.len() == 1 {
                        "Document"
                    } else {
                        "Documents"
                    },
                    support::summarize(
                        documents.iter().map(|d| match d.page {
                            Some(page) => {
                                format!("{} (page {page})", support::display_name(&d.path))
                            }
                            None => support::display_name(&d.path),
                        }),
                        4,
                    ),
                )]
            }
            _ => Vec::new(),
        }
    }

    fn locate_query(&self, app_id: &str) -> LocateQuery {
        support::app_by_id(APPS, app_id)
            .map(KnownApp::locate_query)
            .unwrap_or_else(|| self.generic.locate_query(app_id))
    }
}

/// On macOS, readers are started through their bundle id (documents are then opened with
/// [`open_document_request`], as "open document" events: bundle executables ignore file
/// arguments).
fn launch_target(os: OsKind, entry: &ApplicationEntry) -> Option<LaunchTarget> {
    if os == OsKind::Macos {
        if let Some(package) = entry
            .launch
            .package
            .as_ref()
            .filter(|p| p.kind == PackageKind::MacBundle)
        {
            return Some(LaunchTarget::Package {
                package: package.clone(),
            });
        }
        if let Some(bundle) =
            support::app_by_id(APPS, &entry.app_id).and_then(|app| app.bundle_ids.first())
        {
            return Some(LaunchTarget::Package {
                package: PackageInfo {
                    kind: PackageKind::MacBundle,
                    id: bundle.to_string(),
                    path: None,
                },
            });
        }
    }
    generic::launch_target(&entry.launch)
}

/// `open -b <bundle> <file>` (macOS): the document reaches the running reader, or a newly started
/// one, as an "open document" event. Pages cannot be passed this way; user arguments go to the
/// application after `--args`.
fn open_document_request(bundle: &str, path: &Path, user_args: &[String]) -> LaunchRequest {
    let mut args = vec![
        "-b".to_string(),
        bundle.to_string(),
        path.display().to_string(),
    ];
    if !user_args.is_empty() {
        args.push("--args".to_string());
        args.extend(user_args.iter().cloned());
    }
    LaunchRequest {
        target: LaunchTarget::Executable {
            path: PathBuf::from(MACOS_OPEN),
        },
        args,
        // `open` hands the document to Launch Services; a working directory means nothing there.
        working_directory: None,
        env: Vec::new(),
    }
}

/// A plausible macOS bundle identifier (`com.apple.Preview`): never empty, never an option.
fn is_bundle_id(id: &str) -> bool {
    (1..=255).contains(&id.len())
        && !id.starts_with(['-', '.'])
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// Arguments that open `path` (at `page` where the reader supports it).
fn document_args(app_id: &str, path: &Path, page: Option<u32>) -> Vec<String> {
    let file = path.display().to_string();
    match (app_id, page.filter(|p| (1..=MAX_PAGE).contains(p))) {
        ("sumatrapdf", Some(page)) => vec!["-page".into(), page.to_string(), file],
        ("acrobat", Some(page)) => vec!["/A".into(), format!("page={page}"), file],
        ("okular", Some(page)) => vec!["--page".into(), page.to_string(), file],
        ("zathura", Some(page)) => vec!["-P".into(), page.to_string(), file],
        _ => vec![file],
    }
}

/// The reader's own record of documents and pages.
#[derive(Debug, Default)]
struct History {
    sumatra: Vec<sumatra::FileState>,
    okular_dirs: Vec<PathBuf>,
}

impl History {
    fn load(app_id: &str, main: Option<&ProcessInfo>, sources: &HistorySources) -> Self {
        match app_id {
            "sumatrapdf" => {
                let executable = main.and_then(|p| p.executable_path.as_deref());
                let locations =
                    sumatra::settings_locations(executable, sources.local_app_data.as_deref());
                Self {
                    sumatra: sumatra::load(&locations),
                    ..Self::default()
                }
            }
            "okular" => Self {
                okular_dirs: sources.okular_docdata.clone(),
                ..Self::default()
            },
            _ => Self::default(),
        }
    }

    /// Existing documents in the history whose file name is `name`.
    fn documents_named(&self, name: &str) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = Vec::new();
        for state in &self.sumatra {
            let path = PathBuf::from(&state.path);
            if !state.missing
                && support::display_name(&path).eq_ignore_ascii_case(name)
                && path.is_file()
                && !found
                    .iter()
                    .any(|f| sumatra::same_path(&f.to_string_lossy(), &state.path))
            {
                found.push(path);
            }
        }
        for path in okular::documents_named(&self.okular_dirs, name) {
            if !found.contains(&path) {
                found.push(path);
            }
        }
        found
    }

    fn page_for(&self, path: &Path) -> Option<u32> {
        let text = path.to_string_lossy();
        self.sumatra
            .iter()
            .find(|state| sumatra::same_path(&state.path, &text))
            .and_then(|state| state.page)
            .or_else(|| okular::page_for(&self.okular_dirs, path))
    }
}

/// The file a title names. Candidates are tried longest first: an absolute (or `~/`) path that
/// exists, else the single known document (command line, AppleScript) with that file name, else
/// the single document with that name in the reader's history.
fn resolve(
    candidates: &[String],
    known: &[PathBuf],
    history: &History,
    home: Option<&Path>,
) -> Option<PathBuf> {
    for candidate in candidates {
        if let Some(path) = absolute_path(candidate, home) {
            if path.is_file() {
                return Some(path);
            }
            continue;
        }
        if let Some(path) = unique_by_name(known, candidate) {
            return Some(path);
        }
        if let Some(path) = unique_by_name(&history.documents_named(candidate), candidate) {
            return Some(path);
        }
    }
    None
}

fn absolute_path(candidate: &str, home: Option<&Path>) -> Option<PathBuf> {
    if let Some(rest) = candidate.strip_prefix("~/") {
        return home.map(|h| h.join(rest));
    }
    let path = PathBuf::from(candidate);
    path.is_absolute().then_some(path)
}

/// The single path whose file name is `name` (exact match first, then ignoring case). The same
/// path listed twice (command line and AppleScript) counts once.
fn unique_by_name(paths: &[PathBuf], name: &str) -> Option<PathBuf> {
    let matching = |matches: &dyn Fn(&str) -> bool| {
        let mut found: Vec<&PathBuf> = Vec::new();
        for path in paths {
            if matches(&support::display_name(path)) && !found.contains(&path) {
                found.push(path);
            }
        }
        found
    };
    let exact = matching(&|file_name| file_name == name);
    match exact.as_slice() {
        [only] => return Some((*only).clone()),
        [] => {}
        _ => return None,
    }
    let lower = name.to_lowercase();
    let loose = matching(&|file_name| file_name.to_lowercase() == lower);
    match loose.as_slice() {
        [only] => Some((*only).clone()),
        _ => None,
    }
}

/// Existing files named on a command line: absolute paths and `file://` URIs (desktop
/// launchers start readers with `%U`, i.e. URIs). Nothing else from the command line is kept,
/// because it can contain secrets.
fn command_line_documents(command_line: &[String]) -> Vec<PathBuf> {
    let mut documents = util::document_arguments(command_line, MAX_DOCUMENTS);
    for arg in command_line.iter().skip(1) {
        if documents.len() >= MAX_DOCUMENTS {
            break;
        }
        if !arg
            .get(..7)
            .is_some_and(|p| p.eq_ignore_ascii_case("file://"))
        {
            continue;
        }
        if let Ok(url) = url::Url::parse(arg)
            && let Ok(path) = url.to_file_path()
            && path.is_file()
            && !documents.iter().any(|d| util::same_path(d, &path))
        {
            documents.push(path);
        }
    }
    documents
}

fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|p| util::same_path(p, &path)) {
        paths.push(path);
    }
}

#[cfg(test)]
mod tests {
    use super::support::testing::{platform, process, window};
    use super::*;

    fn identify(p: &ProcessInfo, windows: &[&WindowInfo]) -> Option<String> {
        PdfAdapter::new().identify(p, windows).map(|id| id.app_id)
    }

    #[test]
    fn identifies_readers() {
        let cases = [
            (
                "SumatraPDF-3.5.2-64.exe",
                r"C:\Tools\SumatraPDF-3.5.2-64.exe",
                "sumatrapdf",
            ),
            ("AcroRd32.exe", r"C:\Adobe\AcroRd32.exe", "acrobat"),
            ("evince", "/usr/bin/evince", "evince"),
            ("zathura", "/usr/bin/zathura", "zathura"),
            (
                "FoxitPDFReader.exe",
                r"C:\Foxit\FoxitPDFReader.exe",
                "foxit",
            ),
            (
                "Preview",
                "/System/Applications/Preview.app/Contents/MacOS/Preview",
                "preview",
            ),
        ];
        for (name, exe, expected) in cases {
            assert_eq!(
                identify(&process(1, name, Some(exe)), &[]).as_deref(),
                Some(expected),
                "{name}"
            );
        }
        let mut okular = process(1, "okular", Some("/app/bin/okular"));
        okular.package = Some(PackageInfo {
            kind: PackageKind::Flatpak,
            id: "org.kde.okular".into(),
            path: None,
        });
        assert_eq!(identify(&okular, &[]).as_deref(), Some("okular"));
        let skim_window = window("1", 1, "paper.pdf", Some("net.sourceforge.skim-app.skim"));
        assert_eq!(
            identify(&process(1, "Skim", None), &[&skim_window]).as_deref(),
            Some("skim")
        );
        assert_eq!(
            identify(&process(1, "preview", Some("/usr/bin/preview")), &[]),
            None
        );
        assert_eq!(
            identify(&process(1, "gedit", Some("/usr/bin/gedit")), &[]),
            None
        );
    }

    #[test]
    fn capability_differs_per_reader() {
        let adapter = PdfAdapter::new();
        let (_platform, info) = platform(OsKind::Linux);
        assert_eq!(adapter.capability(&info).level, CapabilityLevel::Partial);
        let page = |app: &str| {
            let capability = adapter.capability_for_app(app, &info);
            let feature = capability
                .features
                .iter()
                .find(|f| f.name == "Restore page")
                .cloned()
                .unwrap();
            (capability.level, feature.supported)
        };
        assert_eq!(page("sumatrapdf"), (CapabilityLevel::Full, true));
        assert_eq!(page("okular"), (CapabilityLevel::Full, true));
        assert_eq!(page("evince"), (CapabilityLevel::Partial, false));
        assert_eq!(page("skim"), (CapabilityLevel::Partial, false));
        assert_eq!(page("acrobat"), (CapabilityLevel::Partial, false));
    }

    struct Docs {
        _dir: tempfile::TempDir,
        a: PathBuf,
        b: PathBuf,
        missing: PathBuf,
    }

    fn docs() -> Docs {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("alpha.pdf");
        let b = dir.path().join("beta.epub");
        std::fs::write(&a, b"%PDF").unwrap();
        std::fs::write(&b, b"PK").unwrap();
        let missing = dir.path().join("gone.pdf");
        Docs {
            _dir: dir,
            a,
            b,
            missing,
        }
    }

    fn entry(app_id: &str, documents: &[(&Path, Option<u32>)]) -> ApplicationEntry {
        let mut entry = ApplicationEntry::new(app_id, app_id, "pdf");
        entry.launch.executable = Some(PathBuf::from(format!("/usr/bin/{app_id}")));
        entry.state = AppState::Documents {
            documents: documents
                .iter()
                .map(|(path, page)| DocumentRef {
                    path: path.to_path_buf(),
                    page: *page,
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
        PdfAdapter::new().launch_plan(&ctx, entry)
    }

    fn args(steps: &[LaunchStep]) -> Vec<Vec<String>> {
        steps.iter().map(|s| s.request.args.clone()).collect()
    }

    #[test]
    fn page_arguments_per_reader() {
        let d = docs();
        let a = d.a.display().to_string();
        let b = d.b.display().to_string();
        let steps = plan(
            OsKind::Windows,
            &entry(
                "sumatrapdf",
                &[(&d.a, Some(12)), (&d.missing, Some(1)), (&d.b, None)],
            ),
        )
        .unwrap();
        assert_eq!(
            args(&steps),
            vec![
                vec!["-page".to_string(), "12".into(), a.clone()],
                vec![b.clone()]
            ]
        );
        assert_eq!(steps[0].expected_windows, 1);
        assert_eq!(steps[1].expected_windows, 0, "SumatraPDF opens a tab");
        assert_eq!(steps[0].title_hint.as_deref(), Some("alpha.pdf"));

        let one = |app: &str, page: u32| {
            args(&plan(OsKind::Linux, &entry(app, &[(&d.a, Some(page))])).unwrap()).remove(0)
        };
        assert_eq!(
            one("acrobat", 3),
            vec!["/A".to_string(), "page=3".into(), a.clone()]
        );
        assert_eq!(
            one("okular", 5),
            vec!["--page".to_string(), "5".into(), a.clone()]
        );
        assert_eq!(
            one("zathura", 2),
            vec!["-P".to_string(), "2".into(), a.clone()]
        );
        assert_eq!(one("evince", 9), vec![a.clone()]);
        assert_eq!(one("okular", 0), vec![a.clone()], "page 0 is invalid");
        assert_eq!(one("okular", MAX_PAGE + 1), vec![a.clone()]);

        let steps = plan(
            OsKind::Linux,
            &entry("okular", &[(&d.a, None), (&d.b, None)]),
        )
        .unwrap();
        assert_eq!(
            steps[1].expected_windows, 1,
            "Okular opens a window per document"
        );
    }

    #[test]
    fn user_arguments_come_first() {
        let d = docs();
        let mut e = entry("sumatrapdf", &[(&d.a, Some(2))]);
        e.launch.arguments = vec!["-reuse-instance".into()];
        let steps = plan(OsKind::Windows, &e).unwrap();
        assert_eq!(
            steps[0].request.args,
            vec![
                "-reuse-instance".to_string(),
                "-page".into(),
                "2".into(),
                d.a.display().to_string()
            ]
        );
    }

    #[test]
    fn macos_readers_open_documents_through_open_with_their_bundle() {
        // A Package launch is `open -n -b <bundle> --args …` on macOS: a new reader instance per
        // document, with the file as a command-line argument that AppKit readers ignore. Documents
        // must be handed over as "open document" events: `open -b <bundle> <file>`.
        let d = docs();
        let open = LaunchTarget::Executable {
            path: PathBuf::from("/usr/bin/open"),
        };
        let mut e = entry("preview", &[(&d.a, None)]);
        e.launch.executable = None;
        e.launch.working_directory = Some(PathBuf::from("/definitely/missing"));
        let steps = plan(OsKind::Macos, &e).unwrap();
        assert_eq!(steps[0].request.target, open);
        assert_eq!(
            steps[0].request.args,
            vec![
                "-b".to_string(),
                "com.apple.Preview".into(),
                d.a.display().to_string()
            ]
        );
        assert_eq!(steps[0].request.working_directory, None);
        assert_eq!(steps[0].title_hint.as_deref(), Some("alpha.pdf"));

        // The captured bundle wins; Skim's page cannot be passed; user arguments go to the app.
        let captured = PackageInfo {
            kind: PackageKind::MacBundle,
            id: "net.sourceforge.skim-app.skim".into(),
            path: Some("/Applications/Skim.app".into()),
        };
        let mut e = entry("skim", &[(&d.a, Some(4)), (&d.b, None)]);
        e.launch.package = Some(captured.clone());
        e.launch.arguments = vec!["--user-flag".into()];
        let steps = plan(OsKind::Macos, &e).unwrap();
        assert_eq!(steps.len(), 2);
        assert!(steps.iter().all(|s| s.request.target == open));
        assert_eq!(
            args(&steps),
            vec![
                vec![
                    "-b".to_string(),
                    "net.sourceforge.skim-app.skim".into(),
                    d.a.display().to_string(),
                    "--args".into(),
                    "--user-flag".into(),
                ],
                vec![
                    "-b".to_string(),
                    "net.sourceforge.skim-app.skim".into(),
                    d.b.display().to_string(),
                    "--args".into(),
                    "--user-flag".into(),
                ],
            ]
        );

        // Nothing left to reopen: the reader starts through its bundle.
        let steps = plan(OsKind::Macos, &entry("preview", &[(&d.missing, None)])).unwrap();
        assert_eq!(
            steps[0].request.target,
            LaunchTarget::Package {
                package: PackageInfo {
                    kind: PackageKind::MacBundle,
                    id: "com.apple.Preview".into(),
                    path: None,
                }
            }
        );
        assert!(steps[0].request.args.is_empty());

        // A captured "bundle id" that is not one is never handed to `open`.
        let mut e = entry("preview", &[(&d.a, None)]);
        e.launch.package = Some(PackageInfo {
            kind: PackageKind::MacBundle,
            id: "-a".into(),
            path: None,
        });
        let steps = plan(OsKind::Macos, &e).unwrap();
        assert_ne!(steps[0].request.target, open);
    }

    #[test]
    fn documents_named_like_flags_stay_absolute_paths() {
        let dir = tempfile::tempdir().unwrap();
        let flag = dir.path().join("-rf.pdf");
        std::fs::write(&flag, b"%PDF").unwrap();
        for (os, app) in [
            (OsKind::Linux, "okular"),
            (OsKind::Windows, "sumatrapdf"),
            (OsKind::Macos, "preview"),
        ] {
            let steps = plan(os, &entry(app, &[(&flag, Some(2))])).unwrap();
            let last = steps[0].request.args.last().unwrap();
            assert_eq!(last, &flag.display().to_string(), "{app}");
            assert!(!last.starts_with('-'), "{app}");
        }
        // A relative "-rf.pdf" (e.g. from an edited workspace) is never passed at all.
        let mut relative = entry("okular", &[]);
        relative.state = AppState::Documents {
            documents: vec![DocumentRef {
                path: PathBuf::from("-rf.pdf"),
                page: None,
            }],
        };
        assert!(
            plan(OsKind::Linux, &relative).unwrap()[0]
                .request
                .args
                .is_empty()
        );
    }

    #[test]
    fn plain_launch_without_documents_and_error_without_target() {
        let d = docs();
        let steps = plan(OsKind::Linux, &entry("evince", &[(&d.missing, None)])).unwrap();
        assert_eq!(steps.len(), 1);
        assert!(steps[0].request.args.is_empty());
        assert_eq!(steps[0].title_hint, None);
        let mut e = entry("evince", &[(&d.a, None)]);
        e.launch.executable = None;
        assert!(plan(OsKind::Linux, &e).is_err());
        let mut relative = entry("evince", &[(Path::new("alpha.pdf"), None)]);
        relative.state = AppState::Documents {
            documents: vec![DocumentRef {
                path: PathBuf::from("alpha.pdf"),
                page: None,
            }],
        };
        assert!(
            plan(OsKind::Linux, &relative).unwrap()[0]
                .request
                .args
                .is_empty()
        );
    }

    fn capture(
        adapter: &PdfAdapter,
        process: &ProcessInfo,
        windows: &[&WindowInfo],
    ) -> CaptureOutput {
        let (platform, info) = platform(OsKind::Linux);
        let ctx = CaptureContext {
            platform: &platform,
            info: &info,
            browser_tab_capture: false,
            apple_events: false,
        };
        let identity = adapter.identify(process, windows).unwrap();
        let all = vec![process.clone()];
        let input = CaptureInput {
            identity: &identity,
            processes: vec![process],
            windows: windows.to_vec(),
            all_processes: &all,
        };
        let out = adapter.capture(&ctx, &input);
        assert!(platform.launches().is_empty());
        out
    }

    #[test]
    fn captures_okular_documents_with_docdata_pages() {
        let d = docs();
        let docdata = tempfile::tempdir().unwrap();
        let size = std::fs::metadata(&d.a).unwrap().len();
        std::fs::write(
            docdata.path().join(format!("{size}.alpha.pdf.xml")),
            format!(
                r#"<documentInfo url="{}"><history><current viewport="6;C2:0:0:1"/></history></documentInfo>"#,
                d.a.display()
            ),
        )
        .unwrap();
        let adapter = PdfAdapter::with_sources(HistorySources {
            okular_docdata: vec![docdata.path().to_path_buf()],
            ..Default::default()
        });
        let mut p = process(3, "okular", Some("/usr/bin/okular"));
        p.command_line = vec![
            "okular".into(),
            "--password=hunter2".into(),
            d.a.display().to_string(),
            "/etc/hostname".into(),
        ];
        let w = window("1", 3, "alpha.pdf — Okular", Some("okular"));
        let out = capture(&adapter, &p, &[&w]);
        assert_eq!(
            out.state,
            AppState::Documents {
                documents: vec![DocumentRef {
                    path: d.a.clone(),
                    page: Some(7)
                }]
            }
        );
        assert!(out.launch.arguments.is_empty());
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    }

    #[test]
    fn captures_sumatra_documents_from_titles_and_settings() {
        let d = docs();
        let local = tempfile::tempdir().unwrap();
        let settings_dir = local.path().join("SumatraPDF");
        std::fs::create_dir(&settings_dir).unwrap();
        std::fs::write(
            settings_dir.join(sumatra::SETTINGS_FILE),
            format!(
                "FileStates [\n\t[\n\t\tFilePath = {}\n\t\tPageNo = 31\n\t]\n\t[\n\t\tFilePath = {}\n\t\tPageNo = 4\n\t]\n]\n",
                d.a.display(),
                d.b.display()
            ),
        )
        .unwrap();
        let adapter = PdfAdapter::with_sources(HistorySources {
            local_app_data: Some(local.path().to_path_buf()),
            ..Default::default()
        });
        // Tabs: the visible document is in the title; the launch file is on the command line.
        let mut p = process(9, "SumatraPDF.exe", Some("/opt/sumatra/SumatraPDF.exe"));
        p.command_line = vec!["SumatraPDF.exe".into(), d.b.display().to_string()];
        let w1 = window(
            "0x1",
            9,
            "alpha.pdf - SumatraPDF",
            Some("SUMATRA_PDF_FRAME"),
        );
        let w2 = window(
            "0x2",
            9,
            "unknown.pdf - SumatraPDF",
            Some("SUMATRA_PDF_FRAME"),
        );
        let out = capture(&adapter, &p, &[&w1, &w2]);
        assert_eq!(
            out.state,
            AppState::Documents {
                documents: vec![
                    DocumentRef {
                        path: d.a.clone(),
                        page: Some(31)
                    },
                    DocumentRef {
                        path: d.b.clone(),
                        page: Some(4)
                    },
                ]
            }
        );
        assert_eq!(out.warnings.len(), 1);
        assert!(out.warnings[0].contains("could not find the file \"unknown.pdf\""));
    }

    #[test]
    fn captures_evince_titles_with_document_titles() {
        let d = docs();
        let adapter = PdfAdapter::with_sources(HistorySources::default());
        // Desktop launchers pass `%U`: a file URI.
        let mut p = process(4, "evince", Some("/usr/bin/evince"));
        p.command_line = vec![
            "evince".into(),
            url::Url::from_file_path(&d.a).unwrap().to_string(),
        ];
        let w = window("1", 4, "Annual Report — alpha.pdf", Some("Evince"));
        let out = capture(&adapter, &p, &[&w]);
        assert_eq!(
            out.state,
            AppState::Documents {
                documents: vec![DocumentRef {
                    path: d.a.clone(),
                    page: None
                }]
            }
        );

        // zathura shows the full path; nothing on the command line is needed.
        let z = process(5, "zathura", Some("/usr/bin/zathura"));
        let w = window("2", 5, &d.b.display().to_string(), Some("Zathura"));
        let out = capture(&adapter, &z, &[&w]);
        assert_eq!(
            out.state,
            AppState::Documents {
                documents: vec![DocumentRef {
                    path: d.b.clone(),
                    page: None
                }]
            }
        );
    }

    #[test]
    fn preview_off_macos_warns_and_still_captures_titles() {
        let adapter = PdfAdapter::with_sources(HistorySources::default());
        let p = process(
            6,
            "Preview",
            Some("/System/Applications/Preview.app/Contents/MacOS/Preview"),
        );
        let w = window("1", 6, "nothing.pdf", Some("com.apple.Preview"));
        let out = capture(&adapter, &p, &[&w]);
        assert_eq!(out.state, AppState::None);
        if !cfg!(target_os = "macos") {
            assert!(out.warnings[0].contains("Automation"));
        }
        assert!(out.warnings.iter().any(|w| w.contains("\"nothing.pdf\"")));
    }

    #[test]
    fn describes_documents_with_pages() {
        let adapter = PdfAdapter::new();
        let state = AppState::Documents {
            documents: vec![
                DocumentRef {
                    path: "/a/paper.pdf".into(),
                    page: Some(12),
                },
                DocumentRef {
                    path: r"C:\b\book.epub".into(),
                    page: None,
                },
            ],
        };
        assert_eq!(
            adapter.describe(&state),
            vec![StateDetail::new(
                "Documents",
                "paper.pdf (page 12), book.epub"
            )]
        );
        assert!(adapter.describe(&AppState::None).is_empty());
    }

    #[test]
    fn locate_queries_and_instance_model() {
        let adapter = PdfAdapter::new();
        assert_eq!(
            adapter.locate_query("okular").desktop_ids[0],
            "org.kde.okular.desktop".to_string()
        );
        assert_eq!(
            adapter.locate_query("skim").bundle_ids,
            vec!["net.sourceforge.skim-app.skim".to_string()]
        );
        assert!(
            adapter
                .locate_query("sumatrapdf")
                .executable_names
                .contains(&"SumatraPDF.exe".to_string())
        );
        assert_eq!(adapter.instance_model(), InstanceModel::SingleInstance);
    }

    #[test]
    fn name_resolution_requires_a_unique_match() {
        let paths = vec![
            PathBuf::from("/a/Report.pdf"),
            PathBuf::from("/b/report.pdf"),
            PathBuf::from("/c/notes.pdf"),
        ];
        assert_eq!(unique_by_name(&paths, "Report.pdf"), Some(paths[0].clone()));
        assert_eq!(unique_by_name(&paths, "NOTES.PDF"), Some(paths[2].clone()));
        assert_eq!(unique_by_name(&paths, "REPORT.PDF"), None);
        assert_eq!(unique_by_name(&paths, "missing.pdf"), None);
        // The same document known twice (command line and AppleScript) is still one match.
        let twice = vec![paths[2].clone(), paths[0].clone(), paths[2].clone()];
        assert_eq!(unique_by_name(&twice, "notes.pdf"), Some(paths[2].clone()));
        assert_eq!(unique_by_name(&twice, "Notes.PDF"), Some(paths[2].clone()));
        assert_eq!(
            absolute_path("~/x.pdf", Some(Path::new("/home/me"))),
            Some(PathBuf::from("/home/me/x.pdf"))
        );
        assert_eq!(absolute_path("~/x.pdf", None), None);
        assert_eq!(absolute_path("x.pdf", None), None);
    }

    #[test]
    fn command_lines_yield_paths_and_file_uris_only() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("plain.pdf");
        let spaced = dir.path().join("my paper.pdf");
        std::fs::write(&plain, b"%PDF").unwrap();
        std::fs::write(&spaced, b"%PDF").unwrap();
        let uri = url::Url::from_file_path(&spaced).unwrap().to_string();
        assert!(uri.contains("%20"));
        let args = vec![
            "evince".to_string(),
            "--password=hunter2".into(),
            plain.display().to_string(),
            uri.clone(),
            uri.to_uppercase(),
            "file:///definitely/missing.pdf".into(),
            "https://example.com/a.pdf".into(),
            "file://".into(),
        ];
        assert_eq!(command_line_documents(&args), vec![plain, spaced]);
    }
}
