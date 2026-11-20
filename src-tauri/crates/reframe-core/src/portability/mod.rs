//! Portable JSON import/export.
//!
//! **Export** ([`export_workspace`]) splits a workspace into the *logical* part (applications,
//! adapter state sanitized by each adapter, sanitized launch arguments, windows, options,
//! restore settings) and the *machine profile* (OS, monitors, and per application the
//! executable, package and working directory). Only modelled data is written, and the document
//! is validated against the import rules before it is returned, so Reframe never writes a file
//! it would refuse to import.
//!
//! **Import** is a three-step flow driven by the UI:
//! 1. [`parse_export`] — strict validation: size, encoding, JSON syntax (with positions), format
//!    and version, NUL characters, every limit in [`limits`] and the workspace model,
//!    references between entries. Unknown extra fields are ignored (forward compatible).
//! 2. [`preview_import`] — resolve every application on this machine (the exported executable
//!    or package when the OS matches and it exists, otherwise the adapter's locate query), list
//!    paths that do not exist here, suggest folder mappings and warn about what needs review.
//! 3. [`build_imported_workspace`] — a new workspace with fresh ids from the user's decisions
//!    (skipped applications, chosen executables, folder mappings).
//!
//! Safety — an import file is untrusted input from another machine or person:
//! - imported terminal startup commands are always switched off, and imported launch
//!   arguments are never applied (the preview lists what was removed), so restoring an
//!   imported workspace cannot run a command the user has not approved;
//! - paths must be absolute (no relative "paths" that an application would read as options),
//!   browser/media links must use allowed schemes, and the export's own executable is only
//!   used when it exists here on the same OS (otherwise this machine is searched);
//! - network locations in the file (`\\server\share`, `/net/…`) are never accessed while
//!   previewing or importing — on Windows that alone would send the user's credentials to the
//!   server — and the preview warns about them;
//! - parsing is bounded: 5 MB, structure limits checked in one pass before anything is
//!   deserialized, then every field limit.

pub mod limits;
pub mod sanitize;

mod command;
mod paths;
pub(crate) use paths::is_network_path;
mod secrets;
mod validate;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::Utc;
use reframe_platform::{
    LaunchTarget, LocateQuery, MonitorInfo, OsKind, PackageInfo, PackageKind, Platform,
};
use uuid::Uuid;

use crate::adapters::AdapterRegistry;
use crate::error::{CoreError, CoreResult};
use crate::model::{
    AppResolution, AppState, ApplicationEntry, EXPORT_FORMAT, EXPORT_VERSION, ImportApplication,
    ImportPreview, ImportResolution, LaunchConfig, MAX_DESCRIPTION_LEN, MAX_IMPORT_BYTES,
    MachineApplication, MachineProfile, PortableApplication, PortableWorkspace,
    WORKSPACE_SCHEMA_VERSION, WindowSnapshot, Workspace, WorkspaceExport,
};
use limits::{MAX_PATH_LEN, MAX_TEXT_LEN};
use sanitize::{clean_terminal_title, clean_text, clean_title};

/// At most this many alternative launch targets are offered per application.
const MAX_ALTERNATIVES: usize = 10;
/// At most this many applications are listed in the removed-arguments warning...
const MAX_APPS_IN_WARNING: usize = 5;
/// ...with at most this many arguments each...
const MAX_ARGS_IN_WARNING: usize = 6;
/// ...of at most this many characters.
const MAX_ARG_CHARS_IN_WARNING: usize = 60;

/// Build the export document: the logical workspace (state sanitized by each adapter's
/// `sanitize_for_export` and then by the default rules as a baseline, launch arguments by
/// [`sanitize::sanitize_arguments`], titles and the description cleaned of secret URLs and
/// credentials) plus the machine profile. Titles and names are cut to
/// [`limits::MAX_TEXT_LEN`] characters.
///
/// Fails with `CoreError::Validation` when the result would break an import rule that
/// sanitizing cannot fix (more than [`limits::MAX_ARGUMENTS`] launch arguments, an overlong
/// path, a file over 5 MB...), so the export can always be imported again.
pub fn export_workspace(
    ws: &Workspace,
    adapters: &AdapterRegistry,
    os: OsKind,
    app_version: &str,
) -> CoreResult<WorkspaceExport> {
    let mut applications = Vec::with_capacity(ws.applications.len());
    let mut machine_applications = Vec::with_capacity(ws.applications.len());
    for entry in &ws.applications {
        let adapter = adapters.get(&entry.adapter_id);
        let terminal = matches!(entry.state, AppState::Terminal { .. });
        applications.push(PortableApplication {
            id: entry.id,
            app_id: entry.app_id.clone(),
            display_name: clean_text(&entry.display_name, MAX_TEXT_LEN),
            adapter_id: entry.adapter_id.clone(),
            enabled: entry.enabled,
            arguments: sanitize::sanitize_arguments(&entry.launch.arguments),
            depends_on: entry.depends_on.clone(),
            state: export_state(&adapter.sanitize_for_export(&entry.state)),
            windows: entry
                .windows
                .iter()
                .map(|w| export_window(w, terminal))
                .collect(),
            options: entry.options.clone(),
        });
        machine_applications.push(MachineApplication {
            entry_id: entry.id,
            executable: entry.launch.executable.clone(),
            package: entry.launch.package.clone(),
            working_directory: entry.launch.working_directory.clone(),
        });
    }
    let export = WorkspaceExport {
        format: EXPORT_FORMAT.to_string(),
        version: EXPORT_VERSION,
        exported_at: Utc::now(),
        app_version: clean_text(app_version, MAX_TEXT_LEN),
        workspace: PortableWorkspace {
            name: ws.name.trim().to_string(),
            description: clean_title(&ws.description, MAX_DESCRIPTION_LEN),
            icon: ws.icon.clone(),
            color: ws.color.clone(),
            applications,
            restore: ws.restore.clone(),
        },
        machine: MachineProfile {
            os,
            monitors: ws.monitors.iter().map(export_monitor).collect(),
            applications: machine_applications,
        },
    };
    let problems = validate::validate_export(&export);
    if !problems.is_empty() {
        return Err(CoreError::Validation(
            problems
                .into_list()
                .into_iter()
                .map(|p| {
                    if p.starts_with('…') {
                        p
                    } else {
                        format!("This workspace cannot be exported: {p}.")
                    }
                })
                .collect(),
        ));
    }
    let size = serde_json::to_vec_pretty(&export)?.len();
    if size > MAX_IMPORT_BYTES {
        return Err(CoreError::validation(format!(
            "This workspace is too large to export ({:.1} MB; files over 5 MB cannot be imported).",
            size as f64 / (1024.0 * 1024.0)
        )));
    }
    Ok(export)
}

/// The baseline applied after the adapter's own `sanitize_for_export`, so an adapter that
/// sanitizes too little cannot leak what the default rules remove (it is idempotent, so it
/// changes nothing after an adapter that already used it). `Custom` data is left to the
/// adapter: only it knows what the data means, and the default drops it.
fn export_state(state: &AppState) -> AppState {
    match state {
        AppState::Custom { .. } => state.clone(),
        other => sanitize::sanitize_state(other),
    }
}

/// Saved windows with sanitized titles; terminal titles often show the running command line,
/// so they are redacted like startup commands.
fn export_window(window: &WindowSnapshot, terminal: bool) -> WindowSnapshot {
    WindowSnapshot {
        title: if terminal {
            clean_terminal_title(&window.title, MAX_TEXT_LEN)
        } else {
            clean_title(&window.title, MAX_TEXT_LEN)
        },
        class_name: window
            .class_name
            .as_deref()
            .map(|c| clean_text(c, MAX_TEXT_LEN)),
        bounds: window.bounds,
        state: window.state,
        monitor_id: window
            .monitor_id
            .as_deref()
            .map(|m| clean_text(m, MAX_TEXT_LEN)),
    }
}

fn export_monitor(monitor: &MonitorInfo) -> MonitorInfo {
    MonitorInfo {
        id: clean_text(&monitor.id, MAX_TEXT_LEN),
        name: clean_text(&monitor.name, MAX_TEXT_LEN),
        ..monitor.clone()
    }
}

/// Parse and strictly validate an export file (size limit, UTF-8, JSON syntax with line and
/// column, format/version, NUL characters, field limits, references). Malformed input yields
/// `CoreError::InvalidImport` with a clear, specific reason. Dependency *cycles* are reported
/// by the restoration planner when the import is committed.
pub fn parse_export(bytes: &[u8]) -> CoreResult<WorkspaceExport> {
    let text = validate::check_file(bytes).map_err(CoreError::InvalidImport)?;
    let export = validate::deserialize(text).map_err(CoreError::InvalidImport)?;
    ensure_valid(&export)?;
    Ok(export)
}

fn ensure_valid(export: &WorkspaceExport) -> CoreResult<()> {
    let problems = validate::validate_export(export);
    if problems.is_empty() {
        Ok(())
    } else {
        Err(CoreError::InvalidImport(problems.summary()))
    }
}

/// Resolve applications and paths on this machine for the mapping screen.
///
/// Per application: when the export comes from the same OS and its executable exists (or its
/// package resolves), the resolution is `Exported`; otherwise the adapter's locate query is
/// run (falling back to the exported executable's file name) and the first candidate is
/// `Found`, the others become alternatives; otherwise `Unresolved`. `missing_paths` lists
/// state paths and the working directory that do not exist here.
pub fn preview_import(
    export: &WorkspaceExport,
    file_path: &str,
    platform: &dyn Platform,
    adapters: &AdapterRegistry,
) -> CoreResult<ImportPreview> {
    ensure_valid(export)?;
    let here = platform.info().os;
    let same_os = export.machine.os == here;
    let mut applications = Vec::with_capacity(export.workspace.applications.len());
    let mut all_missing = Vec::new();
    let mut seen = HashSet::new();
    for app in &export.workspace.applications {
        let machine = machine_entry(export, app.id);
        let (resolution, alternatives) = resolve(app, machine, same_os, platform, adapters);
        let missing_paths = missing_paths(app, machine);
        for path in &missing_paths {
            if seen.insert(path.clone()) {
                all_missing.push(path.clone());
            }
        }
        applications.push(ImportApplication {
            entry_id: app.id,
            app_id: app.app_id.clone(),
            display_name: app.display_name.clone(),
            adapter_id: app.adapter_id.clone(),
            resolution,
            alternatives,
            missing_paths,
        });
    }
    let suggested_path_mappings =
        paths::suggest_mappings(&all_missing, dirs::home_dir().as_deref());
    let warnings = import_warnings(
        export,
        &applications,
        all_missing.len(),
        !suggested_path_mappings.is_empty(),
        here,
    );
    Ok(ImportPreview {
        file_path: file_path.to_string(),
        name: export.workspace.name.trim().to_string(),
        description: export.workspace.description.clone(),
        icon: export.workspace.icon.clone(),
        source_os: export.machine.os,
        exported_at: export.exported_at,
        applications,
        suggested_path_mappings,
        warnings,
    })
}

fn machine_entry(export: &WorkspaceExport, entry_id: Uuid) -> Option<&MachineApplication> {
    export
        .machine
        .applications
        .iter()
        .find(|m| m.entry_id == entry_id)
}

/// The automatic resolution of one application and the other candidates found.
fn resolve(
    app: &PortableApplication,
    machine: Option<&MachineApplication>,
    same_os: bool,
    platform: &dyn Platform,
    adapters: &AdapterRegistry,
) -> (AppResolution, Vec<LaunchTarget>) {
    if same_os && let Some(target) = machine.and_then(|m| exported_target(m, platform)) {
        return (AppResolution::Exported { target }, Vec::new());
    }
    let query = adapters.get(&app.adapter_id).locate_query(&app.app_id);
    let mut candidates = locate(platform, &query);
    if candidates.is_empty()
        && let Some(executable) = machine.and_then(|m| m.executable.as_deref())
    {
        candidates = locate(platform, &executable_query(executable));
    }
    let mut candidates = candidates.into_iter();
    match candidates.next() {
        Some(target) => (
            AppResolution::Found { target },
            candidates.take(MAX_ALTERNATIVES).collect(),
        ),
        None => (AppResolution::Unresolved, Vec::new()),
    }
}

/// The exported package (when it resolves here) or executable (when it exists here).
fn exported_target(machine: &MachineApplication, platform: &dyn Platform) -> Option<LaunchTarget> {
    if let Some(target) = machine
        .package
        .as_ref()
        .and_then(|package| resolve_package(package, platform))
    {
        return Some(target);
    }
    machine
        .executable
        .as_ref()
        .filter(|exe| paths::is_file_here(exe))
        .map(|exe| LaunchTarget::Executable { path: exe.clone() })
}

/// The exported package when it is installed here: its path exists (bundles, AppImages), or
/// the platform locates the same package id (Flatpak/Snap via their desktop ids, macOS bundle
/// ids) — then the package *as found on this machine* is used, never the file's description
/// of it. Windows packages have no path and no locate key, so they are never assumed to
/// resolve; the adapter's locate query gets a chance instead.
fn resolve_package(package: &PackageInfo, platform: &dyn Platform) -> Option<LaunchTarget> {
    if let Some(path) = &package.path {
        return paths::exists_here(path).then(|| LaunchTarget::Package {
            package: package.clone(),
        });
    }
    let mut query = LocateQuery::default();
    match package.kind {
        PackageKind::MacBundle => query.bundle_ids.push(package.id.clone()),
        PackageKind::Flatpak | PackageKind::Snap => {
            query.desktop_ids.push(format!("{}.desktop", package.id));
        }
        PackageKind::WindowsPackage | PackageKind::AppImage => return None,
    }
    platform
        .locate_application(&query)
        .into_iter()
        .find(|target| {
            matches!(target, LaunchTarget::Package { package: found }
                if found.kind == package.kind && found.id.eq_ignore_ascii_case(&package.id))
        })
}

/// Launchable candidates for `query`, deduplicated. `Open` targets are left out: an
/// application entry cannot store them.
fn locate(platform: &dyn Platform, query: &LocateQuery) -> Vec<LaunchTarget> {
    let empty = query.executable_names.is_empty()
        && query.bundle_ids.is_empty()
        && query.desktop_ids.is_empty()
        && query.display_names.is_empty();
    if empty {
        return Vec::new();
    }
    let mut found = Vec::new();
    for target in platform.locate_application(query) {
        if !matches!(target, LaunchTarget::Open { .. }) && !found.contains(&target) {
            found.push(target);
        }
    }
    found
}

/// Look up the exported executable's file name (`C:\…\gimp-2.10.exe` → `gimp-2.10`).
fn executable_query(executable: &Path) -> LocateQuery {
    let text = executable.to_string_lossy();
    let file_name = text.rsplit(['/', '\\']).next().unwrap_or_default().trim();
    let mut query = LocateQuery::default();
    if file_name.is_empty() {
        return query;
    }
    let stem = file_name
        .len()
        .checked_sub(4)
        .and_then(|start| file_name.get(start..))
        .filter(|ext| ext.eq_ignore_ascii_case(".exe"))
        .map_or(file_name, |ext| &file_name[..file_name.len() - ext.len()]);
    let names = if OsKind::current() == OsKind::Windows {
        [format!("{stem}.exe"), stem.to_string()]
    } else {
        [stem.to_string(), file_name.to_string()]
    };
    for name in names {
        if !name.is_empty()
            && !query
                .executable_names
                .iter()
                .any(|n| n.eq_ignore_ascii_case(&name))
        {
            query.executable_names.push(name);
        }
    }
    query
}

/// State paths and the working directory that do not exist on this machine. Network
/// locations are never checked (see `paths::exists_here`), so they are not reported as
/// missing; a warning mentions them instead.
fn missing_paths(app: &PortableApplication, machine: Option<&MachineApplication>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    app.state
        .paths()
        .into_iter()
        .chain(machine.and_then(|m| m.working_directory.clone()))
        .filter(|path| {
            !paths::is_network_path(path) && !paths::exists_here(path) && seen.insert(path.clone())
        })
        .collect()
}

fn import_warnings(
    export: &WorkspaceExport,
    applications: &[ImportApplication],
    missing_paths: usize,
    has_suggestions: bool,
    here: OsKind,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if export.machine.os != here {
        warnings.push(format!(
            "This workspace was exported on {}: applications and folders were matched to this \
             computer where possible, and window positions are adapted to your displays.",
            os_name(export.machine.os)
        ));
    }
    let unresolved = applications
        .iter()
        .filter(|a| a.resolution == AppResolution::Unresolved)
        .count();
    if unresolved == 1 {
        warnings.push(
            "1 application could not be found on this computer — choose its executable or \
             skip it"
                .to_string(),
        );
    } else if unresolved > 1 {
        warnings.push(format!(
            "{unresolved} applications could not be found on this computer — choose their \
             executables or skip them"
        ));
    }
    let commands: usize = export
        .workspace
        .applications
        .iter()
        .map(|a| startup_command_count(&a.state))
        .sum();
    if commands == 1 {
        warnings.push(
            "1 startup command was imported switched off — review it before enabling".to_string(),
        );
    } else if commands > 1 {
        warnings.push(format!(
            "{commands} startup commands were imported switched off — review them before enabling"
        ));
    }
    if let Some(warning) = removed_arguments_warning(&export.workspace.applications) {
        warnings.push(warning);
    }
    let network = network_locations(export);
    if network > 0 {
        warnings.push(format!(
            "{network} {} to other computers (such as \\\\server\\share) — Reframe did not \
             access {}; restore this workspace only if you trust where the file came from",
            if network == 1 {
                "path or link points"
            } else {
                "paths or links point"
            },
            if network == 1 { "it" } else { "them" }
        ));
    }
    if missing_paths > 0 {
        let hint = if has_suggestions {
            "review the suggested folder mappings"
        } else {
            "add folder mappings or update them after importing"
        };
        warnings.push(format!(
            "{missing_paths} folder{} or file{} used by this workspace {} not exist on this \
             computer — {hint}; working directories that still do not exist are left out",
            if missing_paths == 1 { "" } else { "s" },
            if missing_paths == 1 { "" } else { "s" },
            if missing_paths == 1 { "does" } else { "do" },
        ));
    }
    warnings
}

/// "Custom launch arguments were removed for safety: Terminal: -e "rm -rf ~"; add them again
/// in the workspace editor if you trust them" — long lists are shortened.
fn removed_arguments_warning(applications: &[PortableApplication]) -> Option<String> {
    let with_arguments: Vec<&PortableApplication> = applications
        .iter()
        .filter(|a| !a.arguments.is_empty())
        .collect();
    if with_arguments.is_empty() {
        return None;
    }
    let mut listed: Vec<String> = with_arguments
        .iter()
        .take(MAX_APPS_IN_WARNING)
        .map(|app| {
            let mut shown: Vec<String> = app
                .arguments
                .iter()
                .take(MAX_ARGS_IN_WARNING)
                .map(|arg| display_argument(arg))
                .collect();
            if app.arguments.len() > MAX_ARGS_IN_WARNING {
                shown.push("…".to_string());
            }
            format!(
                "{}: {}",
                clean_text(app.display_name.trim(), 40),
                shown.join(" ")
            )
        })
        .collect();
    let unlisted = with_arguments.len().saturating_sub(MAX_APPS_IN_WARNING);
    if unlisted > 0 {
        listed.push(format!(
            "and {unlisted} more application{}",
            if unlisted == 1 { "" } else { "s" }
        ));
    }
    Some(format!(
        "Custom launch arguments were removed for safety: {}; add them again in the workspace \
         editor if you trust them",
        listed.join("; ")
    ))
}

/// An argument for display: plain when it is simple, otherwise quoted with escapes (so spaces,
/// quotes and invisible characters show), cut to [`MAX_ARG_CHARS_IN_WARNING`] characters.
fn display_argument(arg: &str) -> String {
    let short: String = arg.chars().take(MAX_ARG_CHARS_IN_WARNING).collect();
    let cut = if arg.chars().nth(MAX_ARG_CHARS_IN_WARNING).is_some() {
        "…"
    } else {
        ""
    };
    let simple = !short.is_empty()
        && short
            .chars()
            .all(|c| c.is_ascii_graphic() && c != '"' && c != '\'' && c != '\\');
    if simple {
        format!("{short}{cut}")
    } else {
        format!("{short:?}{cut}")
    }
}

/// Paths and links in the file that point to other computers: UNC paths, automount folders,
/// `file://server/…` links (see `paths::is_network_path`).
fn network_locations(export: &WorkspaceExport) -> usize {
    let mut count = 0;
    for app in &export.workspace.applications {
        count += app
            .state
            .paths()
            .iter()
            .filter(|p| paths::is_network_path(p))
            .count();
        match &app.state {
            AppState::Browser { windows, .. } => {
                count += windows
                    .iter()
                    .flat_map(|w| &w.tabs)
                    .filter(|t| paths::is_remote_file_url(&t.url))
                    .count();
            }
            AppState::Media { uri: Some(uri), .. } if paths::is_remote_file_url(uri) => count += 1,
            _ => {}
        }
    }
    for machine in &export.machine.applications {
        let package_path = machine.package.as_ref().and_then(|p| p.path.as_ref());
        count += [
            machine.executable.as_ref(),
            machine.working_directory.as_ref(),
            package_path,
        ]
        .into_iter()
        .flatten()
        .filter(|p| paths::is_network_path(p))
        .count();
    }
    count
}

fn startup_command_count(state: &AppState) -> usize {
    match state {
        AppState::Terminal { sessions } => sessions
            .iter()
            .filter(|s| {
                s.startup_command
                    .as_deref()
                    .is_some_and(|c| !c.trim().is_empty())
            })
            .count(),
        _ => 0,
    }
}

fn os_name(os: OsKind) -> &'static str {
    match os {
        OsKind::Windows => "Windows",
        OsKind::Macos => "macOS",
        OsKind::Linux => "Linux",
        OsKind::Other => "another operating system",
    }
}

/// Create a new workspace (fresh workspace and entry ids) from an export and the user's
/// mapping decisions:
/// - skipped applications are left out, together with dependencies on them;
/// - an explicit target sets the executable or package; otherwise the automatic resolution
///   of [`preview_import`] applies (unresolved applications keep no launch target and must be
///   configured before they can be restored);
/// - folder mappings (longest prefix first, separator-aware, case-insensitive for Windows-style
///   prefixes, `~` = home folder) apply to state paths and working directories; a working
///   directory that still does not exist is left out, since launching in it would fail;
/// - imported launch arguments are dropped and terminal startup commands switched off;
/// - name override (trimmed; blank keeps the exported name), timestamps now, never restored.
///
/// Fails with `CoreError::Validation` for invalid decisions (empty folder mapping, an "open"
/// target) or when the result breaks a workspace rule.
pub fn build_imported_workspace(
    export: &WorkspaceExport,
    resolution: &ImportResolution,
    platform: &dyn Platform,
    adapters: &AdapterRegistry,
) -> CoreResult<Workspace> {
    ensure_valid(export)?;
    let mappings = paths::compile_mappings(&resolution.path_mappings, dirs::home_dir().as_deref())
        .map_err(CoreError::Validation)?;
    let same_os = export.machine.os == platform.info().os;

    let mut new_ids: HashMap<Uuid, Uuid> = HashMap::new();
    let mut entries: Vec<(ApplicationEntry, &[Uuid])> = Vec::new();
    for app in &export.workspace.applications {
        let decision = resolution
            .app_mappings
            .iter()
            .find(|m| m.entry_id == app.id);
        if decision.is_some_and(|d| d.skip) {
            continue;
        }
        let machine = machine_entry(export, app.id);
        let (executable, package) = match decision.and_then(|d| d.target.as_ref()) {
            Some(target) => explicit_target(target, &app.display_name)?,
            None => automatic_target(app, machine, same_os, platform, adapters),
        };
        // A working directory that does not exist here is left out (launching in it would
        // fail); one on a network share is kept unverified (see `paths::exists_here`).
        let working_directory = machine
            .and_then(|m| m.working_directory.as_ref())
            .map(|dir| paths::map_path(dir, &mappings).unwrap_or_else(|| dir.clone()))
            .filter(|dir| paths::is_network_path(dir) || paths::is_dir_here(dir));
        let mut state = app.state.clone();
        state.map_paths(|path| paths::map_path(path, &mappings));
        switch_off_startup_commands(&mut state);
        let id = Uuid::new_v4();
        new_ids.insert(app.id, id);
        entries.push((
            ApplicationEntry {
                id,
                app_id: app.app_id.clone(),
                display_name: app.display_name.trim().to_string(),
                adapter_id: app.adapter_id.clone(),
                enabled: app.enabled,
                launch: LaunchConfig {
                    executable,
                    package,
                    // Launch arguments from a file are untrusted command-line input — a
                    // terminal given `-e <command>` would run it on restore without approval —
                    // so they are never applied; the preview lists what was removed.
                    arguments: Vec::new(),
                    working_directory,
                },
                depends_on: Vec::new(),
                state,
                windows: app.windows.clone(),
                options: app.options.clone(),
            },
            &app.depends_on,
        ));
    }
    let applications = entries
        .into_iter()
        .map(|(mut entry, dependencies)| {
            for dependency in dependencies {
                if let Some(&new_id) = new_ids.get(dependency)
                    && !entry.depends_on.contains(&new_id)
                {
                    entry.depends_on.push(new_id);
                }
            }
            entry
        })
        .collect();

    let now = Utc::now();
    let name = resolution
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| export.workspace.name.trim())
        .to_string();
    let ws = Workspace {
        id: Uuid::new_v4(),
        name,
        description: export.workspace.description.trim().to_string(),
        icon: export.workspace.icon.clone(),
        color: export.workspace.color.clone(),
        created_at: now,
        updated_at: now,
        last_restored_at: None,
        applications,
        monitors: export.machine.monitors.clone(),
        restore: export.workspace.restore.clone(),
        schema_version: WORKSPACE_SCHEMA_VERSION,
    };
    ws.validate().map_err(CoreError::Validation)?;
    Ok(ws)
}

type LaunchFields = (Option<PathBuf>, Option<PackageInfo>);

fn explicit_target(target: &LaunchTarget, display_name: &str) -> CoreResult<LaunchFields> {
    let invalid_path = |path: &Path| {
        let text = path.to_string_lossy();
        text.trim().is_empty() || text.contains('\0') || text.chars().count() > MAX_PATH_LEN
    };
    match target {
        LaunchTarget::Executable { path } => {
            if invalid_path(path) {
                return Err(CoreError::validation(format!(
                    "The executable chosen for {display_name} is not a valid path."
                )));
            }
            Ok((Some(path.clone()), None))
        }
        LaunchTarget::Package { package } => {
            let invalid = package.id.trim().is_empty()
                || package.id.contains('\0')
                || package.id.chars().count() > MAX_TEXT_LEN
                || package.path.as_deref().is_some_and(invalid_path);
            if invalid {
                return Err(CoreError::validation(format!(
                    "The application package chosen for {display_name} is not valid."
                )));
            }
            Ok((None, Some(package.clone())))
        }
        LaunchTarget::Open { .. } => Err(CoreError::validation(format!(
            "Choose an executable or an installed application for {display_name}: opening a \
             file or link cannot start it."
        ))),
    }
}

fn automatic_target(
    app: &PortableApplication,
    machine: Option<&MachineApplication>,
    same_os: bool,
    platform: &dyn Platform,
    adapters: &AdapterRegistry,
) -> LaunchFields {
    match resolve(app, machine, same_os, platform, adapters).0 {
        AppResolution::Exported {
            target: LaunchTarget::Package { package },
        } => {
            // Keep the exported executable as well when it exists (e.g. inside a bundle).
            let executable = machine
                .and_then(|m| m.executable.clone())
                .filter(|exe| paths::is_file_here(exe));
            (executable, Some(package))
        }
        AppResolution::Exported { target } | AppResolution::Found { target } => match target {
            LaunchTarget::Executable { path } => (Some(path), None),
            LaunchTarget::Package { package } => (None, Some(package)),
            LaunchTarget::Open { .. } => (None, None),
        },
        AppResolution::Unresolved => (None, None),
    }
}

fn switch_off_startup_commands(state: &mut AppState) {
    if let AppState::Terminal { sessions } = state {
        for session in sessions {
            session.startup_command_approved = false;
        }
    }
}
