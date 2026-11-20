//! Dependency graph planning (DAG validation and launch ordering).

use std::collections::{HashMap, HashSet};

use uuid::Uuid;

use crate::model::Workspace;

/// Validated execution order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionPlan {
    /// Entries to run, in a valid topological order that preserves workspace order among
    /// independent entries.
    pub order: Vec<Uuid>,
    /// Dependencies of each planned entry, restricted to planned entries.
    pub dependencies: HashMap<Uuid, Vec<Uuid>>,
}

/// Plan the enabled entries (optionally restricted to `selected`). Errors list every problem
/// (cycles named by application, references to missing entries).
///
/// PLACEHOLDER until the restoration engine lands.
pub fn plan(
    workspace: &Workspace,
    selected: Option<&HashSet<Uuid>>,
) -> Result<ExecutionPlan, Vec<String>> {
    let order: Vec<Uuid> = workspace
        .applications
        .iter()
        .filter(|e| e.enabled && selected.is_none_or(|s| s.contains(&e.id)))
        .map(|e| e.id)
        .collect();
    Ok(ExecutionPlan {
        dependencies: order.iter().map(|id| (*id, Vec::new())).collect(),
        order,
    })
}

/// Human-readable dependency problems for the workspace detail page.
pub fn dependency_issues(workspace: &Workspace) -> Vec<String> {
    plan(workspace, None).err().unwrap_or_default()
}
