//! Launching applications: executables, packaged (MSIX/Store) apps and shell targets.
//!
//! Notes:
//! - Executables are spawned detached, in a new process group, with Reframe's standard handles
//!   (see [`spawn_executable`]): console programs get a working console window of their own
//!   because the release app has no console.
//! - Packaged apps are started with `IApplicationActivationManager::ActivateApplication`, which
//!   reports the app's process id. If activation fails and no arguments are needed, Reframe falls
//!   back to `explorer.exe shell:AppsFolder\<AUMID>` (no pid). Packaged apps run in an
//!   environment controlled by Windows, so `working_directory` and `env` do not apply to them.
//! - Files, folders and URLs are opened with the user's default handler through the shell
//!   (`ShellExecuteExW`, "open" verb, without error dialogs); the shell does not report which
//!   process handled the request, so no pid is returned, and `env` does not apply.

use std::mem::size_of;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use windows::Win32::System::Com::{CLSCTX_LOCAL_SERVER, CoCreateInstance};
use windows::Win32::UI::Shell::{
    AO_NOERRORUI, ApplicationActivationManager, IApplicationActivationManager, SEE_MASK_FLAG_NO_UI,
    SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::PCWSTR;

use super::ffi::{ComScope, pcwstr, wide};
use super::logic::launch::{
    apps_folder_target, join_command_line, shell_execute_error, validate_aumid,
};
use crate::{
    LaunchRequest, LaunchTarget, LaunchedProcess, PackageKind, PlatformError, PlatformResult,
};

pub fn launch(request: &LaunchRequest) -> PlatformResult<LaunchedProcess> {
    match &request.target {
        LaunchTarget::Executable { path } => {
            let pid = spawn_executable(
                path,
                &request.args,
                request.working_directory.as_deref(),
                &request.env,
            )?;
            Ok(LaunchedProcess { pid: Some(pid) })
        }
        LaunchTarget::Package { package } if package.kind == PackageKind::WindowsPackage => {
            activate_package(&package.id, &request.args)
        }
        LaunchTarget::Package { package } => Err(PlatformError::unsupported(
            format!("Launching {}", package.id),
            format!("{:?} packages do not exist on Windows", package.kind),
        )),
        LaunchTarget::Open { target } => {
            shell_open(target, &request.args, request.working_directory.as_deref())
        }
    }
}

/// `CREATE_NEW_PROCESS_GROUP`.
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// Start an executable detached from Reframe and return its process id.
///
/// Unlike [`crate::common::launch::spawn_detached`], the child inherits Reframe's standard handles
/// instead of getting the NUL device: a console program whose standard input is NUL (`cmd.exe`,
/// `powershell.exe`, ...) reads end-of-file at once and exits, although Windows gives it a console
/// window. The release app has no standard handles, so console programs use the console Windows
/// creates for them; development builds run in a console, which console children then share. The
/// child gets its own process group (Ctrl+C in a shared console does not reach it), and Reframe
/// keeps no handle to it, so it outlives Reframe.
fn spawn_executable(
    program: &Path,
    args: &[String],
    working_directory: Option<&Path>,
    env: &[(String, String)],
) -> PlatformResult<u32> {
    let failed =
        |message: String| PlatformError::launch_failed(program.display().to_string(), message);
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .creation_flags(CREATE_NEW_PROCESS_GROUP);
    if let Some(dir) = working_directory {
        if !dir.is_dir() {
            return Err(failed(format!(
                "working directory {} does not exist",
                dir.display()
            )));
        }
        command.current_dir(dir);
    }
    for (key, value) in env {
        command.env(key, value);
    }
    let child = command.spawn().map_err(|e| failed(e.to_string()))?;
    // Dropping `child` closes Reframe's process handle; it neither waits for nor ends the process.
    Ok(child.id())
}

fn activate_package(aumid: &str, args: &[String]) -> PlatformResult<LaunchedProcess> {
    let id = validate_aumid(aumid).map_err(|reason| PlatformError::launch_failed(aumid, reason))?;
    let arguments = join_command_line(args);
    match activate_application(id, &arguments) {
        Ok(pid) => Ok(LaunchedProcess {
            pid: (pid != 0).then_some(pid),
        }),
        Err(error) if args.is_empty() => {
            log::info!(
                "activating {id} failed ({}); starting it through the shell instead",
                error.message()
            );
            spawn_executable(&explorer_path(), &[apps_folder_target(id)], None, &[])?;
            Ok(LaunchedProcess { pid: None })
        }
        Err(error) => Err(PlatformError::launch_failed(
            id,
            format!(
                "Windows could not activate the app ({}), and the shell fallback cannot pass \
                 arguments",
                error.message()
            ),
        )),
    }
}

/// `IApplicationActivationManager::ActivateApplication`; returns the app's process id.
pub fn activate_application(aumid: &str, arguments: &str) -> windows::core::Result<u32> {
    let _com = ComScope::apartment();
    let invalid = || windows::core::Error::from_hresult(windows::Win32::Foundation::E_INVALIDARG);
    let id = wide(aumid).ok_or_else(invalid)?;
    let args = wide(arguments).ok_or_else(invalid)?;
    // SAFETY: COM is initialized for this scope; the manager is released before `_com`.
    unsafe {
        let manager: IApplicationActivationManager =
            CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER)?;
        manager.ActivateApplication(pcwstr(&id), pcwstr(&args), AO_NOERRORUI)
    }
}

/// `%SystemRoot%\explorer.exe` (an absolute path, so `PATH` cannot redirect it).
fn explorer_path() -> PathBuf {
    ["SystemRoot", "windir"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|root| PathBuf::from(root).join("explorer.exe"))
        .find(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("explorer.exe"))
}

fn shell_open(
    target: &str,
    args: &[String],
    working_directory: Option<&Path>,
) -> PlatformResult<LaunchedProcess> {
    let failed = |reason: String| PlatformError::launch_failed(target, reason);
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return Err(failed("there is nothing to open".into()));
    }
    if let Some(dir) = working_directory
        && !dir.is_dir()
    {
        return Err(failed(format!(
            "working directory {} does not exist",
            dir.display()
        )));
    }
    let invalid = || failed("it contains a NUL character".into());
    let file = wide(trimmed).ok_or_else(invalid)?;
    let parameters = if args.is_empty() {
        None
    } else {
        Some(wide(join_command_line(args)).ok_or_else(invalid)?)
    };
    let directory = working_directory
        .map(|dir| wide(dir).ok_or_else(invalid))
        .transpose()?;
    let verb = wide("open").ok_or_else(invalid)?;

    let _com = ComScope::apartment();
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        // No error dialogs (failures are reported to the caller); finish any DDE conversation
        // before returning, since the calling thread may not pump messages.
        fMask: SEE_MASK_FLAG_NO_UI | SEE_MASK_NOASYNC,
        lpVerb: pcwstr(&verb),
        lpFile: pcwstr(&file),
        lpParameters: parameters.as_deref().map_or(PCWSTR::null(), pcwstr),
        lpDirectory: directory.as_deref().map_or(PCWSTR::null(), pcwstr),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: every string outlives the call; `cbSize` matches the structure.
    match unsafe { ShellExecuteExW(&mut info) } {
        Ok(()) => Ok(LaunchedProcess { pid: None }),
        Err(error) => {
            let code = info.hInstApp.0 as isize;
            Err(failed(if (1..=32).contains(&code) {
                shell_execute_error(code)
            } else {
                error.message()
            }))
        }
    }
}
