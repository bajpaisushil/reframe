//! Window control: placement, graceful close requests and focus.
//!
//! Requests go through the window manager with EWMH client messages (source indication 2,
//! "pager", i.e. on behalf of the user) and fall back to plain ICCCM/core requests when no
//! EWMH window manager runs. The window manager processes requests asynchronously; the
//! functions wait briefly (well under a second) for the result so a following
//! `Platform::windows` call observes it, but window managers may legitimately constrain or
//! refuse a request (size increments, minimum sizes, focus-stealing prevention).

use std::time::{Duration, Instant};

use x11rb::errors::ReplyError;
use x11rb::protocol::xproto::{
    ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, EventMask, InputFocus, MapState,
    StackMode, Window,
};

use super::windows::{WindowGeometry, win_gravity, window_geometry};
use super::{Conn, WmInfo, XResult, detect_wm, first_u32, reply_opt, u32s};
use crate::linux::ewmh::{self, IcccmState, NetState};
use crate::linux::geometry;
use crate::{PlatformError, Rect, WindowPlacement, WindowState};

/// EWMH source indication for pagers and other tools acting for the user.
const SOURCE_PAGER: u32 = 2;
const NET_WM_STATE_REMOVE: u32 = 0;
const NET_WM_STATE_ADD: u32 = 1;
/// ICCCM IconicState for `WM_CHANGE_STATE`.
const ICONIC_STATE: u32 = 3;
const NORTH_WEST_GRAVITY: u32 = 1;
const POLL_INTERVAL: Duration = Duration::from_millis(15);
const STATE_TIMEOUT: Duration = Duration::from_millis(600);
const MOVE_TIMEOUT: Duration = Duration::from_millis(500);

const NO_EWMH: &str = "No EWMH-compatible window manager is running, so windows cannot be \
                       maximized or made full screen.";

#[derive(Debug, Clone, Copy)]
struct Status {
    net: NetState,
    icccm: Option<IcccmState>,
    map_state: MapState,
    /// The window manager maintains `_NET_WM_STATE_HIDDEN` (see [`ewmh::is_minimized`]).
    hidden_supported: bool,
}

impl Status {
    fn is_minimized(&self) -> bool {
        ewmh::is_minimized(&self.net, self.icccm, self.hidden_supported)
    }

    fn state(&self) -> WindowState {
        ewmh::window_state(&self.net, self.icccm, self.hidden_supported)
    }
}

/// Current state of a window; a missing window is a protocol error (mapped to NotFound).
fn status(c: &Conn, wm: &WmInfo, window: Window) -> XResult<Status> {
    let attrs = c.conn.get_window_attributes(window)?;
    let net = c.prop(window, c.atoms._NET_WM_STATE, 64)?;
    let wm_state = c.prop(window, c.atoms.WM_STATE, 2)?;
    let attrs = attrs.reply()?;
    let net = reply_opt(net)?
        .map(|r| c.atoms.net_state(&u32s(&r)))
        .unwrap_or_default();
    let icccm = reply_opt(wm_state)?
        .filter(|r| r.type_ == c.atoms.WM_STATE)
        .as_ref()
        .and_then(first_u32)
        .and_then(IcccmState::from_value);
    Ok(Status {
        net,
        icccm,
        map_state: attrs.map_state,
        hidden_supported: wm.supports(c.atoms._NET_WM_STATE_HIDDEN),
    })
}

/// Poll `check` until it returns true or `timeout` passes. Returns whether it succeeded.
fn wait_for(timeout: Duration, mut check: impl FnMut() -> XResult<bool>) -> XResult<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        if check()? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Send an EWMH/ICCCM client message about `window` to the root window (where the window
/// manager listens).
fn send_root_message(c: &Conn, window: Window, message: u32, data: [u32; 5]) -> XResult<()> {
    let event = ClientMessageEvent::new(32, window, message, data);
    c.conn.send_event(
        false,
        c.root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        event,
    )?;
    Ok(())
}

fn change_net_state(c: &Conn, window: Window, action: u32, first: u32, second: u32) -> XResult<()> {
    send_root_message(
        c,
        window,
        c.atoms._NET_WM_STATE,
        [action, first, second, SOURCE_PAGER, 0],
    )
}

fn active_window(c: &Conn) -> XResult<Option<Window>> {
    Ok(reply_opt(c.prop(c.root, c.atoms._NET_ACTIVE_WINDOW, 1)?)?
        .as_ref()
        .and_then(first_u32)
        .filter(|&w| w != 0))
}

fn activate(c: &Conn, window: Window) -> XResult<()> {
    let current = active_window(c)?.unwrap_or(0);
    send_root_message(
        c,
        window,
        c.atoms._NET_ACTIVE_WINDOW,
        [SOURCE_PAGER, x11rb::CURRENT_TIME, current, 0, 0],
    )
}

/// Ask the window manager to un-minimize a window: map it (ICCCM), falling back to
/// activating it (EWMH) when the window manager ignores the map request.
fn unminimize(c: &Conn, wm: &WmInfo, window: Window) -> XResult<()> {
    c.conn.map_window(window)?;
    c.flush()?;
    let restored = wait_for(STATE_TIMEOUT, || Ok(!status(c, wm, window)?.is_minimized()))?;
    if !restored && wm.supports(c.atoms._NET_ACTIVE_WINDOW) {
        activate(c, window)?;
        c.flush()?;
        wait_for(STATE_TIMEOUT, || Ok(!status(c, wm, window)?.is_minimized()))?;
    }
    Ok(())
}

/// Send one move/resize request so that the outer frame lands on `outer`.
fn request_move(
    c: &Conn,
    wm: &WmInfo,
    window: Window,
    geom: &WindowGeometry,
    managed: bool,
    outer: Rect,
) -> XResult<()> {
    let (x, y, width, height) = geometry::moveresize_request(outer, geom.insets, geom.border);
    if wm.supports(c.atoms._NET_MOVERESIZE_WINDOW) {
        // Gravity NorthWest: x/y is the frame's top-left corner; width/height the client's.
        let flags = NORTH_WEST_GRAVITY | (0b1111 << 8) | (SOURCE_PAGER << 12);
        return send_root_message(
            c,
            window,
            c.atoms._NET_MOVERESIZE_WINDOW,
            [flags, x as u32, y as u32, width, height],
        );
    }
    // ConfigureWindow on the client. A window manager interprets the position according to
    // the window's win_gravity; without a window manager it is the client's own position.
    let (cx, cy) = if managed {
        geometry::configure_position((outer.x, outer.y), geom.insets, win_gravity(c, window)?)
    } else {
        let client = geom.insets.client(outer);
        (client.x, client.y)
    };
    c.conn.configure_window(
        window,
        &ConfigureWindowAux::new()
            .x(cx)
            .y(cy)
            .width(width)
            .height(height),
    )?;
    Ok(())
}

/// Move/resize and, when `verify` is set, wait until the frame is in place. If the window
/// manager placed the *client* (not the frame) at the requested position, which some
/// window managers do, one corrected request is sent. Other deviations (the window manager
/// keeping the window on screen, size increments) are accepted.
fn move_resize(
    c: &Conn,
    wm: &WmInfo,
    window: Window,
    managed: bool,
    outer: Rect,
    verify: bool,
) -> XResult<()> {
    let geom = window_geometry(c, window)?;
    request_move(c, wm, window, &geom, managed, outer)?;
    c.flush()?;
    if !verify {
        return Ok(());
    }
    let at_target = |r: Rect| r.x == outer.x && r.y == outer.y;
    let mut current = geom.outer();
    if wait_for(MOVE_TIMEOUT, || {
        current = window_geometry(c, window)?.outer();
        Ok(at_target(current))
    })? {
        return Ok(());
    }
    let (dx, dy) = (current.x - outer.x, current.y - outer.y);
    let insets = geom.insets;
    let misread =
        (dx == -insets.left && dy == -insets.top) || (dx == insets.left && dy == insets.top);
    if misread && (dx, dy) != (0, 0) {
        log::debug!(
            "window manager offset the frame by ({dx}, {dy}); correcting placement of {}",
            ewmh::format_window_id(window)
        );
        let corrected = Rect::new(outer.x - dx, outer.y - dy, outer.width, outer.height);
        let geom = window_geometry(c, window)?;
        request_move(c, wm, window, &geom, managed, corrected)?;
        c.flush()?;
        wait_for(MOVE_TIMEOUT, || {
            Ok(at_target(window_geometry(c, window)?.outer()))
        })?;
    } else {
        log::debug!(
            "window {} placed at ({}, {}) instead of ({}, {})",
            ewmh::format_window_id(window),
            current.x,
            current.y,
            outer.x,
            outer.y
        );
    }
    Ok(())
}

/// Move/resize a window so its outer frame covers `placement.bounds`, then apply
/// `placement.state`.
///
/// Order (each step waits briefly for the window manager): leave maximized/full-screen
/// state, un-minimize (unless the window should stay minimized), move/resize, then
/// maximize, make full screen or minimize. A window already minimized that should stay
/// minimized is moved without being shown; whether the window manager applies that
/// geometry before the window is restored depends on the window manager.
pub(crate) fn set_placement(c: &Conn, window: Window, placement: &WindowPlacement) -> XResult<()> {
    let a = &c.atoms;
    let wm = detect_wm(c)?;
    let before = status(c, &wm, window)?;
    let target = placement.state;
    let managed = wm.is_ewmh() || before.icccm.is_some();
    let can_change_state = wm.is_ewmh();

    // 1. Leave maximized / full-screen state so the window takes the requested size.
    let mut left_state = false;
    if can_change_state {
        if before.net.maximized_vert || before.net.maximized_horz {
            change_net_state(
                c,
                window,
                NET_WM_STATE_REMOVE,
                a._NET_WM_STATE_MAXIMIZED_VERT,
                a._NET_WM_STATE_MAXIMIZED_HORZ,
            )?;
            left_state = true;
        }
        if before.net.fullscreen {
            change_net_state(
                c,
                window,
                NET_WM_STATE_REMOVE,
                a._NET_WM_STATE_FULLSCREEN,
                0,
            )?;
            left_state = true;
        }
    }
    if left_state {
        c.flush()?;
        wait_for(STATE_TIMEOUT, || {
            let s = status(c, &wm, window)?;
            Ok(!s.net.maximized_vert && !s.net.maximized_horz && !s.net.fullscreen)
        })?;
    }

    // 2. Un-minimize unless the window should end up minimized anyway.
    let stay_minimized = before.is_minimized() && target == WindowState::Minimized;
    if before.is_minimized() && !stay_minimized {
        unminimize(c, &wm, window)?;
    }

    // 3. Move/resize the outer frame.
    move_resize(c, &wm, window, managed, placement.bounds, !stay_minimized)?;

    // 4. Apply the requested state.
    match target {
        WindowState::Normal => {}
        WindowState::Maximized | WindowState::Fullscreen if !can_change_state => {
            c.flush()?;
            return Err(PlatformError::unsupported(
                if target == WindowState::Maximized {
                    "Maximizing windows"
                } else {
                    "Making windows full screen"
                },
                format!("{NO_EWMH} The window was moved but keeps its normal state."),
            )
            .into());
        }
        WindowState::Maximized => change_net_state(
            c,
            window,
            NET_WM_STATE_ADD,
            a._NET_WM_STATE_MAXIMIZED_VERT,
            a._NET_WM_STATE_MAXIMIZED_HORZ,
        )?,
        WindowState::Fullscreen => {
            change_net_state(c, window, NET_WM_STATE_ADD, a._NET_WM_STATE_FULLSCREEN, 0)?
        }
        WindowState::Minimized => {
            if !stay_minimized {
                send_root_message(c, window, a.WM_CHANGE_STATE, [ICONIC_STATE, 0, 0, 0, 0])?;
            }
        }
    }
    c.flush()?;
    if target != WindowState::Normal && !stay_minimized {
        let reached = wait_for(STATE_TIMEOUT, || {
            Ok(status(c, &wm, window)?.state() == target)
        })?;
        if !reached {
            log::debug!(
                "window manager did not apply {target:?} to {}",
                ewmh::format_window_id(window)
            );
        }
    }
    Ok(())
}

/// Politely ask a window to close: `_NET_CLOSE_WINDOW` through the window manager, or the
/// ICCCM `WM_DELETE_WINDOW` message directly when no EWMH window manager handles it.
///
/// Windows that do not take part in the `WM_DELETE_WINDOW` protocol are refused with
/// `Unsupported`: for them window managers fall back to `XKillClient`, which Reframe never
/// triggers. The application may still prompt about unsaved work or ignore the request.
pub(crate) fn close_window(c: &Conn, window: Window) -> XResult<()> {
    let a = &c.atoms;
    let wm = detect_wm(c)?;
    let protocols = c.prop(window, a.WM_PROTOCOLS, 64)?.reply()?;
    if !u32s(&protocols).contains(&a.WM_DELETE_WINDOW) {
        return Err(PlatformError::unsupported(
            "Closing this window",
            "the application does not accept polite close requests (WM_DELETE_WINDOW); \
             Reframe never force-closes windows, so close it from the application itself",
        )
        .into());
    }
    if wm.supports(a._NET_CLOSE_WINDOW) {
        send_root_message(
            c,
            window,
            a._NET_CLOSE_WINDOW,
            [x11rb::CURRENT_TIME, SOURCE_PAGER, 0, 0, 0],
        )?;
    } else {
        let event = ClientMessageEvent::new(
            32,
            window,
            a.WM_PROTOCOLS,
            [a.WM_DELETE_WINDOW, x11rb::CURRENT_TIME, 0, 0, 0],
        );
        c.conn
            .send_event(false, window, EventMask::NO_EVENT, event)?;
    }
    c.flush()
}

/// Bring a window to the front and give it the input focus, un-minimizing it first.
pub(crate) fn focus_window(c: &Conn, window: Window) -> XResult<()> {
    let wm = detect_wm(c)?;
    let before = status(c, &wm, window)?;
    if before.is_minimized() {
        c.conn.map_window(window)?;
    }
    if wm.supports(c.atoms._NET_ACTIVE_WINDOW) {
        activate(c, window)?;
        c.flush()?;
        let focused = wait_for(STATE_TIMEOUT, || Ok(active_window(c)? == Some(window)))?;
        if !focused {
            log::debug!(
                "window manager did not activate {}",
                ewmh::format_window_id(window)
            );
        }
        return Ok(());
    }
    // No EWMH window manager: raise and focus directly.
    if before.map_state != MapState::VIEWABLE {
        c.conn.map_window(window)?;
        c.flush()?;
        wait_for(STATE_TIMEOUT, || {
            Ok(status(c, &wm, window)?.map_state == MapState::VIEWABLE)
        })?;
    }
    c.conn.configure_window(
        window,
        &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
    )?;
    match c
        .conn
        .set_input_focus(InputFocus::PARENT, window, x11rb::CURRENT_TIME)?
        .check()
    {
        // BadMatch: the window is still not viewable; focus is best effort.
        Ok(()) | Err(ReplyError::X11Error(_)) => {}
        Err(ReplyError::ConnectionError(e)) => return Err(e.into()),
    }
    c.flush()
}
