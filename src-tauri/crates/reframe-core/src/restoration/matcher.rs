//! Matching running windows to workspace entries and saved window snapshots.

use reframe_platform::{WindowId, WindowInfo};

use crate::adapters::AdapterRegistry;
use crate::model::{ApplicationEntry, DesktopState, WindowSnapshot};

/// Running windows that belong to the same logical application as `entry`.
///
/// PLACEHOLDER until the restoration engine lands.
pub fn running_windows_for_entry(
    _entry: &ApplicationEntry,
    _state: &DesktopState,
    _adapters: &AdapterRegistry,
) -> Vec<WindowId> {
    Vec::new()
}

/// Assign candidate windows to saved snapshots (title similarity, class, hints). Returns
/// `(snapshot index, window id)` pairs; each window is used at most once.
///
/// PLACEHOLDER until the restoration engine lands.
pub fn assign_windows(
    _saved: &[WindowSnapshot],
    _candidates: &[WindowInfo],
    _title_hints: &[String],
) -> Vec<(usize, WindowId)> {
    Vec::new()
}
