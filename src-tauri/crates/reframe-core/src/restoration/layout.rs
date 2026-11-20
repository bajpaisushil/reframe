//! Monitor mapping and window geometry remapping between display setups.
//!
//! Saved window positions are virtual-desktop coordinates relative to the monitors that were
//! connected at capture time ([`Workspace::monitors`](crate::model::Workspace)). When the
//! display setup changed (laptop undocked, monitor replaced, resolution changed), every saved
//! monitor is mapped to a connected one and windows are moved proportionally into the mapped
//! monitor's work area, then clamped so they are fully visible.
//!
//! All input geometry is sanitized first (coordinates clamped to ±1,000,000, sizes to
//! 1..=1,000,000), so corrupt or hostile data can neither overflow the arithmetic nor produce
//! a window that lands off-screen.

use reframe_platform::{MonitorInfo, Rect};

use crate::model::{MonitorMapping, WindowSnapshot};

/// Smallest width a remapped window is shrunk to (windows that were already smaller keep
/// their size).
pub const MIN_WINDOW_WIDTH: i32 = 320;
/// Smallest height a remapped window is shrunk to (windows that were already smaller keep
/// their size).
pub const MIN_WINDOW_HEIGHT: i32 = 200;

const COORD_LIMIT: i32 = 1_000_000;
const SIZE_LIMIT: i32 = 1_000_000;

/// Map each saved monitor to a connected one. Priority, applied to all saved monitors one
/// rule at a time (the saved primary monitor first, then in saved order):
///
/// 1. a connected monitor with the same id;
/// 2. a connected monitor with identical bounds (an unused one when possible);
/// 3. an unused connected monitor with the same resolution (nearest relative position wins);
/// 4. the unused connected monitor at the nearest relative position on the desktop;
/// 5. otherwise the primary display — reported as `mapped_id: None`.
///
/// Rules 3 and 4 only consider monitors not already taken, so two saved monitors are spread
/// over two connected ones, and a saved monitor that has no counterpart left (e.g. a secondary
/// display that was disconnected) falls back to the primary. `exact` is true when the mapped
/// monitor has identical bounds, i.e. windows return to exactly where they were.
pub fn map_monitors(saved: &[MonitorInfo], current: &[MonitorInfo]) -> Vec<MonitorMapping> {
    let assignment = assign_monitors(saved, current);
    saved
        .iter()
        .zip(assignment)
        .map(|(monitor, mapped)| {
            let target = mapped.map(|i| &current[i]);
            MonitorMapping {
                saved_id: monitor.id.clone(),
                saved_name: monitor.name.clone(),
                mapped_id: target.map(|t| t.id.clone()),
                mapped_name: target.map(|t| t.name.clone()),
                exact: target.is_some_and(|t| same_geometry(monitor, t)),
            }
        })
        .collect()
}

/// Where a saved window should go on the current monitors.
///
/// - The saved monitor is the snapshot's `monitor_id`; when that is missing or unknown, the
///   saved monitor containing the window's center.
/// - If that monitor maps to a connected monitor with identical geometry, the bounds are
///   returned unchanged as long as the window is fully visible on the connected monitors
///   (otherwise it is moved into the monitor's work area).
/// - Otherwise the window keeps its position and size *relative to the saved monitor's work
///   area*, scaled into the mapped monitor's work area, shrunk no further than
///   [`MIN_WINDOW_WIDTH`]×[`MIN_WINDOW_HEIGHT`] and clamped to be fully visible.
/// - Without any saved reference monitor, a window that is fully visible stays where it is;
///   otherwise it is moved into the monitor containing its center, else into the primary.
///
/// The result is never off-screen, except when no usable monitor is connected at all (an empty
/// or degenerate `current` list): then the bounds are returned unchanged.
pub fn remap_bounds(
    window: &WindowSnapshot,
    saved: &[MonitorInfo],
    current: &[MonitorInfo],
) -> Rect {
    let Some(primary) = primary_index(current) else {
        return window.bounds;
    };
    let rect = sanitize_window(window.bounds);
    let (cx, cy) = rect.center();
    let saved_ref = window
        .monitor_id
        .as_deref()
        .and_then(|id| saved.iter().position(|m| m.id == id && is_valid(m)))
        .or_else(|| {
            saved
                .iter()
                .position(|m| is_valid(m) && sanitize(m.bounds).contains_point(cx, cy))
        });

    match saved_ref {
        Some(si) => {
            let source = &saved[si];
            let target = &current[assign_monitors(saved, current)[si].unwrap_or(primary)];
            if same_geometry(source, target) {
                if fully_visible(rect, current) {
                    return rect;
                }
                return fit_into(rect, usable_area(target));
            }
            let destination = usable_area(target);
            let scaled = scale_between(rect, usable_area(source), destination);
            fit_into(enforce_min_size(scaled, rect), destination)
        }
        None => {
            if fully_visible(rect, current) {
                return rect;
            }
            let containing = current
                .iter()
                .position(|m| is_valid(m) && sanitize(m.bounds).contains_point(cx, cy))
                .unwrap_or(primary);
            fit_into(rect, usable_area(&current[containing]))
        }
    }
}

/// For each saved monitor, the index of the connected monitor it maps to (`None` = primary
/// fallback). See [`map_monitors`] for the rules.
fn assign_monitors(saved: &[MonitorInfo], current: &[MonitorInfo]) -> Vec<Option<usize>> {
    let mut result: Vec<Option<usize>> = vec![None; saved.len()];
    let candidates: Vec<usize> = (0..current.len())
        .filter(|&i| is_valid(&current[i]))
        .collect();
    if candidates.is_empty() {
        return result;
    }
    let mut used = vec![false; current.len()];
    let mut order: Vec<usize> = (0..saved.len()).collect();
    order.sort_by_key(|&i| (!saved[i].is_primary, i));

    // 1. Same id.
    for &i in &order {
        if let Some(&c) = candidates
            .iter()
            .find(|&&c| !used[c] && current[c].id == saved[i].id)
        {
            result[i] = Some(c);
            used[c] = true;
        }
    }

    // 2. Identical bounds (prefer a monitor nobody else uses; mirrored setups may share).
    for &i in &order {
        if result[i].is_some() || !is_valid(&saved[i]) {
            continue;
        }
        let bounds = sanitize(saved[i].bounds);
        if let Some(c) = candidates
            .iter()
            .copied()
            .filter(|&c| sanitize(current[c].bounds) == bounds)
            .min_by_key(|&c| (used[c], c))
        {
            result[i] = Some(c);
            used[c] = true;
        }
    }

    let saved_desktop = desktop_bounds(saved.iter().filter(|m| is_valid(m)));
    let current_desktop = desktop_bounds(candidates.iter().map(|&c| &current[c]));
    let (Some(saved_desktop), Some(current_desktop)) = (saved_desktop, current_desktop) else {
        return result;
    };
    let distance = |s: &MonitorInfo, c: &MonitorInfo| {
        let (sx, sy) = relative_center(sanitize(s.bounds), saved_desktop);
        let (tx, ty) = relative_center(sanitize(c.bounds), current_desktop);
        (sx - tx).hypot(sy - ty)
    };
    let nearest_unused = |i: usize, used: &mut [bool], same_size: bool| {
        let size = {
            let b = sanitize(saved[i].bounds);
            (b.width, b.height)
        };
        let pick = candidates
            .iter()
            .copied()
            .filter(|&c| !used[c])
            .filter(|&c| {
                let b = sanitize(current[c].bounds);
                !same_size || (b.width, b.height) == size
            })
            .min_by(|&a, &b| {
                distance(&saved[i], &current[a])
                    .total_cmp(&distance(&saved[i], &current[b]))
                    .then(current[b].is_primary.cmp(&current[a].is_primary))
                    .then(a.cmp(&b))
            });
        if let Some(c) = pick {
            used[c] = true;
        }
        pick
    };

    // 3. Same resolution, then 4. nearest relative position — unused monitors only.
    for same_size in [true, false] {
        for &i in &order {
            if result[i].is_none() && is_valid(&saved[i]) {
                result[i] = nearest_unused(i, &mut used, same_size);
            }
        }
    }
    // 5. Everything left falls back to the primary display (`None`).
    result
}

/// Index of the primary connected monitor (the first usable one when none is flagged).
fn primary_index(current: &[MonitorInfo]) -> Option<usize> {
    current
        .iter()
        .position(|m| m.is_primary && is_valid(m))
        .or_else(|| current.iter().position(is_valid))
}

fn sanitize(r: Rect) -> Rect {
    Rect::new(
        r.x.clamp(-COORD_LIMIT, COORD_LIMIT),
        r.y.clamp(-COORD_LIMIT, COORD_LIMIT),
        r.width.clamp(0, SIZE_LIMIT),
        r.height.clamp(0, SIZE_LIMIT),
    )
}

/// Saved window bounds with a usable size (degenerate sizes get the minimum size).
fn sanitize_window(r: Rect) -> Rect {
    let mut r = sanitize(r);
    if r.width <= 0 {
        r.width = MIN_WINDOW_WIDTH;
    }
    if r.height <= 0 {
        r.height = MIN_WINDOW_HEIGHT;
    }
    r
}

fn is_valid(m: &MonitorInfo) -> bool {
    !sanitize(m.bounds).is_empty()
}

fn same_geometry(a: &MonitorInfo, b: &MonitorInfo) -> bool {
    sanitize(a.bounds) == sanitize(b.bounds)
}

/// The monitor's work area, restricted to its bounds (some platforms report a desktop-wide
/// work area); the full bounds when the work area is missing or unusable.
fn usable_area(m: &MonitorInfo) -> Rect {
    let bounds = sanitize(m.bounds);
    sanitize(m.work_area)
        .intersection(&bounds)
        .filter(|r| !r.is_empty())
        .unwrap_or(bounds)
}

/// Bounding box of all monitors.
fn desktop_bounds<'a>(monitors: impl Iterator<Item = &'a MonitorInfo>) -> Option<Rect> {
    monitors.map(|m| sanitize(m.bounds)).reduce(|a, b| {
        let x = a.x.min(b.x);
        let y = a.y.min(b.y);
        Rect::new(
            x,
            y,
            a.right().max(b.right()) - x,
            a.bottom().max(b.bottom()) - y,
        )
    })
}

/// Center of `r` relative to `desktop`, in `[0, 1]` per axis.
fn relative_center(r: Rect, desktop: Rect) -> (f64, f64) {
    let (cx, cy) = r.center();
    (
        f64::from(cx - desktop.x) / f64::from(desktop.width.max(1)),
        f64::from(cy - desktop.y) / f64::from(desktop.height.max(1)),
    )
}

/// True when every corner of `r` lies on a connected monitor.
fn fully_visible(r: Rect, monitors: &[MonitorInfo]) -> bool {
    let on_screen = |x: i32, y: i32| {
        monitors
            .iter()
            .filter(|m| is_valid(m))
            .any(|m| sanitize(m.bounds).contains_point(x, y))
    };
    let (right, bottom) = (r.right() - 1, r.bottom() - 1);
    on_screen(r.x, r.y)
        && on_screen(right, r.y)
        && on_screen(r.x, bottom)
        && on_screen(right, bottom)
}

/// Keep `r`'s position and size relative to `from`, expressed relative to `to`.
fn scale_between(r: Rect, from: Rect, to: Rect) -> Rect {
    let sx = f64::from(to.width) / f64::from(from.width.max(1));
    let sy = f64::from(to.height) / f64::from(from.height.max(1));
    Rect::new(
        round(f64::from(to.x) + f64::from(r.x - from.x) * sx),
        round(f64::from(to.y) + f64::from(r.y - from.y) * sy),
        round(f64::from(r.width) * sx).clamp(1, SIZE_LIMIT),
        round(f64::from(r.height) * sy).clamp(1, SIZE_LIMIT),
    )
}

fn round(v: f64) -> i32 {
    // `as` saturates and maps NaN to 0; the clamp keeps later arithmetic overflow-free.
    (v.round() as i32).clamp(-COORD_LIMIT, COORD_LIMIT)
}

/// Do not let scaling shrink a window below the minimum size (or below its original size,
/// when that was already smaller).
fn enforce_min_size(r: Rect, original: Rect) -> Rect {
    let min_width = MIN_WINDOW_WIDTH.min(original.width).max(1);
    let min_height = MIN_WINDOW_HEIGHT.min(original.height).max(1);
    Rect::new(r.x, r.y, r.width.max(min_width), r.height.max(min_height))
}

/// Shrink `r` to fit `area` if needed and move it so it lies completely inside.
fn fit_into(r: Rect, area: Rect) -> Rect {
    let width = clamp(r.width, 1, area.width.max(1));
    let height = clamp(r.height, 1, area.height.max(1));
    Rect::new(
        clamp(r.x, area.x, area.right() - width),
        clamp(r.y, area.y, area.bottom() - height),
        width,
        height,
    )
}

/// `Ord::clamp` without the panic when `hi < lo`.
fn clamp(v: i32, lo: i32, hi: i32) -> i32 {
    if hi < lo { lo } else { v.max(lo).min(hi) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reframe_platform::WindowState;

    fn monitor(id: &str, x: i32, y: i32, w: i32, h: i32, primary: bool) -> MonitorInfo {
        MonitorInfo {
            id: id.into(),
            name: format!("Display {id}"),
            bounds: Rect::new(x, y, w, h),
            work_area: Rect::new(x, y, w, h - 40),
            scale_factor: 1.0,
            is_primary: primary,
        }
    }

    fn window(bounds: Rect, monitor_id: Option<&str>) -> WindowSnapshot {
        WindowSnapshot {
            title: "w".into(),
            class_name: None,
            bounds,
            state: WindowState::Normal,
            monitor_id: monitor_id.map(str::to_string),
        }
    }

    fn mapped(mappings: &[MonitorMapping]) -> Vec<(String, Option<String>, bool)> {
        mappings
            .iter()
            .map(|m| (m.saved_id.clone(), m.mapped_id.clone(), m.exact))
            .collect()
    }

    fn inside(r: Rect, area: Rect) -> bool {
        r.x >= area.x && r.y >= area.y && r.right() <= area.right() && r.bottom() <= area.bottom()
    }

    #[test]
    fn same_id_maps_exactly() {
        let saved = vec![monitor("A", 0, 0, 1920, 1080, true)];
        let current = vec![
            monitor("B", 1920, 0, 1920, 1080, false),
            monitor("A", 0, 0, 1920, 1080, true),
        ];
        assert_eq!(
            mapped(&map_monitors(&saved, &current)),
            vec![("A".into(), Some("A".into()), true)]
        );
    }

    #[test]
    fn same_id_with_new_resolution_is_not_exact() {
        let saved = vec![monitor("A", 0, 0, 2560, 1440, true)];
        let current = vec![monitor("A", 0, 0, 1920, 1080, true)];
        assert_eq!(
            mapped(&map_monitors(&saved, &current)),
            vec![("A".into(), Some("A".into()), false)]
        );
    }

    #[test]
    fn identical_bounds_beat_resolution_and_position() {
        // Output renamed (HDMI-1 -> DP-1) but geometry unchanged.
        let saved = vec![monitor("HDMI-1", 1920, 0, 1920, 1080, false)];
        let current = vec![
            monitor("eDP-1", 0, 0, 1920, 1080, true),
            monitor("DP-1", 1920, 0, 1920, 1080, false),
        ];
        assert_eq!(
            mapped(&map_monitors(&saved, &current)),
            vec![("HDMI-1".into(), Some("DP-1".into()), true)]
        );
    }

    #[test]
    fn same_resolution_then_nearest_position() {
        let saved = vec![
            monitor("L", 0, 0, 1920, 1080, true),
            monitor("R", 1920, 0, 2560, 1440, false),
        ];
        let current = vec![
            monitor("X", 0, 0, 2560, 1440, true),
            monitor("Y", 2560, 0, 1920, 1080, false),
        ];
        // L (primary, processed first) takes the unused 1920x1080 monitor (Y); R then takes
        // the remaining 2560x1440 one (X).
        assert_eq!(
            mapped(&map_monitors(&saved, &current)),
            vec![
                ("L".into(), Some("Y".into()), false),
                ("R".into(), Some("X".into()), false)
            ]
        );

        let saved = vec![
            monitor("L", 0, 0, 1600, 900, true),
            monitor("R", 1600, 0, 1600, 900, false),
        ];
        let current = vec![
            monitor("X", 0, 0, 1920, 1080, true),
            monitor("Y", 1920, 0, 2560, 1440, false),
        ];
        // No id, bounds or resolution match: nearest relative position (left/right).
        assert_eq!(
            mapped(&map_monitors(&saved, &current)),
            vec![
                ("L".into(), Some("X".into()), false),
                ("R".into(), Some("Y".into()), false)
            ]
        );
    }

    #[test]
    fn missing_monitor_falls_back_to_primary() {
        let saved = vec![
            monitor("A", 0, 0, 1920, 1080, true),
            monitor("B", 1920, 0, 1920, 1080, false),
        ];
        let current = vec![monitor("A", 0, 0, 1920, 1080, true)];
        assert_eq!(
            mapped(&map_monitors(&saved, &current)),
            vec![
                ("A".into(), Some("A".into()), true),
                ("B".into(), None, false)
            ]
        );
        assert_eq!(
            mapped(&map_monitors(&saved, &[])),
            vec![("A".into(), None, false), ("B".into(), None, false)]
        );
    }

    #[test]
    fn window_on_unchanged_monitor_is_unchanged() {
        let saved = vec![
            monitor("A", 0, 0, 1920, 1080, true),
            monitor("B", 1920, 0, 1920, 1080, false),
        ];
        let w = window(Rect::new(2000, 50, 800, 600), Some("B"));
        assert_eq!(remap_bounds(&w, &saved, &saved), w.bounds);
        // Straddling two unchanged monitors is fine too.
        let w = window(Rect::new(1500, 50, 800, 600), Some("A"));
        assert_eq!(remap_bounds(&w, &saved, &saved), w.bounds);
    }

    #[test]
    fn window_from_missing_monitor_is_scaled_into_primary() {
        let saved = vec![
            monitor("A", 0, 0, 1920, 1080, true),
            monitor("B", 1920, 0, 1920, 1080, false),
        ];
        let current = vec![monitor("A", 0, 0, 1920, 1080, true)];
        let w = window(Rect::new(1920 + 960, 520, 960, 520), Some("B"));
        let r = remap_bounds(&w, &saved, &current);
        // Same relative position/size in A's work area (B's and A's work areas are equal).
        assert_eq!(r, Rect::new(960, 520, 960, 520));
        assert!(inside(r, usable_area(&current[0])));
    }

    #[test]
    fn proportional_scaling_between_resolutions() {
        let saved = vec![MonitorInfo {
            work_area: Rect::new(0, 0, 2560, 1440),
            ..monitor("A", 0, 0, 2560, 1440, true)
        }];
        let current = vec![MonitorInfo {
            work_area: Rect::new(0, 0, 1280, 720),
            ..monitor("A", 0, 0, 1280, 720, true)
        }];
        // Left half of the screen stays the left half.
        let w = window(Rect::new(0, 0, 1280, 1440), Some("A"));
        assert_eq!(
            remap_bounds(&w, &saved, &current),
            Rect::new(0, 0, 640, 720)
        );
        // Small windows are not shrunk below the minimum size.
        let w = window(Rect::new(100, 100, 400, 300), Some("A"));
        let r = remap_bounds(&w, &saved, &current);
        assert_eq!((r.width, r.height), (MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT));
        assert_eq!((r.x, r.y), (50, 50));
        // ...but windows that were smaller than the minimum keep their size.
        let w = window(Rect::new(100, 100, 150, 100), Some("A"));
        let r = remap_bounds(&w, &saved, &current);
        assert_eq!((r.width, r.height), (150, 100));
    }

    #[test]
    fn unknown_monitor_uses_window_center() {
        let saved = vec![
            monitor("A", 0, 0, 1920, 1080, true),
            monitor("B", 1920, 0, 1920, 1080, false),
        ];
        let current = vec![monitor("A", 0, 0, 1920, 1080, true)];
        // No monitor id: the saved monitor containing the center is B, which is gone.
        let w = window(Rect::new(1920 + 100, 100, 800, 600), None);
        let r = remap_bounds(&w, &saved, &current);
        assert_eq!(r, Rect::new(100, 100, 800, 600));
        // Unknown id and no saved monitors at all: a visible window stays put...
        let w = window(Rect::new(10, 10, 800, 600), Some("GONE"));
        assert_eq!(remap_bounds(&w, &[], &current), w.bounds);
        // ...an off-screen one is pulled into the primary work area.
        let w = window(Rect::new(5000, 3000, 800, 600), Some("GONE"));
        let r = remap_bounds(&w, &[], &current);
        assert!(inside(r, usable_area(&current[0])));
        assert_eq!((r.width, r.height), (800, 600));
    }

    #[test]
    fn never_off_screen_and_never_larger_than_the_work_area() {
        let current = vec![monitor("A", 0, 0, 1366, 768, true)];
        let saved = vec![monitor("BIG", -3840, -2160, 3840, 2160, true)];
        let cases = [
            window(Rect::new(-3840, -2160, 3840, 2160), Some("BIG")),
            window(Rect::new(-100, -100, 5000, 5000), Some("BIG")),
            window(
                Rect::new(i32::MAX - 5, i32::MIN + 5, i32::MAX, -7),
                Some("BIG"),
            ),
            window(
                Rect::new(i32::MAX - 5, i32::MIN + 5, i32::MAX, i32::MAX),
                None,
            ),
            window(Rect::new(0, 0, 0, 0), Some("NOPE")),
        ];
        let area = usable_area(&current[0]);
        for w in &cases {
            let r = remap_bounds(w, &saved, &current);
            assert!(inside(r, area), "{r:?} for {:?}", w.bounds);
            assert!(r.width > 0 && r.height > 0);
        }
    }

    #[test]
    fn empty_or_degenerate_current_monitors_leave_bounds_unchanged() {
        let saved = vec![monitor("A", 0, 0, 1920, 1080, true)];
        let w = window(Rect::new(5000, 5000, 800, 600), Some("A"));
        assert_eq!(remap_bounds(&w, &saved, &[]), w.bounds);
        let broken = vec![monitor("Z", 0, 0, 0, 0, true)];
        assert_eq!(remap_bounds(&w, &saved, &broken), w.bounds);
    }

    #[test]
    fn exact_monitor_but_partially_off_screen_window_is_pulled_in() {
        let saved = vec![monitor("A", 0, 0, 1920, 1080, true)];
        let w = window(Rect::new(1700, 900, 800, 600), Some("A"));
        let r = remap_bounds(&w, &saved, &saved);
        assert!(inside(r, usable_area(&saved[0])));
        assert_eq!((r.width, r.height), (800, 600));
    }

    /// Deterministic pseudo-random layouts: whatever the saved and connected displays look
    /// like, a remapped window is fully visible and has a usable size.
    #[test]
    fn randomized_layouts_never_produce_off_screen_windows() {
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = |range: i32| -> i32 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            i32::try_from(seed % u64::from(range.unsigned_abs())).unwrap()
        };
        let sizes = [
            (1280, 720),
            (1366, 768),
            (1920, 1080),
            (2560, 1440),
            (3840, 2160),
        ];
        for case in 0..3000 {
            let layout = |count: usize, prefix: &str, next: &mut dyn FnMut(i32) -> i32| {
                let mut x = next(4000) - 2000;
                (0..count)
                    .map(|i| {
                        let (w, h) = sizes[usize::try_from(next(5)).unwrap()];
                        let y = next(1200) - 600;
                        let mut m = monitor(&format!("{prefix}{}", next(4)), x, y, w, h, i == 0);
                        // Work areas: taskbar at the bottom, the left, or missing entirely.
                        m.work_area = match next(3) {
                            0 => Rect::new(x, y, w, h - 48),
                            1 => Rect::new(x + 64, y, w - 64, h),
                            _ => Rect::new(0, 0, 0, 0),
                        };
                        x += w + next(300) - 150; // gaps and overlaps
                        m
                    })
                    .collect::<Vec<_>>()
            };
            let saved_count = usize::try_from(next(4)).unwrap();
            let saved = layout(saved_count, "S", &mut next);
            let current_count = 1 + usize::try_from(next(3)).unwrap();
            let current = layout(current_count, "S", &mut next);
            let monitor_id = match next(3) {
                0 => None,
                1 => saved.first().map(|m| m.id.clone()),
                _ => Some("unknown".to_string()),
            };
            let w = window(
                Rect::new(
                    next(12000) - 6000,
                    next(6000) - 3000,
                    next(5000) - 100,
                    next(3000) - 100,
                ),
                monitor_id.as_deref(),
            );
            let r = remap_bounds(&w, &saved, &current);
            assert!(
                r.width > 0 && r.height > 0 && fully_visible(r, &current),
                "case {case}: {r:?} from {:?} on {saved:?} -> {current:?}",
                w.bounds
            );
        }
    }

    #[test]
    fn desktop_wide_work_area_is_restricted_to_the_monitor() {
        let mut m = monitor("B", 1920, 0, 1920, 1080, false);
        m.work_area = Rect::new(0, 0, 3840, 1040);
        assert_eq!(usable_area(&m), Rect::new(1920, 0, 1920, 1040));
    }
}
