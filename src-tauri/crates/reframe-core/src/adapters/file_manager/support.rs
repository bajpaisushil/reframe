//! Helpers shared by the desktop adapters: file managers (the parent module), PDF readers
//! ([`crate::adapters::pdf`]) and music players ([`crate::adapters::music`]).
//!
//! - [`KnownApp`]: a declarative description of one supported application: how to recognise
//!   its processes and windows, and how to find it on a machine.
//! - [`run_with_timeout`] / [`command_output`]: OS queries that may block (COM calls into an
//!   unresponsive Explorer, `osascript` waiting for a permission prompt, `gdbus`, a stale network
//!   mount) run on a helper thread and are abandoned after a timeout, so capture never hangs.
//!   Abandoned helpers are never killed; they finish on their own and their result is discarded.
//!
//! The module lives under the file-manager adapter only so that the shared adapter registry
//! module stays untouched; nothing in it is file-manager specific.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use reframe_platform::{
    LocateQuery, PackageKind, PlatformInfo, ProcessInfo, SupportLevel, WindowInfo,
};

use crate::adapters::{AppIdentity, CaptureInput};
use crate::model::{AdapterCapability, LaunchConfig};
use crate::util;

/// Static description of an application handled by a desktop adapter.
///
/// Matching rules (any one is enough): the process's package id, its executable stem or process
/// name, a trailing executable path, or a window class / application-id hint of one of its
/// windows. Names that are too generic to match on their own (`Finder`, `Preview`, `Music`) are
/// only listed as bundle ids and trailing bundle paths.
#[derive(Debug, Clone, Copy)]
pub(in crate::adapters) struct KnownApp {
    /// Logical application id stored in workspaces ("nautilus", "spotify").
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// Other display names (used to locate the application in Start menus and app folders).
    pub aliases: &'static [&'static str],
    /// Lower-case executable stems / process names, without `.exe`.
    pub executables: &'static [&'static str],
    /// Lower-case stem prefixes for builds whose file name carries a version
    /// (`SumatraPDF-3.5.2-64.exe`).
    pub executable_prefixes: &'static [&'static str],
    /// Trailing executable path components (`Finder.app/Contents/MacOS/Finder`).
    pub executable_paths: &'static [&'static str],
    /// Exact executable file names used to locate an installation.
    pub locate_names: &'static [&'static str],
    /// macOS bundle identifiers (macOS also reports them as window class).
    pub bundle_ids: &'static [&'static str],
    pub flatpak_ids: &'static [&'static str],
    pub snap_names: &'static [&'static str],
    /// Windows package family-name prefixes (`SpotifyAB.SpotifyMusic`).
    pub windows_packages: &'static [&'static str],
    /// X11 `WM_CLASS` parts, GTK application ids and distinctive Win32 window classes
    /// (compared case-insensitively).
    pub window_classes: &'static [&'static str],
    /// Linux desktop entry ids.
    pub desktop_ids: &'static [&'static str],
}

impl KnownApp {
    pub const fn new(id: &'static str, name: &'static str) -> Self {
        Self {
            id,
            name,
            aliases: &[],
            executables: &[],
            executable_prefixes: &[],
            executable_paths: &[],
            locate_names: &[],
            bundle_ids: &[],
            flatpak_ids: &[],
            snap_names: &[],
            windows_packages: &[],
            window_classes: &[],
            desktop_ids: &[],
        }
    }

    pub const fn aliases(mut self, values: &'static [&'static str]) -> Self {
        self.aliases = values;
        self
    }

    pub const fn executables(mut self, values: &'static [&'static str]) -> Self {
        self.executables = values;
        self
    }

    pub const fn executable_prefixes(mut self, values: &'static [&'static str]) -> Self {
        self.executable_prefixes = values;
        self
    }

    pub const fn executable_paths(mut self, values: &'static [&'static str]) -> Self {
        self.executable_paths = values;
        self
    }

    pub const fn locate_names(mut self, values: &'static [&'static str]) -> Self {
        self.locate_names = values;
        self
    }

    pub const fn bundle_ids(mut self, values: &'static [&'static str]) -> Self {
        self.bundle_ids = values;
        self
    }

    pub const fn flatpak_ids(mut self, values: &'static [&'static str]) -> Self {
        self.flatpak_ids = values;
        self
    }

    pub const fn snap_names(mut self, values: &'static [&'static str]) -> Self {
        self.snap_names = values;
        self
    }

    pub const fn windows_packages(mut self, values: &'static [&'static str]) -> Self {
        self.windows_packages = values;
        self
    }

    pub const fn window_classes(mut self, values: &'static [&'static str]) -> Self {
        self.window_classes = values;
        self
    }

    pub const fn desktop_ids(mut self, values: &'static [&'static str]) -> Self {
        self.desktop_ids = values;
        self
    }

    /// Whether `process` (owning `windows`) is this application.
    pub fn matches(&self, process: &ProcessInfo, windows: &[&WindowInfo]) -> bool {
        self.matches_package(process)
            || self.matches_executable(process)
            || self.matches_windows(windows)
    }

    fn matches_package(&self, process: &ProcessInfo) -> bool {
        let Some(package) = &process.package else {
            return false;
        };
        let known: &[&str] = match package.kind {
            PackageKind::MacBundle => self.bundle_ids,
            PackageKind::Flatpak => self.flatpak_ids,
            PackageKind::Snap => self.snap_names,
            PackageKind::WindowsPackage => self.windows_packages,
            PackageKind::AppImage => &[],
        };
        known.iter().any(|k| package_id_matches(&package.id, k))
    }

    fn matches_executable(&self, process: &ProcessInfo) -> bool {
        let exe = process.executable_path.as_deref();
        if let Some(exe) = exe
            && self.executable_paths.iter().any(|tail| exe.ends_with(tail))
        {
            return true;
        }
        exe.map(executable_stem)
            .into_iter()
            .chain(std::iter::once(util::normalized_process_name(
                &process.name,
            )))
            .filter(|stem| !stem.is_empty())
            .any(|stem| {
                self.executables.contains(&stem.as_str())
                    || self.executable_prefixes.iter().any(|p| stem.starts_with(p))
            })
    }

    fn matches_windows(&self, windows: &[&WindowInfo]) -> bool {
        windows.iter().any(|w| {
            [w.class_name.as_deref(), w.app_id_hint.as_deref()]
                .into_iter()
                .flatten()
                .any(|class| {
                    self.window_classes
                        .iter()
                        .chain(self.bundle_ids)
                        .any(|known| known.eq_ignore_ascii_case(class))
                })
        })
    }

    pub fn identity(&self) -> AppIdentity {
        AppIdentity {
            app_id: self.id.to_string(),
            display_name: self.name.to_string(),
        }
    }

    /// Hints for finding this application on a machine.
    pub fn locate_query(&self) -> LocateQuery {
        let owned = |values: &[&str]| values.iter().map(|v| v.to_string()).collect::<Vec<_>>();
        LocateQuery {
            executable_names: owned(self.locate_names),
            bundle_ids: owned(self.bundle_ids),
            desktop_ids: owned(self.desktop_ids),
            display_names: std::iter::once(self.name)
                .chain(self.aliases.iter().copied())
                .map(str::to_string)
                .collect(),
        }
    }
}

/// Package ids compare case-insensitively; Windows package ids (`Family_PublisherId!App`)
/// match their family-name prefix.
fn package_id_matches(id: &str, known: &str) -> bool {
    let id = id.to_ascii_lowercase();
    let known = known.to_ascii_lowercase();
    id == known
        || id
            .strip_prefix(&known)
            .is_some_and(|rest| rest.starts_with('_') || rest.starts_with('!'))
}

/// The first application in `apps` that `process` belongs to.
pub(in crate::adapters) fn find_app<'a>(
    apps: &'a [KnownApp],
    process: &ProcessInfo,
    windows: &[&WindowInfo],
) -> Option<&'a KnownApp> {
    apps.iter().find(|app| app.matches(process, windows))
}

pub(in crate::adapters) fn app_by_id<'a>(apps: &'a [KnownApp], id: &str) -> Option<&'a KnownApp> {
    apps.iter().find(|app| app.id == id)
}

/// Launch configuration of a captured application: executable and package of its main process.
/// Captured command-line arguments are never stored (they can contain secrets). Also returns a
/// warning when the application could not be relaunched from this information.
pub(in crate::adapters) fn launch_config(input: &CaptureInput<'_>) -> (LaunchConfig, Vec<String>) {
    let name = &input.identity.display_name;
    let Some(main) = input.main_process() else {
        return (
            LaunchConfig::default(),
            vec![format!("{name}: no process information available.")],
        );
    };
    let mut warnings = Vec::new();
    if main.executable_path.is_none() && main.package.is_none() {
        warnings.push(format!(
            "{name}: the executable path is not accessible; choose it in the workspace before restoring."
        ));
    }
    let config = LaunchConfig {
        executable: main.executable_path.clone(),
        package: main.package.clone(),
        arguments: Vec::new(),
        working_directory: None,
    };
    (config, warnings)
}

/// Add the "Window position and size" feature as the platform reports it.
pub(in crate::adapters) fn with_window_placement(
    capability: AdapterCapability,
    info: &PlatformInfo,
) -> AdapterCapability {
    let placement = info.capabilities.move_windows.level != SupportLevel::Unsupported;
    capability.feature(
        "Window position and size",
        placement,
        info.capabilities
            .move_windows
            .note
            .as_deref()
            .filter(|_| !placement),
    )
}

/// `a, b, c, +2 more`.
pub(in crate::adapters) fn summarize(
    items: impl IntoIterator<Item = String>,
    max: usize,
) -> String {
    let items: Vec<String> = items.into_iter().collect();
    let shown = items
        .iter()
        .take(max)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if items.len() > max {
        format!("{shown}, +{} more", items.len() - max)
    } else {
        shown
    }
}

/// Final component of a path written for any OS (`C:\Users\me` → `me`, `/home/a/b` → `b`),
/// so workspaces imported from another OS still display sensibly.
pub(in crate::adapters) fn display_name(path: &Path) -> String {
    let text = path.to_string_lossy();
    text.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| text.into_owned())
}

/// Lower-cased file stem of an executable path written for any OS
/// (`C:\Tools\SumatraPDF-3.5.exe` → `sumatrapdf-3.5`, `/usr/bin/nautilus` → `nautilus`).
pub(in crate::adapters) fn executable_stem(path: &Path) -> String {
    util::exe_stem(Path::new(&display_name(path)))
}

/// Run `f` on a helper thread and wait at most `timeout` for its result.
///
/// Returns `None` when it timed out, panicked or the thread could not be started. A timed-out
/// helper keeps running in the background; its result is discarded.
pub(in crate::adapters) fn run_with_timeout<T, F>(label: &str, timeout: Duration, f: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (sender, receiver) = mpsc::sync_channel(1);
    let spawned = std::thread::Builder::new()
        .name(format!("reframe-{label}"))
        .spawn(move || {
            // After a timeout nobody listens any more; dropping the result is intended.
            let _ = sender.send(f());
        });
    if let Err(error) = spawned {
        log::warn!("Could not start helper thread {label}: {error}");
        return None;
    }
    match receiver.recv_timeout(timeout) {
        Ok(value) => Some(value),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            log::warn!("{label} did not finish within {timeout:?}; continuing without it");
            None
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            log::warn!("{label} failed unexpectedly");
            None
        }
    }
}

/// Why an external helper program produced no output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::adapters) enum HelperError {
    /// The program could not be started (not installed, not permitted, wrong OS).
    Unavailable(String),
    /// It did not finish within the timeout. It is left running, never killed.
    TimedOut,
    /// It ran and reported an error (first line of its error output).
    Failed(String),
}

impl std::fmt::Display for HelperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HelperError::Unavailable(reason) => write!(f, "not available: {reason}"),
            HelperError::TimedOut => f.write_str("no answer in time"),
            HelperError::Failed(reason) => f.write_str(reason),
        }
    }
}

/// Helpers used here print a few lines; anything beyond this is cut off.
const MAX_HELPER_OUTPUT: usize = 1 << 20;

/// Run `program` with `args` (no shell involved) and return its standard output, waiting at
/// most `timeout`.
pub(in crate::adapters) fn command_output(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<String, HelperError> {
    let label = Path::new(program)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| program.to_string());
    let owned_program = program.to_string();
    let owned_args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let output = run_with_timeout(&label, timeout, move || {
        Command::new(&owned_program)
            .args(&owned_args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
    })
    .ok_or(HelperError::TimedOut)?
    .map_err(|e| HelperError::Unavailable(format!("{label}: {e}")))?;

    if output.status.success() {
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        if text.len() > MAX_HELPER_OUTPUT {
            let mut cut = MAX_HELPER_OUTPUT;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
        }
        Ok(text)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let first_line: String = stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or_default()
            .chars()
            .take(300)
            .collect();
        Err(HelperError::Failed(if first_line.is_empty() {
            format!("{label} exited with {}", output.status)
        } else {
            first_line
        }))
    }
}

/// Run an AppleScript through `/usr/bin/osascript` (macOS only). The first time Reframe talks
/// to an application macOS may show the Automation permission prompt; callers time out instead
/// of waiting for the user.
pub(in crate::adapters) fn run_applescript(
    script: &str,
    timeout: Duration,
) -> Result<String, HelperError> {
    if !cfg!(target_os = "macos") {
        return Err(HelperError::Unavailable(
            "AppleScript is only available on macOS".to_string(),
        ));
    }
    command_output("/usr/bin/osascript", &["-e", script], timeout)
}

/// Split AppleScript output built with `character id 30` between records and `character id 31`
/// between fields (separators that cannot appear in names or paths typed by users).
pub(in crate::adapters) fn applescript_records(raw: &str) -> Vec<Vec<String>> {
    raw.split('\u{1e}')
        .map(|record| record.trim_matches(['\r', '\n']))
        .filter(|record| !record.trim().is_empty())
        .map(|record| record.split('\u{1f}').map(str::to_string).collect())
        .collect()
}

/// Builders shared by the desktop adapters' tests.
#[cfg(test)]
pub(in crate::adapters) mod testing {
    use reframe_platform::mock::MockPlatform;
    use reframe_platform::{
        OsKind, Platform, PlatformInfo, ProcessInfo, Rect, WindowId, WindowInfo, WindowState,
    };

    pub fn process(pid: u32, name: &str, exe: Option<&str>) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid: None,
            name: name.to_string(),
            product_name: None,
            executable_path: exe.map(Into::into),
            command_line: Vec::new(),
            working_directory: None,
            start_time: None,
            package: None,
        }
    }

    pub fn window(id: &str, pid: u32, title: &str, class: Option<&str>) -> WindowInfo {
        WindowInfo {
            id: WindowId::new(id),
            pid,
            title: title.to_string(),
            class_name: class.map(str::to_string),
            app_id_hint: None,
            bounds: Rect::new(0, 0, 100, 100),
            normal_bounds: None,
            state: WindowState::Normal,
            monitor_id: None,
            desktop: None,
            is_focused: false,
            z_order: 0,
        }
    }

    /// A mock platform reporting `os`.
    pub fn platform(os: OsKind) -> (MockPlatform, PlatformInfo) {
        let platform = MockPlatform::new();
        let mut info = platform.info();
        info.os = os;
        platform.set_info(info.clone());
        (platform, info)
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{process as proc, window as win};
    use super::*;
    use reframe_platform::PackageInfo;

    const APP: KnownApp = KnownApp::new("demo", "Demo")
        .executables(&["demo"])
        .executable_prefixes(&["demo-portable"])
        .executable_paths(&["Demo.app/Contents/MacOS/Demo"])
        .bundle_ids(&["com.example.demo"])
        .flatpak_ids(&["org.example.Demo"])
        .windows_packages(&["Example.Demo"])
        .window_classes(&["demo-window"]);

    fn process(name: &str, exe: Option<&str>) -> ProcessInfo {
        proc(7, name, exe)
    }

    fn window(title: &str, class: Option<&str>) -> WindowInfo {
        win("0x10", 7, title, class)
    }

    #[test]
    fn matches_by_executable_name_prefix_and_path() {
        assert!(APP.matches(&process("x", Some("/usr/bin/demo")), &[]));
        assert!(APP.matches(&process("Demo.exe", None), &[]));
        assert!(APP.matches(&process("x", Some(r"C:\Tools\demo-portable-1.2.exe")), &[]));
        assert!(APP.matches(
            &process("x", Some("/Applications/Demo.app/Contents/MacOS/Demo")),
            &[]
        ));
        assert!(!APP.matches(&process("demonic", Some("/usr/bin/demonic")), &[]));
    }

    #[test]
    fn matches_by_package_and_window_class() {
        let mut p = process("x", None);
        p.package = Some(PackageInfo {
            kind: PackageKind::WindowsPackage,
            id: "Example.Demo_abc123!App".into(),
            path: None,
        });
        assert!(APP.matches(&p, &[]));
        p.package = Some(PackageInfo {
            kind: PackageKind::WindowsPackage,
            id: "Example.DemoPlus_abc!App".into(),
            path: None,
        });
        assert!(!APP.matches(&p, &[]));
        p.package = Some(PackageInfo {
            kind: PackageKind::Flatpak,
            id: "org.example.demo".into(),
            path: None,
        });
        assert!(APP.matches(&p, &[]));

        let other = process("x", None);
        let w = window("t", Some("DEMO-WINDOW"));
        assert!(APP.matches(&other, &[&w]));
        let mac = window("t", Some("com.example.demo"));
        assert!(APP.matches(&other, &[&mac]));
        let unrelated = window("t", Some("other"));
        assert!(!APP.matches(&other, &[&unrelated]));
    }

    #[test]
    fn locate_query_lists_every_hint() {
        let q = APP.locate_query();
        assert_eq!(q.bundle_ids, vec!["com.example.demo".to_string()]);
        assert_eq!(q.display_names, vec!["Demo".to_string()]);
    }

    #[test]
    fn summarize_and_display_names() {
        let items = ["a", "b", "c", "d", "e"].map(String::from);
        assert_eq!(summarize(items.clone(), 3), "a, b, c, +2 more");
        assert_eq!(summarize(items, 5), "a, b, c, d, e");
        assert_eq!(
            display_name(Path::new(r"C:\Users\me\Downloads")),
            "Downloads"
        );
        assert_eq!(display_name(Path::new("/home/me/code/")), "code");
        assert_eq!(display_name(Path::new(r"C:\")), "C:");
        assert_eq!(display_name(Path::new("/")), "/");
    }

    #[test]
    fn applescript_records_split_on_control_characters() {
        let raw = "Downloads\u{1f}/Users/me/Downloads/\u{1e}a, b\u{1f}/tmp/a, b/\u{1e}\n";
        assert_eq!(
            applescript_records(raw),
            vec![
                vec!["Downloads".to_string(), "/Users/me/Downloads/".to_string()],
                vec!["a, b".to_string(), "/tmp/a, b/".to_string()],
            ]
        );
        assert!(applescript_records("\n").is_empty());
    }

    #[test]
    fn run_with_timeout_returns_value_or_gives_up() {
        assert_eq!(
            run_with_timeout("t1", Duration::from_secs(5), || 42),
            Some(42)
        );
        let start = std::time::Instant::now();
        let slow = run_with_timeout("t2", Duration::from_millis(50), || {
            std::thread::sleep(Duration::from_secs(2));
            1
        });
        assert_eq!(slow, None);
        assert!(start.elapsed() < Duration::from_secs(1));
        let panicking: Option<u8> =
            run_with_timeout("t3", Duration::from_secs(5), || panic!("helper failure"));
        assert_eq!(panicking, None);
    }

    #[cfg(unix)]
    #[test]
    fn command_output_reports_success_failure_and_timeouts() {
        let out = command_output("sh", &["-c", "printf 'hello'"], Duration::from_secs(10));
        assert_eq!(out, Ok("hello".to_string()));

        let failed = command_output(
            "sh",
            &["-c", "echo 'first line' >&2; echo second >&2; exit 3"],
            Duration::from_secs(10),
        );
        assert_eq!(failed, Err(HelperError::Failed("first line".to_string())));

        let missing = command_output("/definitely/not/a/program", &[], Duration::from_secs(10));
        assert!(matches!(missing, Err(HelperError::Unavailable(_))));

        let start = std::time::Instant::now();
        let slow = command_output("sleep", &["3"], Duration::from_millis(100));
        assert_eq!(slow, Err(HelperError::TimedOut));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn applescript_is_refused_off_macos() {
        if !cfg!(target_os = "macos") {
            assert!(matches!(
                run_applescript("return 1", Duration::from_secs(1)),
                Err(HelperError::Unavailable(_))
            ));
        }
    }
}
