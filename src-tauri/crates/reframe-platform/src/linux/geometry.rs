//! Pure geometry helpers for X11 window management: frame insets, ICCCM window gravity,
//! monitor selection and per-monitor work areas from panel struts.

use crate::Rect;

/// Signed distances between a client window (including its X border) and the rectangle Reframe
/// reports as the window's outer bounds.
///
/// Positive values grow the client rectangle (window-manager decorations from
/// `_NET_FRAME_EXTENTS`); negative values shrink it (invisible client-side shadows from
/// `_GTK_FRAME_EXTENTS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Insets {
    pub left: i32,
    pub right: i32,
    pub top: i32,
    pub bottom: i32,
}

impl Insets {
    /// Combine WM decorations (`[left, right, top, bottom]` of `_NET_FRAME_EXTENTS`) with GTK
    /// client-side shadow extents (same layout, `_GTK_FRAME_EXTENTS`).
    pub fn from_extents(frame: Option<[u32; 4]>, gtk_shadow: Option<[u32; 4]>) -> Self {
        let clamp = |v: u32| i32::try_from(v.min(10_000)).unwrap_or(0);
        let f = frame.unwrap_or([0; 4]).map(clamp);
        let g = gtk_shadow.unwrap_or([0; 4]).map(clamp);
        Insets {
            left: f[0] - g[0],
            right: f[1] - g[1],
            top: f[2] - g[2],
            bottom: f[3] - g[3],
        }
    }

    /// Insets that turn `client` into `frame` (used when a reparenting window manager does not
    /// publish `_NET_FRAME_EXTENTS` but its frame window is known).
    pub fn between(client: Rect, frame: Rect) -> Self {
        Insets {
            left: client.x - frame.x,
            right: frame.right() - client.right(),
            top: client.y - frame.y,
            bottom: frame.bottom() - client.bottom(),
        }
    }

    pub fn horizontal(&self) -> i32 {
        self.left + self.right
    }

    pub fn vertical(&self) -> i32 {
        self.top + self.bottom
    }

    /// Outer bounds for a client rectangle.
    pub fn outer(&self, client: Rect) -> Rect {
        Rect::new(
            client.x - self.left,
            client.y - self.top,
            client.width + self.horizontal(),
            client.height + self.vertical(),
        )
    }

    /// Client rectangle (including the X border) for desired outer bounds.
    pub fn client(&self, outer: Rect) -> Rect {
        Rect::new(
            outer.x + self.left,
            outer.y + self.top,
            outer.width - self.horizontal(),
            outer.height - self.vertical(),
        )
    }
}

/// Client rectangle in root coordinates including the X border: `origin` is the inside
/// top-left corner (what `TranslateCoordinates(window, root, 0, 0)` returns).
pub(crate) fn client_rect(origin: (i32, i32), width: u16, height: u16, border: u16) -> Rect {
    let b = i32::from(border);
    Rect::new(
        origin.0 - b,
        origin.1 - b,
        i32::from(width) + 2 * b,
        i32::from(height) + 2 * b,
    )
}

/// ICCCM `win_gravity` values (`WM_NORMAL_HINTS`), which decide how a window manager
/// interprets the position in a configure request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Gravity {
    #[default]
    NorthWest,
    North,
    NorthEast,
    West,
    Center,
    East,
    SouthWest,
    South,
    SouthEast,
    Static,
}

impl Gravity {
    /// From the X11 protocol value (`1..=10`); anything else (including `0`, "forget") means
    /// the ICCCM default, NorthWest.
    pub fn from_protocol(value: u32) -> Self {
        match value {
            2 => Gravity::North,
            3 => Gravity::NorthEast,
            4 => Gravity::West,
            5 => Gravity::Center,
            6 => Gravity::East,
            7 => Gravity::SouthWest,
            8 => Gravity::South,
            9 => Gravity::SouthEast,
            10 => Gravity::Static,
            _ => Gravity::NorthWest,
        }
    }
}

/// Position to put in a `ConfigureWindow` request so that a window manager honouring ICCCM
/// gravity semantics places the frame's outer top-left corner at `frame_origin`.
///
/// For the reference-point gravities the window manager aligns the frame's reference point
/// (corner, edge centre or centre) with where the client's reference point would be at the
/// requested position; `Static` means the requested position is the client's own position.
pub(crate) fn configure_position(
    frame_origin: (i32, i32),
    insets: Insets,
    gravity: Gravity,
) -> (i32, i32) {
    let (h, v) = (insets.horizontal(), insets.vertical());
    let dx = match gravity {
        Gravity::NorthWest | Gravity::West | Gravity::SouthWest => 0,
        Gravity::North | Gravity::Center | Gravity::South => h / 2,
        Gravity::NorthEast | Gravity::East | Gravity::SouthEast => h,
        Gravity::Static => insets.left,
    };
    let dy = match gravity {
        Gravity::NorthWest | Gravity::North | Gravity::NorthEast => 0,
        Gravity::West | Gravity::Center | Gravity::East => v / 2,
        Gravity::SouthWest | Gravity::South | Gravity::SouthEast => v,
        Gravity::Static => insets.top,
    };
    (frame_origin.0 + dx, frame_origin.1 + dy)
}

/// Client geometry `(x, y, width, height)` for a `_NET_MOVERESIZE_WINDOW` request with
/// NorthWest gravity, so the outer frame lands on `outer`: the position is the frame's
/// top-left corner, the size is the client's size without its X border. Sizes are clamped to
/// at least 1 pixel.
pub(crate) fn moveresize_request(outer: Rect, insets: Insets, border: u16) -> (i32, i32, u32, u32) {
    let client = insets.client(outer);
    let b = 2 * i32::from(border);
    let width = u32::try_from((client.width - b).max(1)).unwrap_or(1);
    let height = u32::try_from((client.height - b).max(1)).unwrap_or(1);
    (outer.x, outer.y, width, height)
}

/// Index of the monitor showing the largest part of `bounds`; if the window is entirely off
/// screen, the monitor whose centre is nearest to the window's centre.
pub(crate) fn best_monitor(bounds: &Rect, monitors: &[Rect]) -> Option<usize> {
    if monitors.is_empty() {
        return None;
    }
    let mut best: Option<(usize, i64)> = None;
    for (i, m) in monitors.iter().enumerate() {
        let area = m.intersection_area(bounds);
        if area > 0 && best.is_none_or(|(_, a)| area > a) {
            best = Some((i, area));
        }
    }
    if let Some((i, _)) = best {
        return Some(i);
    }
    let (cx, cy) = bounds.center();
    monitors
        .iter()
        .enumerate()
        .min_by_key(|(_, m)| {
            let (mx, my) = m.center();
            let dx = i64::from(mx) - i64::from(cx);
            let dy = i64::from(my) - i64::from(cy);
            dx * dx + dy * dy
        })
        .map(|(i, _)| i)
}

/// Space reserved at the edges of the X screen by a panel or dock (`_NET_WM_STRUT_PARTIAL`
/// semantics: thicknesses are measured from the edges of the *root window*, each with the
/// range of the perpendicular axis it covers, inclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Strut {
    pub left: u32,
    pub right: u32,
    pub top: u32,
    pub bottom: u32,
    pub left_start_y: u32,
    pub left_end_y: u32,
    pub right_start_y: u32,
    pub right_end_y: u32,
    pub top_start_x: u32,
    pub top_end_x: u32,
    pub bottom_start_x: u32,
    pub bottom_end_x: u32,
}

impl Strut {
    /// From the 12 values of `_NET_WM_STRUT_PARTIAL`.
    pub fn from_partial(values: &[u32]) -> Option<Self> {
        let v: [u32; 12] = values.get(..12)?.try_into().ok()?;
        Some(Strut {
            left: v[0],
            right: v[1],
            top: v[2],
            bottom: v[3],
            left_start_y: v[4],
            left_end_y: v[5],
            right_start_y: v[6],
            right_end_y: v[7],
            top_start_x: v[8],
            top_end_x: v[9],
            bottom_start_x: v[10],
            bottom_end_x: v[11],
        })
    }

    /// From the 4 values of the legacy `_NET_WM_STRUT` (each strut spans the whole edge).
    pub fn from_legacy(values: &[u32], screen: Rect) -> Option<Self> {
        let v: [u32; 4] = values.get(..4)?.try_into().ok()?;
        let max_x = u32::try_from(screen.width.max(1) - 1).unwrap_or(0);
        let max_y = u32::try_from(screen.height.max(1) - 1).unwrap_or(0);
        Some(Strut {
            left: v[0],
            right: v[1],
            top: v[2],
            bottom: v[3],
            left_start_y: 0,
            left_end_y: max_y,
            right_start_y: 0,
            right_end_y: max_y,
            top_start_x: 0,
            top_end_x: max_x,
            bottom_start_x: 0,
            bottom_end_x: max_x,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.left == 0 && self.right == 0 && self.top == 0 && self.bottom == 0
    }
}

fn ranges_overlap(start: u32, end: u32, lo: i32, hi_exclusive: i32) -> bool {
    let (start, end) = (i64::from(start), i64::from(end));
    start <= end && start < i64::from(hi_exclusive) && end >= i64::from(lo)
}

/// Shrink `work` (initially a monitor's bounds) by the struts that reserve space on that
/// monitor. A strut applies to a monitor when its covered range overlaps the monitor and its
/// inner edge falls inside the monitor; this keeps a dock on one monitor from eating the
/// whole neighbouring monitor (struts are relative to the screen, not to monitors).
pub(crate) fn apply_struts(monitor: Rect, mut work: Rect, screen: Rect, struts: &[Strut]) -> Rect {
    let to_i32 = |v: u32| i32::try_from(v).unwrap_or(i32::MAX);
    for s in struts {
        if s.left > 0 && ranges_overlap(s.left_start_y, s.left_end_y, monitor.y, monitor.bottom()) {
            let edge = screen.x.saturating_add(to_i32(s.left));
            if edge > monitor.x && edge < monitor.right() && edge > work.x {
                work.width -= edge - work.x;
                work.x = edge;
            }
        }
        if s.right > 0
            && ranges_overlap(s.right_start_y, s.right_end_y, monitor.y, monitor.bottom())
        {
            let edge = screen.right().saturating_sub(to_i32(s.right));
            if edge > monitor.x && edge < monitor.right() && edge < work.right() {
                work.width = edge - work.x;
            }
        }
        if s.top > 0 && ranges_overlap(s.top_start_x, s.top_end_x, monitor.x, monitor.right()) {
            let edge = screen.y.saturating_add(to_i32(s.top));
            if edge > monitor.y && edge < monitor.bottom() && edge > work.y {
                work.height -= edge - work.y;
                work.y = edge;
            }
        }
        if s.bottom > 0
            && ranges_overlap(s.bottom_start_x, s.bottom_end_x, monitor.x, monitor.right())
        {
            let edge = screen.bottom().saturating_sub(to_i32(s.bottom));
            if edge > monitor.y && edge < monitor.bottom() && edge < work.bottom() {
                work.height = edge - work.y;
            }
        }
    }
    if work.is_empty() { monitor } else { work }
}

/// Work area of one monitor.
///
/// `_NET_WORKAREA` is a single rectangle for the whole X screen, so with several monitors a
/// panel on one of them would also shrink the others. When panel struts are known and there
/// are several monitors, the struts alone decide; otherwise the monitor is intersected with
/// `_NET_WORKAREA` (which also covers window-manager margins and panels drawn by the
/// compositor itself, e.g. GNOME Shell) and then reduced by the struts.
pub(crate) fn work_area(
    monitor: Rect,
    monitor_count: usize,
    net_workarea: Option<Rect>,
    screen: Rect,
    struts: &[Strut],
) -> Rect {
    let base = if monitor_count > 1 && !struts.is_empty() {
        monitor
    } else {
        net_workarea
            .and_then(|wa| wa.intersection(&monitor))
            .unwrap_or(monitor)
    };
    apply_struts(monitor, base, screen, struts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insets_round_trip_frame_extents() {
        // openbox: left 1, right 1, top 20, bottom 5
        let insets = Insets::from_extents(Some([1, 1, 20, 5]), None);
        let client = Rect::new(398, 389, 484, 316);
        let outer = insets.outer(client);
        assert_eq!(outer, Rect::new(397, 369, 486, 341));
        assert_eq!(insets.client(outer), client);
    }

    #[test]
    fn gtk_shadows_shrink_the_outer_bounds() {
        let insets = Insets::from_extents(None, Some([26, 26, 23, 29]));
        let client = Rect::new(100, 100, 852, 652);
        assert_eq!(insets.outer(client), Rect::new(126, 123, 800, 600));
        assert_eq!(insets.client(Rect::new(126, 123, 800, 600)), client);
    }

    #[test]
    fn insets_between_client_and_frame() {
        let client = Rect::new(10, 30, 100, 50);
        let frame = Rect::new(8, 10, 104, 75);
        let insets = Insets::between(client, frame);
        assert_eq!(
            insets,
            Insets {
                left: 2,
                right: 2,
                top: 20,
                bottom: 5
            }
        );
        assert_eq!(insets.outer(client), frame);
    }

    #[test]
    fn client_rect_includes_border() {
        assert_eq!(
            client_rect((10, 20), 100, 50, 0),
            Rect::new(10, 20, 100, 50)
        );
        assert_eq!(client_rect((10, 20), 100, 50, 2), Rect::new(8, 18, 104, 54));
    }

    #[test]
    fn gravity_reference_points() {
        let insets = Insets {
            left: 2,
            right: 4,
            top: 20,
            bottom: 6,
        };
        let frame = (100, 200);
        assert_eq!(
            configure_position(frame, insets, Gravity::NorthWest),
            (100, 200)
        );
        assert_eq!(
            configure_position(frame, insets, Gravity::North),
            (103, 200)
        );
        assert_eq!(
            configure_position(frame, insets, Gravity::NorthEast),
            (106, 200)
        );
        assert_eq!(configure_position(frame, insets, Gravity::West), (100, 213));
        assert_eq!(
            configure_position(frame, insets, Gravity::Center),
            (103, 213)
        );
        assert_eq!(configure_position(frame, insets, Gravity::East), (106, 213));
        assert_eq!(
            configure_position(frame, insets, Gravity::SouthWest),
            (100, 226)
        );
        assert_eq!(
            configure_position(frame, insets, Gravity::South),
            (103, 226)
        );
        assert_eq!(
            configure_position(frame, insets, Gravity::SouthEast),
            (106, 226)
        );
        // Static: the requested position is the client's own position.
        assert_eq!(
            configure_position(frame, insets, Gravity::Static),
            (102, 220)
        );
    }

    #[test]
    fn gravity_from_protocol_values() {
        assert_eq!(Gravity::from_protocol(0), Gravity::NorthWest);
        assert_eq!(Gravity::from_protocol(1), Gravity::NorthWest);
        assert_eq!(Gravity::from_protocol(5), Gravity::Center);
        assert_eq!(Gravity::from_protocol(10), Gravity::Static);
        assert_eq!(Gravity::from_protocol(99), Gravity::NorthWest);
    }

    #[test]
    fn moveresize_uses_frame_position_and_client_size() {
        let insets = Insets::from_extents(Some([1, 1, 20, 5]), None);
        let outer = Rect::new(100, 120, 640, 400);
        assert_eq!(moveresize_request(outer, insets, 0), (100, 120, 638, 375));
        assert_eq!(moveresize_request(outer, insets, 2), (100, 120, 634, 371));
        // Never asks for a zero or negative size.
        assert_eq!(
            moveresize_request(Rect::new(0, 0, 1, 1), insets, 0),
            (0, 0, 1, 1)
        );
    }

    #[test]
    fn best_monitor_prefers_largest_intersection() {
        let monitors = [Rect::new(0, 0, 1280, 1080), Rect::new(1280, 0, 1280, 1080)];
        assert_eq!(
            best_monitor(&Rect::new(100, 100, 400, 300), &monitors),
            Some(0)
        );
        assert_eq!(
            best_monitor(&Rect::new(1200, 0, 400, 300), &monitors),
            Some(1)
        );
        // Equal split: the first monitor wins (stable).
        assert_eq!(
            best_monitor(&Rect::new(1180, 0, 200, 300), &monitors),
            Some(0)
        );
        // Off screen: nearest centre.
        assert_eq!(
            best_monitor(&Rect::new(4000, 0, 100, 100), &monitors),
            Some(1)
        );
        assert_eq!(
            best_monitor(&Rect::new(-900, 0, 100, 100), &monitors),
            Some(0)
        );
        assert_eq!(best_monitor(&Rect::new(0, 0, 10, 10), &[]), None);
    }

    #[test]
    fn struts_only_affect_their_monitor() {
        let screen = Rect::new(0, 0, 2560, 1080);
        let left = Rect::new(0, 0, 1280, 1080);
        let right = Rect::new(1280, 0, 1280, 1080);
        // A 30px top panel spanning only the left monitor.
        let top_panel = Strut {
            top: 30,
            top_start_x: 0,
            top_end_x: 1279,
            ..Default::default()
        };
        assert_eq!(
            apply_struts(left, left, screen, &[top_panel]),
            Rect::new(0, 30, 1280, 1050)
        );
        assert_eq!(apply_struts(right, right, screen, &[top_panel]), right);

        // A 48px dock on the left edge of the *right* monitor is expressed as a 1328px left
        // strut; it must not swallow the left monitor.
        let dock = Strut {
            left: 1328,
            left_start_y: 0,
            left_end_y: 1079,
            ..Default::default()
        };
        assert_eq!(apply_struts(left, left, screen, &[dock]), left);
        assert_eq!(
            apply_struts(right, right, screen, &[dock]),
            Rect::new(1328, 0, 1232, 1080)
        );
    }

    #[test]
    fn bottom_and_right_struts_with_uneven_monitors() {
        // Left monitor 1920x1200, right monitor 1920x1080 aligned at the top.
        let screen = Rect::new(0, 0, 3840, 1200);
        let right = Rect::new(1920, 0, 1920, 1080);
        // Bottom panel on the right monitor occupying y 1050..1080 => 150px from the screen
        // bottom.
        let panel = Strut {
            bottom: 150,
            bottom_start_x: 1920,
            bottom_end_x: 3839,
            ..Default::default()
        };
        assert_eq!(
            apply_struts(right, right, screen, &[panel]),
            Rect::new(1920, 0, 1920, 1050)
        );
        let side = Strut {
            right: 64,
            right_start_y: 0,
            right_end_y: 1199,
            ..Default::default()
        };
        assert_eq!(
            apply_struts(right, right, screen, &[side]),
            Rect::new(1920, 0, 1856, 1080)
        );
    }

    #[test]
    fn legacy_struts_span_the_whole_edge() {
        let screen = Rect::new(0, 0, 1920, 1080);
        let s = Strut::from_legacy(&[0, 0, 0, 40], screen).unwrap();
        assert_eq!(
            apply_struts(screen, screen, screen, &[s]),
            Rect::new(0, 0, 1920, 1040)
        );
        assert!(Strut::from_legacy(&[1, 2], screen).is_none());
        assert!(Strut::from_partial(&[0; 11]).is_none());
        assert!(Strut::from_partial(&[0; 12]).unwrap().is_empty());
    }

    #[test]
    fn work_area_combines_net_workarea_and_struts() {
        let screen = Rect::new(0, 0, 2560, 1080);
        let left = Rect::new(0, 0, 1280, 1080);
        let right = Rect::new(1280, 0, 1280, 1080);
        let net = Some(Rect::new(0, 30, 2560, 1050));
        // No struts known (e.g. GNOME Shell draws its own panel): trust _NET_WORKAREA.
        assert_eq!(
            work_area(right, 2, net, screen, &[]),
            Rect::new(1280, 30, 1280, 1050)
        );
        // Struts known on a multi-monitor setup: they decide per monitor.
        let top_panel = Strut {
            top: 30,
            top_start_x: 0,
            top_end_x: 1279,
            ..Default::default()
        };
        assert_eq!(work_area(right, 2, net, screen, &[top_panel]), right);
        assert_eq!(
            work_area(left, 2, net, screen, &[top_panel]),
            Rect::new(0, 30, 1280, 1050)
        );
        // Single monitor: intersection with _NET_WORKAREA (keeps WM margins).
        let single = Rect::new(0, 0, 1920, 1080);
        assert_eq!(
            work_area(single, 1, Some(Rect::new(4, 4, 1912, 1036)), single, &[]),
            Rect::new(4, 4, 1912, 1036)
        );
        // A bogus _NET_WORKAREA outside the monitor is ignored.
        assert_eq!(
            work_area(single, 1, Some(Rect::new(5000, 0, 10, 10)), single, &[]),
            single
        );
    }
}
