//! `file:` URIs, as VS Code stores them (`file:///c%3A/Users/me/My%20Project`,
//! `file:///home/me/reframe`), converted to local paths.
//!
//! The URI is parsed with the `url` crate; the path is assembled here instead of with
//! `Url::to_file_path` because that method only understands the OS it was compiled for, and
//! the conversion must be testable for every OS.

use std::path::PathBuf;

use reframe_platform::OsKind;
use url::Url;

/// The local path a `file:` URI points to on `os`.
///
/// Returns `None` for other schemes (`vscode-remote:`, `vscode-vfs:`, `untitled:`...), for
/// hosts other than `localhost` (except UNC shares on Windows), for malformed URIs and for
/// paths that are not valid UTF-8 after percent-decoding.
pub fn file_uri_to_path(uri: &str, os: OsKind) -> Option<PathBuf> {
    file_uri_to_string(uri, os).map(PathBuf::from)
}

/// Whether `s` starts with a URI scheme (`file:`, `vscode-remote:`...). A Windows drive
/// letter (`C:\`) is not a scheme.
pub fn has_scheme(s: &str) -> bool {
    let Some(colon) = s.find(':') else {
        return false;
    };
    let scheme = &s[..colon];
    scheme.len() >= 2
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

fn file_uri_to_string(uri: &str, os: OsKind) -> Option<String> {
    let url = Url::parse(uri.trim()).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    let host = url
        .host_str()
        .filter(|h| !h.is_empty() && !h.eq_ignore_ascii_case("localhost"));
    let mut segments = url
        .path_segments()?
        .map(percent_decode)
        .collect::<Option<Vec<String>>>()?;
    // A trailing slash yields an empty last segment; keep a lone root segment.
    while segments.len() > 1 && segments.last().is_some_and(String::is_empty) {
        segments.pop();
    }
    let invalid = |s: &String| {
        s.contains('\0') || s.contains('/') || (os == OsKind::Windows && s.contains('\\'))
    };
    if segments.iter().any(invalid) {
        return None;
    }
    match os {
        OsKind::Windows => windows_path(host, &segments),
        OsKind::Macos | OsKind::Linux | OsKind::Other => {
            if host.is_some() {
                return None;
            }
            Some(format!("/{}", segments.join("/")))
        }
    }
}

fn windows_path(host: Option<&str>, segments: &[String]) -> Option<String> {
    if let Some(host) = host {
        // UNC share: file://server/share/dir → \\server\share\dir
        let parts: Vec<&str> = segments
            .iter()
            .map(String::as_str)
            .filter(|s| !s.is_empty())
            .collect();
        if parts.is_empty() {
            return None;
        }
        return Some(format!(r"\\{host}\{}", parts.join(r"\")));
    }
    let (first, rest) = segments.split_first()?;
    let drive = drive_letter(first)?;
    let mut path = format!(r"{drive}:\");
    path.push_str(&rest.join(r"\"));
    Some(path)
}

/// `c:` / `C:` / `c|` → `C` (VS Code writes lower-case drive letters; Windows shows upper case).
fn drive_letter(segment: &str) -> Option<char> {
    let mut chars = segment.chars();
    match (chars.next(), chars.next(), chars.next()) {
        (Some(letter), Some(':' | '|'), None) if letter.is_ascii_alphabetic() => {
            Some(letter.to_ascii_uppercase())
        }
        _ => None,
    }
}

/// Decode `%XX` escapes; malformed escapes are kept literally. `None` if the result is not
/// valid UTF-8.
fn percent_decode(segment: &str) -> Option<String> {
    fn hex(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((high << 4) | low);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(uri: &str, os: OsKind) -> Option<String> {
        file_uri_to_path(uri, os).map(|p| p.to_string_lossy().into_owned())
    }

    #[test]
    fn unix_uris() {
        let l = OsKind::Linux;
        assert_eq!(
            path("file:///home/me/reframe", l).as_deref(),
            Some("/home/me/reframe")
        );
        assert_eq!(
            path("file:///home/me/My%20Project%20%231/", l).as_deref(),
            Some("/home/me/My Project #1")
        );
        assert_eq!(
            path("file:///Users/me/caf%C3%A9", OsKind::Macos).as_deref(),
            Some("/Users/me/café")
        );
        assert_eq!(
            path("file://localhost/srv/app", l).as_deref(),
            Some("/srv/app")
        );
        assert_eq!(path("file:///", l).as_deref(), Some("/"));
        // Malformed escapes stay literal.
        assert_eq!(
            path("file:///tmp/100%25%zz", l).as_deref(),
            Some("/tmp/100%%zz")
        );
        assert_eq!(path("file://server/share/x", l), None);
        assert_eq!(path("file:///tmp/a%2Fb", l), None);
        assert_eq!(path("file:///tmp/bad%FF", l), None);
    }

    #[test]
    fn windows_uris() {
        let w = OsKind::Windows;
        assert_eq!(
            path("file:///c%3A/Users/me/My%20Project", w).as_deref(),
            Some(r"C:\Users\me\My Project")
        );
        assert_eq!(
            path("file:///D:/src/reframe/", w).as_deref(),
            Some(r"D:\src\reframe")
        );
        assert_eq!(path("file:///e%3A", w).as_deref(), Some(r"E:\"));
        assert_eq!(
            path("file:///c%3A/w/team.code-workspace", w).as_deref(),
            Some(r"C:\w\team.code-workspace")
        );
        assert_eq!(
            path("file://nas/projects/My%20App", w).as_deref(),
            Some(r"\\nas\projects\My App")
        );
        assert_eq!(path("file:///home/me/x", w), None);
        assert_eq!(path("file:///c%3A/a%5Cb", w), None);
    }

    #[test]
    fn schemes() {
        assert!(has_scheme("file:///x"));
        assert!(has_scheme("vscode-remote://ssh-remote%2Bbox/x"));
        assert!(!has_scheme(r"C:\Users\me"));
        assert!(!has_scheme("/home/me/a:b"));
        assert!(!has_scheme("reframe"));
    }

    #[test]
    fn other_schemes_and_garbage_are_rejected() {
        for uri in [
            "vscode-remote://ssh-remote%2Bbox/home/me/srv",
            "vscode-remote://wsl%2Bubuntu/home/me/x",
            "vscode-vfs://github/microsoft/vscode",
            "untitled:Untitled-1",
            "/home/me/plain-path",
            "",
            "not a uri",
        ] {
            assert_eq!(path(uri, OsKind::Linux), None, "{uri}");
            assert_eq!(path(uri, OsKind::Windows), None, "{uri}");
        }
    }
}
