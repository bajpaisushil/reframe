//! Spawning applications detached from Reframe.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::{PlatformError, PlatformResult};

/// Spawn `program` so that it outlives Reframe and does not inherit its stdio.
///
/// On Unix the child gets its own process group, and a background thread reaps it when it
/// exits so it never lingers as a zombie. Returns the child's pid.
pub fn spawn_detached(
    program: &Path,
    args: &[String],
    working_directory: Option<&Path>,
    env: &[(String, String)],
) -> PlatformResult<u32> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(dir) = working_directory {
        if !dir.is_dir() {
            return Err(PlatformError::launch_failed(
                program.display().to_string(),
                format!("working directory {} does not exist", dir.display()),
            ));
        }
        command.current_dir(dir);
    }
    for (key, value) in env {
        command.env(key, value);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }

    let mut child = command
        .spawn()
        .map_err(|e| PlatformError::launch_failed(program.display().to_string(), e.to_string()))?;
    let pid = child.id();
    // Reap the child when it exits (prevents zombies on Unix; harmless elsewhere).
    let _ = std::thread::Builder::new()
        .name(format!("reap-{pid}"))
        .spawn(move || {
            let _ = child.wait();
        });
    Ok(pid)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn spawns_and_reports_pid() {
        let pid = spawn_detached(
            Path::new("/bin/sh"),
            &["-c".into(), "exit 0".into()],
            None,
            &[],
        )
        .unwrap();
        assert!(pid > 0);
    }

    #[test]
    fn missing_program_is_a_launch_error() {
        let err = spawn_detached(Path::new("/definitely/not/here"), &[], None, &[]).unwrap_err();
        assert!(matches!(err, PlatformError::LaunchFailed { .. }));
    }

    #[test]
    fn missing_working_directory_is_reported() {
        let err = spawn_detached(
            Path::new("/bin/sh"),
            &[],
            Some(Path::new("/nope/nope")),
            &[],
        )
        .unwrap_err();
        assert!(err.to_string().contains("working directory"));
    }
}
