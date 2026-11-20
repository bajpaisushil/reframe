//! Process enrichment: friendly product names from version resources and packaged-app identity.

use std::collections::{HashMap, HashSet};
use std::ffi::{OsString, c_void};
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::Storage::Packaging::Appx::GetPackageFullName;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::core::PWSTR;

use super::ffi::{OwnedHandle, from_wide, get_application_user_model_id, pcwstr, wide, win32_code};
use super::logic::version::{
    NAME_KEYS, decode_string_value, parse_translations, string_value_path, translation_candidates,
    utf16_units, value_range,
};
use crate::{PackageInfo, PackageKind, ProcessInfo};

/// Upper bound on cached product names (distinct executables); the cache is reset beyond it.
const MAX_CACHED_NAMES: usize = 4096;
/// Version resources larger than this are not read (real ones are a few KiB).
const MAX_VERSION_BLOCK: u32 = 1 << 20;
/// AppUserModelIDs are at most 130 characters; anything longer is bogus.
const MAX_AUMID_UNITS: u32 = 1024;
/// Longest executable path read (the `\\?\` path limit).
const MAX_PATH_UNITS: usize = 32 * 1024;

/// Per-platform caches so repeated listings stay cheap.
#[derive(Debug, Default)]
pub struct ProcessCache {
    /// Executable path → `FileDescription` (or `ProductName`).
    product_names: HashMap<PathBuf, Option<String>>,
    /// (pid, start time) → AppUserModelID of packaged processes.
    packages: HashMap<(u32, Option<u64>), Option<String>>,
}

/// Fill in `product_name` and `package` for every process.
pub fn enrich(processes: &mut [ProcessInfo], cache: &mut ProcessCache) {
    if cache.product_names.len() > MAX_CACHED_NAMES {
        cache.product_names.clear();
    }
    let mut alive = HashSet::with_capacity(processes.len());
    for process in processes.iter_mut() {
        if let Some(path) = &process.executable_path {
            process.product_name = cache
                .product_names
                .entry(path.clone())
                .or_insert_with(|| product_name(path))
                .clone();
        }
        let key = (process.pid, process.start_time);
        alive.insert(key);
        let aumid = cache
            .packages
            .entry(key)
            .or_insert_with(|| package_app_id(process.pid))
            .clone();
        process.package = aumid.map(|id| PackageInfo {
            kind: PackageKind::WindowsPackage,
            id,
            path: None,
        });
    }
    cache.packages.retain(|key, _| alive.contains(key));
}

/// A copy of one value of a version block (bounds-checked). `unit` is the size of the length
/// unit `VerQueryValueW` reports: 1 for binary values, 2 for strings (whose length is in
/// characters).
///
/// The block is borrowed mutably because `VerQueryValueW` may write into its spare area (it
/// converts the strings of 16-bit version resources there).
fn query_value(block: &mut [u8], sub_block: &str, unit: usize) -> Option<Vec<u8>> {
    let sub_block = wide(sub_block)?;
    let mut ptr: *mut c_void = std::ptr::null_mut();
    let mut len = 0u32;
    // SAFETY: `block` is a complete, writable version block from GetFileVersionInfoW; the
    // out-pointers are valid. The returned pointer is only used after the bounds check below.
    let found = unsafe {
        VerQueryValueW(
            block.as_mut_ptr().cast_const().cast(),
            pcwstr(&sub_block),
            &mut ptr,
            &mut len,
        )
    }
    .as_bool();
    if !found || ptr.is_null() || len == 0 {
        return None;
    }
    let byte_len = usize::try_from(len).ok()?.checked_mul(unit)?;
    let range = value_range(block.as_ptr() as usize, block.len(), ptr as usize, byte_len)?;
    Some(block[range].to_vec())
}

/// `FileDescription` (else `ProductName`) of an executable's version resource.
pub fn product_name(path: &Path) -> Option<String> {
    let path_w = wide(path)?;
    // SAFETY: valid NUL-terminated path.
    let size = unsafe { GetFileVersionInfoSizeW(pcwstr(&path_w), None) };
    if size == 0 || size > MAX_VERSION_BLOCK {
        return None;
    }
    let mut block = vec![0u8; size as usize];
    // SAFETY: the buffer has the size passed.
    unsafe { GetFileVersionInfoW(pcwstr(&path_w), None, size, block.as_mut_ptr().cast()) }.ok()?;
    let table = query_value(&mut block, r"\VarFileInfo\Translation", 1)
        .map(|bytes| parse_translations(&bytes))
        .unwrap_or_default();
    let candidates = translation_candidates(&table);
    NAME_KEYS.iter().find_map(|key| {
        candidates.iter().find_map(|&translation| {
            query_value(&mut block, &string_value_path(translation, key), 2)
                .and_then(|bytes| decode_string_value(&utf16_units(&bytes)))
        })
    })
}

/// Full path of a process's executable (`QueryFullProcessImageNameW`), if Reframe may query it.
pub fn executable_path(pid: u32) -> Option<PathBuf> {
    if pid == 0 {
        return None;
    }
    // SAFETY: plain call; failure (access denied, exited) yields an error.
    let handle =
        OwnedHandle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?);
    let mut buf = vec![0u16; 512];
    loop {
        let mut len = u32::try_from(buf.len()).ok()?;
        // SAFETY: `buf` holds `len` UTF-16 units, as passed; the handle is open.
        let result = unsafe {
            QueryFullProcessImageNameW(
                handle.0,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut len,
            )
        };
        match result {
            Ok(()) => {
                let len = usize::try_from(len).ok()?.min(buf.len());
                // Not `String`: Windows paths may contain unpaired surrogates.
                return (len > 0).then(|| PathBuf::from(OsString::from_wide(&buf[..len])));
            }
            Err(error)
                if win32_code(&error) == Some(ERROR_INSUFFICIENT_BUFFER.0)
                    && buf.len() < MAX_PATH_UNITS =>
            {
                let grown = buf.len() * 2;
                buf.resize(grown, 0);
            }
            Err(_) => return None,
        }
    }
}

/// AppUserModelID of a packaged (MSIX/Store) process; `None` for ordinary processes, processes
/// Reframe may not open, and packaged helpers without an application identity.
pub fn package_app_id(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    // SAFETY: plain call; failure (access denied, exited) yields an error.
    let handle =
        OwnedHandle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?);
    // Cheap packaged check first: unpackaged processes report APPMODEL_ERROR_NO_PACKAGE.
    let mut len = 0u32;
    // SAFETY: a zero-length query with no buffer only reports the required length.
    if unsafe { GetPackageFullName(handle.0, &mut len, None) } != ERROR_INSUFFICIENT_BUFFER {
        return None;
    }
    let get_app_id = get_application_user_model_id()?;
    let mut len = 0u32;
    // SAFETY: as above.
    let status = unsafe { get_app_id(handle.0.0, &mut len, std::ptr::null_mut()) };
    if status != ERROR_INSUFFICIENT_BUFFER.0 || len == 0 || len > MAX_AUMID_UNITS {
        return None;
    }
    let mut buf = vec![0u16; len as usize];
    // SAFETY: `buf` holds `len` characters, as passed.
    if unsafe { get_app_id(handle.0.0, &mut len, buf.as_mut_ptr()) } != 0 {
        return None;
    }
    let id = from_wide(&buf);
    let id = id.trim();
    (!id.is_empty()).then(|| id.to_string())
}
