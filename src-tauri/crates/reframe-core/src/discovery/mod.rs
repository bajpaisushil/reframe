//! Desktop discovery ("Explore"): turns raw platform data into applications with windows.

use chrono::Utc;
use reframe_platform::Platform;

use crate::adapters::AdapterRegistry;
use crate::model::{DesktopSnapshot, DesktopState};

/// Options for building a snapshot.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryOptions {
    /// App ids / executable names the user never wants suggested (case-insensitive).
    pub exclusions: Vec<String>,
    /// Reframe's own pid: its windows are never listed.
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
/// owning processes share an executable/package), identify adapters, classify
/// system/utility/application windows, apply exclusions and attach adapter previews.
///
/// PLACEHOLDER: returns no applications until the discovery implementation lands.
pub fn build_snapshot(
    state: &DesktopState,
    _platform: &dyn Platform,
    _adapters: &AdapterRegistry,
    _options: &DiscoveryOptions,
) -> DesktopSnapshot {
    let mut warnings = state.warnings.clone();
    warnings.push("Desktop discovery is not implemented yet.".to_string());
    DesktopSnapshot {
        captured_at: Utc::now(),
        platform: state.info.clone(),
        monitors: state.monitors.clone(),
        applications: Vec::new(),
        background_process_count: state.processes.len() as u32,
        warnings,
    }
}
