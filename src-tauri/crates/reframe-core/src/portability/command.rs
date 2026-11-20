//! Redacting secrets from user-authored shell commands (terminal startup commands) before
//! export, keeping the rest of the command byte-for-byte so it stays reviewable.
//!
//! Understands POSIX shells, PowerShell and `cmd.exe` well enough for the common ways secrets
//! end up in a command:
//! - variable assignments: `export GITHUB_TOKEN=…`, `API_KEY=… npm start`, `set TOKEN=…`,
//!   `setx TOKEN …`, fish `set -x TOKEN …`, PowerShell `$env:TOKEN = "…"`;
//! - options: `--token=…`, `--password …`, `-Dkey.password=…`, `/password:…`, plus the short
//!   password options of well-known tools (`mysql -p…`, `sshpass -p …`, `docker login -p …`,
//!   `sqlcmd -P …`, `redis-cli -a …`, `7z -p…`, `curl -u user:…`, `smbclient -U user%…`,
//!   `xfreerdp /p:…`) and PowerShell `ConvertTo-SecureString "…"`;
//! - configuration commands: `npm config set //registry/:_authToken …`,
//!   `git config user.password …`, `gh secret set NAME --body …`, `--set auth.password=…`;
//! - HTTP headers and auth schemes: `-H "Authorization: Bearer …"`, `Bearer …`;
//! - parameter lists: form data, connection strings (`…;Password=…`), cookies;
//! - URLs with credentials or secret parameters, and credential-shaped values.
//!
//! References are kept because they contain no secret: `$(gh auth token)`, `$TOKEN`,
//! `%TOKEN%`, and absolute paths (`--key ~/.ssh/id_ed25519`).
//!
//! The replacement is `<redacted>`, always inside quotes (`export TOKEN="<redacted>"`): an
//! unquoted `<…>` is a shell redirection, and a user who re-enables the command without
//! editing it must not truncate a file by accident.
//!
//! Credentials in a known format (provider token prefixes, JWTs, AWS key ids, PEM keys) are
//! removed wherever they appear. Redaction is repeated until nothing changes, so it is
//! idempotent.
//!
//! Limitations: this is not a full shell parser. Here-documents and here-strings, positional
//! passwords (`tool user hunter2`, `vault login s.abc`), values piped in (`echo pw | tool
//! --password-stdin`), secrets under innocent names with ordinary-looking values, and option
//! values that start with `-` are not recognised. Imported commands are switched off anyway
//! and must be reviewed before they run.

use super::paths::looks_like_absolute_path;
use super::sanitize::{
    Rejection, UrlCheck, WEB_SCHEMES, check_url, redact_generic_url, url_scheme,
};
use super::secrets::{
    NameKind, is_known_credential_format, is_secret_name, looks_like_credential,
    scrub_known_credentials,
};

/// Text inserted in place of a removed secret.
pub(crate) const REDACTED: &str = "<redacted>";

/// Commands that only run another command (`sudo -E mysql …`).
const WRAPPERS: &[&str] = &[
    "sudo", "doas", "env", "nohup", "time", "exec", "command", "nice", "builtin",
];
/// Tools whose `login` sub-command takes `-p <password>`.
const LOGIN_COMMANDS: &[&str] = &[
    "docker", "podman", "nerdctl", "helm", "buildah", "skopeo", "oras", "crane", "az", "oc", "cf",
];
/// MySQL clients take the password glued to `-p` (`-phunter2`).
const MYSQL_COMMANDS: &[&str] = &[
    "mysql",
    "mariadb",
    "mysqldump",
    "mysqladmin",
    "mysqlimport",
    "mysqlshow",
];
const MONGO_COMMANDS: &[&str] = &[
    "mongo",
    "mongosh",
    "mongodump",
    "mongorestore",
    "mongoexport",
    "mongoimport",
    "mongostat",
    "mongotop",
];
/// Sub-commands that set configuration: a following word with a secret-looking name is a key
/// whose value comes next (`npm config set //registry/:_authToken …`, `gh secret set NAME`).
const CONFIG_WORDS: &[&str] = &["config", "configure", "set"];

/// How a short password option takes its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueStyle {
    /// `-p VALUE` or `-pVALUE`.
    SeparateOrGlued,
    /// Only `-pVALUE`: `-p` alone asks for the password, and the next word is something else
    /// (the database for `mysql -p shop`).
    GluedOnly,
}

/// Short options that take a password, per program.
const SHORT_PASSWORD_OPTIONS: &[(&[&str], &str, ValueStyle)] = &[
    (&["sshpass"], "-p", ValueStyle::SeparateOrGlued),
    (MYSQL_COMMANDS, "-p", ValueStyle::GluedOnly),
    (
        &["sqlcmd", "bcp", "osql"],
        "-P",
        ValueStyle::SeparateOrGlued,
    ),
    (MONGO_COMMANDS, "-p", ValueStyle::SeparateOrGlued),
    (
        &["redis-cli", "keydb-cli", "valkey-cli"],
        "-a",
        ValueStyle::SeparateOrGlued,
    ),
    (
        &["zip", "unzip", "zipcloak"],
        "-P",
        ValueStyle::SeparateOrGlued,
    ),
    (
        &["7z", "7za", "7zr", "7zz", "rar", "unrar"],
        "-p",
        ValueStyle::GluedOnly,
    ),
    (&["ssh-keygen"], "-N", ValueStyle::SeparateOrGlued),
    (&["ssh-keygen"], "-P", ValueStyle::SeparateOrGlued),
    (&["rdesktop"], "-p", ValueStyle::SeparateOrGlued),
];
/// FreeRDP takes `/p:<password>`.
const FREERDP_COMMANDS: &[&str] = &[
    "xfreerdp",
    "xfreerdp3",
    "wlfreerdp",
    "wlfreerdp3",
    "sdl-freerdp",
    "sdl-freerdp3",
];
/// Samba tools take `-U user%password`.
const SAMBA_COMMANDS: &[&str] = &["smbclient", "rpcclient", "smbcacls", "smbget"];
/// Redacting URLs is repeated until nothing changes, at most this many times.
const MAX_URL_PASSES: usize = 4;
/// Redacting a value is repeated until nothing changes, at most this many times.
const MAX_TEXT_PASSES: usize = 6;
/// Redacting a whole command is repeated until nothing changes, at most this many times.
const MAX_COMMAND_PASSES: usize = 4;

/// Redact secrets in `command`; see the module docs. Idempotent: redaction is repeated until
/// nothing changes (removing one secret can reveal the shape of another), and a command that
/// is still changing after a few passes is replaced as a whole.
pub(crate) fn redact_command(command: &str) -> String {
    let mut current = redact_command_once(command);
    if current == command {
        return current;
    }
    for _ in 1..MAX_COMMAND_PASSES {
        let next = redact_command_once(&current);
        if next == current {
            return current;
        }
        current = next;
    }
    format!("\"{REDACTED}\"")
}

fn redact_command_once(command: &str) -> String {
    let mut out = String::with_capacity(command.len() + 16);
    let mut segment = Segment::default();
    for token in tokenize(command) {
        match token {
            Token::Space(text) => out.push_str(text),
            Token::Separator(text) => {
                out.push_str(text);
                segment = Segment::default();
            }
            Token::Word(word) => match segment.process(word) {
                Some(replacement) => out.push_str(&replacement),
                None => out.push_str(word),
            },
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    Space(&'a str),
    /// `;`, `|`, `&`, `&&`, `||`, newlines: a new simple command starts after it.
    Separator(&'a str),
    Word(&'a str),
}

fn is_space(c: char) -> bool {
    c == ' ' || c == '\t'
}

fn is_separator(c: char) -> bool {
    matches!(c, ';' | '|' | '&' | '\n' | '\r')
}

fn tokenize(input: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut chars = input.char_indices().peekable();
    while let Some(&(start, first)) = chars.peek() {
        if is_space(first) || is_separator(first) {
            let space = is_space(first);
            let mut end = start;
            while let Some(&(i, c)) = chars.peek() {
                if (space && is_space(c)) || (!space && is_separator(c)) {
                    end = i + c.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
            let text = &input[start..end];
            tokens.push(if space {
                Token::Space(text)
            } else {
                Token::Separator(text)
            });
            continue;
        }
        let mut end = input.len();
        while let Some(&(i, c)) = chars.peek() {
            if is_space(c) || is_separator(c) {
                end = i;
                break;
            }
            chars.next();
            match c {
                '\\' => {
                    chars.next();
                }
                '\'' | '`' => {
                    for (_, q) in chars.by_ref() {
                        if q == c {
                            break;
                        }
                    }
                }
                '"' => {
                    while let Some((_, q)) = chars.next() {
                        match q {
                            '\\' => {
                                chars.next();
                            }
                            '"' => break,
                            _ => {}
                        }
                    }
                }
                '$' => {
                    if let Some(&(_, open @ ('(' | '{'))) = chars.peek() {
                        let close = if open == '(' { ')' } else { '}' };
                        chars.next();
                        let mut depth = 1u32;
                        for (_, q) in chars.by_ref() {
                            if q == open {
                                depth += 1;
                            } else if q == close {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        tokens.push(Token::Word(&input[start..end]));
    }
    tokens
}

/// What the next word of the current simple command is expected to be.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Expect {
    #[default]
    Nothing,
    /// The value of a secret option or variable.
    Secret,
    /// The next word that is not an option is a secret (`ConvertTo-SecureString -String …`,
    /// `gh secret set NAME --body …`).
    SecretAfterOptions,
    /// `user<separator>password` (`curl -u alice:…`, `smbclient -U alice%…`).
    UserPassword(char),
    /// PowerShell `$env:SECRET` was seen: `=` and then the value may follow.
    Equals,
    /// The value of a secret PowerShell variable (`$env:TOKEN = …`): a quoted literal is
    /// redacted, a command (`Read-Host`, `Get-Secret …`) is kept.
    PsValue,
    /// `set`/`setx` was seen: the next non-switch word is a variable name.
    VariableName,
}

/// State of one simple command (reset at `;`, `|`, `&&`, newlines...).
#[derive(Debug, Default)]
struct Segment {
    expect: Expect,
    /// Lower-cased base name of the command word, once seen.
    command: Option<String>,
    /// A wrapper (`sudo`, `env`...) was seen: the next plain word is the real command.
    after_wrapper: bool,
    /// A `login` sub-command was seen (`docker login`).
    login: bool,
    /// A configuration sub-command was seen (`npm config set`, `git config`).
    config: bool,
}

impl Segment {
    /// The replacement for `raw`, or `None` to keep it.
    fn process(&mut self, raw: &str) -> Option<String> {
        let expect = std::mem::take(&mut self.expect);
        if matches!(
            expect,
            Expect::Secret | Expect::SecretAfterOptions | Expect::PsValue
        ) {
            // `-Password (ConvertTo-SecureString "…" -AsPlainText)`: the literal comes later.
            if is_secure_string_cmdlet(raw) {
                self.expect = Expect::SecretAfterOptions;
                return None;
            }
            // `-Password (Get-Secret x)`, `$(…)`: an expression, evaluated when it runs.
            if raw.starts_with('(') {
                return None;
            }
        }
        match expect {
            Expect::Secret => {
                if is_standalone_auth_scheme(raw) {
                    self.expect = Expect::Secret;
                    return None;
                }
                if !is_option(raw) {
                    return redact_whole(raw);
                }
            }
            Expect::SecretAfterOptions => {
                if !is_option(raw) {
                    return redact_whole(raw);
                }
                self.expect = Expect::SecretAfterOptions;
            }
            Expect::UserPassword(separator) => {
                if !is_option(raw) {
                    return redact_user_password(raw, separator);
                }
            }
            Expect::Equals => {
                if raw == "=" {
                    self.expect = Expect::PsValue;
                    return None;
                }
                if let Some(value) = raw.strip_prefix('=') {
                    return redact_ps_value(value).map(|value| format!("={value}"));
                }
            }
            Expect::PsValue => {
                if !is_option(raw) {
                    return redact_ps_value(raw);
                }
            }
            Expect::VariableName => {
                if raw.starts_with(['-', '/']) {
                    self.expect = Expect::VariableName;
                    return None;
                }
                if !raw.contains('=') {
                    if is_secret_name(&unquote(raw), NameKind::Variable) {
                        self.expect = Expect::Secret;
                    }
                    return None;
                }
            }
            Expect::Nothing => {}
        }

        // PowerShell `$env:NAME = value` (or a plain `$NAME` reference).
        if let Some(name) = ps_variable(raw) {
            if is_secret_name(name, NameKind::Variable) {
                self.expect = Expect::Equals;
            }
            return None;
        }

        // `ConvertTo-SecureString "hunter2" -AsPlainText`, possibly inside `( … )`.
        if is_secure_string_cmdlet(raw) {
            self.expect = Expect::SecretAfterOptions;
            return None;
        }

        let assignment = split_assignment(raw);
        if self.command.is_none() && assignment.is_none() && !(self.after_wrapper && is_option(raw))
        {
            let base = command_basename(&unquote(raw));
            if WRAPPERS.contains(&base.as_str()) {
                self.after_wrapper = true;
                return None;
            }
            self.after_wrapper = false;
            if base == "set" || base == "setx" {
                self.expect = Expect::VariableName;
            }
            self.command = Some(base);
            return redact_value(raw);
        }

        if let Some(rule) = self.known_command_rule(raw) {
            return rule;
        }
        if let Some(assignment) = assignment {
            // `$pw= "hunter2"`: PowerShell allows the value after a space.
            if assignment.value.is_empty()
                && assignment.head.starts_with('$')
                && is_secret_name(&assignment.name, assignment.kind)
            {
                self.expect = Expect::PsValue;
                return None;
            }
            return assignment.redact(raw);
        }
        if let Some(name) = option_name(raw) {
            let config_value = self.config && matches!(name, "body" | "b" | "value");
            if config_value || is_secret_name(name, NameKind::Flag) {
                self.expect = Expect::Secret;
            }
            return None;
        }
        if is_standalone_auth_scheme(raw) {
            self.expect = Expect::Secret;
            return None;
        }
        let word = unquote(raw);
        if let Some(header) = word.strip_suffix(':')
            && is_secret_header(header)
        {
            self.expect = Expect::Secret;
            return None;
        }
        if CONFIG_WORDS.iter().any(|w| word.eq_ignore_ascii_case(w)) {
            self.config = true;
            return None;
        }
        if self.config
            && !is_option(raw)
            && !word.contains('=')
            && is_secret_name(&word, NameKind::Parameter)
        {
            self.expect = Expect::SecretAfterOptions;
            return None;
        }
        redact_value(raw)
    }

    /// Password options of well-known tools. `Some(result)` when a rule applied.
    fn known_command_rule(&mut self, raw: &str) -> Option<Option<String>> {
        let command = self.command.as_deref()?;
        let logs_in = LOGIN_COMMANDS.contains(&command);
        if logs_in && raw == "login" {
            self.login = true;
            return Some(None);
        }
        if self.login
            && logs_in
            && let Some(rule) = short_password_rule(raw, "-p", ValueStyle::SeparateOrGlued)
        {
            self.expect = rule.expect;
            return Some(rule.replacement);
        }
        for (commands, option, style) in SHORT_PASSWORD_OPTIONS {
            if commands.contains(&command)
                && let Some(rule) = short_password_rule(raw, option, *style)
            {
                self.expect = rule.expect;
                return Some(rule.replacement);
            }
        }
        if command == "curl" {
            return self.user_password_rule(raw, &["-u", "-U"], &["--user", "--proxy-user"], ':');
        }
        if SAMBA_COMMANDS.contains(&command) {
            return self.user_password_rule(raw, &["-U"], &["--user"], '%');
        }
        if FREERDP_COMMANDS.contains(&command)
            && raw.len() > 3
            && raw.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("/p:"))
        {
            return Some(redact_glued(&raw[..3], &raw[3..]));
        }
        None
    }

    /// `-u alice:pw`, `-ualice:pw`, `--user alice:pw`, `--user=alice:pw`.
    fn user_password_rule(
        &mut self,
        raw: &str,
        short: &[&str],
        long: &[&str],
        separator: char,
    ) -> Option<Option<String>> {
        if short.contains(&raw) || long.contains(&raw) {
            self.expect = Expect::UserPassword(separator);
            return Some(None);
        }
        let head_len = if let Some(option) = short.iter().find(|o| raw.starts_with(*o)) {
            option.len()
        } else {
            let option = long
                .iter()
                .find(|o| raw.starts_with(*o) && raw[o.len()..].starts_with('='))?;
            option.len() + 1
        };
        Some(
            redact_user_password(&raw[head_len..], separator)
                .map(|value| format!("{}{value}", &raw[..head_len])),
        )
    }
}

/// Result of a short password option rule.
struct ShortRule {
    expect: Expect,
    replacement: Option<String>,
}

/// `-p` (the next word is the password, unless glued-only) or `-pVALUE`.
fn short_password_rule(raw: &str, option: &str, style: ValueStyle) -> Option<ShortRule> {
    if raw == option {
        return Some(ShortRule {
            expect: match style {
                ValueStyle::SeparateOrGlued => Expect::Secret,
                ValueStyle::GluedOnly => Expect::Nothing,
            },
            replacement: None,
        });
    }
    let value = raw.strip_prefix(option)?;
    Some(ShortRule {
        expect: Expect::Nothing,
        replacement: redact_glued(option, value),
    })
}

/// `{head}"<redacted>"` unless the glued value is a reference or a file.
fn redact_glued(head: &str, value: &str) -> Option<String> {
    redact_whole(value).map(|value| format!("{head}{value}"))
}

/// `NAME=value`, `--name=value`, `-Dname=value`, `/name:value`, `$env:NAME=value`, or one of
/// those wrapped in quotes as a whole (`"NAME=value"`).
#[derive(Debug)]
struct Assignment {
    /// Everything before the value: `--token=`, `GITHUB_TOKEN=`, `/password:`.
    head: String,
    name: String,
    kind: NameKind,
    /// For a word quoted as a whole, the value as the program receives it (unquoted);
    /// otherwise the value as written (it may carry its own quotes).
    value: String,
    /// The quote around the whole word, if any.
    quote: Option<char>,
    /// Parsed from the unquoted word because quotes or escapes sit inside it (`--to"ken"=…`,
    /// `'TOKEN='…`): a changed word is rebuilt as one double-quoted word, and the value is
    /// never trusted to be a reference.
    rebuilt: bool,
}

impl Assignment {
    /// The replacement for `raw` (the word this assignment was parsed from), if any.
    fn redact(&self, raw: &str) -> Option<String> {
        let secret = is_secret_name(&self.name, self.kind);
        let replacement = match self.quote {
            Some(quote) => {
                let new_value = if secret {
                    let keep = self.value.is_empty()
                        || (!self.rebuilt && quote != '\'' && is_reference(&self.value))
                        || looks_like_absolute_path(&self.value);
                    if keep {
                        return None;
                    }
                    REDACTED.to_string()
                } else {
                    redact_text(&self.value)?
                };
                let word = format!("{}{new_value}", self.head);
                if self.rebuilt {
                    double_quote(&word)
                } else {
                    requote(raw, &word)
                }
            }
            None => {
                let new_value = if secret {
                    let keep = self.value.is_empty()
                        || is_reference(&self.value)
                        || looks_like_absolute_path(&unquote(&self.value))
                        || (self.head.starts_with('$') && is_ps_command(&self.value));
                    if keep {
                        return None;
                    }
                    requote(&self.value, REDACTED)
                } else {
                    redact_value(&self.value)?
                };
                format!("{}{new_value}", self.head)
            }
        };
        (replacement != raw).then_some(replacement)
    }
}

fn split_assignment(raw: &str) -> Option<Assignment> {
    if let Some(quote) = enclosing_quote(raw) {
        let mut assignment = split_unquoted_assignment(&unquote(raw))?;
        assignment.quote = Some(quote);
        return Some(assignment);
    }
    if let Some(assignment) = split_unquoted_assignment(raw) {
        return Some(assignment);
    }
    // Quotes or escapes inside the word: judge it as the program receives it.
    if raw.contains(['"', '\'', '\\']) {
        let mut assignment = split_unquoted_assignment(&unquote(raw))?;
        assignment.quote = Some('"');
        assignment.rebuilt = true;
        return Some(assignment);
    }
    None
}

fn split_unquoted_assignment(word: &str) -> Option<Assignment> {
    let has_env_prefix = word
        .get(..5)
        .is_some_and(|p| p.eq_ignore_ascii_case("$env:"));
    let (prefix_len, kind, windows_switch) = if word.starts_with("--") {
        (2, NameKind::Flag, false)
    } else if word.starts_with('-') {
        (1, NameKind::Flag, false)
    } else if word.starts_with('/') {
        (1, NameKind::Flag, true)
    } else if has_env_prefix {
        (5, NameKind::Variable, false)
    } else if word.starts_with('$') {
        (1, NameKind::Variable, false)
    } else {
        (0, NameKind::Variable, false)
    };
    let rest = &word[prefix_len..];
    let separator = if windows_switch {
        rest.find([':', '='])?
    } else {
        rest.find('=')?
    };
    let name = &rest[..separator];
    let valid = match kind {
        NameKind::Flag if windows_switch => {
            !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        }
        NameKind::Flag => {
            name.chars().next().is_some_and(char::is_alphabetic)
                && name
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
        }
        _ => is_assignment_key(name),
    };
    if !valid {
        return None;
    }
    let head_len = prefix_len + separator + 1;
    Some(Assignment {
        head: word[..head_len].to_string(),
        name: name.to_string(),
        kind,
        value: word[head_len..].to_string(),
        quote: None,
        rebuilt: false,
    })
}

/// Shell variable names, plus the dotted and namespaced keys of configuration assignments
/// (`auth.password=…`, `Api:Key=…`, `spring.datasource.password=…`, `items[0].token=…`).
fn is_assignment_key(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | ':' | '[' | ']'))
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `$env:NAME` / `$NAME` without an `=`: the name.
fn ps_variable(raw: &str) -> Option<&str> {
    let rest = raw.strip_prefix('$')?;
    let name = match rest.get(..4) {
        Some(prefix) if prefix.eq_ignore_ascii_case("env:") => &rest[4..],
        _ => rest,
    };
    is_identifier(name).then_some(name)
}

/// `--name` / `-name` (two or more characters, no value attached): the name.
fn option_name(raw: &str) -> Option<&str> {
    if raw.contains('=') {
        return None;
    }
    let name = raw.strip_prefix("--").or_else(|| raw.strip_prefix('-'))?;
    let valid = name.len() >= 2
        && name.chars().next().is_some_and(char::is_alphabetic)
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'));
    valid.then_some(name)
}

/// Whether the shell passes `raw` on as an option (`-x`, `--x`, also `"--x"` and `'-x'`).
fn is_option(raw: &str) -> bool {
    let word = raw.trim_start_matches(['"', '\'']);
    word.len() > 1 && word.starts_with('-')
}

fn command_basename(word: &str) -> String {
    let base = word.rsplit(['/', '\\']).next().unwrap_or(word);
    let lower = base.to_lowercase();
    match lower.strip_suffix(".exe") {
        Some(stem) => stem.to_string(),
        None => lower,
    }
}

/// Common PowerShell verbs of cmdlets that produce a value (`Read-Host`, `Get-Secret`).
const PS_VALUE_VERBS: &[&str] = &[
    "get",
    "read",
    "import",
    "convertfrom",
    "convertto",
    "invoke",
    "new",
    "unprotect",
    "resolve",
    "select",
    "join",
    "format",
    "find",
    "request",
    "receive",
];

/// An unquoted `Verb-Noun` cmdlet call (`Read-Host`, `Get-Content`): in PowerShell a bare word
/// after `=` runs a command, so it is not a literal secret.
fn is_ps_command(raw: &str) -> bool {
    raw.split_once('-').is_some_and(|(verb, noun)| {
        PS_VALUE_VERBS.iter().any(|v| verb.eq_ignore_ascii_case(v))
            && noun.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && noun.chars().all(|c| c.is_ascii_alphanumeric())
    })
}

/// The value of a secret PowerShell variable: commands and references are kept, anything
/// else (a quoted literal, a bare value) is redacted.
fn redact_ps_value(raw: &str) -> Option<String> {
    if raw.is_empty() || is_ps_command(raw) {
        return None;
    }
    redact_whole(raw)
}

/// PowerShell's `ConvertTo-SecureString`, whose next non-option word is a plain-text secret.
fn is_secure_string_cmdlet(raw: &str) -> bool {
    raw.trim_start_matches(['(', '$'])
        .trim_start_matches('(')
        .eq_ignore_ascii_case("ConvertTo-SecureString")
}

/// `Bearer …` / `Basic …` as separate words: the next word is the credential.
fn is_standalone_auth_scheme(raw: &str) -> bool {
    raw.eq_ignore_ascii_case("bearer") || raw.eq_ignore_ascii_case("basic")
}

fn is_header_auth_scheme(word: &str) -> bool {
    ["bearer", "basic", "digest", "negotiate", "ntlm", "token"]
        .iter()
        .any(|s| word.eq_ignore_ascii_case(s))
}

/// A secret HTTP header name (`Authorization`, `Cookie`, `X-Api-Key`, `PRIVATE-TOKEN`). Header
/// names are plain tokens, so text with `/`, `:`, `=` or spaces never counts.
fn is_secret_header(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        && ([
            "authorization",
            "proxy-authorization",
            "cookie",
            "set-cookie",
        ]
        .iter()
        .any(|h| name.eq_ignore_ascii_case(h))
            || is_secret_name(name, NameKind::Parameter))
}

/// Values that refer to a secret without containing it: `$TOKEN`, `$(gh auth token)`,
/// `` `cat token` ``, `%TOKEN%` (unquoted or double-quoted). `$'…'` and `$"…"` are literal
/// strings, not references.
fn is_reference(raw: &str) -> bool {
    let inner = raw.strip_prefix('"').unwrap_or(raw);
    let shell = inner.starts_with('$') && !inner.starts_with("$'") && !inner.starts_with("$\"");
    let cmd = inner
        .strip_prefix('%')
        .and_then(|rest| rest.split_once('%'))
        .is_some_and(|(name, _)| is_identifier(name));
    shell || cmd || inner.starts_with('`')
}

/// The quote character when `raw` is one complete quoted string (`'…'` or `"…"`).
fn enclosing_quote(raw: &str) -> Option<char> {
    let quote = raw.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    if raw.len() < 2 || !raw.ends_with(quote) {
        return None;
    }
    let inner = &raw[1..raw.len() - 1];
    let closes_early = if quote == '\'' {
        inner.contains('\'')
    } else {
        let mut chars = inner.chars();
        let mut found = false;
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    chars.next();
                }
                '"' => {
                    found = true;
                    break;
                }
                _ => {}
            }
        }
        found
    };
    (!closes_early).then_some(quote)
}

/// Approximately what the shell passes on: quotes removed, `\` escapes of shell-special
/// characters resolved (other backslashes are kept, so Windows paths stay intact).
fn unquote(raw: &str) -> String {
    const ESCAPABLE: &str = " \t\\'\"$`;&|<>()*?#!{}[]~";
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        match (quote, c) {
            (None, '\'' | '"') => quote = Some(c),
            (Some(q), _) if c == q => quote = None,
            (None, '\\') => match chars.peek() {
                Some(&next) if ESCAPABLE.contains(next) => {
                    out.push(next);
                    chars.next();
                }
                _ => out.push('\\'),
            },
            (Some('"'), '\\') => match chars.peek() {
                Some(&next) if matches!(next, '"' | '\\' | '$' | '`') => {
                    out.push(next);
                    chars.next();
                }
                _ => out.push('\\'),
            },
            _ => out.push(c),
        }
    }
    out
}

/// Put `new_inner` (unquoted text) back in place of `raw`, keeping `raw`'s quoting where that
/// is safe. A replacement containing whitespace or shell metacharacters is double-quoted (so
/// it stays one word, and `<redacted>` cannot act as a redirection).
fn requote(raw: &str, new_inner: &str) -> String {
    match enclosing_quote(raw) {
        Some('\'') if !new_inner.contains('\'') => format!("'{new_inner}'"),
        Some(_) => double_quote(new_inner),
        None if new_inner.chars().any(|c| {
            c.is_whitespace()
                || matches!(
                    c,
                    '\'' | '"' | ';' | '&' | '|' | '<' | '>' | '(' | ')' | '`'
                )
        }) =>
        {
            double_quote(new_inner)
        }
        None => new_inner.to_string(),
    }
}

/// `text` as one double-quoted word: `"` and `` ` `` are escaped, and so is a backslash where
/// it would otherwise escape the next character (Windows paths stay readable).
fn double_quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' | '`' => {
                out.push('\\');
                out.push(c);
            }
            '\\' if matches!(chars.peek(), None | Some('\\' | '"' | '`' | '$')) => {
                out.push_str("\\\\");
            }
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The whole word is a secret value (unless it is a reference or an absolute path).
fn redact_whole(raw: &str) -> Option<String> {
    if is_reference(raw) || looks_like_absolute_path(&unquote(raw)) {
        return None;
    }
    let replacement = requote(raw, REDACTED);
    (replacement != raw).then_some(replacement)
}

/// `user:password` → `user:<redacted>` (or `user%password` with `%`).
fn redact_user_password(raw: &str, separator: char) -> Option<String> {
    if is_reference(raw) {
        return None;
    }
    let value = unquote(raw);
    let (user, password) = value.split_once(separator)?;
    if password.is_empty() || is_reference(password) {
        return None;
    }
    let replacement = requote(raw, &format!("{user}{separator}{REDACTED}"));
    (replacement != raw).then_some(replacement)
}

/// Content checks on a value as written (quotes are kept around the result).
fn redact_value(raw: &str) -> Option<String> {
    if raw.is_empty() || is_reference(raw) {
        return None;
    }
    let new = redact_text(&unquote(raw))?;
    let replacement = requote(raw, &new);
    (replacement != raw).then_some(replacement)
}

/// Content checks on an unquoted value: secret HTTP header lines, URLs with credentials or
/// secret parameters, parameter lists with secret keys, credential-shaped values, and
/// credentials in a known format anywhere. Repeated until nothing changes, so one redaction
/// never enables another on a second pass; text that keeps changing is redacted entirely.
fn redact_text(value: &str) -> Option<String> {
    let mut current = redact_text_once(value).filter(|new| new != value)?;
    for _ in 1..MAX_TEXT_PASSES {
        match redact_text_once(&current) {
            Some(next) if next != current => current = next,
            _ => return Some(current),
        }
    }
    Some(REDACTED.to_string())
}

fn redact_text_once(value: &str) -> Option<String> {
    let step = redact_header_line(value)
        .or_else(|| redact_url(value))
        .or_else(|| {
            if value.contains("://") {
                None
            } else {
                redact_parameters(value)
            }
        })
        .or_else(|| {
            (value != REDACTED && looks_like_credential(value)).then(|| REDACTED.to_string())
        });
    let current = step.as_deref().unwrap_or(value);
    scrub_known_credentials(current).or(step)
}

/// `key=value` pairs separated by `&` or `;` — nested assignments (`password=…` as the value
/// of `--from-literal=`), form data (`grant_type=x&client_secret=…`), connection strings
/// (`Server=db;User Id=sa;Password=…`), cookies (`theme=dark; session=…`): values of secret
/// keys and credential-shaped values are redacted; separators and spacing are kept.
fn redact_parameters(value: &str) -> Option<String> {
    if !value.contains('=') {
        return None;
    }
    let mut out = String::with_capacity(value.len());
    let mut changed = false;
    for piece in value.split_inclusive(['&', ';']) {
        let (body, separator) = match piece.strip_suffix(['&', ';']) {
            Some(body) => (body, &piece[body.len()..]),
            None => (piece, ""),
        };
        match body.split_once('=') {
            Some((key, secret))
                if !secret.is_empty()
                    && secret != REDACTED
                    && (is_secret_name(key.trim(), NameKind::Parameter)
                        || is_known_credential_format(secret)) =>
            {
                out.push_str(key);
                out.push('=');
                out.push_str(REDACTED);
                changed = true;
            }
            _ => out.push_str(body),
        }
        out.push_str(separator);
    }
    changed.then_some(out)
}

/// `Authorization: Bearer abc` → `Authorization: Bearer <redacted>`; `X-Api-Key:abc` →
/// `X-Api-Key:<redacted>`. The original spacing is kept.
fn redact_header_line(value: &str) -> Option<String> {
    let colon = value.find(':')?;
    if !is_secret_header(&value[..colon]) {
        return None;
    }
    let after = &value[colon + 1..];
    let content = after.trim();
    if content.is_empty() {
        return None;
    }
    let prefix = &value[..colon + 1 + (after.len() - after.trim_start().len())];
    let redacted = match content.split_once(char::is_whitespace) {
        Some((scheme, credential))
            if is_header_auth_scheme(scheme) && !credential.trim().is_empty() =>
        {
            format!("{scheme} {REDACTED}")
        }
        _ if is_header_auth_scheme(content) => return None,
        _ => REDACTED.to_string(),
    };
    Some(format!("{prefix}{redacted}"))
}

/// [`redact_url_once`] until nothing changes (removing an unparseable part can leave a URL
/// that parses and is then normalized); a value that keeps changing is redacted entirely.
fn redact_url(value: &str) -> Option<String> {
    let mut current = redact_url_once(value)?;
    for _ in 1..MAX_URL_PASSES {
        match redact_url_once(&current) {
            None => return Some(current),
            Some(next) => current = next,
        }
    }
    Some(REDACTED.to_string())
}

fn redact_url_once(value: &str) -> Option<String> {
    let scheme = url_scheme(value)?.to_ascii_lowercase();
    if WEB_SCHEMES.contains(&scheme.as_str()) {
        return match check_url(value, WEB_SCHEMES) {
            UrlCheck::Clean(_) => None,
            UrlCheck::Sanitized(url) => Some(url.into()),
            UrlCheck::Rejected(Rejection::CredentialInPath) => Some(REDACTED.to_string()),
            UrlCheck::Rejected(_) => redact_generic_url(value),
        };
    }
    redact_generic_url(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(input: &str, expected: &str) {
        let once = redact_command(input);
        assert_eq!(once, expected, "redacting {input:?}");
        assert_eq!(redact_command(&once), once, "redaction must be idempotent");
    }

    #[test]
    fn ordinary_commands_are_untouched() {
        for cmd in [
            "npm run dev",
            "cd ~/code/reframe && cargo watch -x test",
            "source .venv/bin/activate; python manage.py runserver 0.0.0.0:8000",
            "ssh -i ~/.ssh/id_ed25519 deploy@example.com",
            "git log --oneline | head -n 20",
            "docker compose -f docker-compose.dev.yml up --build",
            "Set-Location C:\\Users\\alice\\code; code .",
            "echo \"hello world\" > notes.txt",
            "mysql -u root -p shop",
            "tail -f /var/log/syslog",
            "export PATH=\"$HOME/.cargo/bin:$PATH\"",
            "export EDITOR=vim",
            "",
            "   ",
        ] {
            check(cmd, cmd);
        }
    }

    #[test]
    fn redacts_secret_assignments() {
        check(
            "export GITHUB_TOKEN=ghp_0123456789abcdefghijABCDEFGHIJ012345",
            "export GITHUB_TOKEN=\"<redacted>\"",
        );
        check(
            "export GITHUB_TOKEN=abc && npm start",
            "export GITHUB_TOKEN=\"<redacted>\" && npm start",
        );
        check(
            "export API_KEY='s3cr3t value'; code .",
            "export API_KEY='<redacted>'; code .",
        );
        check(
            "DB_PASSWORD=hunter2 docker compose up",
            "DB_PASSWORD=\"<redacted>\" docker compose up",
        );
        check(
            "set GITHUB_TOKEN=abc && code .",
            "set GITHUB_TOKEN=\"<redacted>\" && code .",
        );
        check("set \"API_KEY=abc\"", "set \"API_KEY=<redacted>\"");
        check(
            "setx GITHUB_TOKEN abc123 /M",
            "setx GITHUB_TOKEN \"<redacted>\" /M",
        );
        check(
            "set -gx OPENAI_API_KEY abc; fish",
            "set -gx OPENAI_API_KEY \"<redacted>\"; fish",
        );
        check(
            "$env:GITHUB_TOKEN = \"abc\"; code .",
            "$env:GITHUB_TOKEN = \"<redacted>\"; code .",
        );
        check(
            "$env:AWS_SECRET_ACCESS_KEY=\"abc\"",
            "$env:AWS_SECRET_ACCESS_KEY=\"<redacted>\"",
        );
        check(
            "docker run -e GITHUB_TOKEN=abc -e MODE=dev app",
            "docker run -e GITHUB_TOKEN=\"<redacted>\" -e MODE=dev app",
        );
    }

    #[test]
    fn keeps_references_to_secrets() {
        for cmd in [
            "export GITHUB_TOKEN=$(gh auth token)",
            "export GITHUB_TOKEN=\"$(pass show github)\"",
            "export TOKEN=$OTHER_TOKEN",
            "set TOKEN=%OTHER_TOKEN%",
            "mytool --token $TOKEN",
            "mytool --key ~/.ssh/id_ed25519",
            "mytool --private-key=/etc/ssl/private/key.pem",
        ] {
            check(cmd, cmd);
        }
    }

    #[test]
    fn redacts_secret_options() {
        check(
            "mytool --token abc --verbose",
            "mytool --token \"<redacted>\" --verbose",
        );
        check(
            "mytool --api-key=abc --env=prod",
            "mytool --api-key=\"<redacted>\" --env=prod",
        );
        check(
            "java -Djavax.net.ssl.keyStorePassword=changeit -jar app.jar",
            "java -Djavax.net.ssl.keyStorePassword=\"<redacted>\" -jar app.jar",
        );
        check(
            "plink -ssh host -pw hunter2",
            "plink -ssh host -pw \"<redacted>\"",
        );
        check(
            "net use Z: \\\\server\\share /password:hunter2",
            "net use Z: \\\\server\\share /password:\"<redacted>\"",
        );
        // A secret option followed by another option has no value to redact.
        check("mytool --password --verbose", "mytool --password --verbose");
        check("app --password-store=basic", "app --password-store=basic");
    }

    #[test]
    fn redacts_known_short_password_options() {
        check(
            "mysql -u root -phunter2 shop",
            "mysql -u root -p\"<redacted>\" shop",
        );
        check("sudo mysql -phunter2", "sudo mysql -p\"<redacted>\"");
        check(
            "sshpass -p hunter2 ssh host",
            "sshpass -p \"<redacted>\" ssh host",
        );
        check(
            "docker login -u me -p hunter2 registry.example.com",
            "docker login -u me -p \"<redacted>\" registry.example.com",
        );
        check(
            "curl -u alice:hunter2 https://example.com",
            "curl -u \"alice:<redacted>\" https://example.com",
        );
        // `-p` means something else for other tools.
        check("ssh -p 2222 host", "ssh -p 2222 host");
    }

    #[test]
    fn redacts_headers_and_auth_schemes() {
        check(
            "curl -H \"Authorization: Bearer abc.def\" https://api.example.com/v1",
            "curl -H \"Authorization: Bearer <redacted>\" https://api.example.com/v1",
        );
        check(
            "curl -H 'X-Api-Key: abc' https://api.example.com",
            "curl -H 'X-Api-Key: <redacted>' https://api.example.com",
        );
        check(
            "curl --header=\"Cookie: session=abc\" https://example.com",
            "curl --header=\"Cookie: <redacted>\" https://example.com",
        );
        // Partially quoted words are re-quoted as a whole (the shell sees the same argument).
        check(
            "http GET example.com Authorization:\"Bearer abc\"",
            "http GET example.com \"Authorization:Bearer <redacted>\"",
        );
        check(
            "tool --auth-header Bearer abc",
            "tool --auth-header Bearer \"<redacted>\"",
        );
    }

    #[test]
    fn redacts_url_credentials() {
        check(
            "git clone https://alice:ghp_abc@github.com/org/repo.git",
            "git clone https://github.com/org/repo.git",
        );
        check(
            "psql postgres://app:secret@localhost:5432/shop",
            "psql postgres://app@localhost:5432/shop",
        );
        check(
            "curl \"https://api.example.com/items?api_key=abc&page=2\"",
            "curl \"https://api.example.com/items?page=2\"",
        );
        check(
            "redis-cli -u redis://:secret@cache:6379/0",
            "redis-cli -u redis://cache:6379/0",
        );
        check(
            "git clone ssh://git@github.com/org/repo.git",
            "git clone ssh://git@github.com/org/repo.git",
        );
    }

    #[test]
    fn redacts_credential_shaped_values() {
        check(
            "echo ghp_0123456789abcdefghijABCDEFGHIJ012345 | gh auth login --with-token",
            "echo \"<redacted>\" | gh auth login --with-token",
        );
        check(
            "deploy --to prod q7XvT2mR9kLp4ZsW8nBc3HdF6jGy1KtN",
            "deploy --to prod \"<redacted>\"",
        );
    }

    #[test]
    fn redacts_password_options_of_more_tools() {
        for (input, expected) in [
            (
                "curl -ualice:hunter2 https://x",
                "curl -u\"alice:<redacted>\" https://x",
            ),
            (
                "curl --user=alice:hunter2 https://x",
                "curl --user=\"alice:<redacted>\" https://x",
            ),
            (
                "curl -U proxy:pw https://x",
                "curl -U \"proxy:<redacted>\" https://x",
            ),
            (
                "smbclient //srv/share -U alice%hunter2",
                "smbclient //srv/share -U \"alice%<redacted>\"",
            ),
            (
                "smbclient //srv/share -Ualice%hunter2",
                "smbclient //srv/share -U\"alice%<redacted>\"",
            ),
            (
                "xfreerdp /v:host /u:alice /p:hunter2",
                "xfreerdp /v:host /u:alice /p:\"<redacted>\"",
            ),
            (
                "docker login -phunter2 registry",
                "docker login -p\"<redacted>\" registry",
            ),
            (
                "az login -u alice -p hunter2",
                "az login -u alice -p \"<redacted>\"",
            ),
            (
                "sqlcmd -S srv -U sa -P hunter2",
                "sqlcmd -S srv -U sa -P \"<redacted>\"",
            ),
            (
                "mongosh -u admin -p hunter2",
                "mongosh -u admin -p \"<redacted>\"",
            ),
            (
                "redis-cli -a hunter2 ping",
                "redis-cli -a \"<redacted>\" ping",
            ),
            (
                "7z x secret.7z -phunter2",
                "7z x secret.7z -p\"<redacted>\"",
            ),
            (
                "zip -P hunter2 out.zip f",
                "zip -P \"<redacted>\" out.zip f",
            ),
            (
                "ssh-keygen -p -P old -N new -f key",
                "ssh-keygen -p -P \"<redacted>\" -N \"<redacted>\" -f key",
            ),
            (
                "openssl rsa -passin pass:hunter2 -in key.pem",
                "openssl rsa -passin \"<redacted>\" -in key.pem",
            ),
            ("export DB_PASS=hunter2", "export DB_PASS=\"<redacted>\""),
            (
                "SSHPASS=hunter2 sshpass -e ssh host",
                "SSHPASS=\"<redacted>\" sshpass -e ssh host",
            ),
            (
                "export GiThUb_ToKeN=abc",
                "export GiThUb_ToKeN=\"<redacted>\"",
            ),
        ] {
            check(input, expected);
        }
        // Short options mean other things for other tools, and `-p` alone prompts.
        for cmd in [
            "mysql -u root -p shop",
            "7z x archive.7z -o/tmp",
            "docker run --user 1000:1000 -p 8080:80 nginx",
            "curl -u alice https://x",
        ] {
            check(cmd, cmd);
        }
    }

    #[test]
    fn redacts_configuration_commands() {
        for (input, expected) in [
            (
                "npm config set //registry.npmjs.org/:_authToken abc123",
                "npm config set //registry.npmjs.org/:_authToken \"<redacted>\"",
            ),
            (
                "npm config set //registry.npmjs.org/:_authToken=abc123",
                "npm config set \"//registry.npmjs.org/:_authToken=<redacted>\"",
            ),
            (
                "yarn config set npmAuthToken abc123",
                "yarn config set npmAuthToken \"<redacted>\"",
            ),
            (
                "git config --global user.password hunter2",
                "git config --global user.password \"<redacted>\"",
            ),
            (
                "aws configure set aws_secret_access_key abc123",
                "aws configure set aws_secret_access_key \"<redacted>\"",
            ),
            (
                "gh secret set API_TOKEN --body abc123",
                "gh secret set API_TOKEN --body \"<redacted>\"",
            ),
            (
                "helm install x --set auth.password=hunter2",
                "helm install x --set auth.password=\"<redacted>\"",
            ),
            (
                "kubectl create secret generic x --from-literal=password=hunter2",
                "kubectl create secret generic x --from-literal=\"password=<redacted>\"",
            ),
            (
                "java -Dspring.datasource.password=hunter2 -jar app.jar",
                "java -Dspring.datasource.password=\"<redacted>\" -jar app.jar",
            ),
        ] {
            check(input, expected);
        }
        for cmd in [
            "aws configure set region us-east-1",
            "git config --global user.name Alice",
            "grep -r password src/",
            "set PATH=C:\\bin;%PATH%",
        ] {
            check(cmd, cmd);
        }
    }

    #[test]
    fn redacts_parameter_lists() {
        check(
            "dotnet ef database update --connection \"Server=.;User Id=sa;Password=hunter2\"",
            "dotnet ef database update --connection \"Server=.;User Id=sa;Password=<redacted>\"",
        );
        check(
            "curl -d 'grant_type=x&client_secret=abc' https://x",
            "curl -d 'grant_type=x&client_secret=<redacted>' https://x",
        );
        check(
            "curl -b \"theme=dark; session=abc\" https://x",
            "curl -b \"theme=dark; session=<redacted>\" https://x",
        );
        check(
            "az storage blob list --connection-string \"AccountName=a;AccountKey=abc==\"",
            "az storage blob list --connection-string \"AccountName=a;AccountKey=<redacted>\"",
        );
    }

    #[test]
    fn powershell_literals_are_redacted_and_commands_kept() {
        for (input, expected) in [
            (
                "$pw = ConvertTo-SecureString \"hunter2\" -AsPlainText -Force",
                "$pw = ConvertTo-SecureString \"<redacted>\" -AsPlainText -Force",
            ),
            (
                "ConvertTo-SecureString -String 'hunter2' -AsPlainText -Force",
                "ConvertTo-SecureString -String '<redacted>' -AsPlainText -Force",
            ),
            (
                "$c = New-Object PSCredential('a', (ConvertTo-SecureString 'pw' -AsPlainText -Force))",
                "$c = New-Object PSCredential('a', (ConvertTo-SecureString '<redacted>' -AsPlainText -Force))",
            ),
            ("$pw= \"hunter2\"", "$pw= \"<redacted>\""),
            ("$pw =\"hunter2\"", "$pw =\"<redacted>\""),
            ("$pw=hunter2", "$pw=\"<redacted>\""),
        ] {
            check(input, expected);
        }
        for cmd in [
            "$pw = Read-Host -AsSecureString",
            "$pw=Read-Host",
            "$env:API_KEY = Get-Content key.txt",
            "Connect-Thing -Password (Get-Secret name)",
        ] {
            check(cmd, cmd);
        }
    }

    #[test]
    fn quotes_inside_names_do_not_hide_assignments() {
        check("tool --to\"ken\"=abc", "tool \"--token=<redacted>\"");
        check("tool --token'=abc'", "tool \"--token=<redacted>\"");
        check("TO\"KEN\"=abc cmd", "\"TOKEN=<redacted>\" cmd");
        check(
            "ConvertTo-SecureString --token'=$ghp_0123456789abcdefghijABCDEFGHIJ012345'",
            "ConvertTo-SecureString \"--token=<redacted>\"",
        );
        // Odd quoting hides whether the value is a reference: it is redacted.
        check("tool --token'=$TOKEN'", "tool \"--token=<redacted>\"");
        check("tool --to\"ken\"=$TOKEN", "tool \"--token=<redacted>\"");
        // Replacements built from partly quoted words keep their word boundaries.
        check("curl http://u:p@h'/a b'", "curl http://h/a%20b");
        check("psql postgres://u:p@h'/a b'", "psql \"postgres://u@h/a b\"");
        check("x --password='a b'c", "x --password=\"<redacted>\"");
        // Credentials in a known format are removed wherever they are.
        check(
            "x --header=Authorization:ghp_0123456789abcdefghijABCDEFGHIJ012345",
            "x --header=\"Authorization:<redacted>\"",
        );
        check(
            "echo \"key is ghp_0123456789abcdefghijABCDEFGHIJ012345 ok\"",
            "echo \"key is <redacted> ok\"",
        );
        let fuzz_two = "&state=//`Read-HostPassword=$'\t;Authorization:curl \
                        ghp_0123456789abcdefghijABCDEFGHIJ012345&state=pass: x";
        let once = redact_command(fuzz_two);
        assert!(!once.contains("ghp_"), "{once}");
        assert_eq!(redact_command(&once), once);
        // A URL ending in `Authorization:` is not a header.
        check(
            "docker login ftp:///net/Authorization:#access_token=x \té",
            "docker login ftp://net/Authorization: \té",
        );
        let fuzz_three = "'TOKEN=#$'<\"=x%T%Password=$'+e `C:\\ C:\\s:;[\t\\\\srv;;jsessionid=C:\\d \
                          login\"=-\n--token,";
        let once = redact_command(fuzz_three);
        assert_eq!(redact_command(&once), once);
        // Found by fuzzing (the result must be stable).
        let fuzz = "&ftp:////curl file:///=x~&state=http://u:p@/curl ConvertTo-SecureString \
                    --token'=$ghp_0123456789abcdefghijABCDEFGHIJ012345C:\\\\%C3x 9--password";
        let once = redact_command(fuzz);
        assert!(!once.contains("ghp_"), "{once}");
        assert_eq!(redact_command(&once), once);
    }

    #[test]
    fn ansi_c_strings_are_literals_not_references() {
        check("export TOKEN=$'hunter2'", "export TOKEN=\"<redacted>\"");
        check("export TOKEN=$\"hunter2\"", "export TOKEN=\"<redacted>\"");
        check("set TOKEN=%OTHER%", "set TOKEN=%OTHER%");
    }

    #[test]
    fn unbalanced_quotes_and_unicode_never_panic() {
        for cmd in [
            "export TOKEN=\"unterminated",
            "echo 'unterminated",
            "echo $(unterminated",
            "echo ${",
            "--token",
            "export TOKEN=",
            "café --password ünïcödé && échö",
            "\\",
            "\"",
            "=",
            "$env:",
            "a=b=c",
        ] {
            let _ = redact_command(cmd);
        }
        assert_eq!(
            redact_command("café --password ünïcödé"),
            "café --password \"<redacted>\""
        );
    }
}
