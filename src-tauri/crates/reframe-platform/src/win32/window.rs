//! Window enumeration (`EnumWindows`) and per-window queries.
//!
//! Every query tolerates windows that disappear mid-enumeration: a failed call skips the window
//! (or the optional detail) instead of failing the whole listing.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::mem::size_of;

use windows::Win32::Foundation::{HWND, LPARAM, PROPERTYKEY, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromWindow};
use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, GW_OWNER, GWL_EXSTYLE, GWL_STYLE, GetClassNameW,
    GetForegroundWindow, GetSystemMetrics, GetWindow, GetWindowLongW, GetWindowPlacement,
    GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    InternalGetWindowText, IsIconic, IsWindowVisible, IsZoomed, SM_CXPADDEDBORDER, SM_CXSIZEFRAME,
    WINDOWPLACEMENT,
};
use windows::core::{BOOL, GUID, PCWSTR};

use super::ffi::{
    from_wide, get_dpi_for_window, get_system_metrics_for_dpi, hwnd, pcwstr, raw_hwnd, wide,
};
use super::logic::filter::{
    UWP_CORE_CLASS, UWP_FRAME_CLASS, WS_EX_TOOLWINDOW, WindowFacts, format_window_id,
    is_application_window, resolve_uwp_pid,
};
use super::logic::geometry::{
    Insets, WS_THICKFRAME, estimated_insets, normal_bounds_from_placement, normal_state_insets,
    rect_from_edges, window_state,
};
use super::monitor::{MonitorRecord, MonitorSnapshot};
use crate::{PlatformError, PlatformResult, Rect, WindowId, WindowInfo, WindowState};

/// Normal-state invisible-border insets measured per window handle (see
/// [`super::logic::geometry::normal_state_insets`]).
pub type InsetsCache = HashMap<isize, Insets>;

/// `PKEY_AppUserModel_ID` (`{9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3}`, 5).
const PKEY_APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
    pid: 5,
};

/// Longest window title read (in UTF-16 units); longer titles are truncated.
const MAX_TITLE_UNITS: usize = 32 * 1024;

unsafe extern "system" fn collect_window(window: HWND, data: LPARAM) -> BOOL {
    // SAFETY: `data` is the `&mut Vec<isize>` passed by `top_level_windows`, alive during the call.
    let handles = unsafe { &mut *(data.0 as *mut Vec<isize>) };
    handles.push(raw_hwnd(window));
    BOOL::from(true)
}

/// All top-level windows, front to back (`EnumWindows` order).
pub fn top_level_windows() -> PlatformResult<Vec<isize>> {
    let mut handles: Vec<isize> = Vec::new();
    // SAFETY: the callback only pushes into `handles`, which outlives the call.
    unsafe {
        EnumWindows(
            Some(collect_window),
            LPARAM(&mut handles as *mut Vec<isize> as isize),
        )
    }
    .map_err(|e| PlatformError::os("Listing windows", e.message()))?;
    Ok(handles)
}

/// `(GWL_STYLE, GWL_EXSTYLE)`.
pub fn styles(window: HWND) -> (u32, u32) {
    // SAFETY: plain queries; an invalid handle yields 0.
    unsafe {
        (
            GetWindowLongW(window, GWL_STYLE) as u32,
            GetWindowLongW(window, GWL_EXSTYLE) as u32,
        )
    }
}

/// Outer window rectangle (`GetWindowRect`).
pub fn window_rect(window: HWND) -> Option<Rect> {
    let mut rect = RECT::default();
    // SAFETY: valid out-pointer.
    unsafe { GetWindowRect(window, &mut rect) }.ok()?;
    Some(rect_from_edges(
        rect.left,
        rect.top,
        rect.right,
        rect.bottom,
    ))
}

/// Visible frame (`DWMWA_EXTENDED_FRAME_BOUNDS`); `None` when DWM cannot report it.
pub fn frame_bounds(window: HWND) -> Option<Rect> {
    let mut rect = RECT::default();
    // SAFETY: the buffer is a RECT and its size is passed.
    unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut RECT as *mut c_void,
            size_of::<RECT>() as u32,
        )
    }
    .ok()?;
    let frame = rect_from_edges(rect.left, rect.top, rect.right, rect.bottom);
    (!frame.is_empty()).then_some(frame)
}

/// The bounds Reframe reports: the DWM visible frame when it is consistent with the window
/// rectangle, else the window rectangle itself.
pub fn visible_bounds(window_rect: Rect, frame: Option<Rect>) -> Rect {
    match frame {
        Some(frame) if Insets::between(window_rect, frame).is_some() => frame,
        _ => window_rect,
    }
}

/// `GetWindowPlacement`.
pub fn window_placement(window: HWND) -> windows::core::Result<WINDOWPLACEMENT> {
    let mut placement = WINDOWPLACEMENT {
        length: size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    // SAFETY: `length` is initialized as the API requires.
    unsafe { GetWindowPlacement(window, &mut placement) }?;
    Ok(placement)
}

/// Owning process id, or `None` if the window no longer exists.
pub fn window_pid(window: HWND) -> Option<u32> {
    let mut pid = 0u32;
    // SAFETY: valid out-pointer; returns 0 for invalid windows.
    let thread = unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
    (thread != 0).then_some(pid)
}

fn class_name(window: HWND) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: the slice length bounds the copy.
    let len = unsafe { GetClassNameW(window, &mut buf) };
    from_wide(&buf[..usize::try_from(len).unwrap_or(0).min(buf.len())])
}

fn window_title(window: HWND, own_process: bool) -> String {
    if own_process {
        // GetWindowTextW *sends* WM_GETTEXT to windows of the calling process, which would block
        // on one of Reframe's own busy UI threads; InternalGetWindowText reads the stored caption.
        let mut buf = vec![0u16; 512];
        loop {
            // SAFETY: the slice length bounds the copy.
            let copied = unsafe { InternalGetWindowText(window, &mut buf) };
            let copied = usize::try_from(copied).unwrap_or(0).min(buf.len());
            if copied + 1 < buf.len() || buf.len() >= MAX_TITLE_UNITS {
                return from_wide(&buf[..copied]);
            }
            let grown = buf.len() * 2;
            buf.resize(grown, 0);
        }
    }
    // For windows of other processes GetWindowTextW reads the stored caption without sending a
    // message, so a hung application cannot block Reframe.
    // SAFETY: plain query.
    let len = unsafe { GetWindowTextLengthW(window) };
    let Ok(len) = usize::try_from(len) else {
        return String::new();
    };
    if len == 0 {
        return String::new();
    }
    let mut buf = vec![0u16; len.min(MAX_TITLE_UNITS) + 1];
    // SAFETY: the slice length bounds the copy.
    let copied = unsafe { GetWindowTextW(window, &mut buf) };
    from_wide(&buf[..usize::try_from(copied).unwrap_or(0).min(buf.len())])
}

fn is_cloaked(window: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: the buffer is a DWORD and its size is passed.
    let result = unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut c_void,
            size_of::<u32>() as u32,
        )
    };
    result.is_ok() && cloaked != 0
}

/// The window's owner (`GW_OWNER`), if any.
fn owner(window: HWND) -> Option<HWND> {
    // SAFETY: plain query; a window without owner yields an error.
    unsafe { GetWindow(window, GW_OWNER) }
        .ok()
        .filter(|owner| !owner.is_invalid())
}

/// The window's AppUserModelID (`PKEY_AppUserModel_ID`), when the application set one or the
/// window belongs to a packaged app. Best effort: requires COM on the calling thread.
fn app_user_model_id(window: HWND) -> Option<String> {
    // SAFETY: COM calls on a valid interface; the PROPVARIANT is cleared exactly once.
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(window).ok()?;
        let mut value = store.GetValue(&PKEY_APP_USER_MODEL_ID).ok()?;
        let inner = &value.Anonymous.Anonymous;
        let text = if inner.vt == VT_LPWSTR && !inner.Anonymous.pwszVal.is_null() {
            inner.Anonymous.pwszVal.to_string().ok()
        } else {
            None
        };
        let _ = PropVariantClear(&mut value);
        text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
    }
}

/// For a UWP frame window, the process that owns the hosted app (see
/// [`super::logic::filter::resolve_uwp_pid`]).
fn uwp_app_pid(frame: HWND, frame_pid: u32) -> u32 {
    let Some(class) = wide(UWP_CORE_CLASS) else {
        return frame_pid;
    };
    let mut children = Vec::new();
    let mut after: Option<HWND> = None;
    // A frame has a handful of children; the bound guards against a pathological window tree.
    for _ in 0..16 {
        // SAFETY: valid class string; a missing child ends the walk with an error.
        let found = unsafe { FindWindowExW(Some(frame), after, pcwstr(&class), PCWSTR::null()) };
        let Ok(child) = found else { break };
        if child.is_invalid() {
            break;
        }
        children.push((UWP_CORE_CLASS.to_string(), window_pid(child).unwrap_or(0)));
        after = Some(child);
    }
    resolve_uwp_pid(frame_pid, &children)
}

/// The process of the application a window belongs to: the window's own process, except for UWP
/// frames, whose app runs in another process than the frame.
fn resolved_pid(window: HWND, class_name: &str, pid: u32) -> u32 {
    if class_name == UWP_FRAME_CLASS {
        uwp_app_pid(window, pid)
    } else {
        pid
    }
}

/// [`resolved_pid`] of a window, or `None` if the window no longer exists.
pub fn app_pid(window: HWND) -> Option<u32> {
    let pid = window_pid(window)?;
    Some(resolved_pid(window, &class_name(window), pid))
}

/// Sizing-frame thickness (`SM_CXSIZEFRAME + SM_CXPADDEDBORDER`) at the window's DPI.
fn frame_thickness(window: HWND) -> i32 {
    let dpi = get_dpi_for_window()
        // SAFETY: plain query; an invalid window yields 0.
        .map(|get_dpi| unsafe { get_dpi(window.0) })
        .filter(|&dpi| dpi > 0);
    match (dpi, get_system_metrics_for_dpi()) {
        // SAFETY: plain queries.
        (Some(dpi), Some(metrics)) => unsafe {
            metrics(SM_CXSIZEFRAME.0, dpi) + metrics(SM_CXPADDEDBORDER.0, dpi)
        },
        // SAFETY: plain queries.
        _ => unsafe { GetSystemMetrics(SM_CXSIZEFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER) },
    }
}

/// Current state of a window and the monitor it is on.
pub fn state_and_monitor(
    window: HWND,
    style: u32,
    window_rect: Rect,
    monitors: &MonitorSnapshot,
) -> (WindowState, Option<&MonitorRecord>) {
    // SAFETY: plain queries; MonitorFromWindow uses the restored position of minimized windows.
    let (iconic, zoomed, monitor) = unsafe {
        (
            IsIconic(window).as_bool(),
            IsZoomed(window).as_bool(),
            MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST),
        )
    };
    let record = monitors.find(monitor.0 as isize);
    let state = window_state(
        iconic,
        zoomed,
        style,
        window_rect,
        record.map(|r| r.info.bounds),
    );
    (state, record)
}

/// Invisible-border insets of the window's *normal* frame, refreshing the cache while the window
/// is restored (when the border can be measured exactly).
pub fn normal_insets(
    window: HWND,
    state: WindowState,
    style: u32,
    window_rect: Rect,
    frame: Option<Rect>,
    cache: &mut InsetsCache,
) -> Insets {
    let handle = raw_hwnd(window);
    let observed = frame.and_then(|f| Insets::between(window_rect, f));
    if state == WindowState::Normal
        && let Some(insets) = observed
    {
        cache.insert(handle, insets);
    }
    let estimate = if state == WindowState::Normal || frame.is_none() {
        Insets::ZERO // not used in these cases
    } else {
        estimated_insets(style & WS_THICKFRAME != 0, frame_thickness(window))
    };
    normal_state_insets(
        state,
        frame.is_some(),
        observed,
        cache.get(&handle).copied(),
        estimate,
    )
}

/// Raw facts about a visible top-level window.
struct Probe {
    handle: isize,
    pid: u32,
    title: String,
    class_name: String,
    style: u32,
    ex_style: u32,
    cloaked: bool,
    has_owner: bool,
    window_rect: Rect,
    frame: Option<Rect>,
}

impl Probe {
    /// `None` for invisible windows (never listed, so the other queries are skipped) and for
    /// windows that vanished meanwhile.
    fn new(handle: isize, own_pid: u32) -> Option<Self> {
        let window = hwnd(handle);
        // SAFETY: plain query on an arbitrary handle.
        if !unsafe { IsWindowVisible(window) }.as_bool() {
            return None;
        }
        let pid = window_pid(window)?;
        let (style, ex_style) = styles(window);
        Some(Self {
            handle,
            pid,
            title: window_title(window, pid == own_pid),
            class_name: class_name(window),
            style,
            ex_style,
            cloaked: is_cloaked(window),
            has_owner: owner(window).is_some(),
            window_rect: window_rect(window)?,
            frame: frame_bounds(window),
        })
    }

    fn facts(&self) -> WindowFacts<'_> {
        WindowFacts {
            visible: true,
            cloaked: self.cloaked,
            style: self.style,
            ex_style: self.ex_style,
            has_owner: self.has_owner,
            title: &self.title,
            class_name: &self.class_name,
            width: self.window_rect.width,
            height: self.window_rect.height,
        }
    }

    fn describe(
        &self,
        monitors: &MonitorSnapshot,
        cache: &mut InsetsCache,
        z_order: usize,
    ) -> Option<WindowInfo> {
        let window = hwnd(self.handle);
        let placement = window_placement(window).ok()?;
        let (state, monitor) = state_and_monitor(window, self.style, self.window_rect, monitors);
        let insets = normal_insets(
            window,
            state,
            self.style,
            self.window_rect,
            self.frame,
            cache,
        );
        let rc = placement.rcNormalPosition;
        let normal = normal_bounds_from_placement(
            rect_from_edges(rc.left, rc.top, rc.right, rc.bottom),
            self.ex_style & WS_EX_TOOLWINDOW == 0,
            &monitors.geometries(),
            insets,
        );
        let normal_bounds = (!normal.is_empty()).then_some(normal);
        // A minimized window has no on-screen frame (Windows parks it at -32000,-32000), so its
        // restored frame is reported instead.
        let bounds = match (state, normal_bounds) {
            (WindowState::Minimized, Some(normal)) => normal,
            _ => visible_bounds(self.window_rect, self.frame),
        };
        Some(WindowInfo {
            id: WindowId::new(format_window_id(self.handle)),
            pid: resolved_pid(window, &self.class_name, self.pid),
            title: self.title.clone(),
            class_name: (!self.class_name.is_empty()).then(|| self.class_name.clone()),
            app_id_hint: app_user_model_id(window),
            bounds,
            normal_bounds,
            state,
            monitor_id: monitor.map(|m| m.info.id.clone()),
            desktop: None,
            is_focused: false,
            z_order: u32::try_from(z_order).unwrap_or(u32::MAX),
        })
    }
}

/// Visible top-level application windows, front to back. Expects a COM apartment and per-monitor
/// DPI awareness on the calling thread (see [`super::ffi`]).
pub fn list_windows(
    monitors: &MonitorSnapshot,
    cache: &mut InsetsCache,
) -> PlatformResult<Vec<WindowInfo>> {
    let handles = top_level_windows()?;
    let own_pid = std::process::id();
    // SAFETY: plain query.
    let foreground = raw_hwnd(unsafe { GetForegroundWindow() });
    let mut listed: Vec<(isize, WindowInfo)> = Vec::new();
    for &handle in &handles {
        let Some(probe) = Probe::new(handle, own_pid) else {
            continue;
        };
        if !is_application_window(&probe.facts()) {
            continue;
        }
        if let Some(info) = probe.describe(monitors, cache, listed.len()) {
            listed.push((handle, info));
        }
    }
    mark_focused(&mut listed, foreground);
    // Forget measurements of windows that no longer exist (handles can be reused).
    let alive: HashSet<isize> = handles.into_iter().collect();
    cache.retain(|handle, _| alive.contains(handle));
    Ok(listed.into_iter().map(|(_, info)| info).collect())
}

/// Mark the foreground window; when the foreground is an owned window that is not listed (a
/// dialog or palette of an application), the nearest listed window up its owner chain is marked
/// instead. The chain is walked with `GW_OWNER` because `GetAncestor(GA_ROOTOWNER)` only follows
/// owners of `WS_POPUP` windows.
fn mark_focused(listed: &mut [(isize, WindowInfo)], foreground: isize) {
    let mut current = foreground;
    // Owner chains are short; the bound guards against pathological cycles.
    for _ in 0..16 {
        if current == 0 {
            return;
        }
        if let Some((_, info)) = listed.iter_mut().find(|(h, _)| *h == current) {
            info.is_focused = true;
            return;
        }
        current = owner(hwnd(current)).map_or(0, raw_hwnd);
    }
}
