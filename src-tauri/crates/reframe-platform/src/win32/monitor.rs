//! Monitors: `EnumDisplayMonitors` + `GetMonitorInfoW` for geometry, the display configuration
//! API for stable ids and friendly names, `GetDpiForMonitor` for the scale factor.
//!
//! Coordinates are physical pixels in the virtual-screen space (origin at the primary monitor's
//! top-left corner); callers hold a [`super::ffi::DpiAwarenessScope`] so this holds even when
//! the host process is not per-monitor DPI aware.

use std::ffi::c_void;
use std::mem::size_of;

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo,
    GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetDC, GetDeviceCaps, GetMonitorInfoW, HDC, HMONITOR, LOGPIXELSX,
    MONITORINFO, MONITORINFOEXW, ReleaseDC,
};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;
use windows::core::BOOL;

use super::ffi::{from_wide, get_dpi_for_monitor};
use super::logic::geometry::{MonitorGeometry, rect_from_edges};
use super::logic::monitor::{
    DisplayTarget, dedupe_ids, is_internal_output, monitor_identity, scale_factor_from_dpi,
};
use crate::{MonitorInfo, PlatformError, PlatformResult, Rect};

/// `MDT_EFFECTIVE_DPI`.
const MDT_EFFECTIVE_DPI: i32 = 0;

/// One monitor as seen by Win32, with the data Reframe reports.
#[derive(Debug, Clone)]
pub struct MonitorRecord {
    /// `HMONITOR` value (valid until the display configuration changes).
    pub handle: isize,
    /// GDI device name, e.g. `\\.\DISPLAY1`.
    pub device_name: String,
    pub info: MonitorInfo,
}

/// All monitors at one point in time.
#[derive(Debug, Clone, Default)]
pub struct MonitorSnapshot {
    pub records: Vec<MonitorRecord>,
}

impl MonitorSnapshot {
    /// Enumerate the monitors. Missing display-configuration data only degrades ids/names.
    pub fn capture() -> PlatformResult<Self> {
        let handles = monitor_handles()?;
        let targets = display_targets();
        let mut records: Vec<MonitorRecord> = handles
            .into_iter()
            .filter_map(|handle| {
                let (monitor, work, primary, device_name) = monitor_details(handle)?;
                let identity = monitor_identity(&device_name, &targets);
                let dpi = monitor_dpi(handle).unwrap_or_else(system_dpi);
                Some(MonitorRecord {
                    handle,
                    device_name,
                    info: MonitorInfo {
                        id: identity.id,
                        name: identity.name,
                        bounds: monitor,
                        work_area: work,
                        scale_factor: scale_factor_from_dpi(dpi),
                        is_primary: primary,
                    },
                })
            })
            .collect();
        // Primary first, then left to right, top to bottom.
        records.sort_by_key(|r| (!r.info.is_primary, r.info.bounds.x, r.info.bounds.y));
        let mut ids: Vec<String> = records.iter().map(|r| r.info.id.clone()).collect();
        dedupe_ids(&mut ids);
        for (record, id) in records.iter_mut().zip(ids) {
            record.info.id = id;
        }
        Ok(Self { records })
    }

    pub fn monitors(&self) -> Vec<MonitorInfo> {
        self.records.iter().map(|r| r.info.clone()).collect()
    }

    pub fn geometries(&self) -> Vec<MonitorGeometry> {
        self.records
            .iter()
            .map(|r| MonitorGeometry {
                bounds: r.info.bounds,
                work_area: r.info.work_area,
                is_primary: r.info.is_primary,
            })
            .collect()
    }

    /// The monitor for an `HMONITOR` value. Falls back to matching the GDI device name in case
    /// the handle was re-created since the snapshot.
    pub fn find(&self, handle: isize) -> Option<&MonitorRecord> {
        self.records
            .iter()
            .find(|r| r.handle == handle)
            .or_else(|| {
                let (_, _, _, device_name) = monitor_details(handle)?;
                self.records.iter().find(|r| r.device_name == device_name)
            })
    }
}

unsafe extern "system" fn collect_monitor(
    monitor: HMONITOR,
    _dc: HDC,
    _clip: *mut RECT,
    data: LPARAM,
) -> BOOL {
    // SAFETY: `data` is the `&mut Vec<isize>` passed by `monitor_handles`, alive during the call.
    let handles = unsafe { &mut *(data.0 as *mut Vec<isize>) };
    handles.push(monitor.0 as isize);
    BOOL::from(true)
}

fn monitor_handles() -> PlatformResult<Vec<isize>> {
    let mut handles: Vec<isize> = Vec::new();
    // SAFETY: the callback only pushes into `handles`, which outlives the call.
    let ok = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect_monitor),
            LPARAM(&mut handles as *mut Vec<isize> as isize),
        )
    };
    if !ok.as_bool() && handles.is_empty() {
        return Err(PlatformError::os(
            "Listing monitors",
            windows::core::Error::from_thread().message(),
        ));
    }
    Ok(handles)
}

/// `(bounds, work area, is primary, GDI device name)` of a monitor.
fn monitor_details(handle: isize) -> Option<(Rect, Rect, bool, String)> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: `info` is a properly sized MONITORINFOEXW (cbSize says so).
    let ok = unsafe {
        GetMonitorInfoW(
            HMONITOR(handle as *mut c_void),
            &mut info as *mut MONITORINFOEXW as *mut MONITORINFO,
        )
    };
    if !ok.as_bool() {
        return None;
    }
    let mi = info.monitorInfo;
    let rect = |r: RECT| rect_from_edges(r.left, r.top, r.right, r.bottom);
    Some((
        rect(mi.rcMonitor),
        rect(mi.rcWork),
        mi.dwFlags & MONITORINFOF_PRIMARY != 0,
        from_wide(&info.szDevice),
    ))
}

/// Effective DPI of a monitor (Windows 8.1+).
fn monitor_dpi(handle: isize) -> Option<u32> {
    let get_dpi = get_dpi_for_monitor()?;
    let (mut x, mut y) = (0u32, 0u32);
    // SAFETY: valid out-pointers; an invalid monitor handle just yields an error HRESULT.
    let hr = unsafe { get_dpi(handle as *mut c_void, MDT_EFFECTIVE_DPI, &mut x, &mut y) };
    (hr >= 0 && x > 0).then_some(x)
}

/// System DPI, the fallback scale when per-monitor DPI is unavailable.
fn system_dpi() -> u32 {
    // SAFETY: screen DC acquired and released here.
    unsafe {
        let dc = GetDC(None);
        if dc.is_invalid() {
            return 96;
        }
        let dpi = GetDeviceCaps(Some(dc), LOGPIXELSX);
        ReleaseDC(None, dc);
        u32::try_from(dpi).ok().filter(|&d| d > 0).unwrap_or(96)
    }
}

/// Active display paths with their GDI source names and target monitor names/paths. Empty when
/// the display configuration API is unavailable or fails (ids then fall back to GDI names).
fn display_targets() -> Vec<DisplayTarget> {
    for _attempt in 0..3 {
        let (mut path_count, mut mode_count) = (0u32, 0u32);
        // SAFETY: valid out-pointers.
        let status = unsafe {
            GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
        };
        if status != ERROR_SUCCESS {
            return Vec::new();
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
        // SAFETY: the buffers hold `path_count` / `mode_count` elements, as the counts say.
        let status = unsafe {
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut path_count,
                paths.as_mut_ptr(),
                &mut mode_count,
                modes.as_mut_ptr(),
                None,
            )
        };
        if status == ERROR_INSUFFICIENT_BUFFER {
            continue; // the configuration changed between the two calls
        }
        if status != ERROR_SUCCESS {
            return Vec::new();
        }
        paths.truncate(path_count as usize);
        return paths.iter().filter_map(describe_path).collect();
    }
    Vec::new()
}

fn describe_path(path: &DISPLAYCONFIG_PATH_INFO) -> Option<DisplayTarget> {
    let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
    source.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
    source.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
    source.header.adapterId = path.sourceInfo.adapterId;
    source.header.id = path.sourceInfo.id;
    // SAFETY: the header (the packet's first field) describes the enclosing, properly sized
    // packet; the pointer is derived from the whole packet, which the call fills in.
    let packet = (&raw mut source).cast::<DISPLAYCONFIG_DEVICE_INFO_HEADER>();
    if unsafe { DisplayConfigGetDeviceInfo(packet) } != ERROR_SUCCESS.0 as i32 {
        return None;
    }
    let gdi_device_name = from_wide(&source.viewGdiDeviceName);
    if gdi_device_name.is_empty() {
        return None;
    }

    let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
    target.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
    target.header.size = size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
    target.header.adapterId = path.targetInfo.adapterId;
    target.header.id = path.targetInfo.id;
    // SAFETY: as above.
    let packet = (&raw mut target).cast::<DISPLAYCONFIG_DEVICE_INFO_HEADER>();
    let target_ok = unsafe { DisplayConfigGetDeviceInfo(packet) } == ERROR_SUCCESS.0 as i32;
    Some(if target_ok {
        DisplayTarget {
            gdi_device_name,
            friendly_name: from_wide(&target.monitorFriendlyDeviceName),
            device_path: from_wide(&target.monitorDevicePath),
            internal: is_internal_output(target.outputTechnology.0),
        }
    } else {
        DisplayTarget {
            gdi_device_name,
            ..Default::default()
        }
    })
}
