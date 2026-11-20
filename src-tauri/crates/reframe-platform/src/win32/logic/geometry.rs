//! Rectangle math: Win32 `RECT` conversion, workspace ↔ screen coordinates, the invisible resize
//! border, and window-state detection.
//!
//! Background:
//! - `GetWindowRect` returns the *outer* window rectangle. Since Windows 10, standard frames have
//!   an invisible resize border (about 7 px at 100 % scaling on the left, right and bottom) that
//!   is part of that rectangle. `DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)` returns the
//!   *visible* frame. Reframe reports and accepts visible frames, so saved layouts line up with
//!   what the user sees; [`Insets`] describe the difference.
//! - `WINDOWPLACEMENT::rcNormalPosition` is in *workspace* coordinates for windows without
//!   `WS_EX_TOOLWINDOW`: screen coordinates shifted by the offset between the monitor's work area
//!   and its full bounds (non-zero when a taskbar is docked at the top or left).

use crate::{Rect, WindowState};

/// Coordinates are clamped to ±`COORD_LIMIT` (and sizes to twice that) before any arithmetic,
/// so bogus values from the OS or from saved layouts can never overflow `i32`.
pub const COORD_LIMIT: i32 = 1 << 24;

/// Largest plausible invisible-border thickness; anything larger means the frame data is bogus.
pub const MAX_INSET: i32 = 64;

/// `WS_CAPTION` (`WS_BORDER | WS_DLGFRAME`).
pub const WS_CAPTION: u32 = 0x00C0_0000;
/// `WS_THICKFRAME` (sizing border).
pub const WS_THICKFRAME: u32 = 0x0004_0000;

/// Clamp a rectangle into the range where coordinate arithmetic cannot overflow.
pub fn sanitize(rect: Rect) -> Rect {
    Rect::new(
        rect.x.clamp(-COORD_LIMIT, COORD_LIMIT),
        rect.y.clamp(-COORD_LIMIT, COORD_LIMIT),
        rect.width.clamp(0, 2 * COORD_LIMIT),
        rect.height.clamp(0, 2 * COORD_LIMIT),
    )
}

/// [`Rect`] from Win32 `RECT` edges (right/bottom exclusive). Inverted edges give an empty rect.
pub fn rect_from_edges(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
    let extent = |from: i32, to: i32| {
        let size = (i64::from(to) - i64::from(from)).clamp(0, i64::from(2 * COORD_LIMIT));
        i32::try_from(size).unwrap_or(0)
    };
    sanitize(Rect::new(
        left,
        top,
        extent(left, right),
        extent(top, bottom),
    ))
}

/// Win32 `RECT` edges `(left, top, right, bottom)` of a rectangle.
pub fn edges(rect: Rect) -> (i32, i32, i32, i32) {
    let r = sanitize(rect);
    (r.x, r.y, r.right(), r.bottom())
}

/// Thickness of each edge between an outer rectangle and a rectangle inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Insets {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Insets {
    pub const ZERO: Insets = Insets {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };

    /// Insets of `inner` (the DWM visible frame) within `outer` (`GetWindowRect`). `None` when
    /// `inner` is not inside `outer` or an edge is implausibly thick (bogus frame data).
    pub fn between(outer: Rect, inner: Rect) -> Option<Insets> {
        let (outer, inner) = (sanitize(outer), sanitize(inner));
        let insets = Insets {
            left: inner.x - outer.x,
            top: inner.y - outer.y,
            right: outer.right() - inner.right(),
            bottom: outer.bottom() - inner.bottom(),
        };
        let plausible = |v: i32| (0..=MAX_INSET).contains(&v);
        (plausible(insets.left)
            && plausible(insets.top)
            && plausible(insets.right)
            && plausible(insets.bottom))
        .then_some(insets)
    }

    fn clamped(self) -> Insets {
        let c = |v: i32| v.clamp(0, MAX_INSET);
        Insets {
            left: c(self.left),
            top: c(self.top),
            right: c(self.right),
            bottom: c(self.bottom),
        }
    }
}

/// Shrink `rect` by `insets` (outer window rectangle → visible frame).
pub fn deflate(rect: Rect, insets: Insets) -> Rect {
    let (r, i) = (sanitize(rect), insets.clamped());
    sanitize(Rect::new(
        r.x + i.left,
        r.y + i.top,
        r.width - i.left - i.right,
        r.height - i.top - i.bottom,
    ))
}

/// Grow `rect` by `insets` (visible frame → outer window rectangle).
pub fn inflate(rect: Rect, insets: Insets) -> Rect {
    let (r, i) = (sanitize(rect), insets.clamped());
    sanitize(Rect::new(
        r.x - i.left,
        r.y - i.top,
        r.width + i.left + i.right,
        r.height + i.top + i.bottom,
    ))
}

/// Estimated *normal-state* invisible border of a window, used only while it is maximized or
/// minimized and Reframe has never seen it in the normal state (where the border can be measured
/// exactly). Windows 10/11 standard frames draw a 1 px visible border and leave the rest of the
/// sizing frame (`frame_thickness` = `SM_CXSIZEFRAME + SM_CXPADDEDBORDER` at the window's DPI)
/// invisible on the left, right and bottom; the top edge has no invisible part. Windows without a
/// sizing border get no adjustment. This is a heuristic: custom-drawn frames may differ slightly.
pub fn estimated_insets(resizable: bool, frame_thickness: i32) -> Insets {
    if !resizable || frame_thickness <= 1 {
        return Insets::ZERO;
    }
    let side = (frame_thickness - 1).min(MAX_INSET);
    Insets {
        left: side,
        top: 0,
        right: side,
        bottom: side,
    }
}

/// The invisible-border insets that describe a window's *normal* (restored) frame.
///
/// - Without DWM frame information everything is reported as outer rectangles: no adjustment.
/// - In the normal state the current measurement is exact.
/// - Maximized/minimized/fullscreen windows have a different border than when restored, so a
///   measurement cached while the window was last restored is preferred, then the estimate.
pub fn normal_state_insets(
    state: WindowState,
    dwm_frame_available: bool,
    observed: Option<Insets>,
    cached: Option<Insets>,
    estimate: Insets,
) -> Insets {
    if !dwm_frame_available {
        return Insets::ZERO;
    }
    match state {
        WindowState::Normal => observed.unwrap_or(Insets::ZERO),
        _ => cached.unwrap_or(estimate),
    }
}

/// Geometry of one monitor, as needed for coordinate conversions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorGeometry {
    pub bounds: Rect,
    pub work_area: Rect,
    pub is_primary: bool,
}

impl MonitorGeometry {
    /// Offset that turns workspace coordinates into screen coordinates on this monitor.
    fn workspace_offset(&self) -> (i32, i32) {
        let (b, w) = (sanitize(self.bounds), sanitize(self.work_area));
        (w.x - b.x, w.y - b.y)
    }
}

/// The monitor `MonitorFromRect(rect, MONITOR_DEFAULTTOPRIMARY)` picks: the one with the largest
/// intersection (the first one wins ties), else the primary monitor, else the first monitor.
pub fn monitor_for_rect(monitors: &[MonitorGeometry], rect: Rect) -> Option<usize> {
    let rect = sanitize(rect);
    let probe = if rect.is_empty() {
        Rect::new(rect.x, rect.y, 1, 1)
    } else {
        rect
    };
    let mut best: Option<(usize, i64)> = None;
    for (index, monitor) in monitors.iter().enumerate() {
        let area = sanitize(monitor.bounds).intersection_area(&probe);
        if area > 0 && best.is_none_or(|(_, best_area)| area > best_area) {
            best = Some((index, area));
        }
    }
    best.map(|(index, _)| index)
        .or_else(|| monitors.iter().position(|m| m.is_primary))
        .or_else(|| (!monitors.is_empty()).then_some(0))
}

fn shift(rect: Rect, dx: i32, dy: i32) -> Rect {
    let r = sanitize(rect);
    sanitize(Rect::new(r.x + dx, r.y + dy, r.width, r.height))
}

/// Convert a `WINDOWPLACEMENT` rectangle from workspace to screen coordinates.
///
/// Windows derives workspace coordinates from the monitor the *screen* rectangle is on, so the
/// answer is the shift by some monitor's offset that lands on that same monitor. The monitor
/// under the workspace rectangle is tried first (offsets are only a taskbar wide, so it is
/// nearly always the right one).
pub fn workspace_to_screen(rect: Rect, monitors: &[MonitorGeometry]) -> Rect {
    let rect = sanitize(rect);
    let Some(first) = monitor_for_rect(monitors, rect) else {
        return rect;
    };
    let moved = |index: usize| {
        let (dx, dy) = monitors[index].workspace_offset();
        shift(rect, dx, dy)
    };
    std::iter::once(first)
        .chain((0..monitors.len()).filter(|&i| i != first))
        .map(|index| (index, moved(index)))
        .find(|(index, screen)| monitor_for_rect(monitors, *screen) == Some(*index))
        .map_or_else(|| moved(first), |(_, screen)| screen)
}

/// Convert a screen rectangle to the workspace coordinates `SetWindowPlacement` expects, using
/// the monitor the rectangle is on (the inverse of [`workspace_to_screen`]).
pub fn screen_to_workspace(rect: Rect, monitors: &[MonitorGeometry]) -> Rect {
    let rect = sanitize(rect);
    match monitor_for_rect(monitors, rect) {
        Some(index) => {
            let (dx, dy) = monitors[index].workspace_offset();
            shift(rect, -dx, -dy)
        }
        None => rect,
    }
}

/// Visible restored frame from `WINDOWPLACEMENT::rcNormalPosition`.
pub fn normal_bounds_from_placement(
    normal_position: Rect,
    workspace_coordinates: bool,
    monitors: &[MonitorGeometry],
    insets: Insets,
) -> Rect {
    let screen = if workspace_coordinates {
        workspace_to_screen(normal_position, monitors)
    } else {
        sanitize(normal_position)
    };
    deflate(screen, insets)
}

/// `rcNormalPosition` that gives a window the visible restored frame `visible`.
pub fn placement_for_visible_bounds(
    visible: Rect,
    workspace_coordinates: bool,
    monitors: &[MonitorGeometry],
    insets: Insets,
) -> Rect {
    let outer = inflate(visible, insets);
    if workspace_coordinates {
        screen_to_workspace(outer, monitors)
    } else {
        outer
    }
}

/// `true` when the style has neither a full caption nor a sizing border.
pub fn is_borderless(style: u32) -> bool {
    style & WS_CAPTION != WS_CAPTION && style & WS_THICKFRAME == 0
}

/// `true` when `window` covers all of `monitor`.
pub fn covers_monitor(window: Rect, monitor: Rect) -> bool {
    let (w, m) = (sanitize(window), sanitize(monitor));
    !m.is_empty() && w.x <= m.x && w.y <= m.y && w.right() >= m.right() && w.bottom() >= m.bottom()
}

/// Window state: minimized (`IsIconic`), maximized (`IsZoomed`), fullscreen (a borderless window
/// covering its whole monitor: games, F11 browsers, video players) or normal.
pub fn window_state(
    iconic: bool,
    zoomed: bool,
    style: u32,
    window_rect: Rect,
    monitor_bounds: Option<Rect>,
) -> WindowState {
    if iconic {
        WindowState::Minimized
    } else if zoomed {
        WindowState::Maximized
    } else if is_borderless(style) && monitor_bounds.is_some_and(|m| covers_monitor(window_rect, m))
    {
        WindowState::Fullscreen
    } else {
        WindowState::Normal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STANDARD_INSETS: Insets = Insets {
        left: 7,
        top: 0,
        right: 7,
        bottom: 7,
    };

    fn monitor(bounds: Rect, work_area: Rect, is_primary: bool) -> MonitorGeometry {
        MonitorGeometry {
            bounds,
            work_area,
            is_primary,
        }
    }

    /// Primary 1920x1080 with the taskbar at the top (40 px), secondary 2560x1440 to the left with
    /// a 48 px taskbar on its left edge.
    fn desk() -> Vec<MonitorGeometry> {
        vec![
            monitor(
                Rect::new(0, 0, 1920, 1080),
                Rect::new(0, 40, 1920, 1040),
                true,
            ),
            monitor(
                Rect::new(-2560, -200, 2560, 1440),
                Rect::new(-2512, -200, 2512, 1440),
                false,
            ),
        ]
    }

    #[test]
    fn rect_edges_round_trip() {
        let r = rect_from_edges(-10, 20, 110, 220);
        assert_eq!(r, Rect::new(-10, 20, 120, 200));
        assert_eq!(edges(r), (-10, 20, 110, 220));
    }

    #[test]
    fn inverted_or_extreme_edges_do_not_overflow() {
        assert_eq!(rect_from_edges(100, 100, 50, 50), Rect::new(100, 100, 0, 0));
        let huge = rect_from_edges(i32::MIN, i32::MIN, i32::MAX, i32::MAX);
        assert!(huge.width > 0 && huge.right() > huge.x);
        let r = sanitize(Rect::new(i32::MAX, i32::MIN, i32::MAX, -5));
        assert_eq!(r, Rect::new(COORD_LIMIT, -COORD_LIMIT, 2 * COORD_LIMIT, 0));
        // Arithmetic on sanitized values is safe.
        let _ = inflate(
            Rect::new(i32::MAX, i32::MAX, i32::MAX, i32::MAX),
            STANDARD_INSETS,
        );
        let _ = deflate(Rect::new(i32::MIN, i32::MIN, 3, 3), STANDARD_INSETS);
    }

    #[test]
    fn insets_between_window_rect_and_visible_frame() {
        let outer = Rect::new(93, 100, 814, 607);
        let visible = Rect::new(100, 100, 800, 600);
        assert_eq!(Insets::between(outer, visible), Some(STANDARD_INSETS));
        assert_eq!(Insets::between(visible, visible), Some(Insets::ZERO));
        // Frame outside the window rectangle or absurdly thick borders are rejected.
        assert_eq!(Insets::between(visible, outer), None);
        assert_eq!(
            Insets::between(Rect::new(0, 0, 1000, 1000), Rect::new(200, 0, 100, 100)),
            None
        );
    }

    #[test]
    fn deflate_and_inflate_are_inverse() {
        let visible = Rect::new(100, 100, 800, 600);
        let outer = inflate(visible, STANDARD_INSETS);
        assert_eq!(outer, Rect::new(93, 100, 814, 607));
        assert_eq!(deflate(outer, STANDARD_INSETS), visible);
        // A frame smaller than its borders collapses to an empty rect instead of going negative.
        assert!(deflate(Rect::new(0, 0, 10, 5), STANDARD_INSETS).is_empty());
    }

    #[test]
    fn estimated_insets_follow_the_standard_frame() {
        assert_eq!(estimated_insets(true, 8), STANDARD_INSETS);
        assert_eq!(
            estimated_insets(true, 12),
            Insets {
                left: 11,
                top: 0,
                right: 11,
                bottom: 11
            }
        );
        assert_eq!(estimated_insets(false, 8), Insets::ZERO);
        assert_eq!(estimated_insets(true, 0), Insets::ZERO);
        assert_eq!(estimated_insets(true, 1000).left, MAX_INSET);
    }

    #[test]
    fn normal_state_insets_prefer_exact_measurements() {
        let maximized = Insets {
            left: 8,
            top: 8,
            right: 8,
            bottom: 8,
        };
        let estimate = estimated_insets(true, 8);
        // Normal state: the live measurement is exact.
        assert_eq!(
            normal_state_insets(
                WindowState::Normal,
                true,
                Some(STANDARD_INSETS),
                None,
                estimate
            ),
            STANDARD_INSETS
        );
        // Maximized: the live measurement describes the maximized border, so it is ignored.
        let cached = Insets {
            left: 6,
            top: 0,
            right: 6,
            bottom: 6,
        };
        assert_eq!(
            normal_state_insets(
                WindowState::Maximized,
                true,
                Some(maximized),
                Some(cached),
                estimate
            ),
            cached
        );
        assert_eq!(
            normal_state_insets(WindowState::Minimized, true, None, None, estimate),
            estimate
        );
        // No DWM frame data: everything is reported as outer rectangles.
        for state in [
            WindowState::Normal,
            WindowState::Maximized,
            WindowState::Minimized,
            WindowState::Fullscreen,
        ] {
            assert_eq!(
                normal_state_insets(state, false, None, Some(cached), estimate),
                Insets::ZERO
            );
        }
    }

    #[test]
    fn monitor_for_rect_matches_monitor_from_rect() {
        let monitors = desk();
        assert_eq!(
            monitor_for_rect(&monitors, Rect::new(100, 100, 50, 50)),
            Some(0)
        );
        assert_eq!(
            monitor_for_rect(&monitors, Rect::new(-1000, 0, 50, 50)),
            Some(1)
        );
        // Spanning both: the larger intersection wins.
        assert_eq!(
            monitor_for_rect(&monitors, Rect::new(-300, 0, 400, 100)),
            Some(1)
        );
        assert_eq!(
            monitor_for_rect(&monitors, Rect::new(-100, 0, 400, 100)),
            Some(0)
        );
        // Off every monitor: primary.
        assert_eq!(
            monitor_for_rect(&monitors, Rect::new(50_000, 50_000, 10, 10)),
            Some(0)
        );
        // Empty rect: the monitor under its origin.
        assert_eq!(monitor_for_rect(&monitors, Rect::new(-5, 5, 0, 0)), Some(1));
        assert_eq!(monitor_for_rect(&[], Rect::new(0, 0, 10, 10)), None);
        // No primary flagged: first monitor.
        let mut no_primary = desk();
        no_primary[0].is_primary = false;
        assert_eq!(
            monitor_for_rect(&no_primary, Rect::new(90_000, 0, 1, 1)),
            Some(0)
        );
    }

    #[test]
    fn workspace_conversion_uses_the_monitors_work_area_offset() {
        let monitors = desk();
        // Primary: taskbar on top shifts y by 40.
        let ws = Rect::new(100, 60, 800, 600);
        let screen = workspace_to_screen(ws, &monitors);
        assert_eq!(screen, Rect::new(100, 100, 800, 600));
        assert_eq!(screen_to_workspace(screen, &monitors), ws);
        // Secondary: taskbar on the left shifts x by 48.
        let ws = Rect::new(-2400, 0, 1000, 700);
        let screen = workspace_to_screen(ws, &monitors);
        assert_eq!(screen, Rect::new(-2352, 0, 1000, 700));
        assert_eq!(screen_to_workspace(screen, &monitors), ws);
    }

    #[test]
    fn workspace_conversion_is_identity_without_offsets_or_monitors() {
        let flat = vec![monitor(
            Rect::new(0, 0, 1920, 1080),
            Rect::new(0, 0, 1920, 1040),
            true,
        )];
        let r = Rect::new(10, 20, 300, 200);
        assert_eq!(workspace_to_screen(r, &flat), r);
        assert_eq!(screen_to_workspace(r, &flat), r);
        assert_eq!(workspace_to_screen(r, &[]), r);
        assert_eq!(screen_to_workspace(r, &[]), r);
    }

    #[test]
    fn workspace_conversion_round_trips_near_monitor_edges() {
        let monitors = desk();
        for x in [-2600, -2560, -1300, -700, -400, -60, 0, 5, 900, 1800] {
            for y in [-250, -200, 0, 30, 500, 1000] {
                let screen = Rect::new(x, y, 640, 480);
                let ws = screen_to_workspace(screen, &monitors);
                assert_eq!(
                    workspace_to_screen(ws, &monitors),
                    screen,
                    "round trip failed for {screen:?} via {ws:?}"
                );
            }
        }
    }

    #[test]
    fn placement_conversion_includes_the_invisible_border() {
        let monitors = desk();
        let visible = Rect::new(200, 140, 800, 600);
        let normal_position =
            placement_for_visible_bounds(visible, true, &monitors, STANDARD_INSETS);
        // Outer rect (193, 140, 814, 607) moved into workspace coordinates (taskbar on top).
        assert_eq!(normal_position, Rect::new(193, 100, 814, 607));
        assert_eq!(
            normal_bounds_from_placement(normal_position, true, &monitors, STANDARD_INSETS),
            visible
        );
        // Tool windows use screen coordinates.
        let tool = placement_for_visible_bounds(visible, false, &monitors, STANDARD_INSETS);
        assert_eq!(tool, Rect::new(193, 140, 814, 607));
        assert_eq!(
            normal_bounds_from_placement(tool, false, &monitors, STANDARD_INSETS),
            visible
        );
    }

    #[test]
    fn state_detection() {
        let monitor = Rect::new(0, 0, 1920, 1080);
        let overlapped = WS_CAPTION | WS_THICKFRAME;
        let popup = 0x8000_0000;
        let full = Rect::new(0, 0, 1920, 1080);
        let partial = Rect::new(10, 10, 800, 600);
        assert_eq!(
            window_state(true, true, overlapped, full, Some(monitor)),
            WindowState::Minimized
        );
        assert_eq!(
            window_state(false, true, overlapped, full, Some(monitor)),
            WindowState::Maximized
        );
        assert_eq!(
            window_state(false, false, popup, full, Some(monitor)),
            WindowState::Fullscreen
        );
        // Larger than the monitor still counts (some games overshoot by a few pixels).
        assert_eq!(
            window_state(
                false,
                false,
                popup,
                Rect::new(-1, -1, 1922, 1082),
                Some(monitor)
            ),
            WindowState::Fullscreen
        );
        assert_eq!(
            window_state(false, false, popup, partial, Some(monitor)),
            WindowState::Normal
        );
        // A framed window covering the monitor is just a large normal window.
        assert_eq!(
            window_state(false, false, overlapped, full, Some(monitor)),
            WindowState::Normal
        );
        assert_eq!(
            window_state(false, false, popup, full, None),
            WindowState::Normal
        );
    }

    #[test]
    fn borderless_styles() {
        assert!(is_borderless(0x8000_0000)); // WS_POPUP
        assert!(is_borderless(0x0080_0000)); // WS_BORDER only
        assert!(!is_borderless(WS_CAPTION));
        assert!(!is_borderless(WS_THICKFRAME));
        assert!(!is_borderless(0x00CF_0000)); // WS_OVERLAPPEDWINDOW
    }
}
