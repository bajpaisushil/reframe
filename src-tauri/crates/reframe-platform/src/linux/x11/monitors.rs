//! Monitors (`Platform::monitors`) through RandR.
//!
//! RandR 1.5 `GetMonitors` lists *logical* monitors, including ones defined with
//! `xrandr --setmonitor` (split screens, tiled displays). Older servers fall back to active
//! CRTCs of connected outputs, and without RandR the whole X screen is one monitor.
//!
//! Limitations: X11 has one global DPI, so every monitor reports the same `scale_factor`
//! (`Xft.dpi / 96`, default 1.0). When no monitor is flagged primary, the first one is
//! reported as primary.

use x11rb::errors::ReplyError;
use x11rb::protocol::randr::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};

use super::{Conn, XResult, first_u32, reply_opt, u32s};
use crate::linux::ewmh;
use crate::linux::geometry::{self, Strut};
use crate::{MonitorInfo, Rect};

/// A monitor before work-area and scale computation.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RawMonitor {
    pub id: String,
    pub name: String,
    pub bounds: Rect,
    pub primary: bool,
}

/// Length (in 32-bit units) read from the EDID output property; the model name lives in the
/// 128-byte base block.
const EDID_LEN: u32 = 128;

fn edid_names(
    c: &Conn,
    outputs: &[Option<randr::Output>],
    wanted: bool,
) -> XResult<Vec<Option<String>>> {
    if !wanted {
        return Ok(vec![None; outputs.len()]);
    }
    let cookies = outputs
        .iter()
        .map(|o| match o {
            Some(output) => c
                .conn
                .randr_get_output_property(
                    *output,
                    c.atoms.EDID,
                    AtomEnum::ANY,
                    0,
                    EDID_LEN,
                    false,
                    false,
                )
                .map(Some),
            None => Ok(None),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut names = Vec::with_capacity(cookies.len());
    for cookie in cookies {
        names.push(match cookie {
            Some(cookie) => reply_opt(cookie)?.and_then(|r| ewmh::edid_monitor_name(&r.data)),
            None => None,
        });
    }
    Ok(names)
}

fn randr_monitors(c: &Conn, with_names: bool) -> XResult<Option<Vec<RawMonitor>>> {
    if c.randr.is_none_or(|v| v < (1, 5)) {
        return Ok(None);
    }
    let reply = match c.conn.randr_get_monitors(c.root, true)?.reply() {
        Ok(reply) => reply,
        Err(ReplyError::X11Error(_)) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let monitors: Vec<_> = reply
        .monitors
        .into_iter()
        .filter(|m| m.width > 0 && m.height > 0)
        .collect();
    let names = c.atom_names(&monitors.iter().map(|m| m.name).collect::<Vec<_>>())?;
    let edid = edid_names(
        c,
        &monitors
            .iter()
            .map(|m| m.outputs.first().copied())
            .collect::<Vec<_>>(),
        with_names,
    )?;
    Ok(Some(
        monitors
            .iter()
            .zip(names)
            .zip(edid)
            .enumerate()
            .map(|(i, ((m, name), edid))| {
                let id = name.unwrap_or_else(|| format!("monitor-{i}"));
                RawMonitor {
                    name: edid.unwrap_or_else(|| id.clone()),
                    id,
                    bounds: Rect::new(
                        i32::from(m.x),
                        i32::from(m.y),
                        i32::from(m.width),
                        i32::from(m.height),
                    ),
                    primary: m.primary,
                }
            })
            .collect(),
    ))
}

fn randr_outputs(c: &Conn, with_names: bool) -> XResult<Option<Vec<RawMonitor>>> {
    if c.randr.is_none_or(|v| v < (1, 3)) {
        return Ok(None);
    }
    let Some(resources) = reply_opt(c.conn.randr_get_screen_resources_current(c.root)?)? else {
        return Ok(None);
    };
    let primary = reply_opt(c.conn.randr_get_output_primary(c.root)?)?
        .map(|r| r.output)
        .unwrap_or(0);
    let infos = resources
        .outputs
        .iter()
        .map(|&o| c.conn.randr_get_output_info(o, resources.config_timestamp))
        .collect::<Result<Vec<_>, _>>()?;
    let mut active = Vec::new();
    for (&output, cookie) in resources.outputs.iter().zip(infos) {
        if let Some(info) = reply_opt(cookie)?
            && info.connection == randr::Connection::CONNECTED
            && info.crtc != 0
        {
            active.push((
                output,
                info.crtc,
                String::from_utf8_lossy(&info.name).into_owned(),
            ));
        }
    }
    let crtcs = active
        .iter()
        .map(|(_, crtc, _)| {
            c.conn
                .randr_get_crtc_info(*crtc, resources.config_timestamp)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let edid = edid_names(
        c,
        &active.iter().map(|(o, _, _)| Some(*o)).collect::<Vec<_>>(),
        with_names,
    )?;
    let mut monitors: Vec<RawMonitor> = Vec::new();
    for (((output, _, name), cookie), edid) in active.into_iter().zip(crtcs).zip(edid) {
        let Some(crtc) = reply_opt(cookie)? else {
            continue;
        };
        if crtc.width == 0 || crtc.height == 0 {
            continue;
        }
        let bounds = Rect::new(
            i32::from(crtc.x),
            i32::from(crtc.y),
            i32::from(crtc.width),
            i32::from(crtc.height),
        );
        let is_primary = output == primary;
        // Cloned outputs share a CRTC area: report it once (prefer the primary output).
        if let Some(existing) = monitors.iter_mut().find(|m| m.bounds == bounds) {
            if is_primary && !existing.primary {
                *existing = RawMonitor {
                    name: edid.unwrap_or_else(|| name.clone()),
                    id: name,
                    bounds,
                    primary: true,
                };
            }
            continue;
        }
        monitors.push(RawMonitor {
            name: edid.unwrap_or_else(|| name.clone()),
            id: name,
            bounds,
            primary: is_primary,
        });
    }
    Ok(Some(monitors))
}

/// The X screen (root window) as it is now. RandR resizes the screen when monitors are added
/// or removed, so the size announced when the connection was set up goes stale.
fn screen_rect(c: &Conn) -> XResult<Rect> {
    let root = c.conn.get_geometry(c.root)?.reply()?;
    Ok(Rect::new(
        0,
        0,
        i32::from(root.width),
        i32::from(root.height),
    ))
}

fn screen_monitor(screen: Rect) -> RawMonitor {
    RawMonitor {
        id: "screen".to_string(),
        name: "X screen".to_string(),
        bounds: screen,
        primary: true,
    }
}

/// Monitor rectangles: RandR monitors, else RandR outputs, else the whole screen. Exactly
/// one monitor is flagged primary. `with_names` reads EDID model names for `name` (otherwise
/// `name` is the RandR name).
pub(super) fn monitor_rects(c: &Conn, with_names: bool) -> XResult<Vec<RawMonitor>> {
    let mut monitors = match randr_monitors(c, with_names)? {
        Some(m) if !m.is_empty() => m,
        _ => match randr_outputs(c, with_names)? {
            Some(m) if !m.is_empty() => m,
            _ => vec![screen_monitor(screen_rect(c)?)],
        },
    };
    let mut seen_primary = false;
    for m in &mut monitors {
        m.primary = m.primary && !seen_primary;
        seen_primary |= m.primary;
    }
    if !seen_primary && let Some(first) = monitors.first_mut() {
        first.primary = true;
    }
    Ok(monitors)
}

/// The `_NET_WORKAREA` rectangle of `desktop` (the first desktop's when the list is shorter).
/// Any client can write the property: values outside X11's 16-bit coordinate space are
/// garbage and make the rectangle unusable (they would also overflow `Rect` arithmetic).
fn net_workarea(values: &[u32], desktop: u32) -> Option<Rect> {
    let v = usize::try_from(desktop)
        .ok()
        .and_then(|d| d.checked_mul(4))
        .and_then(|start| values.get(start..start.checked_add(4)?))
        .or_else(|| values.get(..4))?;
    let pos = |v: u32| i16::try_from(v).ok().map(i32::from);
    let size = |v: u32| u16::try_from(v).ok().map(i32::from);
    Some(Rect::new(pos(v[0])?, pos(v[1])?, size(v[2])?, size(v[3])?)).filter(|r| !r.is_empty())
}

/// `_NET_WORKAREA` of the current desktop and the struts of all managed windows.
fn work_area_inputs(c: &Conn, screen: Rect) -> XResult<(Option<Rect>, Vec<Strut>)> {
    let a = &c.atoms;
    let workarea = c.prop(c.root, a._NET_WORKAREA, 4 * 256)?;
    let current = c.prop(c.root, a._NET_CURRENT_DESKTOP, 1)?;
    let clients = c.prop(c.root, a._NET_CLIENT_LIST, 1 << 16)?;
    let workarea = reply_opt(workarea)?.map(|r| u32s(&r)).unwrap_or_default();
    let desktop = reply_opt(current)?
        .as_ref()
        .and_then(first_u32)
        .unwrap_or(0);
    let net_workarea = net_workarea(&workarea, desktop);

    let clients = reply_opt(clients)?.map(|r| u32s(&r)).unwrap_or_default();
    let cookies = clients
        .iter()
        .map(|&w| {
            Ok((
                c.prop(w, a._NET_WM_STRUT_PARTIAL, 12)?,
                c.prop(w, a._NET_WM_STRUT, 4)?,
            ))
        })
        .collect::<XResult<Vec<_>>>()?;
    let mut struts = Vec::new();
    for (partial, legacy) in cookies {
        let partial = reply_opt(partial)?.and_then(|r| Strut::from_partial(&u32s(&r)));
        let legacy = reply_opt(legacy)?.and_then(|r| Strut::from_legacy(&u32s(&r), screen));
        if let Some(strut) = partial.or(legacy).filter(|s| !s.is_empty()) {
            struts.push(strut);
        }
    }
    Ok((net_workarea, struts))
}

/// Scale factor from `Xft.dpi` in the `RESOURCE_MANAGER` property (1.0 when unset).
fn xft_scale(c: &Conn) -> XResult<f64> {
    let reply = reply_opt(c.conn.get_property(
        false,
        c.root,
        AtomEnum::RESOURCE_MANAGER,
        AtomEnum::STRING,
        0,
        1 << 16,
    )?)?;
    Ok(reply
        .filter(|r| r.format == 8)
        .and_then(|r| ewmh::scale_from_resources(&String::from_utf8_lossy(&r.value)))
        .unwrap_or(1.0))
}

/// List monitors with work areas and scale.
pub(crate) fn list_monitors(c: &Conn) -> XResult<Vec<MonitorInfo>> {
    let raw = monitor_rects(c, true)?;
    let screen = screen_rect(c)?;
    let (net_workarea, struts) = work_area_inputs(c, screen)?;
    let scale = xft_scale(c)?;
    let count = raw.len();
    Ok(raw
        .into_iter()
        .map(|m| MonitorInfo {
            work_area: geometry::work_area(m.bounds, count, net_workarea, screen, &struts),
            id: m.id,
            name: m.name,
            bounds: m.bounds,
            scale_factor: scale,
            is_primary: m.primary,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn net_workarea_of_the_current_desktop() {
        let two_desktops = [0, 30, 2560, 1050, 0, 0, 2560, 1040];
        assert_eq!(
            net_workarea(&two_desktops, 0),
            Some(Rect::new(0, 30, 2560, 1050))
        );
        assert_eq!(
            net_workarea(&two_desktops, 1),
            Some(Rect::new(0, 0, 2560, 1040))
        );
        // A desktop beyond the list (or a bogus index) falls back to the first desktop.
        assert_eq!(
            net_workarea(&two_desktops, 7),
            Some(Rect::new(0, 30, 2560, 1050))
        );
        assert_eq!(
            net_workarea(&two_desktops, u32::MAX),
            Some(Rect::new(0, 30, 2560, 1050))
        );
        assert_eq!(net_workarea(&[], 0), None);
        assert_eq!(net_workarea(&[0, 0, 100], 0), None);
        assert_eq!(net_workarea(&[0, 0, 0, 100], 0), None);
    }

    #[test]
    fn garbage_net_workarea_is_ignored() {
        // Values no X11 screen can have; using them would overflow `Rect::right()`.
        let screen = Rect::new(0, 0, 1920, 1080);
        for garbage in [
            [0x7FFF_FFFF, 0, 0x7FFF_FFFF, 1080],
            [0, 0, 0xFFFF_FFFF, 1080],
            [0xFFFF_FFF0, 0, 1920, 1080],
        ] {
            let wa = net_workarea(&garbage, 0);
            assert_eq!(wa, None, "{garbage:?}");
            assert_eq!(geometry::work_area(screen, 1, wa, screen, &[]), screen);
        }
        // The largest values X11 allows still work.
        assert_eq!(
            net_workarea(&[32767, 32767, 65535, 65535], 0),
            Some(Rect::new(32767, 32767, 65535, 65535))
        );
    }
}
