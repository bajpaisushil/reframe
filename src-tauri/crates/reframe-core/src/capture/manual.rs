//! Applications the user adds by picking an executable, an app bundle or a package instead of
//! capturing a running one.
//!
//! The target is validated against this machine (the file must exist and be startable here),
//! identified through the adapters like a running process would be, and turned into an entry
//! that only launches the application: no state, no saved windows.
//!
//! Limitations: Flatpak, Snap and Windows packages cannot be checked for installation without
//! running external tools, so only their ids are validated; macOS bundles whose `Info.plist` is
//! binary keep the bundle path as their executable (see [`super::plist`]).

use std::path::{Path, PathBuf};

use reframe_platform::{LaunchTarget, OsKind, PackageInfo, PackageKind, Platform, ProcessInfo};

use super::plist;
use crate::adapters::AdapterRegistry;
use crate::discovery::{guard, names};
use crate::error::{CoreError, CoreResult};
use crate::model::{ApplicationEntry, LaunchConfig};
use crate::util;

/// Trailing segments of reverse-DNS package ids that do not name the application
/// (`com.spotify.Client`, `org.telegram.desktop`).
const GENERIC_ID_SEGMENTS: &[&str] = &["app", "application", "client", "desktop", "gui", "main"];

/// Entry for an application the user added by picking an executable / package / app bundle.
///
/// - `Executable`: must be an existing, absolute, runnable file (`.exe`/`.com`/`.bat`/`.cmd` on
///   Windows, executable permission elsewhere). A macOS `.app` bundle is converted to a
///   `MacBundle` package using `CFBundleIdentifier` from its XML `Info.plist`; when that cannot be
///   read (binary plist) the bundle path is kept as the executable.
/// - `Package`: must belong to this OS; bundle and AppImage paths must exist.
/// - `Open` (documents, folders, URLs) is rejected.
pub fn manual_entry(
    platform: &dyn Platform,
    adapters: &AdapterRegistry,
    target: LaunchTarget,
) -> CoreResult<ApplicationEntry> {
    let os = platform.info().os;
    let resolved = match target {
        LaunchTarget::Open { .. } => {
            return Err(CoreError::validation(
                "Choose an application, not a document.",
            ));
        }
        LaunchTarget::Executable { path } => resolve_executable(path, os)?,
        LaunchTarget::Package { package } => resolve_package(package, os)?,
    };

    let mut issues = guard::AdapterIssues::default();
    let identified = guard::identify(adapters, &resolved.process, &[], &mut issues);
    // Only the adapter id is needed; running it through the fallback guard also covers an
    // adapter whose `id()` panics.
    let outcome = guard::with_fallback(
        adapters,
        identified,
        &resolved.process,
        &[],
        &mut issues,
        |_, _| (),
    );
    let display_name = match outcome.identity.display_name.trim() {
        "" => guard::process_label(&resolved.process),
        name => name.to_string(),
    };
    let mut entry =
        ApplicationEntry::new(outcome.identity.app_id, display_name, outcome.adapter_id);
    entry.launch = resolved.launch;
    Ok(entry)
}

/// Launch configuration plus a stand-in process used to identify the application.
struct Resolved {
    launch: LaunchConfig,
    process: ProcessInfo,
}

fn stand_in(name: String, product_name: Option<String>) -> ProcessInfo {
    ProcessInfo {
        pid: 0,
        parent_pid: None,
        name,
        product_name,
        executable_path: None,
        command_line: Vec::new(),
        working_directory: None,
        start_time: None,
        package: None,
    }
}

fn invalid(message: String) -> CoreError {
    CoreError::Validation(vec![message])
}

/// File name for messages (`/usr/bin/gimp` → `gimp`).
fn label(path: &Path) -> String {
    util::path_label(path)
}

fn not_found(path: &Path) -> CoreError {
    invalid(format!(
        "{} was not found. It may have been moved or uninstalled.",
        path.display()
    ))
}

fn resolve_executable(path: PathBuf, os: OsKind) -> CoreResult<Resolved> {
    if path.as_os_str().is_empty() {
        return Err(CoreError::validation("Choose an application."));
    }
    if !path.is_absolute() {
        return Err(invalid(format!(
            "{} is not a full path. Choose the application file.",
            path.display()
        )));
    }
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(not_found(&path)),
        Err(e) => {
            return Err(invalid(format!(
                "Reframe cannot access {}: {e}.",
                path.display()
            )));
        }
    };
    if metadata.is_dir() {
        if is_app_bundle(&path) {
            return resolve_app_bundle(path, os);
        }
        return Err(invalid(format!(
            "{} is a folder. Choose the application file inside it.",
            label(&path)
        )));
    }
    check_runnable(&path, &metadata, os)?;

    let mut process = stand_in(names::file_name(&path, os), None);
    process.command_line = vec![path.display().to_string()];
    process.executable_path = Some(path.clone());
    Ok(Resolved {
        launch: LaunchConfig {
            executable: Some(path),
            ..LaunchConfig::default()
        },
        process,
    })
}

fn is_app_bundle(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
}

/// Name of a bundle as Finder shows it (`/Applications/Visual Studio Code.app` →
/// `Visual Studio Code`).
fn bundle_name(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| label(path))
}

/// A `.app` bundle picked as an executable: launched through its bundle identifier when the
/// `Info.plist` is readable, else by path.
fn resolve_app_bundle(path: PathBuf, os: OsKind) -> CoreResult<Resolved> {
    let name = bundle_name(&path);
    if os != OsKind::Macos {
        return Err(invalid(format!(
            "{name} is a macOS application and cannot be started on this system."
        )));
    }
    let mut process = stand_in(name.clone(), Some(name));
    let launch = match plist::read_bundle_info(&path) {
        Some(info) => {
            let package = PackageInfo {
                kind: PackageKind::MacBundle,
                id: info.identifier,
                path: Some(path),
            };
            process.executable_path = info.executable;
            process.package = Some(package.clone());
            LaunchConfig {
                package: Some(package),
                ..LaunchConfig::default()
            }
        }
        None => {
            // Binary or unreadable Info.plist: keep the bundle path.
            process.executable_path = Some(path.clone());
            LaunchConfig {
                executable: Some(path),
                ..LaunchConfig::default()
            }
        }
    };
    Ok(Resolved { launch, process })
}

/// Reject files that cannot be started as programs, with a message saying why.
fn check_runnable(path: &Path, metadata: &std::fs::Metadata, os: OsKind) -> CoreResult<()> {
    let name = label(path);
    let extension = path.extension().map(|e| e.to_string_lossy().to_lowercase());
    match extension.as_deref() {
        Some("desktop") => {
            return Err(invalid(format!(
                "{name} is a desktop shortcut. Choose the program it starts instead."
            )));
        }
        Some("lnk" | "url") => {
            return Err(invalid(format!(
                "{name} is a shortcut. Choose the program it points to instead."
            )));
        }
        _ => {}
    }
    if os == OsKind::Windows {
        if !matches!(extension.as_deref(), Some("exe" | "com" | "bat" | "cmd")) {
            return Err(invalid(format!(
                "{name} is not a program. Choose an .exe file."
            )));
        }
        return Ok(());
    }
    if !is_executable(metadata) {
        return Err(invalid(format!(
            "{name} is not a program Reframe can start (it is not marked as executable)."
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &std::fs::Metadata) -> bool {
    // No executable bit to check (non-Windows rules on a non-Unix host only happen in tests).
    true
}

fn resolve_package(mut package: PackageInfo, os: OsKind) -> CoreResult<Resolved> {
    package.id = package.id.trim().to_string();
    let (required, kind_label) = match package.kind {
        PackageKind::MacBundle => (OsKind::Macos, "a macOS application"),
        PackageKind::WindowsPackage => (OsKind::Windows, "a Windows Store application"),
        PackageKind::Flatpak => (OsKind::Linux, "a Flatpak application"),
        PackageKind::Snap => (OsKind::Linux, "a Snap application"),
        PackageKind::AppImage => (OsKind::Linux, "an AppImage"),
    };
    let what = match (&package.path, package.id.is_empty()) {
        (Some(path), _) => label(path),
        (None, false) => package.id.clone(),
        (None, true) => "This application".to_string(),
    };
    if os != required {
        return Err(invalid(format!(
            "{what} is {kind_label} and cannot be started on this system."
        )));
    }
    if let Some(path) = &package.path
        && !path.exists()
    {
        return Err(not_found(path));
    }

    let mut process = stand_in(String::new(), None);
    match package.kind {
        PackageKind::MacBundle => {
            if let Some(path) = package.path.clone() {
                if !path.is_dir() {
                    return Err(invalid(format!(
                        "{} is not an application bundle.",
                        label(&path)
                    )));
                }
                let info = plist::read_bundle_info(&path);
                if package.id.is_empty() {
                    package.id = info
                        .as_ref()
                        .map(|i| i.identifier.clone())
                        .unwrap_or_default();
                }
                process.executable_path = info.and_then(|i| i.executable);
            }
            if package.id.is_empty() {
                return Err(invalid(format!(
                    "Reframe could not read the bundle identifier of {what}."
                )));
            }
        }
        PackageKind::AppImage => {
            let Some(path) = package.path.clone() else {
                return Err(CoreError::validation("Choose the .AppImage file."));
            };
            let metadata = std::fs::metadata(&path).map_err(|_| not_found(&path))?;
            if !metadata.is_file() {
                return Err(invalid(format!(
                    "{} is not an AppImage file.",
                    label(&path)
                )));
            }
            if !is_executable(&metadata) {
                return Err(invalid(format!(
                    "{} is not marked as executable. Allow it to run as a program and try again.",
                    label(&path)
                )));
            }
            if package.id.is_empty() {
                package.id = appimage_name(&path);
            }
            process.executable_path = Some(path);
        }
        PackageKind::WindowsPackage | PackageKind::Flatpak | PackageKind::Snap => {
            if package.id.is_empty() {
                return Err(CoreError::validation("The application id is missing."));
            }
            let invalid_char = |c: char| {
                c.is_whitespace()
                    || c.is_control()
                    || (package.kind != PackageKind::WindowsPackage && c == '/')
            };
            if package.id.chars().any(invalid_char) {
                return Err(invalid(format!(
                    "{} is not a valid application id.",
                    package.id
                )));
            }
        }
    }

    let name = package_display_name(&package);
    process.name = name.clone();
    process.product_name = Some(name);
    process.package = Some(package.clone());
    Ok(Resolved {
        launch: LaunchConfig {
            package: Some(package),
            ..LaunchConfig::default()
        },
        process,
    })
}

/// `Obsidian-1.5.3.AppImage` → `Obsidian`: the file stem up to the first `-`/`_` followed by a
/// digit (where version numbers usually start).
fn appimage_name(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().trim().to_string())
        .unwrap_or_default();
    let cut = stem
        .char_indices()
        .zip(stem.chars().skip(1))
        .find(|&((_, c), next)| (c == '-' || c == '_') && next.is_ascii_digit())
        .map(|((i, _), _)| i);
    match cut {
        Some(i) if i > 0 => stem[..i].to_string(),
        _ if stem.is_empty() => "AppImage".to_string(),
        _ => stem,
    }
}

/// Friendly name for a package that is not running (no window title or product name to use).
fn package_display_name(package: &PackageInfo) -> String {
    let name = match package.kind {
        PackageKind::MacBundle => package
            .path
            .as_deref()
            .map(bundle_name)
            .unwrap_or_else(|| reverse_dns_name(&package.id)),
        PackageKind::AppImage => package
            .path
            .as_deref()
            .map(appimage_name)
            .unwrap_or_else(|| package.id.clone()),
        PackageKind::Snap => util::prettify_name(&package.id),
        PackageKind::Flatpak => reverse_dns_name(&package.id),
        PackageKind::WindowsPackage => {
            // `Microsoft.WindowsCalculator_8wekyb3d8bbwe!App` → `Microsoft.WindowsCalculator`.
            let package_name = package.id.split(['_', '!']).next().unwrap_or_default();
            reverse_dns_name(package_name)
        }
    };
    if name.trim().is_empty() {
        package.id.clone()
    } else {
        name
    }
}

/// Application name from a reverse-DNS id: the last meaningful segment, words split
/// (`org.gnome.TextEditor` → `Text Editor`, `com.spotify.Client` → `Spotify`).
fn reverse_dns_name(id: &str) -> String {
    let segments: Vec<&str> = id.split('.').filter(|s| !s.trim().is_empty()).collect();
    let segment = match segments.as_slice() {
        [] => return String::new(),
        [.., previous, last] if GENERIC_ID_SEGMENTS.contains(&last.to_lowercase().as_str()) => {
            *previous
        }
        [.., last] => *last,
    };
    if segment.contains(['-', '_']) {
        util::prettify_name(segment)
    } else {
        capitalize(&split_camel_case(segment))
    }
}

/// `WindowsCalculator` → `Windows Calculator`, `VLCPlayer` → `VLC Player`, `GIMP` → `GIMP`.
fn split_camel_case(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if i > 0 && c.is_uppercase() {
            let previous = chars[i - 1];
            let next_is_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if previous.is_lowercase()
                || previous.is_ascii_digit()
                || (previous.is_uppercase() && next_is_lower)
            {
                out.push(' ');
            }
        }
        out.push(c);
    }
    out
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_dns_names() {
        assert_eq!(reverse_dns_name("org.gnome.TextEditor"), "Text Editor");
        assert_eq!(reverse_dns_name("org.gimp.GIMP"), "GIMP");
        assert_eq!(reverse_dns_name("org.mozilla.firefox"), "Firefox");
        assert_eq!(reverse_dns_name("com.spotify.Client"), "Spotify");
        assert_eq!(reverse_dns_name("org.telegram.desktop"), "Telegram");
        assert_eq!(reverse_dns_name("org.gnome.font-viewer"), "Font Viewer");
        assert_eq!(
            reverse_dns_name("Microsoft.WindowsCalculator"),
            "Windows Calculator"
        );
        assert_eq!(reverse_dns_name("client"), "Client");
        assert_eq!(reverse_dns_name(""), "");
    }

    #[test]
    fn camel_case_splitting() {
        assert_eq!(split_camel_case("VLCPlayer"), "VLC Player");
        assert_eq!(split_camel_case("Win11Tool"), "Win11 Tool");
        assert_eq!(split_camel_case("already split"), "already split");
    }

    #[test]
    fn appimage_names_drop_versions() {
        assert_eq!(
            appimage_name(Path::new("/opt/Obsidian-1.5.3.AppImage")),
            "Obsidian"
        );
        assert_eq!(
            appimage_name(Path::new("/opt/balenaEtcher-1.18.11-x64.AppImage")),
            "balenaEtcher"
        );
        assert_eq!(appimage_name(Path::new("/opt/Krita.AppImage")), "Krita");
        assert_eq!(appimage_name(Path::new("/opt/-1.AppImage")), "-1");
    }

    #[test]
    fn package_display_names() {
        let pkg = |kind, id: &str, path: Option<&str>| PackageInfo {
            kind,
            id: id.into(),
            path: path.map(Into::into),
        };
        assert_eq!(
            package_display_name(&pkg(
                PackageKind::WindowsPackage,
                "Microsoft.WindowsTerminal_8wekyb3d8bbwe!App",
                None
            )),
            "Windows Terminal"
        );
        assert_eq!(
            package_display_name(&pkg(PackageKind::Snap, "spotify", None)),
            "Spotify"
        );
        assert_eq!(
            package_display_name(&pkg(
                PackageKind::MacBundle,
                "com.microsoft.VSCode",
                Some("/Applications/Visual Studio Code.app")
            )),
            "Visual Studio Code"
        );
        assert_eq!(
            package_display_name(&pkg(PackageKind::MacBundle, "com.apple.Safari", None)),
            "Safari"
        );
    }
}
