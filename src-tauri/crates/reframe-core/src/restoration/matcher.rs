//! Matching running windows to workspace entries and saved window snapshots.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use reframe_platform::{ProcessInfo, WindowId, WindowInfo};

use crate::adapters::AdapterRegistry;
use crate::model::{ApplicationEntry, DesktopState, WindowSnapshot};
use crate::util;

/// Minimum score for [`assign_windows`] to pair a window with a snapshot.
pub const MATCH_THRESHOLD: f64 = 0.3;
/// A title hint that appears in both the window title and the snapshot title.
const SHARED_HINT_BONUS: f64 = 0.5;
/// A title hint that appears in the window title only.
const HINT_BONUS: f64 = 0.25;
/// Same window class (case-insensitive).
const CLASS_BONUS: f64 = 0.2;

/// Running windows that belong to the same logical application as `entry`, front-to-back.
///
/// A window belongs to the entry when the adapter registry identifies its owning process as
/// the entry's `app_id`, or — as a fallback — when the process runs the entry's configured
/// executable (compared case-insensitively on Windows, through symlinks elsewhere) or package.
/// Windows whose owning process is unknown cannot be attributed and are never returned.
///
/// Matching is per application, not per project or document: every window of the
/// application is returned, whatever it currently shows.
pub fn running_windows_for_entry(
    entry: &ApplicationEntry,
    state: &DesktopState,
    adapters: &AdapterRegistry,
) -> Vec<WindowId> {
    let processes: HashMap<u32, &ProcessInfo> =
        state.processes.iter().map(|p| (p.pid, p)).collect();
    let mut windows_by_pid: HashMap<u32, Vec<&WindowInfo>> = HashMap::new();
    for window in &state.windows {
        windows_by_pid.entry(window.pid).or_default().push(window);
    }
    let mut verdicts: HashMap<u32, bool> = HashMap::new();
    state
        .windows
        .iter()
        .filter(|w| {
            *verdicts.entry(w.pid).or_insert_with(|| {
                processes.get(&w.pid).is_some_and(|p| {
                    process_belongs_to_entry(
                        entry,
                        p,
                        windows_by_pid
                            .get(&w.pid)
                            .map(Vec::as_slice)
                            .unwrap_or_default(),
                        adapters,
                    )
                })
            })
        })
        .map(|w| w.id.clone())
        .collect()
}

/// Whether `process` (owning `windows`) runs the application of `entry`.
pub(crate) fn process_belongs_to_entry(
    entry: &ApplicationEntry,
    process: &ProcessInfo,
    windows: &[&WindowInfo],
    adapters: &AdapterRegistry,
) -> bool {
    let (_, identity) = adapters.identify(process, windows);
    if identity.app_id.eq_ignore_ascii_case(&entry.app_id) {
        return true;
    }
    if let (Some(configured), Some(running)) = (
        entry.launch.executable.as_deref(),
        process.executable_path.as_deref(),
    ) && same_executable(configured, running)
    {
        return true;
    }
    matches!(
        (&entry.launch.package, &process.package),
        (Some(a), Some(b)) if a.kind == b.kind && a.id.eq_ignore_ascii_case(&b.id)
    )
}

/// Logical application id and display name of the application owning `window` (as the
/// adapters identify its process). Windows whose process is unknown are named after their
/// window class, else "Unknown application", with the id `unknown`.
pub(crate) fn identify_window(
    window: &WindowInfo,
    windows: &[WindowInfo],
    processes: &[ProcessInfo],
    adapters: &AdapterRegistry,
) -> (String, String) {
    if let Some(process) = processes.iter().find(|p| p.pid == window.pid) {
        let owned: Vec<&WindowInfo> = windows.iter().filter(|w| w.pid == window.pid).collect();
        let (_, identity) = adapters.identify(process, &owned);
        return (identity.app_id, identity.display_name);
    }
    let name = window
        .class_name
        .as_deref()
        .map(util::prettify_name)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Unknown application".to_string());
    ("unknown".to_string(), name)
}

/// Whether two executable paths name the same program: case-insensitive with either slash on
/// Windows; on other systems equal paths, or paths that resolve to the same file.
pub(crate) fn same_executable(a: &Path, b: &Path) -> bool {
    if a.as_os_str().is_empty() || b.as_os_str().is_empty() {
        return false;
    }
    if cfg!(windows) {
        let normalize = |p: &Path| p.to_string_lossy().replace('/', "\\").to_lowercase();
        return normalize(a) == normalize(b);
    }
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Assign candidate windows to saved snapshots (title similarity, class, hints). Returns
/// `(snapshot index, window id)` pairs sorted by snapshot index; each window and each
/// snapshot is used at most once.
///
/// Every (snapshot, window) pair is scored with [`util::title_similarity`], plus a bonus when
/// a title hint appears in both titles (a smaller one when it appears in the window title
/// only) and a bonus for an equal window class. Pairs scoring at least [`MATCH_THRESHOLD`] are
/// taken greedily, best first; ties are broken by snapshot index, then candidate order, so
/// the result is deterministic.
pub fn assign_windows(
    saved: &[WindowSnapshot],
    candidates: &[WindowInfo],
    title_hints: &[String],
) -> Vec<(usize, WindowId)> {
    let hints = normalized_hints(title_hints);
    let unique = unique_candidates(candidates);
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (si, snapshot) in saved.iter().enumerate() {
        for &ci in &unique {
            let score = match_score(snapshot, &candidates[ci], &hints);
            if score >= MATCH_THRESHOLD {
                pairs.push((score, si, ci));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));

    let mut snapshot_used = vec![false; saved.len()];
    let mut candidate_used = vec![false; candidates.len()];
    let mut result = Vec::new();
    for (_, si, ci) in pairs {
        if snapshot_used[si] || candidate_used[ci] {
            continue;
        }
        snapshot_used[si] = true;
        candidate_used[ci] = true;
        result.push((si, candidates[ci].id.clone()));
    }
    result.sort_by_key(|(si, _)| *si);
    result
}

/// [`assign_windows`], then pair the remaining snapshots (in order) with the remaining
/// candidates (in the order given). Used once candidates are known to belong to the
/// application, so every window that appeared gets a saved position even when its title
/// changed completely since capture.
pub(crate) fn assign_windows_with_fallback(
    saved: &[WindowSnapshot],
    candidates: &[WindowInfo],
    title_hints: &[String],
) -> Vec<(usize, WindowId)> {
    let mut result = assign_windows(saved, candidates, title_hints);
    let taken_snapshots: HashSet<usize> = result.iter().map(|(si, _)| *si).collect();
    let mut taken_windows: HashSet<WindowId> = result.iter().map(|(_, w)| w.clone()).collect();
    let mut free_windows = candidates
        .iter()
        .filter(|w| taken_windows.insert(w.id.clone()))
        .map(|w| w.id.clone());
    for si in (0..saved.len()).filter(|si| !taken_snapshots.contains(si)) {
        match free_windows.next() {
            Some(window) => result.push((si, window)),
            None => break,
        }
    }
    result.sort_by_key(|(si, _)| *si);
    result
}

/// Score for pairing `snapshot` with `window` (`hints` already trimmed and lower-cased).
fn match_score(snapshot: &WindowSnapshot, window: &WindowInfo, hints: &[String]) -> f64 {
    let mut score = util::title_similarity(&snapshot.title, &window.title);
    if !hints.is_empty() {
        let window_title = window.title.to_lowercase();
        let snapshot_title = snapshot.title.to_lowercase();
        let in_window: Vec<&String> = hints.iter().filter(|h| window_title.contains(*h)).collect();
        if in_window.iter().any(|h| snapshot_title.contains(*h)) {
            score += SHARED_HINT_BONUS;
        } else if !in_window.is_empty() {
            score += HINT_BONUS;
        }
    }
    if let (Some(a), Some(b)) = (&snapshot.class_name, &window.class_name)
        && !a.trim().is_empty()
        && a.trim().eq_ignore_ascii_case(b.trim())
    {
        score += CLASS_BONUS;
    }
    score
}

fn normalized_hints(hints: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for hint in hints {
        let hint = hint.trim().to_lowercase();
        if !hint.is_empty() && !out.contains(&hint) {
            out.push(hint);
        }
    }
    out
}

/// Indices of the first occurrence of every window id.
fn unique_candidates(candidates: &[WindowInfo]) -> Vec<usize> {
    let mut seen = HashSet::new();
    (0..candidates.len())
        .filter(|&i| seen.insert(&candidates[i].id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reframe_platform::mock::MockPlatform;
    use reframe_platform::{PackageInfo, PackageKind, Platform, Rect, WindowState};

    fn snapshot(title: &str, class: Option<&str>) -> WindowSnapshot {
        WindowSnapshot {
            title: title.into(),
            class_name: class.map(str::to_string),
            bounds: Rect::new(0, 0, 800, 600),
            state: WindowState::Normal,
            monitor_id: None,
        }
    }

    fn window(id: &str, title: &str, class: Option<&str>) -> WindowInfo {
        WindowInfo {
            id: WindowId::new(id),
            pid: 1,
            title: title.into(),
            class_name: class.map(str::to_string),
            app_id_hint: None,
            bounds: Rect::new(0, 0, 800, 600),
            normal_bounds: None,
            state: WindowState::Normal,
            monitor_id: None,
            desktop: None,
            is_focused: false,
            z_order: 0,
        }
    }

    fn ids(pairs: &[(usize, WindowId)]) -> Vec<(usize, &str)> {
        pairs.iter().map(|(i, w)| (*i, w.as_str())).collect()
    }

    fn state(platform: &MockPlatform) -> DesktopState {
        DesktopState {
            info: platform.info(),
            monitors: platform.monitors().unwrap(),
            windows: platform.windows().unwrap(),
            processes: platform.processes().unwrap(),
            warnings: Vec::new(),
        }
    }

    /// Only the generic adapter, so these tests do not depend on dedicated adapters.
    fn generic_only() -> AdapterRegistry {
        AdapterRegistry::with_adapters(Vec::new())
    }

    #[test]
    fn assigns_by_title_similarity() {
        let saved = vec![
            snapshot("main.rs — alpha — Visual Studio Code", None),
            snapshot("lib.rs — beta — Visual Studio Code", None),
        ];
        let candidates = vec![
            window("w1", "beta — Visual Studio Code", None),
            window("w2", "alpha — Visual Studio Code", None),
        ];
        assert_eq!(
            ids(&assign_windows(&saved, &candidates, &[])),
            vec![(0, "w2"), (1, "w1")]
        );
    }

    #[test]
    fn shared_title_hints_disambiguate() {
        // By title alone the "notes draft" words pull w1 towards the alpha snapshot, but w1
        // shows the beta project: the project-name hints fix the pairing.
        let saved = vec![
            snapshot("alpha notes draft — Editor", None),
            snapshot("beta — Editor", None),
        ];
        let candidates = vec![
            window("w1", "notes draft beta — Editor", None),
            window("w2", "alpha — Editor", None),
        ];
        assert_eq!(
            ids(&assign_windows(&saved, &candidates, &[])),
            vec![(0, "w1"), (1, "w2")]
        );
        let hints = vec!["Alpha".to_string(), " beta ".to_string()];
        assert_eq!(
            ids(&assign_windows(&saved, &candidates, &hints)),
            vec![(0, "w2"), (1, "w1")]
        );
    }

    #[test]
    fn hint_or_class_lift_weak_titles_above_the_threshold() {
        let saved = vec![snapshot("notes.txt - Notepad", Some("Notepad"))];
        let unrelated_title = vec![window("w1", "Untitled - Notepad", None)];
        assert!(assign_windows(&saved, &unrelated_title, &[]).is_empty());
        let same_class = vec![window("w1", "Untitled - Notepad", Some("notepad"))];
        assert_eq!(
            ids(&assign_windows(&saved, &same_class, &[])),
            vec![(0, "w1")]
        );
        let with_hint = vec![window("w1", "project-x - Notepad", None)];
        let hints = vec!["project-x".to_string()];
        assert_eq!(
            ids(&assign_windows(&saved, &with_hint, &hints)),
            vec![(0, "w1")]
        );
    }

    #[test]
    fn each_window_used_once_and_deterministic() {
        let saved = vec![snapshot("Terminal", None), snapshot("Terminal", None)];
        let candidates = vec![
            window("w1", "Terminal", None),
            window("w1", "Terminal", None), // duplicate id
            window("w2", "Terminal", None),
        ];
        let first = assign_windows(&saved, &candidates, &[]);
        assert_eq!(ids(&first), vec![(0, "w1"), (1, "w2")]);
        for _ in 0..10 {
            assert_eq!(assign_windows(&saved, &candidates, &[]), first);
        }
        assert!(assign_windows(&[], &candidates, &[]).is_empty());
        assert!(assign_windows(&saved, &[], &[]).is_empty());
    }

    #[test]
    fn fallback_pairs_leftovers_in_order() {
        let saved = vec![
            snapshot("Inbox — Mail", None),
            snapshot("Draft 1", None),
            snapshot("Draft 2", None),
        ];
        let candidates = vec![
            window("w1", "Compose", None),
            window("w2", "Inbox — Mail", None),
        ];
        let pairs = assign_windows_with_fallback(&saved, &candidates, &[]);
        assert_eq!(ids(&pairs), vec![(0, "w2"), (1, "w1")]);
    }

    #[test]
    fn running_windows_are_found_by_identity_or_executable() {
        let platform = MockPlatform::new();
        let gimp = platform.add_process("gimp", Some("/opt/gimp/bin/gimp".into()));
        let other = platform.add_process("xterm", Some("/usr/bin/xterm".into()));
        let w1 = platform.add_window(gimp, "Image 1", None, Rect::new(0, 0, 10, 10));
        let _w2 = platform.add_window(other, "Shell", None, Rect::new(0, 0, 10, 10));
        let w3 = platform.add_window(gimp, "Image 2", None, Rect::new(0, 0, 10, 10));
        let state = state(&platform);
        let adapters = generic_only();

        let entry = ApplicationEntry::new("exe:gimp", "GIMP", "generic");
        assert_eq!(
            running_windows_for_entry(&entry, &state, &adapters),
            vec![w3.clone(), w1.clone()]
        );

        // Different logical id, same executable path.
        let mut renamed = ApplicationEntry::new("exe:gimp-2.10", "GIMP", "generic");
        renamed.launch.executable = Some("/opt/gimp/bin/gimp".into());
        assert_eq!(
            running_windows_for_entry(&renamed, &state, &adapters),
            vec![w3, w1]
        );

        let missing = ApplicationEntry::new("exe:inkscape", "Inkscape", "generic");
        assert!(running_windows_for_entry(&missing, &state, &adapters).is_empty());
    }

    #[test]
    fn windows_of_unknown_processes_are_not_attributed() {
        let platform = MockPlatform::new();
        platform.add_window(4242, "Ghost", None, Rect::new(0, 0, 10, 10));
        let entry = ApplicationEntry::new("exe:ghost", "Ghost", "generic");
        assert!(running_windows_for_entry(&entry, &state(&platform), &generic_only()).is_empty());
    }

    #[test]
    fn package_identity_matches() {
        let platform = MockPlatform::new();
        let package = PackageInfo {
            kind: PackageKind::Flatpak,
            id: "org.gimp.GIMP".into(),
            path: None,
        };
        let pid = platform.add_process_info(ProcessInfo {
            pid: 0,
            parent_pid: None,
            name: "gimp".into(),
            product_name: None,
            executable_path: None,
            command_line: vec![],
            working_directory: None,
            start_time: None,
            package: Some(package.clone()),
        });
        let w = platform.add_window(pid, "GIMP", None, Rect::new(0, 0, 10, 10));
        let mut entry = ApplicationEntry::new("something-else", "GIMP", "generic");
        entry.launch.package = Some(package);
        assert_eq!(
            running_windows_for_entry(&entry, &state(&platform), &generic_only()),
            vec![w]
        );
    }

    #[test]
    fn executable_comparison() {
        assert!(same_executable(
            Path::new("/usr/bin/a"),
            Path::new("/usr/bin/a")
        ));
        assert!(!same_executable(
            Path::new("/usr/bin/a"),
            Path::new("/usr/bin/b")
        ));
        assert!(!same_executable(Path::new(""), Path::new("")));
        #[cfg(unix)]
        {
            let dir = tempfile::tempdir().unwrap();
            let real = dir.path().join("real");
            std::fs::write(&real, "x").unwrap();
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            assert!(same_executable(&link, &real));
        }
        #[cfg(windows)]
        assert!(same_executable(
            Path::new("C:/Program Files/App/App.EXE"),
            Path::new("c:\\program files\\app\\app.exe")
        ));
    }
}
