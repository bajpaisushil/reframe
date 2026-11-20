//! Chromium session files ("SNSS"): record framing, `base::Pickle` payloads and replay of the
//! session command log into windows and tabs.
//!
//! The format below was checked against Chromium's `components/sessions/core`
//! (`command_storage_backend.cc`, `session_command.cc`, `session_service_commands.cc`,
//! `base_session_service_commands.cc`, `serialized_navigation_entry.cc`) and `base/pickle.cc`:
//!
//! - **Header**: the bytes `SNSS` followed by an `int32` version. Version 1 (older browsers) and
//!   version 3 (current; the initial state is followed by a marker record, id 255) are plain.
//!   Versions 2, 4 and 5 are encrypted by the browser (5 is stored in `Sessions_Encrypted/`)
//!   and cannot be read without the browser's OS-protected key.
//! - **Records**: a `uint16` size that counts the 1-byte command id, the command id, then
//!   `size - 1` payload bytes. Integers are little-endian (every platform Chromium ships on).
//! - **Payloads** are either fixed C structs, accepted only with their exact size (like
//!   `SessionCommand::GetContents`), or `base::Pickle`s: a `uint32` payload length, then 4-byte
//!   aligned fields; strings are an `int32` byte count plus bytes, UTF-16 strings an `int32`
//!   code-unit count plus UTF-16LE data, both padded to a multiple of 4 bytes.
//!
//! Replay follows `CreateTabsAndWindows`, `AddTabsToWindows`,
//! `SortTabsBasedOnVisualOrderAndClear` and `UpdateSelectedTabIndex`: windows exist only once a
//! window-type record "unconstrains" them, closed tabs and windows are forgotten (records that
//! mention them again create fresh, constrained entries), tabs without navigations or window
//! are dropped, tabs are ordered by (visual index, tab id), the selected navigation is the first
//! one whose index is at least the recorded current index (else the last one), and the selected
//! tab is the one whose visual index equals the window's selected index (else the first tab).
//!
//! Robustness: parsing is a bounded linear scan over a byte slice. It never panics, borrows
//! record payloads instead of copying them, and caps the input size, the number of records and
//! the number of live tabs, windows and navigations ([`Limits`]). Problems are handled the way
//! Chromium handles them, because a command log replayed up to some record is always a
//! consistent, earlier state of the session:
//!
//! - A record cut off at the end of the file (or a zero-length record) ends the data, keeping
//!   everything before it: the browser appends to the file while it is being read. Reported as
//!   [`Session::truncated`].
//! - A record of a known command whose payload does not have the required layout stops the
//!   replay (as in Chromium) and keeps the state built so far. Reported as [`Session::damaged`].
//! - Unknown command ids are skipped (newer browsers add commands Reframe does not need).
//! - Anything else — not a session file, unsupported or encrypted version, a version-3 file
//!   without its marker (an interrupted rewrite, which Chromium also ignores), too large, too
//!   many records — is an [`SnssError`].

use std::collections::HashMap;

use reframe_platform::Rect;

/// Command ids from `session_service_commands.cc`. Only the commands that affect windows, tabs
/// and their current page are interpreted; all others are skipped.
mod command {
    pub const SET_TAB_WINDOW: u8 = 0;
    pub const SET_TAB_INDEX_IN_WINDOW: u8 = 2;
    pub const TAB_NAVIGATION_PATH_PRUNED_FROM_BACK: u8 = 5;
    pub const UPDATE_TAB_NAVIGATION: u8 = 6;
    pub const SET_SELECTED_NAVIGATION_INDEX: u8 = 7;
    pub const SET_SELECTED_TAB_IN_INDEX: u8 = 8;
    pub const SET_WINDOW_TYPE: u8 = 9;
    /// Obsolete, still replayed by Chromium for old files.
    pub const SET_WINDOW_BOUNDS2: u8 = 10;
    pub const TAB_NAVIGATION_PATH_PRUNED_FROM_FRONT: u8 = 11;
    pub const SET_PINNED_STATE: u8 = 12;
    pub const SET_WINDOW_BOUNDS3: u8 = 14;
    pub const TAB_CLOSED: u8 = 16;
    pub const WINDOW_CLOSED: u8 = 17;
    pub const SET_ACTIVE_WINDOW: u8 = 20;
    pub const TAB_NAVIGATION_PATH_PRUNED: u8 = 24;
    pub const SET_WINDOW_USER_TITLE: u8 = 31;
    /// `kInitialStateMarkerCommandId` (version 3 files only).
    pub const INITIAL_STATE_MARKER: u8 = 255;
}

const SIGNATURE: &[u8; 4] = b"SNSS";
const HEADER_LEN: usize = 8;
const VERSION_PLAIN: i32 = 1;
const VERSION_PLAIN_WITH_MARKER: i32 = 3;
const ENCRYPTED_VERSIONS: [i32; 3] = [2, 4, 5];

/// `SessionWindow::TYPE_NORMAL`: a regular tabbed browser window.
pub const WINDOW_TYPE_NORMAL: i32 = 0;

/// Resource limits for parsing one file. The defaults are far above anything a browser writes
/// (it rewrites the file from scratch every few hundred commands) and only stop hostile input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_file_bytes: usize,
    pub max_records: usize,
    pub max_tabs: usize,
    pub max_windows: usize,
    pub max_navigations_per_tab: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_file_bytes: 256 * 1024 * 1024,
            max_records: 4_000_000,
            max_tabs: 50_000,
            max_windows: 5_000,
            max_navigations_per_tab: 1_000,
        }
    }
}

/// Why a file could not be used at all.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SnssError {
    #[error("the session file is empty or cut off")]
    TooShort,
    #[error("the file is not a Chromium session file")]
    BadSignature,
    #[error("the session file is encrypted by the browser (format version {version})")]
    Encrypted { version: i32 },
    #[error("the session file uses an unsupported format version ({0})")]
    UnsupportedVersion(i32),
    #[error("the session file is too large ({bytes} bytes)")]
    TooLarge { bytes: usize },
    #[error("the session file contains too many records")]
    TooManyRecords,
    #[error("the session file was not completely written")]
    Incomplete,
}

/// `ui::WindowShowState` as persisted by `SetWindowBounds3`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShowState {
    #[default]
    Default,
    Normal,
    Minimized,
    Maximized,
    Fullscreen,
}

impl ShowState {
    /// `PersistedShowStateToShowState`: unknown and deprecated values mean "normal".
    fn from_persisted(value: i32) -> Self {
        match value {
            2 => ShowState::Minimized,
            3 => ShowState::Maximized,
            5 => ShowState::Fullscreen,
            _ => ShowState::Normal,
        }
    }
}

/// The current page of one tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTab {
    /// The navigation's virtual URL, as stored (not yet validated).
    pub url: String,
    /// Page title, control characters replaced, possibly empty.
    pub title: String,
    pub pinned: bool,
}

/// One restorable window of the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionWindow {
    pub id: i32,
    /// `SessionWindow::WindowType` (0 normal, 1 popup, 2 app, 3 devtools, 4 app popup...).
    pub window_type: i32,
    /// Restored (non-maximized) bounds as saved by the browser, in its screen coordinates.
    pub bounds: Option<Rect>,
    pub show_state: ShowState,
    /// Name the user gave the window ("Name window..."), shown instead of the tab title.
    pub user_title: Option<String>,
    /// Tabs in tab-strip order; never empty.
    pub tabs: Vec<SessionTab>,
    /// Index into `tabs` of the selected tab.
    pub selected_tab: usize,
}

impl SessionWindow {
    pub fn is_normal(&self) -> bool {
        self.window_type == WINDOW_TYPE_NORMAL
    }

    pub fn active_tab(&self) -> Option<&SessionTab> {
        self.tabs.get(self.selected_tab)
    }
}

/// A replayed session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Session {
    /// File format version (1 or 3).
    pub version: i32,
    /// Every restorable window, all types, in creation (id) order.
    pub windows: Vec<SessionWindow>,
    /// The window that was active last.
    pub active_window: Option<i32>,
    /// Replay stopped at a record with an invalid payload; the windows reflect the session as
    /// it was just before that record.
    pub damaged: bool,
    /// The final record was cut off (normal while the browser is writing).
    pub truncated: bool,
    /// The file declared more tabs, windows or navigations than [`Limits`] allow; extra ones
    /// were ignored.
    pub limits_exceeded: bool,
}

impl Session {
    /// Regular tabbed browser windows (no popups, app or DevTools windows).
    pub fn normal_windows(&self) -> impl Iterator<Item = &SessionWindow> {
        self.windows.iter().filter(|w| w.is_normal())
    }
}

/// Parse and replay a session file with the default [`Limits`].
pub fn parse_session(data: &[u8]) -> Result<Session, SnssError> {
    parse_session_with(data, &Limits::default())
}

/// Parse and replay a session file.
pub fn parse_session_with(data: &[u8], limits: &Limits) -> Result<Session, SnssError> {
    let mut records = Records::new(data, limits)?;
    let mut replay = Replay::new(limits);
    let mut damaged = false;
    let mut marker_seen = false;
    while let Some(record) = records.next_record()? {
        if records.version == VERSION_PLAIN_WITH_MARKER
            && record.id == command::INITIAL_STATE_MARKER
        {
            marker_seen = true;
            continue;
        }
        // After a damaged record keep scanning the framing (the marker and the record limit
        // still matter) but stop replaying, exactly like Chromium.
        if !damaged && replay.apply(record.id, record.payload).is_err() {
            damaged = true;
        }
    }
    if records.version == VERSION_PLAIN_WITH_MARKER && !marker_seen {
        return Err(SnssError::Incomplete);
    }
    Ok(replay.finish(records.version, records.truncated, damaged))
}

// ---------------------------------------------------------------------------------------------
// Framing
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct Record<'a> {
    id: u8,
    payload: &'a [u8],
}

struct Records<'a> {
    data: &'a [u8],
    pos: usize,
    version: i32,
    count: usize,
    max_records: usize,
    truncated: bool,
}

impl<'a> Records<'a> {
    fn new(data: &'a [u8], limits: &Limits) -> Result<Self, SnssError> {
        if data.len() > limits.max_file_bytes {
            return Err(SnssError::TooLarge { bytes: data.len() });
        }
        let Some(header) = data.get(..HEADER_LEN) else {
            return Err(SnssError::TooShort);
        };
        if &header[..4] != SIGNATURE {
            return Err(SnssError::BadSignature);
        }
        let version = i32::from_le_bytes([header[4], header[5], header[6], header[7]]);
        if ENCRYPTED_VERSIONS.contains(&version) {
            return Err(SnssError::Encrypted { version });
        }
        if version != VERSION_PLAIN && version != VERSION_PLAIN_WITH_MARKER {
            return Err(SnssError::UnsupportedVersion(version));
        }
        Ok(Self {
            data,
            pos: HEADER_LEN,
            version,
            count: 0,
            max_records: limits.max_records,
            truncated: false,
        })
    }

    /// The next complete record, `None` at the end of the data (including a cut-off tail).
    fn next_record(&mut self) -> Result<Option<Record<'a>>, SnssError> {
        if self.truncated {
            return Ok(None);
        }
        let rest = self.data.get(self.pos..).unwrap_or_default();
        if rest.is_empty() {
            return Ok(None);
        }
        let (size, body) = match rest {
            [lo, hi, body @ ..] => (usize::from(u16::from_le_bytes([*lo, *hi])), body),
            _ => {
                self.truncated = true;
                return Ok(None);
            }
        };
        // A size of 0 cannot even hold the command id; a size past the end is a partial write.
        let Some(record) = body.get(..size).filter(|r| !r.is_empty()) else {
            self.truncated = true;
            return Ok(None);
        };
        self.count += 1;
        if self.count > self.max_records {
            return Err(SnssError::TooManyRecords);
        }
        self.pos += 2 + size;
        Ok(Some(Record {
            id: record[0],
            payload: &record[1..],
        }))
    }
}

// ---------------------------------------------------------------------------------------------
// Payloads
// ---------------------------------------------------------------------------------------------

/// A record payload without the layout its command requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Malformed;

/// A struct payload made of `N` `int32` fields, accepted only with its exact size.
fn int_fields<const N: usize>(payload: &[u8]) -> Result<[i32; N], Malformed> {
    if payload.len() != N * 4 {
        return Err(Malformed);
    }
    let mut out = [0i32; N];
    for (slot, chunk) in out.iter_mut().zip(payload.as_chunks::<4>().0) {
        *slot = i32::from_le_bytes(*chunk);
    }
    Ok(out)
}

/// `int32` at `offset` of a payload whose length was already checked.
fn int_at(payload: &[u8], offset: usize) -> Result<i32, Malformed> {
    payload
        .get(offset..offset + 4)
        .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or(Malformed)
}

/// Reader over a serialized `base::Pickle`, with `base::PickleIterator`'s bounds and
/// alignment rules.
struct Pickle<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Pickle<'a> {
    fn new(contents: &'a [u8]) -> Result<Self, Malformed> {
        let header = contents.get(..4).ok_or(Malformed)?;
        let payload_len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let payload_len = usize::try_from(payload_len).map_err(|_| Malformed)?;
        if payload_len > contents.len() - 4 {
            return Err(Malformed);
        }
        let header_len = contents.len() - payload_len;
        if !header_len.is_multiple_of(4) {
            return Err(Malformed);
        }
        Ok(Self {
            data: &contents[header_len..],
            pos: 0,
        })
    }

    fn bytes(&mut self, len: usize) -> Result<&'a [u8], Malformed> {
        let end = self.pos.checked_add(len).ok_or(Malformed)?;
        let out = self.data.get(self.pos..end).ok_or(Malformed)?;
        // Fields are padded to 4 bytes; the padding of the last field may be missing.
        let padded = len.checked_next_multiple_of(4).ok_or(Malformed)?;
        self.pos = self.pos.saturating_add(padded).min(self.data.len());
        Ok(out)
    }

    fn int(&mut self) -> Result<i32, Malformed> {
        let b = self.bytes(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// `std::string`: `int32` byte count, then the bytes.
    fn string(&mut self) -> Result<&'a [u8], Malformed> {
        let len = usize::try_from(self.int()?).map_err(|_| Malformed)?;
        self.bytes(len)
    }

    /// `std::u16string`: `int32` code-unit count, then UTF-16LE data. Invalid UTF-16 is
    /// replaced, not rejected (the browser stores whatever the page set as its title).
    fn string16(&mut self) -> Result<String, Malformed> {
        let units = usize::try_from(self.int()?).map_err(|_| Malformed)?;
        let raw = self.bytes(units.checked_mul(2).ok_or(Malformed)?)?;
        let utf16: Vec<u16> = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        Ok(String::from_utf16_lossy(&utf16))
    }
}

/// `UpdateTabNavigation`: tab id, then `SerializedNavigationEntry::WriteToPickle` whose
/// required leading fields are index, virtual URL, title, page state and transition type.
fn decode_navigation(payload: &[u8]) -> Result<(i32, Navigation), Malformed> {
    let mut pickle = Pickle::new(payload)?;
    let tab_id = pickle.int()?;
    let index = pickle.int()?;
    let url = String::from_utf8_lossy(pickle.string()?).into_owned();
    let title = pickle.string16()?;
    let _page_state = pickle.string()?;
    let _transition_type = pickle.int()?;
    Ok((tab_id, Navigation { index, url, title }))
}

/// `SetWindowUserTitle`: window id, then the title as a UTF-8 `std::string`.
fn decode_user_title(payload: &[u8]) -> Result<(i32, String), Malformed> {
    let mut pickle = Pickle::new(payload)?;
    let window_id = pickle.int()?;
    let title = String::from_utf8_lossy(pickle.string()?).into_owned();
    Ok((window_id, title))
}

/// `ClosedPayload { id_type id; int64_t close_time; }`: 16 bytes with 8-byte `int64`
/// alignment, 12 bytes on 32-bit x86 Linux.
fn decode_closed(payload: &[u8]) -> Result<i32, Malformed> {
    match payload.len() {
        12 | 16 => int_at(payload, 0),
        _ => Err(Malformed),
    }
}

// ---------------------------------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Navigation {
    index: i32,
    url: String,
    title: String,
}

#[derive(Debug)]
struct TabBuilder {
    window_id: Option<i32>,
    visual_index: i32,
    current_navigation: i32,
    pinned: bool,
    /// Sorted by `index`, unique indices.
    navigations: Vec<Navigation>,
}

impl Default for TabBuilder {
    fn default() -> Self {
        Self {
            window_id: None,
            visual_index: -1,
            current_navigation: -1,
            pinned: false,
            navigations: Vec::new(),
        }
    }
}

impl TabBuilder {
    /// `FindClosestNavigationWithIndex`: position of the first navigation whose index is at
    /// least `index`.
    fn first_at_or_after(&self, index: i64) -> usize {
        self.navigations
            .partition_point(|n| i64::from(n.index) < index)
    }

    /// `ProcessTabNavigationPathPrunedCommand`: remove `count` navigations starting at index
    /// `index` and shift later ones down.
    fn prune(&mut self, index: i32, count: i32) {
        let (index, count) = (i64::from(index), i64::from(count));
        let end = index + count;
        let current = i64::from(self.current_navigation);
        let current = if (index..end).contains(&current) {
            index - 1
        } else if current >= end {
            current - count
        } else {
            current
        };
        self.current_navigation = clamp_i32(current);
        let from = self.first_at_or_after(index);
        let to = self.first_at_or_after(end);
        self.navigations.drain(from..to);
        for nav in &mut self.navigations {
            if i64::from(nav.index) >= index {
                nav.index = clamp_i32(i64::from(nav.index) - count);
            }
        }
    }

    /// Insert a navigation or replace the one with the same index. Returns false when the
    /// per-tab limit prevents inserting a new index.
    fn upsert(&mut self, navigation: Navigation, max: usize) -> bool {
        let pos = self.first_at_or_after(i64::from(navigation.index));
        if let Some(existing) = self
            .navigations
            .get_mut(pos)
            .filter(|existing| existing.index == navigation.index)
        {
            *existing = navigation;
            return true;
        }
        if self.navigations.len() >= max {
            return false;
        }
        self.navigations.insert(pos, navigation);
        true
    }
}

fn clamp_i32(value: i64) -> i32 {
    i32::try_from(value).unwrap_or(if value < 0 { i32::MIN } else { i32::MAX })
}

#[derive(Debug)]
struct WindowBuilder {
    selected_tab_index: i32,
    window_type: i32,
    /// Chromium creates windows "constrained" and prunes them unless a window-type record
    /// arrives.
    constrained: bool,
    bounds: Option<Rect>,
    show_state: ShowState,
    user_title: Option<String>,
}

impl Default for WindowBuilder {
    fn default() -> Self {
        Self {
            selected_tab_index: -1,
            window_type: WINDOW_TYPE_NORMAL,
            constrained: true,
            bounds: None,
            show_state: ShowState::Default,
            user_title: None,
        }
    }
}

struct Replay<'l> {
    limits: &'l Limits,
    tabs: HashMap<i32, TabBuilder>,
    windows: HashMap<i32, WindowBuilder>,
    active_window: Option<i32>,
    limits_exceeded: bool,
}

impl<'l> Replay<'l> {
    fn new(limits: &'l Limits) -> Self {
        Self {
            limits,
            tabs: HashMap::new(),
            windows: HashMap::new(),
            active_window: None,
            limits_exceeded: false,
        }
    }

    /// `GetTab`: the tab with `id`, created on first use (unless the limit is reached).
    fn tab(&mut self, id: i32) -> Option<&mut TabBuilder> {
        if !self.tabs.contains_key(&id) && self.tabs.len() >= self.limits.max_tabs {
            self.limits_exceeded = true;
            return None;
        }
        Some(self.tabs.entry(id).or_default())
    }

    /// `GetWindow`: the window with `id`, created on first use (unless the limit is reached).
    fn window(&mut self, id: i32) -> Option<&mut WindowBuilder> {
        if !self.windows.contains_key(&id) && self.windows.len() >= self.limits.max_windows {
            self.limits_exceeded = true;
            return None;
        }
        Some(self.windows.entry(id).or_default())
    }

    fn apply(&mut self, id: u8, payload: &[u8]) -> Result<(), Malformed> {
        match id {
            command::SET_TAB_WINDOW => {
                let [window_id, tab_id] = int_fields(payload)?;
                if let Some(tab) = self.tab(tab_id) {
                    tab.window_id = Some(window_id);
                }
            }
            command::SET_TAB_INDEX_IN_WINDOW => {
                let [tab_id, index] = int_fields(payload)?;
                if let Some(tab) = self.tab(tab_id) {
                    tab.visual_index = index;
                }
            }
            command::TAB_NAVIGATION_PATH_PRUNED_FROM_BACK => {
                let [tab_id, index] = int_fields(payload)?;
                if let Some(tab) = self.tab(tab_id) {
                    let from = tab.first_at_or_after(i64::from(index));
                    tab.navigations.truncate(from);
                }
            }
            command::UPDATE_TAB_NAVIGATION => {
                let (tab_id, navigation) = decode_navigation(payload)?;
                let max = self.limits.max_navigations_per_tab;
                let inserted = self.tab(tab_id).map(|tab| tab.upsert(navigation, max));
                if inserted == Some(false) {
                    self.limits_exceeded = true;
                }
            }
            command::SET_SELECTED_NAVIGATION_INDEX => {
                let [tab_id, index] = int_fields(payload)?;
                if let Some(tab) = self.tab(tab_id) {
                    tab.current_navigation = index;
                }
            }
            command::SET_SELECTED_TAB_IN_INDEX => {
                let [window_id, index] = int_fields(payload)?;
                if let Some(window) = self.window(window_id) {
                    window.selected_tab_index = index;
                }
            }
            command::SET_WINDOW_TYPE => {
                let [window_id, window_type] = int_fields(payload)?;
                if let Some(window) = self.window(window_id) {
                    window.constrained = false;
                    window.window_type = window_type;
                }
            }
            command::SET_WINDOW_BOUNDS2 => {
                // { id, x, y, w, h, bool is_maximized } padded to 24 bytes.
                if payload.len() != 24 {
                    return Err(Malformed);
                }
                let [window_id, x, y, w, h] = int_fields(&payload[..20])?;
                let maximized = payload[20] != 0;
                if let Some(window) = self.window(window_id) {
                    window.bounds = Some(Rect::new(x, y, w, h));
                    window.show_state = if maximized {
                        ShowState::Maximized
                    } else {
                        ShowState::Normal
                    };
                }
            }
            command::TAB_NAVIGATION_PATH_PRUNED_FROM_FRONT => {
                let [tab_id, count] = int_fields(payload)?;
                if count <= 0 {
                    return Err(Malformed);
                }
                if let Some(tab) = self.tab(tab_id) {
                    tab.prune(0, count);
                }
            }
            command::SET_PINNED_STATE => {
                // { id, bool pinned } padded to 8 bytes.
                if payload.len() != 8 {
                    return Err(Malformed);
                }
                let tab_id = int_at(payload, 0)?;
                let pinned = payload[4] != 0;
                if let Some(tab) = self.tab(tab_id) {
                    tab.pinned = pinned;
                }
            }
            command::SET_WINDOW_BOUNDS3 => {
                let [window_id, x, y, w, h, show_state] = int_fields(payload)?;
                if let Some(window) = self.window(window_id) {
                    window.bounds = Some(Rect::new(x, y, w, h));
                    window.show_state = ShowState::from_persisted(show_state);
                }
            }
            command::TAB_CLOSED => {
                let tab_id = decode_closed(payload)?;
                self.tabs.remove(&tab_id);
            }
            command::WINDOW_CLOSED => {
                let window_id = decode_closed(payload)?;
                self.windows.remove(&window_id);
            }
            command::SET_ACTIVE_WINDOW => {
                let [window_id] = int_fields(payload)?;
                self.active_window = Some(window_id);
            }
            command::TAB_NAVIGATION_PATH_PRUNED => {
                let [tab_id, index, count] = int_fields(payload)?;
                if index < 0 || count <= 0 {
                    return Err(Malformed);
                }
                if let Some(tab) = self.tab(tab_id) {
                    tab.prune(index, count);
                }
            }
            command::SET_WINDOW_USER_TITLE => {
                let (window_id, title) = decode_user_title(payload)?;
                let title = clean_text(&title);
                if let Some(window) = self.window(window_id) {
                    window.user_title = (!title.is_empty()).then_some(title);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(self, version: i32, truncated: bool, damaged: bool) -> Session {
        let Replay {
            tabs,
            mut windows,
            active_window,
            limits_exceeded,
            ..
        } = self;

        // AddTabsToWindows: every tab with a window and at least one navigation.
        let mut grouped: HashMap<i32, Vec<(i32, i32, SessionTab)>> = HashMap::new();
        for (tab_id, tab) in tabs {
            let Some(window_id) = tab.window_id.filter(|id| *id != 0) else {
                continue;
            };
            let pos = tab.first_at_or_after(i64::from(tab.current_navigation));
            let TabBuilder {
                visual_index,
                pinned,
                mut navigations,
                ..
            } = tab;
            if navigations.is_empty() {
                continue;
            }
            let selected = pos.min(navigations.len() - 1);
            let navigation = navigations.swap_remove(selected);
            grouped.entry(window_id).or_default().push((
                visual_index,
                tab_id,
                SessionTab {
                    url: navigation.url,
                    title: clean_text(&navigation.title),
                    pinned,
                },
            ));
        }

        // SortTabsBasedOnVisualOrderAndClear + UpdateSelectedTabIndex. A tab whose window is
        // unknown would get a fresh, constrained window from Chromium: pruned as well.
        let mut out = Vec::new();
        for (window_id, mut window_tabs) in grouped {
            let Some(window) = windows.remove(&window_id) else {
                continue;
            };
            if window.constrained {
                continue;
            }
            window_tabs.sort_by_key(|(visual_index, tab_id, _)| (*visual_index, *tab_id));
            let selected_tab = window_tabs
                .iter()
                .position(|(visual_index, _, _)| *visual_index == window.selected_tab_index)
                .unwrap_or(0);
            out.push(SessionWindow {
                id: window_id,
                window_type: window.window_type,
                bounds: window.bounds,
                show_state: window.show_state,
                user_title: window.user_title,
                tabs: window_tabs.into_iter().map(|(_, _, tab)| tab).collect(),
                selected_tab,
            });
        }
        out.sort_by_key(|w| w.id);

        Session {
            version,
            windows: out,
            active_window,
            damaged,
            truncated,
            limits_exceeded,
        }
    }
}

/// Maximum characters kept from a page or window title.
const MAX_TITLE_CHARS: usize = 500;

/// Titles are shown in the UI: replace control characters, trim and bound the length.
fn clean_text(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_TITLE_CHARS)
        .collect();
    cleaned.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{PickleWriter, SnssWriter, standard_window};
    use super::*;

    fn urls(window: &SessionWindow) -> Vec<&str> {
        window.tabs.iter().map(|t| t.url.as_str()).collect()
    }

    #[test]
    fn replays_windows_and_tabs_in_order() {
        let data = SnssWriter::new(3)
            .window_type(1, 0)
            .bounds(1, 10, 20, 800, 600, 1)
            // Tabs written out of visual order.
            .tab_window(1, 12)
            .tab_index(12, 2)
            .navigation(12, 0, "https://c.example/", "C")
            .tab_window(1, 10)
            .tab_index(10, 0)
            .navigation(10, 0, "https://a.example/", "A")
            .tab_window(1, 11)
            .tab_index(11, 1)
            .navigation(11, 0, "https://b.example/", "B")
            .selected_tab(1, 1)
            .window_type(2, 0)
            .bounds(2, 900, 0, 640, 480, 3)
            .tab_window(2, 20)
            .tab_index(20, 0)
            .navigation(20, 0, "https://d.example/", "D")
            .selected_tab(2, 0)
            .active_window(2)
            .marker()
            .build();
        let session = parse_session(&data).unwrap();
        assert_eq!(session.version, 3);
        assert!(!session.damaged && !session.truncated && !session.limits_exceeded);
        assert_eq!(session.windows.len(), 2);
        let first = &session.windows[0];
        assert_eq!(first.id, 1);
        assert_eq!(
            urls(first),
            [
                "https://a.example/",
                "https://b.example/",
                "https://c.example/"
            ]
        );
        assert_eq!(first.selected_tab, 1);
        assert_eq!(first.active_tab().unwrap().title, "B");
        assert_eq!(first.bounds, Some(Rect::new(10, 20, 800, 600)));
        assert_eq!(first.show_state, ShowState::Normal);
        let second = &session.windows[1];
        assert_eq!(second.show_state, ShowState::Maximized);
        assert_eq!(session.active_window, Some(2));
    }

    #[test]
    fn version_1_files_need_no_marker() {
        let data = standard_window(
            SnssWriter::new(1),
            1,
            100,
            &[("https://a.example/", "A")],
            0,
        )
        .build();
        let session = parse_session(&data).unwrap();
        assert_eq!(session.version, 1);
        assert_eq!(session.windows.len(), 1);
    }

    #[test]
    fn version_3_files_without_marker_are_incomplete() {
        let data = standard_window(
            SnssWriter::new(3),
            1,
            100,
            &[("https://a.example/", "A")],
            0,
        )
        .build();
        assert_eq!(parse_session(&data), Err(SnssError::Incomplete));
        // Commands after the marker (incremental updates) are replayed too.
        let data = standard_window(
            SnssWriter::new(3),
            1,
            100,
            &[("https://a.example/", "A")],
            0,
        )
        .marker()
        .navigation(100, 1, "https://a.example/next", "Next")
        .selected_navigation(100, 1)
        .build();
        let session = parse_session(&data).unwrap();
        assert_eq!(urls(&session.windows[0]), ["https://a.example/next"]);
    }

    #[test]
    fn selected_navigation_uses_closest_following_index() {
        let base = || {
            SnssWriter::new(1)
                .window_type(1, 0)
                .tab_window(1, 5)
                .tab_index(5, 0)
                .navigation(5, 0, "https://example.com/0", "zero")
                .navigation(5, 1, "https://example.com/1", "one")
                .navigation(5, 3, "https://example.com/3", "three")
        };
        let selected = |writer: SnssWriter| {
            parse_session(&writer.build()).unwrap().windows[0].tabs[0]
                .url
                .clone()
        };
        // No selection recorded (-1): the first navigation.
        assert_eq!(selected(base()), "https://example.com/0");
        assert_eq!(
            selected(base().selected_navigation(5, 1)),
            "https://example.com/1"
        );
        // Index 2 does not exist: the next one (3).
        assert_eq!(
            selected(base().selected_navigation(5, 2)),
            "https://example.com/3"
        );
        // Beyond the last: the last one.
        assert_eq!(
            selected(base().selected_navigation(5, 9)),
            "https://example.com/3"
        );
        // Updating an existing index replaces it.
        assert_eq!(
            selected(
                base()
                    .navigation(5, 1, "https://example.com/new", "new")
                    .selected_navigation(5, 1)
            ),
            "https://example.com/new"
        );
    }

    #[test]
    fn closed_tabs_and_windows_are_removed() {
        let data = SnssWriter::new(1)
            .window_type(1, 0)
            .tab_window(1, 10)
            .tab_index(10, 0)
            .navigation(10, 0, "https://keep.example/", "Keep")
            .tab_window(1, 11)
            .tab_index(11, 1)
            .navigation(11, 0, "https://closed.example/", "Closed")
            .tab_closed(11)
            .window_type(2, 0)
            .tab_window(2, 20)
            .tab_index(20, 0)
            .navigation(20, 0, "https://gone.example/", "Gone")
            .window_closed(2)
            // A late record for the closed window must not bring it back.
            .bounds(2, 0, 0, 100, 100, 1)
            .build();
        let session = parse_session(&data).unwrap();
        assert_eq!(session.windows.len(), 1);
        assert_eq!(urls(&session.windows[0]), ["https://keep.example/"]);
    }

    #[test]
    fn pinned_state_and_user_title_are_read() {
        let data = SnssWriter::new(1)
            .window_type(1, 0)
            .user_title(1, "Research")
            .tab_window(1, 10)
            .tab_index(10, 0)
            .navigation(10, 0, "https://a.example/", "A")
            .pinned(10, true)
            .tab_window(1, 11)
            .tab_index(11, 1)
            .navigation(11, 0, "https://b.example/", "B")
            .pinned(11, true)
            .pinned(11, false)
            .build();
        let window = &parse_session(&data).unwrap().windows[0];
        assert_eq!(window.user_title.as_deref(), Some("Research"));
        assert!(window.tabs[0].pinned);
        assert!(!window.tabs[1].pinned);
    }

    #[test]
    fn unknown_and_unused_commands_are_skipped() {
        let extension_app_id = PickleWriter::new().int(10).string("abcdef").finish();
        let data = SnssWriter::new(1)
            .record(200, &[1, 2, 3, 4, 5])
            .window_type(1, 0)
            .record(13, &extension_app_id)
            .tab_window(1, 10)
            .record(21, &[0; 16]) // last active time
            .tab_index(10, 0)
            .navigation(10, 0, "https://a.example/", "A")
            .record(250, &[])
            .build();
        let session = parse_session(&data).unwrap();
        assert!(!session.damaged);
        assert_eq!(urls(&session.windows[0]), ["https://a.example/"]);
    }

    #[test]
    fn windows_need_a_type_and_tabs_need_a_window_and_navigation() {
        let data = SnssWriter::new(1)
            // Window 1 never gets a type: constrained, pruned.
            .bounds(1, 0, 0, 10, 10, 1)
            .tab_window(1, 10)
            .navigation(10, 0, "https://constrained.example/", "x")
            // Tab 11 has no window; tab 12 has no navigation.
            .window_type(2, 0)
            .navigation(11, 0, "https://orphan.example/", "x")
            .tab_window(2, 12)
            .tab_window(2, 13)
            .navigation(13, 0, "https://ok.example/", "ok")
            // Window 3 is a popup: kept, but not a normal window.
            .window_type(3, 1)
            .tab_window(3, 30)
            .navigation(30, 0, "https://popup.example/", "popup")
            .build();
        let session = parse_session(&data).unwrap();
        let ids: Vec<i32> = session.windows.iter().map(|w| w.id).collect();
        assert_eq!(ids, [2, 3]);
        assert_eq!(urls(&session.windows[0]), ["https://ok.example/"]);
        let normal: Vec<i32> = session.normal_windows().map(|w| w.id).collect();
        assert_eq!(normal, [2]);
    }

    #[test]
    fn selected_tab_falls_back_to_first_tab() {
        let data = SnssWriter::new(1)
            .window_type(1, 0)
            .tab_window(1, 10)
            .tab_index(10, 0)
            .navigation(10, 0, "https://a.example/", "A")
            .tab_window(1, 11)
            .tab_index(11, 1)
            .navigation(11, 0, "https://b.example/", "B")
            .selected_tab(1, 7)
            .build();
        assert_eq!(parse_session(&data).unwrap().windows[0].selected_tab, 0);
    }

    #[test]
    fn navigation_pruning_follows_chromium() {
        let tab = || {
            SnssWriter::new(1)
                .window_type(1, 0)
                .tab_window(1, 5)
                .navigation(5, 0, "https://e.com/0", "0")
                .navigation(5, 1, "https://e.com/1", "1")
                .navigation(5, 2, "https://e.com/2", "2")
                .navigation(5, 3, "https://e.com/3", "3")
        };
        let current = |w: SnssWriter| {
            parse_session(&w.build()).unwrap().windows[0].tabs[0]
                .url
                .clone()
        };

        // From back: navigations >= 2 removed; the selection falls back to the last left.
        assert_eq!(
            current(tab().selected_navigation(5, 3).ints(5, &[5, 2])),
            "https://e.com/1"
        );
        // From front: two removed, indices shift down, the selection follows its entry.
        assert_eq!(
            current(tab().selected_navigation(5, 3).ints(11, &[5, 2])),
            "https://e.com/3"
        );
        // Middle range [1, 3): entry 3 becomes index 1 and stays selected.
        assert_eq!(
            current(tab().selected_navigation(5, 3).ints(24, &[5, 1, 2])),
            "https://e.com/3"
        );
        // Selection inside the pruned range moves to the entry before it.
        assert_eq!(
            current(tab().selected_navigation(5, 2).ints(24, &[5, 1, 2])),
            "https://e.com/0"
        );
        // Invalid prune counts are damage, like in Chromium.
        let session = parse_session(&tab().ints(24, &[5, 1, 0]).build()).unwrap();
        assert!(session.damaged);
    }

    #[test]
    fn old_window_bounds_record_is_understood() {
        let mut payload = Vec::new();
        for v in [1i32, 5, 6, 700, 500] {
            payload.extend_from_slice(&v.to_le_bytes());
        }
        payload.extend_from_slice(&[1, 0, 0, 0]);
        let data = SnssWriter::new(1)
            .window_type(1, 0)
            .record(10, &payload)
            .tab_window(1, 2)
            .navigation(2, 0, "https://a.example/", "A")
            .build();
        let window = &parse_session(&data).unwrap().windows[0];
        assert_eq!(window.bounds, Some(Rect::new(5, 6, 700, 500)));
        assert_eq!(window.show_state, ShowState::Maximized);
    }

    #[test]
    fn header_problems_are_errors() {
        assert_eq!(parse_session(b""), Err(SnssError::TooShort));
        assert_eq!(parse_session(b"SNSS\x03\x00"), Err(SnssError::TooShort));
        assert_eq!(
            parse_session(b"PK\x03\x04\x03\x00\x00\x00"),
            Err(SnssError::BadSignature)
        );
        for version in [2, 4, 5] {
            assert_eq!(
                parse_session(&SnssWriter::new(version).build()),
                Err(SnssError::Encrypted { version })
            );
        }
        assert_eq!(
            parse_session(&SnssWriter::new(9).build()),
            Err(SnssError::UnsupportedVersion(9))
        );
        assert_eq!(
            parse_session(&SnssWriter::new(-1).build()),
            Err(SnssError::UnsupportedVersion(-1))
        );
    }

    #[test]
    fn truncated_tail_keeps_complete_records() {
        let complete =
            standard_window(SnssWriter::new(3), 1, 10, &[("https://a.example/", "A")], 0)
                .marker()
                .build();
        let mut data = complete.clone();
        // A partially written navigation record at the end.
        let extra = SnssWriter::new(3)
            .navigation(10, 1, "https://b.example/", "B")
            .build();
        data.extend_from_slice(&extra[HEADER_LEN..HEADER_LEN + 9]);
        let session = parse_session(&data).unwrap();
        assert!(session.truncated);
        assert_eq!(urls(&session.windows[0]), ["https://a.example/"]);
        // A lone trailing byte, a zero-length record and a giant size are the same situation.
        for tail in [&[0x07][..], &[0, 0, 9, 9], &[0xff, 0xff, 6, 1, 2, 3]] {
            let mut data = complete.clone();
            data.extend_from_slice(tail);
            let session = parse_session(&data).unwrap();
            assert!(session.truncated, "tail {tail:?}");
            assert_eq!(session.windows.len(), 1);
        }
    }

    #[test]
    fn malformed_payload_stops_replay_and_keeps_earlier_state() {
        let good = standard_window(SnssWriter::new(1), 1, 10, &[("https://a.example/", "A")], 0);
        // SetTabWindow with 7 bytes instead of 8.
        let session = parse_session(
            &good
                .clone()
                .record(0, &[1, 0, 0, 0, 11, 0, 0])
                .tab_window(1, 12)
                .navigation(12, 0, "https://later.example/", "Later")
                .build(),
        )
        .unwrap();
        assert!(session.damaged);
        assert_eq!(urls(&session.windows[0]), ["https://a.example/"]);

        // A navigation whose URL claims 2 GiB of data.
        let giant = PickleWriter::new().int(10).int(1).int(i32::MAX).finish();
        let session = parse_session(&good.clone().record(6, &giant).build()).unwrap();
        assert!(session.damaged);
        assert_eq!(urls(&session.windows[0]), ["https://a.example/"]);

        // Negative string length, broken pickle header, odd UTF-16 data.
        for payload in [
            PickleWriter::new().int(10).int(1).int(-4).finish(),
            vec![0xff, 0xff, 0xff, 0x7f, 0, 0, 0, 0],
            vec![1, 0, 0, 0],
            PickleWriter::new()
                .int(10)
                .int(1)
                .string("https://x.example/")
                .int(3)
                .finish(),
        ] {
            let session = parse_session(&good.clone().record(6, &payload).build()).unwrap();
            assert!(session.damaged, "payload {payload:?}");
            assert_eq!(session.windows.len(), 1);
        }
    }

    #[test]
    fn invalid_utf16_titles_are_replaced_not_rejected() {
        let mut pickle = PickleWriter::new()
            .int(10)
            .int(0)
            .string("https://a.example/");
        // One unpaired surrogate and a control character.
        pickle = pickle.int(2).raw(&[0x00, 0xd8, 0x0a, 0x00]);
        let payload = pickle.string("").int(0).finish();
        let data = SnssWriter::new(1)
            .window_type(1, 0)
            .tab_window(1, 10)
            .record(6, &payload)
            .build();
        let session = parse_session(&data).unwrap();
        assert!(!session.damaged);
        assert_eq!(session.windows[0].tabs[0].title, "\u{fffd}");
    }

    #[test]
    fn limits_are_enforced() {
        let limits = Limits {
            max_records: 5,
            ..Limits::default()
        };
        let data =
            standard_window(SnssWriter::new(1), 1, 10, &[("https://a.example/", "A")], 0).build();
        assert_eq!(
            parse_session_with(&data, &limits),
            Err(SnssError::TooManyRecords)
        );

        let limits = Limits {
            max_file_bytes: 16,
            ..Limits::default()
        };
        assert_eq!(
            parse_session_with(&data, &limits),
            Err(SnssError::TooLarge { bytes: data.len() })
        );

        let limits = Limits {
            max_tabs: 2,
            max_navigations_per_tab: 2,
            ..Limits::default()
        };
        let data = SnssWriter::new(1)
            .window_type(1, 0)
            .tab_window(1, 1)
            .navigation(1, 0, "https://e.com/0", "0")
            .navigation(1, 1, "https://e.com/1", "1")
            .navigation(1, 2, "https://e.com/2", "2")
            .tab_window(1, 2)
            .navigation(2, 0, "https://e.com/b", "b")
            .tab_window(1, 3)
            .navigation(3, 0, "https://e.com/c", "c")
            .build();
        let session = parse_session_with(&data, &limits).unwrap();
        assert!(session.limits_exceeded);
        assert_eq!(session.windows[0].tabs.len(), 2);
    }

    /// Deterministic fuzzing: corrupted and truncated variants of a valid file must never
    /// panic or hang, whatever they parse to.
    #[test]
    fn corrupted_input_never_panics() {
        let valid = standard_window(
            SnssWriter::new(3),
            1,
            10,
            &[
                ("https://a.example/", "A"),
                ("https://b.example/path?q=1", "Bé"),
                ("file:///home/u/doc.html", "Doc"),
            ],
            1,
        )
        .pinned(11, true)
        .user_title(1, "Work")
        .ints(24, &[10, 0, 1])
        .marker()
        .build();
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..3000 {
            let mut data = valid.clone();
            let flips = 1 + next() % 8;
            for _ in 0..flips {
                let i = (next() as usize) % data.len();
                data[i] = next() as u8;
            }
            if next() % 3 == 0 {
                let len = (next() as usize) % data.len();
                data.truncate(len);
            }
            let _ = parse_session(&data);
        }
        // Every prefix of the valid file.
        for len in 0..valid.len() {
            let _ = parse_session(&valid[..len]);
        }
    }

    /// Structured fuzzing: well-framed records with known command ids, small interacting ids
    /// and indices, and random or near-valid payloads drive every replay path. Whatever a
    /// file parses to must keep the [`SessionWindow`] invariants.
    #[test]
    fn random_record_streams_never_panic() {
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut with_windows = 0usize;
        for _ in 0..2000 {
            let version = if next() % 2 == 0 { 1 } else { 3 };
            let mut writer = SnssWriter::new(version);
            for _ in 0..next() % 120 {
                // Ids and indices in -1..=4 so that records refer to each other.
                let mut small = || i32::try_from(next() % 6).unwrap() - 1;
                let (a, b, c) = (small(), small(), small());
                writer = match next() % 20 {
                    0 => writer.tab_window(a, b),
                    1 => writer.tab_index(a, b),
                    2 => writer.ints(5, &[a, b]),
                    3 | 4 => writer.navigation(a, b, "https://a.example/", "title"),
                    5 => writer.selected_navigation(a, b),
                    6 => writer.selected_tab(a, b),
                    7 | 8 => writer.window_type(a, b.max(0) % 2),
                    9 => writer.record(10, &[1; 24]),
                    10 => writer.ints(11, &[a, b]),
                    11 => writer.pinned(a, b > 0),
                    12 => writer.bounds(a, b, c, 800, 600, b),
                    13 => writer.tab_closed(a),
                    14 => writer.window_closed(a),
                    15 => writer.active_window(a),
                    16 => writer.ints(24, &[a, b, c]),
                    17 => writer.user_title(a, "name"),
                    18 => writer.marker(),
                    // Unknown commands and corrupt payloads of known ones.
                    _ => {
                        let id = [0u8, 6, 9, 13, 14, 31, 99][(next() as usize) % 7];
                        let payload: Vec<u8> = (0..next() % 40).map(|_| next() as u8).collect();
                        writer.record(id, &payload)
                    }
                };
            }
            let Ok(session) = parse_session(&writer.build()) else {
                continue;
            };
            with_windows += usize::from(!session.windows.is_empty());
            for window in &session.windows {
                assert!(!window.tabs.is_empty());
                assert!(window.selected_tab < window.tabs.len());
            }
        }
        // The streams really reach the window and tab reconstruction.
        assert!(with_windows > 50, "{with_windows}");
    }

    #[test]
    fn clean_text_strips_controls_and_bounds_length() {
        assert_eq!(clean_text("  a\tb\nc  "), "a b c");
        assert_eq!(
            clean_text(&"x".repeat(2000)).chars().count(),
            MAX_TITLE_CHARS
        );
    }
}
