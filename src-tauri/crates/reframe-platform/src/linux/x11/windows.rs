//! Window enumeration (`Platform::windows`) and per-window queries.
//!
//! Sources, in order of preference:
//! 1. `_NET_CLIENT_LIST_STACKING` of an EWMH window manager (bottom-to-top stacking order),
//! 2. `_NET_CLIENT_LIST` ordered by the stacking order of the window manager's frames,
//! 3. without an EWMH window manager: top-level windows (or their descendants, for
//!    reparenting window managers) carrying ICCCM `WM_STATE`,
//! 4. with no window manager at all (no window carries `WM_STATE`): mapped top-level windows
//!    that have a `WM_CLASS`.
//!
//! Requests for all windows are pipelined, so one call costs a handful of round trips.

use std::collections::HashMap;

use x11rb::cookie::Cookie;
use x11rb::properties::WmSizeHints;
use x11rb::protocol::res::{ClientIdMask, ClientIdSpec, ConnectionExt as _, QueryClientIdsReply};
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, GetGeometryReply, GetWindowAttributesReply, MapState,
    TranslateCoordinatesReply, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;

use super::monitors::monitor_rects;
use super::{Conn, PropCookie, WmInfo, XResult, detect_wm, first_u32, four_u32, reply_opt, u32s};
use crate::linux::ewmh::{self, IcccmState};
use crate::linux::geometry::{self, Gravity, Insets};
use crate::linux::icons;
use crate::{Rect, WindowId, WindowInfo, WindowState};

/// Longest property values read (in 32-bit units).
const TEXT_LEN: u32 = 2048;
const LIST_LEN: u32 = 1 << 16;
/// `_NET_WM_ICON` is read up to 8 MiB.
const ICON_LEN: u32 = 2 * 1024 * 1024;
/// `_NET_WM_DESKTOP` value meaning "on all desktops".
const ALL_DESKTOPS: u32 = 0xFFFF_FFFF;

/// A window that may be an application window, with the frame window that contains it when
/// that is already known.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    client: Window,
    frame: Option<Window>,
}

/// The top-level window (child of the root) containing `window`; `None` if it vanished.
pub(super) fn toplevel(c: &Conn, window: Window) -> XResult<Option<Window>> {
    let mut current = window;
    for _ in 0..16 {
        let Some(tree) = reply_opt(c.conn.query_tree(current)?)? else {
            return Ok(None);
        };
        if tree.parent == c.root || tree.parent == x11rb::NONE {
            return Ok(Some(current));
        }
        current = tree.parent;
    }
    Ok(Some(current))
}

/// Order `_NET_CLIENT_LIST` (mapping order) by the stacking order of the root's children.
fn order_by_stacking(c: &Conn, clients: &[Window]) -> XResult<Vec<Candidate>> {
    let tree = c.conn.query_tree(c.root)?.reply()?;
    let position: HashMap<Window, usize> = tree
        .children
        .iter()
        .enumerate()
        .map(|(i, &w)| (w, i))
        .collect();
    let mut ordered = Vec::with_capacity(clients.len());
    for &client in clients {
        let top = toplevel(c, client)?;
        let pos = top.and_then(|t| position.get(&t).copied()).unwrap_or(0);
        ordered.push((
            pos,
            Candidate {
                client,
                frame: top.filter(|&t| t != client),
            },
        ));
    }
    ordered.sort_by_key(|(pos, _)| *pos);
    Ok(ordered.into_iter().map(|(_, c)| c).collect())
}

/// The first descendant of `window` (breadth first, limited depth) that carries `WM_STATE`.
fn find_client_below(c: &Conn, window: Window, depth: usize) -> XResult<Option<Window>> {
    let mut level = vec![window];
    for _ in 0..depth {
        let mut children = Vec::new();
        for w in level {
            if let Some(tree) = reply_opt(c.conn.query_tree(w)?)? {
                children.extend(tree.children);
            }
        }
        if children.is_empty() {
            return Ok(None);
        }
        let cookies = children
            .iter()
            .map(|&w| c.prop(w, c.atoms.WM_STATE, 2))
            .collect::<XResult<Vec<_>>>()?;
        for (&w, cookie) in children.iter().zip(cookies) {
            if reply_opt(cookie)?.is_some_and(|r| r.type_ != u32::from(AtomEnum::NONE)) {
                return Ok(Some(w));
            }
        }
        level = children;
    }
    Ok(None)
}

/// Candidates without an EWMH window manager. Returns `(candidates, bare)` where `bare`
/// means no window manager runs at all.
fn icccm_candidates(c: &Conn) -> XResult<(Vec<Candidate>, bool)> {
    let tree = c.conn.query_tree(c.root)?.reply()?;
    let cookies = tree
        .children
        .iter()
        .map(|&w| c.prop(w, c.atoms.WM_STATE, 2))
        .collect::<XResult<Vec<_>>>()?;
    let mut found = Vec::new();
    let mut to_search = Vec::new();
    for (&w, cookie) in tree.children.iter().zip(cookies) {
        match reply_opt(cookie)? {
            Some(r) if r.type_ != u32::from(AtomEnum::NONE) => found.push(Candidate {
                client: w,
                frame: None,
            }),
            Some(_) => to_search.push(w),
            None => {}
        }
    }
    // Reparenting window managers put WM_STATE on the client inside their frame.
    let mut nested = Vec::new();
    for &frame in &to_search {
        if let Some(client) = find_client_below(c, frame, 3)? {
            nested.push((frame, client));
        }
    }
    if found.is_empty() && nested.is_empty() {
        let bare = tree
            .children
            .iter()
            .map(|&client| Candidate {
                client,
                frame: None,
            })
            .collect();
        return Ok((bare, true));
    }
    // Keep stacking order: root children are bottom-to-top.
    let mut ordered: Vec<(usize, Candidate)> = Vec::new();
    for (i, &w) in tree.children.iter().enumerate() {
        if found.iter().any(|f| f.client == w) {
            ordered.push((
                i,
                Candidate {
                    client: w,
                    frame: None,
                },
            ));
        } else if let Some(&(frame, client)) = nested.iter().find(|(f, _)| *f == w) {
            ordered.push((
                i,
                Candidate {
                    client,
                    frame: Some(frame),
                },
            ));
        }
    }
    Ok((ordered.into_iter().map(|(_, c)| c).collect(), false))
}

/// Candidate client windows in bottom-to-top stacking order.
fn candidates(c: &Conn, wm: &WmInfo) -> XResult<(Vec<Candidate>, bool)> {
    if wm.is_ewmh() {
        let stacking = c.prop(c.root, c.atoms._NET_CLIENT_LIST_STACKING, LIST_LEN)?;
        let list = c.prop(c.root, c.atoms._NET_CLIENT_LIST, LIST_LEN)?;
        let none = u32::from(AtomEnum::NONE);
        if let Some(r) = reply_opt(stacking)?.filter(|r| r.type_ != none) {
            let windows = u32s(&r)
                .into_iter()
                .map(|client| Candidate {
                    client,
                    frame: None,
                })
                .collect();
            return Ok((windows, false));
        }
        if let Some(r) = reply_opt(list)?.filter(|r| r.type_ != none) {
            return Ok((order_by_stacking(c, &u32s(&r))?, false));
        }
    }
    icccm_candidates(c)
}

/// Everything read about one window.
#[derive(Debug, Clone, Default)]
pub(super) struct RawWindow {
    pub id: Window,
    pub map_state: Option<MapState>,
    pub override_redirect: bool,
    pub input_only: bool,
    pub width: u16,
    pub height: u16,
    pub border: u16,
    pub origin: (i32, i32),
    pub title: String,
    pub instance: Option<String>,
    pub class: Option<String>,
    pub gtk_app_id: Option<String>,
    pub net_pid: Option<u32>,
    pub client_machine: Option<String>,
    pub types: Vec<Option<ewmh::WindowType>>,
    pub net_state: ewmh::NetState,
    pub icccm_state: Option<IcccmState>,
    pub desktop: Option<u32>,
    pub frame_extents: Option<[u32; 4]>,
    pub gtk_frame_extents: Option<[u32; 4]>,
    pub xres_pid: Option<u32>,
}

impl RawWindow {
    pub fn client_rect(&self) -> Rect {
        geometry::client_rect(self.origin, self.width, self.height, self.border)
    }
}

/// Pipelined requests for one window.
struct Requests<'c> {
    id: Window,
    attrs: Cookie<'c, RustConnection, GetWindowAttributesReply>,
    geometry: Cookie<'c, RustConnection, GetGeometryReply>,
    origin: Cookie<'c, RustConnection, TranslateCoordinatesReply>,
    net_wm_name: PropCookie<'c>,
    wm_name: PropCookie<'c>,
    wm_class: PropCookie<'c>,
    gtk_app_id: PropCookie<'c>,
    pid: PropCookie<'c>,
    client_machine: PropCookie<'c>,
    window_type: PropCookie<'c>,
    net_state: PropCookie<'c>,
    wm_state: PropCookie<'c>,
    desktop: PropCookie<'c>,
    frame_extents: PropCookie<'c>,
    gtk_frame_extents: PropCookie<'c>,
    xres: Option<Cookie<'c, RustConnection, QueryClientIdsReply>>,
}

impl<'c> Requests<'c> {
    fn send(c: &'c Conn, id: Window) -> XResult<Self> {
        let a = &c.atoms;
        Ok(Self {
            id,
            attrs: c.conn.get_window_attributes(id)?,
            geometry: c.conn.get_geometry(id)?,
            origin: c.conn.translate_coordinates(id, c.root, 0, 0)?,
            net_wm_name: c.prop(id, a._NET_WM_NAME, TEXT_LEN)?,
            wm_name: c.prop(id, AtomEnum::WM_NAME.into(), TEXT_LEN)?,
            wm_class: c.prop(id, AtomEnum::WM_CLASS.into(), 256)?,
            gtk_app_id: c.prop(id, a._GTK_APPLICATION_ID, 256)?,
            pid: c.prop(id, a._NET_WM_PID, 1)?,
            client_machine: c.prop(id, AtomEnum::WM_CLIENT_MACHINE.into(), 64)?,
            window_type: c.prop(id, a._NET_WM_WINDOW_TYPE, 64)?,
            net_state: c.prop(id, a._NET_WM_STATE, 64)?,
            wm_state: c.prop(id, a.WM_STATE, 2)?,
            desktop: c.prop(id, a._NET_WM_DESKTOP, 1)?,
            frame_extents: c.prop(id, a._NET_FRAME_EXTENTS, 4)?,
            gtk_frame_extents: c.prop(id, a._GTK_FRAME_EXTENTS, 4)?,
            xres: if c.xres {
                Some(c.conn.res_query_client_ids(&[ClientIdSpec {
                    client: id,
                    mask: ClientIdMask::LOCAL_CLIENT_PID,
                }])?)
            } else {
                None
            },
        })
    }

    /// Collect the replies; `None` when the window vanished meanwhile.
    fn collect(self, c: &Conn) -> XResult<Option<RawWindow>> {
        let Some(attrs) = reply_opt(self.attrs)? else {
            return Ok(None);
        };
        let Some(geom) = reply_opt(self.geometry)? else {
            return Ok(None);
        };
        let Some(origin) = reply_opt(self.origin)? else {
            return Ok(None);
        };
        let net_wm_name = reply_opt(self.net_wm_name)?.and_then(|r| c.text(&r));
        let wm_name = reply_opt(self.wm_name)?.and_then(|r| c.text(&r));
        let (instance, class) = reply_opt(self.wm_class)?
            .filter(|r| r.format == 8)
            .map(|r| ewmh::parse_wm_class(&r.value))
            .unwrap_or((None, None));
        let gtk_app_id = reply_opt(self.gtk_app_id)?.and_then(|r| c.text(&r));
        let net_pid = reply_opt(self.pid)?
            .as_ref()
            .and_then(first_u32)
            .filter(|&p| p > 0);
        let client_machine = reply_opt(self.client_machine)?.and_then(|r| c.text(&r));
        let types = reply_opt(self.window_type)?
            .map(|r| {
                u32s(&r)
                    .into_iter()
                    .map(|atom| c.atoms.window_type(atom))
                    .collect()
            })
            .unwrap_or_default();
        let net_state = reply_opt(self.net_state)?
            .map(|r| c.atoms.net_state(&u32s(&r)))
            .unwrap_or_default();
        let icccm_state = reply_opt(self.wm_state)?
            .filter(|r| r.type_ == c.atoms.WM_STATE)
            .as_ref()
            .and_then(first_u32)
            .and_then(IcccmState::from_value);
        let desktop = reply_opt(self.desktop)?
            .as_ref()
            .and_then(first_u32)
            .filter(|&d| d != ALL_DESKTOPS);
        let frame_extents = reply_opt(self.frame_extents)?.as_ref().and_then(four_u32);
        let gtk_frame_extents = reply_opt(self.gtk_frame_extents)?
            .as_ref()
            .and_then(four_u32);
        let xres_pid = match self.xres {
            Some(cookie) => reply_opt(cookie)?.and_then(|r| xres_pid(&r)),
            None => None,
        };
        Ok(Some(RawWindow {
            id: self.id,
            map_state: Some(attrs.map_state),
            override_redirect: attrs.override_redirect,
            input_only: attrs.class == WindowClass::INPUT_ONLY,
            width: geom.width,
            height: geom.height,
            border: geom.border_width,
            origin: (i32::from(origin.dst_x), i32::from(origin.dst_y)),
            title: net_wm_name.or(wm_name).unwrap_or_default(),
            instance,
            class,
            gtk_app_id,
            net_pid,
            client_machine,
            types,
            net_state,
            icccm_state,
            desktop,
            frame_extents,
            gtk_frame_extents,
            xres_pid,
        }))
    }
}

fn xres_pid(reply: &QueryClientIdsReply) -> Option<u32> {
    let mask = u32::from(ClientIdMask::LOCAL_CLIENT_PID);
    reply
        .ids
        .iter()
        .find(|v| u32::from(v.spec.mask) & mask != 0)
        .and_then(|v| v.value.first().copied())
        .filter(|&pid| pid > 0)
}

/// Whether a window should be listed (see the `WindowInfo` filtering contract).
pub(super) fn is_application_window(raw: &RawWindow, bare: bool) -> bool {
    if raw.override_redirect || raw.input_only {
        return false;
    }
    // Zero-sized (or 1x1 placeholder) windows are never visible application windows.
    if raw.width <= 1 || raw.height <= 1 {
        return false;
    }
    match raw.icccm_state {
        Some(IcccmState::Withdrawn) => return false,
        // Normal and iconic (minimized) windows are listed even when unmapped (other
        // desktops, minimized).
        Some(IcccmState::Normal | IcccmState::Iconic) => {}
        None => {
            if raw.map_state == Some(MapState::UNMAPPED) && !raw.net_state.hidden {
                return false;
            }
        }
    }
    if bare && (raw.map_state != Some(MapState::VIEWABLE) || raw.class.is_none()) {
        return false;
    }
    if raw.net_state.skip_taskbar {
        return false;
    }
    ewmh::is_application_type(&raw.types)
}

/// Process id of a window's owner, 0 when unknown.
///
/// The X-Resource extension reports the pid the X server sees for the client connection,
/// which is right even for sandboxed apps whose `_NET_WM_PID` is a pid in their own pid
/// namespace (Flatpak); `_NET_WM_PID` is used when XRes is unavailable and the window comes
/// from this machine (`WM_CLIENT_MACHINE`).
pub(super) fn resolve_pid(raw: &RawWindow, hostname: Option<&str>) -> u32 {
    if let Some(pid) = raw.xres_pid {
        return pid;
    }
    let remote = match (raw.client_machine.as_deref(), hostname) {
        (Some(machine), Some(host)) => !same_host(machine, host),
        _ => false,
    };
    if remote { 0 } else { raw.net_pid.unwrap_or(0) }
}

fn same_host(a: &str, b: &str) -> bool {
    let short = |s: &str| s.split('.').next().unwrap_or(s).to_ascii_lowercase();
    a.eq_ignore_ascii_case(b) || short(a) == short(b)
}

fn frame_rect(geom: &GetGeometryReply) -> Rect {
    let b = i32::from(geom.border_width);
    Rect::new(
        i32::from(geom.x),
        i32::from(geom.y),
        i32::from(geom.width) + 2 * b,
        i32::from(geom.height) + 2 * b,
    )
}

/// Insets for a window: `_NET_FRAME_EXTENTS`/`_GTK_FRAME_EXTENTS` when published, else the
/// geometry of the window manager's frame (top-level ancestor) when the window is reparented.
fn insets_for(
    c: &Conn,
    raw: &RawWindow,
    known_frame: Option<Window>,
    frame_cache: &mut HashMap<Window, Option<Rect>>,
) -> XResult<Insets> {
    if raw.frame_extents.is_some() {
        return Ok(Insets::from_extents(
            raw.frame_extents,
            raw.gtk_frame_extents,
        ));
    }
    let frame = match known_frame {
        Some(f) => Some(f),
        None => toplevel(c, raw.id)?.filter(|&t| t != raw.id),
    };
    let gtk = Insets::from_extents(None, raw.gtk_frame_extents);
    let Some(frame) = frame else {
        return Ok(gtk);
    };
    let rect = match frame_cache.get(&frame) {
        Some(r) => *r,
        None => {
            let r = reply_opt(c.conn.get_geometry(frame)?)?.map(|g| frame_rect(&g));
            frame_cache.insert(frame, r);
            r
        }
    };
    Ok(match rect {
        Some(frame_rect) => {
            let wm = Insets::between(raw.client_rect(), frame_rect);
            Insets {
                left: wm.left + gtk.left,
                right: wm.right + gtk.right,
                top: wm.top + gtk.top,
                bottom: wm.bottom + gtk.bottom,
            }
        }
        None => gtk,
    })
}

/// The focused top-level window when no EWMH `_NET_ACTIVE_WINDOW` is available.
fn input_focus_toplevel(c: &Conn) -> XResult<Option<Window>> {
    let focus = c.conn.get_input_focus()?.reply()?.focus;
    if focus == x11rb::NONE || focus == c.root || focus == 1 {
        // 1 = PointerRoot
        return Ok(None);
    }
    toplevel(c, focus)
}

/// List application windows, front to back.
pub(crate) fn list_windows(c: &Conn, hostname: Option<&str>) -> XResult<Vec<WindowInfo>> {
    let wm = detect_wm(c)?;
    let (candidates, bare) = candidates(c, &wm)?;
    let active_cookie = if wm.is_ewmh() {
        Some(c.prop(c.root, c.atoms._NET_ACTIVE_WINDOW, 1)?)
    } else {
        None
    };
    let requests = candidates
        .iter()
        .map(|cand| Requests::send(c, cand.client))
        .collect::<XResult<Vec<_>>>()?;
    let mut raws = Vec::with_capacity(requests.len());
    for (cand, req) in candidates.iter().zip(requests) {
        if let Some(raw) = req.collect(c)? {
            raws.push((*cand, raw));
        }
    }
    let active = match active_cookie {
        Some(cookie) => reply_opt(cookie)?
            .as_ref()
            .and_then(first_u32)
            .filter(|&w| w != 0),
        None => None,
    };
    let focus_top = if active.is_none() && !wm.is_ewmh() {
        input_focus_toplevel(c)?
    } else {
        None
    };
    let monitors = monitor_rects(c, false)?;
    let monitor_bounds: Vec<Rect> = monitors.iter().map(|m| m.bounds).collect();

    let mut frame_cache = HashMap::new();
    let mut seen = std::collections::HashSet::new();
    let mut windows = Vec::new();
    // Candidates are bottom-to-top; the result is front-to-back. A window listed twice by a
    // misbehaving window manager is reported once, at its topmost position.
    for (cand, raw) in raws.iter().rev() {
        if !seen.insert(raw.id) || !is_application_window(raw, bare) {
            continue;
        }
        let insets = insets_for(c, raw, cand.frame, &mut frame_cache)?;
        let bounds = insets.outer(raw.client_rect());
        if bounds.is_empty() {
            continue;
        }
        let state = ewmh::window_state(&raw.net_state, raw.icccm_state);
        let is_focused = match active {
            Some(w) => w == raw.id,
            None => focus_top.is_some_and(|f| f == raw.id || Some(f) == cand.frame),
        };
        let monitor_id =
            geometry::best_monitor(&bounds, &monitor_bounds).map(|i| monitors[i].id.clone());
        windows.push(WindowInfo {
            id: WindowId::new(ewmh::format_window_id(raw.id)),
            pid: resolve_pid(raw, hostname),
            title: raw.title.clone(),
            class_name: raw.class.clone(),
            app_id_hint: raw.gtk_app_id.clone().or_else(|| raw.instance.clone()),
            bounds,
            normal_bounds: (state == WindowState::Normal).then_some(bounds),
            state,
            monitor_id,
            desktop: raw.desktop,
            is_focused,
            z_order: u32::try_from(windows.len()).unwrap_or(u32::MAX),
        });
    }
    Ok(windows)
}

/// Current geometry of one window: client rectangle (root coordinates, including the X
/// border), insets to its outer bounds and border width.
#[derive(Debug, Clone, Copy)]
pub(super) struct WindowGeometry {
    pub client: Rect,
    pub insets: Insets,
    pub border: u16,
}

impl WindowGeometry {
    pub fn outer(&self) -> Rect {
        self.insets.outer(self.client)
    }
}

pub(super) fn window_geometry(c: &Conn, window: Window) -> XResult<WindowGeometry> {
    let a = &c.atoms;
    let geom = c.conn.get_geometry(window)?;
    let origin = c.conn.translate_coordinates(window, c.root, 0, 0)?;
    let frame = c.prop(window, a._NET_FRAME_EXTENTS, 4)?;
    let gtk = c.prop(window, a._GTK_FRAME_EXTENTS, 4)?;
    let geom = geom.reply()?;
    let origin = origin.reply()?;
    let raw = RawWindow {
        id: window,
        width: geom.width,
        height: geom.height,
        border: geom.border_width,
        origin: (i32::from(origin.dst_x), i32::from(origin.dst_y)),
        frame_extents: reply_opt(frame)?.as_ref().and_then(four_u32),
        gtk_frame_extents: reply_opt(gtk)?.as_ref().and_then(four_u32),
        ..Default::default()
    };
    let insets = insets_for(c, &raw, None, &mut HashMap::new())?;
    Ok(WindowGeometry {
        client: raw.client_rect(),
        insets,
        border: raw.border,
    })
}

/// ICCCM `win_gravity` of a window (`WM_NORMAL_HINTS`), NorthWest when unset or unreadable.
pub(super) fn win_gravity(c: &Conn, window: Window) -> XResult<Gravity> {
    let hints = WmSizeHints::get_normal_hints(&c.conn, window)?;
    Ok(hints
        .reply()
        .ok()
        .flatten()
        .and_then(|h| h.win_gravity)
        .map_or(Gravity::NorthWest, |g| Gravity::from_protocol(u32::from(g))))
}

/// PNG of the window's `_NET_WM_ICON` image closest to `size`, if it has one.
pub(crate) fn window_icon(c: &Conn, window: Window, size: u32) -> XResult<Option<Vec<u8>>> {
    let reply = c
        .conn
        .get_property(
            false,
            window,
            c.atoms._NET_WM_ICON,
            AtomEnum::CARDINAL,
            0,
            ICON_LEN,
        )?
        .reply()?;
    let data = u32s(&reply);
    let images = icons::parse_net_wm_icon(&data);
    Ok(icons::choose_icon(&images, size)
        .and_then(|img| icons::argb_to_png(img.width, img.height, img.pixels)))
}

/// Identity hints of one window: `(WM_CLASS class, _GTK_APPLICATION_ID or instance, pid)`.
pub(crate) fn window_class(
    c: &Conn,
    window: Window,
    hostname: Option<&str>,
) -> XResult<(Option<String>, Option<String>, u32)> {
    let Some(raw) = Requests::send(c, window)?.collect(c)? else {
        return Err(super::XError::Platform(crate::PlatformError::not_found(
            format!("window {}", ewmh::format_window_id(window)),
        )));
    };
    let hint = raw.gtk_app_id.clone().or_else(|| raw.instance.clone());
    Ok((raw.class.clone(), hint, resolve_pid(&raw, hostname)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::ewmh::WindowType;

    fn normal_window() -> RawWindow {
        RawWindow {
            id: 0x40000c,
            map_state: Some(MapState::VIEWABLE),
            width: 484,
            height: 316,
            icccm_state: Some(IcccmState::Normal),
            class: Some("XTerm".into()),
            ..Default::default()
        }
    }

    #[test]
    fn application_window_filter() {
        let w = normal_window();
        assert!(is_application_window(&w, false));
        assert!(!is_application_window(
            &RawWindow {
                override_redirect: true,
                ..w.clone()
            },
            false
        ));
        assert!(!is_application_window(
            &RawWindow {
                input_only: true,
                ..w.clone()
            },
            false
        ));
        assert!(!is_application_window(
            &RawWindow {
                width: 0,
                ..w.clone()
            },
            false
        ));
        assert!(!is_application_window(
            &RawWindow {
                icccm_state: Some(IcccmState::Withdrawn),
                ..w.clone()
            },
            false
        ));
        // Minimized (iconic, unmapped) windows are listed.
        assert!(is_application_window(
            &RawWindow {
                icccm_state: Some(IcccmState::Iconic),
                map_state: Some(MapState::UNMAPPED),
                ..w.clone()
            },
            false
        ));
        // Unmapped without WM_STATE: not listed, unless _NET_WM_STATE_HIDDEN says minimized.
        let unmapped = RawWindow {
            icccm_state: None,
            map_state: Some(MapState::UNMAPPED),
            ..w.clone()
        };
        assert!(!is_application_window(&unmapped, false));
        let mut hidden = unmapped.clone();
        hidden.net_state.hidden = true;
        assert!(is_application_window(&hidden, false));
        let mut skip = w.clone();
        skip.net_state.skip_taskbar = true;
        assert!(!is_application_window(&skip, false));
        let dock = RawWindow {
            types: vec![Some(WindowType::Dock)],
            ..w.clone()
        };
        assert!(!is_application_window(&dock, false));
        // Without any window manager only mapped windows with a WM_CLASS count.
        let bare = RawWindow {
            icccm_state: None,
            ..w.clone()
        };
        assert!(is_application_window(&bare, true));
        assert!(!is_application_window(
            &RawWindow {
                class: None,
                ..bare.clone()
            },
            true
        ));
        assert!(!is_application_window(
            &RawWindow {
                map_state: Some(MapState::UNVIEWABLE),
                ..bare
            },
            true
        ));
    }

    #[test]
    fn pid_resolution() {
        let mut w = normal_window();
        assert_eq!(resolve_pid(&w, Some("host")), 0);
        w.net_pid = Some(1234);
        assert_eq!(resolve_pid(&w, Some("host")), 1234);
        w.client_machine = Some("host.example.org".into());
        assert_eq!(resolve_pid(&w, Some("HOST")), 1234);
        w.client_machine = Some("elsewhere".into());
        assert_eq!(
            resolve_pid(&w, Some("host")),
            0,
            "remote _NET_WM_PID is meaningless"
        );
        // XRes knows the real (host namespace) pid.
        w.xres_pid = Some(4321);
        assert_eq!(resolve_pid(&w, Some("host")), 4321);
    }
}
