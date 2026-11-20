//! Linux (and other freedesktop) file managers: folders are recognised from window titles.
//!
//! Titles show the folder's display name (Nautilus, Nemo, Caja, PCManFM: `Downloads`),
//! sometimes followed by the application name (Dolphin: `Downloads — Dolphin`, Thunar:
//! `Downloads - Thunar`), or the full path when the file manager is configured to show it.
//!
//! Full paths (and `~/...`) are used as they are. Names are looked up — and only accepted when
//! exactly one folder matches — in:
//! - the home folder itself (titled `Home` or with its own name), its subfolders and their
//!   subfolders (hidden folders are matched by exact name but not descended into);
//! - the XDG user folders (`~/.config/user-dirs.dirs`);
//! - mounted volumes: `/media/$USER/*`, `/run/media/$USER/*`, `/mnt/*`, and mount points from
//!   `/proc/self/mounts` below `/media`, `/run/media` and `/mnt`.
//!
//! Limitations: names shared by several of these folders, deeper folders shown by name only,
//! virtual locations (Trash, Recent, Network, `sftp://`...) and localized special names (other
//! than `Home` / `File System`) cannot be resolved and are reported as warnings. Only the visible
//! tab of a window is known. All file-system access runs on a helper thread with a timeout, so
//! an unresponsive network mount cannot hang capture.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reframe_platform::WindowInfo;

use super::support;
use crate::model::FolderRef;

/// Application names some file managers append to the folder in the title.
const APP_LABELS: &[&str] = &[
    "dolphin",
    "thunar",
    "file manager",
    "files",
    "nautilus",
    "nemo",
    "caja",
    "pcmanfm",
    "pcmanfm-qt",
    "file browser",
];

/// Separators between the folder name and the application name.
const SEPARATORS: &[&str] = &[" — ", " – ", " - "];

/// Display names file managers use for the home folder and the file-system root.
const HOME_ALIASES: &[&str] = &["Home", "Home Folder"];
const ROOT_ALIASES: &[&str] = &["File System", "Filesystem"];

/// Directory entries examined per folder while scanning.
const MAX_DIR_ENTRIES: usize = 4096;
/// Home subfolders whose children are scanned.
const MAX_DESCENDED: usize = 512;
/// Upper bound for all file-system access of one capture.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);

/// The folder part of a file-manager window title (`Downloads — Dolphin` → `Downloads`).
pub(super) fn folder_from_title(title: &str) -> &str {
    let title = title.trim();
    for separator in SEPARATORS {
        if let Some(index) = title.rfind(separator) {
            let app = title[index + separator.len()..].trim();
            if APP_LABELS
                .iter()
                .any(|label| label.eq_ignore_ascii_case(app))
            {
                return title[..index].trim();
            }
        }
    }
    title
}

/// How one window title was resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FolderMatch {
    Found(PathBuf),
    /// This many folders have the name shown.
    Ambiguous(usize),
    NotFound,
    /// A full path that does not exist (any more).
    Missing(PathBuf),
    /// A remote or virtual location (`sftp://host/`, `trash:///`, `recent:///`).
    NotLocal,
}

/// Where folder names are looked up. Holds paths only; everything is read at capture time.
#[derive(Debug, Clone, Default)]
pub(super) struct FolderSearch {
    pub home: Option<PathBuf>,
    /// `user-dirs.dirs` (XDG user folders).
    pub user_dirs_file: Option<PathBuf>,
    /// `/proc/self/mounts`.
    pub mounts_file: Option<PathBuf>,
    /// Folders whose children are volumes (`/media/$USER`, `/run/media/$USER`, `/mnt`).
    pub volume_parents: Vec<PathBuf>,
    /// Mount points below these folders count as volumes.
    pub mount_prefixes: Vec<PathBuf>,
}

impl FolderSearch {
    /// The current user's locations.
    pub fn from_environment() -> Self {
        let home = dirs::home_dir();
        let user = std::env::var("USER")
            .ok()
            .filter(|u| !u.is_empty() && !u.contains(['/', '\0']))
            .or_else(|| {
                home.as_deref()
                    .and_then(Path::file_name)
                    .map(|n| n.to_string_lossy().into_owned())
            });
        let mut volume_parents = Vec::new();
        if let Some(user) = &user {
            volume_parents.push(Path::new("/media").join(user));
            volume_parents.push(Path::new("/run/media").join(user));
        }
        volume_parents.push(PathBuf::from("/mnt"));
        Self {
            home,
            user_dirs_file: dirs::config_dir().map(|c| c.join("user-dirs.dirs")),
            mounts_file: Some(PathBuf::from("/proc/self/mounts")),
            volume_parents,
            mount_prefixes: ["/media", "/run/media", "/mnt"].map(PathBuf::from).to_vec(),
        }
    }

    /// Resolve the folder part of each title (see the module docs). Touches the file system.
    pub fn resolve(&self, folders: &[String]) -> Vec<FolderMatch> {
        let names: HashSet<String> = folders
            .iter()
            .filter(|f| self.expand(f).is_none() && !is_virtual_location(f))
            .cloned()
            .collect();
        let index = if names.is_empty() {
            FolderIndex::default()
        } else {
            self.index(&names)
        };
        folders
            .iter()
            .map(|folder| {
                if is_virtual_location(folder) {
                    FolderMatch::NotLocal
                } else if let Some(path) = self.expand(folder) {
                    if path.is_dir() {
                        FolderMatch::Found(path)
                    } else {
                        FolderMatch::Missing(path)
                    }
                } else {
                    index.lookup(folder)
                }
            })
            .collect()
    }

    /// Absolute paths and `~` paths.
    fn expand(&self, folder: &str) -> Option<PathBuf> {
        if folder.starts_with('/') {
            return Some(PathBuf::from(folder));
        }
        let home = self.home.as_ref()?;
        if folder == "~" {
            return Some(home.clone());
        }
        folder.strip_prefix("~/").map(|rest| home.join(rest))
    }

    /// Scan the search locations for folders named like `names`.
    fn index(&self, names: &HashSet<String>) -> FolderIndex {
        let mut index = FolderIndex::default();
        if let Some(home) = &self.home {
            for alias in HOME_ALIASES {
                if names.contains(*alias) {
                    index.add(alias, home);
                }
            }
            index.add_if_named(names, home);
            let mut descended = 0;
            for child in list_dir(home) {
                if names.contains(&child.name) && child.path.is_dir() {
                    index.add(&child.name, &child.path);
                }
                if descended < MAX_DESCENDED && child.is_real_dir && !child.name.starts_with('.') {
                    descended += 1;
                    for grandchild in list_dir(&child.path) {
                        if names.contains(&grandchild.name) && grandchild.path.is_dir() {
                            index.add(&grandchild.name, &grandchild.path);
                        }
                    }
                }
            }
        }
        if let Some(text) = self.user_dirs_file.as_deref().and_then(read_small) {
            for dir in parse_user_dirs(&text, self.home.as_deref()) {
                if dir.is_dir() {
                    index.add_if_named(names, &dir);
                }
            }
        }
        for parent in &self.volume_parents {
            for child in list_dir(parent) {
                if names.contains(&child.name) && child.path.is_dir() {
                    index.add(&child.name, &child.path);
                }
            }
        }
        if let Some(text) = self.mounts_file.as_deref().and_then(read_small) {
            for mount in parse_mounts(&text) {
                let under_prefix = self
                    .mount_prefixes
                    .iter()
                    .any(|prefix| mount.starts_with(prefix) && mount != *prefix);
                if under_prefix && mount_name_wanted(names, &mount) && mount.is_dir() {
                    index.add_if_named(names, &mount);
                }
            }
        }
        for alias in ROOT_ALIASES {
            if names.contains(*alias) {
                index.add(alias, Path::new("/"));
            }
        }
        index
    }
}

fn mount_name_wanted(names: &HashSet<String>, mount: &Path) -> bool {
    mount
        .file_name()
        .is_some_and(|n| names.contains(n.to_string_lossy().as_ref()))
}

/// Candidate folders per wanted name.
#[derive(Debug, Default)]
struct FolderIndex {
    by_name: HashMap<String, Vec<PathBuf>>,
}

impl FolderIndex {
    fn add_if_named(&mut self, names: &HashSet<String>, path: &Path) {
        if let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned())
            && names.contains(&name)
        {
            self.add(&name, path);
        }
    }

    /// Record `path` under `name`, ignoring different spellings of the same folder (symlinks,
    /// `/media/u/x` also listed as a mount point).
    fn add(&mut self, name: &str, path: &Path) {
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let entry = self.by_name.entry(name.to_string()).or_default();
        if !entry.contains(&canonical) {
            entry.push(canonical);
        }
    }

    fn lookup(&self, name: &str) -> FolderMatch {
        match self.by_name.get(name).map(Vec::as_slice) {
            Some([only]) => FolderMatch::Found(only.clone()),
            Some(several) if several.len() > 1 => FolderMatch::Ambiguous(several.len()),
            _ => FolderMatch::NotFound,
        }
    }
}

struct DirEntry {
    name: String,
    path: PathBuf,
    /// A directory itself, not a symlink to one (symlinks are matched but never descended).
    is_real_dir: bool,
}

/// Entries of `dir` (unreadable folders are simply empty), capped at [`MAX_DIR_ENTRIES`].
fn list_dir(dir: &Path) -> Vec<DirEntry> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .take(MAX_DIR_ENTRIES)
        .map(|entry| DirEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            path: entry.path(),
            is_real_dir: entry.file_type().is_ok_and(|t| t.is_dir()),
        })
        .collect()
}

/// Small configuration files only (a few KB in practice).
fn read_small(path: &Path) -> Option<String> {
    const LIMIT: u64 = 1 << 20;
    let metadata = fs::metadata(path).ok()?;
    if metadata.len() > LIMIT {
        return None;
    }
    fs::read_to_string(path).ok()
}

/// `sftp://host/dir`, `trash:///`, `recent:///`, `x-nautilus-search://` — not local folders.
fn is_virtual_location(folder: &str) -> bool {
    let Some((scheme, rest)) = folder.split_once(':') else {
        return false;
    };
    scheme.len() > 1
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '+' | '.'))
        && rest.starts_with('/')
}

/// Folders configured in `user-dirs.dirs` (`XDG_DOCUMENTS_DIR="$HOME/Documents"`). Entries that
/// point at the home folder itself mean "disabled" and are skipped.
pub(super) fn parse_user_dirs(text: &str, home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if !(key.starts_with("XDG_") && key.ends_with("_DIR")) {
            continue;
        }
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(value)
            .replace("\\\"", "\"")
            .replace("\\\\", "\\");
        let path = if let Some(rest) = value
            .strip_prefix("${HOME}")
            .or_else(|| value.strip_prefix("$HOME"))
        {
            let Some(home) = home else { continue };
            let rest = rest.trim_start_matches('/');
            if rest.is_empty() {
                continue;
            }
            home.join(rest)
        } else if value.starts_with('/') {
            PathBuf::from(value)
        } else {
            continue;
        };
        if Some(path.as_path()) != home && !dirs.contains(&path) {
            dirs.push(path);
        }
    }
    dirs
}

/// Mount points listed in `/proc/self/mounts` (octal escapes such as `\040` decoded).
pub(super) fn parse_mounts(text: &str) -> Vec<PathBuf> {
    text.lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .map(|field| PathBuf::from(unescape_octal(field)))
        .collect()
}

/// `/media/u/My\040Disk` → `/media/u/My Disk`.
fn unescape_octal(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && bytes[i + 1..=i + 3]
                .iter()
                .all(|b| (b'0'..=b'7').contains(b))
        {
            let value = (bytes[i + 1] - b'0') as u32 * 64
                + (bytes[i + 2] - b'0') as u32 * 8
                + (bytes[i + 3] - b'0') as u32;
            if let Ok(byte) = u8::try_from(value) {
                out.push(byte);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Folders shown in `windows` (front-to-back), plus warnings for windows that were skipped.
pub(super) fn capture(
    windows: &[&WindowInfo],
    search: &FolderSearch,
) -> (Vec<FolderRef>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut titled: Vec<(&WindowInfo, String)> = Vec::new();
    for window in windows {
        let folder = folder_from_title(&window.title);
        if folder.is_empty() {
            warnings.push("a window without a title was skipped.".to_string());
        } else {
            titled.push((window, folder.to_string()));
        }
    }
    if titled.is_empty() {
        return (Vec::new(), warnings);
    }

    let folders: Vec<String> = titled.iter().map(|(_, f)| f.clone()).collect();
    let job_search = search.clone();
    let job_folders = folders.clone();
    let Some(matches) = support::run_with_timeout("folder-lookup", LOOKUP_TIMEOUT, move || {
        job_search.resolve(&job_folders)
    }) else {
        warnings.push(
            "looking up the open folders took too long (an unresponsive network drive?); \
             they were not captured."
                .to_string(),
        );
        return (Vec::new(), warnings);
    };

    let mut found = Vec::new();
    for ((window, folder), result) in titled.into_iter().zip(matches) {
        match result {
            FolderMatch::Found(path) => found.push(FolderRef {
                path,
                window_title: Some(window.title.clone()),
            }),
            FolderMatch::Ambiguous(count) => warnings.push(format!(
                "{count} folders are named \"{folder}\", so that window was skipped. Show the \
                 full path in the file manager's title bar to capture it."
            )),
            FolderMatch::NotFound => warnings.push(format!(
                "could not find the folder \"{folder}\", so that window was skipped. Show the \
                 full path in the file manager's title bar to capture it."
            )),
            FolderMatch::Missing(path) => warnings.push(format!(
                "the folder {} does not exist, so that window was skipped.",
                path.display()
            )),
            FolderMatch::NotLocal => warnings.push(format!(
                "\"{folder}\" is not a local folder, so that window was skipped."
            )),
        }
    }
    (found, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::file_manager::support::testing::window;

    #[test]
    fn strips_known_application_suffixes_only() {
        assert_eq!(folder_from_title("Downloads — Dolphin"), "Downloads");
        assert_eq!(folder_from_title("Downloads - Thunar"), "Downloads");
        assert_eq!(folder_from_title("Downloads - File Manager"), "Downloads");
        assert_eq!(folder_from_title("  reframe  "), "reframe");
        assert_eq!(
            folder_from_title("/home/a/Projects - Old — Dolphin"),
            "/home/a/Projects - Old"
        );
        assert_eq!(folder_from_title("Mix - Tape"), "Mix - Tape");
        assert_eq!(folder_from_title("A - B - pcmanfm-qt"), "A - B");
    }

    #[test]
    fn parses_user_dirs() {
        let text = r#"
# This file is written by xdg-user-dirs-update
XDG_DESKTOP_DIR="$HOME/Desktop"
XDG_DOWNLOAD_DIR="${HOME}/Downloads"
XDG_TEMPLATES_DIR="$HOME/"
XDG_DOCUMENTS_DIR="/data/Docs"
XDG_MUSIC_DIR="relative/ignored"
NOT_A_DIR="$HOME/x"
"#;
        let dirs = parse_user_dirs(text, Some(Path::new("/home/a")));
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/home/a/Desktop"),
                PathBuf::from("/home/a/Downloads"),
                PathBuf::from("/data/Docs"),
            ]
        );
        assert_eq!(
            parse_user_dirs(text, None),
            vec![PathBuf::from("/data/Docs")]
        );
    }

    #[test]
    fn parses_mounts_with_escapes() {
        let text = "sysfs /sys sysfs rw 0 0\n/dev/sdb1 /media/a/My\\040Disk vfat rw 0 0\nbad\n";
        assert_eq!(
            parse_mounts(text),
            vec![PathBuf::from("/sys"), PathBuf::from("/media/a/My Disk")]
        );
        assert_eq!(unescape_octal("a\\134b"), "a\\b");
        assert_eq!(unescape_octal("trailing\\04"), "trailing\\04");
        assert_eq!(unescape_octal("x\\999"), "x\\999");
    }

    #[test]
    fn recognises_virtual_locations() {
        assert!(is_virtual_location("sftp://host/home"));
        assert!(is_virtual_location("trash:///"));
        assert!(is_virtual_location("recentdocuments:/"));
        assert!(!is_virtual_location("Notes: 2024"));
        assert!(!is_virtual_location("/home/a"));
        assert!(!is_virtual_location("Downloads"));
    }

    struct Tree {
        _root: tempfile::TempDir,
        search: FolderSearch,
        home: PathBuf,
    }

    /// home/{Downloads, Documents/{reports, notes}, code/{reframe, notes}, .config/{autostart},
    /// Music}, data/Docs (XDG documents dir outside home), media/a/USB STICK, mnt/backup.
    fn tree() -> Tree {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().to_path_buf();
        let home = base.join("home").join("alice");
        for dir in [
            "Downloads",
            "Documents/reports",
            "Documents/notes",
            "code/reframe",
            "code/notes",
            ".config/autostart",
            "Music",
        ] {
            fs::create_dir_all(home.join(dir)).unwrap();
        }
        fs::create_dir_all(base.join("data/Scans")).unwrap();
        fs::create_dir_all(base.join("media/alice/USB STICK")).unwrap();
        fs::create_dir_all(base.join("mnt/backup")).unwrap();
        fs::create_dir_all(base.join("run/media/alice/Camera")).unwrap();
        let user_dirs = base.join("user-dirs.dirs");
        fs::write(
            &user_dirs,
            format!(
                "XDG_DOWNLOAD_DIR=\"$HOME/Downloads\"\nXDG_PICTURES_DIR=\"{}\"\n",
                base.join("data/Scans").display()
            ),
        )
        .unwrap();
        let mounts = base.join("mounts");
        fs::write(
            &mounts,
            format!(
                "/dev/sdc1 {} vfat rw 0 0\n",
                base.join("run/media/alice/Camera")
                    .display()
                    .to_string()
                    .replace(' ', "\\040")
            ),
        )
        .unwrap();
        let search = FolderSearch {
            home: Some(home.clone()),
            user_dirs_file: Some(user_dirs),
            mounts_file: Some(mounts),
            volume_parents: vec![base.join("media/alice"), base.join("mnt")],
            mount_prefixes: vec![base.join("run/media")],
        };
        Tree {
            _root: root,
            search,
            home,
        }
    }

    fn canonical(path: &Path) -> PathBuf {
        fs::canonicalize(path).unwrap()
    }

    #[test]
    fn resolves_names_against_home_xdg_and_volumes() {
        let t = tree();
        let base = t.home.parent().unwrap().parent().unwrap().to_path_buf();
        let folders: Vec<String> = [
            "Downloads",
            "reports",
            "reframe",
            "notes",
            "autostart",
            ".config",
            "Scans",
            "USB STICK",
            "backup",
            "Camera",
            "Home",
            "alice",
            "nowhere",
            "~/Music",
            "/definitely/missing/folder",
            "sftp://host/x",
        ]
        .map(String::from)
        .to_vec();
        let results = t.search.resolve(&folders);
        let expected = vec![
            FolderMatch::Found(canonical(&t.home.join("Downloads"))),
            FolderMatch::Found(canonical(&t.home.join("Documents/reports"))),
            FolderMatch::Found(canonical(&t.home.join("code/reframe"))),
            FolderMatch::Ambiguous(2),
            // Hidden folders are not descended into.
            FolderMatch::NotFound,
            FolderMatch::Found(canonical(&t.home.join(".config"))),
            FolderMatch::Found(canonical(&base.join("data/Scans"))),
            FolderMatch::Found(canonical(&base.join("media/alice/USB STICK"))),
            FolderMatch::Found(canonical(&base.join("mnt/backup"))),
            FolderMatch::Found(canonical(&base.join("run/media/alice/Camera"))),
            FolderMatch::Found(canonical(&t.home)),
            FolderMatch::Found(canonical(&t.home)),
            FolderMatch::NotFound,
            FolderMatch::Found(t.home.join("Music")),
            FolderMatch::Missing(PathBuf::from("/definitely/missing/folder")),
            FolderMatch::NotLocal,
        ];
        assert_eq!(results, expected);
    }

    #[test]
    fn a_folder_named_like_home_makes_home_ambiguous() {
        let t = tree();
        fs::create_dir_all(t.home.join("Home")).unwrap();
        assert_eq!(
            t.search.resolve(&["Home".to_string()]),
            vec![FolderMatch::Ambiguous(2)]
        );
    }

    #[test]
    fn capture_reports_found_folders_and_warnings() {
        let t = tree();
        let full = t.home.join("code/reframe");
        let w1 = window("1", 1, "Downloads", Some("org.gnome.Nautilus"));
        let w2 = window("2", 1, &format!("{} — Dolphin", full.display()), None);
        let w3 = window("3", 1, "notes", None);
        let w4 = window("4", 1, "   ", None);
        let w5 = window("5", 1, "trash:///", None);
        let (folders, warnings) = capture(&[&w1, &w2, &w3, &w4, &w5], &t.search);
        assert_eq!(
            folders,
            vec![
                FolderRef {
                    path: canonical(&t.home.join("Downloads")),
                    window_title: Some("Downloads".into()),
                },
                FolderRef {
                    path: full.clone(),
                    window_title: Some(format!("{} — Dolphin", full.display())),
                },
            ]
        );
        assert_eq!(warnings.len(), 3);
        assert!(warnings[0].contains("without a title"));
        assert!(warnings[1].contains("2 folders are named \"notes\""));
        assert!(warnings[2].contains("not a local folder"));
    }

    #[test]
    fn real_environment_lookups_work() {
        // Exercises the real home scan, user-dirs file and /proc/self/mounts.
        let search = FolderSearch::from_environment();
        let results = search.resolve(&[
            "~".to_string(),
            "reframe-test-no-such-folder-3f9a".to_string(),
        ]);
        let home_result = match &search.home {
            Some(home) if home.is_dir() => FolderMatch::Found(home.clone()),
            Some(home) => FolderMatch::Missing(home.clone()),
            None => FolderMatch::NotFound,
        };
        assert_eq!(results, vec![home_result, FolderMatch::NotFound]);
    }

    #[test]
    fn missing_home_still_resolves_absolute_paths() {
        let search = FolderSearch::default();
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().display().to_string();
        assert_eq!(
            search.resolve(&[folder.clone(), "Downloads".into(), "~".into()]),
            vec![
                FolderMatch::Found(dir.path().to_path_buf()),
                FolderMatch::NotFound,
                FolderMatch::NotFound,
            ]
        );
    }
}
