//! Minimal reader for XML property lists, enough to read the identifier of a macOS application
//! bundle from `Contents/Info.plist`.
//!
//! Only the *string* values of the root dictionary are extracted; nested dictionaries and arrays
//! are skipped. Binary property lists (`bplist00`, used by many Apple applications) are not
//! supported and yield `None`, as do malformed or oversized files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// `Info.plist` files are a few kilobytes; anything much larger is not one.
const MAX_PLIST_BYTES: u64 = 1024 * 1024;

/// What Reframe needs from a bundle's `Info.plist`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BundleInfo {
    /// `CFBundleIdentifier`, e.g. `com.microsoft.VSCode`.
    pub identifier: String,
    /// Main executable (`Contents/MacOS/<CFBundleExecutable>`) when it exists.
    pub executable: Option<PathBuf>,
}

/// Read `bundle/Contents/Info.plist`. `None` when it is missing, binary, malformed or has no
/// usable `CFBundleIdentifier`.
pub(crate) fn read_bundle_info(bundle: &Path) -> Option<BundleInfo> {
    let strings = read_top_level_strings(&bundle.join("Contents").join("Info.plist"))?;
    let identifier = strings
        .get("CFBundleIdentifier")
        .map(|id| id.trim())
        .filter(|id| is_bundle_identifier(id))?
        .to_string();
    let executable = strings
        .get("CFBundleExecutable")
        .map(|name| name.trim())
        // A plain file name only: never follow a path out of the bundle.
        .filter(|name| !name.is_empty() && Path::new(name).file_name() == Some(name.as_ref()))
        .map(|name| bundle.join("Contents").join("MacOS").join(name))
        .filter(|path| path.is_file());
    Some(BundleInfo {
        identifier,
        executable,
    })
}

/// Bundle identifiers are reverse-DNS strings: no whitespace or control characters.
fn is_bundle_identifier(id: &str) -> bool {
    !id.is_empty() && !id.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// String values of the root dictionary of the XML property list at `path`.
pub(crate) fn read_top_level_strings(path: &Path) -> Option<HashMap<String, String>> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_PLIST_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.starts_with(b"bplist") {
        return None;
    }
    let text = String::from_utf8_lossy(&bytes);
    parse_top_level_strings(text.trim_start_matches('\u{feff}'))
}

/// Parse the string values of the root `<dict>` of an XML property list. Returns `None` for
/// documents that are not well-formed enough to trust.
pub(crate) fn parse_top_level_strings(xml: &str) -> Option<HashMap<String, String>> {
    let mut values = HashMap::new();
    // Names of the open elements; `stack.len()` is the depth of the next element.
    let mut stack: Vec<&str> = Vec::new();
    let mut root_depth: Option<usize> = None;
    let mut pending_key: Option<String> = None;
    let mut capturing = false;
    let mut text = String::new();
    let mut rest = xml;

    while let Some(lt) = rest.find('<') {
        if capturing {
            text.push_str(&decode_entities(&rest[..lt]));
        }
        rest = &rest[lt..];
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = &after[after.find("-->")? + 3..];
            continue;
        }
        if let Some(after) = rest.strip_prefix("<![CDATA[") {
            let end = after.find("]]>")?;
            if capturing {
                text.push_str(&after[..end]);
            }
            rest = &after[end + 3..];
            continue;
        }
        if let Some(after) = rest.strip_prefix("<?") {
            rest = &after[after.find("?>")? + 2..];
            continue;
        }
        if let Some(after) = rest.strip_prefix("<!") {
            // <!DOCTYPE ...>; property lists never use an internal subset.
            rest = &after[after.find('>')? + 1..];
            continue;
        }
        let end = rest.find('>')?;
        let tag = &rest[1..end];
        rest = &rest[end + 1..];

        if let Some(name) = tag.strip_prefix('/') {
            if stack.pop()? != name.trim() {
                return None;
            }
            let depth = stack.len();
            match root_depth {
                Some(root) if depth == root + 1 && capturing => {
                    capturing = false;
                    let value = std::mem::take(&mut text);
                    match name.trim() {
                        "key" => pending_key = Some(value),
                        "string" => {
                            if let Some(key) = pending_key.take() {
                                values.insert(key, value);
                            }
                        }
                        _ => {}
                    }
                }
                Some(root) if depth == root => return Some(values),
                _ => {}
            }
            continue;
        }

        let self_closing = tag.ends_with('/');
        let name = tag
            .trim_end_matches('/')
            .split(|c: char| c.is_whitespace())
            .next()
            .filter(|n| !n.is_empty())?;
        let depth = stack.len();
        match root_depth {
            None if name == "dict" && (depth == 0 || (depth == 1 && stack[0] == "plist")) => {
                if self_closing {
                    return Some(values);
                }
                root_depth = Some(depth);
            }
            Some(root) if depth == root + 1 => match (name, self_closing) {
                ("key", false) | ("string", false) => {
                    capturing = true;
                    text.clear();
                }
                ("key", true) => pending_key = Some(String::new()),
                ("string", true) => {
                    if let Some(key) = pending_key.take() {
                        values.insert(key, String::new());
                    }
                }
                // Any other value (number, boolean, array, nested dict) consumes the key.
                _ => pending_key = None,
            },
            _ => {}
        }
        if !self_closing {
            stack.push(name);
        }
    }
    None
}

/// Decode the predefined XML entities and character references; unknown entities are kept.
fn decode_entities(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let semicolon = rest
            .char_indices()
            .take(12)
            .find(|&(_, c)| c == ';')
            .map(|(i, _)| i);
        let decoded = semicolon.and_then(|semi| {
            let entity = &rest[1..semi];
            let c = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => {
                    let code = if let Some(hex) = entity
                        .strip_prefix("#x")
                        .or_else(|| entity.strip_prefix("#X"))
                    {
                        u32::from_str_radix(hex, 16).ok()
                    } else {
                        entity.strip_prefix('#').and_then(|dec| dec.parse().ok())
                    };
                    code.and_then(char::from_u32)
                }
            };
            c.map(|c| (c, semi))
        });
        match decoded {
            Some((c, semi)) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<!-- a comment with <key>CFBundleIdentifier</key> inside -->
	<key>CFBundleDocumentTypes</key>
	<array>
		<dict>
			<key>CFBundleIdentifier</key>
			<string>nested.should.be.ignored</string>
		</dict>
	</array>
	<key>CFBundleExecutable</key>
	<string>Example</string>
	<key>CFBundleIdentifier</key>
	<string>com.example.Tom&amp;Jerry</string>
	<key>LSRequiresNativeExecution</key>
	<true/>
	<key>CFBundleName</key>
	<string><![CDATA[Example & Co]]></string>
	<key>Empty</key>
	<string/>
</dict>
</plist>
"#;

    #[test]
    fn reads_top_level_strings_only() {
        let values = parse_top_level_strings(INFO_PLIST).unwrap();
        assert_eq!(values["CFBundleIdentifier"], "com.example.Tom&Jerry");
        assert_eq!(values["CFBundleExecutable"], "Example");
        assert_eq!(values["CFBundleName"], "Example & Co");
        assert_eq!(values["Empty"], "");
        assert!(!values.contains_key("LSRequiresNativeExecution"));
        assert!(!values.contains_key("CFBundleDocumentTypes"));
    }

    #[test]
    fn non_string_values_consume_their_key() {
        let xml = "<plist><dict><key>A</key><integer>1</integer><string>orphan</string>\
                   <key>B</key><string>b</string></dict></plist>";
        let values = parse_top_level_strings(xml).unwrap();
        assert_eq!(values.len(), 1);
        assert_eq!(values["B"], "b");
    }

    #[test]
    fn rejects_malformed_documents() {
        assert!(parse_top_level_strings("").is_none());
        assert!(parse_top_level_strings("not xml at all").is_none());
        assert!(parse_top_level_strings("<plist><dict><key>A</key>").is_none());
        assert!(parse_top_level_strings("<plist><dict><key>A</string></dict></plist>").is_none());
        assert!(parse_top_level_strings("<plist><dict><!-- unterminated").is_none());
        assert_eq!(
            parse_top_level_strings("<plist><dict/></plist>"),
            Some(HashMap::new())
        );
    }

    #[test]
    fn decodes_entities() {
        assert_eq!(
            decode_entities("a &lt;b&gt; &#65;&#x42; &quot;&apos;"),
            "a <b> AB \"'"
        );
        assert_eq!(
            decode_entities("R&D &unknown; &#xZZ; &"),
            "R&D &unknown; &#xZZ; &"
        );
        assert_eq!(decode_entities("&#1114112;"), "&#1114112;");
    }

    #[test]
    fn reads_bundle_info_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Example.app");
        let contents = bundle.join("Contents");
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();
        std::fs::write(contents.join("Info.plist"), INFO_PLIST).unwrap();
        // The executable does not exist yet.
        let info = read_bundle_info(&bundle).unwrap();
        assert_eq!(info.identifier, "com.example.Tom&Jerry");
        assert_eq!(info.executable, None);
        std::fs::write(contents.join("MacOS").join("Example"), "#!/bin/sh\n").unwrap();
        let info = read_bundle_info(&bundle).unwrap();
        assert_eq!(
            info.executable,
            Some(contents.join("MacOS").join("Example"))
        );

        std::fs::write(contents.join("Info.plist"), b"bplist00\x00\x01binary").unwrap();
        assert!(read_bundle_info(&bundle).is_none());
        std::fs::remove_file(contents.join("Info.plist")).unwrap();
        assert!(read_bundle_info(&bundle).is_none());
    }

    #[test]
    fn executable_names_cannot_escape_the_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Evil.app");
        let contents = bundle.join("Contents");
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();
        std::fs::write(
            contents.join("Info.plist"),
            "<plist><dict><key>CFBundleIdentifier</key><string>com.evil</string>\
             <key>CFBundleExecutable</key><string>../../../Info.plist</string></dict></plist>",
        )
        .unwrap();
        let info = read_bundle_info(&bundle).unwrap();
        assert_eq!(info.identifier, "com.evil");
        assert_eq!(info.executable, None);
    }

    #[test]
    fn rejects_unusable_identifiers() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Odd.app");
        std::fs::create_dir_all(bundle.join("Contents")).unwrap();
        std::fs::write(
            bundle.join("Contents").join("Info.plist"),
            "<plist><dict><key>CFBundleIdentifier</key><string>has space</string></dict></plist>",
        )
        .unwrap();
        assert!(read_bundle_info(&bundle).is_none());
    }
}
