//! Locating installed applications: `PATH`, the `App Paths` registry, a bounded search of the
//! program directories and start-menu shortcuts (see [`super::logic::locate::rank_candidates`]
//! for the ranking).
//!
//! The directory walks are cached for [`INDEX_TTL`] so locating several applications in a row
//! (as the restore planner does) reads the disk once.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
    REG_SAM_FLAGS, RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegCloseKey, RegGetValueW,
    RegOpenKeyExW,
};
use windows::core::PCWSTR;

use super::ffi::{from_wide, pcwstr, wide};
use super::logic::locate::{
    FileIndex, LocateSources, SearchLimits, clean_registry_path, executable_file_names,
    is_launchable_name, is_shortcut_name, path_key, rank_candidates, shortcut_file_names,
};
use crate::common::locate::find_on_path;
use crate::{LaunchTarget, LocateQuery};

/// How long directory indexes are reused.
pub const INDEX_TTL: Duration = Duration::from_secs(120);
/// Directory levels searched below each root, and the entry budget per root.
const LIMITS: SearchLimits = SearchLimits {
    max_depth: 3,
    max_entries: 25_000,
};
/// Registry values longer than this are not read (App Paths values are short paths).
const MAX_VALUE_BYTES: u32 = 64 * 1024;

/// Cached directory indexes.
#[derive(Debug, Default)]
pub struct LocateCache {
    programs: Option<(Instant, FileIndex)>,
    shortcuts: Option<(Instant, FileIndex)>,
}

/// The cached index, rebuilt when missing or older than [`INDEX_TTL`].
fn fresh_index(
    slot: &mut Option<(Instant, FileIndex)>,
    build: impl FnOnce() -> FileIndex,
) -> &FileIndex {
    let entry = slot
        .take()
        .filter(|(built, _)| built.elapsed() <= INDEX_TTL)
        .unwrap_or_else(|| (Instant::now(), build()));
    &slot.insert(entry).1
}

/// Launch targets for an installed application, most likely first.
pub fn locate(query: &LocateQuery, cache: &mut LocateCache) -> Vec<LaunchTarget> {
    let file_names = executable_file_names(&query.executable_names);
    let shortcut_names = shortcut_file_names(&query.display_names);
    let env = |name: &str| std::env::var(name).ok();

    let app_paths = file_names
        .iter()
        .filter(|name| name.ends_with(".exe"))
        .flat_map(|name| app_path_entries(name, &env))
        .collect();
    let search = if file_names.is_empty() {
        Vec::new()
    } else {
        fresh_index(&mut cache.programs, || {
            FileIndex::build(&program_roots(), is_launchable_name, LIMITS)
        })
        .find(&file_names)
    };
    let shortcuts = if shortcut_names.is_empty() {
        Vec::new()
    } else {
        fresh_index(&mut cache.shortcuts, || {
            FileIndex::build(&start_menu_roots(), is_shortcut_name, LIMITS)
        })
        .find(&shortcut_names)
    };
    rank_candidates(LocateSources {
        on_path: find_on_path(query),
        app_paths,
        // The index may be up to INDEX_TTL old: drop files removed since.
        search: search.into_iter().filter(|p| p.is_file()).collect(),
        shortcuts: shortcuts.into_iter().filter(|p| p.is_file()).collect(),
    })
}

/// Registered `App Paths` for an executable file name: per-user first, then machine-wide
/// (64-bit and 32-bit registry views). Only existing files are returned.
pub fn app_path_entries(file_name: &str, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let subkey = format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{file_name}");
    [
        (HKEY_CURRENT_USER, REG_SAM_FLAGS(0)),
        (HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY),
        (HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY),
    ]
    .into_iter()
    .filter_map(|(root, view)| read_default_value(root, &subkey, view))
    .filter_map(|raw| clean_registry_path(&raw, env))
    .filter(|path| path.is_file())
    .collect()
}

/// Registry key handle, closed on drop.
struct OwnedKey(HKEY);

impl Drop for OwnedKey {
    fn drop(&mut self) {
        // SAFETY: the key was opened by `read_default_value` and is closed exactly once.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

/// The default (unnamed) string value of a registry key, without environment expansion.
fn read_default_value(root: HKEY, subkey: &str, view: REG_SAM_FLAGS) -> Option<String> {
    let subkey = wide(subkey)?;
    let mut key = HKEY::default();
    // SAFETY: valid strings and out-pointer.
    let status = unsafe {
        RegOpenKeyExW(
            root,
            pcwstr(&subkey),
            None,
            KEY_QUERY_VALUE | view,
            &mut key,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let key = OwnedKey(key);
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
    // The value can change between the size query and the read; retry a few times.
    for _ in 0..3 {
        let mut size = 0u32;
        // SAFETY: size query without a buffer.
        let status = unsafe {
            RegGetValueW(
                key.0,
                PCWSTR::null(),
                PCWSTR::null(),
                flags,
                None,
                None,
                Some(&mut size),
            )
        };
        if status != ERROR_SUCCESS || size > MAX_VALUE_BYTES {
            return None;
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut bytes = u32::try_from(buf.len() * 2).ok()?;
        // SAFETY: `buf` holds `bytes` bytes.
        let status = unsafe {
            RegGetValueW(
                key.0,
                PCWSTR::null(),
                PCWSTR::null(),
                flags,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut bytes),
            )
        };
        if status == ERROR_SUCCESS {
            return Some(from_wide(&buf));
        }
        if status != ERROR_MORE_DATA {
            return None;
        }
    }
    None
}

/// Existing directories, without duplicates.
fn existing_dirs(candidates: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for dir in candidates {
        if dir.is_dir() && !dirs.iter().any(|d| path_key(d) == path_key(&dir)) {
            dirs.push(dir);
        }
    }
    dirs
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// `%ProgramFiles%`, `%ProgramFiles(x86)%`, `%ProgramW6432%`, `%LOCALAPPDATA%\Programs`.
fn program_roots() -> Vec<PathBuf> {
    existing_dirs(
        ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
            .into_iter()
            .filter_map(env_dir)
            .chain(env_dir("LOCALAPPDATA").map(|d| d.join("Programs"))),
    )
}

/// Per-user and all-users start menu program folders.
fn start_menu_roots() -> Vec<PathBuf> {
    let programs = Path::new(r"Microsoft\Windows\Start Menu\Programs");
    existing_dirs(
        ["APPDATA", "ProgramData"]
            .into_iter()
            .filter_map(env_dir)
            .map(|d| d.join(programs)),
    )
}
