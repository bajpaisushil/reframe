//! Matching what a window title says to a folder or `.code-workspace` file VS Code opened.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use reframe_platform::OsKind;

use super::paths;
use super::storage::{Recent, RecentKind};
use super::title::{RootCandidate, TitleInfo};

/// The folder or workspace file a window shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub path: PathBuf,
    pub is_workspace_file: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Match {
    Found(Resolved),
    /// Every matching location is already assigned to another window, so this window shows
    /// the same project (e.g. a floating editor window).
    SameAsOther(PathBuf),
    NotFound,
}

/// Existence check used by the resolver (the real filesystem in production).
pub type ExistsFn<'a> = dyn Fn(&Path, RecentKind) -> bool + 'a;

pub struct Resolver<'a> {
    /// Known folders and workspace files, most relevant first.
    pub candidates: &'a [Recent],
    pub os: OsKind,
    /// Expands `~` in paths shown in titles.
    pub home: Option<&'a Path>,
    pub exists: &'a ExistsFn<'a>,
}

impl Resolver<'_> {
    /// The location `title` shows. Title candidates are tried in order; for each, known
    /// locations are tried most relevant first, exact names before case-insensitive ones.
    /// Locations that no longer exist are never returned, and neither are locations in
    /// `taken` (already assigned to other windows).
    pub fn resolve(&self, title: &TitleInfo, taken: &[PathBuf]) -> Match {
        let mut same_as_other = None;
        for candidate in &title.candidates {
            let found = match candidate {
                RootCandidate::Name { name, workspace } => {
                    self.by_name(name, *workspace, taken, &mut same_as_other)
                }
                RootCandidate::Path(path) => self.by_path(path, taken, &mut same_as_other),
            };
            if let Some(resolved) = found {
                return Match::Found(resolved);
            }
        }
        same_as_other.map_or(Match::NotFound, Match::SameAsOther)
    }

    fn by_name(
        &self,
        name: &str,
        workspace: bool,
        taken: &[PathBuf],
        same_as_other: &mut Option<PathBuf>,
    ) -> Option<Resolved> {
        let kinds: &[RecentKind] = if workspace {
            &[RecentKind::Workspace]
        } else {
            &[RecentKind::Folder, RecentKind::Workspace]
        };
        let lower = name.to_lowercase();
        for exact in [true, false] {
            for &kind in kinds {
                for recent in self.candidates.iter().filter(|r| r.kind == kind) {
                    let label = location_label(&recent.path, kind, self.os);
                    let matches = if exact {
                        label == name
                    } else {
                        label.to_lowercase() == lower
                    };
                    if matches
                        && let Some(found) = self.claim(&recent.path, kind, taken, same_as_other)
                    {
                        return Some(found);
                    }
                }
            }
        }
        None
    }

    fn by_path(
        &self,
        shown: &str,
        taken: &[PathBuf],
        same_as_other: &mut Option<PathBuf>,
    ) -> Option<Resolved> {
        let path = expand_home(shown, self.home)?;
        if (self.exists)(&path, RecentKind::Folder) {
            return self.claim(&path, RecentKind::Folder, taken, same_as_other);
        }
        if paths::is_workspace_file(&path) && (self.exists)(&path, RecentKind::Workspace) {
            return self.claim(&path, RecentKind::Workspace, taken, same_as_other);
        }
        if (self.exists)(&path, RecentKind::File) {
            // A file (`${activeEditorLong}`): the most relevant known folder containing it.
            for recent in self
                .candidates
                .iter()
                .filter(|r| r.kind == RecentKind::Folder)
            {
                if path.starts_with(&recent.path)
                    && let Some(found) =
                        self.claim(&recent.path, RecentKind::Folder, taken, same_as_other)
                {
                    return Some(found);
                }
            }
        }
        None
    }

    /// `path` if it exists and is not assigned to another window yet.
    fn claim(
        &self,
        path: &Path,
        kind: RecentKind,
        taken: &[PathBuf],
        same_as_other: &mut Option<PathBuf>,
    ) -> Option<Resolved> {
        if !(self.exists)(path, kind) {
            return None;
        }
        let key = path_key(path, self.os);
        if taken.iter().any(|t| path_key(t, self.os) == key) {
            same_as_other.get_or_insert_with(|| path.to_path_buf());
            return None;
        }
        Some(Resolved {
            path: path.to_path_buf(),
            is_workspace_file: kind == RecentKind::Workspace,
        })
    }
}

/// How VS Code names a location in titles: the folder name, or the workspace file name
/// without `.code-workspace`.
pub fn location_label(path: &Path, kind: RecentKind, os: OsKind) -> String {
    let full = path.to_string_lossy();
    let name = paths::file_name(&full, os);
    match kind {
        RecentKind::Workspace => workspace_name(name).to_string(),
        // A filesystem root ("/").
        _ if name.is_empty() => full.into_owned(),
        _ => name.to_string(),
    }
}

/// `team.code-workspace` → `team`.
pub fn workspace_name(file_name: &str) -> &str {
    const EXTENSION: &str = ".code-workspace";
    let bytes = file_name.as_bytes();
    if bytes.len() > EXTENSION.len()
        && bytes[bytes.len() - EXTENSION.len()..].eq_ignore_ascii_case(EXTENSION.as_bytes())
    {
        &file_name[..file_name.len() - EXTENSION.len()]
    } else {
        file_name
    }
}

/// Comparison key for paths (Windows paths are case-insensitive).
pub fn path_key(path: &Path, os: OsKind) -> String {
    let s = path.to_string_lossy();
    if os == OsKind::Windows {
        s.replace('/', r"\").to_lowercase()
    } else {
        s.into_owned()
    }
}

/// Concatenate `lists` (most relevant first), dropping repeated paths.
pub fn merge(lists: &[&[Recent]], os: OsKind) -> Vec<Recent> {
    let mut seen = HashSet::new();
    lists
        .iter()
        .flat_map(|list| list.iter())
        .filter(|r| seen.insert(path_key(&r.path, os)))
        .cloned()
        .collect()
}

fn expand_home(shown: &str, home: Option<&Path>) -> Option<PathBuf> {
    if shown == "~" {
        return home.map(Path::to_path_buf);
    }
    match shown
        .strip_prefix("~/")
        .or_else(|| shown.strip_prefix(r"~\"))
    {
        Some(rest) => home.map(|h| h.join(rest)),
        None => Some(PathBuf::from(shown)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::vscode::title;

    const APPS: [&str; 1] = ["Visual Studio Code"];

    fn fs_exists(path: &Path, kind: RecentKind) -> bool {
        match kind {
            RecentKind::Folder => path.is_dir(),
            RecentKind::Workspace | RecentKind::File => path.is_file(),
        }
    }

    fn resolve(
        candidates: &[Recent],
        title: &str,
        taken: &[PathBuf],
        home: Option<&Path>,
    ) -> Match {
        let resolver = Resolver {
            candidates,
            os: OsKind::current(),
            home,
            exists: &fs_exists,
        };
        resolver.resolve(&title::parse(title, &APPS, &[]), taken)
    }

    fn found(path: &Path, is_workspace_file: bool) -> Match {
        Match::Found(Resolved {
            path: path.to_path_buf(),
            is_workspace_file,
        })
    }

    #[test]
    fn prefers_the_most_recent_existing_match() {
        let tmp = tempfile::tempdir().unwrap();
        let gone = tmp.path().join("old").join("api");
        let client = tmp.path().join("client").join("api");
        let server = tmp.path().join("server").join("api");
        std::fs::create_dir_all(&client).unwrap();
        std::fs::create_dir_all(&server).unwrap();
        let candidates = vec![
            Recent::new(gone, RecentKind::Folder),
            Recent::new(client.clone(), RecentKind::Folder),
            Recent::new(server.clone(), RecentKind::Folder),
        ];
        let title = "main.rs - api - Visual Studio Code";
        assert_eq!(
            resolve(&candidates, title, &[], None),
            found(&client, false)
        );
        // A second window with the same name gets the next match...
        assert_eq!(
            resolve(&candidates, title, std::slice::from_ref(&client), None),
            found(&server, false)
        );
        // ...and a third one shows a project that is already captured.
        assert_eq!(
            resolve(&candidates, title, &[client.clone(), server.clone()], None),
            Match::SameAsOther(client)
        );
        assert_eq!(
            resolve(&candidates, "lib.rs - web - Visual Studio Code", &[], None),
            Match::NotFound
        );
    }

    #[test]
    fn workspaces_folders_and_case() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("Platform");
        std::fs::create_dir_all(&folder).unwrap();
        let workspace = tmp.path().join("platform.code-workspace");
        std::fs::write(&workspace, "{}").unwrap();
        let file = tmp.path().join("notes.md");
        std::fs::write(&file, "x").unwrap();
        let candidates = vec![
            Recent::new(file, RecentKind::File),
            Recent::new(folder.clone(), RecentKind::Folder),
            Recent::new(workspace.clone(), RecentKind::Workspace),
        ];
        assert_eq!(
            resolve(
                &candidates,
                "platform (Workspace) - Visual Studio Code",
                &[],
                None
            ),
            found(&workspace, true)
        );
        assert_eq!(
            resolve(&candidates, "Platform - Visual Studio Code", &[], None),
            found(&folder, false)
        );
        // Exact names (folders first, then workspaces) win over case-insensitive matches:
        // "platform" exactly names the workspace file, "PLATFORM" names nothing exactly.
        assert_eq!(
            resolve(&candidates, "platform - Visual Studio Code", &[], None),
            found(&workspace, true)
        );
        assert_eq!(
            resolve(&candidates, "PLATFORM - Visual Studio Code", &[], None),
            found(&folder, false)
        );
        // Single files are never projects.
        assert_eq!(
            resolve(&candidates, "notes.md - Visual Studio Code", &[], None),
            Match::NotFound
        );
    }

    #[test]
    fn paths_shown_in_titles() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("code").join("reframe");
        std::fs::create_dir_all(project.join("src")).unwrap();
        let source = project.join("src").join("main.rs");
        std::fs::write(&source, "fn main() {}").unwrap();
        let candidates = vec![Recent::new(project.clone(), RecentKind::Folder)];

        let shown = format!("main.rs - {} - Visual Studio Code", project.display());
        assert_eq!(resolve(&[], &shown, &[], None), found(&project, false));
        // `~` is expanded with the home directory.
        if OsKind::current() != OsKind::Windows {
            assert_eq!(
                resolve(
                    &[],
                    "~/code/reframe - Visual Studio Code",
                    &[],
                    Some(tmp.path())
                ),
                found(&project, false)
            );
        }
        // A file path resolves to the known folder that contains it.
        let long = format!("{} - Visual Studio Code", source.display());
        assert_eq!(
            resolve(&candidates, &long, &[], None),
            found(&project, false)
        );
        assert_eq!(resolve(&[], &long, &[], None), Match::NotFound);
    }

    #[test]
    fn labels_keys_and_merging() {
        let l = OsKind::Linux;
        assert_eq!(
            location_label(
                Path::new("/w/team.code-workspace"),
                RecentKind::Workspace,
                l
            ),
            "team"
        );
        assert_eq!(
            location_label(Path::new("/home/me/reframe"), RecentKind::Folder, l),
            "reframe"
        );
        assert_eq!(location_label(Path::new("/"), RecentKind::Folder, l), "/");
        assert_eq!(
            location_label(
                Path::new(r"C:\src\Team.Code-Workspace"),
                RecentKind::Workspace,
                OsKind::Windows
            ),
            "Team"
        );
        assert_eq!(workspace_name(".code-workspace"), ".code-workspace");
        assert_eq!(
            path_key(Path::new(r"C:\Src/App"), OsKind::Windows),
            path_key(Path::new(r"c:\src\app"), OsKind::Windows)
        );
        let a = [
            Recent::new("/a".into(), RecentKind::Folder),
            Recent::new("/b".into(), RecentKind::Folder),
        ];
        let b = [
            Recent::new("/b".into(), RecentKind::Folder),
            Recent::new("/c.code-workspace".into(), RecentKind::Workspace),
        ];
        let merged = merge(&[&a, &b], l);
        assert_eq!(
            merged
                .iter()
                .map(|r| r.path.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["/a", "/b", "/c.code-workspace"]
        );
    }
}
