//! Browser profiles: which profile a browser process uses and where its session files are.
//!
//! - The profile comes from `--profile-directory=` on the browser's command line, then
//!   `profile.last_used` / `profile.last_active_profiles` in the user data directory's
//!   `Local State` file, then `Default`.
//! - A custom user data directory comes from `--user-data-dir=`.
//! - Session files are `<profile>/Sessions/Session_<timestamp>` (current browsers; the
//!   timestamp is microseconds since 1601) or `<profile>/Current Session` (older ones).
//!   Encrypted sessions live in `<profile>/Sessions_Encrypted/` and cannot be read.
//!
//! Everything read here is untrusted: profile names are validated as a single plain path
//! component before they are joined to a path, and file reads are size-bounded.

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use reframe_platform::OsKind;

/// Name of the profile directory used when nothing else is known.
pub const DEFAULT_PROFILE: &str = "Default";

/// Most profiles considered for one capture.
pub const MAX_CANDIDATE_PROFILES: usize = 8;

/// Largest `Local State` file read.
const MAX_LOCAL_STATE_BYTES: u64 = 16 * 1024 * 1024;

/// Directory entries examined in a `Sessions` folder.
const MAX_DIRECTORY_ENTRIES: usize = 2_000;

/// Switches of a Chromium command line that matter to Reframe.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Switches {
    pub profile_directory: Option<String>,
    pub user_data_dir: Option<String>,
}

/// Parse a Chromium command line like `base::CommandLine`: switches start with `--` or `-`,
/// carry their value after `=`, and parsing stops at a bare `--`. Switch names are
/// case-insensitive on Windows only. The last occurrence of a switch wins.
pub fn parse_switches(command_line: &[String], os: OsKind) -> Switches {
    let mut switches = Switches::default();
    for arg in command_line.iter().skip(1) {
        let arg = arg.trim();
        if arg == "--" {
            break;
        }
        let Some(body) = arg.strip_prefix("--").or_else(|| arg.strip_prefix('-')) else {
            continue;
        };
        let Some((name, value)) = body.split_once('=') else {
            continue;
        };
        let name = if os == OsKind::Windows {
            name.to_ascii_lowercase()
        } else {
            name.to_string()
        };
        let value = unquote(value);
        match name.as_str() {
            "profile-directory" => switches.profile_directory = Some(value),
            "user-data-dir" => switches.user_data_dir = Some(value),
            _ => {}
        }
    }
    switches
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
        .to_string()
}

/// Absolute user data directory from a `--user-data-dir` value: `~/` is expanded and relative
/// paths are resolved against the browser's working directory (when known).
pub fn resolve_user_data_dir(
    value: &str,
    cwd: Option<&Path>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let path = match value.strip_prefix("~/") {
        Some(rest) => home?.join(rest),
        None => PathBuf::from(value),
    };
    if path.is_absolute() {
        Some(path)
    } else {
        cwd.filter(|c| c.is_absolute()).map(|c| c.join(path))
    }
}

/// Profile hints from the user data directory's `Local State` file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalState {
    pub last_used: Option<String>,
    /// Profiles with open windows when the browser last saved this file.
    pub last_active: Vec<String>,
    /// Every profile the browser knows (`profile.info_cache`), sorted.
    pub known: Vec<String>,
}

/// Parse `Local State` (JSON). Anything unexpected yields empty hints.
pub fn parse_local_state(bytes: &[u8]) -> LocalState {
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return LocalState::default();
    };
    let profile = &json["profile"];
    let mut known: Vec<String> = profile["info_cache"]
        .as_object()
        .map(|cache| cache.keys().take(64).cloned().collect())
        .unwrap_or_default();
    known.sort();
    LocalState {
        last_used: profile["last_used"].as_str().map(str::to_string),
        last_active: profile["last_active_profiles"]
            .as_array()
            .map(|names| {
                names
                    .iter()
                    .filter_map(|n| n.as_str())
                    .take(MAX_CANDIDATE_PROFILES)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        known,
    }
}

/// Read and parse `<user_data_dir>/Local State` (missing or unreadable: no hints).
pub fn read_local_state(user_data_dir: &Path) -> LocalState {
    read_bounded(&user_data_dir.join("Local State"), MAX_LOCAL_STATE_BYTES)
        .map(|bytes| parse_local_state(&bytes))
        .unwrap_or_default()
}

/// A profile directory name that is safe to join to the user data directory: one plain path
/// component, not the guest or system profile (whose sessions are never saved).
pub fn valid_profile_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name.trim() == name
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':'])
        && !name.chars().any(char::is_control)
        && name != "Guest Profile"
        && name != "System Profile"
}

/// Profiles to look at, most likely first: the command-line profile, the last used one, the
/// ones active at the last save, `Default`, then any other known profile (a profile opened
/// moments ago may not be recorded as active yet). Invalid names are dropped.
pub fn candidate_profiles(
    command_line_profile: Option<&str>,
    local_state: &LocalState,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let hints = command_line_profile
        .into_iter()
        .chain(local_state.last_used.as_deref())
        .chain(local_state.last_active.iter().map(String::as_str))
        .chain(std::iter::once(DEFAULT_PROFILE))
        .chain(local_state.known.iter().map(String::as_str));
    for name in hints {
        if valid_profile_name(name) && !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
        if out.len() == MAX_CANDIDATE_PROFILES {
            break;
        }
    }
    out
}

/// A session file found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFile {
    pub path: PathBuf,
    pub modified: Option<SystemTime>,
}

/// Plain-text session files of a profile, newest first: `Sessions/Session_<timestamp>` by
/// timestamp (then modification time), followed by the legacy `Current Session`.
pub fn session_files(profile_dir: &Path) -> Vec<SessionFile> {
    let mut files = timestamped_session_files(&profile_dir.join("Sessions"));
    let legacy = profile_dir.join("Current Session");
    if legacy.is_file() {
        files.push(SessionFile {
            modified: modified(&legacy),
            path: legacy,
        });
    }
    files
}

/// The newest encrypted session file of a profile (`Sessions_Encrypted/`), if any.
pub fn newest_encrypted_session(profile_dir: &Path) -> Option<SessionFile> {
    timestamped_session_files(&profile_dir.join("Sessions_Encrypted"))
        .into_iter()
        .next()
}

fn timestamped_session_files(dir: &Path) -> Vec<SessionFile> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(u64, Option<SystemTime>, PathBuf)> = entries
        .take(MAX_DIRECTORY_ENTRIES)
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let timestamp = session_timestamp(name.to_str()?)?;
            let path = entry.path();
            path.is_file().then(|| (timestamp, modified(&path), path))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    found
        .into_iter()
        .map(|(_, modified, path)| SessionFile { path, modified })
        .collect()
}

/// `Session_13350000000000000` → its timestamp. `Tabs_` and `Apps_` files are not sessions.
fn session_timestamp(name: &str) -> Option<u64> {
    let digits = name.strip_prefix("Session_")?;
    if digits.is_empty() || digits.len() > 20 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Read at most `max` bytes of a file in one pass. Files larger than `max` are refused.
///
/// The browser keeps its session file open and appends to it; reading it once into memory
/// gives the same snapshot as copying it to a temporary file first, without ever writing
/// browsing data to disk. Rust opens files with full sharing on Windows, so the browser is
/// never blocked.
pub fn read_bounded(path: &Path, max: u64) -> io::Result<Vec<u8>> {
    // Only regular files: opening a FIFO or a device could block or never end.
    if !std::fs::metadata(path)?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a regular file", path.display()),
        ));
    }
    let file = File::open(path)?;
    let mut data = Vec::new();
    file.take(max.saturating_add(1)).read_to_end(&mut data)?;
    if u64::try_from(data.len()).unwrap_or(u64::MAX) > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is larger than {max} bytes", path.display()),
        ));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_profile_and_user_data_switches() {
        let s = parse_switches(
            &args(&[
                "chrome",
                "--profile-directory=Profile 2",
                "--user-data-dir=/tmp/udd",
                "https://example.com/",
            ]),
            OsKind::Linux,
        );
        assert_eq!(s.profile_directory.as_deref(), Some("Profile 2"));
        assert_eq!(s.user_data_dir.as_deref(), Some("/tmp/udd"));

        let s = parse_switches(
            &args(&[
                "chrome.exe",
                "-Profile-Directory=\"Work\"",
                "--",
                "--user-data-dir=ignored",
            ]),
            OsKind::Windows,
        );
        assert_eq!(s.profile_directory.as_deref(), Some("Work"));
        assert_eq!(s.user_data_dir, None);

        // Switch names are case-sensitive outside Windows; argv[0] is never a switch.
        let s = parse_switches(
            &args(&["--profile-directory=argv0", "--Profile-Directory=X"]),
            OsKind::Linux,
        );
        assert_eq!(s, Switches::default());
    }

    #[cfg(unix)] // POSIX absolute paths
    #[test]
    fn resolves_user_data_dir_values() {
        let home = Path::new("/home/me");
        let cwd = Path::new("/work");
        assert_eq!(
            resolve_user_data_dir("/abs/dir", Some(cwd), Some(home)),
            Some(PathBuf::from("/abs/dir"))
        );
        assert_eq!(
            resolve_user_data_dir("~/profiles/a", None, Some(home)),
            Some(PathBuf::from("/home/me/profiles/a"))
        );
        assert_eq!(
            resolve_user_data_dir("rel/dir", Some(cwd), None),
            Some(PathBuf::from("/work/rel/dir"))
        );
        assert_eq!(resolve_user_data_dir("rel/dir", None, None), None);
        assert_eq!(resolve_user_data_dir("  ", Some(cwd), None), None);
    }

    #[test]
    fn local_state_hints() {
        let json = br#"{"profile":{"last_used":"Profile 1",
            "last_active_profiles":["Profile 1","Default",42],
            "info_cache":{"Default":{"name":"Person 1"}}}}"#;
        let state = parse_local_state(json);
        assert_eq!(state.last_used.as_deref(), Some("Profile 1"));
        assert_eq!(state.last_active, ["Profile 1", "Default"]);
        assert_eq!(state.known, ["Default"]);
        assert_eq!(parse_local_state(b"not json"), LocalState::default());
        assert_eq!(parse_local_state(b"[1,2]"), LocalState::default());
    }

    #[test]
    fn profile_names_are_validated() {
        for ok in ["Default", "Profile 1", "Profile 12", "Work"] {
            assert!(valid_profile_name(ok), "{ok}");
        }
        for bad in [
            "",
            ".",
            "..",
            "../etc",
            "a/b",
            r"a\b",
            "C:",
            " Default",
            "x\n",
            "Guest Profile",
            "System Profile",
        ] {
            assert!(!valid_profile_name(bad), "{bad:?}");
        }
        let long = "p".repeat(256);
        assert!(!valid_profile_name(&long));
    }

    #[test]
    fn candidate_profile_order() {
        let state = LocalState {
            last_used: Some("Profile 1".into()),
            last_active: vec!["Profile 2".into(), "Profile 1".into(), "../evil".into()],
            known: vec!["Default".into(), "Profile 4".into()],
        };
        assert_eq!(
            candidate_profiles(Some("Profile 3"), &state),
            [
                "Profile 3",
                "Profile 1",
                "Profile 2",
                "Default",
                "Profile 4"
            ]
        );
        assert_eq!(
            candidate_profiles(None, &LocalState::default()),
            ["Default"]
        );
        let many = LocalState {
            last_active: (0..20).map(|i| format!("Profile {i}")).collect(),
            ..LocalState::default()
        };
        assert_eq!(
            candidate_profiles(None, &many).len(),
            MAX_CANDIDATE_PROFILES
        );
    }

    #[test]
    fn finds_newest_session_files() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().join("Sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        for name in [
            "Session_13300000000000000",
            "Session_13350000000000000",
            "Tabs_13360000000000000",
            "Apps_13360000000000000",
            "Session_notanumber",
            "Session_",
        ] {
            std::fs::write(sessions.join(name), b"x").unwrap();
        }
        std::fs::create_dir(sessions.join("Session_13399999999999999")).unwrap();
        std::fs::write(dir.path().join("Current Session"), b"x").unwrap();
        let names: Vec<String> = session_files(dir.path())
            .iter()
            .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "Session_13350000000000000",
                "Session_13300000000000000",
                "Current Session"
            ]
        );
        assert!(newest_encrypted_session(dir.path()).is_none());
        let encrypted = dir.path().join("Sessions_Encrypted");
        std::fs::create_dir_all(&encrypted).unwrap();
        std::fs::write(encrypted.join("Session_13350000000000001"), b"x").unwrap();
        assert!(newest_encrypted_session(dir.path()).is_some());
        assert!(session_files(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn bounded_reads() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, vec![7u8; 100]).unwrap();
        assert_eq!(read_bounded(&file, 100).unwrap().len(), 100);
        assert_eq!(
            read_bounded(&file, 99).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(read_bounded(&dir.path().join("missing"), 10).is_err());
        assert_eq!(
            read_bounded(dir.path(), 10).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
}
