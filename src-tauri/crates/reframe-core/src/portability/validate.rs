//! Strict validation of export documents: the file layer (size, encoding, JSON syntax, format
//! and version, NUL characters) and the content layer (every limit in [`super::limits`] and
//! the workspace model, references between entries).
//!
//! The same content validation runs on exports before they are written, so Reframe never
//! produces a file it would refuse to import.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::Path;

use reframe_platform::Rect;
use serde::Deserialize;
use serde::de::{self, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use uuid::Uuid;

use super::limits::*;
use super::paths::is_portable_absolute;
use super::sanitize::{is_allowed_web_url, sanitize_media_uri};
use crate::model::{
    AppState, EXPORT_FORMAT, EXPORT_VERSION, EntryOptions, MAX_APPLICATIONS, MAX_DESCRIPTION_LEN,
    MAX_IMPORT_BYTES, MAX_NAME_LEN, MAX_WINDOWS_PER_APP, MachineProfile, PortableApplication,
    PortableWorkspace, WindowSnapshot, WorkspaceExport,
};

/// Problems found in a document; at most [`Problems::MAX_LISTED`] are kept verbatim.
#[derive(Debug, Default)]
pub(crate) struct Problems {
    listed: Vec<String>,
    total: usize,
}

impl Problems {
    const MAX_LISTED: usize = 20;

    fn push(&mut self, problem: String) {
        self.total += 1;
        if self.listed.len() < Self::MAX_LISTED {
            self.listed.push(problem);
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// Every listed problem (for `CoreError::Validation`).
    pub(crate) fn into_list(self) -> Vec<String> {
        let mut list = self.listed;
        let unlisted = self.total.saturating_sub(list.len());
        if unlisted > 0 {
            list.push(format!("…and {unlisted} more problems"));
        }
        list
    }

    /// One sentence for `CoreError::InvalidImport`: the first few problems.
    pub(crate) fn summary(&self) -> String {
        const SHOWN: usize = 3;
        let mut text = self
            .listed
            .iter()
            .take(SHOWN)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        let more = self.total.saturating_sub(SHOWN);
        if more > 0 {
            let _ = write!(
                text,
                " (and {more} more problem{})",
                if more == 1 { "" } else { "s" }
            );
        }
        text
    }
}

// ---------------------------------------------------------------------------- file layer

/// Size, UTF-8 (a leading byte-order mark is accepted), JSON syntax, structure (nesting depth,
/// list sizes, value count, NUL characters), `format` and `version`. Returns the JSON text on
/// success.
///
/// Nothing is materialized before the structure is known to be reasonable: the syntax check
/// and the header read skip values without allocating, and the structure scan is a single
/// pass over the text. A hostile file therefore costs time linear in its size and little
/// memory; only a document that passes is deserialized.
pub(crate) fn check_file(bytes: &[u8]) -> Result<&str, String> {
    if bytes.len() > MAX_IMPORT_BYTES {
        return Err("the file is larger than 5 MB".to_string());
    }
    let text = std::str::from_utf8(bytes).map_err(|e| {
        format!(
            "the file is not UTF-8 text (invalid byte sequence at byte {})",
            e.valid_up_to()
        )
    })?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.trim().is_empty() {
        return Err("the file is empty".to_string());
    }
    serde_json::from_str::<IgnoredAny>(text)
        .map_err(|e| json_error("the JSON is malformed", &e))?;
    if !text.trim_start().starts_with('{') {
        return Err("the file does not contain a JSON object".to_string());
    }
    let header: Header =
        serde_json::from_str(text).map_err(|e| json_error("unexpected content", &e))?;
    match header.format {
        None => {
            return Err(
                "it has no \"format\" field, so it is not a Reframe workspace export".to_string(),
            );
        }
        Some(HeaderValue::Text(format)) if format == EXPORT_FORMAT => {}
        Some(HeaderValue::Text(format)) => {
            return Err(format!(
                "its format is {:?} instead of {EXPORT_FORMAT:?}",
                snippet(&format)
            ));
        }
        Some(_) => return Err("its \"format\" field is not text".to_string()),
    }
    match header.version {
        None => return Err("it has no \"version\" field".to_string()),
        Some(HeaderValue::Unsigned(0)) => {
            return Err("its format version 0 is not valid".to_string());
        }
        Some(HeaderValue::Unsigned(version)) if version > u64::from(EXPORT_VERSION) => {
            return Err(format!(
                "it was created by a newer version of Reframe (format version {version}; this \
                 version reads up to {EXPORT_VERSION}). Update Reframe to import it"
            ));
        }
        Some(HeaderValue::Unsigned(_)) => {}
        Some(_) => {
            return Err("its \"version\" field is not a positive whole number".to_string());
        }
    }
    scan_structure(text)?;
    Ok(text)
}

/// The two top-level fields read before anything else; every other field is skipped.
#[derive(Deserialize)]
struct Header {
    format: Option<HeaderValue>,
    version: Option<HeaderValue>,
}

/// A header field's value, read without materializing lists or objects.
enum HeaderValue {
    Text(String),
    Unsigned(u64),
    Other,
}

impl<'de> Deserialize<'de> for HeaderValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct HeaderVisitor;

        impl<'de> Visitor<'de> for HeaderVisitor {
            type Value = HeaderValue;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("any JSON value")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<HeaderValue, E> {
                Ok(HeaderValue::Text(v.to_string()))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<HeaderValue, E> {
                Ok(HeaderValue::Unsigned(v))
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<HeaderValue, E> {
                Ok(u64::try_from(v).map_or(HeaderValue::Other, HeaderValue::Unsigned))
            }

            fn visit_f64<E: de::Error>(self, _: f64) -> Result<HeaderValue, E> {
                Ok(HeaderValue::Other)
            }

            fn visit_bool<E: de::Error>(self, _: bool) -> Result<HeaderValue, E> {
                Ok(HeaderValue::Other)
            }

            fn visit_unit<E: de::Error>(self) -> Result<HeaderValue, E> {
                Ok(HeaderValue::Other)
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<HeaderValue, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(HeaderValue::Other)
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<HeaderValue, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(HeaderValue::Other)
            }
        }

        deserializer.deserialize_any(HeaderVisitor)
    }
}

/// One open list or object during [`scan_structure`].
struct Frame {
    is_array: bool,
    entries: usize,
    /// Byte range (inside the quotes) of the current key, for objects.
    key: Option<(usize, usize)>,
}

/// A single pass over syntactically valid JSON: nesting depth, entries per container, total
/// values, and `\u0000` escapes (the only way JSON can hold a NUL character) reported with
/// their JSON path. Robust against any input: unbalanced text is left to the parser.
fn scan_structure(text: &str) -> Result<(), String> {
    let bytes = text.as_bytes();
    let mut stack: Vec<Frame> = Vec::new();
    let mut values = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let start = i + 1;
                let mut nul = false;
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        nul |= bytes.get(i + 1..i + 6) == Some(b"u0000".as_slice());
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                let end = i.min(bytes.len());
                i += 1;
                let mut next = i;
                while next < bytes.len() && bytes[next].is_ascii_whitespace() {
                    next += 1;
                }
                let is_key = bytes.get(next) == Some(&b':');
                if is_key {
                    if let Some(frame) = stack.last_mut() {
                        frame.key = Some((start, end));
                        frame.entries += 1;
                        check_entries(frame, text, start)?;
                    }
                } else {
                    count_value(&mut stack, &mut values, text, start)?;
                }
                if nul {
                    // For a key, the frame already points at it.
                    let path = json_path(&stack, text);
                    return Err(format!(
                        "it contains a NUL character in {}",
                        if path.is_empty() {
                            "the document".to_string()
                        } else {
                            path
                        }
                    ));
                }
                continue;
            }
            open @ (b'[' | b'{') => {
                count_value(&mut stack, &mut values, text, i)?;
                if stack.len() >= MAX_JSON_DEPTH {
                    return Err(format!(
                        "its content is nested more than {MAX_JSON_DEPTH} levels deep at {}",
                        position(text, i)
                    ));
                }
                stack.push(Frame {
                    is_array: open == b'[',
                    entries: 0,
                    key: None,
                });
            }
            b']' | b'}' => {
                stack.pop();
            }
            b'-' | b'0'..=b'9' | b't' | b'f' | b'n' => {
                count_value(&mut stack, &mut values, text, i)?;
                while i < bytes.len()
                    && !matches!(bytes[i], b',' | b']' | b'}' | b':')
                    && !bytes[i].is_ascii_whitespace()
                {
                    i += 1;
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    Ok(())
}

/// Count a value starting at byte `at`; values in lists are entries of that list.
fn count_value(
    stack: &mut [Frame],
    values: &mut usize,
    text: &str,
    at: usize,
) -> Result<(), String> {
    *values += 1;
    if *values > MAX_JSON_VALUES {
        return Err(format!(
            "it contains more than {MAX_JSON_VALUES} values, far more than a workspace needs"
        ));
    }
    if let Some(frame) = stack.last_mut()
        && frame.is_array
    {
        frame.entries += 1;
        check_entries(frame, text, at)?;
    }
    Ok(())
}

fn check_entries(frame: &Frame, text: &str, at: usize) -> Result<(), String> {
    if frame.entries > MAX_JSON_ENTRIES {
        return Err(format!(
            "it contains {} with more than {MAX_JSON_ENTRIES} entries at {}",
            if frame.is_array {
                "a list"
            } else {
                "an object"
            },
            position(text, at)
        ));
    }
    Ok(())
}

/// `workspace.applications[1].displayName` for the current position of [`scan_structure`].
fn json_path(stack: &[Frame], text: &str) -> String {
    let mut path = String::new();
    for frame in stack {
        if frame.is_array {
            let _ = write!(path, "[{}]", frame.entries.saturating_sub(1));
        } else if let Some((start, end)) = frame.key {
            push_path_key(&mut path, &text[start..end]);
        }
    }
    path
}

fn push_path_key(path: &mut String, raw_key: &str) {
    if !path.is_empty() {
        path.push('.');
    }
    path.push_str(&snippet(raw_key));
}

/// `line L, column C` (1-based, columns in characters) of byte offset `at`.
fn position(text: &str, at: usize) -> String {
    let at = (0..=at.min(text.len()))
        .rev()
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(0);
    let before = &text[..at];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    format!("line {line}, column {column}")
}

/// Deserialize the typed document (positions are reported for type errors too).
pub(crate) fn deserialize(text: &str) -> Result<WorkspaceExport, String> {
    serde_json::from_str(text).map_err(|e| {
        let mut message = json_error("unexpected content", &e);
        if message.contains("unknown variant") {
            message.push_str(" (the file may come from a newer version of Reframe)");
        }
        message
    })
}

fn json_error(what: &str, error: &serde_json::Error) -> String {
    let full = error.to_string();
    let suffix = format!(" at line {} column {}", error.line(), error.column());
    let detail = full.strip_suffix(&suffix).unwrap_or(&full);
    if error.line() == 0 {
        return format!("{what}: {detail}");
    }
    if error.is_eof() {
        return format!(
            "the file ends unexpectedly at line {}, column {} (it may be truncated)",
            error.line(),
            error.column()
        );
    }
    format!(
        "{what} at line {}, column {}: {detail}",
        error.line(),
        error.column()
    )
}

/// At most 40 characters of user content for messages.
fn snippet(text: &str) -> String {
    let mut short: String = text.chars().take(40).collect();
    if text.chars().nth(40).is_some() {
        short.push('…');
    }
    short
}

// ---------------------------------------------------------------------------- content layer

/// Every content rule; see the module docs.
pub(crate) fn validate_export(export: &WorkspaceExport) -> Problems {
    let mut p = Problems::default();
    if export.format != EXPORT_FORMAT {
        p.push(format!(
            "its format is {:?} instead of {EXPORT_FORMAT:?}",
            snippet(&export.format)
        ));
    }
    if export.version == 0 || export.version > EXPORT_VERSION {
        p.push(format!(
            "format version {} is not supported",
            export.version
        ));
    }
    check_text(&mut p, "the app version", &export.app_version, MAX_TEXT_LEN);
    validate_workspace(&mut p, &export.workspace);
    validate_machine(&mut p, &export.machine, &export.workspace);
    p
}

fn validate_workspace(p: &mut Problems, ws: &PortableWorkspace) {
    let name = ws.name.trim();
    if name.is_empty() {
        p.push("the workspace name is empty".to_string());
    }
    check_text(p, "the workspace name", name, MAX_NAME_LEN);
    check_text(p, "the description", &ws.description, MAX_DESCRIPTION_LEN);
    check_text(p, "the icon", &ws.icon, MAX_ICON_LEN);
    if let Some(color) = &ws.color {
        let valid = color.len() == 7
            && color.starts_with('#')
            && color[1..].chars().all(|c| c.is_ascii_hexdigit());
        if !valid {
            p.push(format!(
                "the color {:?} is not a #RRGGBB value",
                snippet(color)
            ));
        }
    }
    for problem in ws.restore.validate() {
        p.push(format!("the restore settings are invalid: {problem}"));
    }
    if ws.applications.len() > MAX_APPLICATIONS {
        p.push(format!(
            "the workspace has {} applications (at most {MAX_APPLICATIONS} are allowed)",
            ws.applications.len()
        ));
        // Nothing more to learn from an oversized list.
        return;
    }
    let mut ids = HashSet::new();
    for app in &ws.applications {
        if !ids.insert(app.id) {
            p.push(format!("two applications share the id {}", app.id));
        }
    }
    for (index, app) in ws.applications.iter().enumerate() {
        validate_application(p, index, app, &ids);
    }
}

fn app_label(index: usize, app: &PortableApplication) -> String {
    format!(
        "application #{} ({:?})",
        index + 1,
        snippet(&app.display_name)
    )
}

fn validate_application(
    p: &mut Problems,
    index: usize,
    app: &PortableApplication,
    ids: &HashSet<Uuid>,
) {
    let label = app_label(index, app);
    if app.display_name.trim().is_empty() {
        p.push(format!("application #{} has no name", index + 1));
    }
    check_text(
        p,
        &format!("the name of {label}"),
        &app.display_name,
        MAX_TEXT_LEN,
    );
    if app.app_id.trim().is_empty() {
        p.push(format!("{label} has no application id"));
    }
    check_text(
        p,
        &format!("the application id of {label}"),
        &app.app_id,
        MAX_TEXT_LEN,
    );
    check_id(p, &format!("the application id of {label}"), &app.app_id);
    if app.adapter_id.trim().is_empty() {
        p.push(format!("{label} has no adapter id"));
    }
    check_text(
        p,
        &format!("the adapter id of {label}"),
        &app.adapter_id,
        MAX_TEXT_LEN,
    );
    check_id(p, &format!("the adapter id of {label}"), &app.adapter_id);

    if app.arguments.len() > MAX_ARGUMENTS {
        p.push(format!(
            "{label} has {} launch arguments (at most {MAX_ARGUMENTS} are allowed)",
            app.arguments.len()
        ));
    }
    for (i, argument) in app.arguments.iter().enumerate().take(MAX_ARGUMENTS) {
        check_text(
            p,
            &format!("launch argument {} of {label}", i + 1),
            argument,
            MAX_ARGUMENT_LEN,
        );
    }

    if app.depends_on.len() > MAX_APPLICATIONS {
        p.push(format!("{label} has too many dependencies"));
    } else {
        for dependency in &app.depends_on {
            if *dependency == app.id {
                p.push(format!("{label} depends on itself"));
            } else if !ids.contains(dependency) {
                p.push(format!(
                    "{label} depends on an application that is not in the file ({dependency})"
                ));
            }
        }
    }

    validate_state(p, &label, &app.state);

    if app.windows.len() > MAX_WINDOWS_PER_APP {
        p.push(format!(
            "{label} has {} saved windows (at most {MAX_WINDOWS_PER_APP} are allowed)",
            app.windows.len()
        ));
    } else {
        for (i, window) in app.windows.iter().enumerate() {
            validate_window(p, &format!("saved window {} of {label}", i + 1), window);
        }
    }
    validate_options(p, &label, &app.options);
}

fn validate_window(p: &mut Problems, what: &str, window: &WindowSnapshot) {
    check_text(
        p,
        &format!("the title of {what}"),
        &window.title,
        MAX_TEXT_LEN,
    );
    if let Some(class_name) = &window.class_name {
        check_text(p, &format!("the class of {what}"), class_name, MAX_TEXT_LEN);
    }
    if let Some(monitor) = &window.monitor_id {
        check_text(p, &format!("the monitor of {what}"), monitor, MAX_TEXT_LEN);
    }
    check_rect(p, what, &window.bounds);
}

fn validate_options(p: &mut Problems, label: &str, options: &EntryOptions) {
    if options
        .delay_after_launch_ms
        .is_some_and(|d| d > MAX_LAUNCH_DELAY_MS)
    {
        p.push(format!(
            "the launch delay of {label} is longer than {} seconds",
            MAX_LAUNCH_DELAY_MS / 1000
        ));
    }
    if options
        .timeout_ms
        .is_some_and(|t| !(MIN_TIMEOUT_MS..=MAX_TIMEOUT_MS).contains(&t))
    {
        p.push(format!(
            "the timeout of {label} must be between {} and {} seconds",
            MIN_TIMEOUT_MS / 1000,
            MAX_TIMEOUT_MS / 1000
        ));
    }
    if options.retry_count.is_some_and(|r| r > MAX_RETRIES) {
        p.push(format!("{label} allows more than {MAX_RETRIES} retries"));
    }
}

fn check_count(p: &mut Problems, what: impl FnOnce() -> String, count: usize, max: usize) -> bool {
    if count > max {
        p.push(format!(
            "{} has {count} entries (at most {max} are allowed)",
            what()
        ));
        return false;
    }
    true
}

fn validate_state(p: &mut Problems, label: &str, state: &AppState) {
    match state {
        AppState::None | AppState::Custom { .. } => {}
        AppState::Documents { documents } => {
            if check_count(
                p,
                || format!("the document list of {label}"),
                documents.len(),
                MAX_STATE_ITEMS,
            ) {
                for (i, document) in documents.iter().enumerate() {
                    let what = format!("document {} of {label}", i + 1);
                    check_path(p, &what, &document.path);
                    if document.page == Some(0) {
                        p.push(format!("{what} has page 0 (pages start at 1)"));
                    }
                }
            }
        }
        AppState::Projects { projects } => {
            if check_count(
                p,
                || format!("the project list of {label}"),
                projects.len(),
                MAX_STATE_ITEMS,
            ) {
                for (i, project) in projects.iter().enumerate() {
                    let what = format!("project {} of {label}", i + 1);
                    check_path(p, &what, &project.path);
                    check_optional_text(p, &what, project.window_title.as_deref());
                }
            }
        }
        AppState::Browser { windows, profile } => {
            if let Some(profile) = profile {
                check_text(
                    p,
                    &format!("the browser profile of {label}"),
                    profile,
                    MAX_TEXT_LEN,
                );
            }
            if windows.len() > MAX_BROWSER_WINDOWS {
                p.push(format!(
                    "{label} has {} browser windows (at most {MAX_BROWSER_WINDOWS} are allowed)",
                    windows.len()
                ));
                return;
            }
            for (w, window) in windows.iter().enumerate() {
                let what = format!("browser window {} of {label}", w + 1);
                if window.tabs.len() > MAX_TABS_PER_WINDOW {
                    p.push(format!(
                        "{what} has {} tabs (at most {MAX_TABS_PER_WINDOW} are allowed)",
                        window.tabs.len()
                    ));
                    continue;
                }
                check_optional_text(p, &what, window.window_title.as_deref());
                if let Some(active) = window.active_tab
                    && active as usize >= window.tabs.len()
                {
                    p.push(format!("the active tab of {what} does not exist"));
                }
                for (t, tab) in window.tabs.iter().enumerate() {
                    let tab_what = format!("tab {} in {what}", t + 1);
                    if tab.url.chars().count() > MAX_URL_LEN {
                        p.push(format!(
                            "the address of {tab_what} is longer than {MAX_URL_LEN} characters"
                        ));
                    } else if !is_allowed_web_url(&tab.url) {
                        p.push(format!(
                            "the address of {tab_what} is not an http, https, file or ftp URL"
                        ));
                    }
                    check_text(
                        p,
                        &format!("the title of {tab_what}"),
                        &tab.title,
                        MAX_TEXT_LEN,
                    );
                }
            }
        }
        AppState::Terminal { sessions } => {
            if check_count(
                p,
                || format!("the terminal session list of {label}"),
                sessions.len(),
                MAX_STATE_ITEMS,
            ) {
                for (i, session) in sessions.iter().enumerate() {
                    let what = format!("terminal session {} of {label}", i + 1);
                    if let Some(dir) = &session.working_directory {
                        check_path(p, &format!("the working directory of {what}"), dir);
                    }
                    if let Some(profile) = &session.profile {
                        check_text(p, &format!("the profile of {what}"), profile, MAX_TEXT_LEN);
                    }
                    if let Some(command) = &session.startup_command {
                        check_text(
                            p,
                            &format!("the startup command of {what}"),
                            command,
                            MAX_COMMAND_LEN,
                        );
                    }
                    check_optional_text(p, &what, session.window_title.as_deref());
                }
            }
        }
        AppState::Folders { folders } => {
            if check_count(
                p,
                || format!("the folder list of {label}"),
                folders.len(),
                MAX_STATE_ITEMS,
            ) {
                for (i, folder) in folders.iter().enumerate() {
                    let what = format!("folder {} of {label}", i + 1);
                    check_path(p, &what, &folder.path);
                    check_optional_text(p, &what, folder.window_title.as_deref());
                }
            }
        }
        AppState::Media {
            uri,
            label: media_label,
        } => {
            if let Some(uri) = uri
                && (uri.chars().count() > MAX_URL_LEN || sanitize_media_uri(uri).is_none())
            {
                p.push(format!("the media link of {label} is not an allowed link"));
            }
            if let Some(media_label) = media_label {
                check_text(
                    p,
                    &format!("the media label of {label}"),
                    media_label,
                    MAX_TEXT_LEN,
                );
            }
        }
    }
}

fn validate_machine(p: &mut Problems, machine: &MachineProfile, ws: &PortableWorkspace) {
    if machine.monitors.len() > MAX_MONITORS {
        p.push(format!(
            "the machine profile lists {} monitors (at most {MAX_MONITORS} are allowed)",
            machine.monitors.len()
        ));
    } else {
        for (i, monitor) in machine.monitors.iter().enumerate() {
            let what = format!("monitor {}", i + 1);
            check_text(p, &format!("the id of {what}"), &monitor.id, MAX_TEXT_LEN);
            check_text(
                p,
                &format!("the name of {what}"),
                &monitor.name,
                MAX_TEXT_LEN,
            );
            check_rect(p, &what, &monitor.bounds);
            check_rect(p, &format!("the work area of {what}"), &monitor.work_area);
            if !(monitor.scale_factor.is_finite()
                && monitor.scale_factor > 0.0
                && monitor.scale_factor <= MAX_SCALE_FACTOR)
            {
                p.push(format!("{what} has an invalid scale factor"));
            }
        }
    }
    if machine.applications.len() > MAX_APPLICATIONS {
        p.push(format!(
            "the machine profile lists {} applications (at most {MAX_APPLICATIONS} are allowed)",
            machine.applications.len()
        ));
        return;
    }
    let app_ids: HashSet<Uuid> = ws.applications.iter().map(|a| a.id).collect();
    let mut seen = HashSet::new();
    for entry in &machine.applications {
        let what = format!("the machine profile entry for {}", entry.entry_id);
        if !app_ids.contains(&entry.entry_id) {
            p.push(format!(
                "the machine profile refers to an application that is not in the workspace ({})",
                entry.entry_id
            ));
        }
        if !seen.insert(entry.entry_id) {
            p.push(format!(
                "the machine profile lists application {} twice",
                entry.entry_id
            ));
        }
        if let Some(executable) = &entry.executable {
            check_path(p, &format!("the executable in {what}"), executable);
        }
        if let Some(dir) = &entry.working_directory {
            check_path(p, &format!("the working directory in {what}"), dir);
        }
        if let Some(package) = &entry.package {
            if package.id.trim().is_empty() {
                p.push(format!("the package in {what} has no id"));
            }
            check_text(
                p,
                &format!("the package id in {what}"),
                &package.id,
                MAX_TEXT_LEN,
            );
            if let Some(path) = &package.path {
                check_path(p, &format!("the package path in {what}"), path);
            }
        }
    }
}

fn check_text(p: &mut Problems, what: &str, text: &str, max: usize) {
    if text.chars().count() > max {
        p.push(format!("{what} is longer than {max} characters"));
    }
}

fn check_optional_text(p: &mut Problems, what: &str, title: Option<&str>) {
    if let Some(title) = title {
        check_text(
            p,
            &format!("the window title of {what}"),
            title,
            MAX_TEXT_LEN,
        );
    }
}

/// Paths must be non-empty, at most [`MAX_PATH_LEN`] characters and absolute (Unix, Windows
/// drive or UNC): see `paths::is_portable_absolute`.
fn check_path(p: &mut Problems, what: &str, path: &Path) {
    let text = path.to_string_lossy();
    if text.trim().is_empty() {
        p.push(format!("the path of {what} is empty"));
    } else if text.chars().count() > MAX_PATH_LEN {
        p.push(format!(
            "the path of {what} is longer than {MAX_PATH_LEN} characters"
        ));
    } else if !is_portable_absolute(&text) {
        p.push(format!(
            "the path of {what} is not an absolute path ({:?})",
            snippet(&text)
        ));
    }
}

/// Application and adapter ids are lookup keys (`vscode`, `exe:gimp`, `pkg:org.gimp.GIMP`):
/// path separators or control characters in them are never legitimate.
fn check_id(p: &mut Problems, what: &str, id: &str) {
    if id.contains(['/', '\\']) || id.chars().any(char::is_control) {
        p.push(format!(
            "{what} contains a path separator or a control character"
        ));
    }
}

fn check_rect(p: &mut Problems, what: &str, rect: &Rect) {
    let size_ok =
        (1..MAX_DIMENSION).contains(&rect.width) && (1..MAX_DIMENSION).contains(&rect.height);
    if !size_ok {
        p.push(format!(
            "{what} has an invalid size {}×{} (width and height must be between 1 and {})",
            rect.width,
            rect.height,
            MAX_DIMENSION - 1
        ));
    }
    let position_ok = (-MAX_COORDINATE..=MAX_COORDINATE).contains(&rect.x)
        && (-MAX_COORDINATE..=MAX_COORDINATE).contains(&rect.y);
    if !position_ok {
        p.push(format!(
            "{what} has an out-of-range position ({}, {})",
            rect.x, rect.y
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(value: &serde_json::Value) -> Result<(), String> {
        scan_structure(&serde_json::to_string_pretty(value).unwrap())
    }

    #[test]
    fn nul_characters_are_located() {
        let value = serde_json::json!({
            "workspace": { "applications": [ {}, { "displayName": "a\u{0}b" } ] }
        });
        assert_eq!(
            scan(&value).unwrap_err(),
            "it contains a NUL character in workspace.applications[1].displayName"
        );
        let value = serde_json::json!({ "ok": ["x"], "bad\u{0}key": 1 });
        assert_eq!(
            scan(&value).unwrap_err(),
            "it contains a NUL character in bad\\u0000key"
        );
        assert_eq!(
            scan(&serde_json::json!("\u{0}")).unwrap_err(),
            "it contains a NUL character in the document"
        );
        assert_eq!(
            scan(&serde_json::json!({ "a": [[1, "x"], ["b\u{0}"]] })).unwrap_err(),
            "it contains a NUL character in a[1][0]"
        );
        assert!(scan(&serde_json::json!({ "a": [1, "b"], "c": "\\u0000" })).is_ok());
        // An escaped backslash followed by `u0000` is not a NUL character.
        assert!(scan_structure(r#"{"a": "\\u0000"}"#).is_ok());
        assert!(scan_structure(r#"{"a": "\u0000"}"#).is_err());
    }

    #[test]
    fn structure_limits_are_enforced_before_deserializing() {
        let deep = format!(
            "{}{}",
            "[".repeat(MAX_JSON_DEPTH + 1),
            "]".repeat(MAX_JSON_DEPTH + 1)
        );
        let message = scan_structure(&deep).unwrap_err();
        assert!(
            message.contains("nested more than 64 levels deep at line 1, column 65"),
            "{message}"
        );
        let ok_deep = format!(
            "{}{}",
            "[".repeat(MAX_JSON_DEPTH),
            "]".repeat(MAX_JSON_DEPTH)
        );
        assert!(scan_structure(&ok_deep).is_ok());

        let wide = format!("{{\"a\": [{}0]}}", "0,".repeat(MAX_JSON_ENTRIES));
        let message = scan_structure(&wide).unwrap_err();
        assert!(
            message.contains("a list with more than 10000 entries"),
            "{message}"
        );
        let keys: Vec<String> = (0..=MAX_JSON_ENTRIES)
            .map(|i| format!("\"k{i}\": 0"))
            .collect();
        let message = scan_structure(&format!("{{{}}}", keys.join(","))).unwrap_err();
        assert!(
            message.contains("an object with more than 10000 entries"),
            "{message}"
        );

        // Many small lists, each within the entry limit, still hit the value limit.
        let list = format!("[{}0]", "0,".repeat(9_999));
        let many = format!(
            "[{}{list}]",
            format!("{list},").repeat(MAX_JSON_VALUES / 10_000)
        );
        let message = scan_structure(&many).unwrap_err();
        assert!(message.contains("more than 1000000 values"), "{message}");

        // Garbage never panics (the parser reports it).
        for text in [
            "]]]}}",
            "\"unterminated",
            "{\"a\\",
            "[\"\\u00",
            "\\",
            "{\"é\": [ü]}",
        ] {
            let _ = scan_structure(text);
        }
    }

    #[test]
    fn summaries_mention_the_remaining_count() {
        let mut p = Problems::default();
        assert!(p.is_empty());
        for i in 0..30 {
            p.push(format!("problem {i}"));
        }
        assert_eq!(
            p.summary(),
            "problem 0; problem 1; problem 2 (and 27 more problems)"
        );
        let list = p.into_list();
        assert_eq!(list.len(), Problems::MAX_LISTED + 1);
        assert_eq!(
            list.last().map(String::as_str),
            Some("…and 10 more problems")
        );
    }

    #[test]
    fn rects_are_bounded() {
        let mut p = Problems::default();
        check_rect(&mut p, "w", &Rect::new(0, 0, 800, 600));
        check_rect(&mut p, "w", &Rect::new(-1920, -50, 1, 99_999));
        assert!(p.is_empty());
        check_rect(&mut p, "w", &Rect::new(0, 0, 0, 600));
        check_rect(&mut p, "w", &Rect::new(0, 0, 800, -1));
        check_rect(&mut p, "w", &Rect::new(0, 0, 100_000, 600));
        check_rect(&mut p, "w", &Rect::new(i32::MAX, 0, 800, 600));
        assert_eq!(p.total, 4);
    }
}
