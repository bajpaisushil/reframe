//! macOS implementation: AppKit (`NSWorkspace`, `NSRunningApplication`, `NSScreen`), the
//! CoreGraphics window list and the Accessibility (AX) API.
//!
//! # Coordinates
//!
//! Everything is in the CoreGraphics global display space, in points: origin at the top-left
//! corner of the main display (the one with the menu bar), y growing downwards. This is the space
//! of `CGWindowListCopyWindowInfo`, `CGDisplayBounds` and the Accessibility API; AppKit
//! rectangles (`NSScreen.frame`, bottom-left origin) are converted.
//!
//! # Permissions
//!
//! Reframe asks for as little as possible: listing applications and on-screen windows and
//! launching applications need no permission at all.
//!
//! - **Accessibility** (optional) is required to move, resize, minimize, focus and close
//!   windows, to list minimized windows, to detect full-screen windows and to read other
//!   applications' window titles. Without it those operations return
//!   [`PlatformError::PermissionDenied`] and titles are empty.
//! - **Screen Recording** is never requested and not reported by [`Platform::permissions`]:
//!   Accessibility already provides titles. If the user granted it anyway, CoreGraphics titles
//!   are used. Reframe never captures screen contents.
//! - **Automation** (Apple Events) is not used by this module; [`Platform::permissions`] does not
//!   report it because macOS only exposes it per target application.
//!
//! # Limitations
//!
//! - Only windows on the current Space are listed: macOS has no public API for other Spaces
//!   (this includes full-screen windows that are not currently shown). [`WindowInfo::desktop`]
//!   is always `None`.
//! - Windows of hidden applications (⌘H) are not listed (they are not visible), except
//!   minimized ones.
//! - Minimized windows are listed only with the Accessibility permission, and only when their
//!   CoreGraphics window number can be determined.
//! - macOS has no maximize state: a window is reported as [`WindowState::Maximized`] when it
//!   fills its screen's usable area, and maximizing resizes it to that area.
//!   [`WindowInfo::normal_bounds`] is always `None`.
//! - Full screen uses the window's native full-screen mode (`AXFullScreen`) and only works for
//!   applications that support it.
//! - Focusing is best effort: since macOS 14 the system may decline to activate another
//!   application while the user works elsewhere.
//! - Applications launched through Launch Services (`/usr/bin/open`) report no process id.
//! - Monitor ids are display UUIDs. Identical displays without serial numbers share a UUID; their
//!   ids then include the display's position and change when the arrangement changes.
//! - `NSScreen` may only be used on the main thread. Platform calls arrive on worker threads, so
//!   screen data is fetched through the main dispatch queue with a timeout; if the main thread
//!   does not answer (a host without a main run loop), monitor names, usable areas and scale
//!   factors fall back to CoreGraphics data.
//! - Matching Accessibility windows to CoreGraphics windows uses `_AXUIElementGetWindow`, a
//!   private but long-stable function, when it exists (looked up at run time); otherwise
//!   windows are matched by geometry and title, which can confuse identical overlapping windows
//!   of one application.

mod apps;
mod ax;
mod cf;
mod displays;
mod open;
mod permissions;
mod windows;

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use objc2::rc::autoreleasepool;
use objc2_foundation::NSProcessInfo;

use self::ax::{AxElement, AxResult, name};
use crate::common::{launch, locate, process};
use crate::macos_logic::{
    self as logic, CurrentWindowState, DisplayRecord, FRect, OpenApplication, PlacementStep,
    ScreenRecord, WindowSources, ax_code,
};
use crate::*;

/// How long `NSScreen` data is reused before it is read again.
const SCREEN_CACHE_TTL: Duration = Duration::from_secs(2);
/// How long a worker thread waits for the main thread to read `NSScreen`.
const MAIN_THREAD_TIMEOUT: Duration = Duration::from_millis(750);
/// After the main thread failed to answer, how long to use cached/fallback screen data.
const MAIN_THREAD_BACKOFF: Duration = Duration::from_secs(30);
/// Applications that did not answer an Accessibility request are skipped this long when
/// listing windows, so one hung application does not slow down every poll.
const UNRESPONSIVE_BACKOFF: Duration = Duration::from_secs(10);
/// Longest wait for a window to leave full screen or come back from the Dock.
const TRANSITION_TIMEOUT: Duration = Duration::from_millis(2500);
/// Extra settling time after leaving full screen (the animation outlasts the attribute change).
const FULLSCREEN_SETTLE: Duration = Duration::from_millis(500);
const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// Attempts to apply a frame when the application lags behind (animations, constraints).
const FRAME_ATTEMPTS: u32 = 3;
const FRAME_RETRY_DELAY: Duration = Duration::from_millis(200);

#[derive(Default)]
struct ScreenCache {
    records: Vec<ScreenRecord>,
    fetched_at: Option<Instant>,
    main_thread_unavailable_until: Option<Instant>,
}

/// The macOS [`Platform`]. See the module documentation for permissions and limitations.
pub struct MacPlatform {
    screens: Mutex<ScreenCache>,
    /// pid -> until when Accessibility requests to it are skipped while listing windows.
    unresponsive: Mutex<HashMap<u32, Instant>>,
    accessibility_prompted: AtomicBool,
    screen_recording_prompted: AtomicBool,
}

impl MacPlatform {
    pub fn new() -> Self {
        ax::set_global_timeout();
        Self {
            screens: Mutex::new(ScreenCache::default()),
            unresponsive: Mutex::new(HashMap::new()),
            accessibility_prompted: AtomicBool::new(false),
            screen_recording_prompted: AtomicBool::new(false),
        }
    }

    /// `NSScreen` data matching `displays`, cached for [`SCREEN_CACHE_TTL`].
    fn screen_records(&self, displays: &[DisplayRecord]) -> Vec<ScreenRecord> {
        let now = Instant::now();
        {
            let cache = self.screens.lock().unwrap_or_else(PoisonError::into_inner);
            let covers_displays = displays
                .iter()
                .all(|d| cache.records.iter().any(|s| s.display_id == Some(d.id)));
            let fresh = cache
                .fetched_at
                .is_some_and(|at| now.duration_since(at) < SCREEN_CACHE_TTL);
            let main_thread_down = cache
                .main_thread_unavailable_until
                .is_some_and(|until| now < until);
            if (fresh && covers_displays) || main_thread_down {
                return cache.records.clone();
            }
        }
        // Not holding the lock while waiting for the main thread.
        let fetched = displays::screens(MAIN_THREAD_TIMEOUT);
        let mut cache = self.screens.lock().unwrap_or_else(PoisonError::into_inner);
        match fetched {
            Some(records) => {
                cache.records = records;
                cache.fetched_at = Some(Instant::now());
                cache.main_thread_unavailable_until = None;
            }
            None => {
                log::debug!(
                    "the main thread did not answer within {MAIN_THREAD_TIMEOUT:?}; using \
                     CoreGraphics display data only"
                );
                cache.main_thread_unavailable_until = Some(Instant::now() + MAIN_THREAD_BACKOFF);
            }
        }
        cache.records.clone()
    }

    fn is_unresponsive(&self, pid: u32) -> bool {
        let mut map = self
            .unresponsive
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        map.retain(|_, until| *until > now);
        map.contains_key(&pid)
    }

    fn mark_unresponsive(&self, pid: u32) {
        self.unresponsive
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(pid, Instant::now() + UNRESPONSIVE_BACKOFF);
    }

    fn require_accessibility(operation: &str) -> PlatformResult<()> {
        if permissions::accessibility_granted() {
            Ok(())
        } else {
            Err(logic::accessibility_denied(operation))
        }
    }
}

impl Default for MacPlatform {
    fn default() -> Self {
        Self::new()
    }
}

/// `(major, minor, patch)` of the running macOS.
fn os_version() -> (i64, i64, i64) {
    let version = NSProcessInfo::processInfo().operatingSystemVersion();
    (
        version.majorVersion as i64,
        version.minorVersion as i64,
        version.patchVersion as i64,
    )
}

fn macos_major() -> u32 {
    u32::try_from(os_version().0).unwrap_or(0)
}

/// Polls `condition` until it holds or `timeout` elapses; returns whether it held.
fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// Moves and resizes a window: position, size, position again (the size may only fit once the
/// window is on the destination screen; the second move corrects positions clamped at screen
/// edges). Retries while the application lags behind; a frame the application constrains
/// (minimum size, fixed size) is accepted.
fn set_frame(element: &AxElement, frame: FRect) -> AxResult<()> {
    for attempt in 1..=FRAME_ATTEMPTS {
        element.set_position(frame.x, frame.y)?;
        match element.set_size(frame.width, frame.height) {
            // Fixed-size windows reject sizes; their position is all that can be restored.
            Ok(()) | Err(ax_code::ATTRIBUTE_UNSUPPORTED | ax_code::ILLEGAL_ARGUMENT) => {}
            Err(code) => return Err(code),
        }
        element.set_position(frame.x, frame.y)?;
        let applied = element
            .frame()
            .ok()
            .flatten()
            .is_some_and(|actual| actual.approx_eq(&frame, logic::MATCH_TOLERANCE));
        if applied {
            return Ok(());
        }
        if attempt < FRAME_ATTEMPTS {
            thread::sleep(FRAME_RETRY_DELAY);
        }
    }
    log::debug!("the application constrained the requested frame {frame:?}");
    Ok(())
}

fn apply_placement(
    element: &AxElement,
    steps: &[PlacementStep],
    operation: &str,
) -> PlatformResult<()> {
    let fail = |code| logic::ax_error(operation, code);
    for step in steps {
        match *step {
            PlacementStep::ExitFullscreen => {
                element.set_bool(name::FULL_SCREEN, false).map_err(fail)?;
                if wait_until(TRANSITION_TIMEOUT, || {
                    element.boolean(name::FULL_SCREEN).ok().flatten() != Some(true)
                }) {
                    thread::sleep(FULLSCREEN_SETTLE);
                }
            }
            PlacementStep::Unminimize => {
                element.set_bool(name::MINIMIZED, false).map_err(fail)?;
                wait_until(TRANSITION_TIMEOUT, || {
                    element.boolean(name::MINIMIZED).ok().flatten() != Some(true)
                });
            }
            PlacementStep::SetFrame { frame, best_effort } => {
                if let Err(code) = set_frame(element, frame) {
                    if !best_effort {
                        return Err(fail(code));
                    }
                    log::debug!("could not move a minimized window (AXError {code})");
                }
            }
            PlacementStep::EnterFullscreen => element
                .set_bool(name::FULL_SCREEN, true)
                .map_err(|code| logic::ax_error("Making a window full screen", code))?,
            PlacementStep::Minimize => element.set_bool(name::MINIMIZED, true).map_err(fail)?,
        }
    }
    Ok(())
}

/// Launches an application bundle through Launch Services.
fn open_application(
    app: OpenApplication<'_>,
    label: &str,
    request: &LaunchRequest,
) -> PlatformResult<LaunchedProcess> {
    if let Some(dir) = &request.working_directory {
        log::debug!(
            "{label}: applications started through Launch Services do not use a working \
             directory; ignoring {}",
            dir.display()
        );
    }
    let args = logic::open_application_args(app, &request.env, &request.args)
        .map_err(|message| PlatformError::launch_failed(label, message))?;
    open::run_open(&args, label)?;
    Ok(LaunchedProcess { pid: None })
}

fn launch_bundle(
    package: &PackageInfo,
    request: &LaunchRequest,
) -> PlatformResult<LaunchedProcess> {
    let id = package.id.trim();
    let path = package.path.as_deref().filter(|p| p.is_dir());
    if !id.is_empty() && apps::url_for_bundle_id(id).is_some() {
        return open_application(OpenApplication::BundleId(id), id, request);
    }
    match path {
        Some(path) => open_application(
            OpenApplication::Path(path),
            &path.display().to_string(),
            request,
        ),
        None if id.is_empty() => Err(PlatformError::launch_failed(
            "application",
            "the package has neither a bundle identifier nor an application path",
        )),
        None => Err(PlatformError::launch_failed(
            id,
            "no application with this bundle identifier is installed",
        )),
    }
}

fn open_settings(kind: PermissionKind) -> PlatformResult<()> {
    let url = logic::privacy_settings_url(kind);
    open::run_open(&[OsString::from(url)], "System Settings").map_err(|e| {
        PlatformError::os(
            format!("Opening the {kind:?} privacy settings"),
            e.to_string(),
        )
    })
}

/// A launch target for the bundle at `path`: its bundle identifier when it has one.
fn bundle_target(path: PathBuf, fallback_id: Option<&str>) -> LaunchTarget {
    match apps::bundle_identifier(&path).or_else(|| fallback_id.map(str::to_string)) {
        Some(id) => LaunchTarget::Package {
            package: PackageInfo {
                kind: PackageKind::MacBundle,
                id,
                path: Some(path),
            },
        },
        None => LaunchTarget::Open {
            target: path.display().to_string(),
        },
    }
}

/// The file whose icon represents the application in `request`.
fn icon_source(request: &IconRequest) -> Option<PathBuf> {
    if let Some(package) = request
        .package
        .as_ref()
        .filter(|p| p.kind == PackageKind::MacBundle)
    {
        if let Some(path) = package.path.as_ref().filter(|p| p.exists()) {
            return Some(path.clone());
        }
        let id = package.id.trim();
        if logic::is_valid_bundle_id(id)
            && let Some(path) = apps::url_for_bundle_id(id)
        {
            return Some(path);
        }
    }
    if let Some(executable) = &request.executable_path {
        if let Some(bundle) = logic::enclosing_app_bundle(executable).filter(|b| b.exists()) {
            return Some(bundle);
        }
        if executable.exists() {
            return Some(executable.clone());
        }
    }
    let id = request.window.as_ref().and_then(logic::parse_window_id)?;
    let pid = windows::cg_window(id)?.pid;
    apps::application_path(pid)
}

impl Platform for MacPlatform {
    fn info(&self) -> PlatformInfo {
        autoreleasepool(|_| {
            let (major, minor, patch) = os_version();
            PlatformInfo {
                os: OsKind::Macos,
                os_version: logic::format_macos_version(major, minor, patch),
                display_server: DisplayServer::Quartz,
                desktop_environment: None,
                capabilities: logic::capabilities(
                    permissions::accessibility_granted(),
                    permissions::screen_recording_granted(),
                    u32::try_from(major).unwrap_or(0),
                ),
            }
        })
    }

    /// Active displays: CoreGraphics bounds and UUIDs, `NSScreen` names, usable areas
    /// (`visibleFrame`, without menu bar and Dock) and backing scale factors.
    fn monitors(&self) -> PlatformResult<Vec<MonitorInfo>> {
        autoreleasepool(|_| {
            let displays = displays::display_records()?;
            let screens = self.screen_records(&displays);
            Ok(logic::build_monitors(&displays, &screens))
        })
    }

    /// See [`logic::assemble_windows`] for the filtering rules and the module documentation for
    /// what macOS does not expose.
    fn windows(&self) -> PlatformResult<Vec<WindowInfo>> {
        autoreleasepool(|_| {
            let on_screen = windows::cg_on_screen()?;
            let apps = apps::running_applications();
            let monitors = self.monitors().unwrap_or_else(|e| {
                log::debug!("monitors unavailable while listing windows: {e}");
                Vec::new()
            });
            let frontmost = apps::frontmost_pid();
            let own_pid = std::process::id();

            let mut ax = HashMap::new();
            if permissions::accessibility_granted() {
                // Reframe's own windows are answered by its own main thread; they are excluded
                // from discovery anyway, so they are not queried.
                for app in apps.iter().filter(|a| a.regular && a.pid != own_pid) {
                    if self.is_unresponsive(app.pid) {
                        continue;
                    }
                    match windows::ax_app(app.pid, frontmost == Some(app.pid)) {
                        Ok(data) => {
                            ax.insert(app.pid, data);
                        }
                        Err(code) => {
                            if code == ax_code::CANNOT_COMPLETE {
                                self.mark_unresponsive(app.pid);
                            }
                            log::debug!(
                                "Accessibility windows of pid {} unavailable (AXError {code})",
                                app.pid
                            );
                        }
                    }
                }
            }
            let needs_off_screen = ax
                .values()
                .any(|a| a.windows.iter().any(|w| w.minimized && w.cg_id.is_none()));
            let off_screen = if needs_off_screen {
                windows::cg_all()
            } else {
                Vec::new()
            };
            Ok(logic::assemble_windows(&WindowSources {
                on_screen: &on_screen,
                off_screen: &off_screen,
                apps: &apps,
                ax: &ax,
                monitors: &monitors,
                frontmost_pid: frontmost,
            }))
        })
    }

    /// All processes (via `sysinfo`), with `localizedName` and bundle identity added for
    /// processes that are Launch Services applications.
    fn processes(&self) -> PlatformResult<Vec<ProcessInfo>> {
        let mut processes = process::list_processes()?;
        let apps = autoreleasepool(|_| apps::running_applications());
        logic::enrich_processes(&mut processes, &apps);
        Ok(processes)
    }

    /// - `Package` (`MacBundle`): `open -n -b <bundle id> [--args ...]`, or `open -n -a <path>`
    ///   when the bundle identifier is unknown to Launch Services.
    /// - `Executable` inside an `.app` bundle (or an `.app` itself): the bundle is opened the same
    ///   way, so the application starts like it does from the Finder.
    /// - Other executables: spawned directly and detached ([`launch::spawn_detached`]).
    /// - `Open`: `open <target>` (default application for a file, folder or URL; arguments do
    ///   not apply).
    ///
    /// Launch Services launches report no pid and do not use a working directory. `-n` starts a
    /// new instance so launch arguments always reach it; applications that cannot run twice
    /// declare `LSMultipleInstancesProhibited`, and macOS then activates the running instance.
    fn launch(&self, request: &LaunchRequest) -> PlatformResult<LaunchedProcess> {
        autoreleasepool(|_| match &request.target {
            LaunchTarget::Package { package } => {
                if package.kind != PackageKind::MacBundle {
                    return Err(PlatformError::unsupported(
                        format!("Launching {}", package.id),
                        format!("{:?} packages do not exist on macOS", package.kind),
                    ));
                }
                launch_bundle(package, request)
            }
            LaunchTarget::Executable { path } => {
                let bundle = if logic::is_app_bundle_path(path) && path.is_dir() {
                    Some(path.clone())
                } else {
                    logic::enclosing_app_bundle(path).filter(|b| b.is_dir())
                };
                if let Some(bundle) = bundle {
                    return open_application(
                        OpenApplication::Path(&bundle),
                        &bundle.display().to_string(),
                        request,
                    );
                }
                let pid = launch::spawn_detached(
                    path,
                    &request.args,
                    request.working_directory.as_deref(),
                    &request.env,
                )?;
                Ok(LaunchedProcess { pid: Some(pid) })
            }
            LaunchTarget::Open { target } => {
                if !request.args.is_empty() {
                    log::debug!("{target}: arguments do not apply to opened files and URLs");
                }
                let args = logic::open_target_args(target)
                    .map_err(|message| PlatformError::launch_failed(target, message))?;
                open::run_open(&args, target)?;
                Ok(LaunchedProcess { pid: None })
            }
        })
    }

    /// Applies the placement through Accessibility (see [`logic::plan_placement`]). Needs the
    /// Accessibility permission.
    fn set_window_placement(
        &self,
        window: &WindowId,
        placement: &WindowPlacement,
    ) -> PlatformResult<()> {
        const OPERATION: &str = "Moving a window";
        Self::require_accessibility(OPERATION)?;
        autoreleasepool(|_| {
            let target = windows::resolve(window, OPERATION)?;
            let element = &target.element;
            let current = CurrentWindowState {
                minimized: element
                    .boolean(name::MINIMIZED)
                    .ok()
                    .flatten()
                    .unwrap_or(false),
                fullscreen: element
                    .boolean(name::FULL_SCREEN)
                    .ok()
                    .flatten()
                    .unwrap_or(false),
                frame: element.frame().ok().flatten(),
            };
            let monitors = self.monitors().unwrap_or_default();
            let steps = logic::plan_placement(&current, placement, &monitors);
            apply_placement(element, &steps, OPERATION)
        })
    }

    /// Presses the window's close button through Accessibility, exactly like a click: the
    /// application may ask to save changes or refuse. Nothing is ever terminated. An application
    /// that does not answer in time (typically because it is showing a modal "save changes?"
    /// dialog) has still received the request, so that is not an error. Needs the
    /// Accessibility permission.
    fn close_window(&self, window: &WindowId) -> PlatformResult<()> {
        const OPERATION: &str = "Closing a window";
        Self::require_accessibility(OPERATION)?;
        autoreleasepool(|_| {
            let target = windows::resolve(window, OPERATION)?;
            let button = target
                .element
                .element(name::CLOSE_BUTTON)
                .map_err(|code| logic::ax_error(OPERATION, code))?
                .ok_or_else(|| {
                    PlatformError::unsupported(
                        OPERATION,
                        "the window has no close button that can be pressed through \
                         Accessibility",
                    )
                })?;
            match button.perform(name::PRESS) {
                Ok(()) => Ok(()),
                Err(ax_code::CANNOT_COMPLETE) => {
                    log::debug!("window {window}: close request sent, the application is busy");
                    Ok(())
                }
                Err(code) => Err(logic::ax_error(OPERATION, code)),
            }
        })
    }

    /// Un-minimizes the window, makes it its application's main window, raises it and
    /// activates the application. Best effort (see the module documentation). Needs the
    /// Accessibility permission.
    fn focus_window(&self, window: &WindowId) -> PlatformResult<()> {
        const OPERATION: &str = "Focusing a window";
        Self::require_accessibility(OPERATION)?;
        autoreleasepool(|_| {
            let target = windows::resolve(window, OPERATION)?;
            let element = &target.element;
            if element.boolean(name::MINIMIZED).ok().flatten() == Some(true) {
                element
                    .set_bool(name::MINIMIZED, false)
                    .map_err(|code| logic::ax_error(OPERATION, code))?;
                wait_until(TRANSITION_TIMEOUT, || {
                    element.boolean(name::MINIMIZED).ok().flatten() != Some(true)
                });
            }
            // Not every application lets its main window be set; raising still works then.
            if let Err(code) = element.set_bool(name::MAIN, true) {
                log::debug!("window {window}: AXMain not set (AXError {code})");
            }
            let raised = element.perform(name::RAISE);
            let activated = apps::activate(target.pid);
            match raised {
                Ok(()) if activated => Ok(()),
                Ok(()) => {
                    log::debug!("window {window}: raised, but macOS declined to activate its app");
                    Ok(())
                }
                Err(code) if activated => {
                    log::debug!("window {window}: AXRaise failed (AXError {code}), app activated");
                    Ok(())
                }
                Err(code) => Err(logic::ax_error(OPERATION, code)),
            }
        })
    }

    /// The Finder icon of the application bundle (or executable) rendered at the requested
    /// size (16-1024 px, 64 when unspecified).
    fn application_icon(&self, request: &IconRequest) -> PlatformResult<Option<Vec<u8>>> {
        autoreleasepool(|_| {
            let Some(path) = icon_source(request) else {
                return Ok(None);
            };
            apps::icon_png(&path, logic::icon_pixel_size(request.size))
        })
    }

    /// Bundle identifiers through Launch Services, then `<display name>.app` in the standard
    /// application folders, then executables on `PATH`.
    fn locate_application(&self, query: &LocateQuery) -> Vec<LaunchTarget> {
        autoreleasepool(|_| {
            let mut targets = Vec::new();
            let mut seen: HashSet<PathBuf> = HashSet::new();
            for id in &query.bundle_ids {
                let id = id.trim();
                if !logic::is_valid_bundle_id(id) {
                    continue;
                }
                if let Some(path) = apps::url_for_bundle_id(id)
                    && seen.insert(path.clone())
                {
                    targets.push(bundle_target(path, Some(id)));
                }
            }
            let home = std::env::var_os("HOME").map(PathBuf::from);
            for name in &query.display_names {
                for candidate in logic::app_bundle_candidates(name, home.as_deref()) {
                    if candidate.is_dir() && seen.insert(candidate.clone()) {
                        targets.push(bundle_target(candidate, None));
                    }
                }
            }
            for path in locate::find_on_path(query) {
                if seen.insert(path.clone()) {
                    targets.push(LaunchTarget::Executable { path });
                }
            }
            targets
        })
    }

    /// Accessibility only. Screen Recording is not reported (Accessibility already provides
    /// window titles, so Reframe never asks for it); Automation is per target application and
    /// not reported either. See the module documentation.
    fn permissions(&self) -> Vec<PermissionStatus> {
        let major = autoreleasepool(|_| macos_major());
        vec![logic::permission_status(
            PermissionKind::Accessibility,
            permissions::accessibility_granted(),
            major,
        )]
    }

    /// - Accessibility: the system prompt (`AXIsProcessTrustedWithOptions`) on the first request
    ///   of this session, the Accessibility settings page on later requests (macOS shows the
    ///   prompt only once).
    /// - Screen Recording: `CGRequestScreenCaptureAccess` first, then the settings page; Reframe
    ///   must be restarted after granting it.
    /// - Automation: the Automation settings page.
    ///
    /// Requests for a permission that is already granted do nothing.
    fn request_permission(&self, kind: PermissionKind) -> PlatformResult<()> {
        autoreleasepool(|_| match kind {
            PermissionKind::Accessibility => {
                if permissions::accessibility_granted() {
                    return Ok(());
                }
                if self.accessibility_prompted.swap(true, Ordering::SeqCst) {
                    open_settings(kind)
                } else {
                    permissions::prompt_accessibility();
                    Ok(())
                }
            }
            PermissionKind::ScreenRecording => {
                if permissions::screen_recording_granted() {
                    return Ok(());
                }
                if self.screen_recording_prompted.swap(true, Ordering::SeqCst) {
                    open_settings(kind)
                } else {
                    permissions::request_screen_recording();
                    Ok(())
                }
            }
            PermissionKind::Automation => open_settings(kind),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These run only on macOS (the module is compiled for macOS only). They check that the
    // native calls are wired up without panicking; results depend on the machine's permissions.

    #[test]
    fn reports_quartz_and_permissions() {
        let platform = MacPlatform::new();
        let info = platform.info();
        assert_eq!(info.os, OsKind::Macos);
        assert_eq!(info.display_server, DisplayServer::Quartz);
        assert!(info.os_version.starts_with("macOS"));
        let kinds: Vec<PermissionKind> = platform.permissions().iter().map(|p| p.kind).collect();
        assert_eq!(kinds, vec![PermissionKind::Accessibility]);
    }

    #[test]
    fn native_queries_do_not_panic() {
        let platform = MacPlatform::new();
        let _ = platform.monitors();
        let _ = platform.windows();
        assert!(platform.processes().is_ok());
        let _ = platform.locate_application(&LocateQuery {
            bundle_ids: vec!["com.apple.finder".into()],
            display_names: vec!["Safari".into()],
            executable_names: vec!["ls".into()],
            ..Default::default()
        });
    }

    #[test]
    fn invalid_window_ids_are_not_found_or_need_permission() {
        let platform = MacPlatform::new();
        let err = platform
            .focus_window(&WindowId::new("not-a-number"))
            .unwrap_err();
        assert!(matches!(
            err,
            PlatformError::NotFound { .. } | PlatformError::PermissionDenied { .. }
        ));
    }

    #[test]
    fn missing_paths_have_no_icon() {
        let platform = MacPlatform::new();
        let icon = platform.application_icon(&IconRequest {
            executable_path: Some(PathBuf::from("/definitely/not/here")),
            size: 32,
            ..Default::default()
        });
        assert_eq!(icon, Ok(None));
    }
}
