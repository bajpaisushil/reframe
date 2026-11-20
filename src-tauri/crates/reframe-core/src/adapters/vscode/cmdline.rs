//! Hints from the main VS Code process command line: the folders and workspace files it was
//! started with, and `--user-data-dir`. They only help match window titles to paths; nothing
//! from a command line is ever stored (it may contain secrets).

use std::path::{Component, Path, PathBuf};

use reframe_platform::OsKind;

use super::storage::{Recent, RecentKind};
use super::{paths, uri};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandLineHints {
    /// Folders / workspace files from positional arguments, `--folder-uri` and `--file-uri`.
    /// Not checked for existence.
    pub paths: Vec<Recent>,
    /// `--user-data-dir` (instances started with a custom data directory).
    pub user_data_dir: Option<PathBuf>,
}

/// Options followed by a separate value that is not a path to open (`--profile Work`).
const VALUE_OPTIONS: &[&str] = &[
    "--user-data-dir",
    "--extensions-dir",
    "--profile",
    "--locale",
    "--log",
    "--sync",
    "--folder-uri",
    "--file-uri",
    "--remote",
    "--extensionDevelopmentPath",
    "--extensionTestsPath",
    "--extensionDevelopmentKind",
    "--category",
    "--install-extension",
    "--uninstall-extension",
    "--enable-proposed-api",
    "--crash-reporter-directory",
    "--crash-reporter-id",
    "--disable-extension",
    "--max-memory",
    "--locate-extension",
    "--locate-shell-integration-path",
    "--add-mcp",
    "--logsPath",
    "--waitMarkerFilePath",
    "--inspect-extensions",
    "--inspect-brk-extensions",
    "--install-source",
    "--export-default-configuration",
];

/// Extract hints from `args` (`args[0]` is the executable). Relative paths are resolved
/// against `cwd`, the process working directory, when it is known (`code .`).
pub fn parse(args: &[String], cwd: Option<&Path>, os: OsKind) -> CommandLineHints {
    let mut hints = CommandLineHints::default();
    let mut args = args.iter().skip(1).map(String::as_str);
    let mut only_positional = false;
    while let Some(arg) = args.next() {
        if arg == "-" {
            continue;
        }
        if only_positional || !arg.starts_with('-') {
            if let Some(path) = local_path(arg, cwd, os) {
                hints.paths.push(Recent::guess(path));
            }
            continue;
        }
        if arg == "--" {
            only_positional = true;
            continue;
        }
        let (option, inline_value) = match arg.split_once('=') {
            Some((option, value)) => (option, Some(value)),
            None => (arg, None),
        };
        if !VALUE_OPTIONS.contains(&option) {
            continue;
        }
        let Some(value) = inline_value.or_else(|| args.next()) else {
            break;
        };
        match option {
            "--user-data-dir" => hints.user_data_dir = local_path(value, cwd, os),
            "--folder-uri" => {
                if let Some(path) = uri::file_uri_to_path(value, os) {
                    hints.paths.push(Recent::new(path, RecentKind::Folder));
                }
            }
            "--file-uri" => {
                if let Some(path) =
                    uri::file_uri_to_path(value, os).filter(|p| paths::is_workspace_file(p))
                {
                    hints.paths.push(Recent::new(path, RecentKind::Workspace));
                }
            }
            _ => {}
        }
    }
    hints
}

/// An absolute local path for a command-line path argument; URLs (`vscode://...`) and
/// `file:line:column` targets are not paths.
fn local_path(arg: &str, cwd: Option<&Path>, os: OsKind) -> Option<PathBuf> {
    let arg = arg.trim();
    if arg.is_empty() || arg.contains('\0') || uri::has_scheme(arg) {
        return None;
    }
    if paths::is_absolute(arg, os) {
        return Some(normalize(Path::new(arg)));
    }
    // Absolute on `os`, like `arg` (not on the OS Reframe was compiled for).
    let cwd = cwd.filter(|c| paths::is_absolute(&c.to_string_lossy(), os))?;
    Some(normalize(&cwd.join(arg)))
}

/// Remove `.` and resolve `..` components lexically (`/home/me/proj/.` → `/home/me/proj`).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn positional_paths_and_options() {
        let hints = parse(
            &args(&[
                "/usr/share/code/code",
                "--no-sandbox",
                "--profile",
                "Work",
                "--user-data-dir=/data/vscode",
                "--extensions-dir",
                "/data/ext",
                "/home/me/reframe",
                "/home/me/team.code-workspace",
                "--goto",
                "src/main.rs:10:5",
                "--folder-uri",
                "file:///home/me/My%20Notes",
                "--file-uri=file:///home/me/other.code-workspace",
                "--file-uri",
                "file:///home/me/readme.md",
                "--folder-uri",
                "vscode-remote://ssh-remote%2Bbox/srv",
                "vscode://vscode.git/clone?url=x",
                "-",
            ]),
            None,
            OsKind::Linux,
        );
        assert_eq!(hints.user_data_dir, Some(PathBuf::from("/data/vscode")));
        assert_eq!(
            hints.paths,
            vec![
                Recent::new("/home/me/reframe".into(), RecentKind::Folder),
                Recent::new("/home/me/team.code-workspace".into(), RecentKind::Workspace),
                Recent::new("/home/me/My Notes".into(), RecentKind::Folder),
                Recent::new(
                    "/home/me/other.code-workspace".into(),
                    RecentKind::Workspace
                ),
            ]
        );
    }

    #[test]
    fn relative_paths_use_the_working_directory() {
        let hints = parse(
            &args(&["code", ".", "../api", "--", "-odd-name"]),
            Some(Path::new("/home/me/reframe")),
            OsKind::Linux,
        );
        assert_eq!(
            hints
                .paths
                .iter()
                .map(|r| r.path.clone())
                .collect::<Vec<_>>(),
            vec![
                PathBuf::from("/home/me/reframe"),
                PathBuf::from("/home/me/api"),
                PathBuf::from("/home/me/reframe/-odd-name"),
            ]
        );
        // Without a working directory relative paths are dropped.
        assert!(
            parse(&args(&["code", "."]), None, OsKind::Linux)
                .paths
                .is_empty()
        );
        // The working directory is judged by the given OS, whatever OS runs the test.
        assert_eq!(
            parse(
                &args(&["Code.exe", "."]),
                Some(Path::new(r"C:\src\reframe")),
                OsKind::Windows
            )
            .paths,
            vec![Recent::new(
                PathBuf::from(r"C:\src\reframe"),
                RecentKind::Folder
            )]
        );
        assert!(
            parse(
                &args(&["code", "."]),
                Some(Path::new("relative/dir")),
                OsKind::Linux
            )
            .paths
            .is_empty()
        );
    }

    #[test]
    fn windows_command_lines() {
        let hints = parse(
            &args(&[
                r"C:\Program Files\Microsoft VS Code\Code.exe",
                r"C:\src\reframe",
                "--folder-uri",
                "file:///d%3A/work/api",
            ]),
            None,
            OsKind::Windows,
        );
        assert_eq!(
            hints.paths,
            vec![
                Recent::new(PathBuf::from(r"C:\src\reframe"), RecentKind::Folder),
                Recent::new(PathBuf::from(r"D:\work\api"), RecentKind::Folder),
            ]
        );
        // A trailing option without its value is ignored.
        assert_eq!(
            parse(&args(&["code", "--user-data-dir"]), None, OsKind::Linux),
            CommandLineHints::default()
        );
    }
}
