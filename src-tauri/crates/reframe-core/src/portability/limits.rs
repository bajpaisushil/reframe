//! Size limits of the export format.
//!
//! Imports exceeding them are rejected with `CoreError::InvalidImport`. Exports are trimmed to
//! them where that loses nothing important (titles, tab lists) and refused otherwise, so Reframe
//! never writes a file it would not import again. Workspace-level limits (name, description,
//! application and window counts) live with the model in `model::workspace`, and the file size
//! limit is `model::MAX_IMPORT_BYTES`.

/// Titles (windows, tabs, projects, folders, terminals), names, labels, ids and profile names.
pub const MAX_TEXT_LEN: usize = 1024;
/// URLs of browser tabs and media links.
pub const MAX_URL_LEN: usize = 4096;
/// Filesystem paths.
pub const MAX_PATH_LEN: usize = 4096;
/// Terminal startup commands. Longer commands are dropped on export, never truncated: a
/// truncated command could do something different from the original.
pub const MAX_COMMAND_LEN: usize = 4096;
/// Launch arguments per application.
pub const MAX_ARGUMENTS: usize = 32;
/// Characters per launch argument.
pub const MAX_ARGUMENT_LEN: usize = 1024;
/// Browser windows per application.
pub const MAX_BROWSER_WINDOWS: usize = 50;
/// Tabs per browser window.
pub const MAX_TABS_PER_WINDOW: usize = 500;
/// Documents, projects, folders or terminal sessions per application.
pub const MAX_STATE_ITEMS: usize = 500;
/// Monitors in the machine profile.
pub const MAX_MONITORS: usize = 32;
/// Window and monitor widths and heights must lie in `1..MAX_DIMENSION`.
pub const MAX_DIMENSION: i32 = 100_000;
/// Window and monitor coordinates must lie in `-MAX_COORDINATE..=MAX_COORDINATE` (keeps
/// `x + width` far away from integer overflow).
pub const MAX_COORDINATE: i32 = 1_000_000;
/// Monitor scale factors must lie in `(0, MAX_SCALE_FACTOR]`.
pub const MAX_SCALE_FACTOR: f64 = 16.0;
/// The workspace icon (one emoji, possibly a multi-codepoint sequence); same rule as
/// `Workspace::validate`.
pub const MAX_ICON_LEN: usize = 16;
/// Per-application launch delay override (same bound as the workspace launch delay).
pub const MAX_LAUNCH_DELAY_MS: u32 = 60_000;
/// Per-application window timeout override bounds (same as the workspace timeout).
pub const MIN_TIMEOUT_MS: u32 = 1_000;
pub const MAX_TIMEOUT_MS: u32 = 300_000;
/// Per-application retry override (same bound as the workspace retry count).
pub const MAX_RETRIES: u32 = 5;
/// Import files nested deeper than this are rejected (exports need about 10 levels; the JSON
/// parser itself stops at 128).
pub const MAX_JSON_DEPTH: usize = 64;
/// Import files with a list or object of more entries are rejected before anything is
/// deserialized (the largest list of an export has 500 entries).
pub const MAX_JSON_ENTRIES: usize = 10_000;
/// Import files with more values (strings, numbers, lists...) are rejected before anything is
/// deserialized. A 5 MB export of real data has well under half a million.
pub const MAX_JSON_VALUES: usize = 1_000_000;
