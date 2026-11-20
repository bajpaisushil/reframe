//! Adapter-specific restoration state.
//!
//! Instead of one variant per application, state is modelled as reusable *kinds* (projects,
//! browser windows, terminal sessions, folders, documents, media). A new application adapter
//! normally reuses an existing kind — e.g. a JetBrains IDE stores `Projects`, Firefox stores
//! `Browser` — so adding an application never requires changing the engine. `Custom` is the
//! escape hatch for anything else.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum AppState {
    /// Nothing beyond launching the application.
    #[default]
    None,
    /// Documents passed to the application (generic apps, PDF readers).
    Documents { documents: Vec<DocumentRef> },
    /// Project folders / workspace files (code editors, IDEs).
    Projects { projects: Vec<ProjectRef> },
    /// Browser windows and their tabs.
    Browser {
        windows: Vec<BrowserWindow>,
        /// Browser profile directory name ("Default", "Profile 1").
        profile: Option<String>,
    },
    /// Terminal sessions.
    Terminal { sessions: Vec<TerminalSession> },
    /// Folders open in a file manager.
    Folders { folders: Vec<FolderRef> },
    /// Media application context, where the app exposes one.
    Media {
        /// Playlist / album / station URI (e.g. `spotify:playlist:...`).
        uri: Option<String>,
        label: Option<String>,
    },
    /// Data for third-party adapters.
    Custom { data: serde_json::Value },
}

impl AppState {
    /// Every filesystem path referenced by the state (for availability checks and import
    /// path remapping).
    pub fn paths(&self) -> Vec<PathBuf> {
        match self {
            AppState::Documents { documents } => documents.iter().map(|d| d.path.clone()).collect(),
            AppState::Projects { projects } => projects.iter().map(|p| p.path.clone()).collect(),
            AppState::Terminal { sessions } => sessions
                .iter()
                .filter_map(|s| s.working_directory.clone())
                .collect(),
            AppState::Folders { folders } => folders.iter().map(|f| f.path.clone()).collect(),
            _ => Vec::new(),
        }
    }

    /// Apply `f` to every filesystem path in the state (import path remapping).
    pub fn map_paths(&mut self, mut f: impl FnMut(&PathBuf) -> Option<PathBuf>) {
        let mut apply = |p: &mut PathBuf| {
            if let Some(new) = f(p) {
                *p = new;
            }
        };
        match self {
            AppState::Documents { documents } => {
                documents.iter_mut().for_each(|d| apply(&mut d.path))
            }
            AppState::Projects { projects } => projects.iter_mut().for_each(|p| apply(&mut p.path)),
            AppState::Terminal { sessions } => sessions
                .iter_mut()
                .filter_map(|s| s.working_directory.as_mut())
                .for_each(apply),
            AppState::Folders { folders } => folders.iter_mut().for_each(|f| apply(&mut f.path)),
            _ => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DocumentRef {
    pub path: PathBuf,
    /// 1-based page number where the reader exposes it.
    pub page: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectRef {
    /// Folder, or workspace file when `is_workspace_file` is true (e.g. `.code-workspace`).
    pub path: PathBuf,
    pub is_workspace_file: bool,
    /// Title of the window showing this project at capture time (for window matching).
    pub window_title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BrowserWindow {
    pub tabs: Vec<BrowserTab>,
    /// Index into `tabs` of the selected tab.
    pub active_tab: Option<u32>,
    pub window_title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BrowserTab {
    pub url: String,
    pub title: String,
    pub pinned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TerminalSession {
    pub working_directory: Option<PathBuf>,
    /// Terminal profile / shell profile name where supported (Windows Terminal profiles,
    /// GNOME Terminal profiles).
    pub profile: Option<String>,
    /// A command the *user* configured to run when the session starts. Captured shell history
    /// is never replayed. Only runs when `startup_command_approved` is true.
    pub startup_command: Option<String>,
    /// Set by the user in Reframe's UI. Imports always reset this to `false`.
    pub startup_command_approved: bool,
    pub window_title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FolderRef {
    pub path: PathBuf,
    pub window_title: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_with_kind_tag() {
        let state = AppState::Folders {
            folders: vec![FolderRef {
                path: "/tmp".into(),
                window_title: None,
            }],
        };
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["kind"], "folders");
        let back: AppState = serde_json::from_value(json).unwrap();
        assert_eq!(back, state);
    }

    #[test]
    fn map_paths_rewrites_every_path() {
        let mut state = AppState::Terminal {
            sessions: vec![TerminalSession {
                working_directory: Some("/home/a/code".into()),
                profile: None,
                startup_command: None,
                startup_command_approved: false,
                window_title: None,
            }],
        };
        state.map_paths(|p| {
            Some(PathBuf::from(
                p.to_string_lossy().replace("/home/a", "/home/b"),
            ))
        });
        assert_eq!(state.paths(), vec![PathBuf::from("/home/b/code")]);
    }
}
