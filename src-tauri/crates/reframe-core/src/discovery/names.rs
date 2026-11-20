//! OS-aware normalization of process names, executable paths and package ids, shared by window
//! grouping, classification and exclusion matching.
//!
//! The rules follow the *reported* platform (`PlatformInfo::os`), not the OS Reframe was compiled
//! for, so Windows paths are handled the same way whatever machine runs the tests.

use std::path::Path;

use reframe_platform::{OsKind, PackageInfo, PackageKind};

/// Marker Linux appends to `/proc/<pid>/exe` when a running program's binary was replaced
/// (typically by a package update). Old and new instances are still the same application.
const DELETED_SUFFIX: &str = " (deleted)";

/// Executables that run *other* programs (scripts, Windows programs under Wine). Many unrelated
/// applications share one of these binaries, so their processes are told apart by process name.
/// Version digits are ignored (`python3.12` → `python`).
const SCRIPT_HOSTS: &[&str] = &[
    "gjs",
    "gjs-console",
    "java",
    "javaw",
    "mono",
    "mono-sgen",
    "node",
    "nodejs",
    "perl",
    "pypy",
    "python",
    "pythonw",
    "ruby",
    "tclsh",
    "wine",
    "wine-preloader",
    "wine64-preloader",
    "wish",
];

/// The executable path as text, trimmed, without Linux's " (deleted)" marker.
fn path_text(path: &Path, os: OsKind) -> String {
    let text = path.to_string_lossy();
    let text = text.trim();
    let text = if os == OsKind::Linux {
        text.strip_suffix(DELETED_SUFFIX).unwrap_or(text)
    } else {
        text
    };
    text.to_string()
}

/// Lower-case on Windows (where names and paths are case-insensitive), unchanged elsewhere.
pub(crate) fn fold_case(text: &str, os: OsKind) -> String {
    if os == OsKind::Windows {
        text.to_lowercase()
    } else {
        text.to_string()
    }
}

/// Grouping form of an executable path: case-insensitive with a single separator style and no
/// `\\?\` prefix on Windows; verbatim elsewhere (minus Linux's deleted-binary marker).
pub(crate) fn exe_path_key(path: &Path, os: OsKind) -> String {
    let text = path_text(path, os);
    if os == OsKind::Windows {
        let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
        text.replace('/', "\\").to_lowercase()
    } else {
        text
    }
}

/// File name of an executable path (`C:\Apps\Code.exe` → `Code.exe`). Windows paths are split on
/// both separators so the result does not depend on the host OS.
pub(crate) fn file_name(path: &Path, os: OsKind) -> String {
    let text = path_text(path, os);
    let name = if os == OsKind::Windows {
        text.rsplit(['\\', '/']).next()
    } else {
        text.rsplit('/').next()
    };
    name.unwrap_or_default().to_string()
}

/// Lower-cased file stem of an executable path (`C:\Apps\Code.exe` → `code`,
/// `/usr/bin/python3.12` → `python3`).
pub(crate) fn exe_stem(path: &Path, os: OsKind) -> String {
    strip_extension(&file_name(path, os)).to_lowercase()
}

/// `name.ext` → `name`; names starting with a dot and names without one are kept.
pub(crate) fn strip_extension(file: &str) -> &str {
    match file.rfind('.') {
        Some(i) if i > 0 => &file[..i],
        _ => file,
    }
}

/// Lower-cased process name without a trailing `.exe` (`TextInputHost.exe` → `textinputhost`).
pub(crate) fn process_name(name: &str) -> String {
    crate::util::normalized_process_name(name.trim())
}

/// Lower-cased identifiers of a package for table and exclusion matching. For Windows packages
/// the package name is included as well (`Microsoft.LockApp_cw5n1h2txyewy!WindowsDefaultLockScreen`
/// → also `microsoft.lockapp`).
pub(crate) fn package_names(package: &PackageInfo) -> Vec<String> {
    let id = package.id.trim().to_lowercase();
    if id.is_empty() {
        return Vec::new();
    }
    match package.kind {
        PackageKind::WindowsPackage => {
            let name = aumid_package_name(&id);
            if name.is_empty() || name == id {
                vec![id]
            } else {
                vec![id.clone(), name]
            }
        }
        _ => vec![id],
    }
}

/// Package name part of a Windows AppUserModelID / package family name (before `_` or `!`),
/// lower-cased.
pub(crate) fn aumid_package_name(aumid: &str) -> String {
    aumid
        .split(['_', '!'])
        .next()
        .unwrap_or_default()
        .trim()
        .to_lowercase()
}

/// True when `stem` (a lower-cased executable stem) is a script interpreter or program host.
pub(crate) fn is_script_host(stem: &str) -> bool {
    let base = stem.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    !base.is_empty() && SCRIPT_HOSTS.contains(&base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn windows_paths_are_case_and_separator_insensitive() {
        let a = exe_path_key(Path::new(r"C:\Program Files\App\App.exe"), OsKind::Windows);
        let b = exe_path_key(
            Path::new(r"\\?\c:/program files/app/APP.EXE"),
            OsKind::Windows,
        );
        assert_eq!(a, b);
        assert_eq!(a, r"c:\program files\app\app.exe");
    }

    #[test]
    fn unix_paths_are_kept_verbatim_except_deleted_marker() {
        assert_eq!(
            exe_path_key(Path::new("/usr/bin/Foo"), OsKind::Macos),
            "/usr/bin/Foo"
        );
        assert_eq!(
            exe_path_key(
                Path::new("/usr/lib/firefox/firefox (deleted)"),
                OsKind::Linux
            ),
            "/usr/lib/firefox/firefox"
        );
    }

    #[test]
    fn stems_and_names() {
        assert_eq!(
            exe_stem(
                Path::new(r"C:\Windows\SystemApps\X\TextInputHost.exe"),
                OsKind::Windows
            ),
            "textinputhost"
        );
        assert_eq!(
            exe_stem(Path::new("/usr/bin/python3.12"), OsKind::Linux),
            "python3"
        );
        assert_eq!(
            file_name(&PathBuf::from("/opt/App (deleted)"), OsKind::Linux),
            "App"
        );
        assert_eq!(strip_extension(".hidden"), ".hidden");
        assert_eq!(process_name(" NVIDIA Share.exe "), "nvidia share");
        assert_eq!(fold_case("Foo", OsKind::Linux), "Foo");
        assert_eq!(fold_case("Foo", OsKind::Windows), "foo");
    }

    #[test]
    fn package_names_include_windows_package_name() {
        let pkg = PackageInfo {
            kind: PackageKind::WindowsPackage,
            id: "Microsoft.LockApp_cw5n1h2txyewy!WindowsDefaultLockScreen".into(),
            path: None,
        };
        assert_eq!(
            package_names(&pkg),
            vec![
                "microsoft.lockapp_cw5n1h2txyewy!windowsdefaultlockscreen".to_string(),
                "microsoft.lockapp".to_string()
            ]
        );
        let flatpak = PackageInfo {
            kind: PackageKind::Flatpak,
            id: " ".into(),
            path: None,
        };
        assert!(package_names(&flatpak).is_empty());
    }

    #[test]
    fn script_hosts_ignore_versions() {
        for host in [
            "python3",
            "python",
            "pythonw",
            "perl5",
            "wine64",
            "gjs-console",
            "java",
        ] {
            assert!(is_script_host(host), "{host}");
        }
        for app in ["gimp", "code", "3", "", "firefox"] {
            assert!(!is_script_host(app), "{app}");
        }
    }
}
