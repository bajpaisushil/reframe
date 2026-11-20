//! Small safe wrappers and RAII guards around Win32 primitives shared by the other modules.

use std::ffi::{CStr, OsStr, c_void};
use std::marker::PhantomData;
use std::os::windows::ffi::OsStrExt;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, SetLastError, WIN32_ERROR};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::LibraryLoader::{
    GetModuleHandleW, GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows::Win32::UI::WindowsAndMessaging::IsWindow;
use windows::core::{PCSTR, PCWSTR};

use super::logic::errors::window_not_found;
use super::logic::filter::parse_window_id;
use crate::{PlatformResult, WindowId};

/// NUL-terminated UTF-16 copy of `s`, or `None` if it contains a NUL (Win32 would silently
/// truncate the string there).
pub fn wide(s: impl AsRef<OsStr>) -> Option<Vec<u16>> {
    let mut units: Vec<u16> = s.as_ref().encode_wide().collect();
    if units.contains(&0) {
        return None;
    }
    units.push(0);
    Some(units)
}

/// Borrow a NUL-terminated UTF-16 buffer as a `PCWSTR`.
pub fn pcwstr(units: &[u16]) -> PCWSTR {
    PCWSTR(units.as_ptr())
}

/// String from a UTF-16 buffer, up to the first NUL (lossy for unpaired surrogates).
pub fn from_wide(units: &[u16]) -> String {
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

/// The Win32 error code carried by a `windows` error (`HRESULT_FROM_WIN32`), if any.
pub fn win32_code(error: &windows::core::Error) -> Option<u32> {
    WIN32_ERROR::from_error(error).map(|code| code.0)
}

/// Reset the thread's last-error value before a call whose failure is classified by its error
/// code, so a call that fails without setting one is not mistaken for an earlier, unrelated
/// error (such as a stale `ERROR_INVALID_WINDOW_HANDLE`).
pub fn clear_last_error() {
    // SAFETY: only sets the calling thread's last-error value.
    unsafe { SetLastError(WIN32_ERROR(0)) };
}

/// Message for an error, also when the call failed without an error code.
pub fn failure_message(error: &windows::core::Error) -> String {
    if error.code().is_ok() {
        "Windows rejected the request without giving a reason".to_string()
    } else {
        error.message()
    }
}

/// Raw handle value ⇄ `HWND`. Window handles are opaque values, never dereferenced.
pub fn hwnd(raw: isize) -> HWND {
    HWND(raw as *mut c_void)
}

pub fn raw_hwnd(handle: HWND) -> isize {
    handle.0 as isize
}

/// Resolve a [`WindowId`] to the handle of a window that still exists.
pub fn resolve_window(id: &WindowId) -> PlatformResult<HWND> {
    let handle = parse_window_id(id.as_str())
        .map(hwnd)
        .ok_or_else(|| window_not_found(id.as_str()))?;
    // SAFETY: IsWindow accepts any value and only reports whether it names a window.
    if unsafe { IsWindow(Some(handle)) }.as_bool() {
        Ok(handle)
    } else {
        Err(window_not_found(id.as_str()))
    }
}

type RawProc = unsafe extern "system" fn() -> isize;

/// Look up an export of a system DLL at run time.
///
/// Used for APIs that are newer than the oldest supported Windows 10 builds or missing from
/// compatibility layers: a statically linked import that does not exist would stop Reframe from
/// starting (or abort when called), whereas a failed lookup simply selects a fallback.
fn system_proc(module: &str, name: &CStr) -> Option<RawProc> {
    let module = wide(module)?;
    // SAFETY: valid NUL-terminated strings; the module stays loaded for the process lifetime
    // (it is never freed), so the returned function pointer stays valid.
    unsafe {
        let handle = GetModuleHandleW(pcwstr(&module))
            .or_else(|_| LoadLibraryExW(pcwstr(&module), None, LOAD_LIBRARY_SEARCH_SYSTEM32))
            .ok()?;
        GetProcAddress(handle, PCSTR(name.as_ptr().cast()))
    }
}

/// Declares an accessor for an optional system API, resolved once and cached.
macro_rules! optional_api {
    ($(#[$meta:meta])* $name:ident: $module:literal, $export:literal, $ty:ty) => {
        $(#[$meta])*
        pub fn $name() -> Option<$ty> {
            static CELL: std::sync::OnceLock<Option<$ty>> = std::sync::OnceLock::new();
            *CELL.get_or_init(|| {
                // SAFETY: `$ty` is the documented signature of the export.
                system_proc($module, $export).map(|p| unsafe { std::mem::transmute::<RawProc, $ty>(p) })
            })
        }
    };
}

/// `HRESULT GetDpiForMonitor(HMONITOR, MONITOR_DPI_TYPE, UINT*, UINT*)`.
pub type GetDpiForMonitorFn =
    unsafe extern "system" fn(*mut c_void, i32, *mut u32, *mut u32) -> i32;
/// `DPI_AWARENESS_CONTEXT SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT)`.
pub type SetThreadDpiAwarenessContextFn = unsafe extern "system" fn(isize) -> isize;
/// `UINT GetDpiForWindow(HWND)`.
pub type GetDpiForWindowFn = unsafe extern "system" fn(*mut c_void) -> u32;
/// `int GetSystemMetricsForDpi(int, UINT)`.
pub type GetSystemMetricsForDpiFn = unsafe extern "system" fn(i32, u32) -> i32;
/// `LONG GetApplicationUserModelId(HANDLE, UINT32*, PWSTR)`.
pub type GetApplicationUserModelIdFn =
    unsafe extern "system" fn(*mut c_void, *mut u32, *mut u16) -> u32;

optional_api!(
    /// Windows 8.1+ (shcore.dll).
    get_dpi_for_monitor: "shcore.dll", c"GetDpiForMonitor", GetDpiForMonitorFn
);
optional_api!(
    /// Windows 10 1607+.
    set_thread_dpi_awareness_context: "user32.dll", c"SetThreadDpiAwarenessContext", SetThreadDpiAwarenessContextFn
);
optional_api!(
    /// Windows 10 1607+.
    get_dpi_for_window: "user32.dll", c"GetDpiForWindow", GetDpiForWindowFn
);
optional_api!(
    /// Windows 10 1607+.
    get_system_metrics_for_dpi: "user32.dll", c"GetSystemMetricsForDpi", GetSystemMetricsForDpiFn
);
optional_api!(
    /// Windows 8+ (packaged-app identity).
    get_application_user_model_id: "kernel32.dll", c"GetApplicationUserModelId", GetApplicationUserModelIdFn
);

/// `DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE` and `..._V2`.
const DPI_CONTEXT_PER_MONITOR: isize = -3;
const DPI_CONTEXT_PER_MONITOR_V2: isize = -4;

/// Makes the current thread per-monitor DPI aware while alive, so every coordinate Reframe reads
/// or writes is in physical pixels regardless of how the host process was configured (the
/// Reframe app itself is already per-monitor aware; this matters for other hosts such as tests).
/// The previous awareness is restored on drop. A no-op where the API does not exist.
pub struct DpiAwarenessScope {
    previous: isize,
    _thread_bound: PhantomData<*const ()>,
}

impl DpiAwarenessScope {
    pub fn per_monitor() -> Self {
        let mut previous = 0;
        if let Some(set) = set_thread_dpi_awareness_context() {
            // SAFETY: plain value arguments; an unsupported context returns NULL and changes nothing.
            previous = unsafe { set(DPI_CONTEXT_PER_MONITOR_V2) };
            if previous == 0 {
                previous = unsafe { set(DPI_CONTEXT_PER_MONITOR) };
            }
        }
        Self {
            previous,
            _thread_bound: PhantomData,
        }
    }
}

impl Drop for DpiAwarenessScope {
    fn drop(&mut self) {
        if self.previous != 0
            && let Some(set) = set_thread_dpi_awareness_context()
        {
            // SAFETY: restores the context value returned by the earlier call on this thread.
            unsafe { set(self.previous) };
        }
    }
}

/// Initializes COM (single-threaded apartment, OLE1 DDE disabled, as the shell APIs require) for
/// the current thread while alive, balancing it with `CoUninitialize` on drop. If the thread
/// already uses another apartment model, COM is usable as is and is left alone.
///
/// Declare it *before* any COM interface in a scope so interfaces are released first.
pub struct ComScope {
    initialized: bool,
    _thread_bound: PhantomData<*const ()>,
}

impl ComScope {
    pub fn apartment() -> Self {
        // SAFETY: standard COM initialization; S_OK/S_FALSE must be balanced, RPC_E_CHANGED_MODE
        // (an error HRESULT) must not.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        Self {
            initialized: hr.is_ok(),
            _thread_bound: PhantomData,
        }
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.initialized {
            // SAFETY: balances the successful CoInitializeEx above, on the same thread.
            unsafe { CoUninitialize() };
        }
    }
}

/// Owned kernel handle, closed on drop.
pub struct OwnedHandle(pub HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: the handle is owned by this guard and closed exactly once.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_strings_round_trip_and_reject_nul() {
        let units = wide("Größe ✓").expect("no NUL");
        assert_eq!(units.last(), Some(&0));
        assert_eq!(from_wide(&units), "Größe ✓");
        assert_eq!(wide("a\0b"), None);
        assert_eq!(from_wide(&[0x41, 0x42]), "AB");
    }

    #[test]
    fn optional_apis_resolve_or_degrade() {
        // Present on every supported Windows; the lookup itself must never crash.
        assert!(get_dpi_for_window().is_some());
        let _ = get_application_user_model_id();
        assert!(system_proc("user32.dll", c"NoSuchExportForReframe").is_none());
        assert!(system_proc("no-such-library-for-reframe.dll", c"Anything").is_none());
    }

    #[test]
    fn scopes_are_balanced() {
        for _ in 0..3 {
            let _dpi = DpiAwarenessScope::per_monitor();
            let _com = ComScope::apartment();
            let _nested = ComScope::apartment();
        }
    }

    #[test]
    fn invalid_window_ids_are_not_found() {
        for id in ["", "zz", "0x0", "0x7ffffff0"] {
            let err = resolve_window(&WindowId::new(id)).unwrap_err();
            assert!(
                matches!(err, crate::PlatformError::NotFound { .. }),
                "{id}: {err:?}"
            );
        }
    }
}
