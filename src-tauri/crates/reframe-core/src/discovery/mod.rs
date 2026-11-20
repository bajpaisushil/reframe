//! Desktop discovery ("Explore"): turns raw platform data into applications with windows.
//!
//! [`gather`] reads monitors, windows and processes once. [`build_snapshot`] groups the windows
//! into logical applications (see [`group`] for the grouping keys), identifies the adapter of
//! each application, classifies it ([`classify`]), applies the user's exclusions and attaches the
//! adapter's preview of what it would capture. Capture ([`crate::capture`]) reuses the same
//! grouping, so the keys of a recent snapshot resolve to the same applications.
//!
//! Adapter code never takes discovery down: every adapter call is guarded ([`guard`]) and a
//! failing adapter is replaced by the generic adapter for the affected applications, with a
//! warning in the snapshot.

pub mod classify;
mod exclusions;
pub(crate) mod group;
pub(crate) mod guard;
pub(crate) mod names;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;

use std::collections::HashSet;

use chrono::Utc;
use reframe_platform::{Platform, PlatformInfo, SupportLevel};

use crate::adapters::{AdapterRegistry, CaptureContext, CaptureInput};
use crate::model::{
    AdapterCapability, AppCategory, DesktopSnapshot, DesktopState, DiscoveredApplication,
};
use exclusions::Exclusions;
use group::AppGroup;
use guard::AdapterIssues;

/// Options for building a snapshot.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryOptions {
    /// App ids / executable names the user never wants suggested (case-insensitive).
    pub exclusions: Vec<String>,
    /// Reframe's own pid: windows owned by this process are never listed. Its child processes
    /// (applications Reframe launched while restoring) are listed like any other. `0` means
    /// unknown.
    pub self_pid: u32,
    /// Allow adapters to read browser session files.
    pub browser_tab_capture: bool,
}

/// Read monitors, windows and processes once. Failures (unsupported operation, missing
/// permission) are recorded as warnings so partially supported platforms still work.
pub fn gather(platform: &dyn Platform) -> DesktopState {
    let info = platform.info();
    let mut warnings = Vec::new();
    let monitors = platform.monitors().unwrap_or_else(|e| {
        warnings.push(format!("Monitors: {e}"));
        Vec::new()
    });
    let windows = platform.windows().unwrap_or_else(|e| {
        warnings.push(format!("Windows: {e}"));
        Vec::new()
    });
    let processes = platform.processes().unwrap_or_else(|e| {
        warnings.push(format!("Processes: {e}"));
        Vec::new()
    });
    DesktopState {
        info,
        monitors,
        windows,
        processes,
        warnings,
    }
}

/// Group windows into applications (one entry per logical application: all windows whose
/// owning processes share a package/executable), identify adapters, classify
/// system/utility/application windows, apply exclusions and attach adapter previews.
///
/// - Reframe's own windows (owned by `options.self_pid`) are left out. Its descendants are not:
///   applications it launched during a restore stay its children and are listed.
/// - Applications are ordered by their frontmost window; windows inside an application are
///   front-to-back. The main process is the owner of the frontmost window.
/// - `suggested` = category [`AppCategory::Application`] and not excluded.
/// - `details` are the adapter's description of what it would capture right now (adapters may
///   read files such as browser sessions, so a scan is not free).
/// - `background_process_count` counts processes that own no listed window.
/// - Platform warnings are kept, plus a note when the platform cannot (fully) list windows.
pub fn build_snapshot(
    state: &DesktopState,
    platform: &dyn Platform,
    adapters: &AdapterRegistry,
    options: &DiscoveryOptions,
) -> DesktopSnapshot {
    let mut warnings = state.warnings.clone();
    if let Some(warning) = window_listing_warning(&state.info, &warnings) {
        warnings.push(warning);
    }

    let groups = group::group_windows(state, options);
    let exclusions = Exclusions::new(&options.exclusions);
    let ctx = CaptureContext {
        platform,
        info: &state.info,
        browser_tab_capture: options.browser_tab_capture,
    };
    let mut issues = AdapterIssues::default();
    let applications: Vec<DiscoveredApplication> = groups
        .iter()
        .map(|group| discover(group, state, &ctx, adapters, &exclusions, &mut issues))
        .collect();

    let window_owners: HashSet<u32> = groups.iter().flat_map(|g| g.pids.iter().copied()).collect();
    let background = state
        .processes
        .iter()
        .filter(|p| !window_owners.contains(&p.pid))
        .count();

    warnings.extend(issues.into_warnings());
    dedupe(&mut warnings);
    DesktopSnapshot {
        captured_at: Utc::now(),
        platform: state.info.clone(),
        monitors: state.monitors.clone(),
        applications,
        background_process_count: u32::try_from(background).unwrap_or(u32::MAX),
        warnings,
    }
}

/// Build the Explore entry of one application.
fn discover(
    group: &AppGroup<'_>,
    state: &DesktopState,
    ctx: &CaptureContext<'_>,
    adapters: &AdapterRegistry,
    exclusions: &Exclusions,
    issues: &mut AdapterIssues,
) -> DiscoveredApplication {
    let os = state.info.os;
    let main = group.main_process();
    let processes = group.processes();
    let identified = guard::identify(adapters, main, &group.windows, issues);
    let outcome = guard::with_fallback(
        adapters,
        identified,
        main,
        &group.windows,
        issues,
        |adapter, identity| {
            let capability = adapter.capability(ctx.info);
            let input = CaptureInput {
                identity,
                processes: processes.clone(),
                windows: group.windows.clone(),
                all_processes: &state.processes,
            };
            let output = adapter.capture(ctx, &input);
            for warning in &output.warnings {
                log::debug!("preview of {}: {warning}", identity.display_name);
            }
            (capability, adapter.describe(&output.state))
        },
    );
    let (capability, details) = outcome
        .value
        .unwrap_or_else(|| (AdapterCapability::launch_only(), Vec::new()));
    let identity = group.presented_identity(outcome.identity, outcome.generic);

    let category = classify::classify(os, &processes, &group.windows);
    let excluded = exclusions.matches(os, &identity.app_id, &processes);
    let known_main = group.known_main_process();
    DiscoveredApplication {
        key: group.key.clone(),
        app_id: identity.app_id,
        display_name: identity.display_name,
        adapter_id: outcome.adapter_id,
        capability,
        category,
        executable_path: known_main.and_then(|p| p.executable_path.clone()),
        package: known_main.and_then(|p| p.package.clone()),
        pids: group.pids.clone(),
        windows: group.windows.iter().map(|w| (*w).clone()).collect(),
        suggested: category == AppCategory::Application && !excluded,
        excluded,
        details,
    }
}

/// Explains a platform that cannot (fully) list windows, unless the platform already reported
/// the same reason while listing them.
fn window_listing_warning(info: &PlatformInfo, existing: &[String]) -> Option<String> {
    let capability = &info.capabilities.list_windows;
    let note = capability
        .note
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty());
    let message = match (capability.level, note) {
        (SupportLevel::Full, _) => return None,
        (SupportLevel::Unsupported, Some(note)) => {
            format!("Reframe cannot list open windows on this system: {note}")
        }
        (SupportLevel::Unsupported, None) => {
            "Reframe cannot list open windows on this system.".to_string()
        }
        (SupportLevel::Partial, Some(note)) => format!("Some open windows may be missing: {note}"),
        (SupportLevel::Partial, None) => {
            "Some open windows may be missing on this system.".to_string()
        }
    };
    if note.is_some_and(|note| existing.iter().any(|w| w.contains(note))) {
        return None;
    }
    Some(message)
}

/// Drop repeated warnings, keeping the first occurrence.
pub(crate) fn dedupe(warnings: &mut Vec<String>) {
    let mut seen = HashSet::new();
    warnings.retain(|w| seen.insert(w.clone()));
}
