//! Builders for synthetic Chromium session files (tests only).
//!
//! The writers produce the same byte layout as Chromium: `base::Pickle` payloads with a
//! `uint32` length header and 4-byte aligned fields, and SNSS records made of a `uint16` size
//! (including the command id), the id and the payload.

use std::path::Path;

/// Writes a `base::Pickle`.
#[derive(Debug, Clone, Default)]
pub struct PickleWriter {
    payload: Vec<u8>,
}

impl PickleWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn int(mut self, value: i32) -> Self {
        self.payload.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub fn int64(mut self, value: i64) -> Self {
        self.payload.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub fn bool(self, value: bool) -> Self {
        self.int(i32::from(value))
    }

    /// Raw bytes, padded to 4 bytes.
    pub fn raw(mut self, bytes: &[u8]) -> Self {
        self.payload.extend_from_slice(bytes);
        while !self.payload.len().is_multiple_of(4) {
            self.payload.push(0);
        }
        self
    }

    pub fn string(self, value: &str) -> Self {
        let len = i32::try_from(value.len()).expect("test string fits in i32");
        self.int(len).raw(value.as_bytes())
    }

    pub fn string16(self, value: &str) -> Self {
        let units: Vec<u16> = value.encode_utf16().collect();
        let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
        let len = i32::try_from(units.len()).expect("test string fits in i32");
        self.int(len).raw(&bytes)
    }

    pub fn finish(self) -> Vec<u8> {
        let len = u32::try_from(self.payload.len()).expect("test pickle fits in u32");
        let mut out = len.to_le_bytes().to_vec();
        out.extend_from_slice(&self.payload);
        out
    }
}

/// Writes an SNSS session file record by record.
#[derive(Debug, Clone)]
pub struct SnssWriter {
    data: Vec<u8>,
}

impl SnssWriter {
    pub fn new(version: i32) -> Self {
        let mut data = b"SNSS".to_vec();
        data.extend_from_slice(&version.to_le_bytes());
        Self { data }
    }

    pub fn record(mut self, id: u8, payload: &[u8]) -> Self {
        let size = u16::try_from(payload.len() + 1).expect("test record fits in u16");
        self.data.extend_from_slice(&size.to_le_bytes());
        self.data.push(id);
        self.data.extend_from_slice(payload);
        self
    }

    /// A struct payload made of `int32` fields.
    pub fn ints(self, id: u8, values: &[i32]) -> Self {
        let payload: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.record(id, &payload)
    }

    pub fn tab_window(self, window: i32, tab: i32) -> Self {
        self.ints(0, &[window, tab])
    }

    pub fn tab_index(self, tab: i32, index: i32) -> Self {
        self.ints(2, &[tab, index])
    }

    /// `UpdateTabNavigation` with every field a current browser writes.
    pub fn navigation(self, tab: i32, index: i32, url: &str, title: &str) -> Self {
        let payload = PickleWriter::new()
            .int(tab)
            .int(index)
            .string(url)
            .string16(title)
            .string("page-state")
            .int(0) // transition type
            .int(0) // type mask
            .string("") // referrer
            .int(2) // obsolete referrer policy
            .string("") // original request URL
            .bool(false) // user agent override
            .int64(13_300_000_000_000_000) // timestamp
            .string16("") // removed search terms
            .int(200) // HTTP status
            .int(0) // referrer policy
            .int(0) // extended info entries
            .int64(-1)
            .int64(-1)
            .int64(-1)
            .int(0)
            .finish();
        self.record(6, &payload)
    }

    pub fn selected_navigation(self, tab: i32, index: i32) -> Self {
        self.ints(7, &[tab, index])
    }

    pub fn selected_tab(self, window: i32, index: i32) -> Self {
        self.ints(8, &[window, index])
    }

    pub fn window_type(self, window: i32, window_type: i32) -> Self {
        self.ints(9, &[window, window_type])
    }

    pub fn bounds(self, window: i32, x: i32, y: i32, w: i32, h: i32, show_state: i32) -> Self {
        self.ints(14, &[window, x, y, w, h, show_state])
    }

    pub fn pinned(self, tab: i32, pinned: bool) -> Self {
        let mut payload = tab.to_le_bytes().to_vec();
        payload.extend_from_slice(&[u8::from(pinned), 0xaa, 0xbb, 0xcc]);
        self.record(12, &payload)
    }

    pub fn tab_closed(self, tab: i32) -> Self {
        self.record(16, &closed_payload(tab))
    }

    pub fn window_closed(self, window: i32) -> Self {
        self.record(17, &closed_payload(window))
    }

    pub fn active_window(self, window: i32) -> Self {
        self.ints(20, &[window])
    }

    pub fn user_title(self, window: i32, title: &str) -> Self {
        let payload = PickleWriter::new().int(window).string(title).finish();
        self.record(31, &payload)
    }

    pub fn marker(self) -> Self {
        self.record(255, &[])
    }

    pub fn build(self) -> Vec<u8> {
        self.data
    }
}

fn closed_payload(id: i32) -> Vec<u8> {
    let mut payload = id.to_le_bytes().to_vec();
    payload.extend_from_slice(&[0; 4]);
    payload.extend_from_slice(&13_300_000_000_000_000i64.to_le_bytes());
    payload
}

/// A normal window `window` at (100, 100, 1200, 800) whose tabs get ids `first_tab..`, with
/// `selected` as the selected tab.
pub fn standard_window(
    writer: SnssWriter,
    window: i32,
    first_tab: i32,
    tabs: &[(&str, &str)],
    selected: i32,
) -> SnssWriter {
    window_at(
        writer,
        window,
        first_tab,
        tabs,
        selected,
        (100, 100, 1200, 800),
    )
}

/// Like [`standard_window`] with explicit bounds `(x, y, width, height)`.
pub fn window_at(
    writer: SnssWriter,
    window: i32,
    first_tab: i32,
    tabs: &[(&str, &str)],
    selected: i32,
    (x, y, w, h): (i32, i32, i32, i32),
) -> SnssWriter {
    let mut writer = writer.window_type(window, 0).bounds(window, x, y, w, h, 1);
    for (i, (url, title)) in tabs.iter().enumerate() {
        let i = i32::try_from(i).expect("few tabs");
        let tab = first_tab + i;
        writer = writer
            .tab_window(window, tab)
            .tab_index(tab, i)
            .navigation(tab, 0, url, title)
            .selected_navigation(tab, 0);
    }
    writer.selected_tab(window, selected)
}

/// Write `data` as the session file `Session_<timestamp>` of `profile_dir`.
pub fn write_session_file(profile_dir: &Path, timestamp: u64, data: &[u8]) {
    let sessions = profile_dir.join("Sessions");
    std::fs::create_dir_all(&sessions).expect("create Sessions directory");
    std::fs::write(sessions.join(format!("Session_{timestamp}")), data)
        .expect("write session file");
}
