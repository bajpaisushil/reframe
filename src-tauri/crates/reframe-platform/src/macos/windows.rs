//! Window data sources: the CoreGraphics window list (any process, no permission needed except
//! for titles) and the Accessibility windows of an application (needs the Accessibility
//! permission). Interpretation happens in [`crate::macos_logic`].

use objc2_core_foundation::{CFDictionary, CFString};
use objc2_core_graphics::{
    CGWindowListCopyWindowInfo, CGWindowListOption, kCGNullWindowID, kCGWindowAlpha,
    kCGWindowBounds, kCGWindowIsOnscreen, kCGWindowLayer, kCGWindowName, kCGWindowNumber,
    kCGWindowOwnerName, kCGWindowOwnerPID,
};

use super::ax::{self, AxElement, AxResult, name};
use super::cf;
use crate::macos_logic::{
    self as logic, AxApp, AxWindow, CgWindow, PlistDict, WindowKey, ax_code, cg_key,
};
use crate::{PlatformError, PlatformResult, WindowId};

/// Window-info dictionaries for `option`, keyed by [`cg_key`] names; `None` when the window
/// server returns nothing (e.g. no GUI session).
fn window_dicts(option: CGWindowListOption, relative_to: u32) -> Option<Vec<PlistDict>> {
    let array = CGWindowListCopyWindowInfo(option, relative_to)?;
    // SAFETY: the `kCGWindow*` constants are immutable CFStrings exported by CoreGraphics.
    let keys: [(&CFString, &str); 8] = unsafe {
        [
            (kCGWindowNumber, cg_key::NUMBER),
            (kCGWindowOwnerPID, cg_key::OWNER_PID),
            (kCGWindowLayer, cg_key::LAYER),
            (kCGWindowAlpha, cg_key::ALPHA),
            (kCGWindowBounds, cg_key::BOUNDS),
            (kCGWindowName, cg_key::NAME),
            (kCGWindowOwnerName, cg_key::OWNER_NAME),
            (kCGWindowIsOnscreen, cg_key::IS_ONSCREEN),
        ]
    };
    Some(
        cf::array_items(&array)
            .iter()
            .filter_map(|item| item.downcast_ref::<CFDictionary>())
            .map(|dict| {
                keys.iter()
                    .filter_map(|(key, name)| {
                        let value = cf::dict_get(dict, key)?;
                        Some((name.to_string(), cf::plist_value(&value)?))
                    })
                    .collect()
            })
            .collect(),
    )
}

fn parse(dicts: Vec<PlistDict>) -> Vec<CgWindow> {
    dicts.iter().filter_map(CgWindow::from_dict).collect()
}

/// On-screen windows, front to back (desktop elements excluded).
pub(super) fn cg_on_screen() -> PlatformResult<Vec<CgWindow>> {
    window_dicts(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        kCGNullWindowID,
    )
    .map(parse)
    .ok_or_else(|| {
        PlatformError::os(
            "Listing windows",
            "the window server returned no window list (is Reframe running in a GUI session?)",
        )
    })
}

/// All windows, including off-screen ones (minimized windows, other Spaces, hidden apps).
pub(super) fn cg_all() -> Vec<CgWindow> {
    window_dicts(
        CGWindowListOption::OptionAll | CGWindowListOption::ExcludeDesktopElements,
        kCGNullWindowID,
    )
    .map(parse)
    .unwrap_or_default()
}

/// One window by number, whether on screen or not.
pub(super) fn cg_window(id: u32) -> Option<CgWindow> {
    window_dicts(CGWindowListOption::OptionIncludingWindow, id)
        .map(parse)?
        .into_iter()
        .find(|w| w.number == id)
}

/// Attributes read for every Accessibility window, in [`read_ax_window`] order.
const WINDOW_ATTRIBUTES: [&str; 6] = [
    name::TITLE,
    name::POSITION,
    name::SIZE,
    name::MINIMIZED,
    name::FULL_SCREEN,
    name::SUBROLE,
];

fn read_ax_window(element: &AxElement, focused: Option<&AxElement>) -> AxWindow {
    let values = element.attributes(&WINDOW_ATTRIBUTES).unwrap_or_default();
    let get = |index: usize| values.get(index).and_then(|v| v.as_deref());
    AxWindow {
        cg_id: element.window_id(),
        title: get(0).and_then(ax::to_string),
        frame: ax::to_frame(get(1), get(2)),
        minimized: get(3).and_then(ax::to_bool).unwrap_or(false),
        fullscreen: get(4).and_then(ax::to_bool).unwrap_or(false),
        focused: focused.is_some_and(|f| f.same_as(element)),
        subrole: get(5).and_then(ax::to_string),
    }
}

/// The Accessibility windows of the application with process id `pid`. `read_focus` also reads
/// its `AXFocusedWindow` (only meaningful for the frontmost application).
pub(super) fn ax_app(pid: u32, read_focus: bool) -> AxResult<AxApp> {
    let app = AxElement::application(pid).ok_or(ax_code::FAILURE)?;
    let elements = app.elements(name::WINDOWS)?;
    let (focused, focus_known) = if read_focus {
        match app.element(name::FOCUSED_WINDOW) {
            Ok(Some(window)) => (Some(window), true),
            // "No focused window" is only trustworthy from an application that exposes its
            // windows; otherwise the caller falls back to the topmost window.
            Ok(None) => (None, !elements.is_empty()),
            Err(_) => (None, false),
        }
    } else {
        (None, false)
    };
    Ok(AxApp {
        windows: elements
            .iter()
            .map(|element| read_ax_window(element, focused.as_ref()))
            .collect(),
        focus_known,
    })
}

/// A window found through both CoreGraphics and Accessibility.
pub(super) struct ResolvedWindow {
    pub(super) element: AxElement,
    pub(super) pid: u32,
}

/// Finds the Accessibility element of a window listed by [`crate::Platform::windows`].
///
/// The element whose CoreGraphics window number equals the id is used when macOS reports
/// numbers; otherwise the window is matched by geometry and title exactly as `windows()` does
/// (one-to-one with the application's other on-screen windows), so the same id always maps to
/// the same element.
pub(super) fn resolve(window: &WindowId, operation: &str) -> PlatformResult<ResolvedWindow> {
    let not_found = || PlatformError::not_found(format!("Window {window}"));
    let id = logic::parse_window_id(window).ok_or_else(not_found)?;
    let target = cg_window(id).ok_or_else(not_found)?;
    let app = AxElement::application(target.pid)
        .ok_or_else(|| PlatformError::os(operation, "the application has no valid process id"))?;
    let mut elements = app
        .elements(name::WINDOWS)
        .map_err(|code| logic::ax_error(operation, code))?;

    let index = match elements.iter().position(|e| e.window_id() == Some(id)) {
        Some(index) => Some(index),
        None => {
            let ax_windows: Vec<AxWindow> = elements
                .iter()
                .map(|element| read_ax_window(element, None))
                .collect();
            let mut peers: Vec<CgWindow> = cg_on_screen()
                .unwrap_or_default()
                .into_iter()
                .filter(|w| w.pid == target.pid && w.is_normal_window())
                .collect();
            if !peers.iter().any(|w| w.number == id) {
                peers.push(target.clone());
            }
            let keys: Vec<WindowKey<'_>> = peers.iter().map(CgWindow::key).collect();
            let assignment = logic::assign_windows(&keys, &ax_windows);
            peers
                .iter()
                .position(|w| w.number == id)
                .and_then(|i| assignment.get(i).copied().flatten())
        }
    };
    match index {
        Some(index) if index < elements.len() => Ok(ResolvedWindow {
            element: elements.swap_remove(index),
            pid: target.pid,
        }),
        _ => Err(PlatformError::os(
            operation,
            format!(
                "window {id} is not exposed through the Accessibility API (the application may \
                 not support Accessibility, or the window is on another Space)"
            ),
        )),
    }
}
