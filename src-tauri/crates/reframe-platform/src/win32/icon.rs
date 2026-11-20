//! Application icons as PNG.
//!
//! Sources, in order: the executable (`PrivateExtractIconsW` at the requested size, else the
//! shell icon from `SHGetFileInfoW`), the packaged app (`IShellItemImageFactory` on
//! `shell:AppsFolder\<AUMID>`), then the window (`WM_GETICON`, else the class icon, else the
//! icon of the executable that owns the window). Icons are rendered with `DrawIconEx` into a
//! 32-bit DIB and converted by [`super::logic::icon`]. Every icon, bitmap and device context is
//! released by a guard, including on error paths.
//!
//! Limitation: `PrivateExtractIconsW` only accepts paths shorter than `MAX_PATH`; longer paths use
//! the shell icon, which may be a scaled-up 32 px image.

use std::ffi::c_void;
use std::mem::size_of;
use std::path::Path;

use windows::Win32::Foundation::{HWND, LPARAM, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush, GetDIBits, GetObjectW, HBITMAP, HDC, HGDIOBJ,
    SelectObject,
};
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::System::Com::IBindCtx;
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromParsingName, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON,
    SHGFI_SMALLICON, SHGetFileInfoW, SIIGBF_ICONONLY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DI_FLAGS, DI_MASK, DI_NORMAL, DestroyIcon, DrawIconEx, GCLP_HICON, GCLP_HICONSM,
    GetClassLongPtrW, HICON, ICON_BIG, ICON_SMALL, ICON_SMALL2, PrivateExtractIconsW,
    SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW, WM_GETICON,
};

use super::ffi::{ComScope, pcwstr, resolve_window, wide};
use super::logic::icon::{
    MAX_ICON_SIZE, any_alpha, encode_png, is_blank, is_usable_icon_size, normalized_icon_size,
    rendered_icon_to_rgba, shell_bitmap_to_rgba,
};
use super::logic::launch::{apps_folder_target, validate_aumid};
use super::process::executable_path;
use super::window::app_pid;
use crate::{IconRequest, PackageKind, PlatformError, PlatformResult};

const RENDER: &str = "Rendering an icon";
/// `MAX_PATH`, the buffer size `PrivateExtractIconsW` takes.
const MAX_PATH: usize = 260;
/// How long a window may take to answer `WM_GETICON`.
const WM_GETICON_TIMEOUT_MS: u32 = 200;

/// PNG icon for the request, `Ok(None)` when no source has one. An error is returned only when
/// a source failed and no other source produced an icon.
pub fn application_icon(request: &IconRequest) -> PlatformResult<Option<Vec<u8>>> {
    let size = normalized_icon_size(request.size);
    let _com = ComScope::apartment();
    let mut last_error = None;
    let mut attempt = |result: PlatformResult<Option<Vec<u8>>>| match result {
        Ok(icon) => icon,
        Err(error) => {
            log::debug!("icon source failed: {error}");
            last_error = Some(error);
            None
        }
    };
    if let Some(path) = &request.executable_path
        && let Some(png) = attempt(icon_for_file(path, size))
    {
        return Ok(Some(png));
    }
    if let Some(package) = &request.package
        && package.kind == PackageKind::WindowsPackage
        && let Some(png) = attempt(icon_for_package(&package.id, size))
    {
        return Ok(Some(png));
    }
    if let Some(window) = &request.window
        && let Ok(window) = resolve_window(window)
        && let Some(png) = attempt(icon_for_window(window, size))
    {
        return Ok(Some(png));
    }
    last_error.map_or(Ok(None), Err)
}

/// Icon owned by Reframe, destroyed on drop.
struct OwnedIcon(HICON);

impl Drop for OwnedIcon {
    fn drop(&mut self) {
        // SAFETY: the icon was created for Reframe and is destroyed exactly once.
        let _ = unsafe { DestroyIcon(self.0) };
    }
}

/// GDI bitmap owned by Reframe, deleted on drop.
struct OwnedBitmap(HBITMAP);

impl Drop for OwnedBitmap {
    fn drop(&mut self) {
        // SAFETY: the bitmap is owned and not selected into any DC.
        let _ = unsafe { DeleteObject(HGDIOBJ::from(self.0)) };
    }
}

/// Memory device context, deleted on drop.
struct MemoryDc(HDC);

impl MemoryDc {
    fn new() -> PlatformResult<Self> {
        // SAFETY: creates a DC compatible with the screen.
        let dc = unsafe { CreateCompatibleDC(None) };
        if dc.is_invalid() {
            return Err(PlatformError::os(
                RENDER,
                "could not create a memory device context",
            ));
        }
        Ok(Self(dc))
    }
}

impl Drop for MemoryDc {
    fn drop(&mut self) {
        // SAFETY: owned DC, deleted once (any selected bitmap was deselected before).
        let _ = unsafe { DeleteDC(self.0) };
    }
}

/// A square 32-bit top-down DIB section selected into a memory DC.
struct DibSurface {
    // Field order matters: the bitmap is deselected in `drop`, then deleted, then the DC.
    // Held only so it is released at the right time.
    _bitmap: OwnedBitmap,
    dc: MemoryDc,
    previous: HGDIOBJ,
    bits: *mut u8,
    len: usize,
    side: i32,
}

impl DibSurface {
    fn new(side: u32) -> PlatformResult<Self> {
        let side_i32 =
            i32::try_from(side).map_err(|_| PlatformError::os(RENDER, "icon too large"))?;
        let len = (side as usize) * (side as usize) * 4;
        let dc = MemoryDc::new()?;
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: side_i32,
                biHeight: -side_i32, // negative height: top-down rows
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        // SAFETY: valid BITMAPINFO and out-pointer.
        let bitmap =
            unsafe { CreateDIBSection(Some(dc.0), &info, DIB_RGB_COLORS, &mut bits, None, 0) }
                .map_err(|e| PlatformError::os(RENDER, e.message()))?;
        let bitmap = OwnedBitmap(bitmap);
        if bits.is_null() {
            return Err(PlatformError::os(RENDER, "the bitmap has no pixel memory"));
        }
        // SAFETY: both handles are valid; the previous object is restored in `drop`.
        let previous = unsafe { SelectObject(dc.0, HGDIOBJ::from(bitmap.0)) };
        if previous.is_invalid() {
            // Not selected: the guards delete the bitmap and the DC.
            return Err(PlatformError::os(RENDER, "could not select the bitmap"));
        }
        Ok(Self {
            _bitmap: bitmap,
            dc,
            previous,
            bits: bits.cast(),
            len,
            side: side_i32,
        })
    }

    fn draw(&self, icon: HICON, flags: DI_FLAGS) -> PlatformResult<Vec<u8>> {
        // SAFETY: `bits` points at `len` bytes of DIB memory owned by `bitmap`; GDI has
        // finished pending drawing after GdiFlush.
        unsafe {
            let _ = GdiFlush();
            std::ptr::write_bytes(self.bits, 0, self.len);
            DrawIconEx(self.dc.0, 0, 0, icon, self.side, self.side, 0, None, flags)
                .map_err(|e| PlatformError::os(RENDER, e.message()))?;
            let _ = GdiFlush();
            Ok(std::slice::from_raw_parts(self.bits, self.len).to_vec())
        }
    }
}

impl Drop for DibSurface {
    fn drop(&mut self) {
        // SAFETY: restores the DC's original bitmap; afterwards the fields drop in declaration
        // order, deleting the (now deselected) DIB and then the DC.
        unsafe { SelectObject(self.dc.0, self.previous) };
    }
}

/// Render an icon at `size` px and encode it as PNG (`None` if nothing visible was drawn).
fn render_icon(icon: HICON, size: u32) -> PlatformResult<Option<Vec<u8>>> {
    let surface = DibSurface::new(size)?;
    let color = surface.draw(icon, DI_NORMAL)?;
    let mask = if any_alpha(&color) {
        None
    } else {
        Some(surface.draw(icon, DI_MASK)?)
    };
    let rgba = rendered_icon_to_rgba(&color, mask.as_deref());
    if is_blank(&rgba) {
        return Ok(None);
    }
    encode_png(size, size, &rgba)
        .map(Some)
        .map_err(|e| PlatformError::os("Encoding an icon", e))
}

fn icon_for_file(path: &Path, size: u32) -> PlatformResult<Option<Vec<u8>>> {
    if !path.exists() {
        return Ok(None);
    }
    match extract_file_icon(path, size) {
        Some(icon) => render_icon(icon.0, size),
        None => Ok(None),
    }
}

fn extract_file_icon(path: &Path, size: u32) -> Option<OwnedIcon> {
    let path_w = wide(path)?;
    if path_w.len() <= MAX_PATH {
        let mut buf = [0u16; MAX_PATH];
        buf[..path_w.len()].copy_from_slice(&path_w);
        let mut icons = [HICON::default()];
        let side = i32::try_from(size).unwrap_or(32);
        // SAFETY: NUL-terminated MAX_PATH buffer; room for exactly one icon.
        let extracted =
            unsafe { PrivateExtractIconsW(&buf, 0, side, side, Some(&mut icons), None, 0) };
        if extracted != 0 && extracted != u32::MAX && !icons[0].is_invalid() {
            return Some(OwnedIcon(icons[0]));
        }
    }
    // The icon Explorer shows for the file (the generic program icon when it has none).
    let mut info = SHFILEINFOW::default();
    let kind = if size <= 16 {
        SHGFI_SMALLICON
    } else {
        SHGFI_LARGEICON
    };
    // SAFETY: valid path and output structure with its size.
    let ok = unsafe {
        SHGetFileInfoW(
            pcwstr(&path_w),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut info),
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | kind,
        )
    };
    if info.hIcon.is_invalid() {
        return None;
    }
    let icon = OwnedIcon(info.hIcon); // owned (and destroyed) even if the call reported failure
    (ok != 0).then_some(icon)
}

fn icon_for_package(aumid: &str, size: u32) -> PlatformResult<Option<Vec<u8>>> {
    let Ok(aumid) = validate_aumid(aumid) else {
        return Ok(None);
    };
    let Some(target) = wide(apps_folder_target(aumid)) else {
        return Ok(None);
    };
    let side = i32::try_from(size).unwrap_or(32);
    // SAFETY: COM is initialized by the caller; interfaces are released at the end of scope.
    let bitmap = unsafe {
        let Ok(factory) = SHCreateItemFromParsingName::<_, _, IShellItemImageFactory>(
            pcwstr(&target),
            None::<&IBindCtx>,
        ) else {
            return Ok(None); // not a registered app (or no shell namespace available)
        };
        match factory.GetImage(SIZE { cx: side, cy: side }, SIIGBF_ICONONLY) {
            Ok(bitmap) if !bitmap.is_invalid() => OwnedBitmap(bitmap),
            _ => return Ok(None),
        }
    };
    let (width, height, bgra) = read_bitmap(&bitmap)?;
    if !is_usable_icon_size(width, height, size) {
        return Ok(None);
    }
    let rgba = shell_bitmap_to_rgba(&bgra);
    if is_blank(&rgba) {
        return Ok(None);
    }
    encode_png(width, height, &rgba)
        .map(Some)
        .map_err(|e| PlatformError::os("Encoding an icon", e))
}

/// Pixels of a bitmap as top-down 32-bit BGRA.
fn read_bitmap(bitmap: &OwnedBitmap) -> PlatformResult<(u32, u32, Vec<u8>)> {
    let mut header = BITMAP::default();
    // SAFETY: the output buffer is a BITMAP and its size is passed.
    let copied = unsafe {
        GetObjectW(
            HGDIOBJ::from(bitmap.0),
            size_of::<BITMAP>() as i32,
            Some(&mut header as *mut BITMAP as *mut c_void),
        )
    };
    let limit = 4 * MAX_ICON_SIZE as i32;
    let (width, height) = (header.bmWidth, header.bmHeight.saturating_abs());
    if copied == 0 || !(1..=limit).contains(&width) || !(1..=limit).contains(&height) {
        return Err(PlatformError::os(
            RENDER,
            "the shell returned an unusable bitmap",
        ));
    }
    let dc = MemoryDc::new()?;
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];
    // SAFETY: `pixels` holds `height` rows of `width` 32-bit pixels as described by `info`.
    let rows = unsafe {
        GetDIBits(
            dc.0,
            bitmap.0,
            0,
            height as u32,
            Some(pixels.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        )
    };
    if rows != height {
        return Err(PlatformError::os(
            RENDER,
            "could not read the bitmap pixels",
        ));
    }
    Ok((width as u32, height as u32, pixels))
}

/// The window's own icon (`WM_GETICON`), else its class icon, else the icon of the executable
/// that owns the window (what the taskbar shows for windows without an icon). The executable also
/// covers windows whose icon cannot be read from another process: some compatibility layers
/// (Wine) support neither `WM_GETICON` nor `GetClassLongPtrW` across processes.
fn icon_for_window(window: HWND, size: u32) -> PlatformResult<Option<Vec<u8>>> {
    let mut failure = None;
    for icon in window_icons(window, size) {
        match render_icon(icon, size) {
            Ok(Some(png)) => return Ok(Some(png)),
            Ok(None) => {}
            Err(error) => failure = Some(error),
        }
    }
    if let Some(path) = app_pid(window).and_then(executable_path) {
        match icon_for_file(&path, size) {
            Ok(Some(png)) => return Ok(Some(png)),
            Ok(None) => {}
            Err(error) => failure = Some(error),
        }
    }
    failure.map_or(Ok(None), Err)
}

/// Icons the window and its class provide, the preferred size first. They belong to the window or
/// its class: borrowed, never destroyed.
fn window_icons(window: HWND, size: u32) -> Vec<HICON> {
    let small_first = size <= 16;
    let kinds = if small_first {
        [ICON_SMALL2, ICON_SMALL, ICON_BIG]
    } else {
        [ICON_BIG, ICON_SMALL2, ICON_SMALL]
    };
    let class_slots = if small_first {
        [GCLP_HICONSM, GCLP_HICON]
    } else {
        [GCLP_HICON, GCLP_HICONSM]
    };
    let mut icons: Vec<HICON> = Vec::new();
    let mut add = |raw: usize| {
        let icon = HICON(raw as *mut c_void);
        if raw != 0 && !icons.contains(&icon) {
            icons.push(icon);
        }
    };
    for kind in kinds {
        let mut result = 0usize;
        // SAFETY: valid out-pointer; the timeout and SMTO_ABORTIFHUNG keep a hung window from
        // blocking Reframe.
        let answered = unsafe {
            SendMessageTimeoutW(
                window,
                WM_GETICON,
                WPARAM(kind as usize),
                LPARAM(0),
                SMTO_ABORTIFHUNG | SMTO_BLOCK,
                WM_GETICON_TIMEOUT_MS,
                Some(&mut result),
            )
        };
        if answered.0 != 0 {
            add(result);
        }
    }
    for slot in class_slots {
        // SAFETY: plain query; 0 when the class has no such icon.
        add(unsafe { GetClassLongPtrW(window, slot) });
    }
    icons
}
