//! Launch helpers: command-line quoting, AppUserModelID validation and `ShellExecuteW` errors.

/// `APPLICATION_USER_MODEL_ID_MAX_LENGTH` (including the terminator).
pub const MAX_AUMID_LENGTH: usize = 130;

/// Quote one argument so `CommandLineToArgvW` and the MSVC C runtime parse it back unchanged.
pub fn quote_argument(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\u{b}', '"']) {
        return arg.to_string();
    }
    let mut quoted = String::with_capacity(arg.len() + 2);
    quoted.push('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                // Backslashes before a quote are doubled, and the quote itself is escaped.
                quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.extend(std::iter::repeat_n('\\', backslashes));
                quoted.push(c);
                backslashes = 0;
            }
        }
    }
    // Backslashes before the closing quote are doubled so they do not escape it.
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

/// Join arguments into a single Windows command line (without the program name).
pub fn join_command_line(args: &[String]) -> String {
    args.iter()
        .map(|a| quote_argument(a))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Validate an AppUserModelID before handing it to the shell. Returns the trimmed id.
pub fn validate_aumid(aumid: &str) -> Result<&str, String> {
    let id = aumid.trim();
    if id.is_empty() {
        return Err("the application id (AppUserModelID) is empty".into());
    }
    if id.encode_utf16().count() >= MAX_AUMID_LENGTH {
        return Err(format!(
            "the application id is longer than {} characters",
            MAX_AUMID_LENGTH - 1
        ));
    }
    if id.chars().any(|c| c.is_control() || c == '"') {
        return Err("the application id contains invalid characters".into());
    }
    Ok(id)
}

/// Shell target that starts a registered application by AppUserModelID.
pub fn apps_folder_target(aumid: &str) -> String {
    format!("shell:AppsFolder\\{aumid}")
}

/// Human-readable reason for a `ShellExecuteW` return value of 32 or less.
pub fn shell_execute_error(code: isize) -> String {
    match code {
        0 | 8 => "the system is out of memory or resources".into(),
        2 => "the file was not found".into(),
        3 => "the path was not found".into(),
        5 => "access was denied".into(),
        11 => "the file is not a valid application".into(),
        26 => "a sharing violation occurred".into(),
        27 => "the file association is incomplete or invalid".into(),
        28..=30 => "the application that handles it did not respond".into(),
        31 => "no application is associated with this kind of file".into(),
        32 => "a required library was not found".into(),
        other => format!("ShellExecute failed with code {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference parser implementing the `CommandLineToArgvW` rules for arguments.
    fn parse(command_line: &str) -> Vec<String> {
        let mut args = Vec::new();
        let mut chars = command_line.chars().peekable();
        loop {
            while chars.peek().is_some_and(|c| *c == ' ' || *c == '\t') {
                chars.next();
            }
            if chars.peek().is_none() {
                return args;
            }
            let mut arg = String::new();
            let mut in_quotes = false;
            let mut backslashes = 0usize;
            while let Some(&c) = chars.peek() {
                match c {
                    '\\' => {
                        backslashes += 1;
                        chars.next();
                    }
                    '"' => {
                        arg.extend(std::iter::repeat_n('\\', backslashes / 2));
                        if backslashes % 2 == 1 {
                            arg.push('"');
                        } else {
                            in_quotes = !in_quotes;
                        }
                        backslashes = 0;
                        chars.next();
                    }
                    ' ' | '\t' if !in_quotes => break,
                    _ => {
                        arg.extend(std::iter::repeat_n('\\', backslashes));
                        backslashes = 0;
                        arg.push(c);
                        chars.next();
                    }
                }
            }
            arg.extend(std::iter::repeat_n('\\', backslashes));
            args.push(arg);
        }
    }

    #[test]
    fn simple_arguments_are_not_quoted() {
        assert_eq!(quote_argument("--new-window"), "--new-window");
        assert_eq!(quote_argument(r"C:\path\file.txt"), r"C:\path\file.txt");
        assert_eq!(quote_argument(r"C:\dir\"), r"C:\dir\");
    }

    #[test]
    fn special_arguments_are_quoted() {
        assert_eq!(quote_argument(""), "\"\"");
        assert_eq!(quote_argument("a b"), "\"a b\"");
        assert_eq!(quote_argument(r"C:\My Dir\"), r#""C:\My Dir\\""#);
        assert_eq!(quote_argument(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote_argument(r#"a\"b"#), r#""a\\\"b""#);
    }

    #[test]
    fn quoting_round_trips_through_the_argv_rules() {
        let cases: Vec<Vec<String>> = vec![
            vec!["plain".into(), "two words".into(), "".into()],
            vec![r"C:\Program Files\App\".into(), r#"quote"inside"#.into()],
            vec![r#"\\server\share\"#.into(), r#"\"#.into(), r#"\\""#.into()],
            vec!["tab\there".into(), "new\nline".into(), "ünïcødé ✓".into()],
        ];
        for args in cases {
            assert_eq!(parse(&join_command_line(&args)), args, "{args:?}");
        }
        assert_eq!(join_command_line(&[]), "");
    }

    #[test]
    fn aumid_validation() {
        assert_eq!(
            validate_aumid(" Microsoft.WindowsCalculator_8wekyb3d8bbwe!App "),
            Ok("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App")
        );
        assert!(validate_aumid("").is_err());
        assert!(validate_aumid("   ").is_err());
        assert!(validate_aumid("bad\"id").is_err());
        assert!(validate_aumid("bad\nid").is_err());
        assert!(validate_aumid(&"x".repeat(MAX_AUMID_LENGTH - 1)).is_ok());
        assert!(validate_aumid(&"x".repeat(MAX_AUMID_LENGTH)).is_err());
        assert_eq!(
            apps_folder_target("Microsoft.WindowsTerminal_8wekyb3d8bbwe!App"),
            r"shell:AppsFolder\Microsoft.WindowsTerminal_8wekyb3d8bbwe!App"
        );
    }

    #[test]
    fn shell_execute_errors_are_explained() {
        assert_eq!(shell_execute_error(2), "the file was not found");
        assert_eq!(
            shell_execute_error(31),
            "no application is associated with this kind of file"
        );
        assert!(shell_execute_error(29).contains("did not respond"));
        assert_eq!(shell_execute_error(17), "ShellExecute failed with code 17");
    }
}
