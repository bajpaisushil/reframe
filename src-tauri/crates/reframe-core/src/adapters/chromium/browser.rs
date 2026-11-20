//! Which Chromium-based browser a process or saved entry is, and where it keeps its profiles.
//!
//! Identification is purely name/path based (no file system access, it runs for every process
//! during discovery) and deliberately narrow: only the browsers' *main* executables are
//! claimed. Electron and other Chromium-embedding applications use their own executable names
//! (`code`, `slack`, `msedgewebview2.exe`...) and are never matched; Chromium child processes
//! (`--type=renderer`, `--type=gpu-process`...) are rejected as well.
//!
//! Paths are compared case-insensitively with `\` normalized to `/`, so Windows, macOS and
//! Linux layouts are all recognized whatever the host OS (which also keeps this testable).

use std::path::{Path, PathBuf};

use reframe_platform::{LocateQuery, OsKind, PackageInfo, PackageKind, ProcessInfo};

use crate::model::LaunchConfig;

/// The four browser families handled by the adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Browser {
    Chrome,
    Edge,
    Brave,
    Chromium,
}

impl Browser {
    pub const ALL: [Browser; 4] = [
        Browser::Chrome,
        Browser::Edge,
        Browser::Brave,
        Browser::Chromium,
    ];

    /// Machine-independent application id stored in workspaces.
    pub fn app_id(self) -> &'static str {
        match self {
            Browser::Chrome => "chrome",
            Browser::Edge => "edge",
            Browser::Brave => "brave",
            Browser::Chromium => "chromium",
        }
    }

    pub fn from_app_id(app_id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.app_id() == app_id)
    }

    /// Product name as shown in window titles and the UI.
    pub fn product_name(self) -> &'static str {
        match self {
            Browser::Chrome => "Google Chrome",
            Browser::Edge => "Microsoft Edge",
            Browser::Brave => "Brave",
            Browser::Chromium => "Chromium",
        }
    }
}

/// Release channel. Each channel is a separate installation with its own profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    Stable,
    Beta,
    /// Chrome Dev ("unstable" on Linux), Edge Dev, Brave Dev.
    Dev,
    /// Chrome/Edge Canary ("SxS" on Windows), Brave Nightly.
    Canary,
}

/// How the browser is installed (decides where its profiles live and how it is started).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallKind {
    /// Regular installation (installer, distribution package, `.app` bundle).
    Native,
    /// Flatpak with this application id.
    Flatpak(String),
    /// Snap with this name.
    Snap(String),
}

/// A specific browser installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Install {
    pub browser: Browser,
    pub channel: Channel,
    pub kind: InstallKind,
}

impl Install {
    pub fn native(browser: Browser, channel: Channel) -> Self {
        Self {
            browser,
            channel,
            kind: InstallKind::Native,
        }
    }

    /// "Google Chrome", "Microsoft Edge Beta", "Brave Nightly"...
    pub fn display_name(&self) -> String {
        let suffix = match (self.browser, self.channel) {
            (_, Channel::Stable) => "",
            (_, Channel::Beta) => " Beta",
            (_, Channel::Dev) => " Dev",
            (Browser::Brave, Channel::Canary) => " Nightly",
            (_, Channel::Canary) => " Canary",
        };
        format!("{}{suffix}", self.browser.product_name())
    }

    /// macOS bundle identifier of this browser and channel.
    pub fn mac_bundle_id(&self) -> Option<&'static str> {
        MAC_BUNDLES
            .iter()
            .find(|(_, b, c)| *b == self.browser && *c == self.channel)
            .map(|(id, _, _)| *id)
    }
}

use Browser::{Brave, Chrome, Chromium, Edge};
use Channel::{Beta, Canary, Dev, Stable};

const MAC_BUNDLES: &[(&str, Browser, Channel)] = &[
    ("com.google.Chrome", Chrome, Stable),
    ("com.google.Chrome.beta", Chrome, Beta),
    ("com.google.Chrome.dev", Chrome, Dev),
    ("com.google.Chrome.canary", Chrome, Canary),
    ("com.microsoft.edgemac", Edge, Stable),
    ("com.microsoft.edgemac.Beta", Edge, Beta),
    ("com.microsoft.edgemac.Dev", Edge, Dev),
    ("com.microsoft.edgemac.Canary", Edge, Canary),
    ("com.brave.Browser", Brave, Stable),
    ("com.brave.Browser.beta", Brave, Beta),
    ("com.brave.Browser.dev", Brave, Dev),
    ("com.brave.Browser.nightly", Brave, Canary),
    ("org.chromium.Chromium", Chromium, Stable),
];

const FLATPAKS: &[(&str, Browser, Channel)] = &[
    ("com.google.Chrome", Chrome, Stable),
    ("com.google.ChromeDev", Chrome, Dev),
    ("com.microsoft.Edge", Edge, Stable),
    ("com.microsoft.EdgeDev", Edge, Dev),
    ("com.brave.Browser", Brave, Stable),
    ("org.chromium.Chromium", Chromium, Stable),
    (
        "io.github.ungoogled_software.ungoogled_chromium",
        Chromium,
        Stable,
    ),
    ("com.github.Eloston.UngoogledChromium", Chromium, Stable),
];

const SNAPS: &[(&str, Browser)] = &[("chromium", Chromium), ("brave", Brave)];

/// Executable names that identify a browser on their own (Linux launchers, macOS bundles).
const NAMED_EXECUTABLES: &[(&str, Browser, Channel)] = &[
    ("google-chrome", Chrome, Stable),
    ("google-chrome-stable", Chrome, Stable),
    ("google-chrome-beta", Chrome, Beta),
    ("google-chrome-unstable", Chrome, Dev),
    ("google-chrome-canary", Chrome, Canary),
    ("microsoft-edge", Edge, Stable),
    ("microsoft-edge-stable", Edge, Stable),
    ("microsoft-edge-beta", Edge, Beta),
    ("microsoft-edge-dev", Edge, Dev),
    ("microsoft-edge-canary", Edge, Canary),
    ("brave-browser", Brave, Stable),
    ("brave-browser-stable", Brave, Stable),
    ("brave-browser-beta", Brave, Beta),
    ("brave-browser-dev", Brave, Dev),
    ("brave-browser-nightly", Brave, Canary),
    ("chromium", Chromium, Stable),
    ("chromium-browser", Chromium, Stable),
    // Linux truncates process names to 15 characters.
    ("chromium-browse", Chromium, Stable),
    ("chromium-freeworld", Chromium, Stable),
    // macOS: `<Name>.app/Contents/MacOS/<Name>`.
    ("google chrome", Chrome, Stable),
    ("google chrome beta", Chrome, Beta),
    ("google chrome dev", Chrome, Dev),
    ("google chrome canary", Chrome, Canary),
    ("microsoft edge", Edge, Stable),
    ("microsoft edge beta", Edge, Beta),
    ("microsoft edge dev", Edge, Dev),
    ("microsoft edge canary", Edge, Canary),
    ("brave browser", Brave, Stable),
    ("brave browser beta", Brave, Beta),
    ("brave browser dev", Brave, Dev),
    ("brave browser nightly", Brave, Canary),
];

/// Installation folders (lower case, `/` separators) that tell the channel of a generic
/// browser binary (`chrome.exe`, `msedge`, `brave`...). More specific entries first.
const INSTALL_FOLDERS: &[(&str, Browser, Channel)] = &[
    ("/google/chrome sxs/", Chrome, Canary),
    ("/google/chrome beta/", Chrome, Beta),
    ("/google/chrome dev/", Chrome, Dev),
    ("/google/chrome/", Chrome, Stable),
    ("/opt/google/chrome-canary/", Chrome, Canary),
    ("/opt/google/chrome-beta/", Chrome, Beta),
    ("/opt/google/chrome-unstable/", Chrome, Dev),
    ("/microsoft/edge sxs/", Edge, Canary),
    ("/microsoft/edge beta/", Edge, Beta),
    ("/microsoft/edge dev/", Edge, Dev),
    ("/microsoft/edge/", Edge, Stable),
    ("/opt/microsoft/msedge-canary/", Edge, Canary),
    ("/opt/microsoft/msedge-beta/", Edge, Beta),
    ("/opt/microsoft/msedge-dev/", Edge, Dev),
    ("/opt/microsoft/msedge/", Edge, Stable),
    ("/bravesoftware/brave-browser-nightly/", Brave, Canary),
    ("/bravesoftware/brave-browser-beta/", Brave, Beta),
    ("/bravesoftware/brave-browser-dev/", Brave, Dev),
    ("/bravesoftware/brave-browser/", Brave, Stable),
    ("/opt/brave.com/brave-nightly/", Brave, Canary),
    ("/opt/brave.com/brave-beta/", Brave, Beta),
    ("/opt/brave.com/brave-dev/", Brave, Dev),
    ("/opt/brave.com/brave/", Brave, Stable),
];

fn lookup(table: &[(&str, Browser, Channel)], key: &str) -> Option<(Browser, Channel)> {
    table
        .iter()
        .find(|(k, _, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, b, c)| (*b, *c))
}

/// Chromium child processes carry a `--type=` switch (renderer, GPU, utility, zygote,
/// crashpad handler...). Only the browser process is "the application".
pub fn is_child_process(command_line: &[String]) -> bool {
    command_line
        .iter()
        .skip(1)
        .map(|arg| arg.trim())
        .take_while(|arg| *arg != "--")
        .any(|arg| arg.starts_with("--type=") || arg.starts_with("-type="))
}

/// The browser installation `process` belongs to, if it is a browser main process.
pub fn identify_process(process: &ProcessInfo) -> Option<Install> {
    if is_child_process(&process.command_line) {
        return None;
    }
    if let Some(package) = &process.package {
        match package.kind {
            // A packaged process is the package's application: never guess from its binary.
            PackageKind::Flatpak | PackageKind::Snap | PackageKind::MacBundle => {
                return install_from_package(package);
            }
            PackageKind::WindowsPackage | PackageKind::AppImage => {}
        }
    }
    match &process.executable_path {
        Some(path) => classify_executable(
            &path.to_string_lossy(),
            process.product_name.as_deref(),
            true,
        ),
        None => classify_executable(&process.name, process.product_name.as_deref(), false),
    }
}

/// The installation behind a package (Flatpak id, Snap name, macOS bundle id).
pub fn install_from_package(package: &PackageInfo) -> Option<Install> {
    match package.kind {
        PackageKind::Flatpak => lookup(FLATPAKS, &package.id).map(|(browser, channel)| Install {
            browser,
            channel,
            kind: InstallKind::Flatpak(package.id.clone()),
        }),
        PackageKind::Snap => SNAPS
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(&package.id))
            .map(|(name, browser)| Install {
                browser: *browser,
                channel: Stable,
                kind: InstallKind::Snap((*name).to_string()),
            }),
        PackageKind::MacBundle => {
            lookup(MAC_BUNDLES, &package.id).map(|(b, c)| Install::native(b, c))
        }
        PackageKind::WindowsPackage | PackageKind::AppImage => None,
    }
}

/// The installation a saved entry launches: package first, then executable, then the
/// stable channel of the entry's application id.
pub fn install_from_launch(app_id: &str, launch: &LaunchConfig) -> Option<Install> {
    launch
        .package
        .as_ref()
        .and_then(install_from_package)
        .or_else(|| {
            launch
                .executable
                .as_ref()
                .and_then(|p| classify_executable(&p.to_string_lossy(), None, true))
        })
        .or_else(|| Browser::from_app_id(app_id).map(|b| Install::native(b, Stable)))
}

/// File names of browser main executables (lower case). Helpers (`chrome_crashpad_handler`,
/// `chrome_proxy.exe`, `msedgewebview2.exe`, "Google Chrome Helper"...) are not in the list.
fn is_browser_binary(file: &str) -> bool {
    matches!(
        file,
        "chrome.exe" | "chrome" | "msedge.exe" | "msedge" | "brave.exe" | "brave"
    ) || lookup(NAMED_EXECUTABLES, file).is_some()
}

/// Classify an executable path (or a bare process name when `has_path` is false).
pub fn classify_executable(
    raw: &str,
    product_name: Option<&str>,
    has_path: bool,
) -> Option<Install> {
    let path = raw.trim().replace('\\', "/").to_lowercase();
    let file = path.rsplit('/').next().unwrap_or_default();
    if !is_browser_binary(file) {
        return None;
    }

    // Snaps run from `/snap/<name>/<revision>/...` (launcher: `/snap/bin/<name>`).
    if let Some(rest) = path
        .strip_prefix("/snap/bin/")
        .or_else(|| path.strip_prefix("/snap/"))
    {
        let name = rest.split('/').next().unwrap_or_default();
        return SNAPS
            .iter()
            .find(|(snap, _)| *snap == name)
            .map(|(snap, browser)| Install {
                browser: *browser,
                channel: Stable,
                kind: InstallKind::Snap((*snap).to_string()),
            });
    }

    // A Flatpak installation path on the host names the application.
    if let Some((_, rest)) = path.split_once("/flatpak/app/") {
        let id = rest.split('/').next().unwrap_or_default();
        return FLATPAKS
            .iter()
            .find(|(fid, _, _)| fid.eq_ignore_ascii_case(id))
            .map(|(fid, browser, channel)| Install {
                browser: *browser,
                channel: *channel,
                kind: InstallKind::Flatpak((*fid).to_string()),
            });
    }

    let folder = INSTALL_FOLDERS
        .iter()
        .find(|(fragment, _, _)| path.contains(fragment))
        .map(|(_, b, c)| (*b, *c));
    let in_flatpak_sandbox = path.starts_with("/app/");
    let flatpak = |browser: Browser| {
        let id = FLATPAKS
            .iter()
            .find(|(_, b, c)| *b == browser && *c == Stable)
            .map(|(id, _, _)| (*id).to_string())
            .unwrap_or_default();
        Install {
            browser,
            channel: Stable,
            kind: InstallKind::Flatpak(id),
        }
    };
    let generic_binary = |browser: Browser| {
        if in_flatpak_sandbox {
            return Some(flatpak(browser));
        }
        let channel = folder
            .filter(|(b, _)| *b == browser)
            .map_or(Stable, |(_, c)| c);
        Some(Install::native(browser, channel))
    };

    match file {
        "chrome.exe" | "chrome" => {
            // `/google/chrome/` also covers Linux's `/opt/google/chrome/`.
            if let Some((Chrome, channel)) = folder {
                return Some(Install::native(Chrome, channel));
            }
            if in_flatpak_sandbox {
                return Some(flatpak(if path.contains("chromium") {
                    Chromium
                } else {
                    Chrome
                }));
            }
            let says_chrome =
                product_name.is_some_and(|p| p.to_lowercase().contains("google chrome"));
            // A bare name is most likely Google Chrome; a binary elsewhere (Chromium builds,
            // Chrome for Testing, portable builds) is a Chromium.
            if !has_path || says_chrome {
                Some(Install::native(Chrome, Stable))
            } else {
                Some(Install::native(Chromium, Stable))
            }
        }
        "msedge.exe" | "msedge" => generic_binary(Edge),
        "brave.exe" | "brave" => generic_binary(Brave),
        _ => lookup(NAMED_EXECUTABLES, file).map(|(browser, channel)| {
            if in_flatpak_sandbox {
                flatpak(browser)
            } else {
                Install::native(browser, channel)
            }
        }),
    }
}

/// Folders the platform keeps per-user data in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BaseDirs {
    pub home: Option<PathBuf>,
    /// Linux: `$XDG_CONFIG_HOME` or `~/.config`.
    pub config: Option<PathBuf>,
    /// Windows: `%LOCALAPPDATA%`.
    pub local_data: Option<PathBuf>,
}

impl BaseDirs {
    pub fn from_system() -> Self {
        Self {
            home: dirs::home_dir(),
            config: dirs::config_dir(),
            local_data: dirs::data_local_dir(),
        }
    }
}

/// User data directory relative to the platform's base folder, as path components.
fn relative_user_data_dir(
    browser: Browser,
    channel: Channel,
    os: OsKind,
) -> &'static [&'static str] {
    match os {
        OsKind::Windows => match (browser, channel) {
            (Chrome, Stable) => &["Google", "Chrome", "User Data"],
            (Chrome, Beta) => &["Google", "Chrome Beta", "User Data"],
            (Chrome, Dev) => &["Google", "Chrome Dev", "User Data"],
            (Chrome, Canary) => &["Google", "Chrome SxS", "User Data"],
            (Edge, Stable) => &["Microsoft", "Edge", "User Data"],
            (Edge, Beta) => &["Microsoft", "Edge Beta", "User Data"],
            (Edge, Dev) => &["Microsoft", "Edge Dev", "User Data"],
            (Edge, Canary) => &["Microsoft", "Edge SxS", "User Data"],
            (Brave, Stable) => &["BraveSoftware", "Brave-Browser", "User Data"],
            (Brave, Beta) => &["BraveSoftware", "Brave-Browser-Beta", "User Data"],
            (Brave, Dev) => &["BraveSoftware", "Brave-Browser-Dev", "User Data"],
            (Brave, Canary) => &["BraveSoftware", "Brave-Browser-Nightly", "User Data"],
            (Chromium, _) => &["Chromium", "User Data"],
        },
        OsKind::Macos => match (browser, channel) {
            (Chrome, Stable) => &["Google", "Chrome"],
            (Chrome, Beta) => &["Google", "Chrome Beta"],
            (Chrome, Dev) => &["Google", "Chrome Dev"],
            (Chrome, Canary) => &["Google", "Chrome Canary"],
            (Edge, Stable) => &["Microsoft Edge"],
            (Edge, Beta) => &["Microsoft Edge Beta"],
            (Edge, Dev) => &["Microsoft Edge Dev"],
            (Edge, Canary) => &["Microsoft Edge Canary"],
            (Brave, Stable) => &["BraveSoftware", "Brave-Browser"],
            (Brave, Beta) => &["BraveSoftware", "Brave-Browser-Beta"],
            (Brave, Dev) => &["BraveSoftware", "Brave-Browser-Dev"],
            (Brave, Canary) => &["BraveSoftware", "Brave-Browser-Nightly"],
            (Chromium, _) => &["Chromium"],
        },
        OsKind::Linux | OsKind::Other => match (browser, channel) {
            (Chrome, Stable) => &["google-chrome"],
            (Chrome, Beta) => &["google-chrome-beta"],
            (Chrome, Dev) => &["google-chrome-unstable"],
            (Chrome, Canary) => &["google-chrome-canary"],
            (Edge, Stable) => &["microsoft-edge"],
            (Edge, Beta) => &["microsoft-edge-beta"],
            (Edge, Dev) => &["microsoft-edge-dev"],
            (Edge, Canary) => &["microsoft-edge-canary"],
            (Brave, Stable) => &["BraveSoftware", "Brave-Browser"],
            (Brave, Beta) => &["BraveSoftware", "Brave-Browser-Beta"],
            (Brave, Dev) => &["BraveSoftware", "Brave-Browser-Dev"],
            (Brave, Canary) => &["BraveSoftware", "Brave-Browser-Nightly"],
            (Chromium, _) => &["chromium"],
        },
    }
}

fn join(root: &Path, parts: &[&str]) -> PathBuf {
    let mut path = root.to_path_buf();
    path.extend(parts);
    path
}

/// Where `install` keeps its user data directory on `os`, most likely first. The caller picks
/// the first one that exists.
pub fn default_user_data_dirs(install: &Install, os: OsKind, base: &BaseDirs) -> Vec<PathBuf> {
    let linux = relative_user_data_dir(install.browser, install.channel, OsKind::Linux);
    let mut out = Vec::new();
    match &install.kind {
        InstallKind::Flatpak(id) => {
            if let Some(home) = &base.home {
                out.push(join(
                    &home.join(".var").join("app").join(id).join("config"),
                    linux,
                ));
            }
        }
        InstallKind::Snap(name) => {
            if let Some(home) = &base.home {
                let snap = home.join("snap").join(name);
                if install.browser == Chromium {
                    // The Chromium snap sets CHROME_CONFIG_HOME to $SNAP_USER_COMMON.
                    out.push(snap.join("common").join("chromium"));
                }
                for revision in ["current", "common"] {
                    out.push(join(&snap.join(revision).join(".config"), linux));
                }
            }
        }
        InstallKind::Native => {
            let rel = relative_user_data_dir(install.browser, install.channel, os);
            match os {
                OsKind::Windows => {
                    if let Some(local) = &base.local_data {
                        out.push(join(local, rel));
                    }
                }
                OsKind::Macos => {
                    if let Some(home) = &base.home {
                        out.push(join(&home.join("Library").join("Application Support"), rel));
                    }
                }
                OsKind::Linux | OsKind::Other => {
                    if let Some(config) = &base.config {
                        out.push(join(config, rel));
                    }
                    if let Some(home) = &base.home {
                        let fallback = join(&home.join(".config"), rel);
                        if !out.contains(&fallback) {
                            out.push(fallback);
                        }
                    }
                }
            }
        }
    }
    out
}

/// Distribution launcher scripts preferred over the raw binary on Linux: they set up the
/// environment the browser expects (bundled xdg tools, distribution flags) exactly like the
/// desktop entry does. Candidates only; the caller checks they exist.
pub fn linux_launcher_candidates(executable: &Path) -> Vec<PathBuf> {
    let Some(dir) = executable.parent() else {
        return Vec::new();
    };
    let file = executable
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir_name = dir
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let parent = dir.parent().map(Path::to_path_buf).unwrap_or_default();
    let wrapper = match (parent.to_str(), file.as_str()) {
        (Some("/opt/google"), "chrome") => Some(format!("google-{dir_name}")),
        (Some("/opt/microsoft"), "msedge") => dir_name
            .strip_prefix("msedge")
            .map(|channel| format!("microsoft-edge{channel}")),
        (Some("/opt/brave.com"), "brave") => dir_name
            .strip_prefix("brave")
            .map(|channel| format!("brave-browser{channel}")),
        _ => None,
    };
    if let Some(wrapper) = wrapper {
        return vec![dir.join(wrapper)];
    }
    match executable.to_str() {
        Some("/usr/lib/chromium/chromium") => vec![PathBuf::from("/usr/bin/chromium")],
        Some(
            "/usr/lib/chromium-browser/chromium-browser"
            | "/usr/lib64/chromium-browser/chromium-browser",
        ) => vec![PathBuf::from("/usr/bin/chromium-browser")],
        _ => Vec::new(),
    }
}

/// The `.app` bundle containing a macOS executable (`/Applications/X.app/Contents/MacOS/X`).
pub fn mac_bundle_path(executable: &Path) -> Option<PathBuf> {
    let text = executable.to_str()?;
    let end = text.find(".app/Contents/MacOS/")?;
    Some(PathBuf::from(&text[..end + ".app".len()]))
}

/// Names under which a browser is installed.
struct LocateData {
    windows_executables: &'static [&'static str],
    unix_executables: &'static [&'static str],
    mac_executables: &'static [&'static str],
    bundle_ids: &'static [&'static str],
    desktop_ids: &'static [&'static str],
    display_names: &'static [&'static str],
}

fn locate_data(browser: Browser) -> LocateData {
    match browser {
        Chrome => LocateData {
            windows_executables: &["chrome.exe"],
            unix_executables: &[
                "google-chrome-stable",
                "google-chrome",
                "google-chrome-beta",
                "google-chrome-unstable",
            ],
            mac_executables: &["Google Chrome"],
            bundle_ids: &["com.google.Chrome"],
            desktop_ids: &[
                "google-chrome.desktop",
                "com.google.Chrome.desktop",
                "google-chrome-beta.desktop",
                "google-chrome-unstable.desktop",
            ],
            display_names: &["Google Chrome"],
        },
        Edge => LocateData {
            windows_executables: &["msedge.exe"],
            unix_executables: &[
                "microsoft-edge-stable",
                "microsoft-edge",
                "microsoft-edge-beta",
                "microsoft-edge-dev",
            ],
            mac_executables: &["Microsoft Edge"],
            bundle_ids: &["com.microsoft.edgemac"],
            desktop_ids: &[
                "microsoft-edge.desktop",
                "com.microsoft.Edge.desktop",
                "microsoft-edge-beta.desktop",
                "microsoft-edge-dev.desktop",
            ],
            display_names: &["Microsoft Edge"],
        },
        Brave => LocateData {
            windows_executables: &["brave.exe"],
            unix_executables: &[
                "brave-browser",
                "brave-browser-stable",
                "brave",
                "brave-browser-beta",
                "brave-browser-nightly",
            ],
            mac_executables: &["Brave Browser"],
            bundle_ids: &["com.brave.Browser"],
            desktop_ids: &[
                "brave-browser.desktop",
                "com.brave.Browser.desktop",
                "brave_brave.desktop",
            ],
            display_names: &["Brave Browser", "Brave"],
        },
        Chromium => LocateData {
            // Chromium builds for Windows also ship as `chrome.exe`.
            windows_executables: &["chrome.exe"],
            unix_executables: &["chromium", "chromium-browser"],
            mac_executables: &["Chromium"],
            bundle_ids: &["org.chromium.Chromium"],
            desktop_ids: &[
                "chromium.desktop",
                "chromium-browser.desktop",
                "org.chromium.Chromium.desktop",
                "chromium_chromium.desktop",
            ],
            display_names: &["Chromium"],
        },
    }
}

/// How to find `browser` on a machine running `os`.
pub fn locate_query(browser: Browser, os: OsKind) -> LocateQuery {
    let data = locate_data(browser);
    let executables = match os {
        OsKind::Windows => data.windows_executables,
        OsKind::Macos => data.mac_executables,
        OsKind::Linux | OsKind::Other => data.unix_executables,
    };
    let owned = |items: &[&str]| items.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
    LocateQuery {
        executable_names: owned(executables),
        bundle_ids: owned(data.bundle_ids),
        desktop_ids: owned(data.desktop_ids),
        display_names: owned(data.display_names),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(exe: Option<&str>, name: &str, cmd: &[&str]) -> ProcessInfo {
        ProcessInfo {
            pid: 1,
            parent_pid: None,
            name: name.into(),
            product_name: None,
            executable_path: exe.map(PathBuf::from),
            command_line: cmd.iter().map(|s| s.to_string()).collect(),
            working_directory: None,
            start_time: None,
            package: None,
        }
    }

    fn identify(exe: &str) -> Option<(Browser, Channel, InstallKind)> {
        identify_process(&process(Some(exe), "x", &[exe])).map(|i| (i.browser, i.channel, i.kind))
    }

    fn native(b: Browser, c: Channel) -> Option<(Browser, Channel, InstallKind)> {
        Some((b, c, InstallKind::Native))
    }

    #[test]
    fn identifies_windows_installations() {
        let cases = [
            (
                r"C:\Program Files\Google\Chrome\Application\chrome.exe",
                native(Chrome, Stable),
            ),
            (
                r"C:\Users\me\AppData\Local\Google\Chrome\Application\chrome.exe",
                native(Chrome, Stable),
            ),
            (
                r"C:\Program Files\Google\Chrome Beta\Application\chrome.exe",
                native(Chrome, Beta),
            ),
            (
                r"C:\Users\me\AppData\Local\Google\Chrome SxS\Application\chrome.exe",
                native(Chrome, Canary),
            ),
            (
                r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
                native(Edge, Stable),
            ),
            (
                r"C:\Program Files (x86)\Microsoft\Edge Dev\Application\msedge.exe",
                native(Edge, Dev),
            ),
            (
                r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe",
                native(Brave, Stable),
            ),
            (
                r"C:\Program Files\BraveSoftware\Brave-Browser-Nightly\Application\brave.exe",
                native(Brave, Canary),
            ),
            (
                r"C:\Users\me\AppData\Local\Chromium\Application\chrome.exe",
                native(Chromium, Stable),
            ),
            (
                r"D:\tools\chrome-win64\chrome.exe",
                native(Chromium, Stable),
            ),
            (
                r"C:\Program Files (x86)\Microsoft\EdgeWebView\Application\120.0\msedgewebview2.exe",
                None,
            ),
            (
                r"C:\Program Files\Google\Chrome\Application\chrome_proxy.exe",
                None,
            ),
            (
                r"C:\Users\me\AppData\Local\Programs\Microsoft VS Code\Code.exe",
                None,
            ),
        ];
        for (exe, expected) in cases {
            assert_eq!(identify(exe), expected, "{exe}");
        }
    }

    #[test]
    fn identifies_linux_installations() {
        let cases = [
            ("/opt/google/chrome/chrome", native(Chrome, Stable)),
            ("/opt/google/chrome-beta/chrome", native(Chrome, Beta)),
            ("/opt/google/chrome-unstable/chrome", native(Chrome, Dev)),
            ("/usr/bin/google-chrome-stable", native(Chrome, Stable)),
            ("/opt/microsoft/msedge/msedge", native(Edge, Stable)),
            ("/opt/microsoft/msedge-beta/msedge", native(Edge, Beta)),
            ("/usr/bin/microsoft-edge-dev", native(Edge, Dev)),
            ("/opt/brave.com/brave/brave", native(Brave, Stable)),
            ("/opt/brave.com/brave-nightly/brave", native(Brave, Canary)),
            ("/usr/lib/chromium/chromium", native(Chromium, Stable)),
            (
                "/usr/lib64/chromium-browser/chromium-browser",
                native(Chromium, Stable),
            ),
            (
                "/snap/chromium/2890/usr/lib/chromium-browser/chrome",
                Some((Chromium, Stable, InstallKind::Snap("chromium".into()))),
            ),
            (
                "/snap/brave/412/opt/brave.com/brave/brave",
                Some((Brave, Stable, InstallKind::Snap("brave".into()))),
            ),
            (
                "/app/extra/chrome",
                Some((
                    Chrome,
                    Stable,
                    InstallKind::Flatpak("com.google.Chrome".into()),
                )),
            ),
            (
                "/app/chromium/chrome",
                Some((
                    Chromium,
                    Stable,
                    InstallKind::Flatpak("org.chromium.Chromium".into()),
                )),
            ),
            (
                "/var/lib/flatpak/app/com.brave.Browser/x86_64/stable/active/files/brave/brave",
                Some((
                    Brave,
                    Stable,
                    InstallKind::Flatpak("com.brave.Browser".into()),
                )),
            ),
            (
                "/home/me/.cache/ms-playwright/chromium-1091/chrome-linux/chrome",
                native(Chromium, Stable),
            ),
            ("/opt/google/chrome/chrome_crashpad_handler", None),
            (
                "/snap/chromium/2890/usr/lib/chromium-browser/chrome_crashpad_handler",
                None,
            ),
            (
                "/var/lib/flatpak/app/com.google.Chrome/x86_64/stable/active/files/extra/nacl_helper",
                None,
            ),
            ("/usr/share/code/code", None),
            ("/usr/lib/slack/slack", None),
            ("/snap/firefox/123/usr/lib/firefox/firefox", None),
        ];
        for (exe, expected) in cases {
            assert_eq!(identify(exe), expected, "{exe}");
        }
    }

    #[test]
    fn identifies_macos_installations() {
        let cases = [
            (
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
                native(Chrome, Stable),
            ),
            (
                "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
                native(Chrome, Canary),
            ),
            (
                "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
                native(Edge, Stable),
            ),
            (
                "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
                native(Brave, Stable),
            ),
            (
                "/Applications/Chromium.app/Contents/MacOS/Chromium",
                native(Chromium, Stable),
            ),
            (
                "/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/Versions/120.0/Helpers/Google Chrome Helper (Renderer).app/Contents/MacOS/Google Chrome Helper (Renderer)",
                None,
            ),
            ("/Applications/Slack.app/Contents/MacOS/Slack", None),
        ];
        for (exe, expected) in cases {
            assert_eq!(identify(exe), expected, "{exe}");
        }
    }

    #[test]
    fn packages_decide_for_packaged_processes() {
        let mut p = process(Some("/app/extra/chrome"), "chrome", &["chrome"]);
        p.package = Some(PackageInfo {
            kind: PackageKind::Flatpak,
            id: "com.microsoft.Edge".into(),
            path: None,
        });
        let install = identify_process(&p).unwrap();
        assert_eq!(install.browser, Edge);
        assert_eq!(
            install.kind,
            InstallKind::Flatpak("com.microsoft.Edge".into())
        );

        p.package = Some(PackageInfo {
            kind: PackageKind::MacBundle,
            id: "com.microsoft.edgemac.Beta".into(),
            path: None,
        });
        assert_eq!(identify_process(&p).unwrap(), Install::native(Edge, Beta));

        // An unrelated package is not a browser, whatever its binary is called.
        p.package = Some(PackageInfo {
            kind: PackageKind::Flatpak,
            id: "com.slack.Slack".into(),
            path: None,
        });
        assert_eq!(identify_process(&p), None);
    }

    #[test]
    fn child_processes_and_name_only_processes() {
        let renderer = process(
            Some("/opt/google/chrome/chrome"),
            "chrome",
            &["/opt/google/chrome/chrome", "--type=renderer", "--lang=en"],
        );
        assert_eq!(identify_process(&renderer), None);
        // `--type=` after `--` is a URL argument, not a switch.
        let main = process(
            Some("/opt/google/chrome/chrome"),
            "chrome",
            &["/opt/google/chrome/chrome", "--", "--type=renderer"],
        );
        assert!(identify_process(&main).is_some());
        // Without an executable path the process name is used.
        let name_only = process(None, "msedge.exe", &[]);
        assert_eq!(
            identify_process(&name_only),
            Some(Install::native(Edge, Stable))
        );
        let name_only = process(None, "chrome.exe", &[]);
        assert_eq!(
            identify_process(&name_only),
            Some(Install::native(Chrome, Stable))
        );
        let mut with_product = process(
            Some(r"D:\Portable\App\Chrome-bin\chrome.exe"),
            "chrome.exe",
            &[],
        );
        with_product.product_name = Some("Google Chrome".into());
        assert_eq!(
            identify_process(&with_product),
            Some(Install::native(Chrome, Stable))
        );
    }

    #[test]
    fn user_data_dirs_per_platform() {
        let base = BaseDirs {
            home: Some("/home/me".into()),
            config: Some("/home/me/.config".into()),
            local_data: Some("/win/AppData/Local".into()),
        };
        let first = |install: Install, os| default_user_data_dirs(&install, os, &base)[0].clone();
        assert_eq!(
            first(Install::native(Chrome, Stable), OsKind::Windows),
            PathBuf::from("/win/AppData/Local/Google/Chrome/User Data")
        );
        assert_eq!(
            first(Install::native(Chrome, Canary), OsKind::Windows),
            PathBuf::from("/win/AppData/Local/Google/Chrome SxS/User Data")
        );
        assert_eq!(
            first(Install::native(Edge, Stable), OsKind::Windows),
            PathBuf::from("/win/AppData/Local/Microsoft/Edge/User Data")
        );
        assert_eq!(
            first(Install::native(Brave, Stable), OsKind::Windows),
            PathBuf::from("/win/AppData/Local/BraveSoftware/Brave-Browser/User Data")
        );
        assert_eq!(
            first(Install::native(Chromium, Stable), OsKind::Windows),
            PathBuf::from("/win/AppData/Local/Chromium/User Data")
        );
        assert_eq!(
            first(Install::native(Chrome, Stable), OsKind::Macos),
            PathBuf::from("/home/me/Library/Application Support/Google/Chrome")
        );
        assert_eq!(
            first(Install::native(Edge, Stable), OsKind::Macos),
            PathBuf::from("/home/me/Library/Application Support/Microsoft Edge")
        );
        assert_eq!(
            first(Install::native(Brave, Stable), OsKind::Macos),
            PathBuf::from("/home/me/Library/Application Support/BraveSoftware/Brave-Browser")
        );
        assert_eq!(
            first(Install::native(Chrome, Beta), OsKind::Linux),
            PathBuf::from("/home/me/.config/google-chrome-beta")
        );
        assert_eq!(
            first(Install::native(Edge, Stable), OsKind::Linux),
            PathBuf::from("/home/me/.config/microsoft-edge")
        );
        assert_eq!(
            first(Install::native(Chromium, Stable), OsKind::Linux),
            PathBuf::from("/home/me/.config/chromium")
        );
        let snap = Install {
            browser: Chromium,
            channel: Stable,
            kind: InstallKind::Snap("chromium".into()),
        };
        assert_eq!(
            first(snap, OsKind::Linux),
            PathBuf::from("/home/me/snap/chromium/common/chromium")
        );
        let flatpak = Install {
            browser: Brave,
            channel: Stable,
            kind: InstallKind::Flatpak("com.brave.Browser".into()),
        };
        assert_eq!(
            first(flatpak, OsKind::Linux),
            PathBuf::from("/home/me/.var/app/com.brave.Browser/config/BraveSoftware/Brave-Browser")
        );
    }

    #[test]
    fn launcher_scripts_and_bundles() {
        let c = |p: &str| linux_launcher_candidates(Path::new(p));
        assert_eq!(
            c("/opt/google/chrome/chrome"),
            [PathBuf::from("/opt/google/chrome/google-chrome")]
        );
        assert_eq!(
            c("/opt/google/chrome-beta/chrome"),
            [PathBuf::from("/opt/google/chrome-beta/google-chrome-beta")]
        );
        assert_eq!(
            c("/opt/microsoft/msedge-dev/msedge"),
            [PathBuf::from(
                "/opt/microsoft/msedge-dev/microsoft-edge-dev"
            )]
        );
        assert_eq!(
            c("/opt/brave.com/brave/brave"),
            [PathBuf::from("/opt/brave.com/brave/brave-browser")]
        );
        assert_eq!(
            c("/usr/lib/chromium/chromium"),
            [PathBuf::from("/usr/bin/chromium")]
        );
        assert!(c("/home/me/bin/chrome").is_empty());
        assert_eq!(
            mac_bundle_path(Path::new(
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
            )),
            Some(PathBuf::from("/Applications/Google Chrome.app"))
        );
        assert_eq!(mac_bundle_path(Path::new("/usr/bin/chromium")), None);
        assert_eq!(
            Install::native(Edge, Canary).mac_bundle_id(),
            Some("com.microsoft.edgemac.Canary")
        );
    }

    #[test]
    fn install_from_saved_launch_configuration() {
        let launch = LaunchConfig {
            executable: Some(r"C:\Program Files\Google\Chrome Beta\Application\chrome.exe".into()),
            ..Default::default()
        };
        assert_eq!(
            install_from_launch("chrome", &launch),
            Some(Install::native(Chrome, Beta))
        );
        assert_eq!(
            install_from_launch("edge", &LaunchConfig::default()),
            Some(Install::native(Edge, Stable))
        );
        assert_eq!(
            install_from_launch("exe:gimp", &LaunchConfig::default()),
            None
        );
    }

    #[test]
    fn display_names_and_locate_queries() {
        assert_eq!(
            Install::native(Brave, Canary).display_name(),
            "Brave Nightly"
        );
        assert_eq!(
            Install::native(Chrome, Dev).display_name(),
            "Google Chrome Dev"
        );
        let q = locate_query(Edge, OsKind::Windows);
        assert_eq!(q.executable_names, ["msedge.exe"]);
        assert!(q.bundle_ids.contains(&"com.microsoft.edgemac".to_string()));
        assert!(
            q.desktop_ids
                .contains(&"microsoft-edge.desktop".to_string())
        );
        let q = locate_query(Chrome, OsKind::Linux);
        assert!(
            q.executable_names
                .contains(&"google-chrome-stable".to_string())
        );
        assert!(
            q.desktop_ids
                .contains(&"com.google.Chrome.desktop".to_string())
        );
        let q = locate_query(Chromium, OsKind::Macos);
        assert_eq!(q.executable_names, ["Chromium"]);
        assert!(
            locate_query(Brave, OsKind::Linux)
                .desktop_ids
                .contains(&"brave_brave.desktop".to_string())
        );
    }
}
