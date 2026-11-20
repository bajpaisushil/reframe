//! Pre-launch checks shared by the orchestrator, the switch planner and the health report:
//! does the configured program exist, does the working folder exist.

use std::path::Path;

use crate::portability::is_network_path;

use reframe_platform::common::locate;
use reframe_platform::{LaunchRequest, LaunchTarget, LocateQuery};

/// Message for an executable (or package file) that is not where the workspace expects it.
pub(crate) fn not_installed_message(app: &str, path: &Path) -> String {
    format!(
        "{app} isn't installed at {} - edit the application to choose its location",
        path.display()
    )
}

/// Whether `path` names an existing program. Paths with a directory part must exist; a bare
/// program name (`code`) is looked up on `PATH`, as the OS would when launching it.
pub(crate) fn executable_exists(path: &Path) -> bool {
    if path.as_os_str().is_empty() {
        return false;
    }
    // Never probe network locations (on Windows that sends the user's credentials); the OS
    // reports a clear error at launch if the program is really missing.
    if is_network_path(path) || path.exists() {
        return true;
    }
    let bare_name = !path.is_absolute() && path.components().count() == 1;
    bare_name && {
        let query = LocateQuery {
            executable_names: vec![path.to_string_lossy().into_owned()],
            ..Default::default()
        };
        !locate::find_on_path(&query).is_empty()
    }
}

/// Check that `target` can be started. Packages without a file path and URLs cannot be
/// verified without starting them and are assumed to be available.
pub(crate) fn check_target(target: &LaunchTarget, app: &str) -> Result<(), String> {
    match target {
        LaunchTarget::Executable { path } => {
            if executable_exists(path) {
                Ok(())
            } else {
                Err(not_installed_message(app, path))
            }
        }
        LaunchTarget::Package { package } => match &package.path {
            Some(path) if !is_network_path(path) && !path.exists() => {
                Err(not_installed_message(app, path))
            }
            _ => Ok(()),
        },
        LaunchTarget::Open { target } => {
            let path = Path::new(target);
            if path.is_absolute() && !is_network_path(path) && !path.exists() {
                Err(format!(
                    "{target} no longer exists, so {app} cannot open it - edit the application"
                ))
            } else {
                Ok(())
            }
        }
    }
}

/// Check the working folder of a launch request.
pub(crate) fn check_working_directory(request: &LaunchRequest, app: &str) -> Result<(), String> {
    match &request.working_directory {
        Some(dir) if !is_network_path(dir) && !dir.is_dir() => Err(format!(
            "{app}'s working folder {} no longer exists - edit the application to choose another one",
            dir.display()
        )),
        _ => Ok(()),
    }
}

/// [`check_target`] and [`check_working_directory`].
pub(crate) fn check_request(request: &LaunchRequest, app: &str) -> Result<(), String> {
    check_target(&request.target, app)?;
    check_working_directory(request, app)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reframe_platform::{PackageInfo, PackageKind};

    #[test]
    fn executables_must_exist() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("tool");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        assert!(check_target(&LaunchTarget::Executable { path: exe.clone() }, "Tool").is_ok());

        let missing = dir.path().join("gone");
        let err = check_target(
            &LaunchTarget::Executable {
                path: missing.clone(),
            },
            "Tool",
        )
        .unwrap_err();
        assert_eq!(
            err,
            format!(
                "Tool isn't installed at {} - edit the application to choose its location",
                missing.display()
            )
        );
        assert!(!executable_exists(Path::new("")));
    }

    #[test]
    fn bare_names_are_resolved_on_path() {
        // `sh` exists on every Unix system; a random name does not.
        #[cfg(unix)]
        assert!(executable_exists(Path::new("sh")));
        assert!(!executable_exists(Path::new(
            "reframe-no-such-program-4f2a"
        )));
    }

    #[test]
    fn packages_and_open_targets() {
        let package = |path: Option<&str>| LaunchTarget::Package {
            package: PackageInfo {
                kind: PackageKind::AppImage,
                id: "app".into(),
                path: path.map(Into::into),
            },
        };
        assert!(check_target(&package(None), "App").is_ok());
        let missing = std::env::temp_dir().join("reframe-no-such-dir");
        let app_image = missing.join("App.AppImage").display().to_string();
        assert!(check_target(&package(Some(&app_image)), "App").is_err());
        let open = |t: &str| LaunchTarget::Open { target: t.into() };
        assert!(check_target(&open("https://example.com"), "Browser").is_ok());
        let pdf = missing.join("file.pdf").display().to_string();
        assert!(check_target(&open(&pdf), "Reader").is_err());
    }

    #[test]
    fn working_directory_must_exist() {
        let dir = tempfile::tempdir().unwrap();
        let ok = LaunchRequest::executable("sh").with_working_directory(Some(dir.path().into()));
        assert!(check_working_directory(&ok, "Shell").is_ok());
        let missing = dir.path().join("missing");
        let bad = LaunchRequest::executable("sh").with_working_directory(Some(missing.clone()));
        let err = check_working_directory(&bad, "Shell").unwrap_err();
        assert!(
            err.contains("working folder") && err.contains("missing"),
            "{err}"
        );
    }
}
