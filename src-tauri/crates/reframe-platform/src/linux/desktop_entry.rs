//! A small, forgiving parser for freedesktop `.desktop` files (Desktop Entry Specification).
//!
//! Only the `[Desktop Entry]` group is read; localized keys (`Name[de]=...`) and other groups
//! (`[Desktop Action ...]`) are ignored. `Exec` values are split into arguments following the
//! specification's quoting rules, and field codes (`%f`, `%U`, ...) are removed.

use std::path::{Path, PathBuf};

/// The parts of a desktop entry Reframe uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DesktopEntry {
    /// Desktop file id, e.g. `org.gnome.Calculator.desktop` (sub directories become `-`).
    pub id: String,
    pub path: PathBuf,
    pub name: Option<String>,
    /// `Exec` split into arguments with field codes removed. `None` when absent or empty.
    pub exec: Option<Vec<String>>,
    pub try_exec: Option<String>,
    pub icon: Option<String>,
    pub startup_wm_class: Option<String>,
    pub no_display: bool,
    pub hidden: bool,
    /// `Type=Application`.
    pub is_application: bool,
    /// `X-Flatpak` (the Flatpak application id).
    pub flatpak_id: Option<String>,
    /// `X-SnapInstanceName`.
    pub snap_instance: Option<String>,
}

/// Undo the string escapes of the specification (`\s`, `\n`, `\t`, `\r`, `\\`). Unknown
/// sequences are kept verbatim so the `Exec` quoting rules can interpret them.
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn parse_bool(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("true") || value.trim() == "1"
}

/// Parse the content of a `.desktop` file. Returns `None` when there is no
/// `[Desktop Entry]` group. `id` and `path` are left empty for the caller to fill in.
pub(crate) fn parse(content: &str) -> Option<DesktopEntry> {
    let mut entry = DesktopEntry::default();
    let mut in_main_group = false;
    let mut seen_main_group = false;
    let mut seen_keys: Vec<String> = Vec::new();
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_main_group = line == "[Desktop Entry]";
            if in_main_group {
                if seen_main_group {
                    // A duplicate group is invalid; keep the first one.
                    in_main_group = false;
                }
                seen_main_group = true;
            }
            continue;
        }
        if !in_main_group {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        // Localized keys and keys with invalid characters are ignored.
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            continue;
        }
        // The first occurrence of a key wins.
        if seen_keys.iter().any(|k| k == key) {
            continue;
        }
        seen_keys.push(key.to_string());
        let value = value.trim();
        let text = || Some(unescape(value)).filter(|v| !v.trim().is_empty());
        match key {
            "Type" => entry.is_application = value == "Application",
            "Name" => entry.name = text().map(|v| v.trim().to_string()),
            "Exec" => {
                entry.exec = text()
                    .and_then(|v| split_exec(&v))
                    .filter(|args| !args.is_empty())
            }
            "TryExec" => entry.try_exec = text(),
            "Icon" => entry.icon = text().map(|v| v.trim().to_string()),
            "StartupWMClass" => entry.startup_wm_class = text().map(|v| v.trim().to_string()),
            "NoDisplay" => entry.no_display = parse_bool(value),
            "Hidden" => entry.hidden = parse_bool(value),
            "X-Flatpak" => entry.flatpak_id = text().map(|v| v.trim().to_string()),
            "X-SnapInstanceName" => entry.snap_instance = text().map(|v| v.trim().to_string()),
            _ => {}
        }
    }
    seen_main_group.then_some(entry)
}

/// Remove field codes from one argument; `%%` becomes `%`. Returns `None` when the argument
/// consisted only of field codes.
fn strip_field_codes(arg: &str) -> Option<String> {
    if !arg.contains('%') {
        return Some(arg.to_string());
    }
    let mut out = String::with_capacity(arg.len());
    let mut chars = arg.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            // Field codes (%f %F %u %U %d %D %n %N %i %c %k %v %m) expand to files, URLs, the
            // icon or the name, which a launch target never carries; other letters are
            // invalid codes. Both are dropped.
            Some(code) if code.is_ascii_alphabetic() => {}
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Split an (already unescaped) `Exec` value into arguments: whitespace separates arguments,
/// double quotes group them, and inside quotes `\"`, `` \` ``, `\$` and `\\` are escapes.
/// Field codes are removed. Returns `None` for an unterminated quote.
pub(crate) fn split_exec(exec: &str) -> Option<Vec<String>> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    let mut in_quotes = false;
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        if in_quotes {
            match c {
                '"' => in_quotes = false,
                '\\' => match chars.next() {
                    Some(n @ ('"' | '`' | '$' | '\\')) => current.push(n),
                    Some(n) => {
                        current.push('\\');
                        current.push(n);
                    }
                    None => current.push('\\'),
                },
                _ => current.push(c),
            }
            continue;
        }
        match c {
            ' ' | '\t' | '\n' | '\r' => {
                if has_token {
                    args.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            '"' => {
                in_quotes = true;
                has_token = true;
            }
            '\\' => {
                // Not allowed by the spec outside quotes, but treat it like a shell would.
                if let Some(n) = chars.next() {
                    current.push(n);
                    has_token = true;
                }
            }
            _ => {
                current.push(c);
                has_token = true;
            }
        }
    }
    if in_quotes {
        return None;
    }
    if has_token {
        args.push(current);
    }
    Some(
        args.into_iter()
            .filter_map(|a| {
                if a.is_empty() {
                    // An explicitly quoted empty argument ("") is kept.
                    Some(a)
                } else {
                    strip_field_codes(&a)
                }
            })
            .collect(),
    )
}

/// What an `Exec` line launches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExecCommand {
    /// A program (absolute path or a name looked up on `PATH`) and its fixed arguments.
    Program { program: String, args: Vec<String> },
    /// `flatpak run [options] <app id> [args]`.
    Flatpak {
        app_id: String,
        command: Option<String>,
        args: Vec<String>,
    },
    /// `/snap/bin/<name>` (where `<name>` may be `snap.app`).
    Snap { name: String, args: Vec<String> },
}

fn file_name(program: &str) -> &str {
    Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(program)
}

fn is_env_assignment(arg: &str) -> bool {
    match arg.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && !name.starts_with(|c: char| c.is_ascii_digit())
                && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        }
        None => false,
    }
}

/// Shells whose `-c` command lines are opaque to Reframe.
const SHELLS: &[&str] = &["sh", "bash", "dash", "zsh", "ksh", "fish", "csh", "tcsh"];

/// Interpret split `Exec` arguments: skips an `env VAR=value ...` prefix, recognises Flatpak
/// and Snap launchers. Returns `None` for shell command lines (`sh -c '...'`) and empty input.
pub(crate) fn classify_exec(args: &[String]) -> Option<ExecCommand> {
    let mut rest = args;
    if rest.first().is_some_and(|p| file_name(p) == "env") {
        rest = &rest[1..];
        loop {
            match rest.first().map(String::as_str) {
                Some("-u" | "--unset") => rest = rest.get(2..).unwrap_or(&[]),
                Some(a) if a.starts_with('-') || is_env_assignment(a) => rest = &rest[1..],
                _ => break,
            }
        }
    }
    let (program, args) = rest.split_first()?;
    let base = file_name(program);

    if base == "flatpak" && args.first().map(String::as_str) == Some("run") {
        let mut command = None;
        let mut iter = args[1..].iter();
        while let Some(arg) = iter.next() {
            if let Some(cmd) = arg.strip_prefix("--command=") {
                command = Some(cmd.to_string());
            } else if arg == "--command" {
                command = iter.next().cloned();
            } else if arg.starts_with('-') {
                continue;
            } else {
                let app_args = iter
                    .filter(|a| !a.starts_with("@@"))
                    .cloned()
                    .collect::<Vec<_>>();
                return Some(ExecCommand::Flatpak {
                    app_id: arg.clone(),
                    command: command.filter(|c| !c.is_empty()),
                    args: app_args,
                });
            }
        }
        return None;
    }
    if let Some(name) = program.strip_prefix("/snap/bin/")
        && !name.is_empty()
        && !name.contains('/')
    {
        return Some(ExecCommand::Snap {
            name: name.to_string(),
            args: args.to_vec(),
        });
    }
    if SHELLS.contains(&base) && args.iter().any(|a| a == "-c") {
        return None;
    }
    Some(ExecCommand::Program {
        program: program.clone(),
        args: args.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_main_group_only() {
        let content = "\
# comment
[Desktop Entry]
Type=Application
Name=Text Editor
Name[de]=Texteditor
GenericName=Editor
Exec=gedit %U
TryExec = gedit
Icon=org.gnome.gedit
StartupWMClass=Gedit
NoDisplay=false
Name=Ignored duplicate

[Desktop Action new-window]
Name=New Window
Exec=gedit --new-window
";
        let e = parse(content).unwrap();
        assert!(e.is_application);
        assert_eq!(e.name.as_deref(), Some("Text Editor"));
        assert_eq!(e.exec, Some(s(&["gedit"])));
        assert_eq!(e.try_exec.as_deref(), Some("gedit"));
        assert_eq!(e.icon.as_deref(), Some("org.gnome.gedit"));
        assert_eq!(e.startup_wm_class.as_deref(), Some("Gedit"));
        assert!(!e.no_display && !e.hidden);
    }

    #[test]
    fn missing_group_and_flags() {
        assert!(parse("Name=Orphan\nExec=orphan").is_none());
        let e = parse("[Desktop Entry]\nType=Link\nHidden=true\nNoDisplay=true\nExec=\n").unwrap();
        assert!(!e.is_application);
        assert!(e.hidden && e.no_display);
        assert_eq!(e.exec, None);
        let e = parse(
            "[Desktop Entry]\nX-Flatpak=org.gimp.GIMP\nX-SnapInstanceName=firefox\nName=A\\sB",
        )
        .unwrap();
        assert_eq!(e.flatpak_id.as_deref(), Some("org.gimp.GIMP"));
        assert_eq!(e.snap_instance.as_deref(), Some("firefox"));
        assert_eq!(e.name.as_deref(), Some("A B"));
    }

    #[test]
    fn exec_field_codes_are_stripped() {
        assert_eq!(
            split_exec("code --new-window %F"),
            Some(s(&["code", "--new-window"]))
        );
        assert_eq!(
            split_exec("app %u %U %f %d %D %n %N %i %c %k %v %m"),
            Some(s(&["app"]))
        );
        assert_eq!(split_exec("app --file=%f"), Some(s(&["app", "--file="])));
        assert_eq!(split_exec("app 100%%"), Some(s(&["app", "100%"])));
        assert_eq!(split_exec("app %z"), Some(s(&["app"])));
    }

    #[test]
    fn exec_quoting() {
        assert_eq!(
            split_exec(r#""/opt/My App/bin/app" --title "Hello World" %U"#),
            Some(s(&["/opt/My App/bin/app", "--title", "Hello World"]))
        );
        assert_eq!(
            split_exec(r#"sh -c "echo \"hi\" \$HOME \\ \`x\`""#),
            Some(s(&["sh", "-c", r#"echo "hi" $HOME \ `x`"#]))
        );
        assert_eq!(split_exec(r#"app """#), Some(s(&["app", ""])));
        assert_eq!(split_exec("  app   a  "), Some(s(&["app", "a"])));
        assert_eq!(split_exec(r#"app "unterminated"#), None);
        assert_eq!(split_exec(""), Some(vec![]));
    }

    #[test]
    fn string_escapes_then_exec_quoting() {
        // In the file a literal backslash inside a quoted argument is written as four.
        let e = parse("[Desktop Entry]\nExec=app \"a\\\\\\\\b\" c\\sd\n").unwrap();
        assert_eq!(e.exec, Some(s(&["app", r"a\b", "c", "d"])));
    }

    #[test]
    fn classify_programs_and_wrappers() {
        assert_eq!(
            classify_exec(&s(&["/usr/bin/gedit", "--new"])),
            Some(ExecCommand::Program {
                program: "/usr/bin/gedit".into(),
                args: s(&["--new"])
            })
        );
        assert_eq!(
            classify_exec(&s(&[
                "env",
                "GDK_BACKEND=x11",
                "-u",
                "FOO",
                "-i",
                "code",
                "--x"
            ])),
            Some(ExecCommand::Program {
                program: "code".into(),
                args: s(&["--x"])
            })
        );
        assert_eq!(
            classify_exec(&s(&[
                "/usr/bin/flatpak",
                "run",
                "--branch=stable",
                "--arch=x86_64",
                "--command=code",
                "--file-forwarding",
                "com.visualstudio.code",
                "--new-window",
                "@@",
                "@@"
            ])),
            Some(ExecCommand::Flatpak {
                app_id: "com.visualstudio.code".into(),
                command: Some("code".into()),
                args: s(&["--new-window"])
            })
        );
        assert_eq!(
            classify_exec(&s(&[
                "flatpak",
                "run",
                "--command",
                "gimp-2.10",
                "org.gimp.GIMP"
            ])),
            Some(ExecCommand::Flatpak {
                app_id: "org.gimp.GIMP".into(),
                command: Some("gimp-2.10".into()),
                args: vec![]
            })
        );
        assert_eq!(
            classify_exec(&s(&["flatpak", "run", "--branch=stable"])),
            None
        );
        assert_eq!(
            classify_exec(&s(&[
                "env",
                "BAMF_DESKTOP_FILE_HINT=/var/lib/snapd/desktop/applications/firefox_firefox.desktop",
                "/snap/bin/firefox",
                "--private"
            ])),
            Some(ExecCommand::Snap {
                name: "firefox".into(),
                args: s(&["--private"])
            })
        );
        assert_eq!(classify_exec(&s(&["sh", "-c", "foo | bar"])), None);
        assert_eq!(classify_exec(&s(&["/bin/bash", "-c", "x"])), None);
        assert_eq!(
            classify_exec(&s(&["bash", "/opt/script.sh"])),
            Some(ExecCommand::Program {
                program: "bash".into(),
                args: s(&["/opt/script.sh"])
            })
        );
        assert_eq!(classify_exec(&[]), None);
        assert_eq!(classify_exec(&s(&["env", "A=1"])), None);
    }
}
