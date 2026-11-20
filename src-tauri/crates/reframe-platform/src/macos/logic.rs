//! Pure logic of the macOS platform implementation.
//!
//! Nothing in this module calls into macOS. It works on plain Rust data gathered by
//! [`crate::macos`] (AppKit, CoreGraphics and the Accessibility API), so every decision can be
//! unit-tested on any operating system:
//!
//! - interpreting CoreGraphics window-info dictionaries ([`CgWindow::from_dict`]);
//! - converting AppKit rectangles (bottom-left origin) to the CoreGraphics global space
//!   (top-left origin of the main display) used everywhere else ([`appkit_to_cg`]);
//! - assembling [`MonitorInfo`]s from CoreGraphics displays and `NSScreen`s ([`build_monitors`]);
//! - matching Accessibility windows to CoreGraphics windows ([`assign_windows`]);
//! - detecting "maximized" windows ([`is_maximized`]) and building the window list
//!   ([`assemble_windows`]);
//! - planning window placements ([`plan_placement`]);
//! - building `/usr/bin/open` command lines, locating `.app` bundles, wording permission
//!   instructions and capability notes.
//!
//! `lib.rs` compiles this file for macOS builds and, for unit tests, on every other OS.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::{
    Capability, MonitorInfo, PackageInfo, PackageKind, PermissionKind, PermissionStatus,
    PlatformCapabilities, PlatformError, ProcessInfo, Rect, WindowId, WindowInfo, WindowPlacement,
    WindowState,
};

// ---------------------------------------------------------------------------------------------
// Property-list values
// ---------------------------------------------------------------------------------------------

/// Plain-data copy of a CoreFoundation property-list value, covering the value types found in
/// CoreGraphics window-info dictionaries.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PlistValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Dict(PlistDict),
}

/// Plain-data copy of a `CFDictionary` with string keys.
pub(crate) type PlistDict = BTreeMap<String, PlistValue>;

impl PlistValue {
    /// Numeric value as a float (non-finite floats are rejected).
    pub(crate) fn as_f64(&self) -> Option<f64> {
        match *self {
            PlistValue::Int(v) => Some(v as f64),
            PlistValue::Float(v) if v.is_finite() => Some(v),
            _ => None,
        }
    }

    /// Integer value; floats are accepted when they are whole numbers.
    pub(crate) fn as_i64(&self) -> Option<i64> {
        match *self {
            PlistValue::Int(v) => Some(v),
            PlistValue::Float(v) if v.is_finite() && v.fract() == 0.0 => {
                // `as` saturates at the i64 bounds, which is fine for out-of-range values:
                // callers range-check the result for their own type.
                Some(v as i64)
            }
            _ => None,
        }
    }

    /// Boolean value; CoreFoundation sometimes stores flags as numbers.
    pub(crate) fn as_bool(&self) -> Option<bool> {
        match *self {
            PlistValue::Bool(v) => Some(v),
            PlistValue::Int(v) => Some(v != 0),
            _ => None,
        }
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            PlistValue::String(s) => Some(s),
            _ => None,
        }
    }

    pub(crate) fn as_dict(&self) -> Option<&PlistDict> {
        match self {
            PlistValue::Dict(d) => Some(d),
            _ => None,
        }
    }
}

/// `Some(s)` when `s` contains something other than whitespace.
fn non_blank(s: Option<&str>) -> Option<&str> {
    s.filter(|s| !s.trim().is_empty())
}

// ---------------------------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------------------------

/// Rectangle in floating-point points. Unless stated otherwise it is in the CoreGraphics global
/// display space: origin at the top-left corner of the main display, y growing downwards.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct FRect {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
}

impl FRect {
    pub(crate) const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub(crate) fn from_rect(rect: Rect) -> Self {
        Self::new(
            f64::from(rect.x),
            f64::from(rect.y),
            f64::from(rect.width),
            f64::from(rect.height),
        )
    }

    /// All components finite and the size non-negative.
    pub(crate) fn is_valid(&self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
            && self.width >= 0.0
            && self.height >= 0.0
    }

    /// Valid and with a positive area.
    pub(crate) fn has_area(&self) -> bool {
        self.is_valid() && self.width > 0.0 && self.height > 0.0
    }

    /// Rounds every component to whole points (saturating at the `i32` range).
    pub(crate) fn to_rect(self) -> Rect {
        Rect::new(
            round_to_i32(self.x),
            round_to_i32(self.y),
            round_to_i32(self.width),
            round_to_i32(self.height),
        )
    }

    /// Origin and size each differ by at most `tolerance` points.
    pub(crate) fn approx_eq(&self, other: &FRect, tolerance: f64) -> bool {
        (self.x - other.x).abs() <= tolerance
            && (self.y - other.y).abs() <= tolerance
            && (self.width - other.width).abs() <= tolerance
            && (self.height - other.height).abs() <= tolerance
    }

    /// Parses a CoreGraphics rectangle dictionary (`{X, Y, Width, Height}`, the format of
    /// `kCGWindowBounds` and `CGRectCreateDictionaryRepresentation`).
    pub(crate) fn from_bounds_dict(dict: &PlistDict) -> Option<FRect> {
        let get = |key: &str| dict.get(key).and_then(PlistValue::as_f64);
        let rect = FRect::new(get("X")?, get("Y")?, get("Width")?, get("Height")?);
        rect.is_valid().then_some(rect)
    }
}

fn round_to_i32(value: f64) -> i32 {
    if value.is_nan() {
        0
    } else {
        // Float-to-int `as` casts saturate, so out-of-range values clamp to the i32 bounds.
        value.round() as i32
    }
}

/// Converts an AppKit rectangle (origin at the bottom-left corner of the main screen, y growing
/// upwards, as used by `NSScreen.frame`/`visibleFrame`) to the CoreGraphics global space.
///
/// `main_screen_height` is the height of the screen whose AppKit frame starts at the origin:
/// the display with the menu bar, i.e. `CGMainDisplayID()` / `NSScreen.screens[0]`.
pub(crate) fn appkit_to_cg(rect: FRect, main_screen_height: f64) -> FRect {
    FRect::new(
        rect.x,
        main_screen_height - (rect.y + rect.height),
        rect.width,
        rect.height,
    )
}

// ---------------------------------------------------------------------------------------------
// CoreGraphics window list
// ---------------------------------------------------------------------------------------------

/// Keys of the CoreGraphics window-info dictionaries read by this implementation. The macOS side
/// looks values up with the framework's own `kCGWindow*` constants and stores them under these
/// names (which are also the constants' string values).
pub(crate) mod cg_key {
    pub(crate) const NUMBER: &str = "kCGWindowNumber";
    pub(crate) const OWNER_PID: &str = "kCGWindowOwnerPID";
    pub(crate) const LAYER: &str = "kCGWindowLayer";
    pub(crate) const ALPHA: &str = "kCGWindowAlpha";
    pub(crate) const BOUNDS: &str = "kCGWindowBounds";
    pub(crate) const NAME: &str = "kCGWindowName";
    pub(crate) const OWNER_NAME: &str = "kCGWindowOwnerName";
    pub(crate) const IS_ONSCREEN: &str = "kCGWindowIsOnscreen";
}

/// Smallest width and height (points) of an application window. Any titled window is at least
/// 28 points tall; smaller layer-0 windows are invisible helper windows some applications keep
/// on screen (often 1x1).
pub(crate) const MIN_WINDOW_SIZE: f64 = 10.0;

/// One validated entry of `CGWindowListCopyWindowInfo`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CgWindow {
    /// `kCGWindowNumber` (the `CGWindowID`), never 0.
    pub(crate) number: u32,
    pub(crate) pid: u32,
    pub(crate) layer: i64,
    pub(crate) alpha: f64,
    pub(crate) bounds: FRect,
    /// `kCGWindowName`. macOS only provides other applications' window titles when Reframe has
    /// the Screen Recording permission; blank titles are stored as `None`.
    pub(crate) title: Option<String>,
    pub(crate) owner_name: Option<String>,
    pub(crate) on_screen: bool,
}

impl CgWindow {
    /// Interprets one window-info dictionary. Returns `None` when a required key (number, owner
    /// pid, layer, bounds) is missing or malformed; optional keys fall back to safe defaults.
    pub(crate) fn from_dict(dict: &PlistDict) -> Option<CgWindow> {
        let number = dict
            .get(cg_key::NUMBER)?
            .as_i64()
            .and_then(|n| u32::try_from(n).ok())
            .filter(|&n| n != 0)?;
        let pid = dict
            .get(cg_key::OWNER_PID)?
            .as_i64()
            .and_then(|p| u32::try_from(p).ok())
            .filter(|&p| p != 0)?;
        let layer = dict.get(cg_key::LAYER)?.as_i64()?;
        let bounds = dict
            .get(cg_key::BOUNDS)?
            .as_dict()
            .and_then(FRect::from_bounds_dict)?;
        let alpha = match dict.get(cg_key::ALPHA) {
            Some(value) => value.as_f64()?,
            None => 1.0,
        };
        let text =
            |key: &str| non_blank(dict.get(key).and_then(PlistValue::as_str)).map(str::to_string);
        Some(CgWindow {
            number,
            pid,
            layer,
            alpha,
            bounds,
            title: text(cg_key::NAME),
            owner_name: text(cg_key::OWNER_NAME),
            on_screen: dict
                .get(cg_key::IS_ONSCREEN)
                .and_then(PlistValue::as_bool)
                .unwrap_or(false),
        })
    }

    /// Whether the entry can be a normal application window: on the normal window layer (0;
    /// menus, the Dock, panels and overlays use higher layers), not fully transparent and at
    /// least [`MIN_WINDOW_SIZE`] points in both directions.
    pub(crate) fn is_normal_window(&self) -> bool {
        self.layer == 0
            && self.alpha > 0.0
            && self.bounds.width >= MIN_WINDOW_SIZE
            && self.bounds.height >= MIN_WINDOW_SIZE
    }

    pub(crate) fn key(&self) -> WindowKey<'_> {
        WindowKey {
            number: self.number,
            title: self.title.as_deref(),
            bounds: self.bounds,
            on_screen: self.on_screen,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Monitors
// ---------------------------------------------------------------------------------------------

/// A display as reported by CoreGraphics.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DisplayRecord {
    /// `CGDirectDisplayID` (only stable while the display stays connected).
    pub(crate) id: u32,
    /// `CGDisplayBounds`, in the CoreGraphics global space.
    pub(crate) bounds: FRect,
    /// `CGDisplayCreateUUIDFromDisplayID`, stable across reboots.
    pub(crate) uuid: Option<String>,
    /// `CGMainDisplayID()`: the display with the menu bar, origin of the global space.
    pub(crate) is_main: bool,
    pub(crate) is_builtin: bool,
    /// Pixel width / point width of the current display mode.
    pub(crate) mode_scale: Option<f64>,
}

/// An `NSScreen`, copied on the main thread (AppKit only allows that there).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ScreenRecord {
    /// `deviceDescription["NSScreenNumber"]`: the CoreGraphics display id.
    pub(crate) display_id: Option<u32>,
    /// `localizedName`, e.g. "Built-in Retina Display".
    pub(crate) name: Option<String>,
    /// `frame`, AppKit coordinates.
    pub(crate) frame: FRect,
    /// `visibleFrame` (without menu bar and Dock), AppKit coordinates.
    pub(crate) visible_frame: FRect,
    /// `backingScaleFactor`.
    pub(crate) scale: f64,
}

/// Tolerance (points) when matching an `NSScreen` frame to a CoreGraphics display.
const SCREEN_MATCH_TOLERANCE: f64 = 1.0;

/// Combines CoreGraphics displays with the matching `NSScreen`s (when AppKit could be queried)
/// into [`MonitorInfo`]s, in CoreGraphics order (main display first).
///
/// Without screen data the work area falls back to the full display bounds, the name to a
/// generic label and the scale factor to the display mode's pixel density.
///
/// Monitor ids are the display UUIDs. Identical displays without serial numbers can share a UUID
/// (macOS derives it from vendor, model and serial number); ids must still be unique, so a shared
/// UUID gets the display's origin appended (stable while the arrangement is), and a display that
/// would still collide (mirrored at the same origin) gets its CoreGraphics display id.
pub(crate) fn build_monitors(
    displays: &[DisplayRecord],
    screens: &[ScreenRecord],
) -> Vec<MonitorInfo> {
    let mut uuid_counts: HashMap<&str, usize> = HashMap::new();
    for uuid in displays.iter().filter_map(|d| non_blank(d.uuid.as_deref())) {
        *uuid_counts.entry(uuid).or_default() += 1;
    }
    let mut used_ids: HashSet<String> = HashSet::new();
    let main_height = displays
        .iter()
        .find(|d| d.is_main)
        .or_else(|| displays.first())
        .map(|d| d.bounds.height)
        .or_else(|| {
            screens
                .iter()
                .find(|s| s.frame.x == 0.0 && s.frame.y == 0.0)
                .map(|s| s.frame.height)
        });

    displays
        .iter()
        .enumerate()
        .map(|(index, display)| {
            let screen = screens
                .iter()
                .find(|s| s.display_id == Some(display.id))
                .or_else(|| {
                    let main_height = main_height?;
                    screens.iter().find(|s| {
                        appkit_to_cg(s.frame, main_height)
                            .approx_eq(&display.bounds, SCREEN_MATCH_TOLERANCE)
                    })
                });
            let bounds = display.bounds.to_rect();
            let work_area = screen
                .zip(main_height)
                .map(|(s, h)| appkit_to_cg(s.visible_frame, h))
                .filter(FRect::has_area)
                .map(FRect::to_rect)
                // Inconsistent data (e.g. the arrangement changed between the two reads).
                .filter(|area| bounds.intersection(area).is_some())
                .unwrap_or(bounds);
            let name = non_blank(screen.and_then(|s| s.name.as_deref()))
                .map(|n| n.trim().to_string())
                .unwrap_or_else(|| {
                    if display.is_builtin {
                        "Built-in Display".to_string()
                    } else {
                        format!("Display {}", index + 1)
                    }
                });
            let positive = |v: &f64| v.is_finite() && *v > 0.0;
            let scale_factor = screen
                .map(|s| s.scale)
                .filter(positive)
                .or(display.mode_scale.filter(positive))
                .unwrap_or(1.0);
            let display_id = || format!("cg-display-{}", display.id);
            let id = match non_blank(display.uuid.as_deref()) {
                Some(uuid) if uuid_counts.get(uuid).is_some_and(|&n| n > 1) => {
                    format!("{uuid}@{},{}", bounds.x, bounds.y)
                }
                Some(uuid) => uuid.to_string(),
                None => display_id(),
            };
            let id = if used_ids.insert(id.clone()) {
                id
            } else {
                display_id()
            };
            MonitorInfo {
                id,
                name,
                bounds,
                work_area,
                scale_factor,
                is_primary: display.is_main,
            }
        })
        .collect()
}

/// The monitor showing the largest part of `bounds`; `None` when it is on no monitor at all.
pub(crate) fn monitor_for<'a>(
    bounds: &Rect,
    monitors: &'a [MonitorInfo],
) -> Option<&'a MonitorInfo> {
    monitors
        .iter()
        .map(|m| (m, m.bounds.intersection_area(bounds)))
        .filter(|(_, area)| *area > 0)
        // `max_by_key` keeps the last maximum; reverse so ties go to the first monitor.
        .rev()
        .max_by_key(|(_, area)| *area)
        .map(|(m, _)| m)
}

/// The monitor a window with saved `bounds` should be placed on: the one showing most of it,
/// else the one nearest to its centre (the saved monitor may be disconnected), else the primary.
pub(crate) fn target_monitor<'a>(
    bounds: &Rect,
    monitors: &'a [MonitorInfo],
) -> Option<&'a MonitorInfo> {
    monitor_for(bounds, monitors)
        .or_else(|| {
            let (cx, cy) = bounds.center();
            // `min_by_key` keeps the first minimum, so ties go to the first monitor.
            monitors
                .iter()
                .min_by_key(|m| distance_sq_to_rect(cx, cy, &m.bounds))
        })
        .or_else(|| monitors.iter().find(|m| m.is_primary))
}

fn distance_sq_to_rect(x: i32, y: i32, rect: &Rect) -> i64 {
    let axis = |v: i32, lo: i32, hi: i32| -> i64 {
        let (v, lo, hi) = (i64::from(v), i64::from(lo), i64::from(hi));
        if v < lo {
            lo - v
        } else if v > hi {
            v - hi
        } else {
            0
        }
    };
    let dx = axis(x, rect.x, rect.right());
    let dy = axis(y, rect.y, rect.bottom());
    dx * dx + dy * dy
}

/// Tolerance (points) for [`is_maximized`].
pub(crate) const MAXIMIZED_TOLERANCE: i64 = 2;

/// macOS has no "maximized" window state. Reframe reports a window as maximized when its frame
/// fills its monitor's usable area (`NSScreen.visibleFrame`: without menu bar and Dock), which is
/// what the "Zoom" / "Fill" commands produce.
pub(crate) fn is_maximized(bounds: &Rect, work_area: &Rect) -> bool {
    let close = |a: i32, b: i32| (i64::from(a) - i64::from(b)).abs() <= MAXIMIZED_TOLERANCE;
    !work_area.is_empty()
        && close(bounds.x, work_area.x)
        && close(bounds.y, work_area.y)
        && close(bounds.width, work_area.width)
        && close(bounds.height, work_area.height)
}

// ---------------------------------------------------------------------------------------------
// Accessibility windows and matching
// ---------------------------------------------------------------------------------------------

/// A window as reported by the Accessibility API (an element of an application's `AXWindows`).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct AxWindow {
    /// CoreGraphics window number of the element, when macOS reports it.
    pub(crate) cg_id: Option<u32>,
    pub(crate) title: Option<String>,
    /// `AXPosition` + `AXSize` (CoreGraphics global space). Minimized windows report the frame
    /// they will return to.
    pub(crate) frame: Option<FRect>,
    pub(crate) minimized: bool,
    pub(crate) fullscreen: bool,
    /// The application's `AXFocusedWindow`.
    pub(crate) focused: bool,
    pub(crate) subrole: Option<String>,
}

impl AxWindow {
    /// Floating panels and palettes are not application windows.
    pub(crate) fn is_panel(&self) -> bool {
        matches!(
            self.subrole.as_deref(),
            Some("AXFloatingWindow" | "AXSystemFloatingWindow")
        )
    }
}

/// The Accessibility view of one application's windows.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct AxApp {
    /// `AXWindows`, in the order the application reports them (normally front to back).
    pub(crate) windows: Vec<AxWindow>,
    /// Whether `AXFocusedWindow` was read (successfully, or as "no focused window").
    pub(crate) focus_known: bool,
}

/// The parts of a CoreGraphics window used to find its Accessibility counterpart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WindowKey<'a> {
    pub(crate) number: u32,
    pub(crate) title: Option<&'a str>,
    pub(crate) bounds: FRect,
    /// `kCGWindowIsOnscreen`: an on-screen window cannot be a minimized Accessibility window.
    pub(crate) on_screen: bool,
}

/// Position/size tolerance (points) when matching windows by geometry.
pub(crate) const MATCH_TOLERANCE: f64 = 2.0;

/// How well an Accessibility window matches a CoreGraphics window by geometry and title:
/// `None` when the frames differ by more than [`MATCH_TOLERANCE`]; otherwise 3 for an identical
/// title, 2 when a title is unknown on either side, 1 when both titles are known but differ
/// (e.g. the title changed between the two reads).
pub(crate) fn match_score(key: &WindowKey<'_>, ax: &AxWindow) -> Option<u8> {
    let frame = ax.frame?;
    if !frame.approx_eq(&key.bounds, MATCH_TOLERANCE) {
        return None;
    }
    Some(
        match (non_blank(key.title), non_blank(ax.title.as_deref())) {
            (Some(a), Some(b)) if a == b => 3,
            (Some(_), Some(_)) => 1,
            _ => 2,
        },
    )
}

/// One-to-one assignment of Accessibility windows to CoreGraphics windows of one application.
///
/// Windows whose CoreGraphics number is known on the Accessibility side are paired exactly;
/// the rest are paired greedily by [`match_score`] (best score first, then CoreGraphics z-order,
/// then Accessibility order, which is normally also front to back), never pairing a minimized
/// Accessibility window with an on-screen CoreGraphics window. Returns, for every key, the index
/// of its Accessibility window.
pub(crate) fn assign_windows(keys: &[WindowKey<'_>], ax: &[AxWindow]) -> Vec<Option<usize>> {
    let mut result: Vec<Option<usize>> = vec![None; keys.len()];
    let mut used = vec![false; ax.len()];

    for (ki, key) in keys.iter().enumerate() {
        if let Some(ai) = ax
            .iter()
            .enumerate()
            .position(|(ai, w)| !used[ai] && w.cg_id == Some(key.number))
        {
            result[ki] = Some(ai);
            used[ai] = true;
        }
    }

    let mut pairs: Vec<(u8, usize, usize)> = Vec::new();
    for (ki, key) in keys.iter().enumerate() {
        if result[ki].is_some() {
            continue;
        }
        for (ai, window) in ax.iter().enumerate() {
            // A window whose number is known belongs to a CoreGraphics window that is not in
            // `keys`; it must not be paired by geometry with another one. A minimized window
            // keeps its frame, so it can match an on-screen window's geometry exactly.
            if used[ai] || window.cg_id.is_some() || (key.on_screen && window.minimized) {
                continue;
            }
            if let Some(score) = match_score(key, window) {
                pairs.push((score, ki, ai));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    for (_, ki, ai) in pairs {
        if result[ki].is_none() && !used[ai] {
            result[ki] = Some(ai);
            used[ai] = true;
        }
    }
    result
}

// ---------------------------------------------------------------------------------------------
// Window list assembly
// ---------------------------------------------------------------------------------------------

/// A running application (`NSRunningApplication`), as plain data.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct AppRecord {
    pub(crate) pid: u32,
    pub(crate) bundle_id: Option<String>,
    /// `localizedName`.
    pub(crate) name: Option<String>,
    /// `bundleURL` as a path.
    pub(crate) bundle_path: Option<PathBuf>,
    /// Activation policy "regular": the application appears in the Dock and has a menu bar.
    pub(crate) regular: bool,
    pub(crate) hidden: bool,
}

/// Everything [`assemble_windows`] needs, gathered by the macOS side.
#[derive(Debug, Clone, Copy)]
pub(crate) struct WindowSources<'a> {
    /// `CGWindowListCopyWindowInfo(OnScreenOnly | ExcludeDesktopElements)`, front to back.
    pub(crate) on_screen: &'a [CgWindow],
    /// Off-screen windows (`OptionAll`), used to find the window numbers of minimized windows
    /// when the Accessibility API does not report them. May be empty.
    pub(crate) off_screen: &'a [CgWindow],
    pub(crate) apps: &'a [AppRecord],
    /// Accessibility data per pid; only applications that could be queried.
    pub(crate) ax: &'a HashMap<u32, AxApp>,
    pub(crate) monitors: &'a [MonitorInfo],
    pub(crate) frontmost_pid: Option<u32>,
}

/// Builds the [`WindowInfo`] list (front to back) from CoreGraphics, AppKit and Accessibility
/// data:
///
/// - on-screen windows that pass [`CgWindow::is_normal_window`] (layer 0, visible, not tiny),
///   owned by "regular" (Dock) applications; floating panels identified through Accessibility
///   are dropped, and so are windows outside every monitor (helper windows parked off screen)
///   when monitors are known;
/// - titles from CoreGraphics, falling back to the Accessibility title;
/// - state from Accessibility (minimized, full screen), "maximized" when the frame fills the
///   monitor's usable area ([`is_maximized`]);
/// - minimized windows, which only the Accessibility API reports, appended after the on-screen
///   windows (a minimized window whose CoreGraphics number cannot be determined is skipped);
/// - the focused window from the frontmost application's `AXFocusedWindow`, or, when that is
///   unknown, the frontmost application's topmost window.
pub(crate) fn assemble_windows(src: &WindowSources<'_>) -> Vec<WindowInfo> {
    let apps: HashMap<u32, &AppRecord> = src
        .apps
        .iter()
        .filter(|a| a.regular)
        .map(|a| (a.pid, a))
        .collect();

    let mut seen_numbers = HashSet::new();
    let visible = |w: &CgWindow| {
        src.monitors.is_empty() || monitor_for(&w.bounds.to_rect(), src.monitors).is_some()
    };
    let candidates: Vec<&CgWindow> = src
        .on_screen
        .iter()
        .filter(|w| w.is_normal_window() && apps.contains_key(&w.pid) && visible(w))
        .filter(|w| seen_numbers.insert(w.number))
        .collect();

    // Pair on-screen windows with Accessibility windows, per application.
    let mut matched: Vec<Option<&AxWindow>> = vec![None; candidates.len()];
    let mut ax_used: HashMap<u32, Vec<bool>> = HashMap::new();
    let pids: BTreeSet<u32> = candidates.iter().map(|w| w.pid).collect();
    for pid in &pids {
        let Some(ax_app) = src.ax.get(pid) else {
            continue;
        };
        let indices: Vec<usize> = (0..candidates.len())
            .filter(|&i| candidates[i].pid == *pid)
            .collect();
        let keys: Vec<WindowKey<'_>> = indices.iter().map(|&i| candidates[i].key()).collect();
        let mut used = vec![false; ax_app.windows.len()];
        for (k, assigned) in assign_windows(&keys, &ax_app.windows)
            .into_iter()
            .enumerate()
        {
            if let Some(ai) = assigned {
                matched[indices[k]] = Some(&ax_app.windows[ai]);
                used[ai] = true;
            }
        }
        ax_used.insert(*pid, used);
    }

    let mut out: Vec<WindowInfo> = Vec::new();
    let mut used_numbers: HashSet<u32> = HashSet::new();
    for (cg, ax) in candidates.iter().zip(matched.iter().copied()) {
        let Some(app) = apps.get(&cg.pid) else {
            continue;
        };
        if ax.is_some_and(AxWindow::is_panel) {
            continue;
        }
        let bounds = cg.bounds.to_rect();
        let monitor = monitor_for(&bounds, src.monitors);
        let state = match ax {
            Some(a) if a.fullscreen => WindowState::Fullscreen,
            Some(a) if a.minimized => WindowState::Minimized,
            _ if monitor.is_some_and(|m| is_maximized(&bounds, &m.work_area)) => {
                WindowState::Maximized
            }
            _ => WindowState::Normal,
        };
        let title = non_blank(cg.title.as_deref())
            .or_else(|| non_blank(ax.and_then(|a| a.title.as_deref())))
            .unwrap_or_default()
            .to_string();
        used_numbers.insert(cg.number);
        out.push(window_info(
            cg.number,
            cg.pid,
            title,
            app,
            bounds,
            state,
            monitor,
            ax.is_some_and(|a| a.focused),
            out.len(),
        ));
    }

    // Minimized windows: not on screen, so only the Accessibility API knows about them.
    let ax_pids: BTreeSet<u32> = src.ax.keys().copied().collect();
    for pid in ax_pids {
        let (Some(app), Some(ax_app)) = (apps.get(&pid), src.ax.get(&pid)) else {
            continue;
        };
        let used = ax_used.get(&pid);
        let minimized: Vec<&AxWindow> = ax_app
            .windows
            .iter()
            .enumerate()
            .filter(|(i, w)| {
                w.minimized && !w.is_panel() && !used.is_some_and(|u| u.get(*i) == Some(&true))
            })
            .map(|(_, w)| w)
            .collect();
        if minimized.is_empty() {
            continue;
        }
        let off_screen: Vec<&CgWindow> = src
            .off_screen
            .iter()
            .filter(|w| w.pid == pid && w.layer == 0 && !w.on_screen)
            .filter(|w| !used_numbers.contains(&w.number))
            .collect();
        let keys: Vec<WindowKey<'_>> = off_screen.iter().map(|w| w.key()).collect();
        let ax_list: Vec<AxWindow> = minimized.iter().map(|w| (*w).clone()).collect();
        let mut cg_for_ax: Vec<Option<&CgWindow>> = vec![None; ax_list.len()];
        for (k, assigned) in assign_windows(&keys, &ax_list).into_iter().enumerate() {
            if let Some(ai) = assigned {
                cg_for_ax[ai] = Some(off_screen[k]);
            }
        }
        for (window, cg) in ax_list.iter().zip(cg_for_ax) {
            let Some(number) = window.cg_id.or(cg.map(|c| c.number)) else {
                continue; // cannot be addressed later without its window number
            };
            if used_numbers.contains(&number) {
                continue;
            }
            let cg = cg.or_else(|| src.off_screen.iter().find(|c| c.number == number));
            let Some(frame) = window
                .frame
                .filter(FRect::has_area)
                .or_else(|| cg.map(|c| c.bounds).filter(FRect::has_area))
            else {
                continue;
            };
            let bounds = frame.to_rect();
            let title = non_blank(window.title.as_deref())
                .or_else(|| non_blank(cg.and_then(|c| c.title.as_deref())))
                .unwrap_or_default()
                .to_string();
            used_numbers.insert(number);
            out.push(window_info(
                number,
                pid,
                title,
                app,
                bounds,
                WindowState::Minimized,
                monitor_for(&bounds, src.monitors),
                false,
                out.len(),
            ));
        }
    }

    // Exactly one focused window at most; fall back to the frontmost application's topmost
    // window when its Accessibility focus is unknown.
    let mut focus_seen = false;
    for window in &mut out {
        if window.is_focused {
            window.is_focused = !focus_seen;
            focus_seen = true;
        }
    }
    if !focus_seen && let Some(front) = src.frontmost_pid {
        let focus_known = src.ax.get(&front).is_some_and(|a| a.focus_known);
        if !focus_known
            && let Some(window) = out
                .iter_mut()
                .find(|w| w.pid == front && w.state != WindowState::Minimized)
        {
            window.is_focused = true;
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn window_info(
    number: u32,
    pid: u32,
    title: String,
    app: &AppRecord,
    bounds: Rect,
    state: WindowState,
    monitor: Option<&MonitorInfo>,
    is_focused: bool,
    z_order: usize,
) -> WindowInfo {
    WindowInfo {
        id: WindowId::new(number.to_string()),
        pid,
        title,
        class_name: app.bundle_id.clone(),
        app_id_hint: app.bundle_id.clone(),
        bounds,
        normal_bounds: None,
        state,
        monitor_id: monitor.map(|m| m.id.clone()),
        desktop: None,
        is_focused,
        z_order: u32::try_from(z_order).unwrap_or(u32::MAX),
    }
}

/// Parses a [`WindowId`] produced by this implementation (a decimal `CGWindowID`).
pub(crate) fn parse_window_id(id: &WindowId) -> Option<u32> {
    id.as_str().trim().parse::<u32>().ok().filter(|&n| n != 0)
}

// ---------------------------------------------------------------------------------------------
// Window placement
// ---------------------------------------------------------------------------------------------

/// The current state of the window being placed, read through Accessibility.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct CurrentWindowState {
    pub(crate) minimized: bool,
    pub(crate) fullscreen: bool,
    pub(crate) frame: Option<FRect>,
}

/// One Accessibility operation of a placement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum PlacementStep {
    /// Set `AXFullScreen` to false and wait for the transition to finish.
    ExitFullscreen,
    /// Set `AXMinimized` to false and wait for the window to come back.
    Unminimize,
    /// Set `AXPosition`, `AXSize`, then `AXPosition` again (moving first lets the size fit the
    /// destination screen; the second move corrects positions the system clamped at screen
    /// edges). `best_effort` steps ignore failures (the window keeps its requested state anyway).
    SetFrame { frame: FRect, best_effort: bool },
    /// Set `AXFullScreen` to true.
    EnterFullscreen,
    /// Set `AXMinimized` to true.
    Minimize,
}

/// Plans the Accessibility operations that give a window `placement`.
///
/// - `Normal`: leave full screen / un-minimize, then set the frame to `placement.bounds`.
/// - `Maximized`: like `Normal`, with the frame of the usable area of the monitor that best
///   matches `placement.bounds` (macOS has no maximize state).
/// - `Fullscreen`: move the window to `placement.bounds` first so it enters full screen on the
///   right display; a window already full screen on that display is left alone.
/// - `Minimized`: set the frame first so the window returns there when restored.
///
/// Frames are skipped when `placement.bounds` has no area.
pub(crate) fn plan_placement(
    current: &CurrentWindowState,
    placement: &WindowPlacement,
    monitors: &[MonitorInfo],
) -> Vec<PlacementStep> {
    use PlacementStep::*;

    let bounds = FRect::from_rect(placement.bounds);
    let frame = |frame: FRect, best_effort: bool| {
        frame.has_area().then_some(SetFrame { frame, best_effort })
    };
    let mut steps = Vec::new();
    match placement.state {
        WindowState::Normal | WindowState::Maximized => {
            let target = if placement.state == WindowState::Maximized {
                target_monitor(&placement.bounds, monitors)
                    .map(|m| FRect::from_rect(m.work_area))
                    .unwrap_or(bounds)
            } else {
                bounds
            };
            if current.fullscreen {
                steps.push(ExitFullscreen);
            }
            if current.minimized {
                steps.push(Unminimize);
            }
            steps.extend(frame(target, false));
        }
        WindowState::Fullscreen => {
            if current.fullscreen {
                let wanted = target_monitor(&placement.bounds, monitors).map(|m| &m.id);
                let now = current
                    .frame
                    .and_then(|f| target_monitor(&f.to_rect(), monitors))
                    .map(|m| &m.id);
                if current.frame.is_some() && wanted.is_some() && wanted != now {
                    steps.push(ExitFullscreen);
                    steps.extend(frame(bounds, false));
                    steps.push(EnterFullscreen);
                }
            } else {
                if current.minimized {
                    steps.push(Unminimize);
                }
                steps.extend(frame(bounds, false));
                steps.push(EnterFullscreen);
            }
        }
        WindowState::Minimized => {
            if current.fullscreen {
                steps.push(ExitFullscreen);
                steps.extend(frame(bounds, false));
                steps.push(Minimize);
            } else if current.minimized {
                steps.extend(frame(bounds, true));
            } else {
                steps.extend(frame(bounds, false));
                steps.push(Minimize);
            }
        }
    }
    steps
}

// ---------------------------------------------------------------------------------------------
// Accessibility errors
// ---------------------------------------------------------------------------------------------

/// `AXError` codes (HIServices `AXError.h`).
pub(crate) mod ax_code {
    pub(crate) const SUCCESS: i32 = 0;
    pub(crate) const FAILURE: i32 = -25200;
    pub(crate) const ILLEGAL_ARGUMENT: i32 = -25201;
    pub(crate) const INVALID_UI_ELEMENT: i32 = -25202;
    pub(crate) const CANNOT_COMPLETE: i32 = -25204;
    pub(crate) const ATTRIBUTE_UNSUPPORTED: i32 = -25205;
    pub(crate) const ACTION_UNSUPPORTED: i32 = -25206;
    pub(crate) const NOT_IMPLEMENTED: i32 = -25208;
    pub(crate) const API_DISABLED: i32 = -25211;
    pub(crate) const NO_VALUE: i32 = -25212;
}

/// Short instruction used in errors and capability notes.
pub(crate) const GRANT_ACCESSIBILITY: &str = "Grant Reframe the Accessibility permission: open \
     System Settings > Privacy & Security > Accessibility (System Preferences > Security & \
     Privacy > Privacy on macOS 12) and turn on Reframe.";

/// The error for an operation that needs the Accessibility permission.
pub(crate) fn accessibility_denied(operation: &str) -> PlatformError {
    PlatformError::PermissionDenied {
        operation: operation.to_string(),
        permission: Some(PermissionKind::Accessibility),
        message: format!(
            "macOS only lets apps with the Accessibility permission control other applications' \
             windows. {GRANT_ACCESSIBILITY}"
        ),
    }
}

/// Maps an `AXError` code to a [`PlatformError`].
pub(crate) fn ax_error(operation: &str, code: i32) -> PlatformError {
    match code {
        ax_code::API_DISABLED => accessibility_denied(operation),
        ax_code::INVALID_UI_ELEMENT => PlatformError::not_found("The window (it has been closed)"),
        ax_code::CANNOT_COMPLETE => PlatformError::os(
            operation,
            "the application did not answer the Accessibility request (it may be busy or not \
             support Accessibility)",
        ),
        ax_code::ATTRIBUTE_UNSUPPORTED | ax_code::ACTION_UNSUPPORTED | ax_code::NOT_IMPLEMENTED => {
            PlatformError::unsupported(
                operation,
                "the application does not support this through the macOS Accessibility API",
            )
        }
        ax_code::ILLEGAL_ARGUMENT => {
            PlatformError::os(operation, "the application rejected the requested value")
        }
        other => PlatformError::os(operation, format!("Accessibility error {other}")),
    }
}

// ---------------------------------------------------------------------------------------------
// Permissions, version and capabilities
// ---------------------------------------------------------------------------------------------

/// "System Settings > Privacy & Security" (macOS 13+) or the macOS 12 equivalent.
fn privacy_settings_path(macos_major: u32) -> &'static str {
    if macos_major >= 13 {
        "System Settings > Privacy & Security"
    } else {
        "System Preferences > Security & Privacy > Privacy"
    }
}

/// Settings deep links (`x-apple.systempreferences:` URLs, opened with `/usr/bin/open`).
pub(crate) fn privacy_settings_url(kind: PermissionKind) -> &'static str {
    match kind {
        PermissionKind::Accessibility => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        PermissionKind::ScreenRecording => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        }
        PermissionKind::Automation => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation"
        }
    }
}

/// Status, purpose and step-by-step instructions for one permission, worded for the running
/// macOS version (`macos_major`; 0 when unknown, which selects the current wording).
pub(crate) fn permission_status(
    kind: PermissionKind,
    granted: bool,
    macos_major: u32,
) -> PermissionStatus {
    let modern = macos_major == 0 || macos_major >= 13;
    let root = privacy_settings_path(if modern { 13 } else { macos_major });
    let open_pane = |pane: &str| {
        if modern {
            format!("1. Open {root} > {pane}.")
        } else {
            format!(
                "1. Open {root}, select {pane} in the list, then click the lock and enter your \
                 password."
            )
        }
    };
    let (purpose, instructions) = match kind {
        PermissionKind::Accessibility => (
            "Reading window positions, sizes and states (including minimized and full-screen \
             windows) and moving, resizing, focusing and closing windows when restoring a \
             workspace."
                .to_string(),
            [
                open_pane("Accessibility"),
                "2. Turn on Reframe. If Reframe is not listed, click +, choose Reframe in the \
                 Applications folder, then turn it on."
                    .to_string(),
                "3. Return to Reframe; the permission applies immediately.".to_string(),
            ]
            .join("\n"),
        ),
        PermissionKind::ScreenRecording => {
            let pane = if macos_major == 0 || macos_major >= 15 {
                "Screen & System Audio Recording"
            } else {
                "Screen Recording"
            };
            (
                "Reading the titles of other applications' windows. macOS treats window titles \
                 as screen content; Reframe never records or captures the screen."
                    .to_string(),
                [
                    open_pane(pane),
                    "2. Turn on Reframe (click + to add it if it is not listed).".to_string(),
                    "3. Quit and reopen Reframe; macOS applies this permission only after a \
                     restart."
                        .to_string(),
                ]
                .join("\n"),
            )
        }
        PermissionKind::Automation => (
            "Sending Apple Events to other applications (AppleScript), used by integrations \
             that read application state such as open documents or browser tabs."
                .to_string(),
            [
                open_pane("Automation"),
                "2. Expand Reframe and turn on each application it may control.".to_string(),
                "3. Reframe appears in this list after its first request; macOS then asks for \
                 your approval in a dialog."
                    .to_string(),
            ]
            .join("\n"),
        ),
    };
    PermissionStatus {
        kind,
        granted,
        purpose,
        instructions,
    }
}

/// "macOS 15.1", "macOS 14.6.1"; just "macOS" when the version is unknown.
pub(crate) fn format_macos_version(major: i64, minor: i64, patch: i64) -> String {
    match (major, minor.max(0), patch.max(0)) {
        (major, _, _) if major <= 0 => "macOS".to_string(),
        (major, minor, 0) => format!("macOS {major}.{minor}"),
        (major, minor, patch) => format!("macOS {major}.{minor}.{patch}"),
    }
}

/// Capability report for the current permissions.
pub(crate) fn capabilities(
    accessibility: bool,
    screen_recording: bool,
    macos_major: u32,
) -> PlatformCapabilities {
    let needs_ax = || Capability::unsupported(GRANT_ACCESSIBILITY);

    let mut window_notes = vec![
        "Only windows on the current Space are listed; macOS does not expose other Spaces \
         (including full-screen windows that are not currently shown)."
            .to_string(),
        "Windows of hidden applications are not listed, except minimized ones.".to_string(),
    ];
    if !screen_recording {
        window_notes.push(if accessibility {
            "Without the Screen Recording permission, window titles are read through \
             Accessibility and may be missing for some applications."
                .to_string()
        } else {
            "Without the Screen Recording permission, other applications' window titles are not \
             available."
                .to_string()
        });
    }
    if !accessibility {
        window_notes.push(
            "Without the Accessibility permission, minimized windows are not listed and \
             full-screen windows are reported as normal windows."
                .to_string(),
        );
    }

    PlatformCapabilities {
        list_windows: Capability::partial(window_notes.join(" ")),
        read_window_geometry: Capability::full(),
        move_windows: if accessibility {
            Capability::full()
        } else {
            needs_ax()
        },
        change_window_state: if accessibility {
            Capability::partial(
                "macOS has no maximize state: a maximized window is resized to fill its screen \
                 without the menu bar and Dock. Full screen uses the window's own full-screen \
                 mode and only works for applications that support it.",
            )
        } else {
            needs_ax()
        },
        close_windows: if accessibility {
            Capability::full()
        } else {
            needs_ax()
        },
        focus_windows: if !accessibility {
            needs_ax()
        } else if macos_major >= 14 {
            Capability::partial(
                "macOS 14 and later may decline to switch to another application while you are \
                 working in a different one.",
            )
        } else {
            Capability::full()
        },
        list_monitors: Capability::full(),
        process_details: Capability::partial(
            "Command lines and working directories are not available for processes of other \
             users or protected system processes.",
        ),
        application_icons: Capability::full(),
    }
}

// ---------------------------------------------------------------------------------------------
// Applications, bundles and processes
// ---------------------------------------------------------------------------------------------

/// Whether `path` names an application bundle (`*.app`).
pub(crate) fn is_app_bundle_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
}

/// The application bundle an executable belongs to:
/// `/Applications/Foo.app/Contents/MacOS/foo` → `/Applications/Foo.app`. Nested bundles (helper
/// applications) resolve to the innermost bundle.
pub(crate) fn enclosing_app_bundle(executable: &Path) -> Option<PathBuf> {
    let components: Vec<Component<'_>> = executable.components().collect();
    let is = |index: usize, name: &str| matches!(components.get(index), Some(Component::Normal(c)) if *c == name);
    (0..components.len()).rev().find_map(|i| {
        let Component::Normal(name) = components[i] else {
            return None;
        };
        let bundle_name = Path::new(name);
        (is_app_bundle_path(bundle_name)
            && is(i + 1, "Contents")
            && is(i + 2, "MacOS")
            && components.len() > i + 3)
            .then(|| components[..=i].iter().collect())
    })
}

/// Valid reverse-DNS bundle identifier (letters, digits, `.`, `-`, `_`; not starting with `-`
/// so it can never be mistaken for an option of `open`).
pub(crate) fn is_valid_bundle_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 255
        && !id.starts_with(['-', '.'])
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// Folders searched for `<display name>.app`, in order.
pub(crate) const APPLICATION_DIRS: [&str; 5] = [
    "/Applications",
    "/Applications/Utilities",
    "/System/Applications",
    "/System/Applications/Utilities",
    "/System/Library/CoreServices",
];

/// Candidate bundle paths for an application display name ("Visual Studio Code" →
/// `/Applications/Visual Studio Code.app`, ..., `~/Applications/Visual Studio Code.app`).
/// Names that are empty or contain path separators yield nothing.
pub(crate) fn app_bundle_candidates(display_name: &str, home: Option<&Path>) -> Vec<PathBuf> {
    let name = display_name.trim();
    if name.is_empty()
        || name.contains(['/', '\0'])
        || name == "."
        || name == ".."
        || name.starts_with('.')
    {
        return Vec::new();
    }
    let file = if is_app_bundle_path(Path::new(name)) {
        name.to_string()
    } else {
        format!("{name}.app")
    };
    let mut dirs: Vec<PathBuf> = APPLICATION_DIRS.iter().map(PathBuf::from).collect();
    // `has_root`, not `is_absolute`: macOS semantics on every host (see `open_application_args`).
    if let Some(home) = home.filter(|h| h.has_root()) {
        dirs.push(home.join("Applications"));
    }
    dirs.into_iter().map(|dir| dir.join(&file)).collect()
}

/// The package description of a running application: its bundle identifier and bundle path,
/// when it is an `.app` bundle.
pub(crate) fn package_for(app: &AppRecord) -> Option<PackageInfo> {
    let id = app
        .bundle_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())?;
    match &app.bundle_path {
        Some(path) if !is_app_bundle_path(path) => None,
        path => Some(PackageInfo {
            kind: PackageKind::MacBundle,
            id: id.to_string(),
            path: path.clone(),
        }),
    }
}

/// Adds AppKit's application names and bundle identities to the process list.
pub(crate) fn enrich_processes(processes: &mut [ProcessInfo], apps: &[AppRecord]) {
    let by_pid: HashMap<u32, &AppRecord> = apps.iter().map(|a| (a.pid, a)).collect();
    for process in processes {
        let Some(app) = by_pid.get(&process.pid) else {
            continue;
        };
        if let Some(name) = non_blank(app.name.as_deref()) {
            process.product_name = Some(name.trim().to_string());
        }
        if let Some(package) = package_for(app) {
            process.package = Some(package);
        }
    }
}

/// Default edge length (pixels) of icons when the request does not specify one.
pub(crate) const DEFAULT_ICON_SIZE: u32 = 64;
/// Largest icon edge length rendered.
pub(crate) const MAX_ICON_SIZE: u32 = 1024;

/// Edge length to render an icon at.
pub(crate) fn icon_pixel_size(requested: u32) -> u32 {
    if requested == 0 {
        DEFAULT_ICON_SIZE
    } else {
        requested.clamp(16, MAX_ICON_SIZE)
    }
}

// ---------------------------------------------------------------------------------------------
// `/usr/bin/open` command lines
// ---------------------------------------------------------------------------------------------

/// How `open` identifies the application to launch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum OpenApplication<'a> {
    /// `open -b <bundle id>`.
    BundleId(&'a str),
    /// `open -a <path to .app>`.
    Path(&'a Path),
}

/// Arguments for `open -n -b <id> | -a <path> [--env K=V]... [--args ...]`.
///
/// `-n` starts a new instance so launch arguments always reach the application (applications
/// that cannot run twice opt out with `LSMultipleInstancesProhibited`, and then macOS activates
/// the running instance). Everything after `--args` is passed to the application verbatim.
pub(crate) fn open_application_args(
    app: OpenApplication<'_>,
    env: &[(String, String)],
    args: &[String],
) -> Result<Vec<OsString>, String> {
    let mut out: Vec<OsString> = vec!["-n".into()];
    match app {
        OpenApplication::BundleId(id) => {
            if !is_valid_bundle_id(id) {
                return Err(format!("\"{id}\" is not a valid bundle identifier"));
            }
            out.push("-b".into());
            out.push(id.into());
        }
        OpenApplication::Path(path) => {
            // macOS semantics: a rooted path is absolute. `Path::is_absolute` follows the host OS,
            // which differs on Windows, where this logic is also unit-tested.
            if !path.has_root() {
                return Err(format!(
                    "the application path {} is not absolute",
                    path.display()
                ));
            }
            out.push("-a".into());
            out.push(path.as_os_str().to_owned());
        }
    }
    for (key, value) in env {
        if key.is_empty() || key.contains(['=', '\0']) {
            return Err(format!(
                "\"{key}\" is not a valid environment variable name"
            ));
        }
        out.push("--env".into());
        out.push(format!("{key}={value}").into());
    }
    if !args.is_empty() {
        out.push("--args".into());
        out.extend(args.iter().map(OsString::from));
    }
    Ok(out)
}

/// Arguments for `open <target>` (a file, folder or URL opened with its default application).
pub(crate) fn open_target_args(target: &str) -> Result<Vec<OsString>, String> {
    if target.trim().is_empty() {
        return Err("nothing to open".to_string());
    }
    if target.contains('\0') {
        return Err("the target contains a NUL character".to_string());
    }
    // `open` would read a leading dash as an option; a relative path keeps its meaning.
    let target = if target.starts_with('-') {
        format!("./{target}")
    } else {
        target.to_string()
    };
    Ok(vec![target.into()])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(entries: Vec<(&str, PlistValue)>) -> PlistDict {
        entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    }

    fn bounds(x: f64, y: f64, w: f64, h: f64) -> PlistValue {
        PlistValue::Dict(dict(vec![
            ("X", PlistValue::Float(x)),
            ("Y", PlistValue::Float(y)),
            ("Width", PlistValue::Float(w)),
            ("Height", PlistValue::Float(h)),
        ]))
    }

    fn window_dict(number: i64, pid: i64, layer: i64, rect: (f64, f64, f64, f64)) -> PlistDict {
        dict(vec![
            (cg_key::NUMBER, PlistValue::Int(number)),
            (cg_key::OWNER_PID, PlistValue::Int(pid)),
            (cg_key::LAYER, PlistValue::Int(layer)),
            (cg_key::ALPHA, PlistValue::Float(1.0)),
            (cg_key::BOUNDS, bounds(rect.0, rect.1, rect.2, rect.3)),
            (cg_key::IS_ONSCREEN, PlistValue::Bool(true)),
        ])
    }

    fn cg(number: u32, pid: u32, rect: (f64, f64, f64, f64), title: Option<&str>) -> CgWindow {
        CgWindow {
            number,
            pid,
            layer: 0,
            alpha: 1.0,
            bounds: FRect::new(rect.0, rect.1, rect.2, rect.3),
            title: title.map(str::to_string),
            owner_name: None,
            on_screen: true,
        }
    }

    fn ax(rect: (f64, f64, f64, f64), title: Option<&str>) -> AxWindow {
        AxWindow {
            frame: Some(FRect::new(rect.0, rect.1, rect.2, rect.3)),
            title: title.map(str::to_string),
            subrole: Some("AXStandardWindow".into()),
            ..Default::default()
        }
    }

    fn app(pid: u32, bundle: &str) -> AppRecord {
        AppRecord {
            pid,
            bundle_id: Some(bundle.to_string()),
            name: Some(bundle.rsplit('.').next().unwrap_or(bundle).to_string()),
            bundle_path: Some(PathBuf::from(format!("/Applications/{bundle}.app"))),
            regular: true,
            hidden: false,
        }
    }

    fn monitor(id: &str, bounds: Rect, work_area: Rect, primary: bool) -> MonitorInfo {
        MonitorInfo {
            id: id.to_string(),
            name: id.to_string(),
            bounds,
            work_area,
            scale_factor: 2.0,
            is_primary: primary,
        }
    }

    fn laptop() -> Vec<MonitorInfo> {
        vec![monitor(
            "MAIN",
            Rect::new(0, 0, 1440, 900),
            Rect::new(0, 25, 1440, 800),
            true,
        )]
    }

    fn two_monitors() -> Vec<MonitorInfo> {
        vec![
            monitor(
                "MAIN",
                Rect::new(0, 0, 1440, 900),
                Rect::new(0, 25, 1440, 800),
                true,
            ),
            monitor(
                "RIGHT",
                Rect::new(1440, -180, 2560, 1440),
                Rect::new(1440, -155, 2560, 1415),
                false,
            ),
        ]
    }

    // --- plist values -----------------------------------------------------------------------

    #[test]
    fn plist_numbers_convert_safely() {
        assert_eq!(PlistValue::Int(7).as_f64(), Some(7.0));
        assert_eq!(PlistValue::Float(7.0).as_i64(), Some(7));
        assert_eq!(PlistValue::Float(7.5).as_i64(), None);
        assert_eq!(PlistValue::Float(f64::NAN).as_f64(), None);
        assert_eq!(PlistValue::Float(f64::INFINITY).as_i64(), None);
        assert_eq!(PlistValue::String("7".into()).as_i64(), None);
        assert_eq!(PlistValue::Int(1).as_bool(), Some(true));
        assert_eq!(PlistValue::Bool(false).as_bool(), Some(false));
        assert_eq!(PlistValue::Float(1.0).as_bool(), None);
    }

    // --- CoreGraphics window dictionaries ---------------------------------------------------

    #[test]
    fn parses_a_complete_window_dictionary() {
        let mut d = window_dict(42, 501, 0, (10.0, 20.0, 800.0, 600.0));
        d.insert(cg_key::NAME.into(), PlistValue::String("notes.txt".into()));
        d.insert(
            cg_key::OWNER_NAME.into(),
            PlistValue::String("TextEdit".into()),
        );
        let w = CgWindow::from_dict(&d).unwrap();
        assert_eq!(w.number, 42);
        assert_eq!(w.pid, 501);
        assert_eq!(w.bounds, FRect::new(10.0, 20.0, 800.0, 600.0));
        assert_eq!(w.title.as_deref(), Some("notes.txt"));
        assert_eq!(w.owner_name.as_deref(), Some("TextEdit"));
        assert!(w.on_screen);
        assert!(w.is_normal_window());
    }

    #[test]
    fn missing_or_malformed_required_keys_reject_the_entry() {
        let full = window_dict(42, 501, 0, (0.0, 0.0, 100.0, 100.0));
        for key in [
            cg_key::NUMBER,
            cg_key::OWNER_PID,
            cg_key::LAYER,
            cg_key::BOUNDS,
        ] {
            let mut d = full.clone();
            d.remove(key);
            assert!(CgWindow::from_dict(&d).is_none(), "missing {key}");
        }
        let mut d = full.clone();
        d.insert(cg_key::NUMBER.into(), PlistValue::String("42".into()));
        assert!(CgWindow::from_dict(&d).is_none());
        let mut d = full.clone();
        d.insert(cg_key::NUMBER.into(), PlistValue::Int(0));
        assert!(CgWindow::from_dict(&d).is_none(), "kCGNullWindowID");
        let mut d = full.clone();
        d.insert(cg_key::OWNER_PID.into(), PlistValue::Int(-1));
        assert!(CgWindow::from_dict(&d).is_none());
        let mut d = full.clone();
        d.insert(
            cg_key::NUMBER.into(),
            PlistValue::Int(i64::from(u32::MAX) + 1),
        );
        assert!(CgWindow::from_dict(&d).is_none());
        let mut d = full.clone();
        d.insert(cg_key::BOUNDS.into(), bounds(0.0, 0.0, -5.0, 10.0));
        assert!(CgWindow::from_dict(&d).is_none(), "negative size");
        let mut d = full;
        d.insert(
            cg_key::BOUNDS.into(),
            PlistValue::Dict(dict(vec![("X", PlistValue::Int(0))])),
        );
        assert!(CgWindow::from_dict(&d).is_none(), "incomplete bounds");
    }

    #[test]
    fn optional_keys_have_safe_defaults() {
        let mut d = window_dict(1, 2, 0, (0.0, 0.0, 10.0, 10.0));
        d.remove(cg_key::ALPHA);
        d.remove(cg_key::IS_ONSCREEN);
        d.insert(cg_key::NAME.into(), PlistValue::String("   ".into()));
        let w = CgWindow::from_dict(&d).unwrap();
        assert_eq!(w.alpha, 1.0);
        assert!(!w.on_screen);
        assert_eq!(w.title, None, "blank titles are unknown titles");
    }

    #[test]
    fn only_normal_layer_visible_sized_windows_are_candidates() {
        let base = cg(1, 2, (0.0, 0.0, 100.0, 100.0), None);
        assert!(base.is_normal_window());
        let menu_bar = CgWindow {
            layer: 25,
            ..base.clone()
        };
        assert!(!menu_bar.is_normal_window());
        let transparent = CgWindow {
            alpha: 0.0,
            ..base.clone()
        };
        assert!(!transparent.is_normal_window());
        let empty = CgWindow {
            bounds: FRect::new(5.0, 5.0, 0.0, 30.0),
            ..base.clone()
        };
        assert!(!empty.is_normal_window());
        let helper = CgWindow {
            bounds: FRect::new(0.0, 0.0, 1.0, 1.0),
            ..base.clone()
        };
        assert!(!helper.is_normal_window(), "1x1 helper windows");
        let small = CgWindow {
            bounds: FRect::new(0.0, 0.0, MIN_WINDOW_SIZE, 120.0),
            ..base
        };
        assert!(small.is_normal_window());
    }

    // --- geometry ---------------------------------------------------------------------------

    #[test]
    fn rounds_and_compares_rects() {
        assert_eq!(
            FRect::new(10.4, -20.6, 800.5, 600.49).to_rect(),
            Rect::new(10, -21, 801, 600)
        );
        assert_eq!(
            FRect::new(1e12, -1e12, 1.0, 1.0).to_rect(),
            Rect::new(i32::MAX, i32::MIN, 1, 1)
        );
        let a = FRect::new(0.0, 0.0, 100.0, 100.0);
        assert!(a.approx_eq(&FRect::new(2.0, -2.0, 101.0, 98.0), 2.0));
        assert!(!a.approx_eq(&FRect::new(2.5, 0.0, 100.0, 100.0), 2.0));
        assert!(!FRect::new(0.0, 0.0, f64::NAN, 1.0).is_valid());
        assert!(!FRect::new(0.0, 0.0, 0.0, 1.0).has_area());
    }

    #[test]
    fn flips_appkit_rects_into_coregraphics_space() {
        // Main screen 1440x900: AppKit origin bottom-left, CG origin top-left.
        let main_height = 900.0;
        // Visible frame below a 25 pt menu bar, above a 75 pt Dock.
        let visible = FRect::new(0.0, 75.0, 1440.0, 800.0);
        assert_eq!(
            appkit_to_cg(visible, main_height),
            FRect::new(0.0, 25.0, 1440.0, 800.0)
        );
        // A secondary screen above the main one has negative CG y.
        let above = FRect::new(0.0, 900.0, 1920.0, 1080.0);
        assert_eq!(
            appkit_to_cg(above, main_height),
            FRect::new(0.0, -1080.0, 1920.0, 1080.0)
        );
        // A secondary screen below the main one.
        let below = FRect::new(-200.0, -1080.0, 1920.0, 1080.0);
        assert_eq!(
            appkit_to_cg(below, main_height),
            FRect::new(-200.0, 900.0, 1920.0, 1080.0)
        );
    }

    // --- monitors ---------------------------------------------------------------------------

    fn display(id: u32, rect: (f64, f64, f64, f64), main: bool) -> DisplayRecord {
        DisplayRecord {
            id,
            bounds: FRect::new(rect.0, rect.1, rect.2, rect.3),
            uuid: Some(format!("UUID-{id}")),
            is_main: main,
            is_builtin: main,
            mode_scale: Some(2.0),
        }
    }

    #[test]
    fn builds_monitors_from_displays_and_screens() {
        let displays = vec![
            display(1, (0.0, 0.0, 1440.0, 900.0), true),
            display(2, (1440.0, -180.0, 2560.0, 1440.0), false),
        ];
        let screens = vec![
            ScreenRecord {
                display_id: Some(2),
                name: Some("DELL U2720Q".into()),
                // AppKit: y = 900 - (-180 + 1440) = -360.
                frame: FRect::new(1440.0, -360.0, 2560.0, 1440.0),
                visible_frame: FRect::new(1440.0, -360.0, 2560.0, 1415.0),
                scale: 1.5,
            },
            ScreenRecord {
                display_id: Some(1),
                name: Some("Built-in Retina Display".into()),
                frame: FRect::new(0.0, 0.0, 1440.0, 900.0),
                visible_frame: FRect::new(0.0, 75.0, 1440.0, 800.0),
                scale: 2.0,
            },
        ];
        let monitors = build_monitors(&displays, &screens);
        assert_eq!(monitors.len(), 2);
        assert_eq!(monitors[0].id, "UUID-1");
        assert_eq!(monitors[0].name, "Built-in Retina Display");
        assert!(monitors[0].is_primary);
        assert_eq!(monitors[0].bounds, Rect::new(0, 0, 1440, 900));
        assert_eq!(monitors[0].work_area, Rect::new(0, 25, 1440, 800));
        assert_eq!(monitors[1].name, "DELL U2720Q");
        assert_eq!(monitors[1].bounds, Rect::new(1440, -180, 2560, 1440));
        assert_eq!(monitors[1].work_area, Rect::new(1440, -155, 2560, 1415));
        assert_eq!(monitors[1].scale_factor, 1.5);
        assert!(!monitors[1].is_primary);
    }

    #[test]
    fn monitors_without_screen_data_fall_back_to_coregraphics() {
        let mut second = display(9, (1440.0, 0.0, 1920.0, 1080.0), false);
        second.uuid = None;
        second.mode_scale = None;
        let monitors = build_monitors(&[display(1, (0.0, 0.0, 1440.0, 900.0), true), second], &[]);
        assert_eq!(monitors[0].name, "Built-in Display");
        assert_eq!(monitors[0].work_area, monitors[0].bounds);
        assert_eq!(monitors[0].scale_factor, 2.0);
        assert_eq!(monitors[1].id, "cg-display-9");
        assert_eq!(monitors[1].name, "Display 2");
        assert_eq!(monitors[1].scale_factor, 1.0);
    }

    #[test]
    fn screens_without_a_display_number_match_by_frame() {
        let screens = vec![ScreenRecord {
            display_id: None,
            name: Some("LG UltraFine".into()),
            frame: FRect::new(0.0, 0.0, 2560.0, 1440.0),
            visible_frame: FRect::new(0.0, 0.0, 2560.0, 1415.0),
            scale: 2.0,
        }];
        let monitors = build_monitors(&[display(4, (0.0, 0.0, 2560.0, 1440.0), true)], &screens);
        assert_eq!(monitors[0].name, "LG UltraFine");
        assert_eq!(monitors[0].work_area, Rect::new(0, 25, 2560, 1415));
    }

    #[test]
    fn inconsistent_visible_frames_are_ignored() {
        let screens = vec![ScreenRecord {
            display_id: Some(1),
            name: None,
            frame: FRect::new(0.0, 0.0, 1440.0, 900.0),
            visible_frame: FRect::new(5000.0, 5000.0, 100.0, 100.0),
            scale: f64::NAN,
        }];
        let monitors = build_monitors(&[display(1, (0.0, 0.0, 1440.0, 900.0), true)], &screens);
        assert_eq!(monitors[0].work_area, monitors[0].bounds);
        assert_eq!(
            monitors[0].scale_factor, 2.0,
            "falls back to the display mode"
        );
    }

    #[test]
    fn picks_monitors_by_overlap_then_distance() {
        let monitors = two_monitors();
        let on_right = Rect::new(1300, 100, 600, 400); // mostly on RIGHT
        assert_eq!(monitor_for(&on_right, &monitors).unwrap().id, "RIGHT");
        let nowhere = Rect::new(-5000, -5000, 100, 100);
        assert!(monitor_for(&nowhere, &monitors).is_none());
        assert_eq!(target_monitor(&nowhere, &monitors).unwrap().id, "MAIN");
        let far_right = Rect::new(9000, 0, 100, 100);
        assert_eq!(target_monitor(&far_right, &monitors).unwrap().id, "RIGHT");
        assert!(target_monitor(&far_right, &[]).is_none());
    }

    #[test]
    fn equally_near_monitors_resolve_to_the_first() {
        let monitors = vec![
            monitor(
                "MAIN",
                Rect::new(0, 0, 1440, 900),
                Rect::new(0, 25, 1440, 800),
                true,
            ),
            monitor(
                "LEFT",
                Rect::new(-1440, 0, 1440, 900),
                Rect::new(-1440, 0, 1440, 900),
                false,
            ),
        ];
        // Below both monitors, centred on their shared edge: equally near to each.
        let below = Rect::new(-50, 2000, 100, 100);
        assert!(monitor_for(&below, &monitors).is_none());
        assert_eq!(target_monitor(&below, &monitors).unwrap().id, "MAIN");
    }

    #[test]
    fn identical_displays_get_unique_monitor_ids() {
        let twin = |id: u32, x: f64, main: bool| DisplayRecord {
            uuid: Some("TWIN".into()),
            ..display(id, (x, 0.0, 1920.0, 1080.0), main)
        };
        let displays = vec![
            twin(1, 0.0, true),
            twin(2, 1920.0, false),
            display(3, (-1440.0, 0.0, 1440.0, 900.0), false),
            // Mirrored onto the first display: same UUID and origin.
            twin(4, 0.0, false),
        ];
        let ids: Vec<String> = build_monitors(&displays, &[])
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(ids, ["TWIN@0,0", "TWIN@1920,0", "UUID-3", "cg-display-4"]);
    }

    #[test]
    fn detects_maximized_frames() {
        let work = Rect::new(0, 25, 1440, 800);
        assert!(is_maximized(&Rect::new(0, 25, 1440, 800), &work));
        assert!(is_maximized(&Rect::new(1, 24, 1439, 802), &work));
        assert!(!is_maximized(&Rect::new(0, 25, 1400, 800), &work));
        assert!(
            !is_maximized(&Rect::new(0, 0, 1440, 900), &work),
            "full screen"
        );
        assert!(!is_maximized(&Rect::new(0, 0, 0, 0), &Rect::default()));
    }

    // --- matching ---------------------------------------------------------------------------

    #[test]
    fn scores_matches_by_geometry_then_title() {
        let key = WindowKey {
            number: 1,
            title: Some("Doc"),
            bounds: FRect::new(100.0, 100.0, 500.0, 400.0),
            on_screen: true,
        };
        assert_eq!(
            match_score(&key, &ax((100.0, 100.0, 500.0, 400.0), Some("Doc"))),
            Some(3)
        );
        assert_eq!(
            match_score(&key, &ax((101.0, 99.0, 500.0, 402.0), None)),
            Some(2)
        );
        assert_eq!(
            match_score(&key, &ax((100.0, 100.0, 500.0, 400.0), Some("Other"))),
            Some(1)
        );
        assert_eq!(
            match_score(&key, &ax((103.0, 100.0, 500.0, 400.0), Some("Doc"))),
            None
        );
        let no_frame = AxWindow {
            frame: None,
            ..ax((0.0, 0.0, 0.0, 0.0), Some("Doc"))
        };
        assert_eq!(match_score(&key, &no_frame), None);
    }

    #[test]
    fn assigns_exact_ids_first_and_geometry_one_to_one() {
        let keys = [
            WindowKey {
                number: 10,
                title: None,
                bounds: FRect::new(0.0, 0.0, 500.0, 500.0),
                on_screen: true,
            },
            WindowKey {
                number: 11,
                title: None,
                bounds: FRect::new(0.0, 0.0, 500.0, 500.0),
                on_screen: true,
            },
            WindowKey {
                number: 12,
                title: None,
                bounds: FRect::new(900.0, 0.0, 300.0, 300.0),
                on_screen: true,
            },
        ];
        let mut exact = ax((0.0, 0.0, 500.0, 500.0), None);
        exact.cg_id = Some(11);
        let elsewhere = AxWindow {
            cg_id: Some(99), // known id of a window that is not in `keys`
            ..ax((900.0, 0.0, 300.0, 300.0), None)
        };
        let windows = vec![
            ax((0.0, 0.0, 500.0, 500.0), None),
            exact,
            elsewhere,
            ax((0.0, 0.0, 500.0, 500.0), None),
        ];
        let result = assign_windows(&keys, &windows);
        assert_eq!(result, vec![Some(0), Some(1), None]);
    }

    #[test]
    fn titles_disambiguate_identical_frames() {
        let keys = [
            WindowKey {
                number: 1,
                title: Some("b.txt"),
                bounds: FRect::new(0.0, 0.0, 500.0, 500.0),
                on_screen: true,
            },
            WindowKey {
                number: 2,
                title: Some("a.txt"),
                bounds: FRect::new(0.0, 0.0, 500.0, 500.0),
                on_screen: true,
            },
        ];
        let windows = vec![
            ax((0.0, 0.0, 500.0, 500.0), Some("a.txt")),
            ax((0.0, 0.0, 500.0, 500.0), Some("b.txt")),
        ];
        assert_eq!(assign_windows(&keys, &windows), vec![Some(1), Some(0)]);
        assert_eq!(assign_windows(&[], &windows), Vec::<Option<usize>>::new());
        assert_eq!(assign_windows(&keys, &[]), vec![None, None]);
    }

    #[test]
    fn minimized_windows_never_match_on_screen_windows() {
        let frame = (100.0, 100.0, 500.0, 400.0);
        let mut minimized = ax(frame, None);
        minimized.minimized = true;
        let ax_windows = vec![minimized, ax(frame, None)];
        let on_screen = cg(1, 100, frame, None);
        assert_eq!(
            assign_windows(&[on_screen.key()], &ax_windows),
            vec![Some(1)]
        );
        let off_screen = CgWindow {
            on_screen: false,
            ..cg(2, 100, frame, None)
        };
        assert_eq!(
            assign_windows(&[off_screen.key()], &ax_windows),
            vec![Some(0)]
        );

        // Same frame on screen and minimized (no window numbers from Accessibility): the visible
        // window stays normal and the minimized one is still listed.
        let windows = assemble_windows(&WindowSources {
            on_screen: &[on_screen],
            off_screen: &[off_screen],
            apps: &[app(100, "com.apple.Terminal")],
            ax: &HashMap::from([(
                100,
                AxApp {
                    windows: ax_windows,
                    focus_known: false,
                },
            )]),
            monitors: &laptop(),
            frontmost_pid: None,
        });
        let summary: Vec<(&str, WindowState)> =
            windows.iter().map(|w| (w.id.as_str(), w.state)).collect();
        assert_eq!(
            summary,
            vec![("1", WindowState::Normal), ("2", WindowState::Minimized)]
        );
    }

    // --- window list assembly ---------------------------------------------------------------

    #[test]
    fn assembles_regular_application_windows_front_to_back() {
        let on_screen = vec![
            cg(5, 100, (0.0, 0.0, 1440.0, 25.0), None), // filtered below: menu-bar layer
            cg(6, 300, (50.0, 50.0, 300.0, 200.0), Some("Agent")), // accessory app
            cg(7, 100, (0.0, 25.0, 1440.0, 800.0), Some("Editor")),
            cg(8, 200, (100.0, 100.0, 600.0, 400.0), None),
            cg(9, 100, (200.0, 200.0, 0.0, 400.0), Some("zero")),
        ];
        let mut on_screen = on_screen;
        on_screen[0].layer = 24;
        let mut agent = app(300, "com.example.agent");
        agent.regular = false;
        let apps = vec![
            app(100, "com.example.editor"),
            app(200, "com.example.viewer"),
            agent,
        ];
        let ax_data = HashMap::new();
        let monitors = laptop();
        let windows = assemble_windows(&WindowSources {
            on_screen: &on_screen,
            off_screen: &[],
            apps: &apps,
            ax: &ax_data,
            monitors: &monitors,
            frontmost_pid: Some(200),
        });
        let ids: Vec<&str> = windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, vec!["7", "8"]);
        assert_eq!(windows[0].state, WindowState::Maximized);
        assert_eq!(windows[0].class_name.as_deref(), Some("com.example.editor"));
        assert_eq!(
            windows[0].app_id_hint.as_deref(),
            Some("com.example.editor")
        );
        assert_eq!(windows[0].monitor_id.as_deref(), Some("MAIN"));
        assert_eq!(windows[0].z_order, 0);
        assert_eq!(windows[1].z_order, 1);
        assert_eq!(
            windows[1].title, "",
            "no title without Screen Recording or Accessibility"
        );
        assert_eq!(windows[1].state, WindowState::Normal);
        assert!(
            windows[1].is_focused,
            "frontmost app's top window without AX focus data"
        );
        assert!(!windows[0].is_focused);
        assert_eq!(windows[0].normal_bounds, None);
        assert_eq!(windows[0].desktop, None);
    }

    #[test]
    fn accessibility_supplies_titles_states_focus_and_minimized_windows() {
        let on_screen = vec![
            cg(20, 100, (100.0, 100.0, 800.0, 600.0), None),
            cg(21, 100, (0.0, 0.0, 1440.0, 900.0), None),
            cg(22, 100, (700.0, 50.0, 200.0, 300.0), None),
        ];
        let off_screen = vec![CgWindow {
            on_screen: false,
            ..cg(30, 100, (300.0, 300.0, 640.0, 480.0), Some("Draft"))
        }];
        let mut focused = ax((100.0, 100.0, 800.0, 600.0), Some("Report.pages"));
        focused.focused = true;
        let mut fullscreen = ax((0.0, 0.0, 1440.0, 900.0), Some("Slides"));
        fullscreen.fullscreen = true;
        let mut palette = ax((700.0, 50.0, 200.0, 300.0), Some("Colors"));
        palette.subrole = Some("AXFloatingWindow".into());
        let mut minimized_known = ax((10.0, 10.0, 500.0, 500.0), Some("Notes"));
        minimized_known.minimized = true;
        minimized_known.cg_id = Some(31);
        let mut minimized_matched = ax((300.0, 300.0, 640.0, 480.0), None);
        minimized_matched.minimized = true;
        let mut minimized_unknown = ax((900.0, 900.0, 100.0, 100.0), Some("Lost"));
        minimized_unknown.minimized = true;
        let ax_data = HashMap::from([(
            100,
            AxApp {
                windows: vec![
                    focused,
                    fullscreen,
                    palette,
                    minimized_known,
                    minimized_matched,
                    minimized_unknown,
                ],
                focus_known: true,
            },
        )]);
        let apps = vec![app(100, "com.apple.iWork.Pages")];
        let monitors = laptop();
        let windows = assemble_windows(&WindowSources {
            on_screen: &on_screen,
            off_screen: &off_screen,
            apps: &apps,
            ax: &ax_data,
            monitors: &monitors,
            frontmost_pid: Some(100),
        });
        let summary: Vec<(&str, &str, WindowState, bool)> = windows
            .iter()
            .map(|w| (w.id.as_str(), w.title.as_str(), w.state, w.is_focused))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("20", "Report.pages", WindowState::Normal, true),
                ("21", "Slides", WindowState::Fullscreen, false),
                ("31", "Notes", WindowState::Minimized, false),
                ("30", "Draft", WindowState::Minimized, false),
            ]
        );
        assert_eq!(windows[2].bounds, Rect::new(10, 10, 500, 500));
        assert_eq!(windows[3].bounds, Rect::new(300, 300, 640, 480));
        assert_eq!(windows[3].z_order, 3);
    }

    #[test]
    fn known_accessibility_focus_is_not_overridden() {
        let on_screen = vec![cg(1, 100, (0.0, 30.0, 300.0, 300.0), Some("A"))];
        let ax_data = HashMap::from([(
            100,
            AxApp {
                windows: vec![ax((0.0, 30.0, 300.0, 300.0), Some("A"))],
                focus_known: true, // e.g. focus is in a sheet or panel
            },
        )]);
        let apps = vec![app(100, "com.example.a")];
        let windows = assemble_windows(&WindowSources {
            on_screen: &on_screen,
            off_screen: &[],
            apps: &apps,
            ax: &ax_data,
            monitors: &laptop(),
            frontmost_pid: Some(100),
        });
        assert!(!windows[0].is_focused);
    }

    #[test]
    fn duplicate_window_numbers_and_unknown_apps_are_skipped() {
        let on_screen = vec![
            cg(1, 100, (0.0, 30.0, 300.0, 300.0), Some("A")),
            cg(1, 100, (0.0, 30.0, 300.0, 300.0), Some("A")),
            cg(2, 999, (0.0, 30.0, 300.0, 300.0), Some("unknown pid")),
        ];
        let apps = vec![app(100, "com.example.a")];
        let windows = assemble_windows(&WindowSources {
            on_screen: &on_screen,
            off_screen: &[],
            apps: &apps,
            ax: &HashMap::new(),
            monitors: &[],
            frontmost_pid: None,
        });
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].monitor_id, None);
        assert!(!windows[0].is_focused);
    }

    #[test]
    fn windows_outside_every_monitor_are_skipped() {
        let on_screen = vec![
            cg(1, 100, (-20000.0, -20000.0, 400.0, 300.0), Some("parked")),
            cg(
                2,
                100,
                (1400.0, 100.0, 400.0, 300.0),
                Some("partly visible"),
            ),
        ];
        let apps = vec![app(100, "com.example.a")];
        let windows = assemble_windows(&WindowSources {
            on_screen: &on_screen,
            off_screen: &[],
            apps: &apps,
            ax: &HashMap::new(),
            monitors: &laptop(),
            frontmost_pid: None,
        });
        let ids: Vec<&str> = windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, vec!["2"]);
        assert_eq!(windows[0].monitor_id.as_deref(), Some("MAIN"));
    }

    #[test]
    fn parses_window_ids() {
        assert_eq!(parse_window_id(&WindowId::new("1234")), Some(1234));
        assert_eq!(parse_window_id(&WindowId::new(" 77 ")), Some(77));
        assert_eq!(parse_window_id(&WindowId::new("0")), None);
        assert_eq!(parse_window_id(&WindowId::new("mock-1")), None);
        assert_eq!(parse_window_id(&WindowId::new("-3")), None);
        assert_eq!(parse_window_id(&WindowId::new("99999999999")), None);
    }

    // --- placement planning -----------------------------------------------------------------

    fn placement(rect: Rect, state: WindowState) -> WindowPlacement {
        WindowPlacement {
            bounds: rect,
            state,
        }
    }

    #[test]
    fn plans_normal_placements() {
        let target = Rect::new(100, 100, 800, 600);
        let frame = FRect::from_rect(target);
        let plan = plan_placement(
            &CurrentWindowState::default(),
            &placement(target, WindowState::Normal),
            &laptop(),
        );
        assert_eq!(
            plan,
            vec![PlacementStep::SetFrame {
                frame,
                best_effort: false
            }]
        );
        let plan = plan_placement(
            &CurrentWindowState {
                minimized: true,
                fullscreen: true,
                frame: None,
            },
            &placement(target, WindowState::Normal),
            &laptop(),
        );
        assert_eq!(
            plan,
            vec![
                PlacementStep::ExitFullscreen,
                PlacementStep::Unminimize,
                PlacementStep::SetFrame {
                    frame,
                    best_effort: false
                }
            ]
        );
    }

    #[test]
    fn maximized_uses_the_target_monitors_work_area() {
        let plan = plan_placement(
            &CurrentWindowState::default(),
            &placement(Rect::new(2000, 100, 800, 600), WindowState::Maximized),
            &two_monitors(),
        );
        assert_eq!(
            plan,
            vec![PlacementStep::SetFrame {
                frame: FRect::new(1440.0, -155.0, 2560.0, 1415.0),
                best_effort: false
            }]
        );
    }

    #[test]
    fn fullscreen_moves_first_and_skips_when_already_there() {
        let target = Rect::new(2000, 100, 800, 600);
        let plan = plan_placement(
            &CurrentWindowState::default(),
            &placement(target, WindowState::Fullscreen),
            &two_monitors(),
        );
        assert_eq!(
            plan,
            vec![
                PlacementStep::SetFrame {
                    frame: FRect::from_rect(target),
                    best_effort: false
                },
                PlacementStep::EnterFullscreen
            ]
        );
        let already = CurrentWindowState {
            fullscreen: true,
            frame: Some(FRect::new(1440.0, -180.0, 2560.0, 1440.0)),
            ..Default::default()
        };
        assert!(
            plan_placement(
                &already,
                &placement(target, WindowState::Fullscreen),
                &two_monitors()
            )
            .is_empty()
        );
        let elsewhere = CurrentWindowState {
            fullscreen: true,
            frame: Some(FRect::new(0.0, 0.0, 1440.0, 900.0)),
            ..Default::default()
        };
        assert_eq!(
            plan_placement(
                &elsewhere,
                &placement(target, WindowState::Fullscreen),
                &two_monitors()
            ),
            vec![
                PlacementStep::ExitFullscreen,
                PlacementStep::SetFrame {
                    frame: FRect::from_rect(target),
                    best_effort: false
                },
                PlacementStep::EnterFullscreen
            ]
        );
    }

    #[test]
    fn minimized_sets_the_frame_before_minimizing() {
        let target = Rect::new(10, 40, 400, 300);
        let frame = FRect::from_rect(target);
        assert_eq!(
            plan_placement(
                &CurrentWindowState::default(),
                &placement(target, WindowState::Minimized),
                &laptop()
            ),
            vec![
                PlacementStep::SetFrame {
                    frame,
                    best_effort: false
                },
                PlacementStep::Minimize
            ]
        );
        assert_eq!(
            plan_placement(
                &CurrentWindowState {
                    minimized: true,
                    ..Default::default()
                },
                &placement(target, WindowState::Minimized),
                &laptop()
            ),
            vec![PlacementStep::SetFrame {
                frame,
                best_effort: true
            }]
        );
    }

    #[test]
    fn empty_bounds_keep_the_current_frame() {
        let plan = plan_placement(
            &CurrentWindowState::default(),
            &placement(Rect::new(10, 10, 0, 0), WindowState::Minimized),
            &laptop(),
        );
        assert_eq!(plan, vec![PlacementStep::Minimize]);
    }

    // --- errors, permissions, capabilities --------------------------------------------------

    #[test]
    fn maps_accessibility_errors() {
        assert!(matches!(
            ax_error("Moving a window", ax_code::API_DISABLED),
            PlatformError::PermissionDenied {
                permission: Some(PermissionKind::Accessibility),
                ..
            }
        ));
        assert!(matches!(
            ax_error("x", ax_code::INVALID_UI_ELEMENT),
            PlatformError::NotFound { .. }
        ));
        assert!(ax_error("x", ax_code::ATTRIBUTE_UNSUPPORTED).is_unsupported());
        assert!(ax_error("x", ax_code::ACTION_UNSUPPORTED).is_unsupported());
        assert!(matches!(
            ax_error("x", ax_code::CANNOT_COMPLETE),
            PlatformError::Os { .. }
        ));
        assert!(ax_error("x", -1).to_string().contains("-1"));
    }

    #[test]
    fn permission_texts_follow_the_macos_version() {
        let ax13 = permission_status(PermissionKind::Accessibility, false, 14);
        assert!(!ax13.granted);
        assert!(
            ax13.instructions
                .contains("System Settings > Privacy & Security > Accessibility")
        );
        assert!(ax13.instructions.lines().count() >= 3);
        let ax12 = permission_status(PermissionKind::Accessibility, true, 12);
        assert!(ax12.granted);
        assert!(
            ax12.instructions
                .contains("System Preferences > Security & Privacy")
        );
        let sr15 = permission_status(PermissionKind::ScreenRecording, false, 15);
        assert!(
            sr15.instructions
                .contains("Screen & System Audio Recording")
        );
        assert!(sr15.instructions.contains("Quit and reopen"));
        let sr14 = permission_status(PermissionKind::ScreenRecording, false, 14);
        assert!(sr14.instructions.contains("> Screen Recording."));
        let auto = permission_status(PermissionKind::Automation, false, 0);
        assert!(auto.instructions.contains("Automation"));
        assert!(!auto.purpose.is_empty());
        assert!(privacy_settings_url(PermissionKind::Automation).ends_with("Privacy_Automation"));
    }

    #[test]
    fn formats_macos_versions() {
        assert_eq!(format_macos_version(15, 1, 0), "macOS 15.1");
        assert_eq!(format_macos_version(14, 6, 1), "macOS 14.6.1");
        assert_eq!(format_macos_version(26, 0, 0), "macOS 26.0");
        assert_eq!(format_macos_version(0, 0, 0), "macOS");
        assert_eq!(format_macos_version(13, -1, -1), "macOS 13.0");
    }

    #[test]
    fn capabilities_reflect_permissions() {
        let none = capabilities(false, false, 15);
        assert_eq!(none.move_windows.level, crate::SupportLevel::Unsupported);
        assert!(
            none.move_windows
                .note
                .as_deref()
                .unwrap()
                .contains("Accessibility")
        );
        assert_eq!(none.close_windows.level, crate::SupportLevel::Unsupported);
        assert_eq!(none.focus_windows.level, crate::SupportLevel::Unsupported);
        assert_eq!(
            none.change_window_state.level,
            crate::SupportLevel::Unsupported
        );
        let note = none.list_windows.note.unwrap();
        assert!(note.contains("Screen Recording") && note.contains("minimized"));

        let all = capabilities(true, true, 13);
        assert_eq!(all.move_windows.level, crate::SupportLevel::Full);
        assert_eq!(all.close_windows.level, crate::SupportLevel::Full);
        assert_eq!(all.focus_windows.level, crate::SupportLevel::Full);
        assert_eq!(all.change_window_state.level, crate::SupportLevel::Partial);
        assert!(!all.list_windows.note.unwrap().contains("Screen Recording"));
        assert_eq!(all.list_monitors.level, crate::SupportLevel::Full);
        assert_eq!(all.application_icons.level, crate::SupportLevel::Full);

        let titles_from_ax = capabilities(true, false, 15);
        assert!(
            titles_from_ax
                .list_windows
                .note
                .unwrap()
                .contains("through Accessibility")
        );
        assert_eq!(
            titles_from_ax.focus_windows.level,
            crate::SupportLevel::Partial
        );
    }

    // --- bundles, processes, icons ----------------------------------------------------------

    #[test]
    fn finds_the_enclosing_app_bundle() {
        assert_eq!(
            enclosing_app_bundle(Path::new(
                "/Applications/Visual Studio Code.app/Contents/MacOS/Electron"
            )),
            Some(PathBuf::from("/Applications/Visual Studio Code.app"))
        );
        assert_eq!(
            enclosing_app_bundle(Path::new(
                "/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/Helpers/Google Chrome Helper.app/Contents/MacOS/Google Chrome Helper"
            )),
            Some(PathBuf::from(
                "/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/Helpers/Google Chrome Helper.app"
            ))
        );
        assert_eq!(
            enclosing_app_bundle(Path::new("/Applications/Foo.APP/Contents/MacOS/foo")),
            Some(PathBuf::from("/Applications/Foo.APP"))
        );
        assert_eq!(enclosing_app_bundle(Path::new("/usr/bin/vim")), None);
        assert_eq!(
            enclosing_app_bundle(Path::new("/Applications/Foo.app/Contents/Resources/tool")),
            None
        );
        assert_eq!(
            enclosing_app_bundle(Path::new("/Applications/Foo.app/Contents/MacOS")),
            None
        );
        assert!(is_app_bundle_path(Path::new("/Applications/Safari.app")));
        assert!(!is_app_bundle_path(Path::new("/Applications/Safari")));
    }

    #[test]
    fn validates_bundle_identifiers() {
        assert!(is_valid_bundle_id("com.microsoft.VSCode"));
        assert!(is_valid_bundle_id("org.mozilla.firefox"));
        assert!(is_valid_bundle_id("com.example.my_app-2"));
        assert!(!is_valid_bundle_id(""));
        assert!(!is_valid_bundle_id("-a"));
        assert!(!is_valid_bundle_id("com.example app"));
        assert!(!is_valid_bundle_id("com/example"));
        assert!(!is_valid_bundle_id(&"a".repeat(256)));
    }

    #[test]
    fn lists_bundle_candidates_for_display_names() {
        let home = Path::new("/Users/me");
        let candidates = app_bundle_candidates("Visual Studio Code", Some(home));
        assert_eq!(
            candidates[0],
            PathBuf::from("/Applications/Visual Studio Code.app")
        );
        assert!(candidates.contains(&PathBuf::from(
            "/System/Applications/Utilities/Visual Studio Code.app"
        )));
        assert_eq!(
            candidates.last().unwrap(),
            &PathBuf::from("/Users/me/Applications/Visual Studio Code.app")
        );
        assert_eq!(
            app_bundle_candidates("Safari.app", None)[0],
            PathBuf::from("/Applications/Safari.app")
        );
        assert_eq!(
            app_bundle_candidates("Safari", None).len(),
            APPLICATION_DIRS.len()
        );
        assert!(app_bundle_candidates("", Some(home)).is_empty());
        assert!(app_bundle_candidates("../../etc", Some(home)).is_empty());
        assert!(app_bundle_candidates("..", Some(home)).is_empty());
        assert!(app_bundle_candidates(".hidden", Some(home)).is_empty());
        assert_eq!(
            app_bundle_candidates("Notes", Some(Path::new("relative"))).len(),
            APPLICATION_DIRS.len(),
            "relative home directories are ignored"
        );
    }

    #[test]
    fn enriches_processes_with_bundle_information() {
        let mut processes = vec![
            ProcessInfo {
                pid: 10,
                parent_pid: Some(1),
                name: "Code".into(),
                product_name: None,
                executable_path: Some(PathBuf::from(
                    "/Applications/Visual Studio Code.app/Contents/MacOS/Code",
                )),
                command_line: vec![],
                working_directory: None,
                start_time: None,
                package: None,
            },
            ProcessInfo {
                pid: 11,
                parent_pid: Some(1),
                name: "python3".into(),
                product_name: None,
                executable_path: None,
                command_line: vec![],
                working_directory: None,
                start_time: None,
                package: None,
            },
            ProcessInfo {
                pid: 12,
                parent_pid: Some(1),
                name: "zsh".into(),
                product_name: None,
                executable_path: None,
                command_line: vec![],
                working_directory: None,
                start_time: None,
                package: None,
            },
        ];
        let apps = vec![
            AppRecord {
                pid: 10,
                bundle_id: Some("com.microsoft.VSCode".into()),
                name: Some("Code".into()),
                bundle_path: Some(PathBuf::from("/Applications/Visual Studio Code.app")),
                regular: true,
                hidden: false,
            },
            AppRecord {
                pid: 11,
                bundle_id: Some("org.python.python".into()),
                name: Some("Python".into()),
                bundle_path: Some(PathBuf::from("/usr/local/bin")),
                regular: true,
                hidden: false,
            },
        ];
        enrich_processes(&mut processes, &apps);
        assert_eq!(processes[0].product_name.as_deref(), Some("Code"));
        assert_eq!(
            processes[0].package,
            Some(PackageInfo {
                kind: PackageKind::MacBundle,
                id: "com.microsoft.VSCode".into(),
                path: Some(PathBuf::from("/Applications/Visual Studio Code.app")),
            })
        );
        assert_eq!(processes[1].product_name.as_deref(), Some("Python"));
        assert_eq!(processes[1].package, None, "not an .app bundle");
        assert_eq!(processes[2].product_name, None);
        assert_eq!(processes[2].package, None);
    }

    #[test]
    fn clamps_icon_sizes() {
        assert_eq!(icon_pixel_size(0), DEFAULT_ICON_SIZE);
        assert_eq!(icon_pixel_size(1), 16);
        assert_eq!(icon_pixel_size(64), 64);
        assert_eq!(icon_pixel_size(100_000), MAX_ICON_SIZE);
    }

    // --- open command lines -----------------------------------------------------------------

    #[test]
    fn builds_open_command_lines() {
        let args = open_application_args(
            OpenApplication::BundleId("com.microsoft.VSCode"),
            &[],
            &["--new-window".into(), "/Users/me/project".into()],
        )
        .unwrap();
        assert_eq!(
            args,
            [
                "-n",
                "-b",
                "com.microsoft.VSCode",
                "--args",
                "--new-window",
                "/Users/me/project"
            ]
            .map(OsString::from)
            .to_vec()
        );
        let args = open_application_args(
            OpenApplication::Path(Path::new("/Applications/Foo.app")),
            &[("LANG".into(), "en_US.UTF-8".into())],
            &[],
        )
        .unwrap();
        assert_eq!(
            args,
            [
                "-n",
                "-a",
                "/Applications/Foo.app",
                "--env",
                "LANG=en_US.UTF-8"
            ]
            .map(OsString::from)
            .to_vec()
        );
        assert!(open_application_args(OpenApplication::BundleId("-x"), &[], &[]).is_err());
        assert!(
            open_application_args(OpenApplication::Path(Path::new("Foo.app")), &[], &[]).is_err()
        );
        assert!(
            open_application_args(
                OpenApplication::BundleId("com.a.b"),
                &[("A=B".into(), "c".into())],
                &[]
            )
            .is_err()
        );
    }

    #[test]
    fn builds_open_target_arguments() {
        assert_eq!(
            open_target_args("https://example.com").unwrap(),
            vec![OsString::from("https://example.com")]
        );
        assert_eq!(
            open_target_args("-weird name.txt").unwrap(),
            vec![OsString::from("./-weird name.txt")]
        );
        assert!(open_target_args("  ").is_err());
        assert!(open_target_args("a\0b").is_err());
    }
}
