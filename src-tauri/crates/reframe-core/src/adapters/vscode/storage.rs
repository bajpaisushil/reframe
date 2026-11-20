//! VS Code's own state files, read without modifying them. They turn the folder *name* shown
//! in a window title into a full path.
//!
//! - `User/globalStorage/state.vscdb` — SQLite, table `ItemTable(key, value)`. The key
//!   `history.recentlyOpenedPathsList` holds the "Open Recent" list, most recent first:
//!   `{"entries":[{"folderUri":…} | {"workspace":{"configPath":…}} | {"fileUri":…}]}`. VS Code
//!   keeps the database open while it runs, so it is opened read-only with a short busy
//!   timeout; if that fails (locked, hot journal) the database is read from a private
//!   temporary copy that is deleted afterwards.
//! - `User/globalStorage/storage.json` (`storage.json` in the user-data root before VS Code
//!   1.7x) — `windowsState.lastActiveWindow` / `windowsState.openedWindows[]` (windows of the
//!   last session, with a `folder` URI or `workspaceIdentifier.configURIPath`), profile names
//!   (`userDataProfiles`), folders with hot-exit backups (`backupWorkspaces`) and the pre-1.45
//!   recently-opened list (`openedPathsList`).
//!
//! Only `file:` locations are kept: remote (`vscode-remote:`) and virtual entries are skipped.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reframe_platform::OsKind;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension};
use serde_json::{Map, Value};

use super::{paths, uri};

/// `ItemTable` key of the recently-opened list in `state.vscdb`.
pub const RECENTLY_OPENED_KEY: &str = "history.recentlyOpenedPathsList";
/// How long to wait for VS Code to release a database lock before falling back to a copy.
const BUSY_TIMEOUT: Duration = Duration::from_millis(300);
const MAX_JSON_BYTES: u64 = 32 * 1024 * 1024;
const MAX_DB_COPY_BYTES: u64 = 512 * 1024 * 1024;

/// What a remembered location is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecentKind {
    Folder,
    /// A `.code-workspace` file.
    Workspace,
    /// A single file (never captured as a project; kept so callers can tell it apart).
    File,
}

/// A location VS Code opened (or was asked to open).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recent {
    pub path: PathBuf,
    pub kind: RecentKind,
}

impl Recent {
    pub fn new(path: PathBuf, kind: RecentKind) -> Self {
        Self { path, kind }
    }

    /// A folder, or a workspace when the file name ends with `.code-workspace`.
    pub fn guess(path: PathBuf) -> Self {
        let kind = if paths::is_workspace_file(&path) {
            RecentKind::Workspace
        } else {
            RecentKind::Folder
        };
        Self { path, kind }
    }
}

/// The parts of `storage.json` that matter here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StorageJson {
    /// `windowsState.lastActiveWindow` followed by `windowsState.openedWindows`.
    pub windows: Vec<Recent>,
    /// Pre-1.45 `openedPathsList` entries and folders with hot-exit backups.
    pub legacy: Vec<Recent>,
    /// Names of the user's VS Code profiles (`${profileName}` in titles).
    pub profile_names: Vec<String>,
}

/// Everything read from the candidate user-data directories.
#[derive(Debug, Clone, Default)]
pub struct StorageData {
    /// "Open Recent" entries, most recent first.
    pub recently_opened: Vec<Recent>,
    pub windows: Vec<Recent>,
    pub legacy: Vec<Recent>,
    pub profile_names: Vec<String>,
    /// Problems reading files that exist (missing files are normal and not reported).
    pub problems: Vec<String>,
    /// At least one of the state files exists.
    pub found_any: bool,
}

pub fn state_db_path(user_data: &Path) -> PathBuf {
    user_data
        .join("User")
        .join("globalStorage")
        .join("state.vscdb")
}

fn storage_json_paths(user_data: &Path) -> [PathBuf; 2] {
    [
        user_data
            .join("User")
            .join("globalStorage")
            .join("storage.json"),
        user_data.join("storage.json"),
    ]
}

/// Read the state files of every directory in `user_data_dirs` (in order). Never fails:
/// unreadable files are reported in [`StorageData::problems`].
pub fn load(user_data_dirs: &[PathBuf], os: OsKind) -> StorageData {
    let mut data = StorageData::default();
    for dir in user_data_dirs {
        let db = state_db_path(dir);
        if db.is_file() {
            data.found_any = true;
            let entries = read_state_value(&db, RECENTLY_OPENED_KEY).and_then(|value| {
                value
                    .map(|text| parse_recently_opened(&text, os))
                    .transpose()
            });
            match entries {
                Ok(Some(entries)) => data.recently_opened.extend(entries),
                Ok(None) => {}
                Err(e) => data.problems.push(format!("{}: {e}", db.display())),
            }
        }
        // The current location wins over a stale pre-migration copy.
        if let Some(json) = storage_json_paths(dir).into_iter().find(|p| p.is_file()) {
            data.found_any = true;
            match read_storage_json(&json, os) {
                Ok(parsed) => {
                    data.windows.extend(parsed.windows);
                    data.legacy.extend(parsed.legacy);
                    for name in parsed.profile_names {
                        if !data.profile_names.contains(&name) {
                            data.profile_names.push(name);
                        }
                    }
                }
                Err(e) => data.problems.push(format!("{}: {e}", json.display())),
            }
        }
    }
    data
}

fn read_storage_json(path: &Path, os: OsKind) -> Result<StorageJson, String> {
    let text = read_text(path)?;
    match parse_storage_json(&text, os) {
        Ok(parsed) => Ok(parsed),
        Err(first) => {
            // VS Code may be rewriting the file right now; try once more.
            std::thread::sleep(Duration::from_millis(60));
            let text = read_text(path)?;
            parse_storage_json(&text, os).map_err(|_| first)
        }
    }
}

fn read_text(path: &Path) -> Result<String, String> {
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_JSON_BYTES {
        return Err(format!("the file is unexpectedly large ({size} bytes)"));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Parse `storage.json`.
pub fn parse_storage_json(text: &str, os: OsKind) -> Result<StorageJson, String> {
    let root: Value =
        serde_json::from_str(text).map_err(|e| format!("the file is not valid JSON ({e})"))?;
    let mut out = StorageJson::default();
    if let Some(state) = root.get("windowsState") {
        let last = state.get("lastActiveWindow").into_iter();
        let opened = state
            .get("openedWindows")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        out.windows
            .extend(last.chain(opened).filter_map(|w| window_entry(w, os)));
    }
    if let Some(list) = root.get("openedPathsList") {
        legacy_lists(list, os, &mut out.legacy);
    }
    if let Some(backups) = root.get("backupWorkspaces") {
        backup_entries(backups, os, &mut out.legacy);
    }
    if let Some(profiles) = root.get("userDataProfiles").and_then(Value::as_array) {
        let names = profiles
            .iter()
            .filter_map(|p| p.get("name").and_then(Value::as_str))
            .map(str::trim)
            .filter(|n| !n.is_empty());
        for name in names {
            if !out.profile_names.iter().any(|n| n == name) {
                out.profile_names.push(name.to_string());
            }
        }
    }
    Ok(out)
}

/// Parse the value of [`RECENTLY_OPENED_KEY`].
pub fn parse_recently_opened(text: &str, os: OsKind) -> Result<Vec<Recent>, String> {
    let root: Value = serde_json::from_str(text)
        .map_err(|e| format!("the recently opened list is not valid JSON ({e})"))?;
    if let Some(entries) = root.get("entries").and_then(Value::as_array) {
        return Ok(entries
            .iter()
            .filter_map(|e| recent_from_item(e, None, os))
            .collect());
    }
    // Older releases stored the lists of `openedPathsList` under this key.
    let mut out = Vec::new();
    legacy_lists(&root, os, &mut out);
    Ok(out)
}

/// A `windowsState` window: `folder` (URI) or `workspaceIdentifier.configURIPath`; empty
/// windows have neither.
fn window_entry(window: &Value, os: OsKind) -> Option<Recent> {
    for key in ["folder", "folderUri", "folderPath"] {
        if let Some(path) = window.get(key).and_then(|v| path_from_value(v, os)) {
            return Some(Recent::new(path, RecentKind::Folder));
        }
    }
    for key in ["workspaceIdentifier", "workspace"] {
        if let Some(path) = window.get(key).and_then(|ws| workspace_config(ws, os)) {
            return Some(Recent::new(path, RecentKind::Workspace));
        }
    }
    None
}

fn workspace_config(workspace: &Value, os: OsKind) -> Option<PathBuf> {
    ["configURIPath", "configPath"]
        .into_iter()
        .find_map(|key| workspace.get(key).and_then(|v| path_from_value(v, os)))
}

/// `openedPathsList` of VS Code ≤ 1.44, newest list format first. Strings are URIs (`*2`,
/// `*3` lists) or plain paths (older lists); objects are workspaces.
fn legacy_lists(list: &Value, os: OsKind, out: &mut Vec<Recent>) {
    const LISTS: [(&str, Option<RecentKind>); 7] = [
        ("workspaces3", None),
        ("workspaces2", None),
        ("workspaces", None),
        ("folders2", Some(RecentKind::Folder)),
        ("folders", Some(RecentKind::Folder)),
        ("files2", Some(RecentKind::File)),
        ("files", Some(RecentKind::File)),
    ];
    for (key, kind) in LISTS {
        if let Some(items) = list.get(key).and_then(Value::as_array) {
            out.extend(items.iter().filter_map(|i| recent_from_item(i, kind, os)));
        }
    }
}

/// `backupWorkspaces` (VS Code ≥ 1.74): folders and workspaces with hot-exit backups.
fn backup_entries(backups: &Value, os: OsKind, out: &mut Vec<Recent>) {
    let list = |key: &str| {
        backups
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    for workspace in list("workspaces") {
        if let Some(path) = workspace_config(&workspace, os) {
            out.push(Recent::new(path, RecentKind::Workspace));
        }
    }
    for folder in list("folders") {
        if let Some(path) = folder.get("folderUri").and_then(|v| path_from_value(v, os)) {
            out.push(Recent::new(path, RecentKind::Folder));
        }
    }
}

/// One entry of a recently-opened list.
fn recent_from_item(item: &Value, kind_hint: Option<RecentKind>, os: OsKind) -> Option<Recent> {
    if let Some(map) = item.as_object()
        && !is_uri_components(map)
    {
        if let Some(path) = map.get("folderUri").and_then(|v| path_from_value(v, os)) {
            return Some(Recent::new(path, RecentKind::Folder));
        }
        if let Some(path) = map
            .get("workspace")
            .and_then(|ws| workspace_config(ws, os))
            .or_else(|| workspace_config(item, os))
        {
            return Some(Recent::new(path, RecentKind::Workspace));
        }
        if let Some(path) = map.get("fileUri").and_then(|v| path_from_value(v, os)) {
            return Some(Recent::new(path, RecentKind::File));
        }
        return None;
    }
    let path = path_from_value(item, os)?;
    Some(match kind_hint {
        Some(RecentKind::File) if paths::is_workspace_file(&path) => {
            Recent::new(path, RecentKind::Workspace)
        }
        Some(kind) => Recent::new(path, kind),
        None => Recent::guess(path),
    })
}

/// Serialized URI components (`{"$mid":1,"scheme":"file","path":"/c:/x",...}`).
fn is_uri_components(map: &Map<String, Value>) -> bool {
    map.contains_key("scheme") && (map.contains_key("path") || map.contains_key("external"))
}

/// A location stored as a URI string, a plain absolute path or serialized URI components.
fn path_from_value(value: &Value, os: OsKind) -> Option<PathBuf> {
    match value {
        Value::String(s) => {
            let s = s.trim();
            if uri::has_scheme(s) {
                uri::file_uri_to_path(s, os)
            } else {
                paths::is_absolute(s, os).then(|| PathBuf::from(s))
            }
        }
        Value::Object(map) => {
            let scheme = map.get("scheme").and_then(Value::as_str)?;
            if !scheme.eq_ignore_ascii_case("file") {
                return None;
            }
            if let Some(path) = map
                .get("external")
                .and_then(Value::as_str)
                .and_then(|s| uri::file_uri_to_path(s, os))
            {
                return Some(path);
            }
            if let Some(fs_path) = map
                .get("fsPath")
                .and_then(Value::as_str)
                .filter(|p| paths::is_absolute(p, os))
            {
                return Some(PathBuf::from(fs_path));
            }
            uri_path_to_local(map.get("path").and_then(Value::as_str)?, os)
        }
        _ => None,
    }
}

/// The (decoded) path component of a `file:` URI as a local path (`/c:/Users` → `C:\Users`).
fn uri_path_to_local(path: &str, os: OsKind) -> Option<PathBuf> {
    if os != OsKind::Windows {
        return path.starts_with('/').then(|| PathBuf::from(path));
    }
    let local = path.strip_prefix('/').unwrap_or(path).replace('/', r"\");
    let bytes = local.as_bytes();
    if bytes.len() < 2 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' {
        return None;
    }
    let mut out = String::with_capacity(local.len() + 1);
    out.push(char::from(bytes[0].to_ascii_uppercase()));
    out.push_str(&local[1..]);
    if out.len() == 2 {
        out.push('\\');
    }
    Some(PathBuf::from(out))
}

/// The value stored under `key` in a VS Code `state.vscdb`, without modifying the database:
/// a read-only connection first, then — when VS Code holds a lock or left a hot journal — a
/// private temporary copy (database plus journal/WAL), deleted afterwards.
pub fn read_state_value(db: &Path, key: &str) -> Result<Option<String>, String> {
    let read_only = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let direct = match query_value(db, key, read_only) {
        Ok(value) => return Ok(value),
        Err(e) if copy_may_help(&e) => e,
        Err(e) => return Err(e.to_string()),
    };
    log::debug!(
        "Reading {} directly failed ({direct}); retrying with a copy",
        db.display()
    );
    let copy = TempCopy::of(db).map_err(|e| format!("{direct}; copying it failed: {e}"))?;
    let read_write = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    query_value(&copy.db, key, read_write)
        .map_err(|e| format!("{direct}; reading a copy failed: {e}"))
}

fn query_value(path: &Path, key: &str, flags: OpenFlags) -> rusqlite::Result<Option<String>> {
    let conn = Connection::open_with_flags(path, flags)?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    let value = conn
        .query_row("SELECT value FROM ItemTable WHERE key = ?1", [key], |row| {
            Ok(match row.get_ref(0)? {
                ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
                    Some(String::from_utf8_lossy(bytes).into_owned())
                }
                _ => None,
            })
        })
        .optional()?;
    Ok(value.flatten())
}

/// Errors caused by another process using the database, which reading a copy avoids.
fn copy_may_help(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(e, _) if matches!(
            e.code,
            ErrorCode::DatabaseBusy
                | ErrorCode::DatabaseLocked
                | ErrorCode::ReadOnly
                | ErrorCode::CannotOpen
                | ErrorCode::PermissionDenied
                | ErrorCode::SystemIoFailure
                | ErrorCode::FileLockingProtocolFailed
                | ErrorCode::DatabaseCorrupt
        )
    )
}

/// A private copy of a SQLite database in a fresh temporary directory, deleted on drop.
struct TempCopy {
    dir: PathBuf,
    db: PathBuf,
}

impl TempCopy {
    fn of(db: &Path) -> io::Result<Self> {
        let size = std::fs::metadata(db)?.len();
        if size > MAX_DB_COPY_BYTES {
            return Err(io::Error::other(format!(
                "the database is too large to copy ({size} bytes)"
            )));
        }
        let dir = create_private_temp_dir()?;
        // Constructed before copying so a failed copy is cleaned up too.
        let copy = Self {
            db: dir.join("state.vscdb"),
            dir,
        };
        std::fs::copy(db, &copy.db)?;
        for suffix in ["-journal", "-wal"] {
            let side_file = with_suffix(db, suffix);
            if side_file.is_file()
                && let Err(e) = std::fs::copy(&side_file, with_suffix(&copy.db, suffix))
            {
                log::debug!("Could not copy {}: {e}", side_file.display());
            }
        }
        Ok(copy)
    }
}

impl Drop for TempCopy {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.dir) {
            log::warn!(
                "Could not remove temporary copy {}: {e}",
                self.dir.display()
            );
        }
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn create_private_temp_dir() -> io::Result<PathBuf> {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let base = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    for _ in 0..32 {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = base.join(format!("reframe-vscode-{}-{nanos}-{n}", std::process::id()));
        match private_dir_builder().create(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free temporary directory name",
    ))
}

/// The copy contains the user's recent folders: keep it private to the user.
#[cfg(unix)]
fn private_dir_builder() -> std::fs::DirBuilder {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700);
    builder
}

/// `%TEMP%` is already per-user on Windows.
#[cfg(not(unix))]
fn private_dir_builder() -> std::fs::DirBuilder {
    std::fs::DirBuilder::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn uri(path: &Path) -> String {
        url::Url::from_file_path(path)
            .expect("absolute path")
            .to_string()
    }

    fn create_db(path: &Path, value: Option<rusqlite::types::Value>) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ItemTable (key, value) VALUES ('workbench.panel.width', '300')",
            [],
        )
        .unwrap();
        if let Some(value) = value {
            conn.execute(
                "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
                params![RECENTLY_OPENED_KEY, value],
            )
            .unwrap();
        }
    }

    #[test]
    fn storage_json_windows_profiles_and_backups() {
        let text = r#"{
            "telemetry.machineId": "abc",
            "theme": "vs-dark",
            "windowsState": {
                "lastActiveWindow": {
                    "folder": "file:///home/me/code/reframe",
                    "backupPath": "/home/me/.config/Code/Backups/1",
                    "uiState": {"mode": 1, "x": 0, "y": 0, "width": 1200, "height": 800}
                },
                "openedWindows": [
                    {"folder": "file:///home/me/My%20Notes", "uiState": {"mode": 0}},
                    {"workspaceIdentifier": {"id": "f00", "configURIPath": "file:///home/me/team.code-workspace"}},
                    {"backupPath": "/home/me/.config/Code/Backups/2"},
                    {"folder": "vscode-remote://ssh-remote%2Bbox/srv/app"}
                ]
            },
            "backupWorkspaces": {
                "workspaces": [{"id": "1", "configURIPath": "file:///home/me/old.code-workspace"}],
                "folders": [{"folderUri": "file:///home/me/scratch"}],
                "emptyWindows": [{"backupFolder": "1600000000000"}]
            },
            "userDataProfiles": [
                {"location": "-7a2c", "name": "Work"},
                {"location": "-1b3d", "name": " Rust "},
                {"location": "-1b3e", "name": "Work"}
            ]
        }"#;
        let parsed = parse_storage_json(text, OsKind::Linux).unwrap();
        assert_eq!(
            parsed.windows,
            vec![
                Recent::new("/home/me/code/reframe".into(), RecentKind::Folder),
                Recent::new("/home/me/My Notes".into(), RecentKind::Folder),
                Recent::new("/home/me/team.code-workspace".into(), RecentKind::Workspace),
            ]
        );
        assert_eq!(
            parsed.legacy,
            vec![
                Recent::new("/home/me/old.code-workspace".into(), RecentKind::Workspace),
                Recent::new("/home/me/scratch".into(), RecentKind::Folder),
            ]
        );
        assert_eq!(parsed.profile_names, vec!["Work", "Rust"]);
    }

    #[test]
    fn storage_json_with_windows_uris() {
        let text = r#"{"windowsState": {
            "lastActiveWindow": {"folder": "file:///c%3A/Users/me/source/My%20App"},
            "openedWindows": [{"workspaceIdentifier": {"id": "1", "configURIPath": "file:///d%3A/ws/team.code-workspace"}}]
        }}"#;
        let parsed = parse_storage_json(text, OsKind::Windows).unwrap();
        assert_eq!(
            parsed.windows,
            vec![
                Recent::new(
                    PathBuf::from(r"C:\Users\me\source\My App"),
                    RecentKind::Folder
                ),
                Recent::new(
                    PathBuf::from(r"D:\ws\team.code-workspace"),
                    RecentKind::Workspace
                ),
            ]
        );
    }

    #[test]
    fn legacy_opened_paths_list() {
        let text = r#"{"openedPathsList": {
            "workspaces3": [
                "file:///home/me/a",
                {"id": "x", "configURIPath": "file:///home/me/b.code-workspace"}
            ],
            "workspaces2": [
                "/home/me/c",
                {"id": "y", "configPath": "/home/me/d.code-workspace"},
                "relative/ignored"
            ],
            "files2": ["file:///home/me/notes.txt"],
            "files": ["/home/me/e.code-workspace"],
            "workspaceLabels": ["a"]
        }}"#;
        let parsed = parse_storage_json(text, OsKind::Linux).unwrap();
        assert_eq!(
            parsed.legacy,
            vec![
                Recent::new("/home/me/a".into(), RecentKind::Folder),
                Recent::new("/home/me/b.code-workspace".into(), RecentKind::Workspace),
                Recent::new("/home/me/c".into(), RecentKind::Folder),
                Recent::new("/home/me/d.code-workspace".into(), RecentKind::Workspace),
                Recent::new("/home/me/notes.txt".into(), RecentKind::File),
                Recent::new("/home/me/e.code-workspace".into(), RecentKind::Workspace),
            ]
        );
        assert!(parsed.windows.is_empty() && parsed.profile_names.is_empty());
    }

    #[test]
    fn malformed_storage_json_is_an_error() {
        assert!(parse_storage_json("{\"windowsState\": ", OsKind::Linux).is_err());
        // Valid JSON of an unexpected shape is simply empty.
        assert_eq!(
            parse_storage_json("[1, 2]", OsKind::Linux).unwrap(),
            StorageJson::default()
        );
    }

    #[test]
    fn recently_opened_entries() {
        let text = r#"{"entries": [
            {"folderUri": "file:///home/me/reframe", "label": "reframe"},
            {"folderUri": "vscode-remote://ssh-remote%2Bbox/srv/app", "remoteAuthority": "ssh-remote+box"},
            {"workspace": {"id": "1", "configPath": "file:///home/me/team.code-workspace"}},
            {"fileUri": "file:///home/me/notes.md"},
            {"folderUri": {"$mid": 1, "scheme": "file", "path": "/home/me/legacy uri"}},
            {"unknown": true}
        ]}"#;
        assert_eq!(
            parse_recently_opened(text, OsKind::Linux).unwrap(),
            vec![
                Recent::new("/home/me/reframe".into(), RecentKind::Folder),
                Recent::new("/home/me/team.code-workspace".into(), RecentKind::Workspace),
                Recent::new("/home/me/notes.md".into(), RecentKind::File),
                Recent::new("/home/me/legacy uri".into(), RecentKind::Folder),
            ]
        );
        let windows = r#"{"entries": [
            {"folderUri": {"$mid": 1, "scheme": "file", "path": "/c:/Users/me/proj"}},
            {"folderUri": "file:///c%3A/Users/me/other"}
        ]}"#;
        assert_eq!(
            parse_recently_opened(windows, OsKind::Windows).unwrap(),
            vec![
                Recent::new(PathBuf::from(r"C:\Users\me\proj"), RecentKind::Folder),
                Recent::new(PathBuf::from(r"C:\Users\me\other"), RecentKind::Folder),
            ]
        );
        assert!(parse_recently_opened("not json", OsKind::Linux).is_err());
    }

    #[test]
    fn reads_the_recently_opened_list_from_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.vscdb");
        let json = r#"{"entries":[{"folderUri":"file:///home/me/reframe"}]}"#;
        create_db(&db, Some(rusqlite::types::Value::Text(json.into())));
        assert_eq!(
            read_state_value(&db, RECENTLY_OPENED_KEY)
                .unwrap()
                .as_deref(),
            Some(json)
        );
        assert_eq!(read_state_value(&db, "missing.key").unwrap(), None);

        // Values stored as BLOBs are read too.
        let blob_db = dir.path().join("blob.vscdb");
        create_db(
            &blob_db,
            Some(rusqlite::types::Value::Blob(json.as_bytes().to_vec())),
        );
        assert_eq!(
            read_state_value(&blob_db, RECENTLY_OPENED_KEY)
                .unwrap()
                .as_deref(),
            Some(json)
        );

        // A database without ItemTable is an error, not a panic.
        let other = dir.path().join("other.vscdb");
        Connection::open(&other)
            .unwrap()
            .execute_batch("CREATE TABLE t (x);")
            .unwrap();
        assert!(read_state_value(&other, RECENTLY_OPENED_KEY).is_err());
        assert!(read_state_value(&dir.path().join("absent.vscdb"), RECENTLY_OPENED_KEY).is_err());
    }

    #[test]
    fn locked_database_is_read_from_a_copy() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.vscdb");
        let json = r#"{"entries":[{"folderUri":"file:///home/me/locked"}]}"#;
        create_db(&db, Some(rusqlite::types::Value::Text(json.into())));

        // Another connection (VS Code) holds an exclusive lock.
        let holder = Connection::open(&db).unwrap();
        holder.execute_batch("BEGIN EXCLUSIVE;").unwrap();
        let read_only = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let direct = query_value(&db, RECENTLY_OPENED_KEY, read_only).unwrap_err();
        assert!(copy_may_help(&direct), "{direct}");

        assert_eq!(
            read_state_value(&db, RECENTLY_OPENED_KEY)
                .unwrap()
                .as_deref(),
            Some(json)
        );
        holder.execute_batch("ROLLBACK;").unwrap();
        // Nothing was written next to the original database.
        let mut names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["state.vscdb".to_string()]);
    }

    #[test]
    fn load_combines_user_data_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let reframe = tmp.path().join("reframe");
        let notes = tmp.path().join("notes");

        let primary = tmp.path().join("Code");
        std::fs::create_dir_all(primary.join("User/globalStorage")).unwrap();
        let entries = format!(r#"{{"entries":[{{"folderUri":"{}"}}]}}"#, uri(&reframe));
        create_db(
            &state_db_path(&primary),
            Some(rusqlite::types::Value::Text(entries)),
        );
        std::fs::write(
            primary.join("User/globalStorage/storage.json"),
            format!(
                r#"{{"windowsState":{{"lastActiveWindow":{{"folder":"{}"}}}},"userDataProfiles":[{{"name":"Work"}}]}}"#,
                uri(&notes)
            ),
        )
        .unwrap();
        // A stale pre-migration storage.json in the root is ignored.
        std::fs::write(primary.join("storage.json"), "{ broken").unwrap();

        // An old-layout directory with only a root storage.json.
        let legacy = tmp.path().join("Legacy");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(
            legacy.join("storage.json"),
            r#"{"openedPathsList":{"workspaces3":["file:///srv/old"]},"userDataProfiles":[{"name":"Work"}]}"#,
        )
        .unwrap();

        // A corrupt file is reported, not fatal.
        let corrupt = tmp.path().join("Corrupt");
        std::fs::create_dir_all(corrupt.join("User/globalStorage")).unwrap();
        std::fs::write(corrupt.join("User/globalStorage/storage.json"), "{ nope").unwrap();

        let data = load(
            &[primary.clone(), legacy, corrupt, tmp.path().join("Missing")],
            OsKind::current(),
        );
        assert!(data.found_any);
        assert_eq!(
            data.recently_opened,
            vec![Recent::new(reframe, RecentKind::Folder)]
        );
        assert_eq!(data.windows, vec![Recent::new(notes, RecentKind::Folder)]);
        if OsKind::current() != OsKind::Windows {
            assert_eq!(
                data.legacy,
                vec![Recent::new("/srv/old".into(), RecentKind::Folder)]
            );
        }
        assert_eq!(data.profile_names, vec!["Work".to_string()]);
        assert_eq!(data.problems.len(), 1, "{:?}", data.problems);
        assert!(data.problems[0].contains("Corrupt"));

        let empty = load(&[tmp.path().join("Missing")], OsKind::current());
        assert!(!empty.found_any && empty.problems.is_empty());
    }
}
