//! SumatraPDF's settings file (`SumatraPDF-settings.txt`): recently opened documents and the
//! page each was last shown at.
//!
//! The file lives next to `SumatraPDF.exe` in portable mode, otherwise in
//! `%LOCALAPPDATA%\SumatraPDF`. Its `FileStates` list has one entry per document with
//! `FilePath` and `PageNo` (1-based). SumatraPDF rewrites the file when a document is closed or
//! a setting changes, so the page of a document that is still open can lag behind what is on
//! screen. Only `FilePath`, `PageNo` and `IsMissing` are read; the file also holds values such
//! as decryption keys, which are never read or stored. Pre-3.0 binary settings
//! (`sumatrapdfprefs.dat`) are not supported.

use std::path::{Path, PathBuf};

pub(super) const SETTINGS_FILE: &str = "SumatraPDF-settings.txt";

/// Settings files are a few hundred KB at most; larger files are ignored.
pub(super) const MAX_SETTINGS_BYTES: u64 = 16 << 20;

/// One `FileStates` entry.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct FileState {
    pub path: String,
    pub page: Option<u32>,
    /// SumatraPDF noticed the file is gone.
    pub missing: bool,
}

/// Where the settings file may be: next to the executable (portable mode) first, then
/// `%LOCALAPPDATA%\SumatraPDF`.
pub(super) fn settings_locations(
    executable: Option<&Path>,
    local_app_data: Option<&Path>,
) -> Vec<PathBuf> {
    let mut locations = Vec::new();
    if let Some(dir) = executable.and_then(Path::parent) {
        locations.push(dir.join(SETTINGS_FILE));
    }
    if let Some(base) = local_app_data {
        let path = base.join("SumatraPDF").join(SETTINGS_FILE);
        if !locations.contains(&path) {
            locations.push(path);
        }
    }
    locations
}

/// Read the first settings file that exists.
pub(super) fn load(locations: &[PathBuf]) -> Vec<FileState> {
    for path in locations {
        let Ok(metadata) = std::fs::metadata(path) else {
            continue;
        };
        if !metadata.is_file() || metadata.len() > MAX_SETTINGS_BYTES {
            log::debug!("Ignoring SumatraPDF settings at {}", path.display());
            continue;
        }
        match std::fs::read(path) {
            Ok(bytes) => return parse_file_states(&String::from_utf8_lossy(&bytes)),
            Err(error) => log::debug!("Could not read {}: {error}", path.display()),
        }
    }
    Vec::new()
}

/// `FileStates` entries of a settings file. Tolerates a BOM, CRLF line endings and unknown
/// sections; values of nested lists (e.g. `Favorites`, which have their own `PageNo`) and of
/// other sections (`SessionData`) are ignored.
pub(super) fn parse_file_states(text: &str) -> Vec<FileState> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut states = Vec::new();
    // Section path: "FileStates" then "" for each list element.
    let mut stack: Vec<&str> = Vec::new();
    let mut current: Option<FileState> = None;
    let in_file_state =
        |stack: &[&str]| stack.len() == 2 && stack[0] == "FileStates" && stack[1].is_empty();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line == "]" {
            if in_file_state(&stack)
                && let Some(state) = current.take()
                && !state.path.is_empty()
            {
                states.push(state);
            }
            stack.pop();
            continue;
        }
        if let Some(name) = line.strip_suffix('[')
            && !name.contains('=')
        {
            stack.push(name.trim());
            if in_file_state(&stack) {
                current = Some(FileState::default());
            }
            continue;
        }
        if !in_file_state(&stack) {
            continue;
        }
        let (Some(state), Some((key, value))) = (current.as_mut(), line.split_once('=')) else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "FilePath" => state.path = value.to_string(),
            "PageNo" => state.page = value.parse::<u32>().ok().filter(|p| *p >= 1),
            "IsMissing" => state.missing = value.eq_ignore_ascii_case("true"),
            _ => {}
        }
    }
    states
}

/// Windows paths compare case-insensitively and with either separator.
pub(super) fn same_path(a: &str, b: &str) -> bool {
    let normalize = |s: &str| s.replace('/', "\\").to_lowercase();
    normalize(a.trim_end_matches(['\\', '/'])) == normalize(b.trim_end_matches(['\\', '/']))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "\u{feff}# SumatraPDF settings file\r\n\
MainWindowBackground = #80fff200\r\n\
EscToExit = false\r\n\
FixedPageUI [\r\n\
\tTextColor = #000000\r\n\
]\r\n\
FileStates [\r\n\
\t[\r\n\
\t\tFilePath = C:\\Users\\me\\Documents\\Thesis - final.pdf\r\n\
\t\tFavorites [\r\n\
\t\t\t[\r\n\
\t\t\t\tName = Results\r\n\
\t\t\t\tPageNo = 99\r\n\
\t\t\t]\r\n\
\t\t]\r\n\
\t\tIsPinned = false\r\n\
\t\tIsMissing = false\r\n\
\t\tOpenCount = 12\r\n\
\t\tDecryptionKey = 0123456789abcdef\r\n\
\t\tDisplayMode = automatic\r\n\
\t\tScrollPos = 0 0\r\n\
\t\tPageNo = 42\r\n\
\t\tZoom = fit page\r\n\
\t]\r\n\
\t[\r\n\
\t\tFilePath = D:\\Books\\gone.epub\r\n\
\t\tIsMissing = true\r\n\
\t\tPageNo = 3\r\n\
\t]\r\n\
\t[\r\n\
\t\tFilePath = C:\\Scans\\weird[.pdf\r\n\
\t\tPageNo = 0\r\n\
\t]\r\n\
\t[\r\n\
\t\tPageNo = 5\r\n\
\t]\r\n\
]\r\n\
SessionData [\r\n\
\t[\r\n\
\t\tTabStates [\r\n\
\t\t\t[\r\n\
\t\t\t\tFilePath = C:\\Session\\only.pdf\r\n\
\t\t\t\tPageNo = 7\r\n\
\t\t\t]\r\n\
\t\t]\r\n\
\t]\r\n\
]\r\n";

    #[test]
    fn parses_file_states_only() {
        assert_eq!(
            parse_file_states(FIXTURE),
            vec![
                FileState {
                    path: r"C:\Users\me\Documents\Thesis - final.pdf".into(),
                    page: Some(42),
                    missing: false,
                },
                FileState {
                    path: r"D:\Books\gone.epub".into(),
                    page: Some(3),
                    missing: true,
                },
                FileState {
                    path: r"C:\Scans\weird[.pdf".into(),
                    page: None,
                    missing: false,
                },
            ]
        );
    }

    #[test]
    fn garbage_and_truncated_files_do_not_panic() {
        assert!(parse_file_states("").is_empty());
        assert!(parse_file_states("]]]\n[[[\nFilePath = x\n").is_empty());
        let truncated = "FileStates [\n\t[\n\t\tFilePath = C:\\a.pdf\n\t\tPageNo = 2\n";
        assert!(parse_file_states(truncated).is_empty());
        assert_eq!(
            parse_file_states("FileStates [\n[\nFilePath = /a.pdf\nPageNo = x\n]\n]\n"),
            vec![FileState {
                path: "/a.pdf".into(),
                page: None,
                missing: false
            }]
        );
    }

    #[test]
    fn settings_locations_prefer_portable_mode() {
        let locations = settings_locations(
            Some(Path::new("/opt/sumatra/SumatraPDF.exe")),
            Some(Path::new("/home/me/AppData/Local")),
        );
        assert_eq!(
            locations,
            vec![
                PathBuf::from("/opt/sumatra").join(SETTINGS_FILE),
                PathBuf::from("/home/me/AppData/Local/SumatraPDF").join(SETTINGS_FILE),
            ]
        );
        assert!(settings_locations(None, None).is_empty());
    }

    #[test]
    fn loads_the_first_existing_settings_file() {
        let dir = tempfile::tempdir().unwrap();
        let portable = dir.path().join("portable").join(SETTINGS_FILE);
        let installed = dir.path().join("local").join(SETTINGS_FILE);
        std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
        std::fs::write(&installed, FIXTURE).unwrap();
        let states = load(&[portable, installed]);
        assert_eq!(states.len(), 3);
        assert!(load(&[dir.path().join("none")]).is_empty());
    }

    #[test]
    fn compares_windows_paths_loosely() {
        assert!(same_path(r"C:\Docs\A.pdf", "c:/docs/a.pdf"));
        assert!(!same_path(r"C:\Docs\A.pdf", r"C:\Docs\B.pdf"));
    }
}
