//! X11 access through `x11rb`'s pure-Rust connection.
//!
//! The connection is opened lazily on first use and dropped after a connection-level failure
//! (X server restarted or gone); the failing operation is retried once on a fresh connection
//! and otherwise reported as a [`PlatformError`]. Nothing here panics when the X server is
//! missing, misbehaves or dies.
//!
//! Limitation: requests block; an X server that accepts connections but never answers (frozen)
//! blocks the calling thread.

mod control;
mod monitors;
mod windows;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use x11rb::connection::{Connection, RequestConnection};
use x11rb::cookie::Cookie;
use x11rb::errors::{ConnectError, ConnectionError, ReplyError, ReplyOrIdError};
use x11rb::protocol::ErrorKind;
use x11rb::protocol::randr::{self, ConnectionExt as _};
use x11rb::protocol::res::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt as _, GetPropertyReply, Window};
use x11rb::rust_connection::RustConnection;
use x11rb::x11_utils::X11Error;

pub(crate) use control::{close_window, focus_window, set_placement};
pub(crate) use monitors::list_monitors;
pub(crate) use windows::{list_windows, window_class, window_icon};

use super::ewmh;
use crate::PlatformError;

x11rb::atom_manager! {
    /// Atoms used by the Linux platform, interned once per connection.
    pub(crate) Atoms: AtomsCookie {
        UTF8_STRING,
        WM_STATE,
        WM_CHANGE_STATE,
        WM_PROTOCOLS,
        WM_DELETE_WINDOW,
        EDID,
        _NET_SUPPORTED,
        _NET_SUPPORTING_WM_CHECK,
        _NET_CLIENT_LIST,
        _NET_CLIENT_LIST_STACKING,
        _NET_ACTIVE_WINDOW,
        _NET_CURRENT_DESKTOP,
        _NET_WORKAREA,
        _NET_CLOSE_WINDOW,
        _NET_MOVERESIZE_WINDOW,
        _NET_WM_NAME,
        _NET_WM_PID,
        _NET_WM_ICON,
        _NET_WM_DESKTOP,
        _NET_WM_STRUT,
        _NET_WM_STRUT_PARTIAL,
        _NET_FRAME_EXTENTS,
        _GTK_FRAME_EXTENTS,
        _GTK_APPLICATION_ID,
        _NET_WM_STATE,
        _NET_WM_STATE_MAXIMIZED_VERT,
        _NET_WM_STATE_MAXIMIZED_HORZ,
        _NET_WM_STATE_HIDDEN,
        _NET_WM_STATE_FULLSCREEN,
        _NET_WM_STATE_SKIP_TASKBAR,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_DESKTOP,
        _NET_WM_WINDOW_TYPE_DOCK,
        _NET_WM_WINDOW_TYPE_TOOLBAR,
        _NET_WM_WINDOW_TYPE_MENU,
        _NET_WM_WINDOW_TYPE_UTILITY,
        _NET_WM_WINDOW_TYPE_SPLASH,
        _NET_WM_WINDOW_TYPE_DIALOG,
        _NET_WM_WINDOW_TYPE_DROPDOWN_MENU,
        _NET_WM_WINDOW_TYPE_POPUP_MENU,
        _NET_WM_WINDOW_TYPE_TOOLTIP,
        _NET_WM_WINDOW_TYPE_NOTIFICATION,
        _NET_WM_WINDOW_TYPE_COMBO,
        _NET_WM_WINDOW_TYPE_DND,
        _NET_WM_WINDOW_TYPE_NORMAL,
    }
}

impl Atoms {
    pub fn window_type(&self, atom: Atom) -> Option<ewmh::WindowType> {
        use ewmh::WindowType::*;
        let table = [
            (self._NET_WM_WINDOW_TYPE_DESKTOP, Desktop),
            (self._NET_WM_WINDOW_TYPE_DOCK, Dock),
            (self._NET_WM_WINDOW_TYPE_TOOLBAR, Toolbar),
            (self._NET_WM_WINDOW_TYPE_MENU, Menu),
            (self._NET_WM_WINDOW_TYPE_UTILITY, Utility),
            (self._NET_WM_WINDOW_TYPE_SPLASH, Splash),
            (self._NET_WM_WINDOW_TYPE_DIALOG, Dialog),
            (self._NET_WM_WINDOW_TYPE_DROPDOWN_MENU, DropdownMenu),
            (self._NET_WM_WINDOW_TYPE_POPUP_MENU, PopupMenu),
            (self._NET_WM_WINDOW_TYPE_TOOLTIP, Tooltip),
            (self._NET_WM_WINDOW_TYPE_NOTIFICATION, Notification),
            (self._NET_WM_WINDOW_TYPE_COMBO, Combo),
            (self._NET_WM_WINDOW_TYPE_DND, Dnd),
            (self._NET_WM_WINDOW_TYPE_NORMAL, Normal),
        ];
        table.iter().find(|(a, _)| *a == atom).map(|(_, t)| *t)
    }

    pub fn net_state(&self, atoms: &[Atom]) -> ewmh::NetState {
        ewmh::NetState {
            maximized_vert: atoms.contains(&self._NET_WM_STATE_MAXIMIZED_VERT),
            maximized_horz: atoms.contains(&self._NET_WM_STATE_MAXIMIZED_HORZ),
            hidden: atoms.contains(&self._NET_WM_STATE_HIDDEN),
            fullscreen: atoms.contains(&self._NET_WM_STATE_FULLSCREEN),
            skip_taskbar: atoms.contains(&self._NET_WM_STATE_SKIP_TASKBAR),
        }
    }
}

/// Internal error type of the X11 layer.
#[derive(Debug)]
pub(crate) enum XError {
    /// The display could not be opened.
    Connect(String),
    /// The connection broke (I/O error, server gone). The connection is discarded.
    Connection(String),
    /// The server answered a request with a protocol error.
    Protocol(X11Error),
    /// Anything else (malformed data, unsupported extension...).
    Other(String),
    /// A fully formed platform error (not found, unsupported...).
    Platform(PlatformError),
}

impl XError {
    pub fn is_connection(&self) -> bool {
        matches!(self, XError::Connection(_))
    }

    /// Whether this is a "no such window" protocol error.
    pub fn is_bad_window(&self) -> bool {
        matches!(self, XError::Protocol(e) if matches!(e.error_kind, ErrorKind::Window | ErrorKind::Drawable))
    }

    /// Convert to a [`PlatformError`]; `window` names the window for "not found" errors.
    pub fn into_platform(self, operation: &str, window: Option<u32>) -> PlatformError {
        match self {
            XError::Connect(msg) => {
                PlatformError::os(operation, format!("cannot connect to the X server: {msg}"))
            }
            XError::Connection(msg) => PlatformError::os(
                operation,
                format!("the connection to the X server was lost: {msg}"),
            ),
            e if e.is_bad_window() => PlatformError::not_found(match window {
                Some(w) => format!("window {}", ewmh::format_window_id(w)),
                None => "window".to_string(),
            }),
            XError::Protocol(e) => PlatformError::os(
                operation,
                format!(
                    "X11 error {:?} in {}",
                    e.error_kind,
                    e.request_name.unwrap_or("a request")
                ),
            ),
            XError::Other(msg) => PlatformError::os(operation, msg),
            XError::Platform(e) => e,
        }
    }
}

impl From<ConnectError> for XError {
    fn from(e: ConnectError) -> Self {
        XError::Connect(e.to_string())
    }
}

impl From<ConnectionError> for XError {
    fn from(e: ConnectionError) -> Self {
        match e {
            ConnectionError::ParseError(p) => XError::Other(format!("malformed X11 reply: {p}")),
            ConnectionError::UnsupportedExtension => {
                XError::Other("the X server does not support a required extension".into())
            }
            ConnectionError::MaximumRequestLengthExceeded => {
                XError::Other("X11 request too large".into())
            }
            other => XError::Connection(other.to_string()),
        }
    }
}

impl From<ReplyError> for XError {
    fn from(e: ReplyError) -> Self {
        match e {
            ReplyError::ConnectionError(e) => e.into(),
            ReplyError::X11Error(e) => XError::Protocol(e),
        }
    }
}

impl From<ReplyOrIdError> for XError {
    fn from(e: ReplyOrIdError) -> Self {
        match e {
            ReplyOrIdError::IdsExhausted => XError::Other("X11 resource ids exhausted".into()),
            ReplyOrIdError::ConnectionError(e) => e.into(),
            ReplyOrIdError::X11Error(e) => XError::Protocol(e),
        }
    }
}

impl From<PlatformError> for XError {
    fn from(e: PlatformError) -> Self {
        XError::Platform(e)
    }
}

pub(crate) type XResult<T> = Result<T, XError>;
pub(crate) type PropCookie<'c> = Cookie<'c, RustConnection, GetPropertyReply>;

/// An open X connection with the per-connection data Reframe needs.
pub(crate) struct Conn {
    pub conn: RustConnection,
    pub root: Window,
    pub screen_width: u16,
    pub screen_height: u16,
    pub atoms: Atoms,
    /// RandR version, when the extension is present.
    pub randr: Option<(u32, u32)>,
    /// X-Resource 1.2+ (`XResQueryClientIds`) is available.
    pub xres: bool,
    atom_names: Mutex<HashMap<Atom, String>>,
}

impl Conn {
    fn connect(display: Option<&str>) -> XResult<Self> {
        let Some(display) = display else {
            return Err(XError::Connect("DISPLAY is not set".into()));
        };
        let (conn, screen_num) = RustConnection::connect(Some(display))?;
        let screen = conn
            .setup()
            .roots
            .get(screen_num)
            .ok_or_else(|| XError::Other(format!("the X server has no screen {screen_num}")))?;
        let (root, screen_width, screen_height) =
            (screen.root, screen.width_in_pixels, screen.height_in_pixels);
        let atoms = Atoms::new(&conn)?.reply()?;
        let randr = if conn
            .extension_information(randr::X11_EXTENSION_NAME)?
            .is_some()
        {
            conn.randr_query_version(1, 6)?
                .reply()
                .ok()
                .map(|r| (r.major_version, r.minor_version))
        } else {
            None
        };
        let xres = conn
            .extension_information(res::X11_EXTENSION_NAME)?
            .is_some()
            && conn
                .res_query_version(1, 2)?
                .reply()
                .is_ok_and(|r| (r.server_major, r.server_minor) >= (1, 2));
        Ok(Self {
            conn,
            root,
            screen_width,
            screen_height,
            atoms,
            randr,
            xres,
            atom_names: Mutex::new(HashMap::new()),
        })
    }

    /// Send a `GetProperty` request (any type) for up to `long_length` 32-bit units.
    pub fn prop(
        &self,
        window: Window,
        property: Atom,
        long_length: u32,
    ) -> XResult<PropCookie<'_>> {
        Ok(self
            .conn
            .get_property(false, window, property, AtomEnum::ANY, 0, long_length)?)
    }

    /// Text of a property reply (`UTF8_STRING` as UTF-8, others as Latin-1).
    pub fn text(&self, reply: &GetPropertyReply) -> Option<String> {
        if reply.type_ == u32::from(AtomEnum::NONE) || reply.format != 8 {
            return None;
        }
        let text = ewmh::decode_text(&reply.value, reply.type_ == self.atoms.UTF8_STRING);
        let text = text.trim().to_string();
        (!text.is_empty()).then_some(text)
    }

    /// Names of atoms (cached; atom names never change while the server runs).
    pub fn atom_names(&self, atoms: &[Atom]) -> XResult<Vec<Option<String>>> {
        let mut cache = self.atom_names.lock().unwrap_or_else(|e| e.into_inner());
        let mut cookies = Vec::new();
        for &atom in atoms {
            if atom != 0 && !cache.contains_key(&atom) {
                cookies.push((atom, self.conn.get_atom_name(atom)?));
            }
        }
        for (atom, cookie) in cookies {
            match cookie.reply() {
                Ok(reply) => {
                    cache.insert(atom, String::from_utf8_lossy(&reply.name).into_owned());
                }
                Err(ReplyError::X11Error(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(atoms.iter().map(|a| cache.get(a).cloned()).collect())
    }

    pub fn flush(&self) -> XResult<()> {
        Ok(self.conn.flush()?)
    }
}

/// All 32-bit values of a property reply (empty unless the format is 32).
pub(crate) fn u32s(reply: &GetPropertyReply) -> Vec<u32> {
    reply
        .value32()
        .map(|values| values.collect())
        .unwrap_or_default()
}

pub(crate) fn first_u32(reply: &GetPropertyReply) -> Option<u32> {
    reply.value32().and_then(|mut values| values.next())
}

pub(crate) fn four_u32(reply: &GetPropertyReply) -> Option<[u32; 4]> {
    u32s(reply).get(..4).and_then(|s| s.try_into().ok())
}

/// Await a reply; a protocol error (e.g. the window vanished) becomes `None`, connection
/// failures are errors.
pub(crate) fn reply_opt<R>(cookie: Cookie<'_, RustConnection, R>) -> XResult<Option<R>>
where
    R: x11rb::x11_utils::TryParse,
{
    match cookie.reply() {
        Ok(reply) => Ok(Some(reply)),
        Err(ReplyError::X11Error(_)) => Ok(None),
        Err(ReplyError::ConnectionError(e)) => Err(e.into()),
    }
}

/// The running window manager, as far as EWMH tells.
#[derive(Debug, Clone, Default)]
pub(crate) struct WmInfo {
    /// The `_NET_SUPPORTING_WM_CHECK` window of an EWMH compliant window manager.
    pub check_window: Option<Window>,
    pub name: Option<String>,
    pub supported: Vec<Atom>,
}

impl WmInfo {
    pub fn is_ewmh(&self) -> bool {
        self.check_window.is_some()
    }

    pub fn supports(&self, atom: Atom) -> bool {
        self.supported.contains(&atom)
    }
}

/// Detect an EWMH window manager (`_NET_SUPPORTING_WM_CHECK` on the root window pointing to a
/// child window that points to itself; a stale property left by a dead WM is ignored).
pub(crate) fn detect_wm(c: &Conn) -> XResult<WmInfo> {
    let a = &c.atoms;
    let Some(root_check) = reply_opt(c.prop(c.root, a._NET_SUPPORTING_WM_CHECK, 1)?)? else {
        return Ok(WmInfo::default());
    };
    let Some(check) = first_u32(&root_check).filter(|&w| w != 0) else {
        return Ok(WmInfo::default());
    };
    let self_check = c.prop(check, a._NET_SUPPORTING_WM_CHECK, 1)?;
    let name = c.prop(check, a._NET_WM_NAME, 256)?;
    let supported = c.prop(c.root, a._NET_SUPPORTED, 4096)?;
    let valid = reply_opt(self_check)?
        .as_ref()
        .and_then(first_u32)
        .is_some_and(|w| w == check);
    if !valid {
        return Ok(WmInfo::default());
    }
    let name = reply_opt(name)?.and_then(|r| c.text(&r));
    let supported = reply_opt(supported)?.map(|r| u32s(&r)).unwrap_or_default();
    Ok(WmInfo {
        check_window: Some(check),
        name,
        supported,
    })
}

/// Lazily opened, automatically re-established X connection.
pub(crate) struct X11 {
    display: Option<String>,
    state: Mutex<Option<Arc<Conn>>>,
}

impl X11 {
    /// Connection manager for `display` (the `DISPLAY` value captured when the platform was
    /// created; `None` means there is no X display and every operation fails to connect).
    pub fn new(display: Option<String>) -> Self {
        Self {
            display,
            state: Mutex::new(None),
        }
    }

    fn connection(&self) -> XResult<Arc<Conn>> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(conn) = state.as_ref() {
            return Ok(Arc::clone(conn));
        }
        let conn = Arc::new(Conn::connect(self.display.as_deref())?);
        *state = Some(Arc::clone(&conn));
        Ok(conn)
    }

    fn invalidate(&self, conn: &Arc<Conn>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.as_ref().is_some_and(|c| Arc::ptr_eq(c, conn)) {
            *state = None;
        }
    }

    /// Run `op` on a live connection. After a connection-level failure the connection is
    /// discarded and `op` is retried once on a new connection.
    pub fn run<T>(&self, op: impl Fn(&Conn) -> XResult<T>) -> XResult<T> {
        let conn = self.connection()?;
        match op(&conn) {
            Err(e) if e.is_connection() => {
                self.invalidate(&conn);
                log::info!("X11 connection lost ({e:?}); reconnecting");
                let conn = self.connection()?;
                let result = op(&conn);
                if result.as_ref().is_err_and(XError::is_connection) {
                    self.invalidate(&conn);
                }
                result
            }
            other => other,
        }
    }
}
