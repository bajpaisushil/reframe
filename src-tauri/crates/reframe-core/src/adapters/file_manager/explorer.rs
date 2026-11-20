//! Windows File Explorer: which folder each Explorer window shows.
//!
//! The shell's window list (COM `ShellWindows`, the object behind `Shell.Application.Windows()`)
//! has one entry per Explorer window — one per tab on Windows 11 — exposing the top-level window
//! handle (`IWebBrowserApp::HWND`) and the location as a `file://` URL (`LocationURL`). Entries
//! are matched to the selected windows by HWND. When the shell cannot be queried (timeout, COM
//! failure, non-Windows build), titles that are full paths ("Display the full path in the title
//! bar") are used instead.
//!
//! Limitations: special locations (This PC, Home/Quick access, Recycle Bin, libraries, search
//! results, Control Panel) have no file-system path and are skipped with a warning. Each
//! Windows 11 tab is captured as its own folder and reopens as a separate window.

use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::time::Duration;

use reframe_platform::WindowInfo;

use crate::model::FolderRef;

/// Win32 classes of Explorer folder windows (`ExploreWClass` is the legacy tree-view variant).
const FOLDER_WINDOW_CLASSES: &[&str] = &["CabinetWClass", "ExploreWClass"];

/// How long to wait for Explorer to answer the shell-window query.
#[cfg(windows)]
const SHELL_TIMEOUT: Duration = Duration::from_secs(3);

/// Explorer folder windows. explorer.exe also owns the taskbar, the desktop and other shell UI,
/// which are not file-manager windows.
pub(super) fn is_folder_window(window: &WindowInfo) -> bool {
    window.class_name.as_deref().is_some_and(|class| {
        FOLDER_WINDOW_CLASSES
            .iter()
            .any(|known| known.eq_ignore_ascii_case(class))
    })
}

/// One entry of the shell's window list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ShellWindow {
    /// Top-level window handle (sign-extended on 64-bit Windows).
    pub hwnd: i64,
    /// `file:///C:/Users/me` for folders; empty or `::{GUID}`-style for special locations.
    pub location_url: String,
}

/// Folders shown in the selected Explorer folder windows, plus warnings.
pub(super) fn capture(windows: &[&WindowInfo]) -> (Vec<FolderRef>, Vec<String>) {
    if windows.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let shell = shell_windows();
    folders_for_windows(windows, shell, &|candidate: &str| {
        Path::new(candidate).is_dir()
    })
}

/// The shell's window list.
fn shell_windows() -> Result<Vec<ShellWindow>, String> {
    query_shell_windows().map(|entries| {
        entries
            .into_iter()
            .map(|(hwnd, location_url)| ShellWindow { hwnd, location_url })
            .collect()
    })
}

#[cfg(windows)]
fn query_shell_windows() -> Result<Vec<(i64, String)>, String> {
    super::support::run_with_timeout("explorer-windows", SHELL_TIMEOUT, com::shell_windows)
        .unwrap_or_else(|| Err("File Explorer did not answer in time".to_string()))
}

#[cfg(not(windows))]
fn query_shell_windows() -> Result<Vec<(i64, String)>, String> {
    Err("the File Explorer integration is only available on Windows".to_string())
}

/// Match shell entries to `windows` by HWND (falling back to full-path titles). `is_dir`
/// checks candidate paths (injected for tests).
pub(super) fn folders_for_windows(
    windows: &[&WindowInfo],
    shell: Result<Vec<ShellWindow>, String>,
    is_dir: &dyn Fn(&str) -> bool,
) -> (Vec<FolderRef>, Vec<String>) {
    let mut warnings = Vec::new();
    let shell = shell.unwrap_or_else(|error| {
        warnings.push(format!(
            "could not read File Explorer's open folders ({error}); only windows showing a full \
             path in their title were captured."
        ));
        Vec::new()
    });

    let mut folders = Vec::new();
    for window in windows {
        let entries: Vec<&ShellWindow> = match parse_hwnd(window.id.as_str()) {
            Some(hwnd) => shell.iter().filter(|s| same_hwnd(s.hwnd, hwnd)).collect(),
            None => Vec::new(),
        };
        let from_shell: Vec<String> = entries
            .iter()
            .filter_map(|entry| {
                file_url_candidates(&entry.location_url)
                    .into_iter()
                    .find(|candidate| is_dir(candidate))
            })
            .collect();
        if !from_shell.is_empty() {
            for path in from_shell {
                folders.push(FolderRef {
                    path: PathBuf::from(path),
                    window_title: Some(window.title.clone()),
                });
            }
            continue;
        }
        if let Some(path) = title_path_candidates(&window.title)
            .into_iter()
            .find(|candidate| is_dir(candidate))
        {
            folders.push(FolderRef {
                path: PathBuf::from(path),
                window_title: Some(window.title.clone()),
            });
            continue;
        }
        warnings.push(if entries.is_empty() {
            format!(
                "could not determine the folder shown in \"{}\"; that window was skipped.",
                window.title
            )
        } else {
            format!(
                "\"{}\" shows a location that is not a folder (such as This PC or the Recycle \
                 Bin); that window was skipped.",
                window.title
            )
        });
    }
    (folders, warnings)
}

/// Parse a window id written as an HWND hex string, leniently: surrounding whitespace, an
/// optional `hwnd:` prefix, an optional `0x`, any case and leading zeros. Returns the low
/// 32 bits — the significant part of a window handle, also across 32/64-bit sign extension.
pub(super) fn parse_hwnd(id: &str) -> Option<u32> {
    let mut text = id.trim();
    if text
        .get(..5)
        .is_some_and(|p| p.eq_ignore_ascii_case("hwnd:"))
    {
        text = text[5..].trim_start();
    }
    let digits = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text);
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        // "0x0" / "0" is the null handle, never a window.
        return None;
    }
    if digits.len() > 16 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let value = u64::from_str_radix(digits, 16).ok()?;
    Some((value & 0xFFFF_FFFF) as u32)
}

fn same_hwnd(shell_hwnd: i64, window: u32) -> bool {
    (shell_hwnd as u64 & 0xFFFF_FFFF) as u32 == window
}

/// Windows paths a shell `file:` URL may stand for, most likely first.
///
/// Explorer percent-encodes some characters (spaces as `%20`) but not consistently (`#` and `%`
/// in folder names may appear raw), so both the decoded and the raw spelling are returned and
/// the caller keeps the first that exists. Non-file locations yield nothing.
pub(super) fn file_url_candidates(url: &str) -> Vec<String> {
    let url = url.trim();
    if !url
        .get(..5)
        .is_some_and(|p| p.eq_ignore_ascii_case("file:"))
    {
        return Vec::new();
    }
    let Some(rest) = url[5..].strip_prefix("//") else {
        return Vec::new();
    };
    let (host, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, ""),
    };
    let raw = if host.is_empty() || host.eq_ignore_ascii_case("localhost") {
        // "/C:/Users/me" (also the legacy "/C|/Users/me").
        let path = path.strip_prefix('/').unwrap_or(path);
        let mut chars = path.chars();
        let (Some(letter), Some(colon)) = (chars.next(), chars.next()) else {
            return Vec::new();
        };
        if !letter.is_ascii_alphabetic() || !matches!(colon, ':' | '|') {
            return Vec::new();
        }
        let tail = &path[2..];
        if !(tail.is_empty() || tail.starts_with('/')) {
            return Vec::new();
        }
        format!("{letter}:{}", if tail.is_empty() { "/" } else { tail })
    } else {
        // UNC: file://server/share/dir → \\server\share\dir
        format!("//{host}{path}")
    };

    let to_windows = |s: &str| s.replace('/', "\\");
    let mut candidates = Vec::new();
    if let Some(decoded) = percent_decode(&raw) {
        candidates.push(to_windows(&decoded));
    }
    let raw = to_windows(&raw);
    if !candidates.contains(&raw) {
        candidates.push(raw);
    }
    candidates
}

/// Decode `%XX` escapes as UTF-8. Malformed escapes are kept literally; `None` when the decoded
/// bytes are not UTF-8.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
        {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Full paths a window title may show, longest first: the title itself and the title cut at
/// each ` - ` (Windows 11 appends ` - File Explorer`, localized, and ` and N more tabs`).
/// Only absolute drive (`C:\…`) and UNC (`\\server\share…`) paths are returned.
pub(super) fn title_path_candidates(title: &str) -> Vec<String> {
    let title = title.trim();
    let mut pieces = vec![title.to_string()];
    for (index, _) in title.match_indices(" - ") {
        pieces.push(title[..index].to_string());
    }
    let mut candidates: Vec<String> = Vec::new();
    for piece in pieces {
        let piece = strip_more_tabs(piece.trim()).trim_end().to_string();
        if is_windows_absolute(&piece) && !candidates.contains(&piece) {
            candidates.push(piece);
        }
    }
    candidates.sort_by_key(|c| std::cmp::Reverse(c.len()));
    candidates
}

/// `C:\Work and 2 more tabs` → `C:\Work` (English Windows 11 wording).
fn strip_more_tabs(text: &str) -> &str {
    if let Some(index) = text.rfind(" and ") {
        let tail = &text[index + 5..];
        let mut words = tail.split_whitespace();
        if let (Some(count), Some("more"), Some("tab" | "tabs"), None) =
            (words.next(), words.next(), words.next(), words.next())
            && count.chars().all(|c| c.is_ascii_digit())
        {
            return &text[..index];
        }
    }
    text
}

fn is_windows_absolute(text: &str) -> bool {
    let bytes = text.as_bytes();
    let drive =
        bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\';
    let unc = text.starts_with("\\\\")
        && text[2..]
            .split('\\')
            .take(2)
            .filter(|part| !part.is_empty())
            .count()
            == 2;
    drive || unc
}

/// COM access to the shell's window list. Runs on a dedicated helper thread (see
/// [`query_shell_windows`]): COM is initialised as a single-threaded apartment there and
/// uninitialised before the thread ends, so the caller's COM state is never touched.
#[cfg(windows)]
mod com {
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
        CoInitializeEx, CoUninitialize,
    };
    use windows::Win32::System::Variant::{VARIANT, VT_I4};
    use windows::Win32::UI::Shell::{IShellWindows, IWebBrowserApp, ShellWindows};
    use windows::core::Interface;

    /// Upper bound for entries read (one per Explorer window or tab).
    const MAX_ENTRIES: i32 = 256;

    pub(super) fn shell_windows() -> Result<Vec<(i64, String)>, String> {
        // SAFETY: plain COM initialisation on the current (dedicated helper) thread.
        let init =
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        if init.is_err() {
            return Err(format!("COM initialisation failed ({})", init.message()));
        }
        // Every COM object created by `enumerate` is dropped before it returns.
        let result = enumerate();
        // SAFETY: balances the successful CoInitializeEx above, on the same thread.
        unsafe { CoUninitialize() };
        result
    }

    fn enumerate() -> Result<Vec<(i64, String)>, String> {
        // SAFETY: COM is initialised on this thread; ShellWindows is the documented class
        // implementing IShellWindows.
        let windows: IShellWindows =
            unsafe { CoCreateInstance(&ShellWindows, None, CLSCTX_ALL) }
                .map_err(|e| format!("the shell window list is not available ({})", e.message()))?;
        // SAFETY: valid interface pointer obtained above.
        let count = unsafe { windows.Count() }
            .map_err(|e| format!("could not count shell windows ({})", e.message()))?;
        let mut entries = Vec::new();
        for index in 0..count.clamp(0, MAX_ENTRIES) {
            let mut variant = VARIANT::default();
            // SAFETY: writes the VT_I4 tag and its integer payload into a zero-initialised
            // VARIANT; an integer VARIANT owns no resources, so no VariantClear is needed.
            unsafe {
                (*variant.Anonymous.Anonymous).vt = VT_I4;
                (*variant.Anonymous.Anonymous).Anonymous.lVal = index;
            }
            // SAFETY: valid interface and a well-formed VARIANT; failures are skipped.
            let Ok(dispatch) = (unsafe { windows.Item(&variant) }) else {
                continue;
            };
            let Ok(browser) = dispatch.cast::<IWebBrowserApp>() else {
                continue;
            };
            // SAFETY: valid IWebBrowserApp pointer.
            let Ok(hwnd) = (unsafe { browser.HWND() }) else {
                continue;
            };
            // SAFETY: valid IWebBrowserApp pointer; the BSTR is owned by the returned value.
            let location = unsafe { browser.LocationURL() }
                .map(|url| url.to_string())
                .unwrap_or_default();
            entries.push((hwnd.0 as i64, location));
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::file_manager::support::testing::window;

    #[test]
    fn only_cabinet_windows_are_folder_windows() {
        assert!(is_folder_window(&window(
            "0x1",
            1,
            "Downloads",
            Some("CabinetWClass")
        )));
        assert!(is_folder_window(&window(
            "0x1",
            1,
            "x",
            Some("ExploreWClass")
        )));
        assert!(!is_folder_window(&window(
            "0x1",
            1,
            "",
            Some("Shell_TrayWnd")
        )));
        assert!(!is_folder_window(&window(
            "0x1",
            1,
            "Program Manager",
            Some("Progman")
        )));
        assert!(!is_folder_window(&window(
            "0x1",
            1,
            "Properties",
            Some("#32770")
        )));
        assert!(!is_folder_window(&window("0x1", 1, "Downloads", None)));
    }

    #[test]
    fn parses_window_ids_leniently() {
        assert_eq!(parse_hwnd("0x1A2B3C"), Some(0x1A2B3C));
        assert_eq!(parse_hwnd("0x00000000001a2b3c"), Some(0x1A2B3C));
        assert_eq!(parse_hwnd(" 1A2B3C "), Some(0x1A2B3C));
        assert_eq!(parse_hwnd("HWND:0x10"), Some(0x10));
        assert_eq!(parse_hwnd("0XFFFFFFFF80001234"), Some(0x8000_1234));
        assert_eq!(parse_hwnd("0x0"), None);
        assert_eq!(parse_hwnd(""), None);
        assert_eq!(parse_hwnd("mock-1"), None);
        assert_eq!(parse_hwnd("0x12345678901234567"), None);
    }

    #[test]
    fn multibyte_input_never_panics() {
        for id in ["héllo", "éééé", "hwnd:é", "0xé"] {
            assert_eq!(parse_hwnd(id), None, "{id}");
        }
        for url in ["fé:///x", "filé:///C:/x", "file:///é:/x", "file://é/share"] {
            let _ = file_url_candidates(url);
        }
        assert!(file_url_candidates("file:///é:/x").is_empty());
        assert!(title_path_candidates("é - ü and 2 more tabs - ß").is_empty());
    }

    #[test]
    fn hwnd_comparison_ignores_sign_extension() {
        assert!(same_hwnd(0x1A2B3C, 0x1A2B3C));
        assert!(same_hwnd(0xFFFF_FFFF_8000_1234_u64 as i64, 0x8000_1234));
        assert!(!same_hwnd(0x1A2B3D, 0x1A2B3C));
    }

    #[test]
    fn converts_file_urls_to_windows_paths() {
        assert_eq!(
            file_url_candidates("file:///C:/Users/me/My%20Documents"),
            vec![
                r"C:\Users\me\My Documents".to_string(),
                r"C:\Users\me\My%20Documents".to_string()
            ]
        );
        assert_eq!(file_url_candidates("file:///D:/"), vec![r"D:\".to_string()]);
        assert_eq!(file_url_candidates("FILE:///c:"), vec![r"c:\".to_string()]);
        assert_eq!(
            file_url_candidates("file://localhost/C:/Work"),
            vec![r"C:\Work".to_string()]
        );
        assert_eq!(
            file_url_candidates("file:///C|/Legacy"),
            vec![r"C:\Legacy".to_string()]
        );
        assert_eq!(
            file_url_candidates("file://server/share/team%20docs"),
            vec![
                r"\\server\share\team docs".to_string(),
                r"\\server\share\team%20docs".to_string()
            ]
        );
        assert_eq!(
            file_url_candidates("file:///C:/Users/José/Música"),
            vec![r"C:\Users\José\Música".to_string()]
        );
        assert_eq!(
            file_url_candidates("file:///C:/Users/me/C%23%20code#1"),
            vec![
                r"C:\Users\me\C# code#1".to_string(),
                r"C:\Users\me\C%23%20code#1".to_string()
            ]
        );
        // Not decodable as UTF-8: only the raw spelling.
        assert_eq!(
            file_url_candidates("file:///C:/a%FF"),
            vec![r"C:\a%FF".to_string()]
        );
        for special in [
            "",
            "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}",
            "https://example.com/",
            "file:relative",
            "file:///Users/me",
            "file:///C:relative",
        ] {
            assert!(file_url_candidates(special).is_empty(), "{special}");
        }
    }

    #[test]
    fn title_candidates_cover_full_path_titles() {
        assert_eq!(
            title_path_candidates(r"C:\Users\me\Downloads"),
            vec![r"C:\Users\me\Downloads".to_string()]
        );
        assert_eq!(
            title_path_candidates(r"C:\Projects\A - B - File Explorer"),
            vec![
                r"C:\Projects\A - B - File Explorer".to_string(),
                r"C:\Projects\A - B".to_string(),
                r"C:\Projects\A".to_string(),
            ]
        );
        assert_eq!(
            title_path_candidates(r"C:\Work and 2 more tabs - File Explorer"),
            vec![
                r"C:\Work and 2 more tabs - File Explorer".to_string(),
                r"C:\Work".to_string(),
            ]
        );
        assert_eq!(
            title_path_candidates(r"\\nas\media\films - Explorador de archivos"),
            vec![
                r"\\nas\media\films - Explorador de archivos".to_string(),
                r"\\nas\media\films".to_string(),
            ]
        );
        assert!(title_path_candidates("Downloads - File Explorer").is_empty());
        assert!(title_path_candidates(r"\\nas").is_empty());
        assert!(title_path_candidates("Local Disk (C:)").is_empty());
    }

    fn shell(hwnd: i64, url: &str) -> ShellWindow {
        ShellWindow {
            hwnd,
            location_url: url.to_string(),
        }
    }

    #[test]
    fn matches_shell_entries_by_hwnd_including_tabs() {
        let existing = [r"C:\Users\me\Downloads", r"C:\Work", r"C:\Repo"];
        let is_dir = |p: &str| existing.contains(&p);
        let w1 = window(
            "0x0000A1",
            1,
            "Downloads - File Explorer",
            Some("CabinetWClass"),
        );
        let w2 = window(
            "0xB2",
            1,
            "Work and 1 more tab - File Explorer",
            Some("CabinetWClass"),
        );
        let w3 = window("0xC3", 1, "This PC - File Explorer", Some("CabinetWClass"));
        let w4 = window("0xD4", 1, r"C:\Repo", Some("CabinetWClass"));
        let w5 = window("0xE5", 1, "Gone", Some("CabinetWClass"));
        let entries = vec![
            shell(0xA1, "file:///C:/Users/me/Downloads"),
            shell(0xB2, "file:///C:/Work"),
            shell(0xB2, "file:///C:/Repo"),
            shell(0xC3, ""),
            shell(0xF6, "file:///C:/Other"),
        ];
        let (folders, warnings) =
            folders_for_windows(&[&w1, &w2, &w3, &w4, &w5], Ok(entries), &is_dir);
        let paths: Vec<_> = folders.iter().map(|f| f.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                PathBuf::from(r"C:\Users\me\Downloads"),
                PathBuf::from(r"C:\Work"),
                PathBuf::from(r"C:\Repo"),
                PathBuf::from(r"C:\Repo"),
            ]
        );
        assert_eq!(
            folders[1].window_title.as_deref(),
            Some("Work and 1 more tab - File Explorer")
        );
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("This PC"));
        assert!(warnings[1].contains("could not determine the folder shown in \"Gone\""));
    }

    #[test]
    fn falls_back_to_full_path_titles_when_the_shell_is_unavailable() {
        let is_dir = |p: &str| p == r"C:\Repo";
        let w1 = window("0xD4", 1, r"C:\Repo - File Explorer", Some("CabinetWClass"));
        let w2 = window(
            "0xD5",
            1,
            "Downloads - File Explorer",
            Some("CabinetWClass"),
        );
        let (folders, warnings) =
            folders_for_windows(&[&w1, &w2], Err("COM failed".into()), &is_dir);
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].path, PathBuf::from(r"C:\Repo"));
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("COM failed"));
    }

    #[cfg(not(windows))]
    #[test]
    fn capture_off_windows_reports_why() {
        let w = window("0x1", 1, "Downloads", Some("CabinetWClass"));
        let (folders, warnings) = capture(&[&w]);
        assert!(folders.is_empty());
        assert!(warnings[0].contains("only available on Windows"));
        assert!(capture(&[]).1.is_empty());
    }
}
