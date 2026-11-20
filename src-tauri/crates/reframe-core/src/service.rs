//! [`ReframeService`]: the single entry point used by the Tauri command layer.
//!
//! It owns the platform, adapter registry and database, runs restores on background threads
//! and records activity. Every method is synchronous; the Tauri layer calls them from async
//! commands via `spawn_blocking` where they may take a while (desktop scans, captures).

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::Utc;
use reframe_platform::{
    IconRequest, LaunchTarget, PermissionKind, PermissionStatus, Platform, PlatformInfo,
};
use uuid::Uuid;

use crate::adapters::AdapterRegistry;
use crate::database::Database;
use crate::discovery::{self, DiscoveryOptions};
use crate::error::{CoreError, CoreResult};
use crate::model::*;
use crate::restoration::{self, EventSink, RestoreEnv, RestoreOptions, RestoreTiming};
use crate::{capture, health, portability, switching};

/// Repeated edits to the same workspace within this window produce one activity entry.
const UPDATE_COALESCE_MINUTES: i64 = 15;

struct ActiveRun {
    workspace_id: Uuid,
    cancel: Arc<AtomicBool>,
    latest: Arc<Mutex<RestoreRun>>,
}

pub struct ReframeService {
    platform: Arc<dyn Platform>,
    adapters: Arc<AdapterRegistry>,
    db: Arc<Database>,
    runs: Arc<Mutex<HashMap<Uuid, ActiveRun>>>,
    app_version: String,
    timing: RestoreTiming,
    self_pid: u32,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl ReframeService {
    pub fn new(platform: Arc<dyn Platform>, db: Database, app_version: impl Into<String>) -> Self {
        Self::with_adapters(
            platform,
            Arc::new(AdapterRegistry::builtin()),
            db,
            app_version,
        )
    }

    pub fn with_adapters(
        platform: Arc<dyn Platform>,
        adapters: Arc<AdapterRegistry>,
        db: Database,
        app_version: impl Into<String>,
    ) -> Self {
        Self {
            platform,
            adapters,
            db: Arc::new(db),
            runs: Arc::new(Mutex::new(HashMap::new())),
            app_version: app_version.into(),
            timing: RestoreTiming::default(),
            self_pid: std::process::id(),
        }
    }

    /// Override restore timing (tests).
    pub fn with_timing(mut self, timing: RestoreTiming) -> Self {
        self.timing = timing;
        self
    }

    pub fn platform(&self) -> &Arc<dyn Platform> {
        &self.platform
    }

    pub fn adapters(&self) -> &Arc<AdapterRegistry> {
        &self.adapters
    }

    // ------------------------------------------------------------------ platform

    pub fn platform_info(&self) -> PlatformInfo {
        self.platform.info()
    }

    pub fn permissions(&self) -> Vec<PermissionStatus> {
        self.platform.permissions()
    }

    pub fn request_permission(&self, kind: PermissionKind) -> CoreResult<()> {
        Ok(self.platform.request_permission(kind)?)
    }

    fn discovery_options(&self, settings: &AppSettings) -> DiscoveryOptions {
        DiscoveryOptions {
            exclusions: settings.excluded_apps.clone(),
            self_pid: self.self_pid,
            browser_tab_capture: settings.browser_tab_capture,
        }
    }

    /// Inspect the current desktop ("Explore").
    pub fn scan_desktop(&self) -> CoreResult<DesktopSnapshot> {
        let settings = self.db.load_settings()?;
        let state = discovery::gather(&*self.platform);
        Ok(discovery::build_snapshot(
            &state,
            &*self.platform,
            &self.adapters,
            &self.discovery_options(&settings),
        ))
    }

    /// Application icon as a `data:image/png;base64,...` URL (cached in the database).
    pub fn app_icon(&self, lookup: &IconLookup) -> CoreResult<Option<String>> {
        if lookup.executable_path.is_none() && lookup.package.is_none() {
            return Ok(None);
        }
        let key = lookup.cache_key();
        if let Some(cached) = self.db.cached_icon(&key)? {
            return Ok((!cached.is_empty()).then_some(cached));
        }
        let request = IconRequest {
            executable_path: lookup.executable_path.clone(),
            package: lookup.package.clone(),
            window: None,
            size: 64,
        };
        match self.platform.application_icon(&request) {
            Ok(icon) => {
                let data_url = icon
                    .map(|png| format!("data:image/png;base64,{}", BASE64.encode(png)))
                    .unwrap_or_default();
                self.db.cache_icon(&key, &data_url)?;
                Ok((!data_url.is_empty()).then_some(data_url))
            }
            Err(e) => {
                log::debug!("icon lookup failed for {key}: {e}");
                Ok(None)
            }
        }
    }

    // ------------------------------------------------------------------ workspaces

    pub fn list_workspaces(&self) -> CoreResult<Vec<WorkspaceSummary>> {
        let active = self.db.active_workspace()?;
        Ok(self
            .db
            .list_workspaces()?
            .iter()
            .map(|ws| WorkspaceSummary::from_workspace(ws, active))
            .collect())
    }

    pub fn get_workspace(&self, id: Uuid) -> CoreResult<Workspace> {
        self.db
            .get_workspace(id)?
            .ok_or(CoreError::WorkspaceNotFound(id))
    }

    pub fn create_workspace(&self, input: NewWorkspace) -> CoreResult<Workspace> {
        let mut ws = Workspace::new(input.name.trim(), input.icon.trim());
        ws.description = input.description.trim().to_string();
        ws.color = input.color;
        ws.validate().map_err(CoreError::Validation)?;
        self.db.save_workspace(&ws)?;
        self.record(ActivityKind::Created, &ws, format!("Created {}", ws.name));
        Ok(ws)
    }

    /// Capture selected applications into entries (used to add apps to an existing workspace).
    pub fn capture_applications(
        &self,
        selections: &[CaptureSelection],
    ) -> CoreResult<CaptureResult> {
        if selections.is_empty() {
            return Err(CoreError::validation("Select at least one application."));
        }
        let settings = self.db.load_settings()?;
        let state = discovery::gather(&*self.platform);
        capture::capture_entries(
            &state,
            &*self.platform,
            &self.adapters,
            selections,
            &self.discovery_options(&settings),
        )
    }

    /// Capture the current desktop into a new workspace.
    pub fn capture_workspace(&self, request: CaptureRequest) -> CoreResult<CapturedWorkspace> {
        let result = self.capture_applications(&request.selections)?;
        if result.entries.is_empty() {
            return Err(CoreError::Validation(
                std::iter::once("None of the selected applications could be captured.".to_string())
                    .chain(result.warnings)
                    .collect(),
            ));
        }
        let mut ws = Workspace::new(request.name.trim(), request.icon.trim());
        ws.description = request.description.trim().to_string();
        ws.color = request.color;
        ws.applications = result.entries;
        ws.monitors = result.monitors;
        ws.validate().map_err(CoreError::Validation)?;
        self.db.save_workspace(&ws)?;
        let count = ws.applications.len();
        self.record(
            ActivityKind::Captured,
            &ws,
            format!(
                "Captured {count} application{}",
                if count == 1 { "" } else { "s" }
            ),
        );
        Ok(CapturedWorkspace {
            workspace: ws,
            warnings: result.warnings,
        })
    }

    /// Replace a workspace's applications with a fresh capture, keeping its name, settings
    /// and the per-application options of applications that are still present.
    pub fn recapture_workspace(
        &self,
        id: Uuid,
        selections: &[CaptureSelection],
    ) -> CoreResult<CapturedWorkspace> {
        let mut ws = self.get_workspace(id)?;
        let result = self.capture_applications(selections)?;
        let old = std::mem::take(&mut ws.applications);
        let mut used = vec![false; old.len()];
        let mut id_map: HashMap<Uuid, Uuid> = HashMap::new();
        let mut entries = result.entries;
        for entry in &mut entries {
            if let Some(i) = old
                .iter()
                .enumerate()
                .position(|(i, o)| !used[i] && o.app_id == entry.app_id)
            {
                used[i] = true;
                id_map.insert(old[i].id, entry.id);
                entry.enabled = old[i].enabled;
                entry.options = old[i].options.clone();
                entry.launch.arguments = old[i].launch.arguments.clone();
                entry.depends_on = old[i].depends_on.clone();
            }
        }
        for entry in &mut entries {
            entry.depends_on = entry
                .depends_on
                .iter()
                .filter_map(|d| id_map.get(d).copied())
                .collect();
        }
        ws.applications = entries;
        ws.monitors = result.monitors;
        ws.updated_at = Utc::now();
        ws.validate().map_err(CoreError::Validation)?;
        self.db.save_workspace(&ws)?;
        self.record(
            ActivityKind::Captured,
            &ws,
            format!("Updated {} from the current desktop", ws.name),
        );
        Ok(CapturedWorkspace {
            workspace: ws,
            warnings: result.warnings,
        })
    }

    /// Entry for an application picked manually (executable, app bundle, package).
    pub fn create_manual_entry(&self, target: LaunchTarget) -> CoreResult<ApplicationEntry> {
        capture::manual_entry(&*self.platform, &self.adapters, target)
    }

    /// Save an edited workspace. Server-owned fields (timestamps, schema) are preserved.
    pub fn update_workspace(&self, mut ws: Workspace) -> CoreResult<Workspace> {
        let existing = self.get_workspace(ws.id)?;
        ws.name = ws.name.trim().to_string();
        ws.description = ws.description.trim().to_string();
        ws.created_at = existing.created_at;
        ws.last_restored_at = existing.last_restored_at;
        ws.schema_version = WORKSPACE_SCHEMA_VERSION;
        ws.updated_at = Utc::now();
        ws.validate().map_err(CoreError::Validation)?;
        restoration::planner::plan(&ws, None).map_err(CoreError::Validation)?;
        self.db.save_workspace(&ws)?;
        self.record_update(&ws);
        Ok(ws)
    }

    pub fn delete_workspace(&self, id: Uuid) -> CoreResult<()> {
        if lock(&self.runs).values().any(|r| r.workspace_id == id) {
            return Err(CoreError::Conflict(
                "This workspace is being restored. Wait for it to finish or cancel it first."
                    .into(),
            ));
        }
        let ws = self.get_workspace(id)?;
        self.db.delete_workspace(id)?;
        if self.db.active_workspace()? == Some(id) {
            self.db.set_active_workspace(None)?;
        }
        let mut settings = self.db.load_settings()?;
        if settings.startup.workspace_id == Some(id) {
            settings.startup.workspace_id = None;
            if settings.startup.action == StartupAction::RestoreWorkspace {
                settings.startup.action = StartupAction::DoNothing;
            }
            self.db.save_settings(&settings)?;
        }
        self.add_activity(ActivityEntry::new(
            ActivityKind::Deleted,
            Some((&ws.id, &ws.name, &ws.icon)),
            format!("Deleted {}", ws.name),
        ));
        Ok(())
    }

    pub fn duplicate_workspace(&self, id: Uuid) -> CoreResult<Workspace> {
        let source = self.get_workspace(id)?;
        let mut copy = source.clone();
        let now = Utc::now();
        copy.id = Uuid::new_v4();
        let suffix = " (copy)";
        let max_base = MAX_NAME_LEN.saturating_sub(suffix.chars().count());
        copy.name = format!(
            "{}{suffix}",
            source.name.chars().take(max_base).collect::<String>()
        );
        copy.created_at = now;
        copy.updated_at = now;
        copy.last_restored_at = None;
        let id_map: HashMap<Uuid, Uuid> = copy
            .applications
            .iter()
            .map(|e| (e.id, Uuid::new_v4()))
            .collect();
        for entry in &mut copy.applications {
            entry.id = id_map[&entry.id];
            entry.depends_on = entry
                .depends_on
                .iter()
                .filter_map(|d| id_map.get(d).copied())
                .collect();
        }
        copy.validate().map_err(CoreError::Validation)?;
        self.db.save_workspace(&copy)?;
        self.record(
            ActivityKind::Created,
            &copy,
            format!("Duplicated from {}", source.name),
        );
        Ok(copy)
    }

    pub fn workspace_health(&self, id: Uuid) -> CoreResult<WorkspaceHealth> {
        let ws = self.get_workspace(id)?;
        let state = discovery::gather(&*self.platform);
        Ok(health::workspace_health(
            &ws,
            &state,
            &*self.platform,
            &self.adapters,
        ))
    }

    // ------------------------------------------------------------------ switching & restore

    pub fn plan_switch(&self, id: Uuid, mode: SwitchMode) -> CoreResult<SwitchPlan> {
        let target = self.get_workspace(id)?;
        let previous = match self.db.active_workspace()? {
            Some(prev) if prev != id => self.db.get_workspace(prev)?,
            _ => None,
        };
        let state = discovery::gather(&*self.platform);
        Ok(switching::plan_switch(
            &state,
            &self.adapters,
            &state.info,
            previous.as_ref(),
            &target,
            mode,
        ))
    }

    /// Start a restore on a background thread. Progress is reported through `sink`
    /// (`RunStarted`, `EntryUpdated`, `WindowCloseUpdated`, `RunFinished`).
    pub fn start_restore(&self, request: RestoreRequest, sink: Arc<EventSink>) -> CoreResult<Uuid> {
        let ws = self.get_workspace(request.workspace_id)?;
        if !ws.applications.iter().any(|e| e.enabled) {
            return Err(CoreError::validation(
                "This workspace has no enabled applications to restore.",
            ));
        }
        if let Some(ids) = &request.entry_ids
            && ids.is_empty()
        {
            return Err(CoreError::validation("Nothing selected to restore."));
        }
        let settings = self.db.load_settings()?;
        let run_id = Uuid::new_v4();
        let cancel = Arc::new(AtomicBool::new(false));
        let latest = Arc::new(Mutex::new(RestoreRun::pending(
            run_id,
            &ws,
            request.mode,
            request.entry_ids.as_deref(),
        )));
        {
            let mut runs = lock(&self.runs);
            if runs.values().any(|r| r.workspace_id == ws.id) {
                return Err(CoreError::Conflict(format!(
                    "{} is already being restored.",
                    ws.name
                )));
            }
            runs.insert(
                run_id,
                ActiveRun {
                    workspace_id: ws.id,
                    cancel: Arc::clone(&cancel),
                    latest: Arc::clone(&latest),
                },
            );
        }

        let options = RestoreOptions {
            run_id,
            mode: request.mode,
            entry_ids: request.entry_ids.clone(),
            close_window_ids: if request.mode == SwitchMode::Replace {
                request.close_window_ids.clone()
            } else {
                Vec::new()
            },
        };
        let env = RestoreEnv {
            platform: Arc::clone(&self.platform),
            adapters: Arc::clone(&self.adapters),
            timing: RestoreTiming {
                close_wait: Duration::from_secs(u64::from(settings.close_wait_seconds)),
                ..self.timing.clone()
            },
        };
        let db = Arc::clone(&self.db);
        let runs = Arc::clone(&self.runs);
        let spawn = std::thread::Builder::new()
            .name(format!("restore-{run_id}"))
            .spawn(move || {
                let tracking = {
                    let latest = Arc::clone(&latest);
                    let sink = Arc::clone(&sink);
                    move |event: RestoreEvent| {
                        {
                            let mut run = lock(&latest);
                            match &event {
                                RestoreEvent::RunStarted { run: r }
                                | RestoreEvent::RunFinished { run: r } => *run = r.clone(),
                                RestoreEvent::EntryUpdated { entry, .. } => {
                                    match run
                                        .entries
                                        .iter_mut()
                                        .find(|e| e.entry_id == entry.entry_id)
                                    {
                                        Some(slot) => *slot = entry.clone(),
                                        None => run.entries.push(entry.clone()),
                                    }
                                }
                                RestoreEvent::WindowCloseUpdated { outcome, .. } => {
                                    match run
                                        .closed_windows
                                        .iter_mut()
                                        .find(|c| c.window_id == outcome.window_id)
                                    {
                                        Some(slot) => *slot = outcome.clone(),
                                        None => run.closed_windows.push(outcome.clone()),
                                    }
                                }
                            }
                        }
                        sink(event);
                    }
                };
                if let Err(e) = db.save_run(&lock(&latest).clone()) {
                    log::warn!("could not record restore run start: {e}");
                }
                let run = restoration::execute(&env, &ws, options, &cancel, &tracking);
                Self::finish_run(&db, &ws, &run);
                lock(&runs).remove(&run.id);
            });
        if let Err(e) = spawn {
            lock(&self.runs).remove(&run_id);
            return Err(CoreError::Io(e));
        }
        Ok(run_id)
    }

    /// Persist a finished run, update the workspace's last-restored time, the active workspace
    /// and the activity feed.
    fn finish_run(db: &Database, ws: &Workspace, run: &RestoreRun) {
        if let Err(e) = db.save_run(run) {
            log::warn!("could not save restore run: {e}");
        }
        let restored = run
            .entries
            .iter()
            .filter(|e| {
                matches!(
                    e.status,
                    EntryStatus::Completed | EntryStatus::PartiallyCompleted
                )
            })
            .count();
        let attempted = run
            .entries
            .iter()
            .filter(|e| e.message.as_deref() != Some("Disabled"))
            .count();
        if matches!(
            run.status,
            RunStatus::Completed | RunStatus::PartiallyCompleted
        ) {
            // Reload to avoid overwriting edits made while the restore was running.
            if let Ok(Some(mut current)) = db.get_workspace(ws.id) {
                current.last_restored_at = Some(Utc::now());
                if let Err(e) = db.save_workspace(&current) {
                    log::warn!("could not update last restored time: {e}");
                }
            }
            if let Err(e) = db.set_active_workspace(Some(ws.id)) {
                log::warn!("could not set active workspace: {e}");
            }
        }
        let (kind, message) = match run.status {
            RunStatus::Completed => (ActivityKind::Restored, format!("Restored {}", ws.name)),
            RunStatus::PartiallyCompleted => (
                ActivityKind::RestorePartial,
                format!("Restored {restored} of {attempted} applications"),
            ),
            RunStatus::Cancelled => (
                ActivityKind::RestorePartial,
                format!("Restore cancelled after {restored} applications"),
            ),
            RunStatus::Failed | RunStatus::Running => (
                ActivityKind::RestoreFailed,
                format!("Could not restore {}", ws.name),
            ),
        };
        if let Err(e) = db.add_activity(&ActivityEntry::new(
            kind,
            Some((&ws.id, &ws.name, &ws.icon)),
            message,
        )) {
            log::warn!("could not record activity: {e}");
        }
    }

    pub fn cancel_restore(&self, run_id: Uuid) -> CoreResult<()> {
        let runs = lock(&self.runs);
        let run = runs
            .get(&run_id)
            .ok_or_else(|| CoreError::NotFound("Restore run".into()))?;
        run.cancel.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Restores in progress, with their latest known state.
    pub fn active_runs(&self) -> Vec<RestoreRun> {
        lock(&self.runs)
            .values()
            .map(|r| lock(&r.latest).clone())
            .collect()
    }

    pub fn get_run(&self, run_id: Uuid) -> CoreResult<RestoreRun> {
        if let Some(run) = lock(&self.runs).get(&run_id) {
            return Ok(lock(&run.latest).clone());
        }
        self.db
            .get_run(run_id)?
            .ok_or_else(|| CoreError::NotFound("Restore run".into()))
    }

    pub fn restore_history(
        &self,
        workspace_id: Option<Uuid>,
        limit: u32,
    ) -> CoreResult<Vec<RestoreRun>> {
        self.db.list_runs(workspace_id, limit.clamp(1, 200))
    }

    // ------------------------------------------------------------------ activity & settings

    pub fn activity(&self, limit: u32) -> CoreResult<Vec<ActivityEntry>> {
        self.db.list_activity(limit.clamp(1, 200))
    }

    pub fn settings(&self) -> CoreResult<AppSettings> {
        self.db.load_settings()
    }

    /// Validate and store settings. Registering the OS autostart entry is done by the caller
    /// (the Tauri layer owns the autostart plugin).
    pub fn update_settings(&self, mut settings: AppSettings) -> CoreResult<AppSettings> {
        settings.excluded_apps = settings
            .excluded_apps
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .fold(Vec::new(), |mut acc, s| {
                if !acc.iter().any(|a: &String| a.eq_ignore_ascii_case(&s)) {
                    acc.push(s);
                }
                acc
            });
        settings.validate().map_err(CoreError::Validation)?;
        if let Some(id) = settings.startup.workspace_id
            && self.db.get_workspace(id)?.is_none()
        {
            return Err(CoreError::validation(
                "The startup workspace no longer exists.",
            ));
        }
        self.db.save_settings(&settings)?;
        Ok(settings)
    }

    pub fn active_workspace(&self) -> CoreResult<Option<Uuid>> {
        self.db.active_workspace()
    }

    /// The workspace to restore at login according to the startup settings, if any.
    pub fn startup_restore_target(&self) -> CoreResult<Option<Workspace>> {
        let settings = self.db.load_settings()?;
        let id = match settings.startup.action {
            StartupAction::DoNothing => None,
            StartupAction::RestoreWorkspace => settings.startup.workspace_id,
            StartupAction::RestoreLastActive => self.db.active_workspace()?,
        };
        match id {
            Some(id) => self.db.get_workspace(id),
            None => Ok(None),
        }
    }

    // ------------------------------------------------------------------ import / export

    pub fn export_workspace(&self, id: Uuid, path: &Path) -> CoreResult<()> {
        let ws = self.get_workspace(id)?;
        let export = portability::export_workspace(
            &ws,
            &self.adapters,
            self.platform.info().os,
            &self.app_version,
        )?;
        let json = serde_json::to_string_pretty(&export)?;
        std::fs::write(path, json)?;
        self.record(ActivityKind::Exported, &ws, format!("Exported {}", ws.name));
        Ok(())
    }

    fn read_export(path: &Path) -> CoreResult<WorkspaceExport> {
        let size = std::fs::metadata(path)?.len();
        if size > MAX_IMPORT_BYTES as u64 {
            return Err(CoreError::InvalidImport(
                "the file is larger than 5 MB".into(),
            ));
        }
        portability::parse_export(&std::fs::read(path)?)
    }

    pub fn preview_import(&self, path: &Path) -> CoreResult<ImportPreview> {
        let export = Self::read_export(path)?;
        portability::preview_import(
            &export,
            &path.display().to_string(),
            &*self.platform,
            &self.adapters,
        )
    }

    pub fn commit_import(&self, resolution: ImportResolution) -> CoreResult<Workspace> {
        let export = Self::read_export(Path::new(&resolution.file_path))?;
        let ws = portability::build_imported_workspace(
            &export,
            &resolution,
            &*self.platform,
            &self.adapters,
        )?;
        ws.validate().map_err(CoreError::Validation)?;
        restoration::planner::plan(&ws, None).map_err(CoreError::Validation)?;
        self.db.save_workspace(&ws)?;
        self.record(ActivityKind::Imported, &ws, format!("Imported {}", ws.name));
        Ok(ws)
    }

    // ------------------------------------------------------------------ helpers

    fn record(&self, kind: ActivityKind, ws: &Workspace, message: String) {
        self.add_activity(ActivityEntry::new(
            kind,
            Some((&ws.id, &ws.name, &ws.icon)),
            message,
        ));
    }

    fn record_update(&self, ws: &Workspace) {
        let recent = self.db.list_activity(1).ok().and_then(|mut v| v.pop());
        let coalesce = recent.is_some_and(|a| {
            a.kind == ActivityKind::Updated
                && a.workspace_id == Some(ws.id)
                && Utc::now() - a.created_at < chrono::Duration::minutes(UPDATE_COALESCE_MINUTES)
        });
        if !coalesce {
            self.record(ActivityKind::Updated, ws, format!("Edited {}", ws.name));
        }
    }

    fn add_activity(&self, entry: ActivityEntry) {
        if let Err(e) = self.db.add_activity(&entry) {
            log::warn!("could not record activity: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reframe_platform::mock::MockPlatform;

    fn service() -> ReframeService {
        ReframeService::new(
            Arc::new(MockPlatform::new()),
            Database::open_in_memory().unwrap(),
            "test",
        )
    }

    fn new_ws(name: &str) -> NewWorkspace {
        NewWorkspace {
            name: name.into(),
            description: " desc ".into(),
            icon: "💻".into(),
            color: None,
        }
    }

    #[test]
    fn create_list_update_delete() {
        let svc = service();
        let ws = svc.create_workspace(new_ws("  Dev  ")).unwrap();
        assert_eq!(ws.name, "Dev");
        assert_eq!(ws.description, "desc");
        assert_eq!(svc.list_workspaces().unwrap().len(), 1);

        let mut edited = ws.clone();
        edited.name = "Development".into();
        edited.created_at = Utc::now() + chrono::Duration::days(1); // ignored
        let saved = svc.update_workspace(edited).unwrap();
        assert_eq!(saved.name, "Development");
        assert_eq!(saved.created_at, ws.created_at);

        svc.delete_workspace(ws.id).unwrap();
        assert!(matches!(
            svc.get_workspace(ws.id),
            Err(CoreError::WorkspaceNotFound(_))
        ));
        let kinds: Vec<_> = svc
            .activity(10)
            .unwrap()
            .into_iter()
            .map(|a| a.kind)
            .collect();
        assert_eq!(
            kinds,
            vec![
                ActivityKind::Deleted,
                ActivityKind::Updated,
                ActivityKind::Created
            ]
        );
    }

    #[test]
    fn invalid_workspace_is_rejected() {
        let svc = service();
        assert!(matches!(
            svc.create_workspace(new_ws("   ")),
            Err(CoreError::Validation(_))
        ));
    }

    #[test]
    fn duplicate_remaps_entry_ids_and_dependencies() {
        let svc = service();
        let mut ws = svc.create_workspace(new_ws("Dev")).unwrap();
        let a = ApplicationEntry::new("vscode", "VS Code", "vscode");
        let mut b = ApplicationEntry::new("terminal", "Terminal", "terminal");
        b.depends_on.push(a.id);
        ws.applications = vec![a.clone(), b];
        let ws = svc.update_workspace(ws).unwrap();
        let copy = svc.duplicate_workspace(ws.id).unwrap();
        assert_ne!(copy.id, ws.id);
        assert_eq!(copy.name, "Dev (copy)");
        assert_ne!(copy.applications[0].id, a.id);
        assert_eq!(
            copy.applications[1].depends_on,
            vec![copy.applications[0].id]
        );
    }

    #[test]
    fn deleting_startup_workspace_resets_startup_action() {
        let svc = service();
        let ws = svc.create_workspace(new_ws("Morning")).unwrap();
        let mut settings = svc.settings().unwrap();
        settings.startup.action = StartupAction::RestoreWorkspace;
        settings.startup.workspace_id = Some(ws.id);
        svc.update_settings(settings).unwrap();
        assert_eq!(svc.startup_restore_target().unwrap().unwrap().id, ws.id);
        svc.delete_workspace(ws.id).unwrap();
        let settings = svc.settings().unwrap();
        assert_eq!(settings.startup.action, StartupAction::DoNothing);
        assert!(svc.startup_restore_target().unwrap().is_none());
    }

    #[test]
    fn settings_validation_and_exclusion_cleanup() {
        let svc = service();
        let mut settings = svc.settings().unwrap();
        settings.excluded_apps = vec![" exe:slack ".into(), "EXE:SLACK".into(), "".into()];
        let saved = svc.update_settings(settings.clone()).unwrap();
        assert_eq!(saved.excluded_apps, vec!["exe:slack".to_string()]);
        settings.startup.action = StartupAction::RestoreWorkspace;
        settings.startup.workspace_id = None;
        assert!(svc.update_settings(settings).is_err());
    }

    #[test]
    fn restore_requires_enabled_applications() {
        let svc = service();
        let ws = svc.create_workspace(new_ws("Empty")).unwrap();
        let request = RestoreRequest {
            workspace_id: ws.id,
            mode: SwitchMode::Merge,
            entry_ids: None,
            close_window_ids: vec![],
        };
        let sink: Arc<EventSink> = Arc::new(|_| {});
        assert!(matches!(
            svc.start_restore(request, sink),
            Err(CoreError::Validation(_))
        ));
    }
}
