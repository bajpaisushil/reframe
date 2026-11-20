//! In-memory [`Platform`] test double used by `reframe-core` tests.
//!
//! This is **test infrastructure only**: it is compiled behind the `mock` feature, which the
//! Reframe application never enables. It simulates a desktop with monitors, processes and
//! windows, and reacts to launches according to [`LaunchRule`]s so the restoration engine can
//! be tested deterministically (slow apps, failing launches, single-instance apps, windows
//! that refuse to close because of unsaved work...).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::*;

/// A window that appears after a matching launch.
#[derive(Debug, Clone)]
pub struct MockWindowSpec {
    pub title: String,
    pub class_name: Option<String>,
    pub bounds: Rect,
}

impl MockWindowSpec {
    pub fn titled(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            class_name: None,
            bounds: Rect::new(100, 100, 800, 600),
        }
    }
}

/// How the fake OS reacts to launching a program.
#[derive(Debug, Clone)]
pub struct LaunchRule {
    /// Case-insensitive substring matched against [`LaunchTarget::label`].
    pub program_contains: String,
    /// The first `fail_times` launches fail with [`PlatformError::LaunchFailed`].
    pub fail_times: u32,
    /// Windows that appear after a successful launch.
    pub windows: Vec<MockWindowSpec>,
    /// Delay before the windows appear.
    pub delay: Duration,
    /// Simulates single-instance apps (VS Code, Chrome): the launched process exits at once and
    /// the windows are owned by this already-running pid.
    pub single_instance_pid: Option<u32>,
    /// Process name used for the spawned fake process.
    pub process_name: String,
    /// Executable path reported for the spawned fake process.
    pub executable: Option<PathBuf>,
}

impl LaunchRule {
    pub fn new(program_contains: impl Into<String>) -> Self {
        let program_contains = program_contains.into();
        Self {
            process_name: program_contains.clone(),
            program_contains,
            fail_times: 0,
            windows: Vec::new(),
            delay: Duration::ZERO,
            single_instance_pid: None,
            executable: None,
        }
    }

    pub fn window(mut self, spec: MockWindowSpec) -> Self {
        self.windows.push(spec);
        self
    }

    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    pub fn fail_times(mut self, n: u32) -> Self {
        self.fail_times = n;
        self
    }

    pub fn single_instance(mut self, pid: u32) -> Self {
        self.single_instance_pid = Some(pid);
        self
    }

    pub fn executable(mut self, path: impl Into<PathBuf>) -> Self {
        self.executable = Some(path.into());
        self
    }
}

#[derive(Debug, Default)]
pub struct MockState {
    pub info_override: Option<PlatformInfo>,
    pub monitors: Vec<MonitorInfo>,
    pub windows: Vec<WindowInfo>,
    pub processes: Vec<ProcessInfo>,
    pub rules: Vec<LaunchRule>,
    /// Window titles (substrings) whose close requests are ignored (unsaved work).
    pub refuse_close: Vec<String>,
    /// Targets resolvable through [`Platform::locate_application`], keyed by executable name.
    pub installed: Vec<(String, LaunchTarget)>,
    // --- logs for assertions ---
    pub launches: Vec<LaunchRequest>,
    pub placements: Vec<(WindowId, WindowPlacement)>,
    pub close_requests: Vec<WindowId>,
    pub focus_requests: Vec<WindowId>,
    next_pid: u32,
    next_window: u32,
}

/// See module docs.
#[derive(Debug, Clone)]
pub struct MockPlatform {
    state: Arc<Mutex<MockState>>,
}

impl Default for MockPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl MockPlatform {
    /// A desktop with a single 1920x1080 primary monitor and nothing running.
    pub fn new() -> Self {
        let state = MockState {
            monitors: vec![MonitorInfo {
                id: "MOCK-1".into(),
                name: "Mock Display 1".into(),
                bounds: Rect::new(0, 0, 1920, 1080),
                work_area: Rect::new(0, 0, 1920, 1040),
                scale_factor: 1.0,
                is_primary: true,
            }],
            next_pid: 1000,
            next_window: 1,
            ..Default::default()
        };
        Self {
            state: Arc::new(Mutex::new(state)),
        }
    }

    pub fn with_monitors(self, monitors: Vec<MonitorInfo>) -> Self {
        self.state.lock().unwrap().monitors = monitors;
        self
    }

    /// Direct access to the simulated state for custom setups and assertions.
    pub fn state(&self) -> std::sync::MutexGuard<'_, MockState> {
        self.state.lock().unwrap()
    }

    pub fn set_info(&self, info: PlatformInfo) {
        self.state().info_override = Some(info);
    }

    /// Register a running process; returns its pid.
    pub fn add_process(&self, name: &str, executable: Option<PathBuf>) -> u32 {
        let mut s = self.state();
        let pid = s.next_pid;
        s.next_pid += 1;
        s.processes.push(ProcessInfo {
            pid,
            parent_pid: None,
            name: name.to_string(),
            product_name: None,
            executable_path: executable,
            command_line: vec![name.to_string()],
            working_directory: None,
            start_time: None,
            package: None,
        });
        pid
    }

    /// Register a process with an explicit command line / working directory / parent.
    pub fn add_process_info(&self, mut info: ProcessInfo) -> u32 {
        let mut s = self.state();
        if info.pid == 0 {
            info.pid = s.next_pid;
            s.next_pid += 1;
        }
        let pid = info.pid;
        s.processes.push(info);
        pid
    }

    /// Open a window owned by `pid`; returns its id.
    pub fn add_window(
        &self,
        pid: u32,
        title: &str,
        class_name: Option<&str>,
        bounds: Rect,
    ) -> WindowId {
        let mut s = self.state();

        Self::push_window(&mut s, pid, title, class_name.map(str::to_string), bounds)
    }

    pub fn add_rule(&self, rule: LaunchRule) {
        self.state().rules.push(rule);
    }

    pub fn refuse_close(&self, title_substring: &str) {
        self.state().refuse_close.push(title_substring.to_string());
    }

    pub fn install(&self, executable_name: &str, target: LaunchTarget) {
        self.state()
            .installed
            .push((executable_name.to_string(), target));
    }

    pub fn launches(&self) -> Vec<LaunchRequest> {
        self.state().launches.clone()
    }

    pub fn placements(&self) -> Vec<(WindowId, WindowPlacement)> {
        self.state().placements.clone()
    }

    pub fn close_requests(&self) -> Vec<WindowId> {
        self.state().close_requests.clone()
    }

    pub fn window(&self, id: &WindowId) -> Option<WindowInfo> {
        self.state().windows.iter().find(|w| &w.id == id).cloned()
    }

    fn push_window(
        s: &mut MockState,
        pid: u32,
        title: &str,
        class_name: Option<String>,
        bounds: Rect,
    ) -> WindowId {
        let id = WindowId(format!("mock-{}", s.next_window));
        s.next_window += 1;
        let monitor_id = s
            .monitors
            .iter()
            .max_by_key(|m| m.bounds.intersection_area(&bounds))
            .map(|m| m.id.clone());
        // New windows open on top.
        for w in &mut s.windows {
            w.z_order += 1;
            w.is_focused = false;
        }
        s.windows.push(WindowInfo {
            id: id.clone(),
            pid,
            title: title.to_string(),
            class_name,
            app_id_hint: None,
            bounds,
            normal_bounds: Some(bounds),
            state: WindowState::Normal,
            monitor_id,
            desktop: Some(0),
            is_focused: true,
            z_order: 0,
        });
        id
    }
}

impl Platform for MockPlatform {
    fn info(&self) -> PlatformInfo {
        if let Some(info) = &self.state().info_override {
            return info.clone();
        }
        PlatformInfo {
            os: OsKind::current(),
            os_version: "Mock OS 1.0".into(),
            display_server: DisplayServer::X11,
            desktop_environment: Some("Mock".into()),
            capabilities: PlatformCapabilities {
                list_windows: Capability::full(),
                read_window_geometry: Capability::full(),
                move_windows: Capability::full(),
                change_window_state: Capability::full(),
                close_windows: Capability::full(),
                focus_windows: Capability::full(),
                list_monitors: Capability::full(),
                process_details: Capability::full(),
                application_icons: Capability::unsupported("Mock platform has no icons."),
            },
        }
    }

    fn monitors(&self) -> PlatformResult<Vec<MonitorInfo>> {
        Ok(self.state().monitors.clone())
    }

    fn windows(&self) -> PlatformResult<Vec<WindowInfo>> {
        let mut windows = self.state().windows.clone();
        windows.sort_by_key(|w| w.z_order);
        Ok(windows)
    }

    fn processes(&self) -> PlatformResult<Vec<ProcessInfo>> {
        Ok(self.state().processes.clone())
    }

    fn launch(&self, request: &LaunchRequest) -> PlatformResult<LaunchedProcess> {
        let label = request.target.label();
        let mut s = self.state();
        s.launches.push(request.clone());

        let rule_index = s.rules.iter().position(|r| {
            label
                .to_lowercase()
                .contains(&r.program_contains.to_lowercase())
        });
        let Some(rule_index) = rule_index else {
            // Unknown program: it "starts" but never opens a window.
            let pid = s.next_pid;
            s.next_pid += 1;
            return Ok(LaunchedProcess { pid: Some(pid) });
        };

        if s.rules[rule_index].fail_times > 0 {
            s.rules[rule_index].fail_times -= 1;
            return Err(PlatformError::launch_failed(
                label,
                "simulated launch failure",
            ));
        }

        let rule = s.rules[rule_index].clone();
        let owner_pid = match rule.single_instance_pid {
            Some(pid) => pid,
            None => {
                let pid = s.next_pid;
                s.next_pid += 1;
                s.processes.push(ProcessInfo {
                    pid,
                    parent_pid: None,
                    name: rule.process_name.clone(),
                    product_name: None,
                    executable_path: rule.executable.clone(),
                    command_line: std::iter::once(label.clone())
                        .chain(request.args.iter().cloned())
                        .collect(),
                    working_directory: request.working_directory.clone(),
                    start_time: None,
                    package: None,
                });
                pid
            }
        };
        let launched_pid = if rule.single_instance_pid.is_some() {
            s.next_pid
        } else {
            owner_pid
        };

        if rule.delay.is_zero() {
            for spec in &rule.windows {
                Self::push_window(
                    &mut s,
                    owner_pid,
                    &spec.title,
                    spec.class_name.clone(),
                    spec.bounds,
                );
            }
        } else {
            let state = Arc::clone(&self.state);
            let windows = rule.windows.clone();
            let delay = rule.delay;
            std::thread::spawn(move || {
                std::thread::sleep(delay);
                let mut s = state.lock().unwrap();
                for spec in &windows {
                    Self::push_window(
                        &mut s,
                        owner_pid,
                        &spec.title,
                        spec.class_name.clone(),
                        spec.bounds,
                    );
                }
            });
        }
        Ok(LaunchedProcess {
            pid: Some(launched_pid),
        })
    }

    fn set_window_placement(
        &self,
        window: &WindowId,
        placement: &WindowPlacement,
    ) -> PlatformResult<()> {
        let mut s = self.state();
        s.placements.push((window.clone(), *placement));
        let monitors = s.monitors.clone();
        let w = s
            .windows
            .iter_mut()
            .find(|w| &w.id == window)
            .ok_or_else(|| PlatformError::not_found(format!("window {window}")))?;
        w.normal_bounds = Some(placement.bounds);
        w.state = placement.state;
        w.bounds = match placement.state {
            WindowState::Maximized | WindowState::Fullscreen => monitors
                .iter()
                .max_by_key(|m| m.bounds.intersection_area(&placement.bounds))
                .map(|m| {
                    if placement.state == WindowState::Maximized {
                        m.work_area
                    } else {
                        m.bounds
                    }
                })
                .unwrap_or(placement.bounds),
            _ => placement.bounds,
        };
        w.monitor_id = monitors
            .iter()
            .max_by_key(|m| m.bounds.intersection_area(&placement.bounds))
            .map(|m| m.id.clone());
        Ok(())
    }

    fn close_window(&self, window: &WindowId) -> PlatformResult<()> {
        let mut s = self.state();
        s.close_requests.push(window.clone());
        let Some(index) = s.windows.iter().position(|w| &w.id == window) else {
            return Err(PlatformError::not_found(format!("window {window}")));
        };
        let title = s.windows[index].title.clone();
        if s.refuse_close.iter().any(|t| title.contains(t.as_str())) {
            return Ok(()); // the app shows a "save changes?" prompt and stays open
        }
        let pid = s.windows.remove(index).pid;
        if !s.windows.iter().any(|w| w.pid == pid) {
            s.processes.retain(|p| p.pid != pid);
        }
        Ok(())
    }

    fn focus_window(&self, window: &WindowId) -> PlatformResult<()> {
        let mut s = self.state();
        s.focus_requests.push(window.clone());
        if !s.windows.iter().any(|w| &w.id == window) {
            return Err(PlatformError::not_found(format!("window {window}")));
        }
        for w in &mut s.windows {
            w.is_focused = &w.id == window;
        }
        Ok(())
    }

    fn application_icon(&self, _request: &IconRequest) -> PlatformResult<Option<Vec<u8>>> {
        Ok(None)
    }

    fn locate_application(&self, query: &LocateQuery) -> Vec<LaunchTarget> {
        let s = self.state();
        s.installed
            .iter()
            .filter(|(name, _)| {
                query
                    .executable_names
                    .iter()
                    .any(|n| n.eq_ignore_ascii_case(name))
            })
            .map(|(_, target)| target.clone())
            .collect()
    }

    fn permissions(&self) -> Vec<PermissionStatus> {
        Vec::new()
    }

    fn request_permission(&self, _kind: PermissionKind) -> PlatformResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_rule_spawns_windows_and_logs() {
        let p = MockPlatform::new();
        p.add_rule(LaunchRule::new("editor").window(MockWindowSpec::titled("Doc — Editor")));
        let launched = p
            .launch(&LaunchRequest::executable("/usr/bin/editor"))
            .unwrap();
        assert!(launched.pid.is_some());
        let windows = p.windows().unwrap();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].title, "Doc — Editor");
        assert_eq!(p.launches().len(), 1);
    }

    #[test]
    fn failing_rule_fails_then_succeeds() {
        let p = MockPlatform::new();
        p.add_rule(
            LaunchRule::new("flaky")
                .fail_times(1)
                .window(MockWindowSpec::titled("Flaky")),
        );
        assert!(p.launch(&LaunchRequest::executable("flaky")).is_err());
        assert!(p.launch(&LaunchRequest::executable("flaky")).is_ok());
        assert_eq!(p.windows().unwrap().len(), 1);
    }

    #[test]
    fn refused_close_keeps_window_open() {
        let p = MockPlatform::new();
        let pid = p.add_process("notes", None);
        let unsaved = p.add_window(pid, "notes.txt* — Notes", None, Rect::new(0, 0, 500, 500));
        let saved = p.add_window(pid, "todo.txt — Notes", None, Rect::new(0, 0, 500, 500));
        p.refuse_close("*");
        p.close_window(&unsaved).unwrap();
        p.close_window(&saved).unwrap();
        let ids: Vec<_> = p.windows().unwrap().into_iter().map(|w| w.id).collect();
        assert_eq!(ids, vec![unsaved]);
    }

    #[test]
    fn placement_updates_geometry_and_monitor() {
        let p = MockPlatform::new().with_monitors(vec![
            MonitorInfo {
                id: "A".into(),
                name: "A".into(),
                bounds: Rect::new(0, 0, 1000, 800),
                work_area: Rect::new(0, 0, 1000, 760),
                scale_factor: 1.0,
                is_primary: true,
            },
            MonitorInfo {
                id: "B".into(),
                name: "B".into(),
                bounds: Rect::new(1000, 0, 1000, 800),
                work_area: Rect::new(1000, 0, 1000, 760),
                scale_factor: 1.0,
                is_primary: false,
            },
        ]);
        let pid = p.add_process("x", None);
        let w = p.add_window(pid, "x", None, Rect::new(10, 10, 300, 300));
        p.set_window_placement(
            &w,
            &WindowPlacement {
                bounds: Rect::new(1100, 50, 400, 300),
                state: WindowState::Maximized,
            },
        )
        .unwrap();
        let info = p.window(&w).unwrap();
        assert_eq!(info.monitor_id.as_deref(), Some("B"));
        assert_eq!(info.bounds, Rect::new(1000, 0, 1000, 760));
        assert_eq!(info.state, WindowState::Maximized);
    }
}
