//! Platform data types shared by every OS implementation.
//!
//! All coordinates are in the *virtual desktop* coordinate space of the platform:
//! - Windows: physical pixels (the app is per-monitor DPI aware), origin at the primary
//!   monitor's top-left corner; other monitors may have negative coordinates.
//! - macOS: global display coordinates in points, origin at the top-left of the main display
//!   (the CoreGraphics / Accessibility convention, *not* the flipped AppKit convention).
//! - Linux/X11: root-window pixels, origin at the top-left of the X screen.
//!
//! Types that cross the Tauri boundary derive `ts_rs::TS` so the frontend gets generated
//! TypeScript definitions. Avoid 64-bit integers in these types (they map to `bigint`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Axis-aligned rectangle in virtual-desktop coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub const fn right(&self) -> i32 {
        self.x + self.width
    }

    pub const fn bottom(&self) -> i32 {
        self.y + self.height
    }

    pub fn area(&self) -> i64 {
        i64::from(self.width.max(0)) * i64::from(self.height.max(0))
    }

    pub fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0
    }

    pub fn center(&self) -> (i32, i32) {
        (self.x + self.width / 2, self.y + self.height / 2)
    }

    pub fn contains_point(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    pub fn intersection(&self, other: &Rect) -> Option<Rect> {
        let x1 = self.x.max(other.x);
        let y1 = self.y.max(other.y);
        let x2 = self.right().min(other.right());
        let y2 = self.bottom().min(other.bottom());
        (x2 > x1 && y2 > y1).then(|| Rect::new(x1, y1, x2 - x1, y2 - y1))
    }

    pub fn intersection_area(&self, other: &Rect) -> i64 {
        self.intersection(other).map_or(0, |r| r.area())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum OsKind {
    Windows,
    Macos,
    Linux,
    Other,
}

impl OsKind {
    pub const fn current() -> Self {
        if cfg!(target_os = "windows") {
            OsKind::Windows
        } else if cfg!(target_os = "macos") {
            OsKind::Macos
        } else if cfg!(target_os = "linux") {
            OsKind::Linux
        } else {
            OsKind::Other
        }
    }
}

/// The windowing system Reframe is talking to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DisplayServer {
    Win32,
    Quartz,
    X11,
    /// A Wayland session where Reframe has no X11 access at all.
    Wayland,
    /// A Wayland session where Reframe talks to XWayland: only X11 clients are visible.
    XWayland,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SupportLevel {
    Full,
    Partial,
    Unsupported,
}

/// Whether one platform operation is available, with an explanation when it is limited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Capability {
    pub level: SupportLevel,
    pub note: Option<String>,
}

impl Capability {
    pub fn full() -> Self {
        Self {
            level: SupportLevel::Full,
            note: None,
        }
    }

    pub fn partial(note: impl Into<String>) -> Self {
        Self {
            level: SupportLevel::Partial,
            note: Some(note.into()),
        }
    }

    pub fn unsupported(note: impl Into<String>) -> Self {
        Self {
            level: SupportLevel::Unsupported,
            note: Some(note.into()),
        }
    }

    pub fn is_available(&self) -> bool {
        self.level != SupportLevel::Unsupported
    }
}

/// What the current platform implementation can do. Reported to the UI so limitations are
/// explained instead of hidden.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlatformCapabilities {
    pub list_windows: Capability,
    pub read_window_geometry: Capability,
    pub move_windows: Capability,
    /// Maximize / minimize / restore.
    pub change_window_state: Capability,
    /// Graceful close requests (the application may still prompt to save work).
    pub close_windows: Capability,
    pub focus_windows: Capability,
    pub list_monitors: Capability,
    /// Command line and working directory of other processes.
    pub process_details: Capability,
    pub application_icons: Capability,
}

impl PlatformCapabilities {
    pub fn all_unsupported(note: &str) -> Self {
        Self {
            list_windows: Capability::unsupported(note),
            read_window_geometry: Capability::unsupported(note),
            move_windows: Capability::unsupported(note),
            change_window_state: Capability::unsupported(note),
            close_windows: Capability::unsupported(note),
            focus_windows: Capability::unsupported(note),
            list_monitors: Capability::unsupported(note),
            process_details: Capability::partial("Basic process information only."),
            application_icons: Capability::unsupported(note),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlatformInfo {
    pub os: OsKind,
    /// Human readable, e.g. "Windows 11 (26100)", "Ubuntu 24.04", "macOS 15.1".
    pub os_version: String,
    pub display_server: DisplayServer,
    /// e.g. "GNOME", "KDE", "XFCE" on Linux; `None` elsewhere.
    pub desktop_environment: Option<String>,
    pub capabilities: PlatformCapabilities,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MonitorInfo {
    /// Identifier that is as stable as the platform allows across reboots
    /// (Windows device path / EDID-derived id, macOS display UUID, X11 RandR output name).
    pub id: String,
    /// Friendly name for the UI ("DELL U2720Q", "Built-in Retina Display", "HDMI-1").
    pub name: String,
    pub bounds: Rect,
    /// Bounds minus taskbars, docks and panels.
    pub work_area: Rect,
    pub scale_factor: f64,
    pub is_primary: bool,
}

/// Opaque, platform-specific window identifier (HWND, X11 window id, CGWindowID...).
/// Only valid while the window exists.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(transparent)]
#[ts(export)]
pub struct WindowId(pub String);

impl WindowId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WindowId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum WindowState {
    #[default]
    Normal,
    Maximized,
    Minimized,
    Fullscreen,
}

/// A visible, top-level *application* window. Platform implementations must filter out
/// desktop/background windows, docks, panels, tooltips, menus, invisible/cloaked windows and
/// zero-sized windows. A running process with no such window is not an "application window".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WindowInfo {
    pub id: WindowId,
    pub pid: u32,
    pub title: String,
    /// Win32 window class, X11 `WM_CLASS` class part, macOS bundle identifier.
    pub class_name: Option<String>,
    /// Extra application identity hint: Windows AppUserModelID, X11 `_GTK_APPLICATION_ID` /
    /// `WM_CLASS` instance, macOS bundle id.
    pub app_id_hint: Option<String>,
    /// Current outer frame bounds.
    pub bounds: Rect,
    /// Bounds the window returns to when un-maximized / un-minimized, when the platform
    /// exposes it (Windows does; X11 and macOS generally do not).
    pub normal_bounds: Option<Rect>,
    pub state: WindowState,
    pub monitor_id: Option<String>,
    /// Virtual desktop index where exposed (X11 `_NET_WM_DESKTOP`).
    pub desktop: Option<u32>,
    pub is_focused: bool,
    /// Stacking order, 0 = frontmost.
    pub z_order: u32,
}

impl WindowInfo {
    /// The geometry to save for later restoration.
    pub fn restore_bounds(&self) -> Rect {
        self.normal_bounds.unwrap_or(self.bounds)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PackageKind {
    /// macOS `.app` bundle; `id` is the bundle identifier.
    MacBundle,
    /// Windows packaged (MSIX/Store) app; `id` is the AppUserModelID.
    WindowsPackage,
    /// Flatpak; `id` is the application id.
    Flatpak,
    /// Snap; `id` is the snap name.
    Snap,
    /// AppImage; `path` is the .AppImage file.
    AppImage,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PackageInfo {
    pub kind: PackageKind,
    pub id: String,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: Option<u32>,
    /// Process (image) name, e.g. `Code.exe`, `gnome-terminal-server`.
    pub name: String,
    /// Friendly product name where the OS exposes it (Windows FileDescription, macOS
    /// localizedName, Linux `.desktop` Name). `None` when unknown.
    pub product_name: Option<String>,
    pub executable_path: Option<PathBuf>,
    /// Empty when the platform does not allow reading it.
    pub command_line: Vec<String>,
    pub working_directory: Option<PathBuf>,
    /// Unix timestamp (seconds).
    #[ts(type = "number | null")]
    pub start_time: Option<u64>,
    pub package: Option<PackageInfo>,
}

/// What to start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum LaunchTarget {
    /// Spawn an executable directly.
    Executable { path: PathBuf },
    /// Launch through the OS package mechanism (`open -a`, `shell:AppsFolder\AUMID`,
    /// `flatpak run`, `snap run`).
    Package { package: PackageInfo },
    /// Open a file, folder or URL with the system default handler.
    Open { target: String },
}

impl LaunchTarget {
    pub fn label(&self) -> String {
        match self {
            LaunchTarget::Executable { path } => path.display().to_string(),
            LaunchTarget::Package { package } => package.id.clone(),
            LaunchTarget::Open { target } => target.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LaunchRequest {
    pub target: LaunchTarget,
    pub args: Vec<String>,
    pub working_directory: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

impl LaunchRequest {
    pub fn executable(path: impl Into<PathBuf>) -> Self {
        Self {
            target: LaunchTarget::Executable { path: path.into() },
            args: Vec::new(),
            working_directory: None,
            env: Vec::new(),
        }
    }

    pub fn with_args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_working_directory(mut self, dir: Option<PathBuf>) -> Self {
        self.working_directory = dir;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LaunchedProcess {
    /// `None` when the launch went through an intermediary (`open`, shell activation,
    /// `xdg-open`) that does not report the final process id.
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WindowPlacement {
    /// Normal (restored) bounds. For `Maximized`/`Fullscreen` the window is first moved here
    /// so it maximizes on the right monitor.
    pub bounds: Rect,
    pub state: WindowState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PermissionKind {
    /// macOS Accessibility: required to read window titles/geometry and move windows.
    Accessibility,
    /// macOS Screen Recording: required to read other apps' window titles.
    ScreenRecording,
    /// macOS Automation (Apple Events): required for AppleScript-based integrations.
    Automation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PermissionStatus {
    pub kind: PermissionKind,
    pub granted: bool,
    /// What Reframe uses the permission for.
    pub purpose: String,
    /// How the user grants it.
    pub instructions: String,
}

/// Hints for finding an installed application on this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LocateQuery {
    /// Executable file names, e.g. `["code", "Code.exe"]`. Matched case-insensitively on Windows.
    pub executable_names: Vec<String>,
    /// macOS bundle identifiers, e.g. `["com.microsoft.VSCode"]`.
    pub bundle_ids: Vec<String>,
    /// Linux desktop entry ids, e.g. `["code.desktop"]`.
    pub desktop_ids: Vec<String>,
    /// Display names, e.g. `["Visual Studio Code"]` (used for `.app` bundles, Start menu).
    pub display_names: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IconRequest {
    pub executable_path: Option<PathBuf>,
    pub package: Option<PackageInfo>,
    pub window: Option<WindowId>,
    /// Desired edge length in pixels (implementations may return a different size).
    pub size: u32,
}
