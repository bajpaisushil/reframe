//! Small helpers shared across the engine and adapters.

use std::path::{Path, PathBuf};

/// Lower-cased file stem of an executable path or name: `C:\..\Code.exe` → `code`.
pub fn exe_stem(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    stem.to_lowercase()
}

/// Lower-cased file name without a trailing `.exe`.
pub fn normalized_process_name(name: &str) -> String {
    let lower = name.to_lowercase();
    lower
        .strip_suffix(".exe")
        .map(str::to_string)
        .unwrap_or(lower)
}

/// `gnome-calculator` → `Gnome Calculator`, `WINWORD` → `Winword`.
pub fn prettify_name(raw: &str) -> String {
    let cleaned = raw
        .trim()
        .trim_end_matches(".exe")
        .trim_end_matches(".EXE")
        .trim_end_matches(".app");
    cleaned
        .split(|c: char| c == '-' || c == '_' || c == '.' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Lower-cased alphanumeric tokens of a window title (used for fuzzy matching).
pub fn title_tokens(title: &str) -> Vec<String> {
    title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 1)
        .map(str::to_lowercase)
        .collect()
}

/// Jaccard similarity of title tokens in `[0, 1]`.
pub fn title_similarity(a: &str, b: &str) -> f64 {
    let ta: std::collections::HashSet<_> = title_tokens(a).into_iter().collect();
    let tb: std::collections::HashSet<_> = title_tokens(b).into_iter().collect();
    if ta.is_empty() && tb.is_empty() {
        return if a.trim() == b.trim() { 1.0 } else { 0.0 };
    }
    let inter = ta.intersection(&tb).count() as f64;
    let union = ta.union(&tb).count() as f64;
    if union == 0.0 { 0.0 } else { inter / union }
}

/// Positional command-line arguments that are existing files or folders (documents the app
/// was opened with). Flags and anything that is not an existing path are dropped, because
/// captured command lines can contain secrets.
pub fn document_arguments(command_line: &[String], limit: usize) -> Vec<PathBuf> {
    command_line
        .iter()
        .skip(1)
        .filter(|arg| !arg.starts_with('-'))
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.exists())
        .take(limit)
        .collect()
}

/// Whether two paths name the same file for de-duplication. Windows and macOS (by default)
/// have case-insensitive file systems, so letter case is ignored there.
pub fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// Display a path's final component (`/home/a/code/reframe` → `reframe`).
pub fn path_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(
            exe_stem(Path::new("/usr/bin/gnome-terminal-server")),
            "gnome-terminal-server"
        );
        assert_eq!(normalized_process_name("Code.exe"), "code");
        assert_eq!(prettify_name("gnome-calculator"), "Gnome Calculator");
        assert_eq!(prettify_name("WINWORD.EXE"), "Winword");
    }

    #[test]
    fn similarity() {
        assert!(
            title_similarity(
                "main.rs — reframe — Visual Studio Code",
                "lib.rs — reframe — Visual Studio Code"
            ) > 0.5
        );
        assert!(title_similarity("Inbox - Mail", "reframe — Code") < 0.2);
    }

    #[test]
    fn document_arguments_keep_existing_paths_only() {
        let dir = std::env::temp_dir();
        let file = dir.join("reframe-doc-arg-test.txt");
        std::fs::write(&file, "x").unwrap();
        let args = vec![
            "editor".to_string(),
            "--token=secret".to_string(),
            file.display().to_string(),
            "/definitely/missing/file".to_string(),
            "relative.txt".to_string(),
        ];
        assert_eq!(document_arguments(&args, 10), vec![file.clone()]);
        let _ = std::fs::remove_file(file);
    }
}
