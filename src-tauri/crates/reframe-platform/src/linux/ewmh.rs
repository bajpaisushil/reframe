//! Pure helpers for interpreting ICCCM/EWMH window properties. Nothing here talks to the X
//! server, so all of it is unit tested.

use crate::WindowState;

/// Format an X11 window id the way Reframe stores it: lower-case hex with a `0x` prefix
/// (`0x1a00003`), the same notation `xprop`, `xwininfo` and `wmctrl` use.
pub(crate) fn format_window_id(window: u32) -> String {
    format!("{window:#x}")
}

/// Parse a window id produced by [`format_window_id`]. Also accepts upper-case hex, plain
/// decimal (`27262979`) and surrounding whitespace. Returns `None` for anything that is not
/// a valid, non-zero 32-bit X resource id.
pub(crate) fn parse_window_id(id: &str) -> Option<u32> {
    let id = id.trim();
    let value = if let Some(hex) = id.strip_prefix("0x").or_else(|| id.strip_prefix("0X")) {
        if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(hex, 16).ok()?
    } else if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
        id.parse::<u32>().ok()?
    } else {
        return None;
    };
    // X resource ids use at most 29 bits; 0 is `None`.
    (value != 0 && value <= 0x1FFF_FFFF).then_some(value)
}

/// `_NET_WM_WINDOW_TYPE` values Reframe distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowType {
    Desktop,
    Dock,
    Toolbar,
    Menu,
    Utility,
    Splash,
    Dialog,
    DropdownMenu,
    PopupMenu,
    Tooltip,
    Notification,
    Combo,
    Dnd,
    Normal,
}

impl WindowType {
    /// Whether windows of this type are application windows Reframe lists.
    pub fn is_application_window(self) -> bool {
        matches!(self, WindowType::Normal | WindowType::Dialog)
    }
}

/// Decide whether a window with the given `_NET_WM_WINDOW_TYPE` list is an application
/// window. Per EWMH the list is in order of preference and the first type the reader
/// understands applies; unknown (vendor) types are skipped. No known type means "normal".
pub(crate) fn is_application_type(types: &[Option<WindowType>]) -> bool {
    types
        .iter()
        .flatten()
        .next()
        .is_none_or(|t| t.is_application_window())
}

/// The `_NET_WM_STATE` flags Reframe cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct NetState {
    pub maximized_vert: bool,
    pub maximized_horz: bool,
    pub hidden: bool,
    pub fullscreen: bool,
    pub skip_taskbar: bool,
}

impl NetState {
    pub fn is_maximized(&self) -> bool {
        self.maximized_vert && self.maximized_horz
    }
}

/// ICCCM `WM_STATE` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IcccmState {
    Withdrawn,
    Normal,
    Iconic,
}

impl IcccmState {
    /// From the first CARDINAL of a `WM_STATE` property (1 = Normal, 3 = Iconic, 0 =
    /// Withdrawn; the obsolete 2 "Zoomed" is treated as Normal).
    pub fn from_value(value: u32) -> Option<Self> {
        match value {
            0 => Some(IcccmState::Withdrawn),
            1 | 2 => Some(IcccmState::Normal),
            3 => Some(IcccmState::Iconic),
            _ => None,
        }
    }
}

/// Whether a window is minimized. `hidden_supported` means the window manager maintains
/// `_NET_WM_STATE_HIDDEN` (lists it in `_NET_SUPPORTED`): such window managers also put windows
/// on other desktops (and shaded windows) into ICCCM IconicState, so only the EWMH flag counts,
/// as EWMH asks of pagers. Otherwise IconicState means minimized.
pub(crate) fn is_minimized(
    net: &NetState,
    icccm: Option<IcccmState>,
    hidden_supported: bool,
) -> bool {
    net.hidden || (!hidden_supported && icccm == Some(IcccmState::Iconic))
}

/// Reframe's window state from `_NET_WM_STATE` and `WM_STATE` (see [`is_minimized`] for
/// `hidden_supported`). Minimized wins over fullscreen, which wins over maximized (a minimized
/// maximized window is minimized). Maximized means maximized in both directions;
/// half-maximized windows are Normal.
pub(crate) fn window_state(
    net: &NetState,
    icccm: Option<IcccmState>,
    hidden_supported: bool,
) -> WindowState {
    if is_minimized(net, icccm, hidden_supported) {
        WindowState::Minimized
    } else if net.fullscreen {
        WindowState::Fullscreen
    } else if net.is_maximized() {
        WindowState::Maximized
    } else {
        WindowState::Normal
    }
}

/// Decode a text property. Trailing NUL bytes are removed.
///
/// - `UTF8_STRING`, and anything without escape sequences that is valid UTF-8, is decoded as
///   UTF-8.
/// - `STRING` is decoded as ISO 8859-1.
/// - `COMPOUND_TEXT` is decoded by following its ISO 2022 character set switches. ASCII,
///   ISO 8859-1 and UTF-8 segments (`ESC % G` … `ESC % @`, which Xlib writes for text outside
///   Latin-1, such as an em dash in an `xterm -T` title) are decoded exactly. Characters in any
///   other character set (other ISO 8859 parts, CJK sets, extended segments) become U+FFFD
///   instead of being misread as Latin-1.
pub(crate) fn decode_text(bytes: &[u8], is_utf8: bool) -> String {
    let end = bytes.iter().rposition(|&b| b != 0).map_or(0, |p| p + 1);
    let bytes = &bytes[..end];
    if is_utf8 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    if !bytes.contains(&ESC)
        && let Ok(text) = std::str::from_utf8(bytes)
    {
        return text.to_string();
    }
    decode_compound_text(bytes)
}

const ESC: u8 = 0x1B;
/// Control Sequence Introducer; in compound text it only carries text direction.
const CSI: u8 = 0x9B;

/// A character set selected into the left (GL) or right (GR) half by compound text.
#[derive(Clone, Copy)]
enum CtCharset {
    /// ASCII (or JIS X0201 Roman, which differs from it in two symbols).
    Ascii,
    /// The right half of ISO 8859-1.
    Latin1,
    /// Any other one-byte-per-character set.
    OtherSingle,
    /// A two-bytes-per-character set (JIS X0208, GB 2312, KS C 5601).
    OtherDouble,
}

/// `STRING` / `COMPOUND_TEXT` decoding, starting from the compound-text defaults (ASCII in
/// GL, ISO 8859-1 in GR), which also makes plain `STRING` decode as Latin-1.
fn decode_compound_text(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let (mut gl, mut gr) = (CtCharset::Ascii, CtCharset::Latin1);
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            ESC => {
                // ESC, intermediate bytes (0x20..=0x2F), final byte.
                let start = i + 1;
                let mut end = start;
                while end < bytes.len() && (0x20..=0x2F).contains(&bytes[end]) {
                    end += 1;
                }
                let Some(&last) = bytes.get(end) else { break };
                i = end + 1;
                match (&bytes[start..end], last) {
                    (b"%", b'G') => {
                        // UTF-8 up to ESC % @, or to the end.
                        let rest = &bytes[i..];
                        let len = rest
                            .windows(3)
                            .position(|w| w == b"\x1b%@")
                            .unwrap_or(rest.len());
                        out.push_str(&String::from_utf8_lossy(&rest[..len]));
                        i += (len + 3).min(rest.len());
                    }
                    (b"%/", b'0'..=b'4') => {
                        // Extended segment: two length bytes, then the encoding name, STX and
                        // the text, all counted by that length.
                        let (Some(&m), Some(&l)) = (bytes.get(i), bytes.get(i + 1)) else {
                            break;
                        };
                        let len = (usize::from(m & 0x7F) << 7) | usize::from(l & 0x7F);
                        i = (i + 2 + len).min(bytes.len());
                        out.push(char::REPLACEMENT_CHARACTER);
                    }
                    (b"(", set) => gl = single_byte_set(set),
                    (b")", set) => gr = single_byte_set(set),
                    (b"-", set) => {
                        gr = if set == b'A' {
                            CtCharset::Latin1
                        } else {
                            CtCharset::OtherSingle
                        };
                    }
                    (b"$" | b"$(", _) => gl = CtCharset::OtherDouble,
                    (b"$)", _) => gr = CtCharset::OtherDouble,
                    _ => {}
                }
            }
            CSI => {
                i += 1;
                while i < bytes.len() && !(0x40..=0x7E).contains(&bytes[i]) {
                    i += 1;
                }
                i += 1;
            }
            0x21..=0x7E => i += push_ct_char(&mut out, gl, bytes, i),
            0xA0..=0xFF => i += push_ct_char(&mut out, gr, bytes, i),
            // DEL and C1 control characters are not text.
            0x7F..=0x9F => i += 1,
            b => {
                out.push(char::from(b));
                i += 1;
            }
        }
    }
    out
}

/// The 94-character set a compound-text `ESC (` / `ESC )` sequence selects.
fn single_byte_set(final_byte: u8) -> CtCharset {
    match final_byte {
        b'B' | b'J' => CtCharset::Ascii,
        _ => CtCharset::OtherSingle,
    }
}

/// Decode the character starting at `bytes[i]` in `set`; returns the number of bytes used.
fn push_ct_char(out: &mut String, set: CtCharset, bytes: &[u8], i: usize) -> usize {
    match set {
        CtCharset::Ascii => out.push(char::from(bytes[i] & 0x7F)),
        CtCharset::Latin1 => out.push(char::from(bytes[i])),
        CtCharset::OtherSingle => out.push(char::REPLACEMENT_CHARACTER),
        CtCharset::OtherDouble => {
            out.push(char::REPLACEMENT_CHARACTER);
            let pair = bytes
                .get(i + 1)
                .is_some_and(|&next| (0x21..=0x7E).contains(&(next & 0x7F)));
            return if pair { 2 } else { 1 };
        }
    }
    1
}

/// Split a `WM_CLASS` value (`instance\0class\0`) into `(instance, class)`; empty parts are
/// `None`.
pub(crate) fn parse_wm_class(bytes: &[u8]) -> (Option<String>, Option<String>) {
    let mut parts = bytes
        .split(|&b| b == 0)
        .map(|p| decode_text(p, false).trim().to_string());
    let instance = parts.next().filter(|s| !s.is_empty());
    let class = parts.next().filter(|s| !s.is_empty());
    (instance, class)
}

/// `Xft.dpi` from the `RESOURCE_MANAGER` resource database string, as a scale factor relative
/// to 96 DPI. Returns `None` when the resource is absent or not a sane number.
pub(crate) fn scale_from_resources(resources: &str) -> Option<f64> {
    let mut dpi = None;
    for line in resources.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        // Exact resource name, or the loose bindings commonly used for it.
        let matches = key == "Xft.dpi" || key == "Xft*dpi" || key == "*dpi" || key == "*.dpi";
        if !matches {
            continue;
        }
        if let Ok(v) = value.trim().parse::<f64>()
            && v.is_finite()
            && (24.0..=960.0).contains(&v)
        {
            // The exact name wins over loose bindings.
            if key == "Xft.dpi" || dpi.is_none() {
                dpi = Some(v);
            }
        }
    }
    dpi.map(|d| (d / 96.0 * 100.0).round() / 100.0)
}

/// Monitor model name from an EDID blob (the `0xFC` display product name descriptor).
pub(crate) fn edid_monitor_name(edid: &[u8]) -> Option<String> {
    const HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
    if edid.len() < 128 || edid[..8] != HEADER {
        return None;
    }
    for offset in [54usize, 72, 90, 108] {
        let d = &edid[offset..offset + 18];
        if d[0] == 0 && d[1] == 0 && d[2] == 0 && d[3] == 0xFC {
            let text: String = d[5..]
                .iter()
                .take_while(|&&b| b != 0x0A && b != 0)
                .map(|&b| char::from(b))
                .collect();
            let text = text.trim();
            if !text.is_empty() && text.chars().all(|c| !c.is_control()) {
                return Some(text.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_ids_round_trip() {
        assert_eq!(format_window_id(0x1a00003), "0x1a00003");
        assert_eq!(parse_window_id("0x1a00003"), Some(0x1a00003));
        assert_eq!(parse_window_id(&format_window_id(0x40000c)), Some(0x40000c));
        assert_eq!(parse_window_id(" 0X1A00003 "), Some(0x1a00003));
        assert_eq!(parse_window_id("27262979"), Some(27_262_979));
    }

    #[test]
    fn invalid_window_ids_are_rejected() {
        for bad in [
            "",
            "0x",
            "0",
            "0x0",
            "abc",
            "0xZZ",
            "mock-1",
            "-5",
            "0x1a00003 extra",
            "99999999999",
            "0xFFFFFFFF",
            "HWND(1234)",
        ] {
            assert_eq!(parse_window_id(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn window_type_filtering() {
        use WindowType::*;
        assert!(is_application_type(&[]));
        assert!(is_application_type(&[Some(Normal)]));
        assert!(is_application_type(&[Some(Dialog)]));
        for excluded in [
            Desktop,
            Dock,
            Toolbar,
            Menu,
            Utility,
            Splash,
            DropdownMenu,
            PopupMenu,
            Tooltip,
            Notification,
            Combo,
            Dnd,
        ] {
            assert!(!is_application_type(&[Some(excluded)]), "{excluded:?}");
        }
        // Unknown vendor types are skipped, the first known type decides.
        assert!(is_application_type(&[None, Some(Normal)]));
        assert!(!is_application_type(&[None, Some(Dock), Some(Normal)]));
        assert!(is_application_type(&[Some(Normal), Some(Utility)]));
        assert!(is_application_type(&[None]));
    }

    #[test]
    fn net_wm_state_to_window_state() {
        for ewmh in [true, false] {
            let normal = NetState::default();
            assert_eq!(window_state(&normal, None, ewmh), WindowState::Normal);
            assert_eq!(
                window_state(&normal, Some(IcccmState::Normal), ewmh),
                WindowState::Normal
            );
            let max = NetState {
                maximized_vert: true,
                maximized_horz: true,
                ..Default::default()
            };
            assert_eq!(window_state(&max, None, ewmh), WindowState::Maximized);
            let half = NetState {
                maximized_vert: true,
                ..Default::default()
            };
            assert_eq!(window_state(&half, None, ewmh), WindowState::Normal);
            let full = NetState {
                fullscreen: true,
                ..max
            };
            assert_eq!(window_state(&full, None, ewmh), WindowState::Fullscreen);
            let hidden = NetState {
                hidden: true,
                ..full
            };
            assert_eq!(window_state(&hidden, None, ewmh), WindowState::Minimized);
            assert_eq!(
                window_state(&hidden, Some(IcccmState::Iconic), ewmh),
                WindowState::Minimized
            );
        }
    }

    #[test]
    fn iconic_state_means_minimized_only_without_net_wm_state_hidden() {
        let max = NetState {
            maximized_vert: true,
            maximized_horz: true,
            ..Default::default()
        };
        // ICCCM-only window manager: IconicState is the only minimized signal.
        assert_eq!(
            window_state(&max, Some(IcccmState::Iconic), false),
            WindowState::Minimized
        );
        assert!(is_minimized(&max, Some(IcccmState::Iconic), false));
        // EWMH window managers (openbox, mutter) also iconify windows on other desktops and
        // shaded windows; without _NET_WM_STATE_HIDDEN those are not minimized.
        assert_eq!(
            window_state(&max, Some(IcccmState::Iconic), true),
            WindowState::Maximized
        );
        assert_eq!(
            window_state(&NetState::default(), Some(IcccmState::Iconic), true),
            WindowState::Normal
        );
        assert!(!is_minimized(
            &NetState::default(),
            Some(IcccmState::Iconic),
            true
        ));
    }

    #[test]
    fn icccm_state_values() {
        assert_eq!(IcccmState::from_value(0), Some(IcccmState::Withdrawn));
        assert_eq!(IcccmState::from_value(1), Some(IcccmState::Normal));
        assert_eq!(IcccmState::from_value(3), Some(IcccmState::Iconic));
        assert_eq!(IcccmState::from_value(7), None);
    }

    #[test]
    fn text_decoding() {
        assert_eq!(
            decode_text("Grüße — UTF-8".as_bytes(), true),
            "Grüße — UTF-8"
        );
        // Latin-1 "Grüße"
        assert_eq!(decode_text(&[0x47, 0x72, 0xFC, 0xDF, 0x65], false), "Grüße");
        assert_eq!(decode_text(b"title\0\0", false), "title");
        assert_eq!(decode_text(b"", false), "");
        // COMPOUND_TEXT with an escape sequence selecting Latin-1 for the right half.
        assert_eq!(decode_text(b"\x1b-Aabc\xe9", false), "abcé");
        // Invalid UTF-8 declared as UTF8_STRING is decoded lossily, never panics.
        assert!(decode_text(&[0xFF, b'a'], true).ends_with('a'));
    }

    #[test]
    fn compound_text_utf8_segments_are_decoded() {
        // What Xlib writes for `xterm -T "make — build é"`: the em dash is outside Latin-1.
        assert_eq!(
            decode_text(b"make \x1b%G\xe2\x80\x94\x1b%@ build \xe9", false),
            "make — build é"
        );
        // A segment that runs to the end, and Latin-1 again after a closed segment.
        assert_eq!(decode_text(b"a \x1b%G\xc3\xa9", false), "a é");
        assert_eq!(decode_text(b"\x1b%G\xc3\xa9\x1b%@\xe9", false), "éé");
    }

    #[test]
    fn compound_text_other_charsets_become_replacement_characters() {
        // ISO 8859-5 (Cyrillic) in the right half: one character per byte.
        assert_eq!(
            decode_text(b"\x1b-L\xbf\xe0 ok", false),
            "\u{FFFD}\u{FFFD} ok"
        );
        // JIS X0208 in the right half, then back to Latin-1: two bytes per character.
        assert_eq!(
            decode_text(b"\x1b$)B\xc6\xfc\xcb\xdc ok\x1b-A\xe9", false),
            "\u{FFFD}\u{FFFD} oké"
        );
        // GB 2312 in the left half, then ASCII again.
        assert_eq!(
            decode_text(b"\x1b$(A\x30\x21\x1b(B ok", false),
            "\u{FFFD} ok"
        );
        // An extended segment (KOI8-R) is skipped as a whole, using its length bytes.
        assert_eq!(
            decode_text(b"a\x1b%/1\x80\x89koi8-r\x02\xf0\xd2b", false),
            "a\u{FFFD}b"
        );
        // Direction markers carry no text.
        assert_eq!(decode_text(b"\x9b1]abc\x9b]", false), "abc");
        // Truncated sequences never panic.
        for bytes in [&b"ab\x1b"[..], b"ab\x1b%", b"ab\x1b%/1\x80", b"\x1b$)B\xc6"] {
            let _ = decode_text(bytes, false);
        }
    }

    #[test]
    fn wm_class_parsing() {
        assert_eq!(
            parse_wm_class(b"xterm\0XTerm\0"),
            (Some("xterm".into()), Some("XTerm".into()))
        );
        assert_eq!(
            parse_wm_class(b"navigator\0firefox"),
            (Some("navigator".into()), Some("firefox".into()))
        );
        assert_eq!(parse_wm_class(b"\0Code\0"), (None, Some("Code".into())));
        assert_eq!(parse_wm_class(b""), (None, None));
    }

    #[test]
    fn xft_dpi_scale() {
        assert_eq!(
            scale_from_resources("Xft.antialias:\t1\nXft.dpi:\t192\nXft.hinting:\t1\n"),
            Some(2.0)
        );
        assert_eq!(scale_from_resources("Xft.dpi: 144"), Some(1.5));
        assert_eq!(scale_from_resources("Xft.dpi:\t120\n"), Some(1.25));
        assert_eq!(scale_from_resources("*dpi: 96\nXft.dpi: 192"), Some(2.0));
        assert_eq!(scale_from_resources("*dpi: 120"), Some(1.25));
        assert_eq!(scale_from_resources("Xft.dpi: lots"), None);
        assert_eq!(scale_from_resources("Xft.dpi: 0"), None);
        assert_eq!(scale_from_resources("Xcursor.size: 24"), None);
        assert_eq!(scale_from_resources(""), None);
    }

    #[test]
    fn edid_names() {
        let mut edid = vec![0u8; 128];
        edid[..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        // Descriptor 2 (offset 72): monitor name "DELL U2720Q\n"
        let name = b"DELL U2720Q\n ";
        edid[72..77].copy_from_slice(&[0, 0, 0, 0xFC, 0]);
        edid[77..77 + name.len()].copy_from_slice(name);
        assert_eq!(edid_monitor_name(&edid).as_deref(), Some("DELL U2720Q"));
        // Wrong header / too short.
        assert_eq!(edid_monitor_name(&edid[..100]), None);
        edid[0] = 1;
        assert_eq!(edid_monitor_name(&edid), None);
        assert_eq!(edid_monitor_name(&[0u8; 128]), None);
    }
}
