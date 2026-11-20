//! Which launch steps of an entry are already satisfied by running windows (Merge/Replace).
//!
//! Shared by the orchestrator, the switch planner and the health report (all through
//! [`decide_entry`], over the entries in workspace order), so the plan shown before a switch
//! matches what the restore does:
//!
//! - A step with a title hint (project, folder or document name) is satisfied by a running
//!   window of the entry's application whose title contains the hint as a whole name
//!   (case-insensitive, trimmed): `proj-alpha` matches "main.rs — proj-alpha — Editor", but
//!   `app` does not match "my-app — Editor", so a short name is not mistaken for a longer one.
//!   Each window satisfies at most one step; longer (more specific) hints choose first.
//! - A step without a hint (generic applications) is satisfied as soon as the application has
//!   a window: the per-application rule.
//!
//! Only windows that no other entry took over are considered, so two entries of the same
//! application never share a window. Unsatisfied steps are launched.

use std::collections::HashSet;

use reframe_platform::{WindowId, WindowInfo};

use super::matcher;
use crate::adapters::{AdapterRegistry, LaunchStep};
use crate::model::{AppState, ApplicationEntry, DesktopState};

/// The reuse decision for one entry.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Reuse {
    /// Per launch step: already satisfied by a running window, so it is not launched.
    pub satisfied: Vec<bool>,
    /// Running windows the entry takes over (to be claimed for it), front-to-back. Empty when
    /// no step is satisfied.
    pub windows: Vec<WindowId>,
}

impl Reuse {
    pub fn satisfied_count(&self) -> usize {
        self.satisfied.iter().filter(|s| **s).count()
    }

    /// Every step is satisfied: nothing needs to be launched.
    pub fn complete(&self) -> bool {
        !self.satisfied.is_empty() && self.satisfied.iter().all(|s| *s)
    }
}

/// The reuse decision for `entry` (with launch plan `plan`) on the desktop `state`, or `None`
/// when its application has no window that an earlier entry did not take already. The
/// windows the entry takes over are added to `taken`; call this for the entries in workspace
/// order with one `taken` set, as the restore does.
pub(crate) fn decide_entry(
    entry: &ApplicationEntry,
    plan: &Result<Vec<LaunchStep>, String>,
    state: &DesktopState,
    adapters: &AdapterRegistry,
    taken: &mut HashSet<WindowId>,
) -> Option<Reuse> {
    let running: Vec<WindowInfo> = matcher::running_windows_for_entry(entry, state, adapters)
        .into_iter()
        .filter(|id| !taken.contains(id))
        .filter_map(|id| state.windows.iter().find(|w| w.id == id))
        .cloned()
        .collect();
    if running.is_empty() {
        return None;
    }
    let reuse = decide(entry, &step_hints(plan), &running);
    if reuse.satisfied_count() > 0 {
        taken.extend(reuse.windows.iter().cloned());
    }
    Some(reuse)
}

/// One title hint per launch step. When the launch plan cannot be built (or is empty) the
/// entry counts as a single step without a hint, i.e. the per-application rule applies.
pub(crate) fn step_hints(plan: &Result<Vec<LaunchStep>, String>) -> Vec<Option<String>> {
    match plan {
        Ok(steps) if !steps.is_empty() => steps.iter().map(|s| s.title_hint.clone()).collect(),
        _ => vec![None],
    }
}

/// Decide which of `entry`'s launch steps are satisfied by `running` — the windows of its
/// application that no other entry took, front-to-back. `hints` holds one title hint per
/// launch step (see [`step_hints`]).
///
/// The windows taken over are the ones matched by hinted steps plus, when a step without a
/// hint is satisfied, the remaining windows that fit the entry's saved windows (at least one,
/// so another entry of the same application does not count the same window as its own).
pub(crate) fn decide(
    entry: &ApplicationEntry,
    hints: &[Option<String>],
    running: &[WindowInfo],
) -> Reuse {
    let normalized: Vec<Option<String>> = hints
        .iter()
        .map(|h| {
            h.as_deref()
                .map(|h| h.trim().to_lowercase())
                .filter(|h| !h.is_empty())
        })
        .collect();
    let titles: Vec<String> = running.iter().map(|w| w.title.to_lowercase()).collect();
    let mut satisfied = vec![false; hints.len()];
    let mut taken: Vec<usize> = Vec::new();

    // Longer hints are more specific: they choose first (stable, so ties keep step order).
    let mut hinted: Vec<(usize, &str)> = normalized
        .iter()
        .enumerate()
        .filter_map(|(i, h)| h.as_deref().map(|h| (i, h)))
        .collect();
    hinted.sort_by_key(|(_, h)| std::cmp::Reverse(h.chars().count()));
    for (step, hint) in hinted {
        if let Some(w) =
            (0..running.len()).find(|w| !taken.contains(w) && contains_name(&titles[*w], hint))
        {
            taken.push(w);
            satisfied[step] = true;
        }
    }

    let unhinted = normalized.iter().any(Option::is_none);
    if unhinted && !running.is_empty() {
        for (step, hint) in normalized.iter().enumerate() {
            if hint.is_none() {
                satisfied[step] = true;
            }
        }
        // Per-application rule: take over the windows that fit the saved ones.
        let all_hints: Vec<String> = hints.iter().flatten().cloned().collect();
        for (_, id) in matcher::assign_windows_with_fallback(&entry.windows, running, &all_hints) {
            if let Some(w) = running.iter().position(|w| w.id == id)
                && !taken.contains(&w)
            {
                taken.push(w);
            }
        }
        if taken.is_empty() {
            taken.push(0);
        }
    }

    taken.sort_unstable();
    Reuse {
        satisfied,
        windows: taken.into_iter().map(|w| running[w].id.clone()).collect(),
    }
}

/// What an entry's launch steps open, for messages ("2 projects", "1 folder").
pub(crate) fn step_noun(state: &AppState, count: usize) -> &'static str {
    let (one, many) = match state {
        AppState::Projects { .. } => ("project", "projects"),
        AppState::Folders { .. } => ("folder", "folders"),
        AppState::Documents { .. } => ("document", "documents"),
        AppState::Browser { .. } => ("window", "windows"),
        AppState::Terminal { .. } => ("session", "sessions"),
        _ => ("item", "items"),
    };
    if count == 1 { one } else { many }
}

/// Why an entry whose application is running still has to launch something
/// ("1 of 2 projects not open").
pub(crate) fn not_open_reason(state: &AppState, reuse: &Reuse) -> String {
    let total = reuse.satisfied.len();
    let missing = total - reuse.satisfied_count();
    if total == 1 {
        format!("Running, but this {} isn't open", step_noun(state, 1))
    } else {
        format!("{missing} of {total} {} not open", step_noun(state, total))
    }
}

/// Whether `title` (lower-case) contains `name` (lower-case, non-empty) as a whole name: not
/// glued to a letter, digit, `-`, `_` or `.` on either side.
fn contains_name(title: &str, name: &str) -> bool {
    let is_name_char = |c: char| c.is_alphanumeric() || matches!(c, '-' | '_' | '.');
    let starts_with_name_char = name.chars().next().is_some_and(is_name_char);
    let ends_with_name_char = name.chars().next_back().is_some_and(is_name_char);
    !name.is_empty()
        && title.match_indices(name).any(|(start, _)| {
            let before = title[..start].chars().next_back();
            let after = title[start + name.len()..].chars().next();
            (!starts_with_name_char || !before.is_some_and(is_name_char))
                && (!ends_with_name_char || !after.is_some_and(is_name_char))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use reframe_platform::{Rect, WindowState};

    use crate::model::WindowSnapshot;

    fn window(id: &str, title: &str) -> WindowInfo {
        WindowInfo {
            id: WindowId::new(id),
            pid: 1,
            title: title.into(),
            class_name: None,
            app_id_hint: None,
            bounds: Rect::new(0, 0, 800, 600),
            normal_bounds: None,
            state: WindowState::Normal,
            monitor_id: None,
            desktop: None,
            is_focused: false,
            z_order: 0,
        }
    }

    fn snapshot(title: &str) -> WindowSnapshot {
        WindowSnapshot {
            title: title.into(),
            class_name: None,
            bounds: Rect::new(0, 0, 800, 600),
            state: WindowState::Normal,
            monitor_id: None,
        }
    }

    fn hints(names: &[Option<&str>]) -> Vec<Option<String>> {
        names.iter().map(|h| h.map(str::to_string)).collect()
    }

    fn ids(reuse: &Reuse) -> Vec<&str> {
        reuse.windows.iter().map(WindowId::as_str).collect()
    }

    #[test]
    fn whole_names_only() {
        assert!(contains_name("main.rs — proj-alpha — editor", "proj-alpha"));
        assert!(contains_name("proj-alpha", "proj-alpha"));
        assert!(contains_name("~/code/reframe: bash", "reframe"));
        assert!(contains_name(
            "my project (old) - editor",
            "my project (old)"
        ));
        assert!(!contains_name("my-app — editor", "app"));
        assert!(!contains_name("api-gateway — editor", "api"));
        assert!(!contains_name("webapp — editor", "web"));
        assert!(!contains_name("anything", ""));
        // A later whole-name occurrence counts even if an earlier one is glued.
        assert!(contains_name("my-app — app — editor", "app"));
    }

    #[test]
    fn hinted_steps_are_satisfied_by_matching_windows() {
        let entry = ApplicationEntry::new("editor", "Editor", "test");
        let running = vec![
            window("w1", "notes.md — Proj-Alpha — Editor"),
            window("w2", "other — Editor"),
        ];
        let reuse = decide(
            &entry,
            &hints(&[Some(" proj-alpha "), Some("proj-beta")]),
            &running,
        );
        assert_eq!(reuse.satisfied, vec![true, false]);
        assert_eq!(ids(&reuse), vec!["w1"], "unrelated windows are left alone");
        assert!(!reuse.complete());
        assert_eq!(reuse.satisfied_count(), 1);

        let none = decide(&entry, &hints(&[Some("proj-beta")]), &running);
        assert_eq!(none.satisfied, vec![false]);
        assert!(none.windows.is_empty());
    }

    #[test]
    fn specific_hints_choose_first_and_windows_are_used_once() {
        let entry = ApplicationEntry::new("editor", "Editor", "test");
        // w2 is in front and contains "api" as a whole name too.
        let running = vec![
            window("w2", "api v2 — Editor"),
            window("w1", "api — Editor"),
        ];
        // "api v2" is longer, so it chooses first and "api" gets the other window.
        let reuse = decide(&entry, &hints(&[Some("api"), Some("api v2")]), &running);
        assert_eq!(reuse.satisfied, vec![true, true]);
        assert_eq!(ids(&reuse), vec!["w2", "w1"]);

        // Two steps with the same hint need two windows.
        let one = vec![window("w1", "api — Editor")];
        let reuse = decide(&entry, &hints(&[Some("api"), Some("api")]), &one);
        assert_eq!(reuse.satisfied, vec![true, false]);
    }

    #[test]
    fn steps_without_hints_use_the_per_application_rule() {
        let mut entry = ApplicationEntry::new("exe:mail", "Mail", "generic");
        let running = vec![
            window("w1", "Compose"),
            window("w2", "Inbox — Mail"),
            window("w3", "Calendar"),
        ];
        // No saved windows: the application counts as running and one window is taken over.
        let reuse = decide(&entry, &hints(&[None]), &running);
        assert!(reuse.complete());
        assert_eq!(ids(&reuse), vec!["w1"]);

        // Saved windows: the windows that fit them are taken over (best title first).
        entry.windows = vec![snapshot("Inbox — Mail")];
        let reuse = decide(&entry, &hints(&[None]), &running);
        assert_eq!(ids(&reuse), vec!["w2"]);

        // Nothing running: nothing is satisfied.
        let reuse = decide(&entry, &hints(&[None]), &[]);
        assert_eq!(reuse.satisfied, vec![false]);
        assert!(reuse.windows.is_empty() && !reuse.complete());
    }

    #[test]
    fn launch_plan_errors_fall_back_to_the_per_application_rule() {
        assert_eq!(step_hints(&Err("no executable".into())), vec![None]);
        assert_eq!(step_hints(&Ok(Vec::new())), vec![None]);
    }

    #[test]
    fn messages() {
        let projects = AppState::Projects {
            projects: Vec::new(),
        };
        let partial = Reuse {
            satisfied: vec![true, false],
            windows: Vec::new(),
        };
        assert_eq!(
            not_open_reason(&projects, &partial),
            "1 of 2 projects not open"
        );
        let single = Reuse {
            satisfied: vec![false],
            windows: Vec::new(),
        };
        assert_eq!(
            not_open_reason(&projects, &single),
            "Running, but this project isn't open"
        );
        assert_eq!(step_noun(&AppState::None, 2), "items");
        assert_eq!(
            step_noun(
                &AppState::Folders {
                    folders: Vec::new()
                },
                1
            ),
            "folder"
        );
    }
}
