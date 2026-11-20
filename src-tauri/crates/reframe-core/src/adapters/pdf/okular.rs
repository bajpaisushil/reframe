//! Okular's per-document metadata ("docdata"): `~/.local/share/okular/docdata/<size>.<name>.xml`.
//!
//! Each file records the document (`<documentInfo url="/home/me/paper.pdf">`) and its viewing
//! history; the current position is `<current viewport="N;C2:…"/>` with a 0-based page `N`
//! (an older/alternative form `<current page="N"/>` is read as a 1-based page). Okular writes
//! the file when a document is closed (and on some other occasions), so the page of a document
//! that is still open is the one Okular last saved. Only these two values are read.

use std::fs;
use std::path::{Path, PathBuf};

/// Docdata files are small; larger files are ignored.
const MAX_DOCDATA_BYTES: u64 = 1 << 20;
/// Docdata entries examined per folder when resolving a bare file name.
const MAX_SCANNED: usize = 20_000;

/// The values Reframe reads from a docdata file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct DocData {
    /// The document: a local path, or a URL for remote documents.
    pub url: Option<String>,
    /// 1-based page.
    pub page: Option<u32>,
}

/// Docdata folders for the current user: XDG data dir, the local data dir (Windows builds)
/// and the Flatpak sandbox.
pub(super) fn docdata_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for base in [dirs::data_dir(), dirs::data_local_dir()]
        .into_iter()
        .flatten()
    {
        let dir = base.join("okular").join("docdata");
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".var/app/org.kde.okular/data/okular/docdata"));
    }
    dirs
}

/// Parse a docdata file. Malformed input yields empty values, never an error.
pub(super) fn parse(xml: &str) -> DocData {
    let url = start_tag(xml, "documentInfo").and_then(|tag| attribute(tag, "url"));
    let history_start = xml.find("<history").unwrap_or(0);
    let page = start_tag(&xml[history_start..], "current").and_then(|tag| {
        if let Some(viewport) = attribute(tag, "viewport") {
            let digits: String = viewport
                .trim()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            digits.parse::<u32>().ok().and_then(|p| p.checked_add(1))
        } else {
            attribute(tag, "page")?
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|p| *p >= 1)
        }
    });
    DocData { url, page }
}

/// The text of the first `<name …>` start tag (between `<` and `>`).
fn start_tag<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("<{name}");
    let mut from = 0;
    while let Some(offset) = xml[from..].find(&needle) {
        let start = from + offset;
        let after = start + needle.len();
        let next = xml[after..].chars().next();
        if matches!(next, Some(c) if c.is_whitespace() || c == '/' || c == '>') {
            let end = xml[after..].find('>').map(|i| after + i)?;
            return Some(&xml[start..end]);
        }
        from = after;
    }
    None
}

/// Value of `name="…"` or `name='…'` inside a start tag, with XML entities decoded.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let mut from = 0;
    while let Some(offset) = tag[from..].find(name) {
        let start = from + offset;
        let preceded_by_space = tag[..start].chars().last().is_some_and(char::is_whitespace);
        let rest = tag[start + name.len()..].trim_start();
        if preceded_by_space
            && let Some(rest) = rest.strip_prefix('=')
            && let Some(quote) = rest
                .trim_start()
                .chars()
                .next()
                .filter(|q| *q == '"' || *q == '\'')
        {
            let value = &rest.trim_start()[1..];
            // An unterminated value means a malformed tag: no value.
            return value.find(quote).map(|end| unescape(&value[..end]));
        }
        from = start + name.len();
    }
    None
}

/// Decode `&amp;` `&lt;` `&gt;` `&quot;` `&apos;` and numeric character references.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find('&') {
        out.push_str(&rest[..index]);
        let after = &rest[index..];
        let decoded = after.find(';').and_then(|end| {
            let entity = &after[1..end];
            let ch = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => entity
                    .strip_prefix("#x")
                    .or_else(|| entity.strip_prefix("#X"))
                    .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                    .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            };
            ch.map(|c| (c, end))
        });
        match decoded {
            Some((ch, end)) => {
                out.push(ch);
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The local file a docdata `url` refers to (`/home/me/a.pdf`, `file:///home/me/a.pdf`,
/// `C:/Users/me/a.pdf`). Remote URLs yield `None`.
pub(super) fn url_to_path(url: &str) -> Option<PathBuf> {
    if url.starts_with("file:") {
        return url::Url::parse(url).ok()?.to_file_path().ok();
    }
    let path = PathBuf::from(url);
    path.is_absolute().then_some(path)
}

fn read_docdata(path: &Path) -> Option<DocData> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_DOCDATA_BYTES {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    Some(parse(&String::from_utf8_lossy(&bytes)))
}

/// The page Okular saved for `document` (located by file size and name, confirmed by the
/// recorded URL when there is one).
pub(super) fn page_for(dirs: &[PathBuf], document: &Path) -> Option<u32> {
    let size = fs::metadata(document).ok()?.len();
    let name = document.file_name()?.to_string_lossy().into_owned();
    let file_name = format!("{size}.{name}.xml");
    dirs.iter().find_map(|dir| {
        let data = read_docdata(&dir.join(&file_name))?;
        let matches = match data.url.as_deref().and_then(url_to_path) {
            Some(recorded) => recorded == document,
            None => true,
        };
        if matches { data.page } else { None }
    })
}

/// Existing documents named `name` that Okular has metadata for.
pub(super) fn documents_named(dirs: &[PathBuf], name: &str) -> Vec<PathBuf> {
    let suffix = format!(".{name}.xml");
    let mut found: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten().take(MAX_SCANNED) {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let Some(size) = file_name.strip_suffix(&suffix) else {
                continue;
            };
            if size.is_empty() || !size.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let Some(path) = read_docdata(&entry.path())
                .and_then(|data| data.url)
                .and_then(|url| url_to_path(&url))
            else {
                continue;
            };
            if path
                .file_name()
                .is_some_and(|n| n.to_string_lossy() == name)
                && path.is_file()
                && !found.contains(&path)
            {
                found.push(path);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE documentInfo>
<documentInfo url="/home/me/Papers/R&amp;D notes.pdf">
 <generalInfo>
  <history>
   <oldPage viewport="0;C2:0.5:0.241:1"/>
   <current viewport="11;C2:0.499:0.318:1"/>
  </history>
  <views>
   <view name="PageView">
    <zoom mode="1" value="1.2"/>
   </view>
  </views>
 </generalInfo>
</documentInfo>
"#;

    #[test]
    fn parses_url_and_zero_based_page() {
        assert_eq!(
            parse(FIXTURE),
            DocData {
                url: Some("/home/me/Papers/R&D notes.pdf".into()),
                page: Some(12),
            }
        );
    }

    #[test]
    fn parses_alternative_forms_and_garbage() {
        let legacy = r#"<documentInfo url='file:///home/me/a.pdf'><history><current page="4"/></history></documentInfo>"#;
        assert_eq!(
            parse(legacy),
            DocData {
                url: Some("file:///home/me/a.pdf".into()),
                page: Some(4),
            }
        );
        // `<currentX>` is not `<current>`; no history means no page.
        let other = r#"<documentInfo url="/a.pdf"><currentX viewport="3"/></documentInfo>"#;
        assert_eq!(parse(other).page, None);
        assert_eq!(parse(""), DocData::default());
        assert_eq!(
            parse("<documentInfo url=\"unterminated"),
            DocData::default()
        );
        assert_eq!(parse("<<<>>> &&& <current viewport=\"x\"/>").page, None);
        assert_eq!(
            parse(r#"<current viewport="4294967295;C2"/>"#).page,
            None,
            "overflow is rejected"
        );
    }

    #[test]
    fn attribute_names_inside_values_are_skipped() {
        let tag = r#"<current note=" viewport=1" viewport="5;C2""#;
        assert_eq!(attribute(tag, "viewport").as_deref(), Some("5;C2"));
        assert_eq!(
            attribute(r#"<x urlx="a" url="b""#, "url").as_deref(),
            Some("b")
        );
        assert_eq!(attribute(r#"<x url="open"#, "url"), None);
    }

    #[test]
    fn unescapes_entities() {
        assert_eq!(
            unescape("a &amp; b &lt;c&gt; &#233;&#x41; &bogus; &"),
            "a & b <c> éA &bogus; &"
        );
    }

    #[test]
    #[cfg(unix)]
    fn converts_urls_to_paths() {
        assert_eq!(
            url_to_path("/home/me/a.pdf"),
            Some(PathBuf::from("/home/me/a.pdf"))
        );
        assert_eq!(url_to_path("relative.pdf"), None);
        assert_eq!(url_to_path("https://example.com/a.pdf"), None);
        #[cfg(unix)]
        assert_eq!(
            url_to_path("file:///home/me/My%20Paper.pdf"),
            Some(PathBuf::from("/home/me/My Paper.pdf"))
        );
    }

    #[test]
    fn finds_pages_and_documents_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let docdata = dir.path().join("docdata");
        fs::create_dir(&docdata).unwrap();
        let docs = dir.path().join("docs");
        fs::create_dir(&docs).unwrap();
        let paper = docs.join("paper.pdf");
        fs::write(&paper, b"%PDF-1.7 test").unwrap();
        let size = fs::metadata(&paper).unwrap().len();
        fs::write(
            docdata.join(format!("{size}.paper.pdf.xml")),
            format!(
                r#"<documentInfo url="{}"><history><current viewport="2;C2:0:0:1"/></history></documentInfo>"#,
                paper.display()
            ),
        )
        .unwrap();
        // A same-named document elsewhere that no longer exists.
        fs::write(
            docdata.join("99.paper.pdf.xml"),
            r#"<documentInfo url="/nonexistent/paper.pdf"><history><current viewport="8"/></history></documentInfo>"#,
        )
        .unwrap();
        fs::write(docdata.join("notdigits.paper.pdf.xml"), "<x/>").unwrap();

        let dirs = vec![dir.path().join("missing"), docdata.clone()];
        assert_eq!(page_for(&dirs, &paper), Some(3));
        assert_eq!(documents_named(&dirs, "paper.pdf"), vec![paper.clone()]);
        assert!(documents_named(&dirs, "other.pdf").is_empty());

        // A docdata file recording another document with the same size and name is ignored.
        let twin = dir.path().join("twin");
        fs::create_dir(&twin).unwrap();
        let other = twin.join("paper.pdf");
        fs::write(&other, b"%PDF-1.7 test").unwrap();
        assert_eq!(page_for(&dirs, &other), None);
    }
}
