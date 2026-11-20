//! Capturing selected applications into workspace entries.

use reframe_platform::{LaunchTarget, Platform};

use crate::adapters::AdapterRegistry;
use crate::discovery::DiscoveryOptions;
use crate::error::{CoreError, CoreResult};
use crate::model::{ApplicationEntry, CaptureResult, CaptureSelection, DesktopState};

/// Build entries for the selected applications/windows of a *fresh* desktop state (selections
/// reference `DiscoveredApplication::key` and window ids from a recent snapshot; windows that
/// no longer exist are skipped with a warning). Each entry gets adapter-captured state, launch
/// configuration and one `WindowSnapshot` per selected window.
///
/// PLACEHOLDER until the capture implementation lands.
pub fn capture_entries(
    _state: &DesktopState,
    _platform: &dyn Platform,
    _adapters: &AdapterRegistry,
    _selections: &[CaptureSelection],
    _options: &DiscoveryOptions,
) -> CoreResult<CaptureResult> {
    Err(CoreError::NotImplemented("Capturing applications"))
}

/// Entry for an application the user added by picking an executable / package / app bundle.
///
/// PLACEHOLDER until the capture implementation lands.
pub fn manual_entry(
    _platform: &dyn Platform,
    _adapters: &AdapterRegistry,
    _target: LaunchTarget,
) -> CoreResult<ApplicationEntry> {
    Err(CoreError::NotImplemented("Adding applications manually"))
}
