//! Best-effort parsing of VS Code window titles.
//!
//! The default `window.title` is
//! `${dirty}${activeEditorShort}${separator}${rootName}${separator}${profileName}${separator}${appName}`:
//! `${separator}` is `window.titleSeparator` (" - " on Windows/Linux, " — " on macOS), `${dirty}`
//! is "● ", empty variables disappear together with their separator, multi-root workspaces
//! show as `<name> (Workspace)` and remote windows append the remote to the root name
//! ("reframe [SSH: build-box]"). VS Code itself may prefix "[Extension Development Host]" and
//! append "[Administrator]" / "[Superuser]" / "[Unsupported]". On macOS the default omits
//! `${dirty}` and `${appName}` ("main.rs — reframe"); a window whose variables are all empty
//! shows just the product name.
//!
//! Users can customize the title, so parsing never fails. It yields candidate root names,
//! most likely first, which the resolver only accepts when they match a folder or workspace
//! VS Code actually opened.

const DIRTY_MARKER: char = '\u{25CF}';
const EXTENSION_HOST: &str = "[Extension Development Host]";
/// Separators tried when the application name is not part of the title.
const FALLBACK_SEPARATORS: [&str; 5] = [" \u{2014} ", " - ", " \u{2013} ", " | ", " \u{00B7} "];
const TRAILING_DECORATIONS: [&str; 4] = [
    "[Administrator]",
    "[Superuser]",
    "[Unsupported]",
    "[Restricted Mode]",
];
/// Kinds of remote / virtual workspaces shown as `[<kind>: <name>]` or `[<kind>]`.
const REMOTE_KINDS: [&str; 13] = [
    "SSH",
    "WSL",
    "Dev Container",
    "Codespaces",
    "Container",
    "Attached Container",
    "Tunnel",
    "Remote",
    "Kubernetes",
    "Docker",
    "GitHub",
    "Azure Repos",
    "Remote Repositories",
];
const WORKSPACE_SUFFIX: &str = " (Workspace)";
const UNTITLED_WORKSPACE: &str = "Untitled";
/// Titles with more segments than this are truncated (only the right-most ones matter).
const MAX_SEGMENTS: usize = 8;

/// Something in a title that may identify the window's folder or workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootCandidate {
    /// A folder or workspace name; `workspace` when the title marks it as a workspace.
    Name { name: String, workspace: bool },
    /// An absolute or `~/` path (custom titles using `${rootPath}`, `${activeEditorLong}`...).
    Path(String),
}

/// What a window title says about the window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TitleInfo {
    /// Most likely first.
    pub candidates: Vec<RootCandidate>,
    /// The active editor has unsaved changes ("● ").
    pub dirty: bool,
    /// "[Extension Development Host]" window started by an extension debug session.
    pub extension_host: bool,
    /// Remote indicator such as "SSH: build-box" or "WSL: Ubuntu".
    pub remote: Option<String>,
    /// "Untitled (Workspace)": an unsaved multi-root workspace.
    pub untitled_workspace: bool,
    /// The title is only the application name: a window without folder or editor.
    pub empty_window: bool,
}

/// Parse `title`. `app_names` are the possible `${appName}` values (the window's own product
/// first); `profile_names` are the user's VS Code profile names (`${profileName}`).
pub fn parse(title: &str, app_names: &[&str], profile_names: &[String]) -> TitleInfo {
    let mut info = TitleInfo::default();
    let mut rest = title.trim();

    // Leading decorations: "[Extension Development Host]", "● ", "[SSH: host]".
    loop {
        let before = rest;
        if let Some(r) = rest.strip_prefix(EXTENSION_HOST) {
            info.extension_host = true;
            rest = trim_separator_start(r);
        }
        if let Some(r) = rest.strip_prefix(DIRTY_MARKER) {
            info.dirty = true;
            rest = r.trim_start();
        }
        if let Some((label, r)) = leading_bracket(rest)
            && is_remote_label(label)
        {
            info.remote.get_or_insert_with(|| label.trim().to_string());
            rest = trim_separator_start(r);
        }
        if rest.len() == before.len() {
            break;
        }
    }
    // Trailing decorations: "[Administrator]", "[Unsupported]"...
    loop {
        let before = rest.len();
        for decoration in TRAILING_DECORATIONS {
            if let Some(r) = rest.strip_suffix(decoration) {
                rest = r.trim_end();
            }
        }
        if rest.len() == before {
            break;
        }
    }
    if rest.is_empty() {
        return info;
    }

    // `${appName}`: normally last, sometimes first in custom titles.
    let mut names: Vec<&str> = app_names
        .iter()
        .map(|n| n.trim())
        .filter(|n| !n.is_empty())
        .collect();
    names.sort_by_key(|n| std::cmp::Reverse(n.len()));
    if names.contains(&rest) {
        info.empty_window = true;
        return info;
    }
    let mut separator: Option<&str> = None;
    for name in &names {
        if let Some(r) = rest.strip_suffix(name)
            && let Some(sep) = trailing_separator(r)
        {
            rest = &r[..r.len() - sep.len()];
            separator = Some(sep);
            break;
        }
        if let Some(r) = rest.strip_prefix(name)
            && let Some(sep) = leading_separator(r)
        {
            rest = &r[sep.len()..];
            separator = Some(sep);
            break;
        }
    }
    let separator =
        separator.or_else(|| FALLBACK_SEPARATORS.into_iter().find(|s| rest.contains(s)));

    let mut segments: Vec<&str> = match separator {
        Some(sep) => rest.split(sep).collect(),
        None => vec![rest],
    };
    for segment in &mut segments {
        *segment = segment.trim();
        if let Some(r) = segment.strip_prefix(DIRTY_MARKER) {
            info.dirty = true;
            *segment = r.trim_start();
        }
        // Remote windows append the remote to the root name: "reframe [SSH: build-box]".
        if let Some((base, label)) = trailing_bracket(segment)
            && is_remote_label(label)
        {
            info.remote.get_or_insert_with(|| label.trim().to_string());
            *segment = base;
        }
    }
    segments.retain(|s| !s.is_empty());
    if segments.len() > MAX_SEGMENTS {
        segments.drain(..segments.len() - MAX_SEGMENTS);
    }
    // `${profileName}` sits right before `${appName}` for non-default profiles.
    if segments.len() > 1
        && let Some(last) = segments.last()
        && profile_names.iter().any(|p| p.trim() == *last)
    {
        segments.pop();
    }

    // Single segments right to left (`${rootName}` is the last one in the default title),
    // then runs of adjacent segments for names that contain the separator.
    let mut candidates = Vec::new();
    for segment in segments.iter().rev() {
        push_candidates(&mut info, &mut candidates, segment);
    }
    if let Some(sep) = separator {
        for end in (1..segments.len()).rev() {
            for start in (0..end).rev() {
                push_candidates(&mut info, &mut candidates, &segments[start..=end].join(sep));
            }
        }
    }
    let mut unique: Vec<RootCandidate> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if !unique.contains(&candidate) {
            unique.push(candidate);
        }
    }
    info.candidates = unique;
    info
}

fn push_candidates(info: &mut TitleInfo, out: &mut Vec<RootCandidate>, segment: &str) {
    let segment = segment.trim();
    if segment.is_empty() {
        return;
    }
    if looks_like_path(segment) {
        out.push(RootCandidate::Path(segment.to_string()));
        return;
    }
    if let Some(name) = segment.strip_suffix(WORKSPACE_SUFFIX) {
        let name = name.trim();
        if name == UNTITLED_WORKSPACE {
            info.untitled_workspace = true;
        } else if !name.is_empty() {
            out.push(RootCandidate::Name {
                name: name.to_string(),
                workspace: true,
            });
        }
        return;
    }
    out.push(RootCandidate::Name {
        name: segment.to_string(),
        workspace: false,
    });
    // Localized VS Code labels workspaces "<name> (<translated word>)".
    if let Some(base) = paren_suffix_base(segment) {
        out.push(RootCandidate::Name {
            name: base.to_string(),
            workspace: true,
        });
    }
}

fn looks_like_path(s: &str) -> bool {
    let b = s.as_bytes();
    s.starts_with('/')
        || s == "~"
        || s.starts_with("~/")
        || s.starts_with(r"~\")
        || s.starts_with(r"\\")
        || (b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && (b[2] == b'\\' || b[2] == b'/'))
}

/// "name (label)" → "name".
fn paren_suffix_base(s: &str) -> Option<&str> {
    let inner = s.strip_suffix(')')?;
    let open = inner.rfind(" (")?;
    let label = &inner[open + 2..];
    let base = inner[..open].trim();
    (!base.is_empty() && !label.is_empty() && !label.contains('(')).then_some(base)
}

fn leading_bracket(s: &str) -> Option<(&str, &str)> {
    let inner = s.strip_prefix('[')?;
    let close = inner.find(']')?;
    Some((&inner[..close], &inner[close + 1..]))
}

/// "base [label]" → ("base", "label").
fn trailing_bracket(s: &str) -> Option<(&str, &str)> {
    let inner = s.strip_suffix(']')?;
    let open = inner.rfind('[')?;
    Some((inner[..open].trim_end(), &inner[open + 1..]))
}

fn is_remote_label(label: &str) -> bool {
    let label = label.trim();
    let kind = label.split_once(':').map_or(label, |(kind, _)| kind).trim();
    if REMOTE_KINDS.iter().any(|k| k.eq_ignore_ascii_case(kind)) {
        return true;
    }
    // Remote extensions Reframe doesn't know use the same "<Kind>: <name>" shape.
    match label.split_once(": ") {
        Some((kind, name)) => {
            !kind.is_empty()
                && kind.len() <= 32
                && !name.trim().is_empty()
                && kind
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == ' ' || c == '-')
        }
        None => false,
    }
}

fn is_separator_char(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '-' | '\u{2014}'
                | '\u{2013}'
                | '|'
                | '\u{00B7}'
                | '\u{2022}'
                | ':'
                | '~'
                | '*'
                | '+'
                | '='
                | '>'
                | '\u{00BB}'
                | '/'
                | '\\'
                | ','
                | ';'
        )
}

/// The separator at the end of `s` (" - ", " — ", " :: "...): a short run of separator
/// characters that starts with whitespace and contains something else.
fn trailing_separator(s: &str) -> Option<&str> {
    let mut start = s.len();
    for (count, (i, c)) in s.char_indices().rev().enumerate() {
        if count >= 8 || !is_separator_char(c) {
            break;
        }
        start = i;
    }
    let run = &s[start..];
    let run = &run[run.find(char::is_whitespace)?..];
    (!run.trim().is_empty() && run.len() < s.len()).then_some(run)
}

/// The separator at the start of `s`: a short run that ends with whitespace.
fn leading_separator(s: &str) -> Option<&str> {
    let mut end = 0;
    for (count, (i, c)) in s.char_indices().enumerate() {
        if count >= 8 || !is_separator_char(c) {
            break;
        }
        end = i + c.len_utf8();
    }
    let run = &s[..end];
    let (last_space, space) = run.char_indices().rev().find(|(_, c)| c.is_whitespace())?;
    let run = &run[..last_space + space.len_utf8()];
    (!run.trim().is_empty() && run.len() < s.len()).then_some(run)
}

/// Remove whitespace and a leading separator ("- ", "— ") followed by whitespace.
fn trim_separator_start(s: &str) -> &str {
    let s = s.trim_start();
    let punctuation_end = s
        .char_indices()
        .find(|&(_, c)| c.is_whitespace() || !is_separator_char(c))
        .map_or(s.len(), |(i, _)| i);
    let after = &s[punctuation_end..];
    if punctuation_end > 0 && after.starts_with(char::is_whitespace) {
        after.trim_start()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const APPS: [&str; 3] = [
        "Visual Studio Code",
        "Visual Studio Code - Insiders",
        "VSCodium",
    ];

    fn name(n: &str) -> RootCandidate {
        RootCandidate::Name {
            name: n.into(),
            workspace: false,
        }
    }

    fn workspace(n: &str) -> RootCandidate {
        RootCandidate::Name {
            name: n.into(),
            workspace: true,
        }
    }

    fn first(title: &str) -> Option<RootCandidate> {
        parse(title, &APPS, &[]).candidates.into_iter().next()
    }

    #[test]
    fn default_titles_with_both_separators() {
        let info = parse("main.rs - reframe - Visual Studio Code", &APPS, &[]);
        assert_eq!(
            info.candidates,
            vec![name("reframe"), name("main.rs"), name("main.rs - reframe")]
        );
        assert!(!info.dirty && !info.empty_window && info.remote.is_none());
        assert_eq!(
            first("main.rs \u{2014} reframe \u{2014} Visual Studio Code"),
            Some(name("reframe"))
        );
        assert_eq!(first("reframe - Visual Studio Code"), Some(name("reframe")));
        assert_eq!(first("main.rs - reframe - VSCodium"), Some(name("reframe")));
        // The Insiders product name itself contains " - ".
        assert_eq!(
            first("main.rs - reframe - Visual Studio Code - Insiders"),
            Some(name("reframe"))
        );
        assert_eq!(
            first("main.rs \u{2014} reframe \u{2014} Visual Studio Code - Insiders"),
            Some(name("reframe"))
        );
    }

    #[test]
    fn macos_default_titles_have_no_app_name() {
        assert_eq!(first("main.rs \u{2014} reframe"), Some(name("reframe")));
        assert_eq!(first("reframe"), Some(name("reframe")));
        let info = parse(
            "main.rs \u{2014} reframe \u{2014} Work",
            &APPS,
            &["Work".to_string()],
        );
        assert_eq!(info.candidates[0], name("reframe"));
        let info = parse("main.rs \u{2014} platform (Workspace)", &APPS, &[]);
        assert_eq!(info.candidates[0], workspace("platform"));
    }

    #[test]
    fn dirty_marker_and_decorations() {
        let info = parse(
            "\u{25CF} main.rs - reframe - Visual Studio Code",
            &APPS,
            &[],
        );
        assert!(info.dirty);
        assert_eq!(info.candidates[0], name("reframe"));
        let info = parse(
            "main.rs - reframe - Visual Studio Code [Administrator]",
            &APPS,
            &[],
        );
        assert_eq!(info.candidates[0], name("reframe"));
        assert_eq!(
            first("reframe - Visual Studio Code [Superuser] [Unsupported]"),
            Some(name("reframe"))
        );
    }

    #[test]
    fn workspaces_and_untitled_workspaces() {
        let info = parse(
            "main.rs - platform (Workspace) - Visual Studio Code",
            &APPS,
            &[],
        );
        assert_eq!(info.candidates[0], workspace("platform"));
        assert!(!info.untitled_workspace);
        let info = parse("Untitled (Workspace) - Visual Studio Code", &APPS, &[]);
        assert!(info.untitled_workspace);
        assert!(info.candidates.is_empty());
        // Localized label: try the folder name first, then the workspace name.
        let info = parse(
            "plattform (Arbeitsbereich) - Visual Studio Code",
            &APPS,
            &[],
        );
        assert_eq!(
            info.candidates,
            vec![name("plattform (Arbeitsbereich)"), workspace("plattform")]
        );
    }

    #[test]
    fn remote_windows_are_flagged() {
        let info = parse(
            "main.rs - reframe [SSH: build-box] - Visual Studio Code",
            &APPS,
            &[],
        );
        assert_eq!(info.remote.as_deref(), Some("SSH: build-box"));
        assert_eq!(info.candidates[0], name("reframe"));
        let info = parse(
            "[WSL: Ubuntu] main.rs - reframe - Visual Studio Code",
            &APPS,
            &[],
        );
        assert_eq!(info.remote.as_deref(), Some("WSL: Ubuntu"));
        let info = parse(
            "app [Dev Container: Rust @ desktop-linux] - Visual Studio Code",
            &APPS,
            &[],
        );
        assert_eq!(
            info.remote.as_deref(),
            Some("Dev Container: Rust @ desktop-linux")
        );
        let info = parse("vscode [GitHub] - Visual Studio Code", &APPS, &[]);
        assert_eq!(info.remote.as_deref(), Some("GitHub"));
        let info = parse("srv [Coder: dev-box] - Visual Studio Code", &APPS, &[]);
        assert_eq!(info.remote.as_deref(), Some("Coder: dev-box"));
        // An ordinary bracket in a folder name is not a remote.
        let info = parse("notes [old] - Visual Studio Code", &APPS, &[]);
        assert_eq!(info.remote, None);
        assert_eq!(info.candidates[0], name("notes [old]"));
    }

    #[test]
    fn extension_development_host() {
        let info = parse(
            "[Extension Development Host] main.rs - sample - Visual Studio Code",
            &APPS,
            &[],
        );
        assert!(info.extension_host);
        assert_eq!(info.candidates[0], name("sample"));
        let info = parse(
            "[Extension Development Host] - \u{25CF} index.ts - sample - Visual Studio Code",
            &APPS,
            &[],
        );
        assert!(info.extension_host && info.dirty);
        assert_eq!(info.candidates[0], name("sample"));
    }

    #[test]
    fn profile_names_are_skipped() {
        let title = "main.rs - reframe - Work - Visual Studio Code";
        let info = parse(title, &APPS, &["Work".to_string()]);
        assert_eq!(info.candidates[0], name("reframe"));
        // Unknown profile: still a candidate, but the root name follows.
        let info = parse(title, &APPS, &[]);
        assert_eq!(info.candidates[0], name("Work"));
        assert_eq!(info.candidates[1], name("reframe"));
        // A lone segment equal to a profile name is kept.
        assert_eq!(
            parse("Work - Visual Studio Code", &APPS, &["Work".to_string()]).candidates,
            vec![name("Work")]
        );
    }

    #[test]
    fn empty_and_unreadable_titles() {
        let info = parse("Visual Studio Code", &APPS, &[]);
        assert!(info.empty_window && info.candidates.is_empty());
        assert!(parse("Visual Studio Code - Insiders", &APPS, &[]).empty_window);
        let info = parse("   ", &APPS, &[]);
        assert!(!info.empty_window && info.candidates.is_empty());
        assert_eq!(
            first("Untitled-1 - Visual Studio Code"),
            Some(name("Untitled-1"))
        );
    }

    #[test]
    fn custom_titles() {
        // `${rootName}` only.
        assert_eq!(first("reframe"), Some(name("reframe")));
        // App name first.
        assert_eq!(first("Visual Studio Code - reframe"), Some(name("reframe")));
        // Custom separator.
        assert_eq!(
            first("main.rs :: reframe :: Visual Studio Code"),
            Some(name("reframe"))
        );
        // Root first, no app name.
        let info = parse("reframe \u{2014} main.rs", &APPS, &[]);
        assert_eq!(info.candidates[..2], [name("main.rs"), name("reframe")]);
        // `${rootPath}`: paths are candidates as well.
        let info = parse("main.rs - ~/code/reframe - Visual Studio Code", &APPS, &[]);
        assert_eq!(
            info.candidates[0],
            RootCandidate::Path("~/code/reframe".into())
        );
        let info = parse(r"C:\src\reframe - Visual Studio Code", &APPS, &[]);
        assert_eq!(
            info.candidates[0],
            RootCandidate::Path(r"C:\src\reframe".into())
        );
        // Folder names that contain the separator.
        let info = parse("main.rs - my - project - Visual Studio Code", &APPS, &[]);
        assert!(info.candidates.contains(&name("my - project")));
        // Folder names ending with separator characters.
        assert_eq!(first("c++ - Visual Studio Code"), Some(name("c++")));
    }

    #[test]
    fn unusual_characters_never_panic() {
        // Multi-byte whitespace around separators (no-break and em spaces).
        assert_eq!(
            first("Visual Studio Code\u{00A0}-\u{00A0}reframe"),
            Some(name("reframe"))
        );
        assert_eq!(
            first(
                "main.rs\u{2003}\u{2014}\u{2003}reframe\u{2003}\u{2014}\u{2003}Visual Studio Code"
            ),
            Some(name("reframe"))
        );
        for title in [
            "\u{25CF}",
            "[",
            "]",
            "[]",
            "[: ]",
            "()",
            " (Workspace)",
            "- - -",
            "\u{2014}\u{2014}\u{2014} Visual Studio Code",
            "Visual Studio Code \u{2014}",
            "\u{1F600} \u{2014} \u{1F680} \u{2014} Visual Studio Code",
            "~",
            "C:",
            "a ] b [ c",
            "[Extension Development Host]",
        ] {
            let _ = parse(title, &APPS, &["\u{1F600}".to_string()]);
        }
    }
}
