//! Local SQLite persistence.
//!
//! Workspaces, restore runs and activity are stored as versioned JSON documents alongside
//! indexed metadata columns. The nested workspace structure (entries, adapter state) evolves
//! with adapters; keeping it as a document with `schema_version` lets Rust code migrate old
//! documents explicitly instead of maintaining a wide relational schema.
//!
//! Schema migrations are tracked with `PRAGMA user_version`.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::error::{CoreError, CoreResult};
use crate::model::{ActivityEntry, AppSettings, RestoreRun, WORKSPACE_SCHEMA_VERSION, Workspace};

/// Keep this many restore runs / activity entries; older rows are pruned.
const MAX_RUNS: u32 = 200;
const MAX_ACTIVITY: u32 = 500;

const KEY_SETTINGS: &str = "settings";
const KEY_ACTIVE_WORKSPACE: &str = "active_workspace";

const MIGRATIONS: &[&str] = &[
    // v1
    r#"
    CREATE TABLE workspaces (
        id               TEXT PRIMARY KEY,
        name             TEXT NOT NULL,
        icon             TEXT NOT NULL,
        created_at       TEXT NOT NULL,
        updated_at       TEXT NOT NULL,
        last_restored_at TEXT,
        schema_version   INTEGER NOT NULL,
        document         TEXT NOT NULL
    );
    CREATE TABLE restore_runs (
        id           TEXT PRIMARY KEY,
        workspace_id TEXT NOT NULL,
        status       TEXT NOT NULL,
        started_at   TEXT NOT NULL,
        finished_at  TEXT,
        document     TEXT NOT NULL
    );
    CREATE INDEX restore_runs_started ON restore_runs(started_at DESC);
    CREATE INDEX restore_runs_workspace ON restore_runs(workspace_id, started_at DESC);
    CREATE TABLE activity (
        id           TEXT PRIMARY KEY,
        kind         TEXT NOT NULL,
        workspace_id TEXT,
        created_at   TEXT NOT NULL,
        document     TEXT NOT NULL
    );
    CREATE INDEX activity_created ON activity(created_at DESC);
    CREATE TABLE kv (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    CREATE TABLE icon_cache (
        key        TEXT PRIMARY KEY,
        data_url   TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );
    "#,
];

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    /// Open (creating if needed) the database at `path` and run migrations.
    pub fn open(path: &Path) -> CoreResult<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    /// In-memory database (tests).
    pub fn open_in_memory() -> CoreResult<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> CoreResult<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.migrate()?;
        Ok(db)
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        // A panic while holding the lock leaves the connection usable; recover the guard.
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn migrate(&self) -> CoreResult<()> {
        let mut conn = self.conn();
        let current: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let current = usize::try_from(current).unwrap_or(0);
        if current > MIGRATIONS.len() {
            return Err(CoreError::validation(
                "This database was created by a newer version of Reframe. Please update Reframe.",
            ));
        }
        for (index, sql) in MIGRATIONS.iter().enumerate().skip(current) {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", (index + 1) as i64)?;
            tx.commit()?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------ workspaces

    pub fn list_workspaces(&self) -> CoreResult<Vec<Workspace>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT document FROM workspaces ORDER BY updated_at DESC")?;
        let docs = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        docs.iter().map(|doc| decode_workspace(doc)).collect()
    }

    pub fn get_workspace(&self, id: Uuid) -> CoreResult<Option<Workspace>> {
        let conn = self.conn();
        let doc: Option<String> = conn
            .query_row(
                "SELECT document FROM workspaces WHERE id = ?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        doc.map(|d| decode_workspace(&d)).transpose()
    }

    /// Insert or replace a workspace.
    pub fn save_workspace(&self, ws: &Workspace) -> CoreResult<()> {
        let document = serde_json::to_string(ws)?;
        self.conn().execute(
            "INSERT INTO workspaces (id, name, icon, created_at, updated_at, last_restored_at, schema_version, document)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, icon = excluded.icon,
                 updated_at = excluded.updated_at, last_restored_at = excluded.last_restored_at,
                 schema_version = excluded.schema_version, document = excluded.document",
            params![
                ws.id.to_string(),
                ws.name,
                ws.icon,
                ws.created_at.to_rfc3339(),
                ws.updated_at.to_rfc3339(),
                ws.last_restored_at.map(|t| t.to_rfc3339()),
                ws.schema_version,
                document
            ],
        )?;
        Ok(())
    }

    /// Returns whether a workspace was deleted. Its run history is removed too.
    pub fn delete_workspace(&self, id: Uuid) -> CoreResult<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let deleted = tx.execute("DELETE FROM workspaces WHERE id = ?1", [id.to_string()])? > 0;
        tx.execute(
            "DELETE FROM restore_runs WHERE workspace_id = ?1",
            [id.to_string()],
        )?;
        tx.commit()?;
        Ok(deleted)
    }

    // ------------------------------------------------------------------ restore runs

    /// Insert or update a run (called when it starts and when it finishes).
    pub fn save_run(&self, run: &RestoreRun) -> CoreResult<()> {
        let document = serde_json::to_string(run)?;
        let status = serde_json::to_value(run.status)?
            .as_str()
            .unwrap_or_default()
            .to_string();
        let conn = self.conn();
        conn.execute(
            "INSERT INTO restore_runs (id, workspace_id, status, started_at, finished_at, document)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET status = excluded.status, finished_at = excluded.finished_at,
                 document = excluded.document",
            params![
                run.id.to_string(),
                run.workspace_id.to_string(),
                status,
                run.started_at.to_rfc3339(),
                run.finished_at.map(|t| t.to_rfc3339()),
                document
            ],
        )?;
        conn.execute(
            "DELETE FROM restore_runs WHERE id NOT IN (SELECT id FROM restore_runs ORDER BY started_at DESC LIMIT ?1)",
            [MAX_RUNS],
        )?;
        Ok(())
    }

    pub fn get_run(&self, id: Uuid) -> CoreResult<Option<RestoreRun>> {
        let conn = self.conn();
        let doc: Option<String> = conn
            .query_row(
                "SELECT document FROM restore_runs WHERE id = ?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(doc.map(|d| serde_json::from_str(&d)).transpose()?)
    }

    pub fn list_runs(&self, workspace_id: Option<Uuid>, limit: u32) -> CoreResult<Vec<RestoreRun>> {
        let conn = self.conn();
        let docs: Vec<String> = match workspace_id {
            Some(id) => conn
                .prepare("SELECT document FROM restore_runs WHERE workspace_id = ?1 ORDER BY started_at DESC LIMIT ?2")?
                .query_map(params![id.to_string(), limit], |row| row.get(0))?
                .collect::<Result<_, _>>()?,
            None => conn
                .prepare("SELECT document FROM restore_runs ORDER BY started_at DESC LIMIT ?1")?
                .query_map([limit], |row| row.get(0))?
                .collect::<Result<_, _>>()?,
        };
        docs.iter()
            .map(|d| serde_json::from_str(d).map_err(CoreError::from))
            .collect()
    }

    // ------------------------------------------------------------------ activity

    pub fn add_activity(&self, entry: &ActivityEntry) -> CoreResult<()> {
        let document = serde_json::to_string(entry)?;
        let kind = serde_json::to_value(entry.kind)?
            .as_str()
            .unwrap_or_default()
            .to_string();
        let conn = self.conn();
        conn.execute(
            "INSERT INTO activity (id, kind, workspace_id, created_at, document) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                entry.id.to_string(),
                kind,
                entry.workspace_id.map(|id| id.to_string()),
                entry.created_at.to_rfc3339(),
                document
            ],
        )?;
        conn.execute(
            "DELETE FROM activity WHERE id NOT IN (SELECT id FROM activity ORDER BY created_at DESC LIMIT ?1)",
            [MAX_ACTIVITY],
        )?;
        Ok(())
    }

    pub fn list_activity(&self, limit: u32) -> CoreResult<Vec<ActivityEntry>> {
        let conn = self.conn();
        let docs: Vec<String> = conn
            .prepare("SELECT document FROM activity ORDER BY created_at DESC LIMIT ?1")?
            .query_map([limit], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        docs.iter()
            .map(|d| serde_json::from_str(d).map_err(CoreError::from))
            .collect()
    }

    // ------------------------------------------------------------------ settings & state

    /// Stored settings, or defaults when none were saved (or they are unreadable).
    pub fn load_settings(&self) -> CoreResult<AppSettings> {
        Ok(self
            .kv_get(KEY_SETTINGS)?
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default())
    }

    pub fn save_settings(&self, settings: &AppSettings) -> CoreResult<()> {
        self.kv_set(KEY_SETTINGS, &serde_json::to_string(settings)?)
    }

    pub fn active_workspace(&self) -> CoreResult<Option<Uuid>> {
        Ok(self
            .kv_get(KEY_ACTIVE_WORKSPACE)?
            .and_then(|s| Uuid::parse_str(&s).ok()))
    }

    pub fn set_active_workspace(&self, id: Option<Uuid>) -> CoreResult<()> {
        match id {
            Some(id) => self.kv_set(KEY_ACTIVE_WORKSPACE, &id.to_string()),
            None => {
                self.conn()
                    .execute("DELETE FROM kv WHERE key = ?1", [KEY_ACTIVE_WORKSPACE])?;
                Ok(())
            }
        }
    }

    fn kv_get(&self, key: &str) -> CoreResult<Option<String>> {
        Ok(self
            .conn()
            .query_row("SELECT value FROM kv WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?)
    }

    fn kv_set(&self, key: &str, value: &str) -> CoreResult<()> {
        self.conn().execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // ------------------------------------------------------------------ icon cache

    pub fn cached_icon(&self, key: &str) -> CoreResult<Option<String>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT data_url FROM icon_cache WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Cache an icon; an empty string records "no icon available".
    pub fn cache_icon(&self, key: &str, data_url: &str) -> CoreResult<()> {
        self.conn().execute(
            "INSERT INTO icon_cache (key, data_url, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET data_url = excluded.data_url, updated_at = excluded.updated_at",
            params![key, data_url, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }
}

/// Decode a stored workspace document, migrating older schema versions.
fn decode_workspace(doc: &str) -> CoreResult<Workspace> {
    let mut ws: Workspace = serde_json::from_str(doc)?;
    if ws.schema_version > WORKSPACE_SCHEMA_VERSION {
        return Err(CoreError::validation(format!(
            "Workspace \"{}\" was saved by a newer version of Reframe.",
            ws.name
        )));
    }
    // Future migrations go here (match on ws.schema_version).
    ws.schema_version = WORKSPACE_SCHEMA_VERSION;
    Ok(ws)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn run_for(ws: &Workspace, minutes_ago: i64) -> RestoreRun {
        RestoreRun {
            id: Uuid::new_v4(),
            workspace_id: ws.id,
            workspace_name: ws.name.clone(),
            mode: SwitchMode::Merge,
            status: RunStatus::Completed,
            started_at: Utc::now() - chrono::Duration::minutes(minutes_ago),
            finished_at: Some(Utc::now()),
            entries: Vec::new(),
            closed_windows: Vec::new(),
            messages: Vec::new(),
        }
    }

    #[test]
    fn workspace_crud_round_trip() {
        let db = Database::open_in_memory().unwrap();
        let mut ws = Workspace::new("Development", "💻");
        ws.applications.push(ApplicationEntry::new(
            "vscode",
            "Visual Studio Code",
            "vscode",
        ));
        db.save_workspace(&ws).unwrap();
        assert_eq!(db.get_workspace(ws.id).unwrap().unwrap(), ws);

        ws.name = "Dev".into();
        db.save_workspace(&ws).unwrap();
        let all = db.list_workspaces().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Dev");

        assert!(db.delete_workspace(ws.id).unwrap());
        assert!(db.get_workspace(ws.id).unwrap().is_none());
        assert!(!db.delete_workspace(ws.id).unwrap());
    }

    #[test]
    fn runs_are_listed_newest_first_and_deleted_with_workspace() {
        let db = Database::open_in_memory().unwrap();
        let ws = Workspace::new("A", "🅰️");
        db.save_workspace(&ws).unwrap();
        let old = run_for(&ws, 10);
        let new = run_for(&ws, 1);
        db.save_run(&old).unwrap();
        db.save_run(&new).unwrap();
        let runs = db.list_runs(Some(ws.id), 10).unwrap();
        assert_eq!(
            runs.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![new.id, old.id]
        );
        assert_eq!(db.get_run(old.id).unwrap().unwrap().id, old.id);
        db.delete_workspace(ws.id).unwrap();
        assert!(db.list_runs(None, 10).unwrap().is_empty());
    }

    #[test]
    fn settings_default_and_persist() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.load_settings().unwrap(), AppSettings::default());
        let s = AppSettings {
            theme: ThemePreference::Dark,
            excluded_apps: vec!["exe:slack".into()],
            ..AppSettings::default()
        };
        db.save_settings(&s).unwrap();
        assert_eq!(db.load_settings().unwrap(), s);
    }

    #[test]
    fn active_workspace_and_icons() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.active_workspace().unwrap(), None);
        let id = Uuid::new_v4();
        db.set_active_workspace(Some(id)).unwrap();
        assert_eq!(db.active_workspace().unwrap(), Some(id));
        db.set_active_workspace(None).unwrap();
        assert_eq!(db.active_workspace().unwrap(), None);

        assert_eq!(db.cached_icon("exe:/usr/bin/x").unwrap(), None);
        db.cache_icon("exe:/usr/bin/x", "data:image/png;base64,AAA")
            .unwrap();
        assert_eq!(
            db.cached_icon("exe:/usr/bin/x").unwrap().as_deref(),
            Some("data:image/png;base64,AAA")
        );
    }

    #[test]
    fn activity_is_newest_first() {
        let db = Database::open_in_memory().unwrap();
        let ws = Workspace::new("A", "🅰️");
        let first = ActivityEntry::new(
            ActivityKind::Created,
            Some((&ws.id, &ws.name, &ws.icon)),
            "Created",
        );
        let mut second = ActivityEntry::new(
            ActivityKind::Restored,
            Some((&ws.id, &ws.name, &ws.icon)),
            "Restored",
        );
        second.created_at = first.created_at + chrono::Duration::seconds(5);
        db.add_activity(&first).unwrap();
        db.add_activity(&second).unwrap();
        let items = db.list_activity(10).unwrap();
        assert_eq!(items[0].kind, ActivityKind::Restored);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn reopening_a_file_database_keeps_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("reframe.db");
        let ws = Workspace::new("Persisted", "💾");
        {
            let db = Database::open(&path).unwrap();
            db.save_workspace(&ws).unwrap();
        }
        let db = Database::open(&path).unwrap();
        assert_eq!(db.get_workspace(ws.id).unwrap().unwrap().name, "Persisted");
    }

    #[test]
    fn newer_database_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reframe.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "user_version", 99).unwrap();
        }
        assert!(Database::open(&path).is_err());
    }
}
