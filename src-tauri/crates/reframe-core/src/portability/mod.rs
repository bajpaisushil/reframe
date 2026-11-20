//! Portable JSON import/export.

pub mod sanitize;

use reframe_platform::{OsKind, Platform};

use crate::adapters::AdapterRegistry;
use crate::error::{CoreError, CoreResult};
use crate::model::{ImportPreview, ImportResolution, Workspace, WorkspaceExport};

/// Build the export document: logical workspace (sanitized by each adapter) + machine profile.
///
/// PLACEHOLDER until the portability implementation lands.
pub fn export_workspace(
    _ws: &Workspace,
    _adapters: &AdapterRegistry,
    _os: OsKind,
    _app_version: &str,
) -> CoreResult<WorkspaceExport> {
    Err(CoreError::NotImplemented("Exporting workspaces"))
}

/// Parse and strictly validate an export file (size limit, format/version, field limits,
/// references). Malformed input yields `CoreError::InvalidImport` with a clear reason.
///
/// PLACEHOLDER until the portability implementation lands.
pub fn parse_export(_bytes: &[u8]) -> CoreResult<WorkspaceExport> {
    Err(CoreError::NotImplemented("Importing workspaces"))
}

/// Resolve applications and paths on this machine for the mapping screen.
///
/// PLACEHOLDER until the portability implementation lands.
pub fn preview_import(
    _export: &WorkspaceExport,
    _file_path: &str,
    _platform: &dyn Platform,
    _adapters: &AdapterRegistry,
) -> CoreResult<ImportPreview> {
    Err(CoreError::NotImplemented("Importing workspaces"))
}

/// Create a new workspace (fresh ids) from an export and the user's mapping decisions.
/// Imported terminal startup commands are always reset to unapproved.
///
/// PLACEHOLDER until the portability implementation lands.
pub fn build_imported_workspace(
    _export: &WorkspaceExport,
    _resolution: &ImportResolution,
    _platform: &dyn Platform,
    _adapters: &AdapterRegistry,
) -> CoreResult<Workspace> {
    Err(CoreError::NotImplemented("Importing workspaces"))
}
