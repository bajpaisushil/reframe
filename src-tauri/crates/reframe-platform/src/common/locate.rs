//! Locating installed applications: shared PATH lookup used by every platform.

use std::path::PathBuf;

use crate::LocateQuery;

/// Resolve executable names through the `PATH` environment variable.
pub fn find_on_path(query: &LocateQuery) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for name in &query.executable_names {
        if let Ok(path) = which::which(name)
            && !found.contains(&path)
        {
            found.push(path);
        }
    }
    found
}
