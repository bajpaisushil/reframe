//! Quoting for every parser a terminal launch passes through.
//!
//! Restoring a terminal crosses several command-line parsers: POSIX shells and fish (`sh -c`
//! scripts, Terminology's `-e` string), GLib's `g_shell_parse_argv` (the `--command=` option of
//! Xfce Terminal, MATE Terminal, LXTerminal and Tilix), a shell Reframe cannot identify (text that
//! macOS Terminal and iTerm2 type into the user's login shell), Windows Terminal's `;` sub-command
//! splitter, the Windows argument convention (`CommandLineToArgvW` / the Microsoft C runtime),
//! `cmd.exe`'s own handling of `/K`, PowerShell, and AppleScript string literals.
//!
//! Every value that crosses one of those parsers goes through exactly one function here, and each
//! function is tested against a reference implementation of the parser it targets (and, where the
//! program is installed, the real shell), with hostile input (quotes, `$`, backticks, `;`, `&`,
//! `|`, newlines, Unicode, spaces, backslashes, a trailing backslash).

use base64::Engine;

/// Characters that are never special to POSIX shells, fish or GLib's shell parser. Notably
/// absent: `=` (zsh `=cmd` expansion), `%` (old fish job expansion), `~`, `#`, globs, quotes.
fn is_posix_safe(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':' | ',' | '+' | '@')
}

/// Quote `s` as a single word for a POSIX shell (sh, bash, zsh, dash, ksh), fish, or GLib's
/// `g_shell_parse_argv`.
///
/// Words made only of safe characters are returned unchanged. Anything else is wrapped in single
/// quotes, inside which POSIX shells and GLib treat every character literally (even newlines).
/// fish also reads `\\` and `\'` as escapes inside single quotes, so neither a quote nor a
/// backslash is ever left inside them: each `'` becomes `'\''` and each `\` becomes `'\\'` (close
/// the quotes, add the escaped character, reopen), which all of these parsers read the same way.
pub fn posix_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(is_posix_safe) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\'' => out.push_str("'\\''"),
            '\\' => out.push_str("'\\\\'"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// Join `argv` into one command line in which every element stays one word for the parsers
/// [`posix_quote`] supports.
pub fn posix_join<S: AsRef<str>>(argv: &[S]) -> String {
    argv.iter()
        .map(|a| posix_quote(a.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether `c` is read literally inside a single-quoted word by every common interactive shell:
/// sh, bash, zsh, ksh, fish, tcsh, nu, elvish, xonsh and PowerShell. Excluded: the quote itself
/// and the typographic single quotes PowerShell treats as quotes (they end the word), backslash
/// (an escape in fish and xonsh), `!` (history expansion in tcsh, even inside quotes), and control
/// characters and line separators (they end or edit the line being typed).
fn is_inert_in_any_shell(c: char) -> bool {
    !(matches!(
        c,
        '\'' | '\\' | '!' | '\u{2018}'..='\u{201B}' | '\u{2028}' | '\u{2029}'
    ) || c.is_control())
}

/// `s` as a single-quoted word that any common interactive shell reads back unchanged, for text
/// typed into a shell Reframe cannot identify (macOS Terminal's `do script`, iTerm2's
/// `write text`: the user may have chosen zsh, fish, tcsh, nu, PowerShell...).
///
/// No escaping scheme works in all of those shells, so this does not escape anything: it returns
/// `None` when `s` contains a character that one of them treats specially even inside single
/// quotes (see [`is_inert_in_any_shell`]), and the caller must then not type `s` at all.
pub fn quote_for_any_shell(s: &str) -> Option<String> {
    s.chars()
        .all(is_inert_in_any_shell)
        .then(|| format!("'{s}'"))
}

/// Escape one argument for Windows Terminal's sub-command splitter.
///
/// `wt.exe` splits every argument at each `;` that is not preceded by a backslash (the regex
/// `^;|[^\\];`) and then replaces each `\;` with `;` (`Commandline::AddArg`). Escaping every `;`
/// as `\;` therefore survives both steps unchanged, including a backslash that precedes a `;` in
/// the original (`a\;b` → `a\\;b` → `a\;b`).
pub fn wt_escape(s: &str) -> String {
    s.replace(';', "\\;")
}

/// Quote one argument exactly as Rust's `std::process::Command` does on Windows (the encoding the
/// platform layer uses for every launch); `CommandLineToArgvW` and the Microsoft C runtime parse
/// the result back into the original argument.
///
/// Only empty arguments and arguments containing a space or a tab are wrapped in quotes. A run of
/// `n` backslashes before a quote becomes `2n + 1` backslashes plus the quote, and a run before the
/// closing quote becomes `2n` backslashes; other backslashes are literal.
pub fn windows_quote_arg(arg: &str) -> String {
    let quote = arg.is_empty() || arg.contains([' ', '\t']);
    let mut out = String::with_capacity(arg.len() + 2);
    if quote {
        out.push('"');
    }
    let mut backslashes = 0usize;
    for c in arg.chars() {
        if c == '\\' {
            backslashes += 1;
        } else {
            if c == '"' {
                push_backslashes(&mut out, backslashes + 1);
            }
            backslashes = 0;
        }
        out.push(c);
    }
    if quote {
        push_backslashes(&mut out, backslashes);
        out.push('"');
    }
    out
}

fn push_backslashes(out: &mut String, n: usize) {
    out.extend(std::iter::repeat_n('\\', n));
}

/// Environment variable that carries a Command Prompt startup command when it cannot be passed
/// on the command line verbatim (see [`cmd_startup`]).
pub const CMD_STARTUP_VARIABLE: &str = "REFRAME_STARTUP_COMMAND";

/// How a startup command reaches `cmd.exe /K` when Reframe starts `cmd.exe` itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CmdStartup {
    /// Pass `/K` followed by this argument: the platform's argument encoding
    /// ([`windows_quote_arg`]) at most wraps it in quotes, and `cmd.exe` strips exactly those
    /// quotes again, so it receives the text verbatim (`%VARIABLES%` expand as when typed).
    Argument(String),
    /// Pass `/K %REFRAME_STARTUP_COMMAND%` and set [`CMD_STARTUP_VARIABLE`] to this value. Used
    /// when the command contains `"`: the standard encoding would turn it into `\"`, which
    /// `cmd.exe` does not understand. `cmd.exe` substitutes the variable and then parses the
    /// result like typed input, so quotes, `&`, `|` and redirections work, but `%VARIABLES%`
    /// inside the command are not expanded a second time.
    Environment(String),
}

/// A startup command as one `cmd.exe` command line: line breaks become ` & ` (`cmd.exe` would
/// otherwise stop reading at the first one) and a trailing backslash gets a protective space, so
/// that quoting for the Windows argument convention never doubles it.
pub fn cmd_command_line(command: &str) -> String {
    let mut text = command
        .split(['\r', '\n'])
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" & ");
    if text.ends_with('\\') {
        text.push(' ');
    }
    text
}

/// Prepare a command for `cmd.exe /K` started directly by Reframe (see [`CmdStartup`]).
pub fn cmd_startup(command: &str) -> CmdStartup {
    let text = cmd_command_line(command);
    let encoded = windows_quote_arg(&text);
    if encoded == text || encoded == format!("\"{text}\"") {
        CmdStartup::Argument(text)
    } else {
        CmdStartup::Environment(text)
    }
}

/// An AppleScript string literal (including the surrounding quotes) that evaluates to `s`.
/// Backslashes and quotes are escaped; line breaks and tabs use AppleScript's `\n`, `\r`, `\t`.
pub fn applescript_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The value for PowerShell's `-EncodedCommand`: Base64 of the UTF-16LE script. The result only
/// contains `A-Z a-z 0-9 + / =`, so it passes unchanged through any command-line re-quoting.
pub fn powershell_encoded_command(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Strings that try to break out of every quoting layer.
    pub(crate) const HOSTILE: &[&str] = &[
        "",
        " ",
        "plain",
        "two words",
        "it's",
        "'",
        "''",
        "\"",
        "say \"hi\"",
        "$HOME",
        "${PATH}",
        "$(rm -rf ~)",
        "`id`",
        "a;b",
        ";",
        "a && b",
        "a & b",
        "a | b",
        "a || b",
        "> out < in",
        "line1\nline2",
        "trailing newline\n",
        "tab\there",
        "carriage\rreturn",
        "ünïcödé 日本語 🚀",
        "back\\slash",
        "trailing backslash\\",
        "\\",
        "\\\\",
        "\\\"",
        "a\\;b",
        "C:\\Program Files\\App\\",
        "%PATH% !x! ^& (y)",
        "-rf",
        "--working-directory=/tmp",
        "~/home",
        "#comment",
        "*?[a]{b,c}",
        "=cmd",
        "'\"'\"'",
        "\\'",
        "end\\'",
        // Breaks out of plain POSIX single quoting when fish parses it.
        "\\';echo INJECTED;\\'",
        "x\\\\'y",
        "\u{2019}; echo smart-quote \u{2018}",
        "!!",
    ];

    // ---- POSIX shells, fish, GLib -------------------------------------------------------

    /// Reference parser for the POSIX shell (and GLib `g_shell_parse_argv`) word syntax our
    /// quoting produces. Returns `None` if a character that is special to a shell appears
    /// unquoted.
    pub(crate) fn posix_words(s: &str) -> Option<Vec<String>> {
        let mut words = Vec::new();
        let mut chars = s.chars().peekable();
        loop {
            while chars.peek().is_some_and(|c| *c == ' ') {
                chars.next();
            }
            if chars.peek().is_none() {
                break;
            }
            let mut word = String::new();
            while let Some(&c) = chars.peek() {
                match c {
                    ' ' => break,
                    '\'' => {
                        chars.next();
                        loop {
                            match chars.next()? {
                                '\'' => break,
                                c => word.push(c),
                            }
                        }
                    }
                    '\\' => {
                        chars.next();
                        word.push(chars.next()?);
                    }
                    c if is_posix_safe(c) => {
                        chars.next();
                        word.push(c);
                    }
                    _ => return None,
                }
            }
            words.push(word);
        }
        Some(words)
    }

    /// Reference parser for fish's word syntax: inside single quotes `\\` and `\'` are escapes
    /// (any other backslash is literal); outside, only the escapes our quoting emits (`\\`, `\'`)
    /// are accepted. Returns `None` for anything else special.
    pub(crate) fn fish_words(s: &str) -> Option<Vec<String>> {
        let mut words = Vec::new();
        let mut chars = s.chars().peekable();
        loop {
            while chars.peek().is_some_and(|c| *c == ' ') {
                chars.next();
            }
            if chars.peek().is_none() {
                break;
            }
            let mut word = String::new();
            while let Some(&c) = chars.peek() {
                match c {
                    ' ' => break,
                    '\'' => {
                        chars.next();
                        loop {
                            match chars.next()? {
                                '\'' => break,
                                '\\' => match chars.peek() {
                                    Some(&e @ ('\\' | '\'')) => {
                                        chars.next();
                                        word.push(e);
                                    }
                                    _ => word.push('\\'),
                                },
                                c => word.push(c),
                            }
                        }
                    }
                    '\\' => {
                        chars.next();
                        match chars.next()? {
                            e @ ('\\' | '\'') => word.push(e),
                            _ => return None,
                        }
                    }
                    c if is_posix_safe(c) => {
                        chars.next();
                        word.push(c);
                    }
                    _ => return None,
                }
            }
            words.push(word);
        }
        Some(words)
    }

    #[test]
    fn posix_quote_round_trips_hostile_input() {
        for s in HOSTILE {
            let quoted = posix_quote(s);
            assert_eq!(
                posix_words(&quoted),
                Some(vec![s.to_string()]),
                "POSIX: {s:?} -> {quoted}"
            );
            assert_eq!(
                fish_words(&quoted),
                Some(vec![s.to_string()]),
                "fish: {s:?} -> {quoted}"
            );
        }
    }

    #[test]
    fn posix_quote_leaves_safe_words_alone() {
        assert_eq!(posix_quote("/usr/bin/bash"), "/usr/bin/bash");
        assert_eq!(posix_quote("a-b_c.d:e,f+g@h"), "a-b_c.d:e,f+g@h");
        assert_eq!(posix_quote(""), "''");
        assert_eq!(posix_quote("it's"), "'it'\\''s'");
        assert_eq!(posix_quote("$(id)"), "'$(id)'");
        assert_eq!(posix_quote("=ls"), "'=ls'");
        // Backslashes never stay inside single quotes (fish would read `\\` and `\'` there).
        assert_eq!(posix_quote("a\\b"), "'a'\\\\'b'");
        assert_eq!(posix_quote("end\\"), "'end'\\\\''");
    }

    #[test]
    fn posix_join_keeps_every_element_one_word() {
        let argv: Vec<String> = HOSTILE.iter().map(|s| s.to_string()).collect();
        assert_eq!(posix_words(&posix_join(&argv)), Some(argv.clone()));
        assert_eq!(fish_words(&posix_join(&argv)), Some(argv));
    }

    /// Installed shells among the ones whose syntax [`posix_quote`] targets.
    #[cfg(unix)]
    pub(crate) fn real_shells() -> Vec<&'static str> {
        [
            "/bin/sh",
            "/bin/bash",
            "/bin/dash",
            "/bin/zsh",
            "/usr/bin/zsh",
            "/usr/bin/fish",
            "/usr/local/bin/fish",
            "/opt/homebrew/bin/fish",
        ]
        .into_iter()
        .filter(|shell| std::path::Path::new(shell).exists())
        .collect()
    }

    /// Run `script` with `shell -c` and return its standard output.
    #[cfg(unix)]
    pub(crate) fn run_shell(shell: &str, script: &str) -> String {
        let out = std::process::Command::new(shell)
            .arg("-c")
            .arg(script)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{shell} failed for {script:?}: {out:?}"
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// Real shells (POSIX shells, zsh and fish, where installed) must agree with the reference
    /// parsers: every hostile string comes back as exactly one, unchanged word.
    #[cfg(unix)]
    #[test]
    fn posix_quote_survives_real_shells() {
        // ASCII unit separator: never in HOSTILE, and literal inside quotes in every shell.
        const SEP: char = '\u{1f}';
        for shell in real_shells() {
            for s in HOSTILE {
                let printed = run_shell(shell, &format!("printf '%s' {}", posix_quote(s)));
                assert_eq!(printed, *s, "{shell}: {s:?}");
            }
            let argv: Vec<&str> = HOSTILE.to_vec();
            let printed = run_shell(shell, &format!("printf '%s{SEP}' {}", posix_join(&argv)));
            let words: Vec<&str> = printed.split(SEP).collect();
            assert_eq!(&words[..words.len() - 1], &argv[..], "{shell}");
        }
    }

    #[test]
    fn quote_for_any_shell_refuses_characters_some_shell_reads_inside_quotes() {
        for unsafe_text in [
            "it's",
            "back\\slash",
            "trailing\\",
            "wow!",
            "!!",
            "Bob\u{2019}s files",
            "\u{2018}quoted\u{2019}",
            "line\nbreak",
            "carriage\rreturn",
            "tab\there",
            "escape\u{1b}[2J",
            "kill\u{15}line",
            "next\u{85}line",
            "para\u{2029}graph",
        ] {
            assert_eq!(quote_for_any_shell(unsafe_text), None, "{unsafe_text:?}");
        }
        for folder in [
            "/Users/u/My Projects",
            "/tmp/$HOME `id` $(id) ; & | > < * ? [a] {b,c} ~ # % ^ = \"x\"",
            "/Users/u/ünïcödé 日本語 🚀",
            "/a/(b)",
            "",
        ] {
            assert_eq!(
                quote_for_any_shell(folder),
                Some(format!("'{folder}'")),
                "{folder:?}"
            );
        }
    }

    #[test]
    fn quote_for_any_shell_round_trips_whatever_it_accepts() {
        let mut accepted = 0;
        for s in HOSTILE {
            let Some(quoted) = quote_for_any_shell(s) else {
                continue;
            };
            accepted += 1;
            for parse in [posix_words, fish_words] {
                assert_eq!(parse(&quoted), Some(vec![s.to_string()]), "{s:?}");
            }
        }
        assert!(accepted > 10, "only {accepted} hostile strings accepted");
    }

    /// The exact text macOS terminals type (`cd '<folder>' && ...`) works in real shells for a
    /// folder full of characters that are special outside quotes.
    #[cfg(unix)]
    #[test]
    fn quote_for_any_shell_changes_directory_in_real_shells() {
        let temp = tempfile::tempdir().unwrap();
        let name = "a b $HOME `id` $(id) ; & | > < * ? [x] {y,z} ~ # % ^ = \"q\" ünï 🚀";
        let folder = temp.path().join(name);
        std::fs::create_dir(&folder).unwrap();
        let folder = folder.canonicalize().unwrap();
        let quoted = quote_for_any_shell(folder.to_str().unwrap()).unwrap();
        for shell in real_shells() {
            let printed = run_shell(shell, &format!("cd {quoted} && pwd"));
            assert_eq!(
                printed.trim_end_matches('\n'),
                folder.to_str().unwrap(),
                "{shell}"
            );
        }
    }

    // ---- Windows Terminal ----------------------------------------------------------------

    /// Reference model of `wt.exe`'s sub-command splitting (`AppCommandlineArgs::BuildCommands`):
    /// split each argument at `;` not preceded by `\`, then replace `\;` with `;`.
    pub(crate) fn wt_split(args: &[String]) -> Vec<Vec<String>> {
        fn delimiter(s: &str) -> Option<usize> {
            let bytes = s.as_bytes();
            (0..bytes.len()).find(|&i| bytes[i] == b';' && (i == 0 || bytes[i - 1] != b'\\'))
        }
        let mut commands: Vec<Vec<String>> = vec![Vec::new()];
        for arg in args {
            let mut remaining = arg.as_str();
            loop {
                match delimiter(remaining) {
                    None => {
                        commands
                            .last_mut()
                            .unwrap()
                            .push(remaining.replace("\\;", ";"));
                        break;
                    }
                    Some(pos) => {
                        if pos > 0 {
                            commands
                                .last_mut()
                                .unwrap()
                                .push(remaining[..pos].replace("\\;", ";"));
                        }
                        commands.push(Vec::new());
                        remaining = &remaining[pos + 1..];
                        if remaining.is_empty() {
                            break;
                        }
                    }
                }
            }
        }
        commands
    }

    #[test]
    fn wt_escape_round_trips_and_never_splits() {
        for s in HOSTILE {
            let escaped = wt_escape(s);
            let commands = wt_split(&["-d".to_string(), escaped.clone()]);
            assert_eq!(
                commands.len(),
                1,
                "{s:?} split into sub-commands: {escaped}"
            );
            assert_eq!(commands[0], vec!["-d".to_string(), s.to_string()]);
        }
        // A bare ";" argument is the delimiter Reframe itself inserts between tabs.
        let commands = wt_split(&["a".into(), ";".into(), "b".into()]);
        assert_eq!(commands, vec![vec!["a".to_string()], vec!["b".to_string()]]);
    }

    // ---- Windows argument convention -----------------------------------------------------

    /// Reference parser for arguments following the Microsoft C runtime rules (also used by
    /// `CommandLineToArgvW` for everything after the program name).
    pub(crate) fn windows_split(cmdline: &str) -> Vec<String> {
        let chars: Vec<char> = cmdline.chars().collect();
        let mut args = Vec::new();
        let mut i = 0;
        loop {
            while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') {
                i += 1;
            }
            if i >= chars.len() {
                break;
            }
            let mut arg = String::new();
            let mut quoted = false;
            while i < chars.len() {
                let c = chars[i];
                if !quoted && (c == ' ' || c == '\t') {
                    break;
                }
                if c == '\\' {
                    let start = i;
                    while i < chars.len() && chars[i] == '\\' {
                        i += 1;
                    }
                    let n = i - start;
                    if i < chars.len() && chars[i] == '"' {
                        arg.extend(std::iter::repeat_n('\\', n / 2));
                        if n % 2 == 1 {
                            arg.push('"');
                            i += 1;
                        }
                    } else {
                        arg.extend(std::iter::repeat_n('\\', n));
                    }
                    continue;
                }
                if c == '"' {
                    if quoted && chars.get(i + 1) == Some(&'"') {
                        arg.push('"');
                        i += 2;
                    } else {
                        quoted = !quoted;
                        i += 1;
                    }
                    continue;
                }
                arg.push(c);
                i += 1;
            }
            args.push(arg);
        }
        args
    }

    #[test]
    fn reference_windows_parser_matches_microsoft_documentation() {
        // Examples from "Parsing C++ command-line arguments" (Microsoft Learn).
        let cases: &[(&str, &[&str])] = &[
            (r#""abc" d e"#, &["abc", "d", "e"]),
            (r#"a\\b d"e f"g h"#, &[r"a\\b", "de fg", "h"]),
            (r#"a\\\"b c d"#, &[r#"a\"b"#, "c", "d"]),
            (r#"a\\\\"b c" d e"#, &[r"a\\b c", "d", "e"]),
            (r#"a"b"" c d"#, &[r#"ab" c d"#]),
        ];
        for (input, expected) in cases {
            assert_eq!(windows_split(input), *expected, "{input}");
        }
    }

    #[test]
    fn windows_quote_arg_round_trips_hostile_input() {
        for s in HOSTILE {
            let quoted = windows_quote_arg(s);
            assert_eq!(
                windows_split(&quoted),
                vec![s.to_string()],
                "{s:?} -> {quoted}"
            );
        }
        let line = HOSTILE
            .iter()
            .map(|s| windows_quote_arg(s))
            .collect::<Vec<_>>()
            .join(" ");
        let expected: Vec<String> = HOSTILE.iter().map(|s| s.to_string()).collect();
        assert_eq!(windows_split(&line), expected);
    }

    #[test]
    fn windows_quote_arg_backslash_rules() {
        assert_eq!(windows_quote_arg(r"C:\dir\file"), r"C:\dir\file");
        assert_eq!(windows_quote_arg(r"C:\My Dir\"), r#""C:\My Dir\\""#);
        // Like Rust's std: quotes are escaped, but only spaces and tabs force surrounding quotes.
        assert_eq!(windows_quote_arg(r#"a\"b"#), r#"a\\\"b"#);
        assert_eq!(windows_quote_arg(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(windows_quote_arg("tab\there"), "\"tab\there\"");
        assert_eq!(windows_quote_arg("line\nbreak"), "line\nbreak");
        assert_eq!(windows_quote_arg(""), r#""""#);
    }

    // ---- cmd.exe -------------------------------------------------------------------------

    /// What `cmd.exe /K` executes for a raw command line remainder (the "old behaviour" quote
    /// rule: strip the first quote and the last quote when the text starts with a quote).
    pub(crate) fn cmd_k_received(remainder: &str) -> String {
        match remainder.strip_prefix('"') {
            Some(rest) => match rest.rfind('"') {
                Some(last) => format!("{}{}", &rest[..last], &rest[last + 1..]),
                None => rest.to_string(),
            },
            None => remainder.to_string(),
        }
    }

    #[test]
    fn cmd_startup_delivers_quote_free_commands_verbatim() {
        for command in [
            "npm run dev",
            "cd C:\\code & dir",
            "echo a|sort",
            "dir C:\\",
            "%USERPROFILE%\\x.bat",
        ] {
            match cmd_startup(command) {
                CmdStartup::Argument(text) => {
                    let received = cmd_k_received(&windows_quote_arg(&text));
                    assert_eq!(received.trim_end(), command, "{command}");
                }
                other => panic!("{command}: {other:?}"),
            }
        }
    }

    #[test]
    fn cmd_startup_uses_environment_for_quotes() {
        assert_eq!(
            cmd_startup(r#"cd "C:\My Projects\app" & npm start"#),
            CmdStartup::Environment(r#"cd "C:\My Projects\app" & npm start"#.to_string())
        );
        assert_eq!(
            cmd_startup("echo one\r\necho two\n"),
            CmdStartup::Argument("echo one & echo two".to_string())
        );
        assert_eq!(
            cmd_startup("cd C:\\temp\\"),
            CmdStartup::Argument("cd C:\\temp\\ ".to_string())
        );
    }

    /// Reference model of how Windows Terminal turns a tab's trailing arguments back into one
    /// command line (`AppCommandlineArgs::_getNewTerminalArgs`): an argument containing a space
    /// is wrapped in plain quotes; nothing is escaped.
    pub(crate) fn wt_join(args: &[String]) -> String {
        args.iter()
            .map(|a| {
                if a.contains(' ') {
                    format!("\"{a}\"")
                } else {
                    a.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// In a Windows Terminal tab `cmd.exe /K <text>` goes through the Windows argument
    /// convention, wt's `;` splitter and wt's re-joining; `cmd.exe` must still receive the text.
    #[test]
    fn cmd_command_line_survives_windows_terminal() {
        for command in [
            "npm run dev",
            r#"cd "C:\My Projects\app" & npm start"#,
            r#"echo "a;b" & echo \;"#,
            "dir C:\\temp\\",
            "echo \"unbalanced",
            "set X=1&& echo %X% ^& !Y!",
            "echo one\r\necho \"two\"\n",
            "ünïcödé 日本語",
        ] {
            let text = cmd_command_line(command);
            let args = ["cmd.exe".to_string(), "/K".to_string(), wt_escape(&text)];
            let line = args
                .iter()
                .map(|a| windows_quote_arg(a))
                .collect::<Vec<_>>()
                .join(" ");
            let commands = wt_split(&windows_split(&line));
            assert_eq!(commands.len(), 1, "{command:?} split into sub-commands");
            let joined = wt_join(&commands[0]);
            let remainder = joined.strip_prefix("cmd.exe /K ").unwrap();
            assert_eq!(cmd_k_received(remainder), text, "{command:?}");
        }
    }

    // ---- AppleScript ---------------------------------------------------------------------

    /// Reference decoder for an AppleScript string literal; `None` if it is not one literal.
    pub(crate) fn applescript_decode(literal: &str) -> Option<String> {
        let inner = literal.strip_prefix('"')?.strip_suffix('"')?;
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => match chars.next()? {
                    '\\' => out.push('\\'),
                    '"' => out.push('"'),
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    _ => return None,
                },
                '"' => return None,
                c => out.push(c),
            }
        }
        Some(out)
    }

    #[test]
    fn applescript_string_round_trips_hostile_input() {
        for s in HOSTILE {
            let literal = applescript_string(s);
            assert_eq!(
                applescript_decode(&literal).as_deref(),
                Some(*s),
                "{s:?} -> {literal}"
            );
            assert!(!literal.contains('\n'));
        }
    }

    // ---- PowerShell ----------------------------------------------------------------------

    #[test]
    fn powershell_encoded_command_is_utf16le_base64() {
        for s in HOSTILE {
            let encoded = powershell_encoded_command(s);
            assert!(
                encoded
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='))
            );
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&encoded)
                .unwrap();
            let (pairs, rest) = bytes.as_chunks::<2>();
            assert!(rest.is_empty());
            let units: Vec<u16> = pairs.iter().map(|b| u16::from_le_bytes(*b)).collect();
            assert_eq!(String::from_utf16(&units).unwrap(), *s);
        }
        assert_eq!(powershell_encoded_command("dir"), "ZABpAHIA");
    }
}
