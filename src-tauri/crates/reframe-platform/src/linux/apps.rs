//! Installed applications from freedesktop `.desktop` files: lookup for
//! [`Platform::locate_application`](crate::Platform::locate_application), friendly product
//! names for processes and icon names.
//!
//! Entries are read from `$XDG_DATA_HOME/applications`, every `$XDG_DATA_DIRS/applications`,
//! the Flatpak export directories and `/var/lib/snapd/desktop/applications`, with the usual
//! precedence (the first file with a given desktop id wins; `Hidden=true` deletes an id). The
//! parsed index is cached and rebuilt when one of those directories changes (or after a few
//! minutes, which also catches edits in sub directories).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use super::desktop_entry::{self, DesktopEntry, ExecCommand};
use super::xdg::XdgDirs;
use crate::{LaunchTarget, LocateQuery, PackageInfo, PackageKind, ProcessInfo};

/// `.desktop` files larger than this are skipped.
const MAX_DESKTOP_FILE: u64 = 256 * 1024;
/// Maximum sub directory depth scanned below an `applications` directory.
const MAX_DEPTH: usize = 3;
/// Rebuild the cached index at least this often.
const MAX_INDEX_AGE: Duration = Duration::from_secs(180);

/// Application directories in precedence order (duplicates removed).
pub(crate) fn application_dirs(xdg: &XdgDirs) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut push = |p: PathBuf| {
        if !dirs.contains(&p) {
            dirs.push(p);
        }
    };
    if let Some(data_home) = &xdg.data_home {
        push(data_home.join("applications"));
    }
    for dir in &xdg.data_dirs {
        push(dir.join("applications"));
    }
    if let Some(home) = &xdg.home {
        push(home.join(".local/share/flatpak/exports/share/applications"));
    }
    push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    push(PathBuf::from("/var/lib/snapd/desktop/applications"));
    dirs
}

/// A parsed application entry with its interpreted `Exec` line.
#[derive(Debug, Clone)]
pub(crate) struct AppEntry {
    pub entry: DesktopEntry,
    pub command: Option<ExecCommand>,
}

impl AppEntry {
    fn new(entry: DesktopEntry) -> Self {
        let command = entry.exec.as_deref().and_then(desktop_entry::classify_exec);
        Self { entry, command }
    }

    /// Flatpak application id (`X-Flatpak` or the id in `flatpak run <id>`).
    pub fn flatpak_id(&self) -> Option<&str> {
        self.entry.flatpak_id.as_deref().or(match &self.command {
            Some(ExecCommand::Flatpak { app_id, .. }) => Some(app_id.as_str()),
            _ => None,
        })
    }

    /// Snap name (`X-SnapInstanceName` or `/snap/bin/<name>[.<app>]`).
    pub fn snap_name(&self) -> Option<&str> {
        self.entry.snap_instance.as_deref().or(match &self.command {
            Some(ExecCommand::Snap { name, .. }) => name.split('.').next(),
            _ => None,
        })
    }

    /// The program of a plain `Exec` line.
    pub fn program(&self) -> Option<&str> {
        match &self.command {
            Some(ExecCommand::Program { program, .. }) => Some(program.as_str()),
            _ => None,
        }
    }

    /// Whether the `Exec` line passes fixed arguments (used to prefer an app's main entry).
    fn has_fixed_args(&self) -> bool {
        match &self.command {
            Some(ExecCommand::Program { args, .. })
            | Some(ExecCommand::Flatpak { args, .. })
            | Some(ExecCommand::Snap { args, .. }) => !args.is_empty(),
            None => true,
        }
    }

    /// Ranking key: visible entries without fixed arguments first.
    fn preference(&self) -> (bool, bool) {
        (self.entry.no_display, self.has_fixed_args())
    }
}

fn base_name(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
}

/// Generic interpreters: the script they run identifies the application, not the binary.
const INTERPRETERS: &[&str] = &[
    "python", "python2", "python3", "perl", "ruby", "node", "nodejs", "java", "sh", "bash", "dash",
    "zsh", "env", "mono", "gjs", "lua", "php", "wish", "tclsh",
];

pub(crate) fn is_interpreter(name: &str) -> bool {
    INTERPRETERS.contains(&name)
        || name
            .strip_prefix("python")
            .is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit() || b == b'.'))
}

/// Parsed `.desktop` files, in precedence order, with lookup tables.
#[derive(Debug, Default)]
pub(crate) struct DesktopIndex {
    entries: Vec<AppEntry>,
    by_id: HashMap<String, usize>,
    by_program_base: HashMap<String, Vec<usize>>,
    by_program_path: HashMap<String, Vec<usize>>,
}

impl DesktopIndex {
    /// Scan the given `applications` directories.
    pub fn scan(dirs: &[PathBuf]) -> Self {
        let mut seen = HashSet::new();
        let mut entries = Vec::new();
        for dir in dirs {
            scan_dir(dir, "", 0, &mut seen, &mut entries);
        }
        Self::from_entries(entries)
    }

    pub fn from_entries(entries: Vec<DesktopEntry>) -> Self {
        let mut index = DesktopIndex::default();
        for entry in entries {
            let i = index.entries.len();
            let app = AppEntry::new(entry);
            index.by_id.entry(app.entry.id.to_lowercase()).or_insert(i);
            if let Some(program) = app.program() {
                index
                    .by_program_base
                    .entry(base_name(program).to_string())
                    .or_default()
                    .push(i);
                if program.starts_with('/') {
                    index
                        .by_program_path
                        .entry(program.to_string())
                        .or_default()
                        .push(i);
                    // Also index the resolved path (symlinked binaries, merged /usr).
                    if let Some(real) = fs::canonicalize(program)
                        .ok()
                        .and_then(|p| p.to_str().map(str::to_string))
                        .filter(|p| p != program)
                    {
                        index.by_program_path.entry(real).or_default().push(i);
                    }
                }
            }
            index.entries.push(app);
        }
        index
    }

    pub fn entries(&self) -> &[AppEntry] {
        &self.entries
    }

    pub fn by_id(&self, id: &str) -> Option<&AppEntry> {
        let id = id.trim().to_lowercase();
        let id = if id.ends_with(".desktop") {
            id
        } else {
            format!("{id}.desktop")
        };
        self.by_id.get(&id).map(|&i| &self.entries[i])
    }

    fn best<'a>(&'a self, mut candidates: Vec<&'a AppEntry>) -> Option<&'a AppEntry> {
        // Stable sort keeps precedence order among equally preferred entries.
        candidates.sort_by_key(|e| e.preference());
        candidates.into_iter().next()
    }

    fn by_flatpak(&self, app_id: &str) -> Option<&AppEntry> {
        if let Some(e) = self.by_id(app_id).filter(|e| e.flatpak_id().is_some()) {
            return Some(e);
        }
        self.best(
            self.entries
                .iter()
                .filter(|e| e.flatpak_id() == Some(app_id))
                .collect(),
        )
    }

    fn by_snap(&self, name: &str) -> Option<&AppEntry> {
        let snap = name.split('.').next().unwrap_or(name);
        if let Some(e) = self.by_id(&format!("{snap}_{snap}.desktop")) {
            return Some(e);
        }
        self.best(
            self.entries
                .iter()
                .filter(|e| e.snap_name() == Some(snap))
                .collect(),
        )
    }

    /// Entries whose plain `Exec` program is `path` (absolute match) or, for `Exec` lines
    /// naming a bare program, has the same file name.
    fn by_program(&self, path: &str) -> Option<&AppEntry> {
        if let Some(found) = self.by_program_path.get(path) {
            return self.best(found.iter().map(|&i| &self.entries[i]).collect());
        }
        let base = base_name(path);
        let found = self.by_program_base.get(base)?;
        self.best(
            found
                .iter()
                .map(|&i| &self.entries[i])
                .filter(|e| e.program().is_some_and(|p| !p.contains('/')))
                .collect(),
        )
    }

    fn by_package(&self, package: &PackageInfo) -> Option<&AppEntry> {
        match package.kind {
            PackageKind::Flatpak => self.by_flatpak(&package.id),
            PackageKind::Snap => self.by_snap(&package.id),
            PackageKind::AppImage => package
                .path
                .as_deref()
                .and_then(Path::to_str)
                .and_then(|p| self.by_program_path.get(p))
                .and_then(|found| self.best(found.iter().map(|&i| &self.entries[i]).collect())),
            PackageKind::MacBundle | PackageKind::WindowsPackage => None,
        }
    }

    fn by_wm_class(&self, class: &str) -> Option<&AppEntry> {
        let wanted = class.trim();
        if wanted.is_empty() {
            return None;
        }
        self.best(
            self.entries
                .iter()
                .filter(|e| {
                    e.entry
                        .startup_wm_class
                        .as_deref()
                        .is_some_and(|c| c.eq_ignore_ascii_case(wanted))
                })
                .collect(),
        )
        .or_else(|| self.by_id(&wanted.to_lowercase()))
    }

    /// The entry describing a process: by package, else by executable (or, for interpreters,
    /// the script they run).
    pub fn entry_for_process(&self, process: &ProcessInfo) -> Option<&AppEntry> {
        if let Some(package) = &process.package {
            return self.by_package(package);
        }
        let exe = process.executable_path.as_deref()?.to_str()?;
        if !is_interpreter(base_name(exe)) {
            return self.by_program(exe);
        }
        let script = process
            .command_line
            .iter()
            .skip(1)
            .find(|a| !a.starts_with('-'))?;
        if is_interpreter(base_name(script)) {
            return None;
        }
        self.by_program(script)
    }

    /// Friendly product name (`Name=`) for a process.
    pub fn product_name(&self, process: &ProcessInfo) -> Option<String> {
        self.entry_for_process(process)
            .and_then(|e| e.entry.name.clone())
    }

    /// The best entry to take an icon from.
    pub fn entry_for_icon(
        &self,
        package: Option<&PackageInfo>,
        executable: Option<&Path>,
        wm_class: Option<&str>,
    ) -> Option<&AppEntry> {
        let with_icon = |e: &&AppEntry| e.entry.icon.is_some();
        package
            .and_then(|p| self.by_package(p))
            .filter(with_icon)
            .or_else(|| {
                executable
                    .and_then(Path::to_str)
                    .and_then(|exe| self.by_program(exe))
                    .filter(with_icon)
            })
            .or_else(|| wm_class.and_then(|c| self.by_wm_class(c)).filter(with_icon))
    }
}

fn scan_dir(
    dir: &Path,
    prefix: &str,
    depth: usize,
    seen: &mut HashSet<String>,
    out: &mut Vec<DesktopEntry>,
) {
    let Ok(read) = fs::read_dir(dir) else {
        return;
    };
    let mut items: Vec<_> = read.flatten().collect();
    // Deterministic order (read_dir order is file-system dependent).
    items.sort_by_key(|e| e.file_name());
    for item in items {
        let name = item.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let path = item.path();
        let Ok(file_type) = item.file_type() else {
            continue;
        };
        let is_dir = file_type.is_dir() || (file_type.is_symlink() && path.is_dir());
        if is_dir {
            if depth < MAX_DEPTH && !name.starts_with('.') {
                scan_dir(&path, &format!("{prefix}{name}-"), depth + 1, seen, out);
            }
            continue;
        }
        if !name.ends_with(".desktop") {
            continue;
        }
        let id = format!("{prefix}{name}");
        if seen.contains(&id) {
            continue;
        }
        let Some(mut entry) = read_entry(&path) else {
            continue;
        };
        seen.insert(id.clone());
        if entry.hidden || !entry.is_application {
            continue;
        }
        entry.id = id;
        entry.path = path;
        out.push(entry);
    }
}

fn read_entry(path: &Path) -> Option<DesktopEntry> {
    let file = fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > MAX_DESKTOP_FILE {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_DESKTOP_FILE).read_to_end(&mut bytes).ok()?;
    desktop_entry::parse(&String::from_utf8_lossy(&bytes))
}

/// Resolve a program name or path to an existing executable file.
fn resolve_program(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        let path = PathBuf::from(program);
        return (path.is_absolute() && is_executable_file(&path)).then_some(path);
    }
    which::which(program).ok()
}

fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// How to launch an entry, if it is installed and representable as a [`LaunchTarget`].
/// Entries whose `Exec` is a shell command line, or whose program/`TryExec` cannot be
/// found, yield `None`. Fixed `Exec` arguments are not part of a launch target and are lost.
pub(crate) fn launch_target(app: &AppEntry) -> Option<LaunchTarget> {
    if let Some(try_exec) = &app.entry.try_exec
        && resolve_program(try_exec).is_none()
    {
        return None;
    }
    if let Some(id) = app.flatpak_id() {
        return Some(LaunchTarget::Package {
            package: PackageInfo {
                kind: PackageKind::Flatpak,
                id: id.to_string(),
                path: None,
            },
        });
    }
    match app.command.as_ref()? {
        ExecCommand::Snap { name, .. } => Some(LaunchTarget::Package {
            package: PackageInfo {
                kind: PackageKind::Snap,
                id: name.clone(),
                path: None,
            },
        }),
        ExecCommand::Program { program, .. } => {
            resolve_program(program).map(|path| LaunchTarget::Executable { path })
        }
        ExecCommand::Flatpak { .. } => None, // handled through flatpak_id() above
    }
}

fn same_target(a: &LaunchTarget, b: &LaunchTarget) -> bool {
    match (a, b) {
        (LaunchTarget::Executable { path: pa }, LaunchTarget::Executable { path: pb }) => {
            pa == pb
                || match (fs::canonicalize(pa), fs::canonicalize(pb)) {
                    (Ok(ca), Ok(cb)) => ca == cb,
                    _ => false,
                }
        }
        (LaunchTarget::Package { package: pa }, LaunchTarget::Package { package: pb }) => {
            pa.kind == pb.kind && pa.id.eq_ignore_ascii_case(&pb.id) && pa.path == pb.path
        }
        _ => a == b,
    }
}

/// Launch targets for a query, most likely first, without duplicates:
/// 1. entries whose desktop id matches `desktop_ids`,
/// 2. entries whose `Name=` matches `display_names` (case-insensitive),
/// 3. `path_hits` (executables found on `PATH`),
/// 4. entries whose `Exec` program, Flatpak `--command` or Snap name matches
///    `executable_names`.
pub(crate) fn locate(
    index: &DesktopIndex,
    query: &LocateQuery,
    path_hits: &[PathBuf],
) -> Vec<LaunchTarget> {
    let mut out: Vec<LaunchTarget> = Vec::new();
    let mut push = |target: LaunchTarget| {
        if !out.iter().any(|t| same_target(t, &target)) {
            out.push(target);
        }
    };

    for id in &query.desktop_ids {
        if let Some(target) = index.by_id(id).and_then(launch_target) {
            push(target);
        }
    }
    for name in &query.display_names {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let mut matches: Vec<&AppEntry> = index
            .entries()
            .iter()
            .filter(|e| {
                e.entry
                    .name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
            })
            .collect();
        matches.sort_by_key(|e| e.preference());
        for target in matches.into_iter().filter_map(launch_target) {
            push(target);
        }
    }
    for path in path_hits {
        push(LaunchTarget::Executable { path: path.clone() });
    }
    for exe in &query.executable_names {
        let exe = exe.trim();
        if exe.is_empty() {
            continue;
        }
        let exe_base = base_name(exe);
        let mut matches: Vec<&AppEntry> = index
            .entries()
            .iter()
            .filter(|e| match &e.command {
                Some(ExecCommand::Program { program, .. }) => {
                    program == exe || base_name(program) == exe_base
                }
                Some(ExecCommand::Flatpak { command, .. }) => {
                    command.as_deref().is_some_and(|c| base_name(c) == exe_base)
                }
                Some(ExecCommand::Snap { name, .. }) => {
                    name == exe_base || name.split('.').next() == Some(exe_base)
                }
                None => false,
            })
            .collect();
        matches.sort_by_key(|e| e.preference());
        for target in matches.into_iter().filter_map(launch_target) {
            push(target);
        }
    }
    out
}

struct CachedIndex {
    index: Arc<DesktopIndex>,
    built: Instant,
    stamps: Vec<Option<SystemTime>>,
}

/// Lazily built, automatically refreshed [`DesktopIndex`].
pub(crate) struct DesktopCache {
    dirs: Vec<PathBuf>,
    state: Mutex<Option<CachedIndex>>,
}

impl DesktopCache {
    pub fn new(dirs: Vec<PathBuf>) -> Self {
        Self {
            dirs,
            state: Mutex::new(None),
        }
    }

    pub fn get(&self) -> Arc<DesktopIndex> {
        let stamps: Vec<Option<SystemTime>> = self
            .dirs
            .iter()
            .map(|d| fs::metadata(d).and_then(|m| m.modified()).ok())
            .collect();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(cached) = state.as_ref()
            && cached.stamps == stamps
            && cached.built.elapsed() < MAX_INDEX_AGE
        {
            return Arc::clone(&cached.index);
        }
        let index = Arc::new(DesktopIndex::scan(&self.dirs));
        *state = Some(CachedIndex {
            index: Arc::clone(&index),
            built: Instant::now(),
            stamps,
        });
        index
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::testutil::TempDir;

    fn process(exe: Option<&str>, cmd: &[&str], package: Option<PackageInfo>) -> ProcessInfo {
        ProcessInfo {
            pid: 42,
            parent_pid: None,
            name: "p".into(),
            product_name: None,
            executable_path: exe.map(PathBuf::from),
            command_line: cmd.iter().map(|s| s.to_string()).collect(),
            working_directory: None,
            start_time: None,
            package,
        }
    }

    /// A fake installation: `bin/` holds executables, `high/` and `low/` are application
    /// directories in precedence order.
    fn fixture() -> (TempDir, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new("apps");
        let bin = tmp.path().join("bin");
        for name in ["editor", "viewer", "helper"] {
            let p = tmp.write(&format!("bin/{name}"), "#!/bin/sh\n");
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let editor = bin.join("editor").display().to_string();
        let viewer = bin.join("viewer").display().to_string();
        tmp.write(
            "high/applications/editor.desktop",
            format!(
                "[Desktop Entry]\nType=Application\nName=Super Editor\nExec={editor} %F\nIcon=editor-icon\nStartupWMClass=SuperEditor\n"
            ),
        );
        // Lower precedence duplicate id: ignored.
        tmp.write(
            "low/applications/editor.desktop",
            "[Desktop Entry]\nType=Application\nName=Old Editor\nExec=/old/editor\n",
        );
        tmp.write(
            "low/applications/editor-private.desktop",
            format!(
                "[Desktop Entry]\nType=Application\nName=Super Editor (private)\nExec={editor} --private\nNoDisplay=true\n"
            ),
        );
        // Hidden in high precedence deletes the low precedence entry.
        tmp.write(
            "high/applications/deleted.desktop",
            "[Desktop Entry]\nType=Application\nHidden=true\n",
        );
        tmp.write(
            "low/applications/deleted.desktop",
            format!("[Desktop Entry]\nType=Application\nName=Deleted\nExec={viewer}\n"),
        );
        tmp.write(
            "low/applications/vendor/viewer.desktop",
            format!("[Desktop Entry]\nType=Application\nName=Viewer\nExec={viewer} %u\n"),
        );
        tmp.write(
            "low/applications/org.gimp.GIMP.desktop",
            "[Desktop Entry]\nType=Application\nName=GNU Image Manipulation Program\nExec=/usr/bin/flatpak run --branch=stable --command=gimp-2.10 org.gimp.GIMP @@u %U @@\nX-Flatpak=org.gimp.GIMP\nIcon=org.gimp.GIMP\n",
        );
        tmp.write(
            "low/applications/firefox_firefox.desktop",
            "[Desktop Entry]\nType=Application\nName=Firefox Web Browser\nExec=env BAMF_DESKTOP_FILE_HINT=/var/lib/snapd/desktop/applications/firefox_firefox.desktop /snap/bin/firefox %u\nX-SnapInstanceName=firefox\nIcon=/snap/firefox/1/default256.png\n",
        );
        tmp.write(
            "low/applications/missing.desktop",
            "[Desktop Entry]\nType=Application\nName=Missing\nExec=definitely-not-installed-reframe\n",
        );
        tmp.write(
            "low/applications/tryexec.desktop",
            format!(
                "[Desktop Entry]\nType=Application\nName=Gated\nExec={viewer}\nTryExec=/nonexistent/thing\n"
            ),
        );
        tmp.write(
            "low/applications/shell.desktop",
            "[Desktop Entry]\nType=Application\nName=Shell Thing\nExec=sh -c \"echo hi\"\n",
        );
        tmp.write(
            "low/applications/link.desktop",
            "[Desktop Entry]\nType=Link\nName=Link\nURL=https://example.com\n",
        );
        (tmp, bin)
    }

    fn index(tmp: &TempDir) -> DesktopIndex {
        DesktopIndex::scan(&[
            tmp.path().join("high/applications"),
            tmp.path().join("low/applications"),
        ])
    }

    #[test]
    fn scan_honours_precedence_hidden_and_subdirectories() {
        let (tmp, _) = fixture();
        let idx = index(&tmp);
        let ids: Vec<&str> = idx.entries().iter().map(|e| e.entry.id.as_str()).collect();
        assert!(ids.contains(&"editor.desktop"));
        assert!(ids.contains(&"vendor-viewer.desktop"));
        assert!(!ids.contains(&"deleted.desktop"));
        assert!(!ids.contains(&"link.desktop"));
        assert_eq!(
            idx.by_id("editor").unwrap().entry.name.as_deref(),
            Some("Super Editor")
        );
        assert_eq!(ids.iter().filter(|i| **i == "editor.desktop").count(), 1);
    }

    #[test]
    fn locate_ranks_and_dedupes() {
        let (tmp, bin) = fixture();
        let idx = index(&tmp);
        let editor = LaunchTarget::Executable {
            path: bin.join("editor"),
        };
        let viewer = LaunchTarget::Executable {
            path: bin.join("viewer"),
        };

        // Desktop id first, then display name, PATH hits, Exec matches; no duplicates.
        let query = LocateQuery {
            executable_names: vec!["editor".into()],
            desktop_ids: vec!["vendor-viewer.desktop".into()],
            display_names: vec!["super editor".into()],
            ..Default::default()
        };
        let found = locate(&idx, &query, &[bin.join("editor")]);
        assert_eq!(found, vec![viewer.clone(), editor.clone()]);

        let gimp = locate(
            &idx,
            &LocateQuery {
                executable_names: vec!["gimp-2.10".into()],
                ..Default::default()
            },
            &[],
        );
        assert_eq!(
            gimp,
            vec![LaunchTarget::Package {
                package: PackageInfo {
                    kind: PackageKind::Flatpak,
                    id: "org.gimp.GIMP".into(),
                    path: None
                }
            }]
        );
        let firefox = locate(
            &idx,
            &LocateQuery {
                display_names: vec!["Firefox Web Browser".into()],
                ..Default::default()
            },
            &[],
        );
        assert!(matches!(
            &firefox[..],
            [LaunchTarget::Package { package }] if package.kind == PackageKind::Snap && package.id == "firefox"
        ));
        // Uninstalled, TryExec-gated, shell and unknown entries yield nothing.
        for name in ["Missing", "Gated", "Shell Thing", "Nope"] {
            let q = LocateQuery {
                display_names: vec![name.into()],
                ..Default::default()
            };
            assert!(locate(&idx, &q, &[]).is_empty(), "{name}");
        }
    }

    #[test]
    fn product_names_for_processes() {
        let (tmp, bin) = fixture();
        let idx = index(&tmp);
        let editor = bin.join("editor").display().to_string();
        assert_eq!(
            idx.product_name(&process(Some(&editor), &[], None))
                .as_deref(),
            Some("Super Editor")
        );
        let flatpak = PackageInfo {
            kind: PackageKind::Flatpak,
            id: "org.gimp.GIMP".into(),
            path: None,
        };
        assert_eq!(
            idx.product_name(&process(Some("/app/bin/gimp-2.10"), &[], Some(flatpak)))
                .as_deref(),
            Some("GNU Image Manipulation Program")
        );
        let snap = PackageInfo {
            kind: PackageKind::Snap,
            id: "firefox".into(),
            path: None,
        };
        assert_eq!(
            idx.product_name(&process(None, &[], Some(snap))).as_deref(),
            Some("Firefox Web Browser")
        );
        // Interpreter running a script that a desktop entry launches.
        assert_eq!(
            idx.product_name(&process(
                Some("/usr/bin/python3.12"),
                &["python3", "-u", &editor],
                None
            ))
            .as_deref(),
            Some("Super Editor")
        );
        assert_eq!(
            idx.product_name(&process(Some("/usr/bin/python3"), &["python3"], None)),
            None
        );
        assert_eq!(
            idx.product_name(&process(Some("/usr/bin/unknown"), &[], None)),
            None
        );
    }

    #[test]
    fn icon_entries() {
        let (tmp, bin) = fixture();
        let idx = index(&tmp);
        let e = idx
            .entry_for_icon(None, Some(&bin.join("editor")), None)
            .unwrap();
        assert_eq!(e.entry.icon.as_deref(), Some("editor-icon"));
        let e = idx.entry_for_icon(None, None, Some("supereditor")).unwrap();
        assert_eq!(e.entry.id, "editor.desktop");
        let e = idx.entry_for_icon(None, None, Some("Editor")).unwrap();
        assert_eq!(e.entry.id, "editor.desktop");
        assert!(idx.entry_for_icon(None, None, Some("Viewer")).is_none());
    }

    #[test]
    fn interpreters() {
        assert!(is_interpreter("python3"));
        assert!(is_interpreter("python3.12"));
        assert!(is_interpreter("bash"));
        assert!(!is_interpreter("pythonista"));
        assert!(!is_interpreter("code"));
    }

    #[test]
    fn cache_reuses_and_rebuilds_the_index() {
        let tmp = TempDir::new("apps-cache");
        let dir = tmp.path().join("applications");
        fs::create_dir_all(&dir).unwrap();
        let cache = DesktopCache::new(vec![dir.clone()]);
        let first = cache.get();
        assert!(first.entries().is_empty());
        assert!(
            Arc::ptr_eq(&first, &cache.get()),
            "unchanged index is reused"
        );

        tmp.write(
            "applications/new.desktop",
            "[Desktop Entry]\nType=Application\nName=New\nExec=/bin/true\n",
        );
        // Simulate a directory change (mtime granularity differs between file systems).
        if let Some(cached) = cache.state.lock().unwrap().as_mut() {
            cached.stamps = vec![None];
        }
        let rebuilt = cache.get();
        assert!(!Arc::ptr_eq(&first, &rebuilt));
        assert_eq!(rebuilt.entries().len(), 1);
        // Missing directories are fine.
        let empty = DesktopCache::new(vec![tmp.path().join("nope")]);
        assert!(empty.get().entries().is_empty());
    }

    #[test]
    fn application_dirs_order() {
        let xdg = XdgDirs::from_values(Some("/home/u"), None, Some("/usr/share:/usr/local/share"));
        let dirs = application_dirs(&xdg);
        assert_eq!(dirs[0], PathBuf::from("/home/u/.local/share/applications"));
        assert_eq!(dirs[1], PathBuf::from("/usr/share/applications"));
        assert_eq!(dirs[2], PathBuf::from("/usr/local/share/applications"));
        assert!(dirs.contains(&PathBuf::from(
            "/var/lib/flatpak/exports/share/applications"
        )));
        assert!(dirs.contains(&PathBuf::from("/var/lib/snapd/desktop/applications")));
    }
}
