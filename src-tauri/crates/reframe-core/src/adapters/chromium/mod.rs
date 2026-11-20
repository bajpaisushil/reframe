//! Chromium-based browsers (Google Chrome, Microsoft Edge, Brave, Chromium): windows and tabs
//! from the browser's own session files.
//!
//! # How it works
//!
//! - **Identify**: the browser *main* process, by executable name and install folder (all
//!   channels: Beta, Dev, Canary/Nightly), Flatpak id, Snap name or macOS bundle id. Child
//!   processes, Electron apps and WebView hosts are never claimed (see [`browser`]).
//! - **Capture** (only when the user enabled "Read browser tabs"): find the profile's newest
//!   plain session file (`<user data>/<profile>/Sessions/Session_<n>`, or `Current Session` on
//!   older browsers), replay it ([`snss`]) and pair its normal windows with the windows the
//!   user selected ([`matching`]). Candidate profiles are the one from
//!   `--profile-directory=` (in a `--user-data-dir=` when given), those recorded in
//!   `Local State` (last used, active, known) and `Default`; the one whose session explains the
//!   selected windows best wins, ties going to the most likely one ([`profile`]).
//!   Only http(s), file and ftp tabs are kept ([`urls`]). Whatever goes wrong — setting off,
//!   no profile folder, no or unreadable session file — the capture still succeeds with the
//!   windows (no tabs) and a warning.
//! - **Restore**: one `--new-window <urls...>` launch per saved window (plus
//!   `--profile-directory=` for the saved profile when it exists on this machine), split into
//!   more windows when a window has too many tabs for one command line ([`launch`]). URLs
//!   follow a `--` separator so no saved address can ever be read as a browser switch. With
//!   nothing restorable, the browser is simply started (it may restore its own session).
//!
//! # Limitations
//!
//! - Pinned tabs are reopened as normal tabs, tab groups are not recreated, and only the
//!   current page of each tab is reopened (no back/forward history): browsers offer no
//!   command-line options for these.
//! - Only one profile per entry: windows of other profiles (all profiles share one browser
//!   process) are kept without tabs, with a warning. Incognito and guest windows are never
//!   recorded by the browser, so they are never captured.
//! - Browsers save their session a few seconds after a change; a tab opened just before the
//!   capture may be missing.
//! - A current session file that cannot be opened is reported, never replaced by an older file
//!   or another profile's session (those describe tabs that may no longer be open). On Windows
//!   a running browser may keep its current session file locked; its tabs then cannot be read.
//! - Browsers that write only encrypted session files (`Sessions_Encrypted/`, enabled in newer
//!   Chromium versions) cannot be read; the user gets a warning explaining it.
//! - If the browser is not running and is set to "Continue where you left off", it may also
//!   restore its own previous session when Reframe starts it.
//! - On macOS the launch goes through the app bundle; passing the URLs to an already running
//!   browser relies on the platform starting a new instance with the arguments (Chromium
//!   forwards them to the running browser and exits).

mod browser;
mod launch;
mod matching;
mod profile;
mod snss;
#[cfg(test)]
mod test_support;
mod urls;

use std::path::{Path, PathBuf};
use std::time::Duration;

use reframe_platform::{
    LaunchRequest, LocateQuery, OsKind, PackageInfo, PackageKind, PlatformInfo, ProcessInfo,
    SupportLevel, WindowInfo,
};

use self::browser::{BaseDirs, Browser, Channel, Install, InstallKind};
use self::matching::OsWindow;
use self::profile::DEFAULT_PROFILE;
use self::snss::{Session, SessionWindow, SnssError};
use super::generic::{self, GenericAdapter};
use super::{
    AppAdapter, AppIdentity, CaptureContext, CaptureInput, CaptureOutput, InstanceModel,
    LaunchContext, LaunchStep,
};
use crate::model::{
    AdapterCapability, AppState, ApplicationEntry, BrowserTab, BrowserWindow, CapabilityLevel,
    LaunchConfig, StateDetail,
};

/// Environment variable naming a user data directory to read instead of each browser's
/// default one (for testing and unusual setups). Ignored when unset or empty; a directory
/// given to [`ChromiumAdapter::with_user_data_dir`] takes precedence.
pub const USER_DATA_ENV: &str = "REFRAME_CHROMIUM_USER_DATA";

/// Session files tried per profile, newest first, before giving up on the profile.
const MAX_SESSION_FILES_TRIED: usize = 3;

/// A plain session file this much older than the profile's newest encrypted one is a leftover
/// from before the browser stopped writing plain files.
const STALE_PLAIN_SESSION: Duration = Duration::from_secs(10 * 60);

/// Tab titles listed by [`AppAdapter::describe`].
const DESCRIBED_TABS: usize = 3;

#[derive(Debug, Default)]
pub struct ChromiumAdapter {
    generic: GenericAdapter,
    /// Overrides every browser's user data directory.
    user_data_dir: Option<PathBuf>,
}

impl ChromiumAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read every browser's profiles from `dir` instead of its default user data directory.
    pub fn with_user_data_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            user_data_dir: Some(dir.into()),
            ..Self::default()
        }
    }

    fn user_data_override(&self) -> Option<PathBuf> {
        self.user_data_dir.clone().or_else(|| {
            std::env::var_os(USER_DATA_ENV)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        })
    }

    /// Windows and tabs of the selected browser windows.
    fn capture_tabs(
        &self,
        ctx: &CaptureContext<'_>,
        input: &CaptureInput<'_>,
        found: &Found,
    ) -> TabCapture {
        let name = input.identity.display_name.as_str();
        let without_tabs = |reason: String| TabCapture {
            state: windows_without_tabs(&input.windows, found.command_line_profile.as_deref()),
            warnings: vec![reason],
        };
        if !ctx.browser_tab_capture {
            return without_tabs(format!(
                "{name}: tabs were not captured because \"Read browser tabs\" is turned off in \
                 Settings. Only the windows were saved; the browser may reopen its own last \
                 session."
            ));
        }
        let Some(install) = &found.install else {
            return without_tabs(format!(
                "{name}: could not tell which browser this is, so its tabs were not captured."
            ));
        };

        let candidates: Vec<PathBuf> = match self.user_data_override() {
            Some(dir) => vec![dir],
            None => found
                .command_line_user_data
                .iter()
                .chain(&found.default_user_data)
                .cloned()
                .collect(),
        };
        let Some(user_data) = candidates.iter().find(|dir| dir.is_dir()) else {
            let looked = candidates
                .first()
                .map(|dir| format!(" (looked in {})", dir.display()))
                .unwrap_or_default();
            return without_tabs(format!(
                "{name}: the browser's profile folder was not found{looked}, so its tabs were \
                 not captured."
            ));
        };

        let local_state = profile::read_local_state(user_data);
        let profiles: Vec<String> =
            profile::candidate_profiles(found.command_line_profile.as_deref(), &local_state)
                .into_iter()
                .filter(|p| user_data.join(p).is_dir())
                .collect();
        if profiles.is_empty() {
            return without_tabs(format!(
                "{name}: no browser profile was found in {}, so its tabs were not captured.",
                user_data.display()
            ));
        }

        let os_windows: Vec<OsWindow<'_>> = input
            .windows
            .iter()
            .map(|w| OsWindow {
                title: &w.title,
                bounds: w.restore_bounds(),
                state: w.state,
            })
            .collect();

        let mut best: Option<(String, Session, matching::Matching)> = None;
        let mut encrypted = false;
        let mut problems = Vec::new();
        let mut inaccessible: Option<String> = None;
        for profile_name in profiles {
            match load_session(&user_data.join(&profile_name)) {
                SessionLoad::Loaded(session) => {
                    let normal: Vec<&SessionWindow> = session.normal_windows().collect();
                    let matched =
                        matching::match_windows(&normal, session.active_window, &os_windows);
                    if best
                        .as_ref()
                        .is_none_or(|(_, _, b)| matched.score > b.score)
                    {
                        best = Some((profile_name, session, matched));
                    }
                }
                SessionLoad::Encrypted => encrypted = true,
                SessionLoad::Inaccessible(problem) => {
                    inaccessible.get_or_insert(problem);
                }
                SessionLoad::Unreadable(problem) => problems.push(problem),
                SessionLoad::Missing => {}
            }
        }

        // A profile whose session cannot be opened is most likely the one in use (its file
        // held open by the browser); the readable sessions of other profiles are then the last
        // sessions of profiles that are not open, which could still match by window geometry.
        if let Some(problem) = inaccessible {
            return without_tabs(format!(
                "{name}: the browser's current session file could not be opened ({problem}), \
                 so its tabs were not captured. A running browser may keep this file locked. \
                 Only the windows were saved."
            ));
        }

        let Some((profile_name, session, matched)) = best else {
            let reason = if encrypted {
                "this browser version encrypts its saved session, so Reframe cannot read the \
                 open tabs"
                    .to_string()
            } else if let Some(problem) = problems.first() {
                format!("the browser's session could not be read ({problem})")
            } else {
                "the browser has not saved a session yet".to_string()
            };
            return without_tabs(format!("{name}: {reason}. Only the windows were saved."));
        };

        let normal: Vec<&SessionWindow> = session.normal_windows().collect();
        let mut warnings = Vec::new();
        if input.windows.is_empty() {
            // Nothing to pair with: keep every regular window of the session.
            let windows = normal
                .iter()
                .map(|w| browser_window(w, Some(expected_window_title(w, install, found.os))))
                .collect();
            session_warnings(name, &session, &mut warnings);
            return TabCapture {
                state: AppState::Browser {
                    windows,
                    profile: Some(profile_name),
                },
                warnings,
            };
        }

        let windows = input
            .windows
            .iter()
            .zip(&matched.assignments)
            .map(|(os_window, assignment)| match assignment {
                Some(index) => browser_window(normal[*index], Some(os_window.title.clone())),
                None => window_without_tabs(os_window),
            })
            .collect();
        let profile = if matched.matched() == 0 {
            warnings.push(format!(
                "{name}: the selected windows could not be matched to the browser's saved \
                 session, so their tabs were not captured. The browser saves its session a few \
                 seconds after changes; try capturing again."
            ));
            found.command_line_profile.clone()
        } else {
            let unmatched = input.windows.len() - matched.matched();
            if unmatched > 0 {
                warnings.push(format!(
                    "{name}: {} could not be matched to the browser's saved session \
                     (incognito, app, DevTools or another profile's windows); their tabs were \
                     not captured.",
                    count(unmatched, "window", "windows")
                ));
            }
            session_warnings(name, &session, &mut warnings);
            Some(profile_name)
        };
        TabCapture {
            state: AppState::Browser { windows, profile },
            warnings,
        }
    }

    /// User data directories the entry's browser may use on this machine, most likely first.
    fn launch_user_data_dirs(&self, entry: &ApplicationEntry, os: OsKind) -> Vec<PathBuf> {
        if let Some(dir) = self.user_data_override() {
            return vec![dir];
        }
        let base = BaseDirs::from_system();
        let command_line: Vec<String> = std::iter::once(String::new())
            .chain(entry.launch.arguments.iter().cloned())
            .collect();
        let custom = profile::parse_switches(&command_line, os)
            .user_data_dir
            .and_then(|value| {
                profile::resolve_user_data_dir(
                    &value,
                    entry.launch.working_directory.as_deref(),
                    base.home.as_deref(),
                )
            });
        let defaults = browser::install_from_launch(&entry.app_id, &entry.launch)
            .map(|install| browser::default_user_data_dirs(&install, os, &base))
            .unwrap_or_default();
        custom.into_iter().chain(defaults).collect()
    }

    /// `--profile-directory=<profile>` for the saved profile when it exists here, `Default`
    /// included: without it the browser opens the addresses in its last-used profile, which
    /// may be another one. A missing profile is not passed: the browser would silently create
    /// a new, empty one.
    fn profile_switch(
        &self,
        ctx: &LaunchContext<'_>,
        entry: &ApplicationEntry,
        profile: Option<&str>,
    ) -> Option<String> {
        let profile = profile?;
        if !profile::valid_profile_name(profile) {
            return None;
        }
        let user_data = self
            .launch_user_data_dirs(entry, ctx.info.os)
            .into_iter()
            .find(|dir| dir.is_dir())?;
        user_data
            .join(profile)
            .is_dir()
            .then(|| format!("--profile-directory={profile}"))
    }
}

impl AppAdapter for ChromiumAdapter {
    fn id(&self) -> &'static str {
        "chromium"
    }

    fn name(&self) -> &'static str {
        "Chrome, Edge, Brave & Chromium"
    }

    fn identify(&self, process: &ProcessInfo, _windows: &[&WindowInfo]) -> Option<AppIdentity> {
        let install = browser::identify_process(process)?;
        Some(AppIdentity {
            app_id: install.browser.app_id().to_string(),
            display_name: install.display_name(),
        })
    }

    fn capability(&self, info: &PlatformInfo) -> AdapterCapability {
        let placement = info.capabilities.move_windows.level != SupportLevel::Unsupported;
        AdapterCapability {
            level: CapabilityLevel::Full,
            features: Vec::new(),
        }
        .feature(
            "Reopen windows and tabs",
            true,
            Some(
                "Needs \"Read browser tabs\" enabled in Settings. Internal pages such as New \
                 Tab or Settings are not reopened.",
            ),
        )
        .feature("Pinned tabs", false, Some("Reopened as normal tabs."))
        .feature(
            "Tab groups",
            false,
            Some("Tabs are reopened without their groups."),
        )
        .feature(
            "Back and forward history",
            false,
            Some("Only the current page of each tab is reopened."),
        )
        .feature(
            "Sign-ins, cookies and history",
            true,
            Some("Kept by your browser profile."),
        )
        .feature(
            "Window position and size",
            placement,
            info.capabilities
                .move_windows
                .note
                .as_deref()
                .filter(|_| !placement),
        )
    }

    fn capture(&self, ctx: &CaptureContext<'_>, input: &CaptureInput<'_>) -> CaptureOutput {
        let name = input.identity.display_name.as_str();
        let Some(main) = input.main_process() else {
            return CaptureOutput {
                state: windows_without_tabs(&input.windows, None),
                launch: LaunchConfig::default(),
                warnings: vec![format!(
                    "{name}: no process information available, so its tabs were not captured."
                )],
            };
        };
        let found = Found::inspect(main, &input.identity.app_id, ctx.info.os);
        let launch = launch_config(main, &found);
        let mut warnings = Vec::new();
        if launch.executable.is_none() && launch.package.is_none() {
            warnings.push(format!(
                "{name}: the executable path is not accessible; choose it in the workspace \
                 before restoring."
            ));
        }
        let tabs = self.capture_tabs(ctx, input, &found);
        warnings.extend(tabs.warnings);
        CaptureOutput {
            state: tabs.state,
            launch,
            warnings,
        }
    }

    fn launch_plan(
        &self,
        ctx: &LaunchContext<'_>,
        entry: &ApplicationEntry,
    ) -> Result<Vec<LaunchStep>, String> {
        let target = generic::launch_target(&entry.launch)
            .ok_or_else(|| format!("No executable is configured for {}.", entry.display_name))?;
        let (windows, profile) = match &entry.state {
            AppState::Browser { windows, profile } => (windows.as_slice(), profile.as_deref()),
            _ => (&[][..], None),
        };

        let mut fixed: Vec<String> = Vec::new();
        if let Some(switch) = self.profile_switch(ctx, entry, profile) {
            fixed.push(switch);
        }
        fixed.extend(entry.launch.arguments.iter().cloned());
        // Everything after `--` is an address, never a switch. Users' own arguments may
        // already contain the separator.
        let separator = !fixed.iter().any(|a| a == "--");
        let fixed_chars = launch::arg_chars(&target.label())
            + launch::arg_chars("--new-window")
            + launch::arg_chars("--")
            + fixed.iter().map(|a| launch::arg_chars(a)).sum::<usize>();
        if fixed_chars + launch::MIN_URL_BUDGET > launch::MAX_COMMAND_LINE_CHARS {
            return Err(format!(
                "The launch arguments configured for {} are too long to reopen its tabs.",
                entry.display_name
            ));
        }

        let request = |args: Vec<String>| LaunchRequest {
            target: target.clone(),
            args,
            working_directory: entry.launch.working_directory.clone(),
            env: Vec::new(),
        };
        let launches = launch::window_launches(windows, fixed_chars);
        if launches.is_empty() {
            // Nothing restorable: start the browser (it may restore its own session).
            return Ok(vec![LaunchStep {
                request: request(fixed),
                expected_windows: 1,
                title_hint: None,
            }]);
        }
        Ok(launches
            .into_iter()
            .map(|window| {
                let mut args = Vec::with_capacity(fixed.len() + window.urls.len() + 2);
                args.push("--new-window".to_string());
                args.extend(fixed.iter().cloned());
                if separator {
                    args.push("--".to_string());
                }
                args.extend(window.urls);
                LaunchStep {
                    request: request(args),
                    expected_windows: 1,
                    title_hint: window.title_hint,
                }
            })
            .collect())
    }

    fn instance_model(&self) -> InstanceModel {
        InstanceModel::SingleInstance
    }

    fn describe(&self, state: &AppState) -> Vec<StateDetail> {
        let AppState::Browser { windows, profile } = state else {
            return self.generic.describe(state);
        };
        let mut details = vec![StateDetail::new("Windows", windows.len().to_string())];
        let tabs: Vec<&BrowserTab> = windows.iter().flat_map(|w| &w.tabs).collect();
        if tabs.is_empty() {
            details.push(StateDetail::new("Tabs", "None saved"));
        } else {
            details.push(StateDetail::new("Tabs", tabs.len().to_string()));
            let mut listed = tabs
                .iter()
                .take(DESCRIBED_TABS)
                .map(|t| short_title(t))
                .collect::<Vec<_>>()
                .join(", ");
            if tabs.len() > DESCRIBED_TABS {
                listed = format!("{listed} and {} more", tabs.len() - DESCRIBED_TABS);
            }
            details.push(StateDetail::new("Includes", listed));
        }
        if let Some(profile) = profile.as_deref().filter(|p| *p != DEFAULT_PROFILE) {
            details.push(StateDetail::new("Profile", profile));
        }
        details
    }

    fn locate_query(&self, app_id: &str) -> LocateQuery {
        match Browser::from_app_id(app_id) {
            Some(browser) => browser::locate_query(browser, OsKind::current()),
            None => self.generic.locate_query(app_id),
        }
    }
}

/// What the main process tells about the browser installation and its profile.
struct Found {
    os: OsKind,
    install: Option<Install>,
    command_line_profile: Option<String>,
    /// `--user-data-dir`, made absolute.
    command_line_user_data: Option<PathBuf>,
    /// Default user data directories of `install`.
    default_user_data: Vec<PathBuf>,
}

impl Found {
    fn inspect(main: &ProcessInfo, app_id: &str, os: OsKind) -> Self {
        let base = BaseDirs::from_system();
        let install = browser::identify_process(main)
            .or_else(|| Browser::from_app_id(app_id).map(|b| Install::native(b, Channel::Stable)));
        let switches = profile::parse_switches(&main.command_line, os);
        let command_line_user_data = switches.user_data_dir.as_deref().and_then(|value| {
            profile::resolve_user_data_dir(
                value,
                main.working_directory.as_deref(),
                base.home.as_deref(),
            )
        });
        let default_user_data = install
            .as_ref()
            .map(|install| browser::default_user_data_dirs(install, os, &base))
            .unwrap_or_default();
        Self {
            os,
            install,
            command_line_profile: switches
                .profile_directory
                .filter(|p| profile::valid_profile_name(p)),
            command_line_user_data,
            default_user_data,
        }
    }
}

struct TabCapture {
    state: AppState,
    warnings: Vec<String>,
}

/// Warnings about the quality of a session whose tabs were captured.
fn session_warnings(name: &str, session: &Session, warnings: &mut Vec<String>) {
    if session.damaged {
        warnings.push(format!(
            "{name}: part of the browser's session file is damaged; recent tab changes may be \
             missing."
        ));
    }
    if session.limits_exceeded {
        warnings.push(format!(
            "{name}: the browser's session is unusually large; some tabs were not captured."
        ));
    }
}

enum SessionLoad {
    Loaded(Session),
    Encrypted,
    /// The newest session file exists but cannot be opened. On Windows a running browser keeps
    /// the file it writes locked; older files hold earlier sessions and are not used instead.
    Inaccessible(String),
    Unreadable(String),
    Missing,
}

/// The current session of a profile: its newest plain session file. An older file is used
/// only while the newer one is still being written (it holds an earlier session, so in any
/// other case it would describe tabs that are no longer open).
fn load_session(profile_dir: &Path) -> SessionLoad {
    let newest_encrypted = profile::newest_encrypted_session(profile_dir);
    let mut problem: Option<String> = None;
    let max_bytes = u64::try_from(snss::Limits::default().max_file_bytes).unwrap_or(u64::MAX);
    for file in profile::session_files(profile_dir)
        .into_iter()
        .take(MAX_SESSION_FILES_TRIED)
    {
        let data = match profile::read_bounded(&file.path, max_bytes) {
            Ok(data) => data,
            Err(error) => {
                log::debug!("chromium: cannot read {}: {error}", file.path.display());
                return match error.kind() {
                    // Replaced by a newer file since the folder was listed (a retry reads
                    // it), too large, or not a regular file.
                    std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::InvalidData
                    | std::io::ErrorKind::InvalidInput => {
                        SessionLoad::Unreadable(error.to_string())
                    }
                    _ => SessionLoad::Inaccessible(error.to_string()),
                };
            }
        };
        match snss::parse_session(&data) {
            Ok(session) => {
                let stale = newest_encrypted
                    .as_ref()
                    .and_then(|encrypted| encrypted.modified)
                    .zip(file.modified)
                    .and_then(|(encrypted, plain)| encrypted.duration_since(plain).ok())
                    .is_some_and(|age| age > STALE_PLAIN_SESSION);
                if stale {
                    log::debug!(
                        "chromium: {} is older than the encrypted session",
                        file.path.display()
                    );
                    return SessionLoad::Encrypted;
                }
                log::debug!(
                    "chromium: read {} ({} windows)",
                    file.path.display(),
                    session.windows.len()
                );
                return SessionLoad::Loaded(session);
            }
            Err(SnssError::Encrypted { .. }) => return SessionLoad::Encrypted,
            // Created or rewritten right now: the previous file is the latest complete state.
            Err(error @ (SnssError::Incomplete | SnssError::TooShort)) => {
                log::debug!("chromium: {} is incomplete", file.path.display());
                problem.get_or_insert_with(|| error.to_string());
            }
            Err(error) => {
                log::debug!("chromium: cannot parse {}: {error}", file.path.display());
                return SessionLoad::Unreadable(error.to_string());
            }
        }
    }
    // A failing plain file next to encrypted ones is most likely being rewritten right now
    // (browsers write both while migrating): report the failure, which a retry can fix.
    if let Some(problem) = problem {
        SessionLoad::Unreadable(problem)
    } else if newest_encrypted.is_some() {
        SessionLoad::Encrypted
    } else {
        SessionLoad::Missing
    }
}

/// How to start the captured browser again.
fn launch_config(main: &ProcessInfo, found: &Found) -> LaunchConfig {
    let mut package = main.package.clone();
    let mut executable = main.executable_path.clone();
    if let Some(install) = &found.install {
        match &install.kind {
            InstallKind::Flatpak(id) => {
                package.get_or_insert_with(|| PackageInfo {
                    kind: PackageKind::Flatpak,
                    id: id.clone(),
                    path: None,
                });
                // The binary (inside the sandbox, or under the Flatpak installation) only
                // runs through `flatpak run`.
                executable = None;
            }
            InstallKind::Snap(name) => {
                package.get_or_insert_with(|| PackageInfo {
                    kind: PackageKind::Snap,
                    id: name.clone(),
                    path: None,
                });
                // Revision paths change with every update; the launcher does not.
                let launcher = Path::new("/snap/bin").join(name);
                executable = launcher.is_file().then_some(launcher);
            }
            InstallKind::Native => {
                if found.os == OsKind::Linux {
                    executable = executable.map(|exe| {
                        browser::linux_launcher_candidates(&exe)
                            .into_iter()
                            .find(|candidate| candidate.is_file())
                            .unwrap_or(exe)
                    });
                }
                if found.os == OsKind::Macos && package.is_none() {
                    package = executable
                        .as_deref()
                        .and_then(browser::mac_bundle_path)
                        .zip(install.mac_bundle_id())
                        .map(|(path, id)| PackageInfo {
                            kind: PackageKind::MacBundle,
                            id: id.to_string(),
                            path: Some(path),
                        });
                }
            }
        }
    }
    // A custom profile location is needed to reach the same browser instance again. It is a
    // folder path, not a secret; nothing else from the captured command line is kept.
    let arguments = found
        .command_line_user_data
        .as_deref()
        .filter(|dir| dir.is_dir() && !found.default_user_data.iter().any(|d| d == dir))
        .and_then(Path::to_str)
        .map(|dir| vec![format!("--user-data-dir={dir}")])
        .unwrap_or_default();
    LaunchConfig {
        executable,
        package,
        arguments,
        working_directory: None,
    }
}

/// A saved window with the restorable tabs of `window`.
fn browser_window(window: &SessionWindow, window_title: Option<String>) -> BrowserWindow {
    let mut tabs = Vec::new();
    let mut active_tab = None;
    for (index, tab) in window.tabs.iter().enumerate() {
        let Some(url) = urls::restorable_url(&tab.url) else {
            continue;
        };
        if index == window.selected_tab {
            active_tab = u32::try_from(tabs.len()).ok();
        }
        let title = if tab.title.is_empty() {
            url.clone()
        } else {
            tab.title.clone()
        };
        tabs.push(BrowserTab {
            url,
            title,
            pinned: tab.pinned,
        });
    }
    BrowserWindow {
        tabs,
        active_tab,
        window_title,
    }
}

fn window_without_tabs(window: &WindowInfo) -> BrowserWindow {
    BrowserWindow {
        tabs: Vec::new(),
        active_tab: None,
        window_title: Some(window.title.clone()),
    }
}

/// State for windows whose tabs are unknown (`None` when there are no windows either).
fn windows_without_tabs(windows: &[&WindowInfo], profile: Option<&str>) -> AppState {
    if windows.is_empty() {
        return AppState::None;
    }
    AppState::Browser {
        windows: windows.iter().map(|w| window_without_tabs(w)).collect(),
        profile: profile.map(str::to_string),
    }
}

/// The title the browser shows for `window`: its name or its active tab's title, followed by
/// the product name except on macOS.
fn expected_window_title(window: &SessionWindow, install: &Install, os: OsKind) -> String {
    if let Some(title) = &window.user_title {
        return title.clone();
    }
    let page = window
        .active_tab()
        .map(|tab| {
            if tab.title.is_empty() {
                urls::display_host(&tab.url).unwrap_or_else(|| tab.url.clone())
            } else {
                tab.title.clone()
            }
        })
        .unwrap_or_default();
    if os == OsKind::Macos {
        page
    } else {
        format!("{page} - {}", install.browser.product_name())
    }
}

fn short_title(tab: &BrowserTab) -> String {
    const MAX: usize = 40;
    let title = if tab.title.trim().is_empty() || tab.title == tab.url {
        urls::display_host(&tab.url).unwrap_or_else(|| tab.url.clone())
    } else {
        tab.title.trim().to_string()
    };
    if title.chars().count() > MAX {
        let cut: String = title.chars().take(MAX - 1).collect();
        format!("{}…", cut.trim_end())
    } else {
        title
    }
}

fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{SnssWriter, standard_window, window_at, write_session_file};
    use super::*;
    use crate::adapters::AdapterRegistry;
    use reframe_platform::mock::MockPlatform;
    use reframe_platform::{LaunchTarget, Platform, Rect, WindowId, WindowState};

    fn chrome_process(cmd: &[&str]) -> ProcessInfo {
        ProcessInfo {
            pid: 4242,
            parent_pid: None,
            name: "chrome".into(),
            product_name: None,
            executable_path: Some("/opt/google/chrome/chrome".into()),
            command_line: std::iter::once("/opt/google/chrome/chrome")
                .chain(cmd.iter().copied())
                .map(str::to_string)
                .collect(),
            working_directory: None,
            start_time: None,
            package: None,
        }
    }

    fn window(id: &str, title: &str, bounds: Rect) -> WindowInfo {
        WindowInfo {
            id: WindowId::new(id),
            pid: 4242,
            title: title.into(),
            class_name: Some("Google-chrome".into()),
            app_id_hint: None,
            bounds,
            normal_bounds: Some(bounds),
            state: WindowState::Normal,
            monitor_id: None,
            desktop: None,
            is_focused: false,
            z_order: 0,
        }
    }

    fn identity() -> AppIdentity {
        AppIdentity {
            app_id: "chrome".into(),
            display_name: "Google Chrome".into(),
        }
    }

    struct Fixture {
        dir: tempfile::TempDir,
        adapter: ChromiumAdapter,
        platform: MockPlatform,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let adapter = ChromiumAdapter::with_user_data_dir(dir.path());
            Self {
                dir,
                adapter,
                platform: MockPlatform::new(),
            }
        }

        fn profile(&self, name: &str) -> PathBuf {
            let path = self.dir.path().join(name);
            std::fs::create_dir_all(&path).unwrap();
            path
        }

        fn local_state(&self, json: &str) {
            std::fs::write(self.dir.path().join("Local State"), json).unwrap();
        }

        fn capture(
            &self,
            process: &ProcessInfo,
            windows: &[&WindowInfo],
            tabs: bool,
        ) -> CaptureOutput {
            let info = self.platform.info();
            let ctx = CaptureContext {
                platform: &self.platform,
                info: &info,
                browser_tab_capture: tabs,
            };
            let identity = identity();
            let all = vec![process.clone()];
            let input = CaptureInput {
                identity: &identity,
                processes: vec![process],
                windows: windows.to_vec(),
                all_processes: &all,
            };
            self.adapter.capture(&ctx, &input)
        }

        fn plan(&self, entry: &ApplicationEntry) -> Result<Vec<LaunchStep>, String> {
            let info = self.platform.info();
            let ctx = LaunchContext {
                platform: &self.platform,
                info: &info,
            };
            self.adapter.launch_plan(&ctx, entry)
        }
    }

    fn two_window_session() -> Vec<u8> {
        let writer = window_at(
            SnssWriter::new(3),
            1,
            10,
            &[
                ("https://mail.example.com/inbox", "Inbox - Example Mail"),
                ("chrome://newtab/", "New Tab"),
                ("https://calendar.example.com/", "Calendar"),
            ],
            0,
            (0, 0, 960, 1040),
        )
        .pinned(10, true);
        window_at(
            writer,
            2,
            20,
            &[
                ("https://docs.rs/serde", "serde - Rust"),
                (
                    "https://github.com/reframe/reframe",
                    "reframe/reframe: Workspaces",
                ),
            ],
            1,
            (960, 0, 960, 1040),
        )
        .active_window(2)
        .marker()
        .build()
    }

    fn browser_state(output: &CaptureOutput) -> (&[BrowserWindow], Option<&str>) {
        match &output.state {
            AppState::Browser { windows, profile } => (windows, profile.as_deref()),
            other => panic!("expected browser state, got {other:?}"),
        }
    }

    #[test]
    fn registry_routes_browsers_to_this_adapter() {
        let registry = AdapterRegistry::builtin();
        let (adapter, identity) = registry.identify(&chrome_process(&[]), &[]);
        assert_eq!(adapter.id(), "chromium");
        assert_eq!(identity.app_id, "chrome");
        assert_eq!(identity.display_name, "Google Chrome");
        let renderer = chrome_process(&["--type=renderer"]);
        assert_eq!(registry.identify(&renderer, &[]).0.id(), "generic");
        let mut edge = chrome_process(&[]);
        edge.executable_path =
            Some(r"C:\Program Files (x86)\Microsoft\Edge Beta\Application\msedge.exe".into());
        let identity = ChromiumAdapter::new().identify(&edge, &[]).unwrap();
        assert_eq!(identity.app_id, "edge");
        assert_eq!(identity.display_name, "Microsoft Edge Beta");
    }

    #[test]
    fn captures_windows_and_tabs_from_the_last_used_profile() {
        let fx = Fixture::new();
        fx.local_state(r#"{"profile":{"last_used":"Profile 1"}}"#);
        fx.profile("Default");
        write_session_file(
            &fx.profile("Profile 1"),
            13_350_000_000_000_000,
            &two_window_session(),
        );
        let docs = window(
            "w1",
            "reframe/reframe: Workspaces - Google Chrome",
            Rect::new(960, 0, 960, 1040),
        );
        let mail = window(
            "w2",
            "Inbox - Example Mail - Google Chrome",
            Rect::new(0, 0, 960, 1040),
        );
        let out = fx.capture(&chrome_process(&[]), &[&docs, &mail], true);
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        let (windows, profile) = browser_state(&out);
        assert_eq!(profile, Some("Profile 1"));
        assert_eq!(windows.len(), 2);
        // Front-to-back order of the selected windows is kept.
        assert_eq!(
            windows[0].window_title.as_deref(),
            Some("reframe/reframe: Workspaces - Google Chrome")
        );
        let urls: Vec<&str> = windows[0].tabs.iter().map(|t| t.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://docs.rs/serde",
                "https://github.com/reframe/reframe"
            ]
        );
        assert_eq!(windows[0].active_tab, Some(1));
        // The New Tab page is dropped; the pinned flag survives.
        let mail_tabs = &windows[1].tabs;
        assert_eq!(mail_tabs.len(), 2);
        assert_eq!(mail_tabs[0].title, "Inbox - Example Mail");
        assert!(mail_tabs[0].pinned);
        assert_eq!(mail_tabs[1].url, "https://calendar.example.com/");
        assert_eq!(windows[1].active_tab, Some(0));
        // The distribution launcher script replaces the binary when it is installed here.
        let executable = out.launch.executable.unwrap();
        assert!(
            executable == Path::new("/opt/google/chrome/chrome")
                || executable == Path::new("/opt/google/chrome/google-chrome"),
            "{executable:?}"
        );
        assert!(out.launch.arguments.is_empty());
    }

    #[test]
    fn captures_only_selected_windows() {
        let fx = Fixture::new();
        write_session_file(
            &fx.profile("Default"),
            13_350_000_000_000_000,
            &two_window_session(),
        );
        let mail = window(
            "w2",
            "Inbox - Example Mail - Google Chrome",
            Rect::new(0, 0, 960, 1040),
        );
        let out = fx.capture(&chrome_process(&[]), &[&mail], true);
        let (windows, profile) = browser_state(&out);
        assert_eq!(profile, Some("Default"));
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].tabs[0].url, "https://mail.example.com/inbox");
    }

    #[test]
    fn chooses_the_profile_that_owns_the_windows() {
        let fx = Fixture::new();
        fx.local_state(
            r#"{"profile":{"last_used":"Default","last_active_profiles":["Default","Profile 2"]}}"#,
        );
        let mail = standard_window(
            SnssWriter::new(3),
            1,
            10,
            &[("https://mail.example.com/", "Mail")],
            0,
        )
        .marker()
        .build();
        let docs = standard_window(
            SnssWriter::new(3),
            1,
            10,
            &[("https://docs.example.com/", "Team Docs")],
            0,
        )
        .marker()
        .build();
        write_session_file(&fx.profile("Default"), 13_350_000_000_000_000, &mail);
        write_session_file(&fx.profile("Profile 2"), 13_350_000_000_000_000, &docs);
        let os = window(
            "w1",
            "Team Docs - Google Chrome",
            Rect::new(100, 100, 1200, 800),
        );
        let out = fx.capture(&chrome_process(&[]), &[&os], true);
        let (windows, profile) = browser_state(&out);
        assert_eq!(profile, Some("Profile 2"));
        assert_eq!(windows[0].tabs[0].url, "https://docs.example.com/");
    }

    #[test]
    fn command_line_profile_and_unmatched_windows() {
        let fx = Fixture::new();
        write_session_file(
            &fx.profile("Work"),
            13_350_000_000_000_000,
            &two_window_session(),
        );
        let mail = window(
            "w2",
            "Inbox - Example Mail - Google Chrome",
            Rect::new(0, 0, 960, 1040),
        );
        let devtools = window(
            "w3",
            "DevTools - mail.example.com/inbox",
            Rect::new(0, 0, 400, 300),
        );
        let out = fx.capture(
            &chrome_process(&["--profile-directory=Work"]),
            &[&mail, &devtools],
            true,
        );
        let (windows, profile) = browser_state(&out);
        assert_eq!(profile, Some("Work"));
        assert_eq!(windows.len(), 2);
        assert!(!windows[0].tabs.is_empty());
        assert!(windows[1].tabs.is_empty());
        assert_eq!(
            windows[1].window_title.as_deref(),
            Some("DevTools - mail.example.com/inbox")
        );
        assert_eq!(out.warnings.len(), 1);
        assert!(
            out.warnings[0].contains("1 window could not be matched"),
            "{:?}",
            out.warnings
        );

        // The same command line as Linux shows it for Chromium (rewritten process title).
        let mut squashed = chrome_process(&[]);
        squashed.command_line = vec!["/opt/google/chrome/chrome --profile-directory=Work".into()];
        let out = fx.capture(&squashed, &[&mail], true);
        let (windows, profile) = browser_state(&out);
        assert_eq!(profile, Some("Work"), "{:?}", out.warnings);
        assert!(!windows[0].tabs.is_empty());
    }

    #[test]
    fn without_selected_windows_every_normal_window_is_kept() {
        let fx = Fixture::new();
        write_session_file(
            &fx.profile("Default"),
            13_350_000_000_000_000,
            &two_window_session(),
        );
        let out = fx.capture(&chrome_process(&[]), &[], true);
        let (windows, _) = browser_state(&out);
        assert_eq!(windows.len(), 2);
        assert_eq!(
            windows[0].window_title.as_deref(),
            Some("Inbox - Example Mail - Google Chrome")
        );
    }

    #[test]
    fn disabled_capture_keeps_windows_without_tabs() {
        let fx = Fixture::new();
        write_session_file(
            &fx.profile("Default"),
            13_350_000_000_000_000,
            &two_window_session(),
        );
        let mail = window(
            "w2",
            "Inbox - Example Mail - Google Chrome",
            Rect::new(0, 0, 960, 1040),
        );
        let out = fx.capture(
            &chrome_process(&["--profile-directory=Profile 3"]),
            &[&mail],
            false,
        );
        let (windows, profile) = browser_state(&out);
        assert_eq!(windows.len(), 1);
        assert!(windows[0].tabs.is_empty());
        assert_eq!(profile, Some("Profile 3"));
        assert_eq!(out.warnings.len(), 1);
        assert!(out.warnings[0].contains("Read browser tabs"));
        // No windows and no tabs: nothing but the launch.
        let out = fx.capture(&chrome_process(&[]), &[], false);
        assert_eq!(out.state, AppState::None);
    }

    #[test]
    fn missing_or_unreadable_sessions_degrade_gracefully() {
        let fx = Fixture::new();
        fx.profile("Default");
        let mail = window(
            "w2",
            "Inbox - Example Mail - Google Chrome",
            Rect::new(0, 0, 960, 1040),
        );
        let out = fx.capture(&chrome_process(&[]), &[&mail], true);
        let (windows, _) = browser_state(&out);
        assert!(windows[0].tabs.is_empty());
        assert!(
            out.warnings[0].contains("has not saved a session"),
            "{:?}",
            out.warnings
        );

        write_session_file(
            &fx.profile("Default"),
            13_350_000_000_000_000,
            b"SNSS\x03\x00\x00\x00\x05\x00",
        );
        let out = fx.capture(&chrome_process(&[]), &[&mail], true);
        assert!(
            out.warnings[0].contains("could not be read"),
            "{:?}",
            out.warnings
        );
        assert!(
            out.warnings[0].contains("not completely written"),
            "{:?}",
            out.warnings
        );

        // A newer, incomplete file falls back to the previous complete one.
        write_session_file(
            &fx.profile("Default"),
            13_340_000_000_000_000,
            &two_window_session(),
        );
        let out = fx.capture(&chrome_process(&[]), &[&mail], true);
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        assert_eq!(browser_state(&out).0[0].tabs.len(), 2);
    }

    /// Older session files hold earlier sessions: they may only stand in for a newest file
    /// that is still being written.
    #[test]
    fn older_sessions_do_not_replace_an_unusable_newest_one() {
        let fx = Fixture::new();
        let default = fx.profile("Default");
        write_session_file(&default, 13_340_000_000_000_000, &two_window_session());
        let mail = window(
            "w2",
            "Inbox - Example Mail - Google Chrome",
            Rect::new(0, 0, 960, 1040),
        );
        // The newest file is encrypted: the older plain one predates the encryption.
        write_session_file(
            &default,
            13_350_000_000_000_000,
            &SnssWriter::new(4).build(),
        );
        let out = fx.capture(&chrome_process(&[]), &[&mail], true);
        assert!(out.warnings[0].contains("encrypts"), "{:?}", out.warnings);
        assert!(browser_state(&out).0[0].tabs.is_empty());
        // The newest file is not a session file at all.
        write_session_file(&default, 13_350_000_000_000_000, b"garbage-garbage-");
        let out = fx.capture(&chrome_process(&[]), &[&mail], true);
        assert!(
            out.warnings[0].contains("not a Chromium session file"),
            "{:?}",
            out.warnings
        );
        assert!(browser_state(&out).0[0].tabs.is_empty());
    }

    /// On Windows a running browser keeps the session file it writes locked. Neither an older
    /// file (an earlier session) nor the last session of a profile that is not open may stand
    /// in for it.
    #[cfg(unix)]
    #[test]
    fn inaccessible_current_session_is_reported_not_replaced() {
        use std::os::unix::fs::PermissionsExt;
        let fx = Fixture::new();
        fx.local_state(
            r#"{"profile":{"last_used":"Default","info_cache":{"Default":{},"Profile 1":{}}}}"#,
        );
        let default = fx.profile("Default");
        write_session_file(&default, 13_340_000_000_000_000, &two_window_session());
        write_session_file(&default, 13_350_000_000_000_000, &two_window_session());
        let current = default.join("Sessions").join("Session_13350000000000000");
        std::fs::set_permissions(&current, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&current).is_ok() {
            return; // Running as root: permissions cannot make the file unreadable.
        }
        write_session_file(
            &fx.profile("Profile 1"),
            13_330_000_000_000_000,
            &two_window_session(),
        );
        let mail = window(
            "w2",
            "Inbox - Example Mail - Google Chrome",
            Rect::new(0, 0, 960, 1040),
        );
        let out = fx.capture(&chrome_process(&[]), &[&mail], true);
        let (windows, _) = browser_state(&out);
        assert!(windows[0].tabs.is_empty(), "{windows:?}");
        assert_eq!(out.warnings.len(), 1, "{:?}", out.warnings);
        assert!(
            out.warnings[0].contains("could not be opened"),
            "{:?}",
            out.warnings
        );
    }

    #[test]
    fn encrypted_sessions_are_reported() {
        let fx = Fixture::new();
        let profile = fx.profile("Default");
        let encrypted = profile.join("Sessions_Encrypted");
        std::fs::create_dir_all(&encrypted).unwrap();
        std::fs::write(
            encrypted.join("Session_13350000000000000"),
            SnssWriter::new(5).build(),
        )
        .unwrap();
        let mail = window(
            "w2",
            "Inbox - Example Mail - Google Chrome",
            Rect::new(0, 0, 960, 1040),
        );
        let out = fx.capture(&chrome_process(&[]), &[&mail], true);
        assert!(out.warnings[0].contains("encrypts"), "{:?}", out.warnings);
        assert!(browser_state(&out).0[0].tabs.is_empty());
    }

    #[test]
    fn plain_sessions_left_over_from_before_encryption_are_ignored() {
        let fx = Fixture::new();
        let profile = fx.profile("Default");
        write_session_file(&profile, 13_350_000_000_000_000, &two_window_session());
        let plain = profile.join("Sessions").join("Session_13350000000000000");
        let an_hour_ago = std::time::SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&plain)
            .unwrap()
            .set_modified(an_hour_ago)
            .unwrap();
        let encrypted = profile.join("Sessions_Encrypted");
        std::fs::create_dir_all(&encrypted).unwrap();
        std::fs::write(
            encrypted.join("Session_13360000000000000"),
            SnssWriter::new(5).build(),
        )
        .unwrap();
        let mail = window(
            "w2",
            "Inbox - Example Mail - Google Chrome",
            Rect::new(0, 0, 960, 1040),
        );
        let out = fx.capture(&chrome_process(&[]), &[&mail], true);
        assert!(out.warnings[0].contains("encrypts"), "{:?}", out.warnings);
        assert!(browser_state(&out).0[0].tabs.is_empty());

        // Both kinds written at the same time (the migration period): the plain one is used.
        std::fs::File::options()
            .write(true)
            .open(&plain)
            .unwrap()
            .set_modified(std::time::SystemTime::now())
            .unwrap();
        let out = fx.capture(&chrome_process(&[]), &[&mail], true);
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        assert_eq!(browser_state(&out).0[0].tabs.len(), 2);
    }

    #[test]
    fn damaged_sessions_keep_earlier_tabs_with_a_warning() {
        let fx = Fixture::new();
        let mut data = two_window_session();
        // A tab-window record with a 3-byte payload: damaged from here on.
        data.extend_from_slice(&[4, 0, 0, 1, 2, 3]);
        write_session_file(&fx.profile("Default"), 13_350_000_000_000_000, &data);
        let mail = window(
            "w2",
            "Inbox - Example Mail - Google Chrome",
            Rect::new(0, 0, 960, 1040),
        );
        let out = fx.capture(&chrome_process(&[]), &[&mail], true);
        assert_eq!(browser_state(&out).0[0].tabs.len(), 2);
        assert_eq!(out.warnings.len(), 1, "{:?}", out.warnings);
        assert!(out.warnings[0].contains("damaged"));
    }

    #[test]
    fn missing_profile_folder_is_reported() {
        let fx = Fixture::new();
        let adapter = ChromiumAdapter::with_user_data_dir(fx.dir.path().join("nowhere"));
        let fx = Fixture { adapter, ..fx };
        let mail = window(
            "w2",
            "Inbox - Example Mail - Google Chrome",
            Rect::new(0, 0, 960, 1040),
        );
        let out = fx.capture(&chrome_process(&[]), &[&mail], true);
        assert!(
            out.warnings[0].contains("profile folder was not found"),
            "{:?}",
            out.warnings
        );
    }

    #[test]
    fn custom_user_data_dir_is_kept_for_launching() {
        let fx = Fixture::new();
        let custom = fx.dir.path().join("custom-udd");
        std::fs::create_dir_all(&custom).unwrap();
        let process = chrome_process(&[&format!("--user-data-dir={}", custom.display())]);
        let found = Found::inspect(&process, "chrome", OsKind::Linux);
        let launch = launch_config(&process, &found);
        assert_eq!(
            launch.arguments,
            [format!("--user-data-dir={}", custom.display())]
        );
        // Other captured arguments are never kept.
        let process = chrome_process(&["--remote-debugging-port=9222", "--token=secret"]);
        let found = Found::inspect(&process, "chrome", OsKind::Linux);
        assert!(launch_config(&process, &found).arguments.is_empty());
    }

    #[test]
    fn launch_configuration_for_packages() {
        let mut snap = chrome_process(&[]);
        snap.executable_path = Some("/snap/chromium/2890/usr/lib/chromium-browser/chrome".into());
        let found = Found::inspect(&snap, "chromium", OsKind::Linux);
        let launch = launch_config(&snap, &found);
        assert_eq!(
            launch.package,
            Some(PackageInfo {
                kind: PackageKind::Snap,
                id: "chromium".into(),
                path: None
            })
        );
        assert_ne!(launch.executable, snap.executable_path);

        let mut flatpak = chrome_process(&[]);
        flatpak.executable_path = Some("/app/extra/chrome".into());
        let found = Found::inspect(&flatpak, "chrome", OsKind::Linux);
        let launch = launch_config(&flatpak, &found);
        assert_eq!(launch.executable, None);
        assert_eq!(launch.package.unwrap().id, "com.google.Chrome");

        let mut mac = chrome_process(&[]);
        mac.executable_path =
            Some("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge".into());
        let found = Found::inspect(&mac, "edge", OsKind::Macos);
        let launch = launch_config(&mac, &found);
        assert_eq!(
            launch.package,
            Some(PackageInfo {
                kind: PackageKind::MacBundle,
                id: "com.microsoft.edgemac".into(),
                path: Some("/Applications/Microsoft Edge.app".into())
            })
        );
    }

    fn browser_entry(windows: Vec<BrowserWindow>, profile: Option<&str>) -> ApplicationEntry {
        let mut entry = ApplicationEntry::new("chrome", "Google Chrome", "chromium");
        entry.launch.executable = Some("/opt/google/chrome/google-chrome".into());
        entry.state = AppState::Browser {
            windows,
            profile: profile.map(str::to_string),
        };
        entry
    }

    fn tabs(urls: &[&str]) -> BrowserWindow {
        BrowserWindow {
            tabs: urls
                .iter()
                .map(|u| BrowserTab {
                    url: u.to_string(),
                    title: format!("Title of {u}"),
                    pinned: false,
                })
                .collect(),
            active_tab: Some(0),
            window_title: None,
        }
    }

    #[test]
    fn launch_plan_opens_one_window_per_saved_window() {
        let fx = Fixture::new();
        fx.profile("Profile 1");
        let entry = browser_entry(
            vec![
                tabs(&[
                    "https://a.example/",
                    "chrome://settings/",
                    "https://b.example/",
                ]),
                tabs(&["chrome://newtab/"]),
                tabs(&["https://c.example/"]),
            ],
            Some("Profile 1"),
        );
        let steps = fx.plan(&entry).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(
            steps[0].request.args,
            [
                "--new-window",
                "--profile-directory=Profile 1",
                "--",
                "https://a.example/",
                "https://b.example/"
            ]
        );
        assert_eq!(
            steps[0].request.target,
            LaunchTarget::Executable {
                path: "/opt/google/chrome/google-chrome".into()
            }
        );
        assert_eq!(steps[0].expected_windows, 1);
        assert_eq!(
            steps[0].title_hint.as_deref(),
            Some("Title of https://a.example/")
        );
        assert_eq!(
            steps[1].request.args,
            [
                "--new-window",
                "--profile-directory=Profile 1",
                "--",
                "https://c.example/"
            ]
        );
    }

    #[test]
    fn profile_switch_only_for_existing_profiles() {
        let fx = Fixture::new();
        fx.profile("Default");
        let window = vec![tabs(&["https://a.example/"])];
        for profile in [Some("Profile 9"), Some("../escape"), None] {
            let steps = fx.plan(&browser_entry(window.clone(), profile)).unwrap();
            assert_eq!(
                steps[0].request.args,
                ["--new-window", "--", "https://a.example/"],
                "{profile:?}"
            );
        }
        // Without the switch the browser would use its last-used profile, which can be
        // another one: `Default` is named explicitly too.
        let steps = fx
            .plan(&browser_entry(window.clone(), Some("Default")))
            .unwrap();
        assert_eq!(
            steps[0].request.args,
            [
                "--new-window",
                "--profile-directory=Default",
                "--",
                "https://a.example/"
            ]
        );
    }

    #[test]
    fn launch_plan_without_restorable_tabs_starts_the_browser() {
        let fx = Fixture::new();
        let mut entry = browser_entry(vec![tabs(&["chrome://newtab/"]), tabs(&[])], None);
        entry.launch.arguments = vec!["--disable-gpu".into()];
        let steps = fx.plan(&entry).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].request.args, ["--disable-gpu"]);
        assert_eq!(steps[0].title_hint, None);
        let mut entry = ApplicationEntry::new("chrome", "Google Chrome", "chromium");
        assert!(fx.plan(&entry).is_err());
        entry.launch.executable = Some("/usr/bin/google-chrome".into());
        assert_eq!(
            fx.plan(&entry).unwrap()[0].request.args,
            Vec::<String>::new()
        );
    }

    #[test]
    fn launch_plan_splits_large_windows() {
        let fx = Fixture::new();
        let urls: Vec<String> = (0..230)
            .map(|i| format!("https://example.com/page/{i}"))
            .collect();
        let refs: Vec<&str> = urls.iter().map(String::as_str).collect();
        let entry = browser_entry(vec![tabs(&refs)], None);
        let steps = fx.plan(&entry).unwrap();
        assert_eq!(steps.len(), 3);
        let mut reopened = Vec::new();
        for step in &steps {
            let args = &step.request.args;
            assert_eq!(args[0], "--new-window");
            let separator = args.iter().position(|a| a == "--").unwrap();
            reopened.extend(args[separator + 1..].iter().cloned());
            let chars: usize =
                step.request.target.label().len() + args.iter().map(|a| a.len() + 3).sum::<usize>();
            assert!(chars < launch::MAX_COMMAND_LINE_CHARS);
        }
        assert_eq!(reopened, urls);
    }

    #[test]
    fn launch_plan_uses_the_bundle_on_macos_and_keeps_user_arguments() {
        let fx = Fixture::new();
        let mut entry = browser_entry(vec![tabs(&["https://a.example/"])], None);
        entry.launch.package = Some(PackageInfo {
            kind: PackageKind::MacBundle,
            id: "com.google.Chrome".into(),
            path: Some("/Applications/Google Chrome.app".into()),
        });
        entry.launch.arguments = vec!["--user-data-dir=/tmp/somewhere".into()];
        let steps = fx.plan(&entry).unwrap();
        assert!(matches!(
            steps[0].request.target,
            LaunchTarget::Package { .. }
        ));
        assert_eq!(
            steps[0].request.args,
            [
                "--new-window",
                "--user-data-dir=/tmp/somewhere",
                "--",
                "https://a.example/"
            ]
        );
        // Arguments that already end switch parsing are respected.
        entry.launch.arguments = vec!["--".into()];
        let steps = fx.plan(&entry).unwrap();
        assert_eq!(
            steps[0].request.args,
            ["--new-window", "--", "https://a.example/"]
        );
        // Absurdly long configured arguments are refused instead of producing broken launches.
        entry.launch.arguments = vec![format!("--flag={}", "x".repeat(25_000))];
        assert!(fx.plan(&entry).is_err());
    }

    #[test]
    fn describes_windows_and_tabs() {
        let adapter = ChromiumAdapter::new();
        let state = AppState::Browser {
            windows: vec![
                tabs(&["https://a.example/", "https://b.example/"]),
                tabs(&["https://c.example/", "https://d.example/"]),
            ],
            profile: Some("Profile 1".into()),
        };
        let details = adapter.describe(&state);
        assert_eq!(details[0], StateDetail::new("Windows", "2"));
        assert_eq!(details[1], StateDetail::new("Tabs", "4"));
        assert_eq!(details[2].label, "Includes");
        assert!(
            details[2].value.ends_with("and 1 more"),
            "{}",
            details[2].value
        );
        assert_eq!(details[3], StateDetail::new("Profile", "Profile 1"));
        let empty = AppState::Browser {
            windows: vec![tabs(&[])],
            profile: Some("Default".into()),
        };
        assert_eq!(
            adapter.describe(&empty),
            [
                StateDetail::new("Windows", "1"),
                StateDetail::new("Tabs", "None saved")
            ]
        );
        assert!(adapter.describe(&AppState::None).is_empty());
        let long = BrowserTab {
            url: "https://x.example/".into(),
            title: "A very long page title that goes on and on and on forever".into(),
            pinned: false,
        };
        assert!(short_title(&long).chars().count() <= 40);
    }

    #[test]
    fn capability_and_locate_query() {
        let adapter = ChromiumAdapter::new();
        let platform = MockPlatform::new();
        let capability = adapter.capability(&platform.info());
        assert_eq!(capability.level, CapabilityLevel::Full);
        let feature = |name: &str| {
            capability
                .features
                .iter()
                .find(|f| f.name == name)
                .unwrap_or_else(|| panic!("missing feature {name}"))
                .clone()
        };
        assert!(feature("Reopen windows and tabs").supported);
        assert!(!feature("Pinned tabs").supported);
        assert!(!feature("Tab groups").supported);
        assert!(feature("Sign-ins, cookies and history").supported);
        assert!(feature("Window position and size").supported);
        assert_eq!(adapter.instance_model(), InstanceModel::SingleInstance);

        let query = adapter.locate_query("brave");
        assert!(query.bundle_ids.contains(&"com.brave.Browser".to_string()));
        assert!(query.display_names.contains(&"Brave Browser".to_string()));
        assert!(
            adapter
                .locate_query("exe:gimp")
                .executable_names
                .contains(&"gimp".to_string())
        );
    }
}
