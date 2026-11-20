//! Paths from other computers: parsing Windows and Unix paths on any OS, prefix matching,
//! remapping (`C:\Users\a\code` → `/home/a/code`) and inferring mapping suggestions.
//!
//! Windows paths (`C:\…`, `C:/…`, `\\server\share\…`, `\\?\C:\…`) compare case-insensitively
//! and accept both separators; Unix paths compare case-sensitively and only split on `/`
//! (a backslash is an ordinary file-name character there). macOS volumes are usually
//! case-insensitive too, but are compared like any Unix path.

use std::path::{Path, PathBuf};

use super::limits::MAX_PATH_LEN;
use crate::model::PathMapping;

/// At most this many mapping suggestions are offered.
const MAX_SUGGESTIONS: usize = 8;
/// Only the first missing paths are examined when inferring suggestions (bounds the work).
const MAX_EXAMINED_PATHS: usize = 200;
/// At most this many filesystem checks are made while inferring suggestions: an import file
/// with very deep paths must not keep the preview busy.
const MAX_PROBES: usize = 2_000;
/// Unix folders where merely looking up a path can contact another machine (automounted NFS
/// hosts on macOS and Linux, the AFS global namespace).
const NETWORK_ROOTS: &[&str] = &["net", "Network", "afs"];
/// Folders under `C:\Users` that are not somebody's home folder.
const SHARED_WINDOWS_PROFILES: &[&str] = &["public", "default", "default user", "all users"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Style {
    Windows,
    Unix,
}

/// A path parsed independently of the OS Reframe runs on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PortablePath {
    style: Style,
    /// `C:` for drive paths, `\\server\share` for UNC paths, `/` for Unix absolute paths,
    /// empty for relative paths.
    root: String,
    components: Vec<String>,
}

impl PortablePath {
    pub(crate) fn parse(raw: &str) -> Self {
        let verbatim = raw
            .strip_prefix(r"\\?\")
            .or_else(|| raw.strip_prefix(r"\\.\"));
        if let Some(rest) = verbatim {
            return match rest.get(..4) {
                Some(unc) if unc.eq_ignore_ascii_case(r"UNC\") => Self::parse_unc(&rest[4..]),
                _ => Self::parse(rest),
            };
        }
        if is_drive_prefixed(raw) {
            let drive = raw[..1].to_ascii_uppercase();
            return Self {
                style: Style::Windows,
                root: format!("{drive}:"),
                components: split(&raw[2..], &['\\', '/']),
            };
        }
        if let Some(rest) = raw.strip_prefix(r"\\") {
            return Self::parse_unc(rest);
        }
        if raw.starts_with('/') {
            return Self {
                style: Style::Unix,
                root: "/".to_string(),
                components: split(raw, &['/']),
            };
        }
        let windows = raw.contains('\\') && !raw.contains('/');
        Self {
            style: if windows { Style::Windows } else { Style::Unix },
            root: String::new(),
            components: if windows {
                split(raw, &['\\', '/'])
            } else {
                split(raw, &['/'])
            },
        }
    }

    fn parse_unc(rest: &str) -> Self {
        let mut parts = split(rest, &['\\', '/']).into_iter();
        let server = parts.next().unwrap_or_default();
        let share = parts.next().unwrap_or_default();
        Self {
            style: Style::Windows,
            root: format!(r"\\{server}\{share}"),
            components: parts.collect(),
        }
    }

    pub(crate) fn is_absolute(&self) -> bool {
        !self.root.is_empty()
    }

    fn text_eq(&self, a: &str, b: &str) -> bool {
        match self.style {
            Style::Windows => a == b || a.to_lowercase() == b.to_lowercase(),
            Style::Unix => a == b,
        }
    }

    /// The components after `prefix` when `self` lies inside (or is) `prefix`.
    fn strip_prefix(&self, prefix: &PortablePath) -> Option<&[String]> {
        let inside = self.style == prefix.style
            && self.text_eq(&self.root, &prefix.root)
            && prefix.components.len() <= self.components.len()
            && prefix
                .components
                .iter()
                .zip(&self.components)
                .all(|(a, b)| self.text_eq(a, b));
        inside.then(|| &self.components[prefix.components.len()..])
    }

    fn same_path(&self, other: &PortablePath) -> bool {
        self.components.len() == other.components.len() && self.strip_prefix(other).is_some()
    }

    fn join(&self, rest: &[String]) -> PortablePath {
        let mut joined = self.clone();
        joined.components.extend(rest.iter().cloned());
        joined
    }

    fn truncated(&self, len: usize) -> PortablePath {
        let mut truncated = self.clone();
        truncated.components.truncate(len);
        truncated
    }

    /// The path as text, with the separators of its own style.
    pub(crate) fn render(&self) -> String {
        let separator = match self.style {
            Style::Windows => "\\",
            Style::Unix => "/",
        };
        let body = self.components.join(separator);
        match self.style {
            _ if self.root.is_empty() => body,
            Style::Unix => format!("/{body}"),
            Style::Windows if self.root.starts_with(r"\\") && body.is_empty() => self.root.clone(),
            Style::Windows => format!("{}\\{body}", self.root),
        }
    }
}

fn is_drive_prefixed(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn split(text: &str, separators: &[char]) -> Vec<String> {
    text.split(separators)
        .filter(|c| !c.is_empty() && *c != ".")
        .map(str::to_string)
        .collect()
}

/// Whether `value` is an absolute path on some OS: `/…` (Unix), `C:\…` or `C:/…` (Windows
/// drive), `\\server\share…` (Windows UNC, including `\\?\` forms). Paths in import files
/// must be absolute: a relative path would resolve against Reframe's own working directory,
/// and could be mistaken for an option when it is handed to an application (`--flag`).
pub(crate) fn is_portable_absolute(value: &str) -> bool {
    let bytes = value.as_bytes();
    value.starts_with('/')
        || value.starts_with(r"\\")
        || (bytes.len() >= 3 && is_drive_prefixed(value) && matches!(bytes[2], b'\\' | b'/'))
}

/// Whether `value` looks like an absolute filesystem path on some OS (`/…`, `~/…`, `C:\…`,
/// `\\server\…`). Used to recognise file references next to secret-looking option names.
pub(crate) fn looks_like_absolute_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    value.starts_with('/')
        || value.starts_with("~/")
        || value.starts_with("~\\")
        || value.starts_with(r"\\")
        || (bytes.len() >= 3 && is_drive_prefixed(value) && matches!(bytes[2], b'\\' | b'/'))
}

/// A validated folder mapping, ready to apply.
#[derive(Debug, Clone)]
pub(crate) struct CompiledMapping {
    from: PortablePath,
    to: PortablePath,
    from_len: usize,
}

/// Validate the user's folder mappings and order them longest prefix first. `~` at the start
/// of a destination means `home`.
pub(crate) fn compile_mappings(
    mappings: &[PathMapping],
    home: Option<&Path>,
) -> Result<Vec<CompiledMapping>, Vec<String>> {
    let mut problems = Vec::new();
    let mut compiled = Vec::new();
    for mapping in mappings {
        let from = mapping.from_prefix.trim();
        let to = mapping.to_prefix.trim();
        if from.is_empty() || to.is_empty() {
            problems.push(format!(
                "A folder mapping needs both a source and a destination folder ({from:?} → {to:?})."
            ));
            continue;
        }
        if from.contains('\0') || to.contains('\0') {
            problems.push("Folder mappings cannot contain NUL characters.".to_string());
            continue;
        }
        if from.chars().count() > MAX_PATH_LEN || to.chars().count() > MAX_PATH_LEN {
            problems.push(format!(
                "Folder mappings can be at most {MAX_PATH_LEN} characters long."
            ));
            continue;
        }
        let to = expand_home(to, home);
        compiled.push(CompiledMapping {
            from: PortablePath::parse(from),
            to: PortablePath::parse(&to),
            from_len: from.chars().count(),
        });
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    compiled.sort_by(|a, b| {
        b.from
            .components
            .len()
            .cmp(&a.from.components.len())
            .then(b.from_len.cmp(&a.from_len))
    });
    Ok(compiled)
}

fn expand_home(path: &str, home: Option<&Path>) -> String {
    let Some(home) = home else {
        return path.to_string();
    };
    if path == "~" {
        return home.to_string_lossy().into_owned();
    }
    match path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        Some(rest) => {
            // Join in the home path's own style so a mapping never mixes `/` and `\`.
            let home = home.to_string_lossy();
            let windows_style = home.contains('\\') || home.as_bytes().get(1) == Some(&b':');
            let sep = if windows_style { '\\' } else { '/' };
            let rest = rest.replace(['/', '\\'], &sep.to_string());
            format!("{}{sep}{rest}", home.trim_end_matches(['/', '\\']))
        }
        None => path.to_string(),
    }
}

/// Apply the first (longest) matching mapping; `None` when no mapping applies.
pub(crate) fn map_path(path: &Path, mappings: &[CompiledMapping]) -> Option<PathBuf> {
    if mappings.is_empty() {
        return None;
    }
    let parsed = PortablePath::parse(&path.to_string_lossy());
    mappings.iter().find_map(|m| {
        parsed
            .strip_prefix(&m.from)
            .map(|rest| PathBuf::from(m.to.join(rest).render()))
    })
}

/// Whether `path` exists on this machine. Relative paths never count: they would silently
/// resolve against Reframe's working directory.
///
/// Paths from an import file are never looked up when that could leave this computer:
/// network locations ([`is_network_path`]) and paths with `..` components (which could climb
/// into one) count as missing without touching the filesystem. On Windows, merely checking
/// whether `\\host\share\x` exists makes the system connect to `host` and authenticate with
/// the user's credentials; elsewhere it may trigger an automount.
pub(crate) fn exists_here(path: &Path) -> bool {
    probe_allowed(path) && path.exists()
}

/// Like [`exists_here`], for directories.
pub(crate) fn is_dir_here(path: &Path) -> bool {
    probe_allowed(path) && path.is_dir()
}

/// Like [`exists_here`], for files.
pub(crate) fn is_file_here(path: &Path) -> bool {
    probe_allowed(path) && path.is_file()
}

fn probe_allowed(path: &Path) -> bool {
    path.is_absolute()
        && !is_network_path(path)
        && !path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        && !path.to_string_lossy().split(['/', '\\']).any(|c| c == "..")
}

/// Whether `path` names a network location on any OS: UNC paths (`\\server\share`,
/// `//server/share`, `\\?\UNC\…`), Windows device paths other than drives (`\\.\pipe\…`),
/// and Unix automount roots (`/net/…`, `/Network/…`, `/afs/…`). Verbatim drive paths
/// (`\\?\C:\…`, as produced by `canonicalize` on Windows) are local.
pub(crate) fn is_network_path(path: &Path) -> bool {
    let text = path.to_string_lossy();
    // Windows treats `/` and `\` alike: two leading separators start a UNC or device path.
    if let [b'/' | b'\\', b'/' | b'\\', rest @ ..] = text.as_bytes() {
        let verbatim_drive = matches!(
            rest,
            [b'?' | b'.', b'/' | b'\\', drive, b':', ..] if drive.is_ascii_alphabetic()
        );
        return !verbatim_drive;
    }
    let mut components = text
        .split(['/', '\\'])
        .filter(|c| !c.is_empty() && *c != ".");
    text.starts_with('/')
        && components
            .next()
            .is_some_and(|c| NETWORK_ROOTS.contains(&c))
}

/// Whether a URL is a `file:` URL on another machine (`file://server/share/doc.pdf`), which
/// opens a network location like a UNC path.
pub(crate) fn is_remote_file_url(url: &str) -> bool {
    url::Url::parse(url.trim()).is_ok_and(|u| {
        u.scheme() == "file"
            && u.host_str()
                .is_some_and(|h| !h.is_empty() && !h.eq_ignore_ascii_case("localhost"))
    })
}

/// Counts filesystem checks against [`MAX_PROBES`].
struct Prober {
    remaining: usize,
}

impl Prober {
    fn new() -> Self {
        Self {
            remaining: MAX_PROBES,
        }
    }

    fn exhausted(&self) -> bool {
        self.remaining == 0
    }

    /// [`exists_here`] for the path built by `path` (only built while budget remains).
    fn exists(&mut self, path: impl FnOnce() -> String) -> bool {
        if self.exhausted() {
            return false;
        }
        self.remaining -= 1;
        exists_here(Path::new(&path()))
    }
}

/// Suggest folder mappings for `missing` paths (from another machine or user account):
/// 1. home folders of the source machine (`C:\Users\<name>`, `/Users/<name>`,
///    `/home/<name>`) → `home`, when at least one remapped path exists here;
/// 2. for the remaining paths, the deepest common parent folder whose relative structure
///    exists under `home`, either keeping the parent's own name (`D:\dev` → `~/dev`) or not
///    (`D:\dev` → `~`).
pub(crate) fn suggest_mappings(missing: &[PathBuf], home: Option<&Path>) -> Vec<PathMapping> {
    let Some(home) = home else {
        return Vec::new();
    };
    let home = PortablePath::parse(&home.to_string_lossy());
    if !home.is_absolute() {
        return Vec::new();
    }
    let paths: Vec<PortablePath> = missing
        .iter()
        .take(MAX_EXAMINED_PATHS)
        .map(|p| PortablePath::parse(&p.to_string_lossy()))
        .filter(|p| p.is_absolute() && !p.components.is_empty())
        .collect();
    let mut suggestions: Vec<(PortablePath, PortablePath)> = Vec::new();
    let mut prober = Prober::new();

    // 1. Home folders.
    let mut source_homes: Vec<PortablePath> = Vec::new();
    for path in &paths {
        if let Some(source_home) = home_prefix(path)
            && !source_homes.iter().any(|h| h.same_path(&source_home))
        {
            source_homes.push(source_home);
        }
    }
    for source_home in source_homes {
        if !source_home.same_path(&home)
            && remaps_to_existing(&mut prober, &paths, &source_home, &home, false)
        {
            suggestions.push((source_home, home.clone()));
        }
    }

    // 2. Common parent folders of what is left.
    let uncovered: Vec<&PortablePath> = paths
        .iter()
        .filter(|p| {
            !suggestions
                .iter()
                .any(|(from, _)| p.strip_prefix(from).is_some())
        })
        .collect();
    let mut groups: Vec<Vec<&PortablePath>> = Vec::new();
    for path in uncovered {
        match groups
            .iter_mut()
            .find(|g| g[0].truncated(1).same_path(&path.truncated(1)))
        {
            Some(group) => group.push(path),
            None => groups.push(vec![path]),
        }
    }
    for group in groups {
        if suggestions.len() >= MAX_SUGGESTIONS {
            break;
        }
        let mut common = group[0].clone();
        for path in &group[1..] {
            let shared = common
                .components
                .iter()
                .zip(&path.components)
                .take_while(|(a, b)| common.text_eq(a, b))
                .count();
            common.components.truncate(shared);
        }
        let group_paths: Vec<PortablePath> = group.into_iter().cloned().collect();
        for depth in (1..=common.components.len()).rev() {
            if prober.exhausted() {
                break;
            }
            let parent = common.truncated(depth);
            if suggestions.iter().any(|(from, _)| from.same_path(&parent)) {
                break;
            }
            let name = parent.components[depth - 1].clone();
            let keep_name = home.join(&[name]);
            let target =
                if remaps_to_existing(&mut prober, &group_paths, &parent, &keep_name, false) {
                    Some(keep_name)
                } else if remaps_to_existing(&mut prober, &group_paths, &parent, &home, true) {
                    Some(home.clone())
                } else {
                    None
                };
            if let Some(target) = target.filter(|t| !t.same_path(&parent)) {
                suggestions.push((parent, target));
                break;
            }
        }
    }

    suggestions
        .into_iter()
        .take(MAX_SUGGESTIONS)
        .map(|(from, to)| PathMapping {
            from_prefix: from.render(),
            to_prefix: to.render(),
        })
        .collect()
}

/// `C:\Users\<name>`, `/Users/<name>` or `/home/<name>` when `path` lies in such a folder.
fn home_prefix(path: &PortablePath) -> Option<PortablePath> {
    let c = &path.components;
    if c.len() < 2 {
        return None;
    }
    let is_home = match path.style {
        Style::Windows => {
            path.root.len() == 2
                && c[0].eq_ignore_ascii_case("users")
                && !SHARED_WINDOWS_PROFILES
                    .iter()
                    .any(|n| c[1].eq_ignore_ascii_case(n))
        }
        Style::Unix => {
            path.root == "/"
                && ((c[0] == "home" && c[1] != "linuxbrew")
                    || (c[0] == "Users" && c[1] != "Shared"))
        }
    };
    is_home.then(|| path.truncated(2))
}

/// Whether mapping `from` → `to` makes at least one of `paths` exist on this machine.
/// `require_rest`: the path itself must lie strictly below `from` (mapping a folder onto the
/// home folder itself proves nothing, the home folder always exists).
fn remaps_to_existing(
    prober: &mut Prober,
    paths: &[PortablePath],
    from: &PortablePath,
    to: &PortablePath,
    require_rest: bool,
) -> bool {
    paths.iter().any(|p| {
        !prober.exhausted()
            && p.strip_prefix(from).is_some_and(|rest| {
                (!require_rest || !rest.is_empty()) && prober.exists(|| to.join(rest).render())
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping(from: &str, to: &str) -> PathMapping {
        PathMapping {
            from_prefix: from.into(),
            to_prefix: to.into(),
        }
    }

    fn apply(path: &str, mappings: &[PathMapping]) -> Option<String> {
        let compiled = compile_mappings(mappings, Some(Path::new("/home/bob"))).unwrap();
        map_path(Path::new(path), &compiled).map(|p| p.to_string_lossy().into_owned())
    }

    #[test]
    fn parses_paths_of_every_os() {
        let p = PortablePath::parse(r"C:\Users\Alice\code");
        assert_eq!(p.style, Style::Windows);
        assert_eq!(p.root, "C:");
        assert_eq!(p.components, ["Users", "Alice", "code"]);
        assert_eq!(p.render(), r"C:\Users\Alice\code");

        let p = PortablePath::parse("d:/work//proj/./src/");
        assert_eq!(p.render(), r"D:\work\proj\src");

        let p = PortablePath::parse(r"\\server\share\team\docs");
        assert_eq!(p.root, r"\\server\share");
        assert_eq!(p.render(), r"\\server\share\team\docs");

        assert_eq!(
            PortablePath::parse(r"\\?\C:\very\long").render(),
            r"C:\very\long"
        );
        assert_eq!(
            PortablePath::parse(r"\\?\UNC\server\share\x").render(),
            r"\\server\share\x"
        );

        let p = PortablePath::parse("/home/alice/my\\file");
        assert_eq!(p.style, Style::Unix);
        assert_eq!(p.components, ["home", "alice", "my\\file"]);
        assert_eq!(PortablePath::parse("/").render(), "/");
        assert_eq!(PortablePath::parse("C:\\").render(), "C:\\");
        assert!(!PortablePath::parse("relative/dir").is_absolute());
        assert!(!PortablePath::parse("").is_absolute());
    }

    #[test]
    fn windows_to_unix_mapping_converts_separators() {
        let m = [mapping(r"C:\Users\alice", "/home/bob")];
        assert_eq!(
            apply(r"C:\Users\alice\code\reframe", &m).as_deref(),
            Some("/home/bob/code/reframe")
        );
        // Case-insensitive and separator-agnostic for Windows-style prefixes.
        assert_eq!(
            apply("c:/users/ALICE/Documents/a.pdf", &m).as_deref(),
            Some("/home/bob/Documents/a.pdf")
        );
        assert_eq!(apply(r"C:\Users\alice", &m).as_deref(), Some("/home/bob"));
        // Whole components only.
        assert_eq!(apply(r"C:\Users\alice2\code", &m), None);
        assert_eq!(apply(r"D:\Users\alice\code", &m), None);
    }

    #[test]
    fn unix_mappings_are_case_sensitive_and_can_target_windows() {
        let m = [mapping("/Users/alice", r"C:\Users\alice")];
        assert_eq!(
            apply("/Users/alice/Projects/app", &m).as_deref(),
            Some(r"C:\Users\alice\Projects\app")
        );
        assert_eq!(apply("/Users/Alice/Projects/app", &m), None);
        assert_eq!(apply("/Users/alice-old/x", &m), None);
    }

    #[test]
    fn longest_prefix_wins_and_home_is_expanded() {
        let m = [
            mapping("/home/alice", "/home/bob"),
            mapping("/home/alice/code/", "~/src"),
            mapping("  /mnt/data  ", "~"),
        ];
        assert_eq!(
            apply("/home/alice/code/reframe", &m).as_deref(),
            Some("/home/bob/src/reframe")
        );
        assert_eq!(
            apply("/home/alice/notes.txt", &m).as_deref(),
            Some("/home/bob/notes.txt")
        );
        assert_eq!(
            apply("/mnt/data/photos", &m).as_deref(),
            Some("/home/bob/photos")
        );
        assert_eq!(apply("/opt/tool", &m), None);
    }

    #[test]
    fn invalid_mappings_are_reported() {
        assert!(compile_mappings(&[mapping("  ", "/home/bob")], None).is_err());
        assert!(compile_mappings(&[mapping("/a", "")], None).is_err());
        assert!(compile_mappings(&[mapping("/a\0", "/b")], None).is_err());
        assert!(compile_mappings(&[mapping(&"x".repeat(5000), "/b")], None).is_err());
        assert!(compile_mappings(&[], None).unwrap().is_empty());
    }

    #[test]
    fn absolute_path_detection() {
        for p in [
            "/usr/bin",
            "~/x",
            r"C:\x",
            "c:/x",
            r"\\server\share",
            "~\\x",
        ] {
            assert!(looks_like_absolute_path(p), "{p}");
        }
        for p in ["relative", "C:", "~", "hunter2", "-x", ""] {
            assert!(!looks_like_absolute_path(p), "{p}");
        }
    }

    #[test]
    fn network_locations_are_recognised_and_never_probed() {
        for p in [
            r"\\attacker.example\share\doc.pdf",
            "//attacker.example/share/doc.pdf",
            r"\/attacker.example\share",
            r"\\?\UNC\attacker.example\share\x",
            r"\\.\pipe\x",
            "/net/attacker.example/export",
            "/Network/Servers/host/x",
            "/afs/example.org/user",
            "/./net/host",
        ] {
            assert!(is_network_path(Path::new(p)), "{p}");
            assert!(!exists_here(Path::new(p)), "{p}");
        }
        for p in [
            "/home/alice/net",
            r"C:\net\x",
            "/netflix",
            "relative/net",
            r"\\?\C:\Program Files\App\app.exe",
            r"\\.\D:\data",
        ] {
            assert!(!is_network_path(Path::new(p)), "{p}");
        }
        // `..` could climb into a network folder: never probed.
        let dir = tempfile::tempdir().unwrap();
        let climbing = dir.path().join("..").join(dir.path().file_name().unwrap());
        assert!(dir.path().exists() && climbing.exists());
        assert!(!exists_here(&climbing));
        assert!(!is_dir_here(&climbing));
        assert!(exists_here(dir.path()) && is_dir_here(dir.path()));
        assert!(!exists_here(Path::new("relative")));

        assert!(is_remote_file_url("file://server/share/doc.pdf"));
        assert!(!is_remote_file_url("file:///home/a/doc.pdf"));
        assert!(!is_remote_file_url("file://localhost/home/a/doc.pdf"));
        assert!(!is_remote_file_url("https://example.com/"));
    }

    #[test]
    fn portable_absolute_paths() {
        for p in ["/usr/bin", r"C:\x", "c:/x", r"\\server\share", r"\\?\C:\x"] {
            assert!(is_portable_absolute(p), "{p}");
        }
        for p in [
            "relative",
            "C:",
            "C:foo",
            "~/x",
            "~",
            "--install-extension",
            "-c",
            "",
        ] {
            assert!(!is_portable_absolute(p), "{p}");
        }
    }

    #[test]
    fn suggestion_work_is_bounded_for_deep_paths() {
        let home = temp_home();
        // 200 paths of ~2000 components each would mean millions of filesystem checks
        // without the probe budget.
        let deep = format!("D:{}", r"\a".repeat(2_000));
        let missing: Vec<PathBuf> = (0..MAX_EXAMINED_PATHS)
            .map(|i| PathBuf::from(format!(r"{deep}\{i}")))
            .collect();
        let start = std::time::Instant::now();
        assert!(suggest_mappings(&missing, Some(home.path())).is_empty());
        assert!(start.elapsed() < std::time::Duration::from_secs(20));
    }

    fn temp_home() -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("code/reframe/src")).unwrap();
        std::fs::create_dir_all(home.path().join("Documents")).unwrap();
        home
    }

    fn render(path: &Path) -> String {
        PortablePath::parse(&path.to_string_lossy()).render()
    }

    #[test]
    fn suggests_home_mapping_from_windows() {
        let home = temp_home();
        let missing = vec![
            PathBuf::from(r"C:\Users\Alice\code\reframe"),
            PathBuf::from(r"C:\Users\Alice\Downloads\gone.zip"),
            PathBuf::from(r"C:\Users\Public\Documents\shared.txt"),
        ];
        let suggestions = suggest_mappings(&missing, Some(home.path()));
        assert_eq!(
            suggestions,
            vec![mapping(r"C:\Users\Alice", &render(home.path()))]
        );
    }

    #[test]
    fn suggests_home_mapping_from_macos() {
        let home = temp_home();
        let missing = vec![PathBuf::from("/Users/alice/Documents")];
        let suggestions = suggest_mappings(&missing, Some(home.path()));
        assert_eq!(
            suggestions,
            vec![mapping("/Users/alice", &render(home.path()))]
        );
    }

    #[test]
    fn no_suggestion_when_nothing_would_exist() {
        let home = temp_home();
        let missing = vec![
            PathBuf::from("/Users/alice/Movies/film.mkv"),
            PathBuf::from(r"E:\archive\2019"),
        ];
        assert!(suggest_mappings(&missing, Some(home.path())).is_empty());
        assert!(suggest_mappings(&missing, None).is_empty());
        assert!(suggest_mappings(&[], Some(home.path())).is_empty());
    }

    #[test]
    fn suggests_common_parent_with_the_same_structure() {
        let home = temp_home();
        std::fs::create_dir_all(home.path().join("dev/reframe")).unwrap();
        let missing = vec![
            PathBuf::from(r"D:\dev\reframe"),
            PathBuf::from(r"D:\dev\other\thing"),
        ];
        let suggestions = suggest_mappings(&missing, Some(home.path()));
        assert_eq!(
            suggestions,
            vec![mapping(r"D:\dev", &render(&home.path().join("dev")))]
        );

        // The deepest parent that works is preferred.
        let missing = vec![
            PathBuf::from(r"D:\work\code\reframe"),
            PathBuf::from(r"D:\work\code\other"),
        ];
        let suggestions = suggest_mappings(&missing, Some(home.path()));
        assert_eq!(
            suggestions,
            vec![mapping(r"D:\work\code", &render(&home.path().join("code")))]
        );

        // Same relative structure directly under the home folder: `E:\work` → home.
        let missing = vec![
            PathBuf::from(r"E:\work\Documents"),
            PathBuf::from(r"E:\work\code\reframe"),
        ];
        let suggestions = suggest_mappings(&missing, Some(home.path()));
        assert_eq!(suggestions, vec![mapping(r"E:\work", &render(home.path()))]);
    }
}
