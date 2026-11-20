//! Process enumeration via `sysinfo` (works on Windows, macOS and Linux).

use std::collections::HashMap;

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::{PlatformError, PlatformResult, ProcessInfo};

/// Snapshot of all processes visible to the current user.
///
/// Command lines and working directories may be empty for processes owned by other users or
/// protected by the OS.
pub fn list_processes() -> PlatformResult<Vec<ProcessInfo>> {
    let mut system =
        System::new().map_err(|e| PlatformError::os("Listing processes", e.to_string()))?;
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_cmd(UpdateKind::OnlyIfNotSet)
            .with_cwd(UpdateKind::OnlyIfNotSet),
    );

    let mut processes: Vec<ProcessInfo> = system
        .processes()
        .iter()
        .filter(|(_, p)| p.thread_kind().is_none()) // skip Linux threads (tasks)
        .map(|(pid, p)| ProcessInfo {
            pid: pid.as_u32(),
            parent_pid: p.parent().map(|pp| pp.as_u32()),
            name: p.name().to_string_lossy().into_owned(),
            product_name: None,
            executable_path: p.exe().map(|e| e.to_path_buf()),
            command_line: p
                .cmd()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect(),
            working_directory: p.cwd().map(|c| c.to_path_buf()),
            start_time: Some(p.start_time()),
            package: None,
        })
        .collect();
    processes.sort_by_key(|p| p.pid);
    Ok(processes)
}

/// Human-readable OS name and version, e.g. "Ubuntu 24.04", "Windows 11 Pro 26100".
pub fn long_os_version() -> String {
    System::long_os_version().unwrap_or_else(|_| std::env::consts::OS.to_string())
}

/// All descendants of `root` (children, grandchildren...), breadth first.
pub fn descendants(processes: &[ProcessInfo], root: u32) -> Vec<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for p in processes {
        if let Some(parent) = p.parent_pid
            && parent != p.pid
        {
            children.entry(parent).or_default().push(p.pid);
        }
    }
    let mut out = Vec::new();
    let mut queue = vec![root];
    while let Some(pid) = queue.pop() {
        if let Some(kids) = children.get(&pid) {
            for &kid in kids {
                if !out.contains(&kid) && kid != root {
                    out.push(kid);
                    queue.push(kid);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, parent: Option<u32>) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid: parent,
            name: format!("p{pid}"),
            product_name: None,
            executable_path: None,
            command_line: vec![],
            working_directory: None,
            start_time: None,
            package: None,
        }
    }

    #[test]
    fn descendants_walks_the_tree() {
        let ps = vec![
            proc(1, None),
            proc(2, Some(1)),
            proc(3, Some(2)),
            proc(4, Some(1)),
            proc(5, None),
        ];
        let mut d = descendants(&ps, 1);
        d.sort();
        assert_eq!(d, vec![2, 3, 4]);
        assert!(descendants(&ps, 5).is_empty());
    }

    #[test]
    fn lists_current_process() {
        let me = std::process::id();
        assert!(list_processes().unwrap().iter().any(|p| p.pid == me));
    }
}
