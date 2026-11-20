//! Dependency graph planning (DAG validation and launch ordering).
//!
//! Entries declare the entries that must be running before they start
//! ([`ApplicationEntry::depends_on`]). The planner validates that graph and produces a launch
//! order with Kahn's algorithm, using the workspace order (entry index) as the priority among
//! entries that are ready at the same time, so independent applications launch in the order
//! the user arranged them.
//!
//! **Entries outside the run.** Only *planned* entries — enabled and, when a selection is
//! given, selected — take part in the graph. A dependency on an entry that is disabled or not
//! selected for this run is treated as *satisfied*: Reframe assumes the user keeps that
//! application running themselves (or deliberately does not want it), so the dependent is not
//! blocked and the edge does not participate in cycle detection for this run.
//! [`dependency_issues`] is stricter and checks every entry, so latent problems are visible on
//! the workspace page before a disabled entry is re-enabled.

use std::cmp::Reverse;
use std::collections::hash_map::Entry;
use std::collections::{BinaryHeap, HashMap, HashSet};

use uuid::Uuid;

use crate::model::{ApplicationEntry, Workspace};

/// Validated execution order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionPlan {
    /// Entries to run, in a valid topological order that preserves workspace order among
    /// independent entries.
    pub order: Vec<Uuid>,
    /// Dependencies of each planned entry, restricted to planned entries.
    pub dependencies: HashMap<Uuid, Vec<Uuid>>,
}

/// What kind of problem a [`PlanIssue`] is (used to explain why entries were skipped).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IssueKind {
    DuplicateId,
    SelfDependency,
    MissingReference,
    Cycle,
}

/// One planning problem together with the entries it concerns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlanIssue {
    pub kind: IssueKind,
    pub message: String,
    pub entries: Vec<Uuid>,
}

impl PlanIssue {
    /// Why an entry involved in this issue cannot be restored.
    pub fn skip_reason(&self) -> &'static str {
        match self.kind {
            IssueKind::DuplicateId => {
                "Skipped because another application in this workspace has the same id"
            }
            IssueKind::SelfDependency => "Skipped because it depends on itself",
            IssueKind::MissingReference => {
                "Skipped because it depends on an application that is not in this workspace"
            }
            IssueKind::Cycle => "Skipped because of a circular dependency",
        }
    }
}

/// Reason given to planned entries that are not themselves part of any issue when the plan
/// as a whole is invalid.
pub(crate) const INVALID_PLAN_REASON: &str =
    "Skipped because the workspace's application dependencies are invalid";

/// Plan the enabled entries (optionally restricted to `selected`). Errors list every problem
/// (cycles named by application, references to missing entries, self-dependencies, duplicate
/// ids). Dependencies on entries that are disabled or not selected are treated as satisfied
/// (see the module docs).
pub fn plan(
    workspace: &Workspace,
    selected: Option<&HashSet<Uuid>>,
) -> Result<ExecutionPlan, Vec<String>> {
    analyze(workspace, |e| {
        e.enabled && selected.is_none_or(|s| s.contains(&e.id))
    })
    .map_err(|issues| issues.into_iter().map(|i| i.message).collect())
}

/// Human-readable dependency problems for the workspace detail page. Unlike [`plan`], every
/// entry is checked — including disabled ones — so problems are reported before they would
/// block a restore.
pub fn dependency_issues(workspace: &Workspace) -> Vec<String> {
    match analyze(workspace, |_| true) {
        Ok(_) => Vec::new(),
        Err(issues) => issues.into_iter().map(|i| i.message).collect(),
    }
}

/// Validate the graph formed by the entries for which `include` returns true and order them.
pub(crate) fn analyze(
    workspace: &Workspace,
    include: impl Fn(&ApplicationEntry) -> bool,
) -> Result<ExecutionPlan, Vec<PlanIssue>> {
    let apps = &workspace.applications;
    let mut issues = Vec::new();

    // First occurrence of every id; later duplicates are reported and left out of the graph.
    let mut index_of: HashMap<Uuid, usize> = HashMap::with_capacity(apps.len());
    let mut duplicates: Vec<usize> = Vec::new();
    for (i, entry) in apps.iter().enumerate() {
        match index_of.entry(entry.id) {
            Entry::Occupied(_) => duplicates.push(i),
            Entry::Vacant(slot) => {
                slot.insert(i);
            }
        }
    }
    let included: Vec<bool> = apps.iter().map(&include).collect();
    for &dup in &duplicates {
        let first = index_of[&apps[dup].id];
        if included[dup] || included[first] {
            issues.push(PlanIssue {
                kind: IssueKind::DuplicateId,
                message: format!(
                    "{} and {} have the same id; remove one of them and add it again.",
                    apps[first].display_name, apps[dup].display_name
                ),
                entries: vec![apps[dup].id],
            });
        }
    }
    let duplicate_set: HashSet<usize> = duplicates.into_iter().collect();
    let nodes: Vec<usize> = (0..apps.len())
        .filter(|i| included[*i] && !duplicate_set.contains(i))
        .collect();
    let node_set: HashSet<usize> = nodes.iter().copied().collect();

    // Edges between planned nodes; dependencies outside the run are satisfied by definition.
    let mut deps: HashMap<usize, Vec<usize>> = HashMap::with_capacity(nodes.len());
    for &i in &nodes {
        let entry = &apps[i];
        let mut list = Vec::new();
        let mut depends_on_itself = false;
        let mut missing = false;
        for dep in &entry.depends_on {
            if *dep == entry.id {
                depends_on_itself = true;
                continue;
            }
            match index_of.get(dep) {
                None => missing = true,
                Some(&j) if node_set.contains(&j) => list.push(j),
                Some(_) => {}
            }
        }
        if depends_on_itself {
            issues.push(PlanIssue {
                kind: IssueKind::SelfDependency,
                message: format!("{} cannot depend on itself.", entry.display_name),
                entries: vec![entry.id],
            });
        }
        if missing {
            issues.push(PlanIssue {
                kind: IssueKind::MissingReference,
                message: format!(
                    "{} depends on an application that is not in this workspace.",
                    entry.display_name
                ),
                entries: vec![entry.id],
            });
        }
        list.sort_unstable();
        list.dedup();
        deps.insert(i, list);
    }

    // Kahn's algorithm; the lowest workspace index among ready entries goes first.
    let mut remaining_deps: HashMap<usize, usize> =
        nodes.iter().map(|&i| (i, deps[&i].len())).collect();
    let mut dependents: HashMap<usize, Vec<usize>> = HashMap::new();
    for &i in &nodes {
        for &d in &deps[&i] {
            dependents.entry(d).or_default().push(i);
        }
    }
    let mut ready: BinaryHeap<Reverse<usize>> = nodes
        .iter()
        .filter(|i| remaining_deps[*i] == 0)
        .map(|&i| Reverse(i))
        .collect();
    let mut order: Vec<usize> = Vec::with_capacity(nodes.len());
    while let Some(Reverse(i)) = ready.pop() {
        order.push(i);
        for &k in dependents.get(&i).map(Vec::as_slice).unwrap_or_default() {
            if let Some(count) = remaining_deps.get_mut(&k) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    ready.push(Reverse(k));
                }
            }
        }
    }

    if order.len() < nodes.len() {
        let ordered: HashSet<usize> = order.iter().copied().collect();
        let blocked: Vec<usize> = nodes
            .iter()
            .copied()
            .filter(|i| !ordered.contains(i))
            .collect();
        issues.extend(find_cycles(apps, &blocked, &deps));
    }

    if !issues.is_empty() {
        return Err(issues);
    }
    Ok(ExecutionPlan {
        order: order.iter().map(|&i| apps[i].id).collect(),
        dependencies: nodes
            .iter()
            .map(|&i| (apps[i].id, deps[&i].iter().map(|&d| apps[d].id).collect()))
            .collect(),
    })
}

/// Name disjoint cycles among `blocked` (the nodes Kahn's algorithm could not order). Every
/// blocked node has at least one blocked dependency, so following blocked dependencies from
/// any of them must eventually revisit a node: that loop is a cycle.
fn find_cycles(
    apps: &[ApplicationEntry],
    blocked: &[usize],
    deps: &HashMap<usize, Vec<usize>>,
) -> Vec<PlanIssue> {
    let blocked_set: HashSet<usize> = blocked.iter().copied().collect();
    let mut reported: HashSet<usize> = HashSet::new();
    let mut issues = Vec::new();
    for &start in blocked {
        if reported.contains(&start) {
            continue;
        }
        let mut path: Vec<usize> = Vec::new();
        let mut position: HashMap<usize, usize> = HashMap::new();
        let mut current = start;
        let cycle = loop {
            if let Some(&p) = position.get(&current) {
                break Some(path[p..].to_vec());
            }
            if reported.contains(&current) {
                break None; // leads into a cycle that is already reported
            }
            position.insert(current, path.len());
            path.push(current);
            let next = deps
                .get(&current)
                .and_then(|ds| ds.iter().copied().find(|d| blocked_set.contains(d)));
            match next {
                Some(next) => current = next,
                None => break None, // cannot happen for blocked nodes; stay defensive
            }
        };
        let Some(mut cycle) = cycle else {
            continue;
        };
        // Start at the member that comes first in the workspace; direction is preserved.
        if let Some(min_pos) = cycle
            .iter()
            .enumerate()
            .min_by_key(|(_, i)| **i)
            .map(|(p, _)| p)
        {
            cycle.rotate_left(min_pos);
        }
        let mut names: Vec<&str> = cycle
            .iter()
            .map(|&i| apps[i].display_name.as_str())
            .collect();
        names.push(apps[cycle[0]].display_name.as_str());
        issues.push(PlanIssue {
            kind: IssueKind::Cycle,
            message: format!("Circular dependency: {}", names.join(" -> ")),
            entries: cycle.iter().map(|&i| apps[i].id).collect(),
        });
        reported.extend(cycle);
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> ApplicationEntry {
        ApplicationEntry::new(format!("exe:{}", name.to_lowercase()), name, "generic")
    }

    fn workspace(entries: Vec<ApplicationEntry>) -> Workspace {
        let mut ws = Workspace::new("Test", "🧪");
        ws.applications = entries;
        ws
    }

    fn names(ws: &Workspace, ids: &[Uuid]) -> Vec<String> {
        ids.iter()
            .map(|id| ws.entry(*id).unwrap().display_name.clone())
            .collect()
    }

    #[test]
    fn independent_entries_keep_workspace_order() {
        let ws = workspace(vec![entry("A"), entry("B"), entry("C")]);
        let plan = plan(&ws, None).unwrap();
        assert_eq!(names(&ws, &plan.order), ["A", "B", "C"]);
        assert!(plan.dependencies.values().all(Vec::is_empty));
    }

    #[test]
    fn dependencies_come_first_and_priority_is_workspace_order() {
        // C depends on D (later in the list); B depends on A.
        let a = entry("A");
        let mut b = entry("B");
        let mut c = entry("C");
        let d = entry("D");
        b.depends_on.push(a.id);
        c.depends_on.push(d.id);
        let ws = workspace(vec![a.clone(), b.clone(), c.clone(), d.clone()]);
        let plan = plan(&ws, None).unwrap();
        assert_eq!(names(&ws, &plan.order), ["A", "B", "D", "C"]);
        assert_eq!(plan.dependencies[&b.id], vec![a.id]);
        assert_eq!(plan.dependencies[&c.id], vec![d.id]);
    }

    #[test]
    fn diamond_and_duplicate_dependencies() {
        let a = entry("A");
        let mut b = entry("B");
        let mut c = entry("C");
        let mut d = entry("D");
        b.depends_on = vec![a.id, a.id];
        c.depends_on = vec![a.id];
        d.depends_on = vec![c.id, b.id];
        let ws = workspace(vec![d.clone(), c.clone(), b.clone(), a.clone()]);
        let plan = plan(&ws, None).unwrap();
        assert_eq!(names(&ws, &plan.order), ["A", "C", "B", "D"]);
        assert_eq!(plan.dependencies[&b.id], vec![a.id]);
        // Dependencies are listed in workspace order (C is before B in the list).
        assert_eq!(plan.dependencies[&d.id], vec![c.id, b.id]);
    }

    #[test]
    fn cycles_are_named_by_application() {
        let mut terminal = entry("Terminal");
        let mut code = entry("VS Code");
        let other = entry("Browser");
        terminal.depends_on.push(code.id);
        code.depends_on.push(terminal.id);
        let ws = workspace(vec![terminal, code, other]);
        let errors = plan(&ws, None).unwrap_err();
        assert_eq!(
            errors,
            vec!["Circular dependency: Terminal -> VS Code -> Terminal".to_string()]
        );
        assert_eq!(dependency_issues(&ws), errors);
    }

    #[test]
    fn longer_cycle_and_entries_downstream_of_it() {
        let mut a = entry("A");
        let mut b = entry("B");
        let mut c = entry("C");
        let mut downstream = entry("Downstream");
        // A -> C -> B -> A (A depends on C, C on B, B on A); Downstream depends on B.
        a.depends_on.push(c.id);
        c.depends_on.push(b.id);
        b.depends_on.push(a.id);
        downstream.depends_on.push(b.id);
        let ws = workspace(vec![downstream, a, b, c]);
        let issues = analyze(&ws, |_| true).unwrap_err();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].kind, IssueKind::Cycle);
        assert_eq!(issues[0].message, "Circular dependency: A -> C -> B -> A");
        assert_eq!(issues[0].entries.len(), 3);
    }

    #[test]
    fn two_separate_cycles_are_both_reported() {
        let mut a = entry("A");
        let mut b = entry("B");
        let mut c = entry("C");
        let mut d = entry("D");
        a.depends_on.push(b.id);
        b.depends_on.push(a.id);
        c.depends_on.push(d.id);
        d.depends_on.push(c.id);
        let ws = workspace(vec![a, b, c, d]);
        assert_eq!(
            plan(&ws, None).unwrap_err(),
            vec![
                "Circular dependency: A -> B -> A".to_string(),
                "Circular dependency: C -> D -> C".to_string()
            ]
        );
    }

    #[test]
    fn missing_references_and_self_dependencies_are_errors() {
        let mut a = entry("A");
        let mut b = entry("B");
        a.depends_on.push(Uuid::new_v4());
        b.depends_on.push(b.id);
        let ws = workspace(vec![a, b]);
        let errors = plan(&ws, None).unwrap_err();
        assert_eq!(
            errors,
            vec![
                "A depends on an application that is not in this workspace.".to_string(),
                "B cannot depend on itself.".to_string()
            ]
        );
    }

    #[test]
    fn disabled_and_unselected_dependencies_count_as_satisfied() {
        let mut a = entry("A");
        a.enabled = false;
        let mut b = entry("B");
        let mut c = entry("C");
        b.depends_on.push(a.id);
        c.depends_on.push(b.id);
        let ws = workspace(vec![a.clone(), b.clone(), c.clone()]);

        let full = plan(&ws, None).unwrap();
        assert_eq!(full.order, vec![b.id, c.id]);
        assert!(full.dependencies[&b.id].is_empty());
        assert_eq!(full.dependencies[&c.id], vec![b.id]);

        // Only C selected: its dependency on B is assumed to be running already.
        let selected: HashSet<Uuid> = [c.id].into_iter().collect();
        let subset = plan(&ws, Some(&selected)).unwrap();
        assert_eq!(subset.order, vec![c.id]);
        assert!(subset.dependencies[&c.id].is_empty());
    }

    #[test]
    fn cycle_through_a_disabled_entry_does_not_block_a_run_but_is_reported() {
        let mut a = entry("A");
        let mut b = entry("B");
        a.depends_on.push(b.id);
        b.depends_on.push(a.id);
        b.enabled = false;
        let ws = workspace(vec![a.clone(), b]);
        assert_eq!(plan(&ws, None).unwrap().order, vec![a.id]);
        assert_eq!(
            dependency_issues(&ws),
            vec!["Circular dependency: A -> B -> A".to_string()]
        );
    }

    #[test]
    fn duplicate_ids_are_reported() {
        let a = entry("A");
        let mut copy = entry("A copy");
        copy.id = a.id;
        let ws = workspace(vec![a, copy]);
        let issues = analyze(&ws, |_| true).unwrap_err();
        assert_eq!(issues[0].kind, IssueKind::DuplicateId);
        assert!(issues[0].message.contains("same id"));
    }

    #[test]
    fn empty_workspace_plans_nothing() {
        let ws = workspace(Vec::new());
        let plan = plan(&ws, None).unwrap();
        assert!(plan.order.is_empty());
        assert!(dependency_issues(&ws).is_empty());
    }
}
