//! The user's "never suggest" list (`AppSettings::excluded_apps`).

use reframe_platform::{OsKind, ProcessInfo};

use super::names;

/// Case-insensitive exclusion entries. An application is excluded when an entry equals its app
/// id, or the executable stem / file name / full path, process name or package id of one of its
/// processes. Entries written as app ids (`exe:slack`, `pkg:com.spotify.client`) also match the
/// bare stem or package id, so `exe:slack` excludes Slack even when a dedicated adapter gives it a
/// different app id.
#[derive(Debug, Clone, Default)]
pub(crate) struct Exclusions {
    entries: Vec<String>,
}

impl Exclusions {
    pub fn new(list: &[String]) -> Self {
        let mut entries: Vec<String> = Vec::new();
        for entry in list {
            let entry = entry.trim().to_lowercase();
            if !entry.is_empty() && !entries.contains(&entry) {
                entries.push(entry);
            }
        }
        Self { entries }
    }

    pub fn matches(&self, os: OsKind, app_id: &str, processes: &[&ProcessInfo]) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        let mut candidates: Vec<String> = vec![app_id.trim().to_lowercase()];
        for process in processes {
            candidates.push(process.name.trim().to_lowercase());
            candidates.push(names::process_name(&process.name));
            if let Some(exe) = process.executable_path.as_deref() {
                candidates.push(names::exe_stem(exe, os));
                candidates.push(names::file_name(exe, os).to_lowercase());
                candidates.push(exe.to_string_lossy().trim().to_lowercase());
            }
            if let Some(package) = &process.package {
                candidates.extend(names::package_names(package));
            }
        }
        candidates.retain(|c| !c.is_empty());
        self.entries.iter().any(|entry| {
            let bare = entry
                .strip_prefix("exe:")
                .or_else(|| entry.strip_prefix("pkg:"))
                .unwrap_or(entry);
            candidates.iter().any(|c| c == entry || c == bare)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reframe_platform::{PackageInfo, PackageKind};

    fn process(name: &str, exe: Option<&str>) -> ProcessInfo {
        ProcessInfo {
            pid: 1,
            parent_pid: None,
            name: name.into(),
            product_name: None,
            executable_path: exe.map(Into::into),
            command_line: Vec::new(),
            working_directory: None,
            start_time: None,
            package: None,
        }
    }

    fn list(entries: &[&str]) -> Exclusions {
        Exclusions::new(&entries.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn matches_app_id_stem_name_and_path_case_insensitively() {
        let slack = process(
            "Slack.exe",
            Some(r"C:\Users\a\AppData\Local\slack\slack.exe"),
        );
        let os = OsKind::Windows;
        assert!(list(&["EXE:SLACK"]).matches(os, "exe:slack", &[&slack]));
        assert!(list(&["slack"]).matches(os, "slack-adapter-id", &[&slack]));
        assert!(list(&["SLACK.EXE"]).matches(os, "x", &[&slack]));
        assert!(list(&[r"c:\users\a\appdata\local\slack\slack.exe"]).matches(os, "x", &[&slack]));
        // A dedicated adapter's app id still matches an `exe:` entry through the stem.
        assert!(list(&["exe:slack"]).matches(os, "slack", &[&slack]));
        assert!(!list(&["teams", " "]).matches(os, "exe:slack", &[&slack]));
        assert!(!list(&[]).matches(os, "exe:slack", &[&slack]));
    }

    #[test]
    fn matches_package_ids() {
        let mut spotify = process("spotify", Some("/app/extra/share/spotify/spotify"));
        spotify.package = Some(PackageInfo {
            kind: PackageKind::Flatpak,
            id: "com.spotify.Client".into(),
            path: None,
        });
        let os = OsKind::Linux;
        assert!(list(&["pkg:com.spotify.client"]).matches(os, "music", &[&spotify]));
        assert!(list(&["com.spotify.Client"]).matches(os, "music", &[&spotify]));
        assert!(list(&["Spotify"]).matches(os, "music", &[&spotify]));
    }

    #[test]
    fn any_process_of_the_application_counts() {
        let main = process("code", Some("/usr/share/code/code"));
        let helper = process("code-tunnel", Some("/usr/share/code/bin/code-tunnel"));
        assert!(list(&["code-tunnel"]).matches(OsKind::Linux, "vscode", &[&main, &helper]));
    }
}
