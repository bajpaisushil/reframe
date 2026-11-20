//! Shells: recognising shell processes, telling interactive shells from scripts, and building
//! the command that runs a user-approved startup command and then keeps an interactive shell.
//!
//! Command lines are only *inspected* here (to tell `bash` from `bash -c ...`); nothing read from
//! another process's command line is ever stored.

use std::path::Path;

use reframe_platform::ProcessInfo;

use super::escape::{posix_quote, powershell_encoded_command};

/// Shell syntax families (they differ in how "run this, then stay interactive" is spelled).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellFamily {
    /// sh, bash, zsh, dash, ksh, mksh and relatives.
    Posix,
    Fish,
    Nu,
    /// csh, tcsh.
    Csh,
    Elvish,
    Xonsh,
    /// Windows PowerShell (`powershell`) and PowerShell 7 (`pwsh`).
    PowerShell,
    Cmd,
}

/// Final path component of a file name or path written with `/` or `\` separators.
pub(crate) fn base_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Normalised program name: final path component, lower-case, without the leading `-` of login
/// shells and without `.exe` (`C:\Windows\System32\CMD.EXE` → `cmd`, `-zsh` → `zsh`).
pub(crate) fn normalize_name(name: &str) -> String {
    let base = base_name(name.trim())
        .trim_start_matches('-')
        .to_lowercase();
    base.strip_suffix(".exe")
        .map(str::to_string)
        .unwrap_or(base)
}

/// The shell family of a program name, if it is a known shell.
pub fn family_of_name(name: &str) -> Option<ShellFamily> {
    use ShellFamily::*;
    Some(match normalize_name(name).as_str() {
        "sh" | "bash" | "rbash" | "zsh" | "dash" | "ash" | "ksh" | "ksh93" | "mksh" | "lksh"
        | "oksh" | "loksh" | "pdksh" | "yash" | "posh" => Posix,
        "fish" => Fish,
        "nu" => Nu,
        "csh" | "tcsh" => Csh,
        "elvish" => Elvish,
        "xonsh" => Xonsh,
        "pwsh" | "pwsh-preview" | "powershell" => PowerShell,
        "cmd" => Cmd,
        _ => return None,
    })
}

fn is_interpreter(program: &str) -> bool {
    normalize_name(program).starts_with("python")
}

/// The shell a process is running, with its normalised name (`bash`, `pwsh`, `cmd`...).
///
/// Checked in order: the process name, the executable's file name, and `argv[0]` (or `argv[1]`
/// when `argv[0]` is an interpreter, e.g. `python3 /usr/bin/xonsh`).
pub fn process_shell(process: &ProcessInfo) -> Option<(ShellFamily, String)> {
    let mut candidates: Vec<String> = vec![process.name.clone()];
    if let Some(exe) = process.executable_path.as_deref().and_then(Path::file_name) {
        candidates.push(exe.to_string_lossy().into_owned());
    }
    match process.command_line.as_slice() {
        [first, second, ..] if is_interpreter(first) => candidates.push(second.clone()),
        [first, ..] => candidates.push(first.clone()),
        [] => {}
    }
    candidates
        .iter()
        .find_map(|c| family_of_name(c).map(|f| (f, normalize_name(c))))
}

/// Arguments after the shell's own name in its command line.
fn shell_arguments(process: &ProcessInfo) -> &[String] {
    match process.command_line.as_slice() {
        [first, second, rest @ ..] if is_interpreter(first) && family_of_name(second).is_some() => {
            rest
        }
        [_, rest @ ..] => rest,
        [] => &[],
    }
}

/// Whether a shell process is an interactive session (as opposed to a script or a `-c` command
/// run by some program). Unknown command lines (not readable) count as interactive.
pub fn is_interactive(process: &ProcessInfo, family: ShellFamily) -> bool {
    if process.command_line.is_empty() {
        return true;
    }
    let args = shell_arguments(process);
    match family {
        ShellFamily::PowerShell => powershell_is_interactive(args),
        ShellFamily::Cmd => cmd_is_interactive(args),
        _ => unix_shell_is_interactive(args),
    }
}

/// sh/bash/zsh/fish/nu/csh/elvish/xonsh: `-i` forces interactive; `-c`/`--command` or a script
/// operand means non-interactive.
fn unix_shell_is_interactive(args: &[String]) -> bool {
    let mut command_mode = false;
    for arg in args {
        if arg == "--interactive" {
            return true;
        }
        if matches!(arg.as_str(), "--command" | "--commands")
            || arg.starts_with("--command=")
            || arg.starts_with("--commands=")
        {
            command_mode = true;
            continue;
        }
        if let Some(cluster) = arg.strip_prefix('-').filter(|c| !c.starts_with('-')) {
            if cluster.contains('i') {
                return true;
            }
            if cluster.contains('c') {
                command_mode = true;
            }
        }
    }
    if command_mode {
        return false;
    }
    // A first argument that is not an option is a script to run.
    match args.first() {
        Some(first) => first.starts_with('-') || first.starts_with('+'),
        None => true,
    }
}

/// `powershell`/`pwsh`: `-NoExit` keeps it interactive; `-Command`, `-File`, `-EncodedCommand`
/// or a script operand otherwise make it run and exit.
fn powershell_is_interactive(args: &[String]) -> bool {
    // Parameters that take a value (so the next argument is not a script operand).
    const WITH_VALUE: &[&str] = &[
        "executionpolicy",
        "ex",
        "ep",
        "settingsfile",
        "workingdirectory",
        "wd",
        "configurationname",
        "config",
        "configurationfile",
        "windowstyle",
        "w",
        "inputformat",
        "inp",
        "if",
        "outputformat",
        "o",
        "of",
        "version",
        "v",
        "psconsolefile",
        "custompipename",
    ];
    let mut no_exit = false;
    let mut runs_something = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let Some(name) = arg
            .strip_prefix("--")
            .or_else(|| arg.strip_prefix('-'))
            .or_else(|| arg.strip_prefix('/'))
        else {
            // Positional operand: a script file (pwsh) or command text (Windows PowerShell).
            runs_something = true;
            break;
        };
        let name = name.to_lowercase();
        if name.len() >= 3 && "noexit".starts_with(&name) {
            no_exit = true;
        } else if name == "cwa"
            || (!name.is_empty() && "command".starts_with(&name))
            || (name.len() >= 8 && "commandwithargs".starts_with(&name))
            || (!name.is_empty() && "file".starts_with(&name))
            || name == "e"
            || name == "ec"
            || (name.len() >= 2 && "encodedcommand".starts_with(&name))
        {
            // Everything after -Command / -File belongs to it.
            runs_something = true;
            break;
        } else if WITH_VALUE.contains(&name.as_str()) {
            iter.next();
        }
    }
    // -NoExit may also appear before -Command; anything after -Command was not inspected.
    no_exit || !runs_something
}

/// `cmd.exe`: the first of `/C`, `/R` (run and exit) or `/K` (run and stay) decides.
fn cmd_is_interactive(args: &[String]) -> bool {
    for arg in args {
        let lower = arg.to_lowercase();
        let switch = |s: &str| {
            lower
                .strip_prefix(s)
                .is_some_and(|rest| !rest.starts_with(|c: char| c.is_ascii_alphabetic()))
        };
        if switch("/k") {
            return true;
        }
        if switch("/c") || switch("/r") {
            return false;
        }
    }
    true
}

/// Arguments that start `shell` so that it runs the user-approved `command` and then stays an
/// interactive shell. `login` asks for a login shell (macOS terminals start login shells, which
/// is where Homebrew and similar tools set up `PATH`).
///
/// | Shell | Arguments |
/// |-------|-----------|
/// | POSIX | `shell [-l] -c 'trap <exec> INT⏎<command>⏎⏎<exec>'`, `<exec>` = `exec <shell> [-l]` |
/// | csh, elvish, xonsh | `shell -c '<command>⏎⏎exec <shell>'` |
/// | fish | `fish [-l] -C <command>` (runs it, then reads interactive input) |
/// | nu | `nu [-l] -e <command>` (runs it, then enters the REPL) |
/// | PowerShell | `pwsh [-Login] -NoExit -Command <command>` |
/// | cmd | `cmd.exe /K <command>` (see [`super::escape::cmd_startup`] for the exact delivery) |
/// | unknown | `/bin/sh -c 'trap <exec> INT⏎<command>⏎⏎<exec>'` |
///
/// The command is the user's own shell code, so it is placed verbatim; only the shell path that
/// follows `exec` is quoted. Two line breaks separate them so that a trailing comment, `&` or
/// line-continuation backslash in the command cannot swallow the `exec`. The POSIX `trap` makes
/// Ctrl+C hand over to the interactive shell: without it, interrupting a long-running command
/// (a dev server) would also end the non-interactive shell running it, and close the tab.
///
/// On Windows, PowerShell is started with [`powershell_startup_args`] instead.
pub fn startup_argv(shell: &Path, command: &str, login: bool) -> Vec<String> {
    let shell_str = shell.to_string_lossy().into_owned();
    let family = shell
        .file_name()
        .and_then(|n| family_of_name(&n.to_string_lossy()));
    let exec_shell = |login_flag: bool| {
        format!(
            "exec {}{}",
            posix_quote(&shell_str),
            if login_flag { " -l" } else { "" }
        )
    };
    let posix_script = |login_flag: bool| {
        let exec = exec_shell(login_flag);
        format!("trap {} INT\n{command}\n\n{exec}", posix_quote(&exec))
    };
    let mut argv = vec![shell_str.clone()];
    match family {
        Some(ShellFamily::Posix) => {
            if login {
                argv.push("-l".into());
            }
            argv.push("-c".into());
            argv.push(posix_script(login));
        }
        Some(ShellFamily::Csh | ShellFamily::Elvish | ShellFamily::Xonsh) => {
            argv.push("-c".into());
            argv.push(format!("{command}\n\n{}", exec_shell(false)));
        }
        Some(ShellFamily::Fish) => {
            if login {
                argv.push("-l".into());
            }
            argv.push("-C".into());
            argv.push(command.to_string());
        }
        Some(ShellFamily::Nu) => {
            if login {
                argv.push("-l".into());
            }
            argv.push("-e".into());
            argv.push(command.to_string());
        }
        Some(ShellFamily::PowerShell) => {
            if login {
                argv.push("-Login".into());
            }
            argv.extend(["-NoExit".into(), "-Command".into(), command.to_string()]);
        }
        Some(ShellFamily::Cmd) => {
            argv.extend(["/K".into(), command.to_string()]);
        }
        None => {
            argv = vec!["/bin/sh".into(), "-c".into(), posix_script(false)];
        }
    }
    argv
}

/// `<powershell> -NoExit -EncodedCommand <base64>`: how Windows launches run an approved command
/// in PowerShell. The command travels as Base64 (see
/// [`super::escape::powershell_encoded_command`]), so no layer between Reframe and PowerShell (the
/// Windows argument convention, Windows Terminal's re-joining of a tab's command line, a
/// terminal's own re-quoting) can change how PowerShell splits it.
pub fn powershell_startup_args(powershell: &Path, command: &str) -> Vec<String> {
    vec![
        powershell.to_string_lossy().into_owned(),
        "-NoExit".into(),
        "-EncodedCommand".into(),
        powershell_encoded_command(command),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(name: &str, cmd: &[&str]) -> ProcessInfo {
        ProcessInfo {
            pid: 1,
            parent_pid: None,
            name: name.into(),
            product_name: None,
            executable_path: None,
            command_line: cmd.iter().map(|s| s.to_string()).collect(),
            working_directory: None,
            start_time: None,
            package: None,
        }
    }

    #[test]
    fn recognises_shell_names() {
        for (name, family) in [
            ("bash", ShellFamily::Posix),
            ("-zsh", ShellFamily::Posix),
            ("/bin/dash", ShellFamily::Posix),
            ("ksh93", ShellFamily::Posix),
            ("mksh", ShellFamily::Posix),
            ("fish", ShellFamily::Fish),
            ("nu", ShellFamily::Nu),
            ("tcsh", ShellFamily::Csh),
            ("csh", ShellFamily::Csh),
            ("elvish", ShellFamily::Elvish),
            ("xonsh", ShellFamily::Xonsh),
            ("pwsh", ShellFamily::PowerShell),
            ("PowerShell.exe", ShellFamily::PowerShell),
            (r"C:\Windows\System32\CMD.EXE", ShellFamily::Cmd),
        ] {
            assert_eq!(family_of_name(name), Some(family), "{name}");
        }
        for name in [
            "python3",
            "vim",
            "sshd",
            "bash-language-server",
            "zshrc",
            "conhost.exe",
            "",
        ] {
            assert_eq!(family_of_name(name), None, "{name}");
        }
    }

    #[test]
    fn detects_shells_from_name_executable_or_argv() {
        assert_eq!(
            process_shell(&proc("bash", &[])),
            Some((ShellFamily::Posix, "bash".into()))
        );
        // Login shell on macOS: argv[0] is "-zsh".
        assert_eq!(
            process_shell(&proc("login-ish", &["-zsh"])),
            Some((ShellFamily::Posix, "zsh".into()))
        );
        // xonsh runs under Python.
        assert_eq!(
            process_shell(&proc("python3.12", &["/usr/bin/python3", "/usr/bin/xonsh"])),
            Some((ShellFamily::Xonsh, "xonsh".into()))
        );
        let mut p = proc("weird-name", &[]);
        p.executable_path = Some("/usr/bin/fish".into());
        assert_eq!(process_shell(&p), Some((ShellFamily::Fish, "fish".into())));
        assert_eq!(process_shell(&proc("vim", &["vim", "notes.txt"])), None);
    }

    #[test]
    fn tells_interactive_shells_from_commands_and_scripts() {
        let cases: &[(&str, &[&str], bool)] = &[
            ("bash", &["bash"], true),
            ("bash", &["-bash"], true),
            ("bash", &["/bin/bash", "--login"], true),
            ("bash", &["bash", "--rcfile", "/tmp/rc"], true),
            ("bash", &["bash", "-c", "make"], false),
            ("bash", &["bash", "-lc", "make"], false),
            ("bash", &["bash", "-ic", "x"], true),
            ("bash", &["bash", "build.sh"], false),
            ("sh", &["sh", "-c", "exec foo"], false),
            ("zsh", &["zsh", "-l"], true),
            ("fish", &["fish", "-C", "set x 1"], true),
            ("fish", &["fish", "-c", "ls"], false),
            ("fish", &["fish", "--command=ls"], false),
            ("nu", &["nu", "-e", "ls"], true),
            ("nu", &["nu", "--commands", "ls"], false),
            ("tcsh", &["tcsh", "-c", "ls"], false),
            ("xonsh", &["/usr/bin/python3", "/usr/bin/xonsh"], true),
            (
                "xonsh",
                &["/usr/bin/python3", "/usr/bin/xonsh", "-c", "ls"],
                false,
            ),
            ("pwsh", &["pwsh.exe"], true),
            ("pwsh", &["pwsh", "-NoLogo"], true),
            ("pwsh", &["pwsh", "-Command", "Get-Date"], false),
            ("pwsh", &["pwsh", "-c", "Get-Date"], false),
            ("pwsh", &["pwsh", "-NoExit", "-Command", "Get-Date"], true),
            ("pwsh", &["pwsh", "-noe", "-c", "x"], true),
            ("pwsh", &["pwsh", "-File", "x.ps1"], false),
            ("pwsh", &["pwsh", "x.ps1"], false),
            ("pwsh", &["pwsh", "-EncodedCommand", "ZABpAHIA"], false),
            (
                "powershell",
                &["powershell.exe", "-ExecutionPolicy", "Bypass"],
                true,
            ),
            ("cmd", &["cmd.exe"], true),
            ("cmd", &["cmd.exe", "/c", "dir"], false),
            ("cmd", &["cmd.exe", "/D", "/C", "dir"], false),
            ("cmd", &["cmd.exe", "/k", "vcvars64.bat"], true),
            ("cmd", &["cmd.exe", "/c", "start", "cmd", "/k"], false),
            // Unreadable command line: assume interactive.
            ("bash", &[], true),
        ];
        for (name, cmd, expected) in cases {
            let p = proc(name, cmd);
            let (family, _) = process_shell(&p).unwrap();
            assert_eq!(is_interactive(&p, family), *expected, "{name} {cmd:?}");
        }
    }

    #[test]
    fn startup_argv_per_shell_family() {
        assert_eq!(
            startup_argv(Path::new("/bin/bash"), "npm start", false),
            vec![
                "/bin/bash",
                "-c",
                "trap 'exec /bin/bash' INT\nnpm start\n\nexec /bin/bash"
            ]
        );
        assert_eq!(
            startup_argv(Path::new("/bin/zsh"), "npm start", true),
            vec![
                "/bin/zsh",
                "-l",
                "-c",
                "trap 'exec /bin/zsh -l' INT\nnpm start\n\nexec /bin/zsh -l"
            ]
        );
        assert_eq!(
            startup_argv(Path::new("/usr/bin/fish"), "make", false),
            vec!["/usr/bin/fish", "-C", "make"]
        );
        assert_eq!(
            startup_argv(Path::new("/usr/bin/nu"), "ls", true),
            vec!["/usr/bin/nu", "-l", "-e", "ls"]
        );
        assert_eq!(
            startup_argv(Path::new("/bin/tcsh"), "ls", true),
            vec!["/bin/tcsh", "-c", "ls\n\nexec /bin/tcsh"]
        );
        assert_eq!(
            startup_argv(Path::new("/usr/bin/pwsh"), "Get-Date", false),
            vec!["/usr/bin/pwsh", "-NoExit", "-Command", "Get-Date"]
        );
        // Unknown $SHELL (e.g. tmux): run the command with /bin/sh, then exec the program.
        assert_eq!(
            startup_argv(Path::new("/usr/bin/tmux"), "ls", false),
            vec![
                "/bin/sh",
                "-c",
                "trap 'exec /usr/bin/tmux' INT\nls\n\nexec /usr/bin/tmux"
            ]
        );
        // Shell paths that need quoting are quoted after exec, and again inside the trap.
        let script = &startup_argv(Path::new("/opt/my shells/bash"), "x", false)[2];
        let (trap, rest) = script.split_once('\n').unwrap();
        assert_eq!(rest, "x\n\nexec '/opt/my shells/bash'");
        assert_eq!(
            crate::adapters::terminal::escape::tests::posix_words(trap).unwrap(),
            ["trap", "exec '/opt/my shells/bash'", "INT"]
        );
    }

    #[test]
    fn powershell_startup_args_encode_the_command() {
        let args = powershell_startup_args(Path::new(r"C:\pwsh\pwsh.exe"), "echo \"a b\"; dir");
        assert_eq!(
            args[..3],
            [r"C:\pwsh\pwsh.exe", "-NoExit", "-EncodedCommand"]
        );
        assert_eq!(args[3], powershell_encoded_command("echo \"a b\"; dir"));
        assert_eq!(args.len(), 4);
    }

    /// Installed POSIX shells to run the template with.
    #[cfg(unix)]
    fn posix_shells() -> Vec<&'static Path> {
        [
            "/bin/sh",
            "/bin/bash",
            "/bin/dash",
            "/usr/bin/zsh",
            "/bin/zsh",
        ]
        .into_iter()
        .map(Path::new)
        .filter(|p| p.exists())
        .collect()
    }

    #[cfg(unix)]
    fn run_template(shell: &Path, command: &str) -> std::process::Output {
        let argv = startup_argv(shell, command, false);
        std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    }

    /// The POSIX template really runs the command and then hands over to the shell (which
    /// exits at once here because its standard input is empty).
    #[cfg(unix)]
    #[test]
    fn posix_startup_template_runs_command_then_execs_shell() {
        for shell in posix_shells() {
            for command in [
                "printf 'ran:%s' \"$0\"",
                "printf ran # trailing comment",
                "printf ran \\",
                "false",
            ] {
                let out = run_template(shell, command);
                assert!(out.status.success(), "{shell:?} {command}: {out:?}");
                if command != "false" {
                    assert!(
                        String::from_utf8_lossy(&out.stdout).starts_with("ran"),
                        "{shell:?} {command}"
                    );
                }
            }
        }
    }

    /// Ctrl+C (SIGINT) while the command runs hands over to the interactive shell instead of
    /// ending the shell that runs the command (which would close the tab). Here the command
    /// interrupts its own shell; the rest of it must not run, and the exec'd shell exits cleanly.
    #[cfg(unix)]
    #[test]
    fn posix_startup_template_survives_ctrl_c() {
        for shell in posix_shells() {
            let out = run_template(shell, "kill -INT $$; echo NOT-REACHED");
            assert!(out.status.success(), "{shell:?}: {out:?}");
            assert!(
                !String::from_utf8_lossy(&out.stdout).contains("NOT-REACHED"),
                "{shell:?}: {out:?}"
            );
        }
    }
}
