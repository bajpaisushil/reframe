//! Launching applications on Linux: executables, Flatpak, Snap, AppImage and `xdg-open`.

use std::path::{Path, PathBuf};

use crate::common::launch::spawn_detached;
use crate::{LaunchRequest, LaunchTarget, LaunchedProcess, PackageInfo, PackageKind};
use crate::{PlatformError, PlatformResult};

/// Flatpak application ids and snap names are restricted to these characters; anything else
/// (spaces, a leading `-` that would be parsed as an option, path separators) is rejected.
fn valid_package_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn find_tool(tool: &str, what: &str) -> PlatformResult<PathBuf> {
    which::which(tool).map_err(|_| {
        PlatformError::launch_failed(
            what,
            format!("the `{tool}` command is not installed or not on PATH"),
        )
    })
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

/// Environment for the child: the request's variables, plus `DISPLAY` when this platform
/// instance targets a specific X display and the request does not set it itself.
fn child_env(request: &LaunchRequest, display: Option<&str>) -> Vec<(String, String)> {
    let mut env = request.env.clone();
    if let Some(display) = display
        && !env.iter().any(|(k, _)| k == "DISPLAY")
    {
        env.push(("DISPLAY".to_string(), display.to_string()));
    }
    env
}

/// Start `request` detached from Reframe.
///
/// - `Executable`: spawned directly; the pid is the application's.
/// - Flatpak: `flatpak run <id> <args>`; the returned pid is the launcher's, the application
///   runs as one of its descendants (inside the sandbox).
/// - Snap: `snap run <name> <args>`; `snap run` execs into the application, keeping the pid.
/// - AppImage: the `.AppImage` file is executed (it must already be executable; Reframe never
///   changes file permissions).
/// - `Open`: `xdg-open <target>`; the final process is unknown (`pid: None`) and extra
///   arguments are ignored.
pub(crate) fn launch(
    request: &LaunchRequest,
    display: Option<&str>,
) -> PlatformResult<LaunchedProcess> {
    let env = child_env(request, display);
    let cwd = request.working_directory.as_deref();
    match &request.target {
        LaunchTarget::Executable { path } => {
            if path.as_os_str().is_empty() {
                return Err(PlatformError::launch_failed(
                    "(empty path)",
                    "no executable was given",
                ));
            }
            let pid = spawn_detached(path, &request.args, cwd, &env)?;
            Ok(LaunchedProcess { pid: Some(pid) })
        }
        LaunchTarget::Package { package } => launch_package(package, request, cwd, &env),
        LaunchTarget::Open { target } => {
            let target = target.trim();
            if target.is_empty() || target.starts_with('-') {
                return Err(PlatformError::launch_failed(
                    format!("\"{target}\""),
                    "not a file, folder or URL that can be opened",
                ));
            }
            let xdg_open = find_tool("xdg-open", target)?;
            if !request.args.is_empty() {
                log::debug!("xdg-open ignores extra arguments for {target}");
            }
            spawn_detached(&xdg_open, &[target.to_string()], cwd, &env)?;
            Ok(LaunchedProcess { pid: None })
        }
    }
}

fn launch_package(
    package: &PackageInfo,
    request: &LaunchRequest,
    cwd: Option<&Path>,
    env: &[(String, String)],
) -> PlatformResult<LaunchedProcess> {
    let (tool, kind_name) = match package.kind {
        PackageKind::Flatpak => ("flatpak", "Flatpak"),
        PackageKind::Snap => ("snap", "Snap"),
        PackageKind::AppImage => return launch_appimage(package, request, cwd, env),
        PackageKind::MacBundle | PackageKind::WindowsPackage => {
            return Err(PlatformError::unsupported(
                format!("Launching {}", package.id),
                "macOS app bundles and Windows packages cannot be started on Linux.",
            ));
        }
    };
    if !valid_package_id(&package.id) {
        return Err(PlatformError::launch_failed(
            &package.id,
            format!(
                "\"{}\" is not a valid {kind_name} application id",
                package.id
            ),
        ));
    }
    let program = find_tool(tool, &package.id)?;
    let mut args = vec!["run".to_string(), package.id.clone()];
    args.extend(request.args.iter().cloned());
    let pid = spawn_detached(&program, &args, cwd, env)?;
    Ok(LaunchedProcess { pid: Some(pid) })
}

fn launch_appimage(
    package: &PackageInfo,
    request: &LaunchRequest,
    cwd: Option<&Path>,
    env: &[(String, String)],
) -> PlatformResult<LaunchedProcess> {
    let path = package
        .path
        .clone()
        .or_else(|| Some(PathBuf::from(&package.id)).filter(|p| p.is_absolute()))
        .ok_or_else(|| {
            PlatformError::launch_failed(&package.id, "the AppImage file location is unknown")
        })?;
    let label = path.display().to_string();
    if !path.is_file() {
        return Err(PlatformError::launch_failed(
            label,
            "the AppImage file no longer exists",
        ));
    }
    if !is_executable(&path) {
        return Err(PlatformError::launch_failed(
            label,
            "the AppImage file is not marked as executable (chmod +x)",
        ));
    }
    let pid = spawn_detached(&path, &request.args, cwd, env)?;
    Ok(LaunchedProcess { pid: Some(pid) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::testutil::TempDir;

    fn package(kind: PackageKind, id: &str, path: Option<PathBuf>) -> LaunchRequest {
        LaunchRequest {
            target: LaunchTarget::Package {
                package: PackageInfo {
                    kind,
                    id: id.into(),
                    path,
                },
            },
            args: vec![],
            working_directory: None,
            env: vec![],
        }
    }

    #[test]
    fn package_ids_are_validated() {
        assert!(valid_package_id("org.gimp.GIMP"));
        assert!(valid_package_id("firefox_esr"));
        assert!(valid_package_id("lxd.lxc"));
        for bad in ["", "--command=sh", "a b", "../x", "x;y", "-x"] {
            assert!(!valid_package_id(bad), "{bad:?}");
        }
        let err = launch(&package(PackageKind::Flatpak, "--command=sh", None), None).unwrap_err();
        assert!(matches!(err, PlatformError::LaunchFailed { .. }));
    }

    #[test]
    fn executables_launch_and_report_pid() {
        let launched = launch(
            &LaunchRequest::executable("/bin/sh").with_args(["-c", "exit 0"]),
            None,
        )
        .unwrap();
        assert!(launched.pid.is_some_and(|p| p > 0));
        let err = launch(&LaunchRequest::executable("/no/such/program"), None).unwrap_err();
        assert!(matches!(err, PlatformError::LaunchFailed { .. }));
        let err = launch(&LaunchRequest::executable(""), None).unwrap_err();
        assert!(matches!(err, PlatformError::LaunchFailed { .. }));
    }

    #[test]
    fn appimages_must_exist_and_be_executable() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new("appimage");
        let plain = tmp.write("Plain-1.0.AppImage", "#!/bin/sh\nexit 0\n");
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o644)).unwrap();
        let req = package(PackageKind::AppImage, "Plain-1.0", Some(plain));
        let err = launch(&req, None).unwrap_err();
        assert!(err.to_string().contains("executable"), "{err}");
        // An executable image (a link to /bin/true, so no freshly written file is executed,
        // which could fail with ETXTBSY while other tests fork).
        let image = tmp.path().join("Tool-1.0.AppImage");
        std::os::unix::fs::symlink("/bin/true", &image).unwrap();
        let req = package(PackageKind::AppImage, "Tool-1.0", Some(image.clone()));
        assert!(launch(&req, None).unwrap().pid.is_some());
        // The id may carry the absolute path when `path` is missing.
        let by_id = package(PackageKind::AppImage, image.to_str().unwrap(), None);
        assert!(launch(&by_id, None).unwrap().pid.is_some());
        let missing = package(
            PackageKind::AppImage,
            "Gone",
            Some(tmp.path().join("Gone.AppImage")),
        );
        assert!(
            launch(&missing, None)
                .unwrap_err()
                .to_string()
                .contains("no longer exists")
        );
        let unknown = package(PackageKind::AppImage, "Relative", None);
        assert!(matches!(
            launch(&unknown, None).unwrap_err(),
            PlatformError::LaunchFailed { .. }
        ));
    }

    #[test]
    fn missing_launch_tools_are_reported() {
        // Only checked where the tool is absent: never actually runs flatpak/snap/xdg-open.
        let cases = [
            (
                "flatpak",
                package(PackageKind::Flatpak, "org.example.App", None),
            ),
            ("snap", package(PackageKind::Snap, "example", None)),
            (
                "xdg-open",
                LaunchRequest {
                    target: LaunchTarget::Open {
                        target: "/tmp".into(),
                    },
                    args: vec![],
                    working_directory: None,
                    env: vec![],
                },
            ),
        ];
        for (tool, request) in cases {
            if which::which(tool).is_ok() {
                continue;
            }
            let err = launch(&request, None).unwrap_err();
            assert!(matches!(err, PlatformError::LaunchFailed { .. }), "{err:?}");
            assert!(err.to_string().contains(tool), "{err}");
        }
    }

    #[test]
    fn foreign_packages_are_unsupported() {
        let err = launch(
            &package(PackageKind::MacBundle, "com.apple.Safari", None),
            None,
        )
        .unwrap_err();
        assert!(err.is_unsupported());
    }

    #[test]
    fn open_targets_are_checked() {
        let open = |target: &str| LaunchRequest {
            target: LaunchTarget::Open {
                target: target.into(),
            },
            args: vec![],
            working_directory: None,
            env: vec![],
        };
        assert!(matches!(
            launch(&open("  "), None).unwrap_err(),
            PlatformError::LaunchFailed { .. }
        ));
        assert!(matches!(
            launch(&open("--help"), None).unwrap_err(),
            PlatformError::LaunchFailed { .. }
        ));
    }

    #[test]
    fn display_is_added_unless_overridden() {
        let mut req = LaunchRequest::executable("/bin/true");
        assert_eq!(
            child_env(&req, Some(":91")),
            vec![("DISPLAY".to_string(), ":91".to_string())]
        );
        assert!(child_env(&req, None).is_empty());
        req.env = vec![("DISPLAY".into(), ":5".into())];
        assert_eq!(child_env(&req, Some(":91")), req.env);
    }
}
