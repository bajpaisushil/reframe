//! Chromium-based browsers (Chrome, Edge, Brave, Chromium): windows and tabs from session files.
//!
//! PLACEHOLDER: does not claim any application yet (`identify` returns `None`), so these
//! applications are handled by the generic adapter until the dedicated integration lands.

use reframe_platform::{LocateQuery, PlatformInfo, ProcessInfo, WindowInfo};

use super::generic::GenericAdapter;
use super::{
    AppAdapter, AppIdentity, CaptureContext, CaptureInput, CaptureOutput, LaunchContext, LaunchStep,
};
use crate::model::{AdapterCapability, AppState, ApplicationEntry, StateDetail};

#[derive(Debug, Default)]
pub struct ChromiumAdapter {
    generic: GenericAdapter,
}

impl ChromiumAdapter {
    pub fn new() -> Self {
        Self {
            generic: GenericAdapter::new(),
        }
    }
}

impl AppAdapter for ChromiumAdapter {
    fn id(&self) -> &'static str {
        "chromium"
    }

    fn name(&self) -> &'static str {
        "Chrome & Edge"
    }

    fn identify(&self, _process: &ProcessInfo, _windows: &[&WindowInfo]) -> Option<AppIdentity> {
        None
    }

    fn capability(&self, info: &PlatformInfo) -> AdapterCapability {
        self.generic.capability(info)
    }

    fn capture(&self, ctx: &CaptureContext<'_>, input: &CaptureInput<'_>) -> CaptureOutput {
        self.generic.capture(ctx, input)
    }

    fn launch_plan(
        &self,
        ctx: &LaunchContext<'_>,
        entry: &ApplicationEntry,
    ) -> Result<Vec<LaunchStep>, String> {
        self.generic.launch_plan(ctx, entry)
    }

    fn describe(&self, state: &AppState) -> Vec<StateDetail> {
        self.generic.describe(state)
    }

    fn locate_query(&self, app_id: &str) -> LocateQuery {
        self.generic.locate_query(app_id)
    }
}
