//! Locating installed applications: name normalization, registry value cleanup, a bounded
//! directory search and result ranking. File-system access uses `std::fs` only, so the search is
//! tested on every OS.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use crate::LaunchTarget;

/// Extensions `CreateProcess` (via `std::process::Command`) can start.
const LAUNCHABLE_EXTENSIONS: [&str; 4] = ["exe", "com", "bat", "cmd"];
/// Characters that cannot appear in a Windows file name.
const RESERVED_FILE_NAME_CHARS: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// Limits of [`FileIndex::build`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchLimits {
    /// Directory levels below each root that are read (root = level 0).
    pub max_depth: usize,
    /// Directory entries examined per root before giving up.
    pub max_entries: usize,
}

fn extension_of(name: &str) -> Option<String> {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
}

fn is_valid_file_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(RESERVED_FILE_NAME_CHARS)
        && !name.chars().any(char::is_control)
        && name != "."
        && name != ".."
}

/// Executable file names to look for, lowercase: `"code"` → `"code.exe"`, `"Code.exe"` →
/// `"code.exe"`. Names that already end in a launchable extension keep it; anything else gets
/// `.exe` appended (so `"python3.12"` → `"python3.12.exe"`). Paths and invalid names are dropped.
pub fn executable_file_names(names: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for raw in names {
        let name = raw.trim().trim_matches('"').trim();
        if !is_valid_file_name(name) {
            continue;
        }
        let lower = name.to_lowercase();
        let file_name = match extension_of(&lower) {
            Some(ext) if LAUNCHABLE_EXTENSIONS.contains(&ext.as_str()) => lower,
            _ => format!("{lower}.exe"),
        };
        if !out.contains(&file_name) {
            out.push(file_name);
        }
    }
    out
}

/// Start-menu shortcut file names for display names: `"Visual Studio Code"` →
/// `"visual studio code.lnk"`.
pub fn shortcut_file_names(display_names: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for raw in display_names {
        let name = raw.trim();
        if !is_valid_file_name(name) {
            continue;
        }
        let file_name = format!("{}.lnk", name.to_lowercase());
        if !out.contains(&file_name) {
            out.push(file_name);
        }
    }
    out
}

/// Expand `%NAME%` references like `ExpandEnvironmentStringsW`: unknown names are left as they
/// are, a lone `%` is kept literally.
pub fn expand_env_vars(input: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('%') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &after[..end];
        match (!name.is_empty()).then(|| lookup(name)).flatten() {
            Some(value) => {
                out.push_str(&value);
                rest = &after[end + 1..];
            }
            None => {
                // Keep the text literally; the closing `%` may open the next reference.
                out.push('%');
                out.push_str(name);
                rest = &after[end..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Turn an App Paths default value into a path: environment references expanded, surrounding
/// quotes (and anything after the closing quote) removed.
pub fn clean_registry_path(raw: &str, lookup: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let expanded = expand_env_vars(raw.trim(), lookup);
    let text = expanded.trim();
    let text = match text.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next().unwrap_or(quoted),
        None => text,
    };
    let text = text.trim();
    (!text.is_empty() && !text.chars().any(char::is_control)).then(|| PathBuf::from(text))
}

/// Case- and separator-insensitive comparison key for a Windows path.
pub fn path_key(path: &Path) -> String {
    let text = path.to_string_lossy().replace('/', "\\").to_lowercase();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    text.trim_end_matches('\\').to_string()
}

/// `true` for file names (lowercase) that `CreateProcess` can start.
pub fn is_launchable_name(name: &str) -> bool {
    extension_of(name).is_some_and(|ext| LAUNCHABLE_EXTENSIONS.contains(&ext.as_str()))
}

/// `true` for shortcut file names (lowercase).
pub fn is_shortcut_name(name: &str) -> bool {
    extension_of(name).is_some_and(|ext| ext == "lnk")
}

/// Files found by one bounded breadth-first walk, indexed by lowercase file name, so repeated
/// lookups do not walk the disk again.
#[derive(Debug, Default, Clone)]
pub struct FileIndex {
    by_name: HashMap<String, Vec<PathBuf>>,
}

impl FileIndex {
    /// Walk `roots` breadth first (so shallower files come first; roots in the given order) and
    /// index every file whose lowercase name passes `keep`. Symbolic links and junctions to
    /// directories are not followed; unreadable directories are skipped.
    pub fn build(roots: &[PathBuf], keep: impl Fn(&str) -> bool, limits: SearchLimits) -> Self {
        let mut by_name: HashMap<String, Vec<PathBuf>> = HashMap::new();
        let mut seen = HashSet::new();
        for root in roots {
            let mut queue = VecDeque::from([(root.clone(), 0usize)]);
            let mut examined = 0usize;
            'walk: while let Some((dir, depth)) = queue.pop_front() {
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for entry in entries.flatten() {
                    examined += 1;
                    if examined > limits.max_entries {
                        break 'walk;
                    }
                    let Ok(file_type) = entry.file_type() else {
                        continue;
                    };
                    let path = entry.path();
                    if file_type.is_dir() {
                        if depth < limits.max_depth {
                            queue.push_back((path, depth + 1));
                        }
                        continue;
                    }
                    let name = entry.file_name().to_string_lossy().to_lowercase();
                    if !keep(&name) {
                        continue;
                    }
                    // Links to files are fine; links to directories are never followed.
                    let is_file = if file_type.is_symlink() {
                        std::fs::metadata(&path).is_ok_and(|m| m.is_file())
                    } else {
                        file_type.is_file()
                    };
                    if is_file && seen.insert(path_key(&path)) {
                        by_name.entry(name).or_default().push(path);
                    }
                }
            }
        }
        Self { by_name }
    }

    /// Indexed paths for lowercase file names, in the order of `file_names`.
    pub fn find(&self, file_names: &[String]) -> Vec<PathBuf> {
        file_names
            .iter()
            .filter_map(|name| self.by_name.get(name))
            .flatten()
            .cloned()
            .collect()
    }
}

/// Candidates from every source, before ranking.
#[derive(Debug, Default, Clone)]
pub struct LocateSources {
    /// `PATH` lookups (may include `.cmd`/`.bat` shims).
    pub on_path: Vec<PathBuf>,
    /// Registered `App Paths` (HKCU first, then HKLM).
    pub app_paths: Vec<PathBuf>,
    /// Bounded search of the program directories.
    pub search: Vec<PathBuf>,
    /// Start-menu shortcuts matching a display name.
    pub shortcuts: Vec<PathBuf>,
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| extensions.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Rank candidates, most likely first, without duplicates: binaries found on `PATH`, registered
/// App Paths, the program-directory search, script shims on `PATH` (`.cmd`/`.bat`, which open a
/// console window), and finally start-menu shortcuts (opened through the shell). Files that
/// cannot be started directly (extension-less scripts, `.ps1`, ...) are dropped.
pub fn rank_candidates(sources: LocateSources) -> Vec<LaunchTarget> {
    let LocateSources {
        on_path,
        app_paths,
        search,
        shortcuts,
    } = sources;
    let launchable = |p: &PathBuf| has_extension(p, &LAUNCHABLE_EXTENSIONS);
    let (path_binaries, path_scripts): (Vec<PathBuf>, Vec<PathBuf>) = on_path
        .into_iter()
        .filter(launchable)
        .partition(|p| has_extension(p, &["exe", "com"]));

    let mut seen = HashSet::new();
    let mut targets = Vec::new();
    let executables = path_binaries
        .into_iter()
        .chain(app_paths.into_iter().filter(launchable))
        .chain(search.into_iter().filter(launchable))
        .chain(path_scripts);
    for path in executables {
        if seen.insert(path_key(&path)) {
            targets.push(LaunchTarget::Executable { path });
        }
    }
    for shortcut in shortcuts {
        if has_extension(&shortcut, &["lnk"]) && seen.insert(path_key(&shortcut)) {
            targets.push(LaunchTarget::Open {
                target: shortcut.to_string_lossy().into_owned(),
            });
        }
    }
    targets
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A unique scratch directory under the system temp dir, removed on drop.
    struct TempTree(PathBuf);

    impl TempTree {
        fn new() -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "reframe-win32-locate-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }

        fn file(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(&path, b"MZ").expect("write");
            path
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn env(name: &str) -> Option<String> {
        match name.to_ascii_lowercase().as_str() {
            "programfiles" => Some(r"C:\Program Files".into()),
            "localappdata" => Some(r"C:\Users\me\AppData\Local".into()),
            _ => None,
        }
    }

    #[test]
    fn executable_names_are_normalized() {
        assert_eq!(
            executable_file_names(&names(&[
                "code",
                "Code.exe",
                " \"Notepad++.EXE\" ",
                "python3.12",
                "tool.cmd",
                "",
                r"C:\x\y.exe",
                "a/b",
                "bad|name",
            ])),
            vec!["code.exe", "notepad++.exe", "python3.12.exe", "tool.cmd"]
        );
    }

    #[test]
    fn shortcut_names_from_display_names() {
        assert_eq!(
            shortcut_file_names(&names(&[
                "Visual Studio Code",
                "visual studio code",
                "a\\b",
                " "
            ])),
            vec!["visual studio code.lnk"]
        );
    }

    #[test]
    fn environment_expansion() {
        assert_eq!(
            expand_env_vars(r"%ProgramFiles%\App\app.exe", env),
            r"C:\Program Files\App\app.exe"
        );
        assert_eq!(
            expand_env_vars(r"%LOCALAPPDATA%\%UNKNOWN%\x", env),
            r"C:\Users\me\AppData\Local\%UNKNOWN%\x"
        );
        assert_eq!(
            expand_env_vars("100% sure %PROGRAMFILES%", env),
            r"100% sure C:\Program Files"
        );
        assert_eq!(expand_env_vars("%%", env), "%%");
        assert_eq!(expand_env_vars("50%", env), "50%");
        assert_eq!(expand_env_vars("", env), "");
    }

    #[test]
    fn registry_values_are_cleaned() {
        assert_eq!(
            clean_registry_path(r#" "%ProgramFiles%\App\app.exe" "#, env),
            Some(PathBuf::from(r"C:\Program Files\App\app.exe"))
        );
        assert_eq!(
            clean_registry_path(r#""C:\A B\b.exe" --flag"#, env),
            Some(PathBuf::from(r"C:\A B\b.exe"))
        );
        assert_eq!(
            clean_registry_path(r"C:\Program Files\c.exe", env),
            Some(PathBuf::from(r"C:\Program Files\c.exe"))
        );
        assert_eq!(clean_registry_path("  ", env), None);
        assert_eq!(clean_registry_path("\"\"", env), None);
        assert_eq!(clean_registry_path("a\u{1}b", env), None);
    }

    #[test]
    fn path_keys_ignore_case_separators_and_prefixes() {
        assert_eq!(
            path_key(Path::new(r"C:\Program Files\App\App.EXE")),
            path_key(Path::new("c:/program files/app/app.exe"))
        );
        assert_eq!(
            path_key(Path::new(r"\\?\C:\Tools\")),
            path_key(Path::new(r"C:\tools"))
        );
    }

    const WIDE: SearchLimits = SearchLimits {
        max_depth: 3,
        max_entries: 10_000,
    };

    #[test]
    fn launchable_and_shortcut_names() {
        assert!(is_launchable_name("code.exe"));
        assert!(is_launchable_name("tool.cmd"));
        assert!(!is_launchable_name("readme.txt"));
        assert!(!is_launchable_name("code"));
        assert!(is_shortcut_name("visual studio code.lnk"));
        assert!(!is_shortcut_name("game.url"));
    }

    #[test]
    fn index_finds_shallow_matches_first_and_respects_depth() {
        let tree = TempTree::new();
        let deep = tree.file("Vendor/Product/bin/App.exe");
        let shallow = tree.file("Other/app.EXE");
        let _too_deep = tree.file("a/b/c/d/app.exe");
        let _other = tree.file("Vendor/readme.txt");
        let index = FileIndex::build(std::slice::from_ref(&tree.0), is_launchable_name, WIDE);
        assert_eq!(
            index.find(&names(&["app.exe"])),
            vec![shallow.clone(), deep]
        );
        assert!(index.find(&names(&["readme.txt"])).is_empty());

        let shallow_only = FileIndex::build(
            std::slice::from_ref(&tree.0),
            is_launchable_name,
            SearchLimits {
                max_depth: 1,
                max_entries: 10_000,
            },
        );
        assert_eq!(shallow_only.find(&names(&["app.exe"])), vec![shallow]);
    }

    #[test]
    fn index_lookups_follow_the_requested_name_order() {
        let tree = TempTree::new();
        let first = tree.file("x/first.exe");
        let second = tree.file("second.exe");
        let index = FileIndex::build(std::slice::from_ref(&tree.0), is_launchable_name, WIDE);
        assert_eq!(
            index.find(&names(&["first.exe", "missing.exe", "second.exe"])),
            vec![first.clone(), second.clone()]
        );
        assert_eq!(
            index.find(&names(&["second.exe", "first.exe"])),
            vec![second, first]
        );
        assert!(index.find(&[]).is_empty());
    }

    #[test]
    fn index_stops_at_the_entry_budget_and_skips_missing_roots() {
        let tree = TempTree::new();
        for i in 0..20 {
            tree.file(&format!("dir{i:02}/filler.txt"));
        }
        tree.file("zz/app.exe");
        // The same root twice must not produce duplicates.
        let roots = [
            tree.0.join("does-not-exist"),
            tree.0.clone(),
            tree.0.clone(),
        ];
        let limited = FileIndex::build(
            &roots,
            is_launchable_name,
            SearchLimits {
                max_depth: 3,
                max_entries: 5,
            },
        );
        assert!(limited.find(&names(&["app.exe"])).is_empty());
        let unlimited = FileIndex::build(&roots, is_launchable_name, WIDE);
        assert_eq!(unlimited.find(&names(&["app.exe"])).len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn index_does_not_follow_directory_links() {
        let tree = TempTree::new();
        let target = tree.file("real/app.exe");
        std::os::unix::fs::symlink(tree.0.join("real"), tree.0.join("loop")).expect("dir link");
        std::os::unix::fs::symlink(tree.0.clone(), tree.0.join("real/up")).expect("cycle");
        std::os::unix::fs::symlink(&target, tree.0.join("linked.exe")).expect("file link");
        let index = FileIndex::build(std::slice::from_ref(&tree.0), is_launchable_name, WIDE);
        assert_eq!(index.find(&names(&["app.exe"])), vec![target]);
        assert_eq!(index.find(&names(&["linked.exe"])).len(), 1);
    }

    #[test]
    fn ranking_orders_sources_and_removes_duplicates() {
        let sources = LocateSources {
            on_path: vec![
                PathBuf::from(r"C:\VS Code\bin\code.cmd"),
                PathBuf::from(r"C:\VS Code\bin\code"),
                PathBuf::from(r"C:\Tools\code.exe"),
            ],
            app_paths: vec![
                PathBuf::from(r"C:\VS Code\Code.exe"),
                PathBuf::from(r"c:\tools\CODE.EXE"),
            ],
            search: vec![
                PathBuf::from(r"C:\vs code\code.exe"),
                PathBuf::from(r"C:\Other\code.exe"),
                PathBuf::from(r"C:\Other\code.ps1"),
            ],
            shortcuts: vec![
                PathBuf::from(r"C:\Start\Visual Studio Code.lnk"),
                PathBuf::from(r"C:\Start\Visual Studio Code.url"),
            ],
        };
        let labels: Vec<String> = rank_candidates(sources)
            .iter()
            .map(LaunchTarget::label)
            .collect();
        assert_eq!(
            labels,
            vec![
                r"C:\Tools\code.exe",
                r"C:\VS Code\Code.exe",
                r"C:\Other\code.exe",
                r"C:\VS Code\bin\code.cmd",
                r"C:\Start\Visual Studio Code.lnk",
            ]
        );
    }

    #[test]
    fn shortcuts_become_open_targets() {
        let targets = rank_candidates(LocateSources {
            shortcuts: vec![PathBuf::from(r"C:\Start\App.lnk")],
            ..Default::default()
        });
        assert_eq!(
            targets,
            vec![LaunchTarget::Open {
                target: r"C:\Start\App.lnk".into()
            }]
        );
    }
}
