//! Path-string helpers that take an explicit [`OsKind`] (so Windows paths can be handled and
//! tested on any OS), and the locations of VS Code's user-data directory.

use std::path::{Path, PathBuf};

use reframe_platform::{OsKind, PackageInfo, PackageKind};

use super::variant::Variant;

fn is_separator(c: char, os: OsKind) -> bool {
    c == '/' || (os == OsKind::Windows && c == '\\')
}

/// Final component of `path` (`C:\a\Code.exe` → `Code.exe` on Windows); empty for a root.
pub fn file_name(path: &str, os: OsKind) -> &str {
    let trimmed = path.trim_end_matches(|c| is_separator(c, os));
    match trimmed.rfind(|c| is_separator(c, os)) {
        Some(i) => &trimmed[i + 1..],
        None => trimmed,
    }
}

/// `path` without its final component; `None` for a bare name or a root.
pub fn parent(path: &str, os: OsKind) -> Option<&str> {
    let trimmed = path.trim_end_matches(|c| is_separator(c, os));
    let i = trimmed.rfind(|c| is_separator(c, os))?;
    let parent = trimmed[..i].trim_end_matches(|c| is_separator(c, os));
    if parent.is_empty() {
        // The parent is the root itself ("/x" → "/").
        Some(&trimmed[..1])
    } else {
        Some(parent)
    }
}

/// `base` followed by `parts`, joined with the OS's separator.
pub fn join(base: &str, parts: &[&str], os: OsKind) -> String {
    let sep = if os == OsKind::Windows { '\\' } else { '/' };
    let mut out = base.trim_end_matches(|c| is_separator(c, os)).to_string();
    for part in parts {
        out.push(sep);
        out.push_str(part);
    }
    out
}

/// Whether `path` is absolute on `os` (`/x`; `C:\x`, `C:/x` or `\\server\share` on Windows).
pub fn is_absolute(path: &str, os: OsKind) -> bool {
    if os == OsKind::Windows {
        let b = path.as_bytes();
        (b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && (b[2] == b'\\' || b[2] == b'/'))
            || path.starts_with(r"\\")
            || path.starts_with("//")
    } else {
        path.starts_with('/')
    }
}

/// The outermost `.app` bundle containing a macOS path
/// (`/Applications/Visual Studio Code.app/Contents/MacOS/Electron` →
/// `/Applications/Visual Studio Code.app`).
pub fn mac_bundle_path(path: &str) -> Option<&str> {
    let mut offset = 0;
    for component in path.split('/') {
        let end = offset + component.len();
        let bytes = component.as_bytes();
        if bytes.len() > 4 && bytes[bytes.len() - 4..].eq_ignore_ascii_case(b".app") {
            return Some(&path[..end]);
        }
        offset = end + 1;
    }
    None
}

/// Whether `path` names a `.code-workspace` file.
pub fn is_workspace_file(path: &Path) -> bool {
    path.file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|n| n.ends_with(".code-workspace") && n.len() > ".code-workspace".len())
}

/// Machine-specific inputs for [`user_data_dirs`] (passed in so the lookup is testable).
#[derive(Debug, Clone, Default)]
pub struct UserDataInputs<'a> {
    /// `--user-data-dir` from the main process command line.
    pub cli_user_data_dir: Option<PathBuf>,
    /// The main process executable (portable installations keep their data next to it).
    pub executable: Option<&'a Path>,
    pub package: Option<&'a PackageInfo>,
    /// Per-user configuration root: `%APPDATA%`, `~/Library/Application Support`,
    /// `$XDG_CONFIG_HOME` or `~/.config`.
    pub config_dir: Option<PathBuf>,
    pub home: Option<PathBuf>,
    /// `VSCODE_PORTABLE` and `VSCODE_APPDATA` environment overrides.
    pub vscode_portable: Option<PathBuf>,
    pub vscode_appdata: Option<PathBuf>,
}

/// Candidate user-data directories for `variant`, most likely first. Mirrors VS Code's own
/// lookup (portable mode, `VSCODE_PORTABLE`, `VSCODE_APPDATA`, `--user-data-dir`, the per-OS
/// default) and adds Flatpak locations on Linux. Callers check which ones actually exist.
pub fn user_data_dirs(variant: Variant, os: OsKind, inputs: &UserDataInputs<'_>) -> Vec<PathBuf> {
    let name = variant.data_folder_name();
    let linux_like = matches!(os, OsKind::Linux | OsKind::Other);
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut push = |dir: PathBuf| {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    };
    if let Some(dir) = inputs
        .executable
        .and_then(Path::to_str)
        .and_then(|exe| portable_user_data_dir(exe, variant, os))
    {
        push(PathBuf::from(dir));
    }
    if let Some(dir) = &inputs.vscode_portable {
        push(dir.join("user-data"));
    }
    if let Some(dir) = &inputs.vscode_appdata {
        push(dir.join(name));
    }
    if let Some(dir) = &inputs.cli_user_data_dir {
        push(dir.clone());
    }
    if linux_like
        && let (Some(home), Some(package)) = (&inputs.home, inputs.package)
        && package.kind == PackageKind::Flatpak
    {
        push(flatpak_dir(home, &package.id, name));
    }
    if let Some(config) = &inputs.config_dir {
        push(config.join(name));
    }
    if linux_like && let Some(home) = &inputs.home {
        // The platform may not report Flatpak packages; these only matter if they exist.
        for id in variant.flatpak_ids() {
            push(flatpak_dir(home, id, name));
        }
    }
    dirs
}

fn flatpak_dir(home: &Path, app_id: &str, data_folder: &str) -> PathBuf {
    home.join(".var")
        .join("app")
        .join(app_id)
        .join("config")
        .join(data_folder)
}

/// Where a portable (zip/tar.gz) installation keeps its user data: `data/user-data` next to
/// the executable on Windows/Linux, `<app>-portable-data/user-data` next to the bundle on macOS.
fn portable_user_data_dir(executable: &str, variant: Variant, os: OsKind) -> Option<String> {
    match os {
        OsKind::Macos => {
            let bundle = mac_bundle_path(executable)?;
            let folder = format!("{}-portable-data", variant.application_name());
            Some(join(parent(bundle, os)?, &[&folder, "user-data"], os))
        }
        _ => Some(join(parent(executable, os)?, &["data", "user-data"], os)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_strings_per_os() {
        let w = OsKind::Windows;
        let l = OsKind::Linux;
        assert_eq!(
            file_name(r"C:\Program Files\VS Code\Code.exe", w),
            "Code.exe"
        );
        assert_eq!(
            file_name(r"C:\Program Files\VS Code\Code.exe", l),
            r"C:\Program Files\VS Code\Code.exe"
        );
        assert_eq!(file_name("/home/me/reframe/", l), "reframe");
        assert_eq!(file_name("/", l), "");
        assert_eq!(parent("/usr/share/code/code", l), Some("/usr/share/code"));
        assert_eq!(parent("/code", l), Some("/"));
        assert_eq!(parent("code", l), None);
        assert_eq!(parent(r"D:\VSCode\Code.exe", w), Some(r"D:\VSCode"));
        assert_eq!(
            join(r"D:\VSCode\", &["data", "user-data"], w),
            r"D:\VSCode\data\user-data"
        );
        assert_eq!(join("/", &["data"], l), "/data");
        assert!(
            is_absolute(r"C:\x", w) && is_absolute("c:/x", w) && is_absolute(r"\\nas\share", w)
        );
        assert!(!is_absolute("/x", w) && !is_absolute(r"x\y", w));
        assert!(is_absolute("/x", l) && !is_absolute("x/y", l) && !is_absolute(r"C:\x", l));
        assert_eq!(
            mac_bundle_path(
                "/Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper.app/Contents/MacOS/Code Helper"
            ),
            Some("/Applications/Visual Studio Code.app")
        );
        assert_eq!(mac_bundle_path("/usr/local/bin/code"), None);
        assert!(is_workspace_file(Path::new("/w/team.code-workspace")));
        assert!(is_workspace_file(Path::new("/w/Team.CODE-WORKSPACE")));
        assert!(!is_workspace_file(Path::new("/w/.code-workspace")));
        assert!(!is_workspace_file(Path::new("/w/team")));
    }

    #[test]
    fn default_user_data_dirs_per_os() {
        let inputs = UserDataInputs {
            config_dir: Some("/home/me/.config".into()),
            home: Some("/home/me".into()),
            ..Default::default()
        };
        assert_eq!(
            user_data_dirs(Variant::Stable, OsKind::Linux, &inputs),
            vec![
                PathBuf::from("/home/me/.config/Code"),
                PathBuf::from("/home/me/.var/app/com.visualstudio.code/config/Code"),
            ]
        );
        let mac = UserDataInputs {
            config_dir: Some("/Users/me/Library/Application Support".into()),
            home: Some("/Users/me".into()),
            ..Default::default()
        };
        assert_eq!(
            user_data_dirs(Variant::Insiders, OsKind::Macos, &mac),
            vec![PathBuf::from(
                "/Users/me/Library/Application Support/Code - Insiders"
            )]
        );
        let win = UserDataInputs {
            config_dir: Some(PathBuf::from(r"C:\Users\me\AppData\Roaming")),
            ..Default::default()
        };
        assert_eq!(
            user_data_dirs(Variant::Codium, OsKind::Windows, &win),
            vec![PathBuf::from(r"C:\Users\me\AppData\Roaming").join("VSCodium")]
        );
    }

    #[test]
    fn portable_flatpak_and_overrides_come_first() {
        let exe = PathBuf::from("/opt/VSCode-linux-x64/code");
        let flatpak = PackageInfo {
            kind: PackageKind::Flatpak,
            id: "com.visualstudio.code".into(),
            path: None,
        };
        let inputs = UserDataInputs {
            cli_user_data_dir: Some("/data/vscode-profile".into()),
            executable: Some(&exe),
            package: Some(&flatpak),
            config_dir: Some("/home/me/.config".into()),
            home: Some("/home/me".into()),
            vscode_portable: None,
            vscode_appdata: Some("/srv/appdata".into()),
        };
        assert_eq!(
            user_data_dirs(Variant::Stable, OsKind::Linux, &inputs),
            vec![
                PathBuf::from("/opt/VSCode-linux-x64/data/user-data"),
                PathBuf::from("/srv/appdata/Code"),
                PathBuf::from("/data/vscode-profile"),
                PathBuf::from("/home/me/.var/app/com.visualstudio.code/config/Code"),
                PathBuf::from("/home/me/.config/Code"),
            ]
        );
        let mac_exe = PathBuf::from("/Users/me/Tools/VSCodium.app/Contents/MacOS/Electron");
        let mac = UserDataInputs {
            executable: Some(&mac_exe),
            vscode_portable: Some("/Volumes/usb/vscodium".into()),
            ..Default::default()
        };
        assert_eq!(
            user_data_dirs(Variant::Codium, OsKind::Macos, &mac),
            vec![
                PathBuf::from("/Users/me/Tools/codium-portable-data/user-data"),
                PathBuf::from("/Volumes/usb/vscodium/user-data"),
            ]
        );
        assert_eq!(
            portable_user_data_dir(r"D:\Apps\VSCode\Code.exe", Variant::Stable, OsKind::Windows),
            Some(r"D:\Apps\VSCode\data\user-data".to_string())
        );
    }
}
