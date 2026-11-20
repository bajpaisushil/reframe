//! Monitor mapping and window geometry remapping between display setups.

use reframe_platform::{MonitorInfo, Rect};

use crate::model::{MonitorMapping, WindowSnapshot};

/// Map each saved monitor to a connected one (exact id match first, then same
/// resolution/position, then primary). Unmapped monitors fall back to the primary display.
///
/// PLACEHOLDER until the restoration engine lands.
pub fn map_monitors(saved: &[MonitorInfo], _current: &[MonitorInfo]) -> Vec<MonitorMapping> {
    saved
        .iter()
        .map(|m| MonitorMapping {
            saved_id: m.id.clone(),
            saved_name: m.name.clone(),
            mapped_id: None,
            mapped_name: None,
            exact: false,
        })
        .collect()
}

/// Where a saved window should go on the current monitors: unchanged if its monitor is
/// present, otherwise scaled proportionally into the mapped monitor's work area and clamped
/// so it is fully visible.
///
/// PLACEHOLDER until the restoration engine lands.
pub fn remap_bounds(
    window: &WindowSnapshot,
    _saved: &[MonitorInfo],
    _current: &[MonitorInfo],
) -> Rect {
    window.bounds
}
