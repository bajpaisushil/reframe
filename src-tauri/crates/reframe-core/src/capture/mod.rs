//! Capturing selected applications into workspace entries.
//!
//! Selections come from a recent Explore snapshot ([`crate::discovery::build_snapshot`]); they
//! are resolved against a *fresh* desktop state with the same grouping, so an application that
//! was closed, or windows that disappeared since the scan, are reported instead of captured.

mod manual;
mod plist;
#[cfg(test)]
mod tests;

pub use manual::manual_entry;

use std::collections::HashMap;

use reframe_platform::{Platform, ProcessInfo, WindowId, WindowInfo};

use crate::adapters::{AdapterRegistry, CaptureContext, CaptureInput, CaptureOutput};
use crate::discovery::group::{self, AppGroup};
use crate::discovery::guard::{self, AdapterIssues};
use crate::discovery::{self, DiscoveryOptions};
use crate::error::{CoreError, CoreResult};
use crate::model::{
    AppState, ApplicationEntry, CaptureResult, CaptureSelection, DesktopState, LaunchConfig,
    MAX_WINDOWS_PER_APP, WindowSnapshot,
};

/// Build entries for the selected applications/windows of a *fresh* desktop state (selections
/// reference `DiscoveredApplication::key` and window ids from a recent snapshot; windows that
/// no longer exist are skipped with a warning). Each entry gets adapter-captured state, launch
/// configuration and one `WindowSnapshot` per selected window.
///
/// - Entries follow the order of the selections; selecting an application twice yields one
///   entry with the union of the selected windows (an empty window list means all windows).
/// - The adapter sees only the selected windows and the processes owning them.
/// - Selections are explicit, so excluded and system applications are captured too.
/// - At most [`MAX_WINDOWS_PER_APP`] windows (the frontmost) are saved per application.
/// - Warnings: platform warnings of the fresh scan, applications that are gone, windows that
///   closed since the scan, adapter warnings (prefixed with the application name) and adapters
///   that failed (those applications fall back to the generic adapter).
pub fn capture_entries(
    state: &DesktopState,
    platform: &dyn Platform,
    adapters: &AdapterRegistry,
    selections: &[CaptureSelection],
    options: &DiscoveryOptions,
) -> CoreResult<CaptureResult> {
    if selections.is_empty() {
        return Err(CoreError::validation("Select at least one application."));
    }
    let groups = group::group_windows(state, options);
    let by_key: HashMap<&str, &AppGroup<'_>> = groups.iter().map(|g| (g.key.as_str(), g)).collect();

    let mut session = Session {
        state,
        ctx: CaptureContext {
            platform,
            info: &state.info,
            browser_tab_capture: options.browser_tab_capture,
            apple_events: true,
        },
        adapters,
        issues: AdapterIssues::default(),
        warnings: state.warnings.clone(),
    };
    let mut entries = Vec::new();
    for wanted in merge_selections(selections) {
        match by_key.get(wanted.key) {
            Some(group) => entries.push(session.capture(group, &wanted)),
            None => session.warnings.push(format!(
                "{} is no longer running.",
                group::key_label(wanted.key)
            )),
        }
    }

    let Session {
        issues,
        mut warnings,
        ..
    } = session;
    warnings.extend(issues.into_warnings());
    discovery::dedupe(&mut warnings);
    Ok(CaptureResult {
        entries,
        monitors: state.monitors.clone(),
        warnings,
    })
}

/// One application to capture after merging duplicate selections.
struct Wanted<'s> {
    key: &'s str,
    /// `None` = every window of the application.
    window_ids: Option<Vec<&'s WindowId>>,
}

/// Merge selections of the same application, keeping the order of first appearance.
fn merge_selections(selections: &[CaptureSelection]) -> Vec<Wanted<'_>> {
    let mut merged: Vec<Wanted<'_>> = Vec::new();
    for selection in selections {
        let ids = (!selection.window_ids.is_empty()).then(|| {
            let mut ids: Vec<&WindowId> = Vec::new();
            for id in &selection.window_ids {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            ids
        });
        match merged.iter_mut().find(|w| w.key == selection.app_key) {
            Some(existing) => {
                existing.window_ids = match (existing.window_ids.take(), ids) {
                    (Some(mut all), Some(more)) => {
                        for id in more {
                            if !all.contains(&id) {
                                all.push(id);
                            }
                        }
                        Some(all)
                    }
                    _ => None,
                };
            }
            None => merged.push(Wanted {
                key: &selection.app_key,
                window_ids: ids,
            }),
        }
    }
    merged
}

struct Session<'a> {
    state: &'a DesktopState,
    ctx: CaptureContext<'a>,
    adapters: &'a AdapterRegistry,
    issues: AdapterIssues,
    warnings: Vec<String>,
}

impl Session<'_> {
    fn capture(&mut self, group: &AppGroup<'_>, wanted: &Wanted<'_>) -> ApplicationEntry {
        let mut windows: Vec<&WindowInfo> = match &wanted.window_ids {
            None => group.windows.clone(),
            Some(ids) => group
                .windows
                .iter()
                .copied()
                .filter(|w| ids.contains(&&w.id))
                .collect(),
        };
        let closed = wanted
            .window_ids
            .as_ref()
            .map_or(0, |ids| ids.len().saturating_sub(windows.len()));
        let open = windows.len();
        windows.truncate(MAX_WINDOWS_PER_APP);
        let processes = group.processes_for(&windows);

        let main = group.main_process();
        let identified = guard::identify(self.adapters, main, &group.windows, &mut self.issues);
        let state = self.state;
        let ctx = &self.ctx;
        let outcome = guard::with_fallback(
            self.adapters,
            identified,
            main,
            &group.windows,
            &mut self.issues,
            |adapter, identity| {
                let input = CaptureInput {
                    identity,
                    processes: processes.clone(),
                    windows: windows.clone(),
                    all_processes: &state.processes,
                };
                adapter.capture(ctx, &input)
            },
        );
        let output = outcome
            .value
            .unwrap_or_else(|| minimal_output(group.known_main_process()));
        let identity = group.presented_identity(outcome.identity, outcome.generic);
        let name = identity.display_name;

        if closed > 0 {
            self.warnings.push(format!(
                "{name}: {closed} window{} closed since the scan{}.",
                if closed == 1 { "" } else { "s" },
                if windows.is_empty() {
                    "; only the application itself was saved"
                } else {
                    ""
                }
            ));
        }
        if open > MAX_WINDOWS_PER_APP {
            self.warnings.push(format!(
                "{name} has {open} windows; only the {MAX_WINDOWS_PER_APP} frontmost were saved."
            ));
        }
        for warning in &output.warnings {
            if let Some(warning) = prefixed(&name, warning) {
                self.warnings.push(warning);
            }
        }

        let mut entry = ApplicationEntry::new(identity.app_id, name, outcome.adapter_id);
        entry.launch = output.launch;
        entry.state = output.state;
        entry.windows = windows.iter().map(|w| window_snapshot(w)).collect();
        entry
    }
}

/// Launch-only result used when even the generic adapter failed.
fn minimal_output(main: Option<&ProcessInfo>) -> CaptureOutput {
    CaptureOutput {
        state: AppState::None,
        launch: LaunchConfig {
            executable: main.and_then(|p| p.executable_path.clone()),
            package: main.and_then(|p| p.package.clone()),
            ..LaunchConfig::default()
        },
        warnings: Vec::new(),
    }
}

/// `warning` prefixed with the application name unless it already starts with it.
fn prefixed(name: &str, warning: &str) -> Option<String> {
    let warning = warning.trim();
    if warning.is_empty() {
        return None;
    }
    let mentions_name = warning
        .strip_prefix(name)
        .is_some_and(|rest| rest.starts_with(':') || rest.starts_with(' '));
    Some(if mentions_name {
        warning.to_string()
    } else {
        format!("{name}: {warning}")
    })
}

/// The saved arrangement of one window: its normal (restored) bounds, state and monitor.
fn window_snapshot(window: &WindowInfo) -> WindowSnapshot {
    WindowSnapshot {
        title: window.title.clone(),
        class_name: window.class_name.clone(),
        bounds: window.restore_bounds(),
        state: window.state,
        monitor_id: window.monitor_id.clone(),
    }
}
