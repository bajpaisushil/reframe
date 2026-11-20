//! Helpers for reading the friendly product name out of an executable's version resource
//! (`GetFileVersionInfoW` + `VerQueryValueW`).

use std::ops::Range;

/// String values tried in order: Explorer and Task Manager show `FileDescription`;
/// `ProductName` is the fallback for files that leave it empty.
pub const NAME_KEYS: [&str; 2] = ["FileDescription", "ProductName"];

/// Translations tried when the file's `\VarFileInfo\Translation` table is missing or its entries
/// have no strings: US English with the Unicode, Windows-1252 and neutral code pages, then
/// language-neutral Unicode.
const FALLBACK_TRANSLATIONS: [(u16, u16); 4] = [
    (0x0409, 0x04b0),
    (0x0409, 0x04e4),
    (0x0409, 0x0000),
    (0x0000, 0x04b0),
];

/// Parse a `\VarFileInfo\Translation` value: little-endian `(language, code page)` pairs.
pub fn parse_translations(bytes: &[u8]) -> Vec<(u16, u16)> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&[l0, l1, c0, c1]| (u16::from_le_bytes([l0, l1]), u16::from_le_bytes([c0, c1])))
        .collect()
}

/// Translations to query, best first: the file's own table, then the common fallbacks.
pub fn translation_candidates(table: &[(u16, u16)]) -> Vec<(u16, u16)> {
    let mut candidates = Vec::with_capacity(table.len() + FALLBACK_TRANSLATIONS.len());
    for translation in table.iter().copied().chain(FALLBACK_TRANSLATIONS) {
        if !candidates.contains(&translation) {
            candidates.push(translation);
        }
    }
    candidates
}

/// Sub-block path of a string value, e.g. `\StringFileInfo\040904b0\FileDescription`.
pub fn string_value_path((language, code_page): (u16, u16), key: &str) -> String {
    format!("\\StringFileInfo\\{language:04x}{code_page:04x}\\{key}")
}

/// Byte range of a `VerQueryValueW` result inside the version block, or `None` if the pointer
/// and length do not lie entirely inside the block.
pub fn value_range(
    block_start: usize,
    block_len: usize,
    ptr: usize,
    len: usize,
) -> Option<Range<usize>> {
    let offset = ptr.checked_sub(block_start)?;
    let end = offset.checked_add(len)?;
    (end <= block_len).then_some(offset..end)
}

/// UTF-16 code units from little-endian bytes (a trailing odd byte is ignored).
pub fn utf16_units(bytes: &[u8]) -> Vec<u16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .collect()
}

/// Decode a version string value: up to the first NUL, trimmed; `None` when empty.
pub fn decode_string_value(units: &[u16]) -> Option<String> {
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    let text = String::from_utf16_lossy(&units[..end]);
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translation_table_parsing() {
        let bytes = [0x09, 0x04, 0xb0, 0x04, 0x07, 0x04, 0xe4, 0x04, 0xff];
        assert_eq!(
            parse_translations(&bytes),
            vec![(0x0409, 0x04b0), (0x0407, 0x04e4)]
        );
        assert!(parse_translations(&[]).is_empty());
    }

    #[test]
    fn candidates_keep_file_order_then_fallbacks_without_duplicates() {
        let candidates = translation_candidates(&[(0x0407, 0x04e4), (0x0409, 0x04b0)]);
        assert_eq!(
            candidates,
            vec![
                (0x0407, 0x04e4),
                (0x0409, 0x04b0),
                (0x0409, 0x04e4),
                (0x0409, 0x0000),
                (0x0000, 0x04b0),
            ]
        );
        assert_eq!(
            translation_candidates(&[]).len(),
            FALLBACK_TRANSLATIONS.len()
        );
    }

    #[test]
    fn string_value_paths() {
        assert_eq!(
            string_value_path((0x0409, 0x04b0), "FileDescription"),
            "\\StringFileInfo\\040904b0\\FileDescription"
        );
        assert_eq!(
            string_value_path((0x0000, 0x0000), "ProductName"),
            "\\StringFileInfo\\00000000\\ProductName"
        );
    }

    #[test]
    fn value_ranges_are_bounds_checked() {
        assert_eq!(value_range(1000, 100, 1010, 20), Some(10..30));
        assert_eq!(value_range(1000, 100, 1000, 100), Some(0..100));
        assert_eq!(value_range(1000, 100, 1090, 20), None);
        assert_eq!(value_range(1000, 100, 990, 4), None);
        assert_eq!(value_range(1000, 100, usize::MAX, 4), None);
        assert_eq!(value_range(0, usize::MAX, 10, usize::MAX), None);
    }

    #[test]
    fn string_values_decode() {
        let mut bytes = Vec::new();
        for unit in "  Visual Studio Code \0junk".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes.push(0x41); // odd trailing byte
        assert_eq!(
            decode_string_value(&utf16_units(&bytes)).as_deref(),
            Some("Visual Studio Code")
        );
        assert_eq!(decode_string_value(&[0x20, 0x20, 0]), None);
        assert_eq!(decode_string_value(&[]), None);
        // Unpaired surrogates are replaced, not fatal.
        assert_eq!(
            decode_string_value(&[0xd800, 0x41]).as_deref(),
            Some("\u{fffd}A")
        );
    }
}
