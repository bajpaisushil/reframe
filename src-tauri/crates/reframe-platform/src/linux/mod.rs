//! Linux implementation: X11/EWMH via `x11rb`, Wayland detection.
//!
//! What works where:
//!
//! | Session | Windows & monitors | Launching, processes, icons, app lookup |
//! |---------|--------------------|-----------------------------------------|
//! | X11 with an EWMH window manager | full | full |
//! | X11 without an EWMH window manager | listing/moving via ICCCM; no maximize/full screen | full |
//! | Wayland with XWayland (`DISPLAY` reachable) | only X11/XWayland applications are visible | full |
//! | Wayland without XWayland | unsupported (Wayland forbids it) | full |
//! | No graphical session | unsupported | full |
//!
//! The X connection is opened lazily and re-established after the X server restarts; a
//! missing or dead X server yields [`PlatformError`]s, never panics.
//!
//! Known limitations (also reported through [`PlatformInfo::capabilities`]):
//! - Window pids come from the X-Resource extension or `_NET_WM_PID`; `pid` is `0` when
//!   neither is available (e.g. windows of remote X clients).
//! - X11 does not expose the restore geometry of maximized/minimized windows, so
//!   `normal_bounds` is only reported for normal windows.
//! - Monitor scale is the global `Xft.dpi` (X11 has no per-monitor scale).
//! - Icons: PNG only (window icons, `hicolor` theme, pixmaps); SVG-only icons give `None`.
//! - Window managers may constrain placement (size increments, keeping windows on screen) or
//!   refuse focus changes; requests are best effort and verified where cheap.

mod apps;
mod desktop_entry;
mod ewmh;
mod geometry;
mod icons;
mod launch;
mod packages;
mod session;
#[cfg(test)]
mod testutil;
mod x11;
mod xdg;

use std::path::PathBuf;
use std::sync::OnceLock;

use self::apps::DesktopCache;
use self::icons::IconDirs;
use self::packages::PackageDetector;
use self::session::{SessionEnv, SessionKind};
use self::x11::{X11, XError, XResult};
use self::xdg::XdgDirs;
use crate::common::{locate, process};
use crate::*;

const WAYLAND_REASON: &str = "Wayland compositors do not allow applications to inspect or \
                              arrange other applications' windows";
const WAYLAND_MONITORS_REASON: &str = "Monitor layout is only available through X11, and this \
                                       Wayland session has no reachable XWayland display";
const NO_SESSION_REASON: &str = "No graphical session was found (neither DISPLAY nor \
                                 WAYLAND_DISPLAY is set)";
const XWAYLAND_NOTE: &str = "Wayland session: only applications running through XWayland are \
                             visible and can be arranged; native Wayland windows are not.";
const XWAYLAND_MONITORS_NOTE: &str = "Monitors are reported by XWayland: names may be generic \
                                      and fractional scaling can shift coordinates.";
const NO_EWMH_NOTE: &str = "No EWMH-compatible window manager is running: windows are found \
                            through ICCCM WM_STATE, so stacking order, focus and frame sizes \
                            may be approximate.";
const ICON_NOTE: &str = "PNG icons only (window icons, the hicolor theme and pixmaps); \
                         applications with only SVG icons show no icon.";
const NO_RANDR_NOTE: &str = "The X server has no RandR extension: the whole X screen is \
                             reported as one monitor.";

/// What the X side of the session looks like right now.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Reach {
    NoSession,
    /// Wayland session without a reachable X display.
    WaylandOnly,
    /// X11 session, but the X server cannot be reached.
    Unreachable(String),
    X {
        xwayland: bool,
        ewmh: bool,
        randr: bool,
    },
}

fn display_server(reach: &Reach) -> DisplayServer {
    match reach {
        Reach::NoSession => DisplayServer::Unknown,
        Reach::WaylandOnly => DisplayServer::Wayland,
        Reach::Unreachable(_) => DisplayServer::X11,
        Reach::X { xwayland: true, .. } => DisplayServer::XWayland,
        Reach::X {
            xwayland: false, ..
        } => DisplayServer::X11,
    }
}

fn capabilities(reach: &Reach) -> PlatformCapabilities {
    let process_details = Capability::full();
    let application_icons = Capability::partial(ICON_NOTE);
    let all_window_ops = |cap: Capability, monitors: Capability| PlatformCapabilities {
        list_windows: cap.clone(),
        read_window_geometry: cap.clone(),
        move_windows: cap.clone(),
        change_window_state: cap.clone(),
        close_windows: cap.clone(),
        focus_windows: cap,
        list_monitors: monitors,
        process_details: process_details.clone(),
        application_icons: application_icons.clone(),
    };
    match reach {
        Reach::NoSession => all_window_ops(
            Capability::unsupported(NO_SESSION_REASON),
            Capability::unsupported(NO_SESSION_REASON),
        ),
        Reach::WaylandOnly => all_window_ops(
            Capability::unsupported(format!("{WAYLAND_REASON}.")),
            Capability::unsupported(format!("{WAYLAND_MONITORS_REASON}.")),
        ),
        Reach::Unreachable(error) => {
            let note = format!("Cannot connect to the X server: {error}");
            all_window_ops(
                Capability::unsupported(&note),
                Capability::unsupported(&note),
            )
        }
        Reach::X {
            xwayland: true,
            randr,
            ..
        } => {
            let monitors = if *randr {
                Capability::partial(XWAYLAND_MONITORS_NOTE)
            } else {
                Capability::partial(NO_RANDR_NOTE)
            };
            all_window_ops(Capability::partial(XWAYLAND_NOTE), monitors)
        }
        Reach::X {
            xwayland: false,
            ewmh,
            randr,
        } => {
            let monitors = if *randr {
                Capability::full()
            } else {
                Capability::partial(NO_RANDR_NOTE)
            };
            if *ewmh {
                all_window_ops(Capability::full(), monitors)
            } else {
                PlatformCapabilities {
                    list_windows: Capability::partial(NO_EWMH_NOTE),
                    read_window_geometry: Capability::partial(NO_EWMH_NOTE),
                    move_windows: Capability::partial(
                        "No EWMH-compatible window manager is running: windows are moved with \
                         plain ConfigureWindow requests.",
                    ),
                    change_window_state: Capability::partial(
                        "No EWMH-compatible window manager is running: only minimizing \
                         (ICCCM) is attempted; maximize and full screen are unavailable.",
                    ),
                    close_windows: Capability::full(),
                    focus_windows: Capability::partial(
                        "No EWMH-compatible window manager is running: focus is set directly.",
                    ),
                    list_monitors: monitors,
                    process_details,
                    application_icons,
                }
            }
        }
    }
}

fn read_hostname() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
}

/// Clamp a rectangle to what X11 can represent (16-bit coordinates and sizes).
fn clamp_to_x11(r: Rect) -> Rect {
    let pos = |v: i32| v.clamp(i32::from(i16::MIN), i32::from(i16::MAX));
    let size = |v: i32| v.clamp(1, i32::from(i16::MAX));
    Rect::new(pos(r.x), pos(r.y), size(r.width), size(r.height))
}

/// The Linux [`Platform`].
pub struct LinuxPlatform {
    env: SessionEnv,
    session: SessionKind,
    /// Explicit X display (see [`LinuxPlatform::for_display`]); also passed to launched apps.
    display_override: Option<String>,
    x11: X11,
    desktop: DesktopCache,
    icon_dirs: IconDirs,
    packages: PackageDetector,
    hostname: Option<String>,
    os_version: OnceLock<String>,
}

impl LinuxPlatform {
    /// Platform for the current session (`XDG_SESSION_TYPE`, `WAYLAND_DISPLAY`, `DISPLAY`).
    pub fn new() -> Self {
        Self::build(SessionEnv::from_process(), None)
    }

    /// Platform talking to a specific X display (e.g. `":1"`) instead of `$DISPLAY`.
    /// Applications launched through it get `DISPLAY` set to that display unless the launch
    /// request sets it. Used by tests and for controlling a secondary X server.
    pub fn for_display(display: impl Into<String>) -> Self {
        let display = display.into();
        let mut env = SessionEnv::from_process();
        env.display = Some(display.clone());
        Self::build(env, Some(display))
    }

    fn build(env: SessionEnv, display_override: Option<String>) -> Self {
        let xdg = XdgDirs::from_process();
        Self {
            session: session::classify(&env),
            x11: X11::new(env.display.clone()),
            desktop: DesktopCache::new(apps::application_dirs(&xdg)),
            icon_dirs: IconDirs::standard(&xdg),
            packages: PackageDetector::default(),
            hostname: read_hostname(),
            os_version: OnceLock::new(),
            display_override,
            env,
        }
    }

    /// Run an X operation, translating session restrictions and X errors.
    fn with_x<T>(
        &self,
        operation: &str,
        window: Option<u32>,
        wayland_reason: &str,
        op: impl Fn(&x11::Conn) -> XResult<T>,
    ) -> PlatformResult<T> {
        if self.session == SessionKind::Headless {
            return Err(PlatformError::unsupported(operation, NO_SESSION_REASON));
        }
        match self.x11.run(op) {
            Ok(value) => Ok(value),
            Err(XError::Connect(_)) if self.session == SessionKind::Wayland => {
                Err(PlatformError::unsupported(operation, wayland_reason))
            }
            Err(e) => Err(e.into_platform(operation, window)),
        }
    }

    fn parse_window(window: &WindowId) -> PlatformResult<u32> {
        ewmh::parse_window_id(window.as_str()).ok_or_else(|| {
            PlatformError::not_found(format!("window \"{window}\" (not an X11 window id)"))
        })
    }

    fn reach(&self) -> (Reach, Option<String>) {
        if self.session == SessionKind::Headless {
            return (Reach::NoSession, None);
        }
        match self
            .x11
            .run(|c| Ok((x11::detect_wm(c)?, c.randr.is_some())))
        {
            Ok((wm, randr)) => (
                Reach::X {
                    xwayland: self.session == SessionKind::Wayland,
                    ewmh: wm.is_ewmh(),
                    randr,
                },
                wm.name,
            ),
            Err(_) if self.session == SessionKind::Wayland => (Reach::WaylandOnly, None),
            Err(e) => {
                let message = match e {
                    XError::Connect(m) | XError::Connection(m) | XError::Other(m) => m,
                    other => format!("{other:?}"),
                };
                (Reach::Unreachable(message), None)
            }
        }
    }

    /// Icon names to try when the desktop entry gives none that resolves to a PNG.
    fn fallback_icon_names(
        package: Option<&PackageInfo>,
        executable: Option<&std::path::Path>,
        hints: &[Option<String>],
    ) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        let mut push = |name: Option<String>| {
            if let Some(name) = name.map(|n| n.trim().to_string())
                && !name.is_empty()
                && !name.contains('/')
                && !names.contains(&name)
            {
                names.push(name);
            }
        };
        if let Some(p) = package {
            push(Some(p.id.clone()));
            push(Some(p.id.to_lowercase()));
        }
        // An interpreter's icon would be misleading for the script it runs.
        if let Some(exe) = executable {
            push(
                exe.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .filter(|n| !apps::is_interpreter(n)),
            );
        }
        for hint in hints {
            push(hint.clone());
            push(hint.as_ref().map(|h| h.to_lowercase()));
        }
        names
    }
}

impl Default for LinuxPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl Platform for LinuxPlatform {
    fn info(&self) -> PlatformInfo {
        let (reach, wm_name) = self.reach();
        PlatformInfo {
            os: OsKind::Linux,
            os_version: self
                .os_version
                .get_or_init(process::long_os_version)
                .clone(),
            display_server: display_server(&reach),
            desktop_environment: session::desktop_environment(&self.env).or(wm_name),
            capabilities: capabilities(&reach),
        }
    }

    fn monitors(&self) -> PlatformResult<Vec<MonitorInfo>> {
        self.with_x(
            "Listing monitors",
            None,
            WAYLAND_MONITORS_REASON,
            x11::list_monitors,
        )
    }

    fn windows(&self) -> PlatformResult<Vec<WindowInfo>> {
        let hostname = self.hostname.as_deref();
        self.with_x("Listing windows", None, WAYLAND_REASON, |c| {
            x11::list_windows(c, hostname)
        })
    }

    fn processes(&self) -> PlatformResult<Vec<ProcessInfo>> {
        let mut processes = process::list_processes()?;
        let index = self.desktop.get();
        for p in &mut processes {
            p.package = self
                .packages
                .detect(p.pid, p.executable_path.as_deref(), &p.command_line);
            p.product_name = index.product_name(p);
        }
        Ok(processes)
    }

    fn launch(&self, request: &LaunchRequest) -> PlatformResult<LaunchedProcess> {
        launch::launch(request, self.display_override.as_deref())
    }

    fn set_window_placement(
        &self,
        window: &WindowId,
        placement: &WindowPlacement,
    ) -> PlatformResult<()> {
        let id = Self::parse_window(window)?;
        if placement.bounds.is_empty() {
            return Err(PlatformError::os(
                "Moving windows",
                format!(
                    "the requested size {}x{} is empty",
                    placement.bounds.width, placement.bounds.height
                ),
            ));
        }
        let placement = WindowPlacement {
            bounds: clamp_to_x11(placement.bounds),
            state: placement.state,
        };
        self.with_x("Moving windows", Some(id), WAYLAND_REASON, |c| {
            x11::set_placement(c, id, &placement)
        })
    }

    fn close_window(&self, window: &WindowId) -> PlatformResult<()> {
        let id = Self::parse_window(window)?;
        self.with_x("Closing windows", Some(id), WAYLAND_REASON, |c| {
            x11::close_window(c, id)
        })
    }

    fn focus_window(&self, window: &WindowId) -> PlatformResult<()> {
        let id = Self::parse_window(window)?;
        self.with_x("Focusing windows", Some(id), WAYLAND_REASON, |c| {
            x11::focus_window(c, id)
        })
    }

    fn application_icon(&self, request: &IconRequest) -> PlatformResult<Option<Vec<u8>>> {
        let size = match request.size {
            0 => 64,
            s => s.min(1024),
        };
        let has_other_inputs = request.executable_path.is_some() || request.package.is_some();
        let mut hints: Vec<Option<String>> = Vec::new();
        let mut window_exe: Option<PathBuf> = None;
        if let Some(window) = &request.window {
            let id = match Self::parse_window(window) {
                Ok(id) => Some(id),
                Err(e) if !has_other_inputs => return Err(e),
                Err(_) => None,
            };
            if let Some(id) = id {
                let hostname = self.hostname.as_deref();
                let result = self.with_x("Reading window icons", Some(id), WAYLAND_REASON, |c| {
                    match x11::window_icon(c, id, size)? {
                        Some(png) => Ok((Some(png), None)),
                        None => Ok((None, Some(x11::window_class(c, id, hostname)?))),
                    }
                });
                match result {
                    Ok((Some(png), _)) => return Ok(Some(png)),
                    Ok((None, Some((class, hint, pid)))) => {
                        hints.push(class);
                        hints.push(hint);
                        if pid > 0 {
                            window_exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok();
                        }
                    }
                    Ok((None, None)) => {}
                    Err(e @ PlatformError::NotFound { .. }) if !has_other_inputs => {
                        return Err(e);
                    }
                    Err(e) => log::debug!("window icon lookup failed: {e}"),
                }
            }
        }

        let index = self.desktop.get();
        let executable = request.executable_path.as_deref().or(window_exe.as_deref());
        let wm_class = hints.first().cloned().flatten();
        if let Some(app) =
            index.entry_for_icon(request.package.as_ref(), executable, wm_class.as_deref())
            && let Some(icon) = &app.entry.icon
            && let Some(png) =
                icons::find_icon_png(icon, size, &self.icon_dirs).and_then(|p| icons::read_png(&p))
        {
            return Ok(Some(png));
        }
        for name in Self::fallback_icon_names(request.package.as_ref(), executable, &hints) {
            if let Some(png) =
                icons::find_icon_png(&name, size, &self.icon_dirs).and_then(|p| icons::read_png(&p))
            {
                return Ok(Some(png));
            }
        }
        Ok(None)
    }

    fn locate_application(&self, query: &LocateQuery) -> Vec<LaunchTarget> {
        let path_hits = locate::find_on_path(query);
        apps::locate(&self.desktop.get(), query, &path_hits)
    }

    fn permissions(&self) -> Vec<PermissionStatus> {
        Vec::new()
    }

    fn request_permission(&self, kind: PermissionKind) -> PlatformResult<()> {
        Err(PlatformError::unsupported(
            format!("{kind:?} permission"),
            "Linux does not require additional permissions",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_reflect_the_session() {
        let x11 = capabilities(&Reach::X {
            xwayland: false,
            ewmh: true,
            randr: true,
        });
        assert_eq!(x11.list_windows.level, SupportLevel::Full);
        assert_eq!(x11.change_window_state.level, SupportLevel::Full);
        assert_eq!(x11.list_monitors.level, SupportLevel::Full);
        assert_eq!(x11.process_details.level, SupportLevel::Full);

        let xwayland = capabilities(&Reach::X {
            xwayland: true,
            ewmh: true,
            randr: true,
        });
        assert_eq!(xwayland.list_windows.level, SupportLevel::Partial);
        assert!(
            xwayland
                .list_windows
                .note
                .as_deref()
                .unwrap()
                .contains("XWayland")
        );
        assert_eq!(xwayland.move_windows.level, SupportLevel::Partial);

        let wayland = capabilities(&Reach::WaylandOnly);
        for cap in [
            &wayland.list_windows,
            &wayland.move_windows,
            &wayland.close_windows,
            &wayland.focus_windows,
            &wayland.change_window_state,
        ] {
            assert_eq!(cap.level, SupportLevel::Unsupported);
            assert!(cap.note.as_deref().unwrap().contains("Wayland compositors"));
        }
        assert_eq!(wayland.process_details.level, SupportLevel::Full);

        let bare = capabilities(&Reach::X {
            xwayland: false,
            ewmh: false,
            randr: false,
        });
        assert_eq!(bare.list_windows.level, SupportLevel::Partial);
        assert_eq!(bare.close_windows.level, SupportLevel::Full);
        assert_eq!(bare.list_monitors.level, SupportLevel::Partial);

        let down = capabilities(&Reach::Unreachable("Connection refused".into()));
        assert_eq!(down.list_windows.level, SupportLevel::Unsupported);
        assert!(
            down.list_windows
                .note
                .as_deref()
                .unwrap()
                .contains("Connection refused")
        );
        assert_eq!(
            capabilities(&Reach::NoSession).list_monitors.level,
            SupportLevel::Unsupported
        );
    }

    #[test]
    fn display_server_mapping() {
        assert_eq!(display_server(&Reach::NoSession), DisplayServer::Unknown);
        assert_eq!(display_server(&Reach::WaylandOnly), DisplayServer::Wayland);
        assert_eq!(
            display_server(&Reach::Unreachable(String::new())),
            DisplayServer::X11
        );
        let x = |xwayland| Reach::X {
            xwayland,
            ewmh: true,
            randr: true,
        };
        assert_eq!(display_server(&x(true)), DisplayServer::XWayland);
        assert_eq!(display_server(&x(false)), DisplayServer::X11);
    }

    #[test]
    fn x11_rect_clamping() {
        assert_eq!(
            clamp_to_x11(Rect::new(-100_000, 50, 0, 100_000)),
            Rect::new(-32768, 50, 1, 32767)
        );
        let r = Rect::new(10, 20, 640, 480);
        assert_eq!(clamp_to_x11(r), r);
    }

    #[test]
    fn unreachable_display_never_panics() {
        // A malformed display name cannot reach any X server.
        let p = LinuxPlatform::for_display("not-a-display");
        let info = p.info();
        assert_eq!(info.os, OsKind::Linux);
        assert!(!info.os_version.is_empty());
        assert_eq!(
            info.capabilities.list_windows.level,
            SupportLevel::Unsupported
        );
        let err = p.windows().unwrap_err();
        assert!(matches!(
            err,
            PlatformError::Os { .. } | PlatformError::Unsupported { .. }
        ));
        assert!(p.monitors().is_err());
        let w = WindowId::new("0x400001");
        let placement = WindowPlacement {
            bounds: Rect::new(0, 0, 100, 100),
            state: WindowState::Normal,
        };
        assert!(p.set_window_placement(&w, &placement).is_err());
        assert!(p.close_window(&w).is_err());
        assert!(p.focus_window(&w).is_err());
        // Processes and app lookup do not need a display.
        assert!(!p.processes().unwrap().is_empty());
        assert!(p.permissions().is_empty());
        assert!(
            p.request_permission(PermissionKind::Accessibility)
                .unwrap_err()
                .is_unsupported()
        );
    }

    fn unsupported_reason(result: PlatformResult<impl std::fmt::Debug>) -> String {
        match result {
            Err(PlatformError::Unsupported { reason, .. }) => reason,
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn wayland_without_x_reports_unsupported_window_management() {
        let env = SessionEnv {
            session_type: Some("wayland".into()),
            wayland_display: Some("wayland-0".into()),
            current_desktop: Some("ubuntu:GNOME".into()),
            ..Default::default()
        };
        let p = LinuxPlatform::build(env, None);
        let info = p.info();
        assert_eq!(info.display_server, DisplayServer::Wayland);
        assert_eq!(info.desktop_environment.as_deref(), Some("GNOME"));
        assert_eq!(
            info.capabilities.list_windows.level,
            SupportLevel::Unsupported
        );
        assert_eq!(unsupported_reason(p.windows()), WAYLAND_REASON);
        let w = WindowId::new("0x400001");
        let placement = WindowPlacement {
            bounds: Rect::new(0, 0, 100, 100),
            state: WindowState::Normal,
        };
        assert_eq!(
            unsupported_reason(p.set_window_placement(&w, &placement)),
            WAYLAND_REASON
        );
        assert_eq!(unsupported_reason(p.close_window(&w)), WAYLAND_REASON);
        assert_eq!(unsupported_reason(p.focus_window(&w)), WAYLAND_REASON);
        assert_eq!(unsupported_reason(p.monitors()), WAYLAND_MONITORS_REASON);
        // Launching and processes still work.
        assert!(p.processes().is_ok());
        let launched = p
            .launch(&LaunchRequest::executable("/bin/sh").with_args(["-c", "exit 0"]))
            .unwrap();
        assert!(launched.pid.is_some());
    }

    #[test]
    fn headless_session_reports_no_display() {
        let p = LinuxPlatform::build(SessionEnv::default(), None);
        let info = p.info();
        assert_eq!(info.display_server, DisplayServer::Unknown);
        assert_eq!(info.desktop_environment, None);
        assert_eq!(unsupported_reason(p.windows()), NO_SESSION_REASON);
        assert_eq!(unsupported_reason(p.monitors()), NO_SESSION_REASON);
        assert!(p.processes().is_ok());
    }

    #[test]
    fn x11_session_without_display_is_an_os_error() {
        let env = SessionEnv {
            session_type: Some("x11".into()),
            ..Default::default()
        };
        let p = LinuxPlatform::build(env, None);
        let info = p.info();
        assert_eq!(info.display_server, DisplayServer::X11);
        let note = info.capabilities.list_windows.note.unwrap();
        assert!(note.contains("DISPLAY is not set"), "{note}");
        let err = p.windows().unwrap_err();
        assert!(matches!(err, PlatformError::Os { .. }), "{err:?}");
        assert!(err.to_string().contains("DISPLAY is not set"), "{err}");
    }

    #[test]
    fn bad_window_ids_are_not_found() {
        let p = LinuxPlatform::for_display("not-a-display");
        let placement = WindowPlacement {
            bounds: Rect::new(0, 0, 100, 100),
            state: WindowState::Normal,
        };
        for id in ["mock-1", "", "0x0", "HWND"] {
            let w = WindowId::new(id);
            assert!(matches!(
                p.set_window_placement(&w, &placement),
                Err(PlatformError::NotFound { .. })
            ));
            assert!(matches!(
                p.close_window(&w),
                Err(PlatformError::NotFound { .. })
            ));
            assert!(matches!(
                p.focus_window(&w),
                Err(PlatformError::NotFound { .. })
            ));
        }
        let empty = WindowPlacement {
            bounds: Rect::new(0, 0, 0, 100),
            state: WindowState::Normal,
        };
        assert!(
            p.set_window_placement(&WindowId::new("0x400001"), &empty)
                .is_err()
        );
        let icon = IconRequest {
            window: Some(WindowId::new("bogus")),
            ..Default::default()
        };
        assert!(matches!(
            p.application_icon(&icon),
            Err(PlatformError::NotFound { .. })
        ));
    }

    #[test]
    fn fallback_icon_name_candidates() {
        let package = PackageInfo {
            kind: PackageKind::Flatpak,
            id: "org.gimp.GIMP".into(),
            path: None,
        };
        let names = LinuxPlatform::fallback_icon_names(
            Some(&package),
            Some(std::path::Path::new("/usr/bin/xterm")),
            &[Some("XTerm".into()), Some("xterm".into()), None],
        );
        assert_eq!(
            names,
            vec!["org.gimp.GIMP", "org.gimp.gimp", "xterm", "XTerm"]
        );
        let script = LinuxPlatform::fallback_icon_names(
            None,
            Some(std::path::Path::new("/usr/bin/python3.12")),
            &[],
        );
        assert!(script.is_empty());
    }
}
