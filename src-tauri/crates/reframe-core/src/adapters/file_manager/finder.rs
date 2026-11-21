//! macOS Finder: which folder each Finder window shows, asked through AppleScript.
//!
//! `osascript` asks Finder for every window's name and target folder (front to back); the
//! names are matched to the selected windows' titles. It runs only when a workspace is
//! captured, never for the Explore preview. The first query triggers macOS's Automation
//! permission prompt ("Reframe wants to control Finder"); until it is granted — or when Finder
//! does not answer within a few seconds — no folders are captured and a warning explains why.
//!
//! Limitations: windows showing no folder (Recents, AirDrop, search results) are skipped; only
//! the front tab of a window is known; when window titles are unavailable (no Accessibility
//! permission) every Finder window's folder is captured.

use std::path::PathBuf;
use std::time::Duration;

use reframe_platform::WindowInfo;

use super::support;
use crate::model::FolderRef;

/// Generous enough for a busy Finder; the permission prompt itself is never waited for.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Name and POSIX path of every Finder window, front to back, separated by `character id 31`
/// (fields) and `character id 30` (records). Windows without a folder target are skipped.
const SCRIPT: &str = r#"with timeout of 4 seconds
	tell application id "com.apple.finder"
		set out to ""
		repeat with w in (every Finder window)
			try
				set out to out & (name of w) & (character id 31) & (POSIX path of (target of w as alias)) & (character id 30)
			end try
		end repeat
		return out
	end tell
end timeout"#;

/// One Finder window as reported by AppleScript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FinderWindow {
    pub name: String,
    pub path: PathBuf,
}

/// Folders shown in the selected Finder windows, plus warnings.
pub(super) fn capture(windows: &[&WindowInfo]) -> (Vec<FolderRef>, Vec<String>) {
    if windows.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let records = support::run_applescript(SCRIPT, TIMEOUT)
        .map(|raw| parse_output(&raw))
        .map_err(|e| e.to_string());
    folders_for_windows(windows, records)
}

/// Parse the script output; records without an absolute path are dropped.
pub(super) fn parse_output(raw: &str) -> Vec<FinderWindow> {
    support::applescript_records(raw)
        .into_iter()
        .filter_map(|fields| match fields.as_slice() {
            [name, path] if path.starts_with('/') => Some(FinderWindow {
                name: name.clone(),
                // POSIX paths of folders end with "/"; keep it only for the root.
                path: PathBuf::from(match path.trim_end_matches('/') {
                    "" => "/",
                    trimmed => trimmed,
                }),
            }),
            _ => None,
        })
        .collect()
}

/// Assign Finder windows to the selected windows by title: exact name first, then
/// case-insensitive, each Finder window used once (both lists are front to back, so two
/// windows with the same name are paired in order).
pub(super) fn folders_for_windows(
    windows: &[&WindowInfo],
    records: Result<Vec<FinderWindow>, String>,
) -> (Vec<FolderRef>, Vec<String>) {
    let records = match records {
        Ok(records) => records,
        Err(error) => {
            return (
                Vec::new(),
                vec![format!(
                    "could not ask Finder for its windows ({error}). If macOS asked, allow \
                     Reframe to control Finder in System Settings > Privacy & Security > \
                     Automation."
                )],
            );
        }
    };

    if windows.iter().all(|w| w.title.trim().is_empty()) {
        // No titles to match (Accessibility permission missing): every folder Finder shows.
        let folders = records
            .into_iter()
            .map(|r| FolderRef {
                path: r.path,
                window_title: Some(r.name),
            })
            .collect();
        return (folders, Vec::new());
    }

    let mut used = vec![false; records.len()];
    let mut folders = Vec::new();
    let mut warnings = Vec::new();
    for window in windows {
        let title = window.title.trim();
        let exact = (0..records.len()).find(|&i| !used[i] && records[i].name == title);
        let index = exact.or_else(|| {
            (0..records.len()).find(|&i| !used[i] && records[i].name.eq_ignore_ascii_case(title))
        });
        match index {
            Some(i) => {
                used[i] = true;
                folders.push(FolderRef {
                    path: records[i].path.clone(),
                    window_title: Some(window.title.clone()),
                });
            }
            None => warnings.push(format!(
                "the Finder window \"{title}\" does not show a folder (Recents, AirDrop or a \
                 search); it was skipped."
            )),
        }
    }
    (folders, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::file_manager::support::testing::window;

    fn record(name: &str, path: &str) -> FinderWindow {
        FinderWindow {
            name: name.to_string(),
            path: PathBuf::from(path),
        }
    }

    #[test]
    fn parses_records() {
        let raw = "Downloads\u{1f}/Users/me/Downloads/\u{1e}src\u{1f}/Users/me/a/src/\u{1e}\
                   broken\u{1e}odd\u{1f}relative/\u{1e}Macintosh HD\u{1f}/\u{1e}\n";
        assert_eq!(
            parse_output(raw),
            vec![
                record("Downloads", "/Users/me/Downloads"),
                record("src", "/Users/me/a/src"),
                record("Macintosh HD", "/"),
            ]
        );
    }

    #[test]
    fn pairs_windows_by_name_in_order() {
        let records = vec![
            record("src", "/Users/me/a/src/"),
            record("Downloads", "/Users/me/Downloads/"),
            record("src", "/Users/me/b/src/"),
        ];
        let w1 = window("1", 1, "src", Some("com.apple.finder"));
        let w2 = window("2", 1, "downloads", Some("com.apple.finder"));
        let w3 = window("3", 1, "src", Some("com.apple.finder"));
        let w4 = window("4", 1, "Recents", Some("com.apple.finder"));
        let (folders, warnings) = folders_for_windows(&[&w1, &w2, &w3, &w4], Ok(records));
        let paths: Vec<_> = folders.iter().map(|f| f.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/Users/me/a/src/"),
                PathBuf::from("/Users/me/Downloads/"),
                PathBuf::from("/Users/me/b/src/"),
            ]
        );
        assert_eq!(folders[1].window_title.as_deref(), Some("downloads"));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("Recents"));
    }

    #[test]
    fn untitled_windows_take_every_folder() {
        let records = vec![record("a", "/a/"), record("b", "/b/")];
        let w = window("1", 1, "", None);
        let (folders, warnings) = folders_for_windows(&[&w], Ok(records));
        assert_eq!(folders.len(), 2);
        assert!(warnings.is_empty());
    }

    #[test]
    fn errors_explain_the_automation_permission() {
        let w = window("1", 1, "Downloads", None);
        let (folders, warnings) = folders_for_windows(&[&w], Err("no answer in time".into()));
        assert!(folders.is_empty());
        assert!(warnings[0].contains("Automation"));
    }

    #[test]
    fn script_reports_names_and_paths() {
        assert!(SCRIPT.contains("POSIX path of (target of w as alias)"));
        assert!(SCRIPT.contains("character id 30"));
        if !cfg!(target_os = "macos") {
            let w = window("1", 1, "Downloads", None);
            let (folders, warnings) = capture(&[&w]);
            assert!(folders.is_empty());
            assert!(warnings[0].contains("only available on macOS"));
        }
    }
}
