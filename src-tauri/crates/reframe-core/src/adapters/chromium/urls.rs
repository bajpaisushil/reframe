//! Which tab addresses are worth reopening.
//!
//! Only web pages and local files are restored: `http`, `https`, `file` and `ftp`. Browser
//! internal pages (`chrome://`, `edge://`, `brave://`, `about:`, `chrome-search://`,
//! `devtools://`), extension pages, `javascript:`, `data:` and `blob:` URLs are skipped: they
//! either make no sense in a new window or could run code when passed on a command line.
//! Hosted "new tab" pages are skipped too.

use url::Url;

/// Longest address kept (characters). Longer ones are almost always transient state blobs, and
/// every kept address must fit on a Windows command line on its own.
pub const MAX_URL_CHARS: usize = 8192;

const RESTORABLE_SCHEMES: [&str; 4] = ["http", "https", "file", "ftp"];

/// Server-hosted new-tab pages (older Chrome, Edge).
const NEW_TAB_PAGES: [&str; 3] = [
    "https://www.google.com/_/chrome/newtab",
    "https://ntp.msn.com/edge/ntp",
    "https://ntp.msn.cn/edge/ntp",
];

/// The canonical form of `raw` if it should be reopened, `None` otherwise. Embedded
/// credentials (`https://user:password@host/`) are removed: they are never stored.
pub fn restorable_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > MAX_URL_CHARS {
        return None;
    }
    let mut url = Url::parse(raw).ok()?;
    if !RESTORABLE_SCHEMES.contains(&url.scheme()) {
        return None;
    }
    if url.scheme() != "file" && url.host_str().is_none_or(str::is_empty) {
        return None;
    }
    if !url.username().is_empty() || url.password().is_some() {
        // Cannot fail for URLs with a host (checked above) or `file:` URLs.
        let _ = url.set_username("");
        let _ = url.set_password(None);
    }
    let canonical: String = url.into();
    if canonical.len() > MAX_URL_CHARS || NEW_TAB_PAGES.iter().any(|p| canonical.starts_with(p)) {
        return None;
    }
    Some(canonical)
}

/// Host of a web address without a leading `www.` (used when a page has no title).
pub fn display_host(raw: &str) -> Option<String> {
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    let host = host.strip_prefix("www.").unwrap_or(host);
    (!host.is_empty()).then(|| host.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_web_pages_and_files() {
        assert_eq!(
            restorable_url("https://example.com/a?b=c#d").as_deref(),
            Some("https://example.com/a?b=c#d")
        );
        assert_eq!(
            restorable_url("http://example.com").as_deref(),
            Some("http://example.com/")
        );
        assert_eq!(
            restorable_url("file:///home/me/notes.html").as_deref(),
            Some("file:///home/me/notes.html")
        );
        assert_eq!(
            restorable_url("file:///C:/Users/me/report.pdf").as_deref(),
            Some("file:///C:/Users/me/report.pdf")
        );
        assert_eq!(
            restorable_url("ftp://ftp.example.org/pub/").as_deref(),
            Some("ftp://ftp.example.org/pub/")
        );
        assert_eq!(
            restorable_url("  https://example.com/x  ").as_deref(),
            Some("https://example.com/x")
        );
    }

    #[test]
    fn skips_internal_and_dangerous_addresses() {
        for raw in [
            "chrome://newtab/",
            "chrome://settings/",
            "edge://newtab/",
            "brave://newtab/",
            "about:blank",
            "chrome-search://local-ntp/local-ntp.html",
            "devtools://devtools/bundled/inspector.html",
            "chrome-extension://abcdefghijklmnop/popup.html",
            "javascript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "blob:https://example.com/uuid",
            "view-source:https://example.com/",
            "https://www.google.com/_/chrome/newtab?ie=UTF-8",
            "https://ntp.msn.com/edge/ntp?locale=en-US",
            "--gpu-launcher=calc.exe",
            "",
            "   ",
            "not a url",
            "https://",
        ] {
            assert_eq!(restorable_url(raw), None, "{raw:?}");
        }
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_CHARS));
        assert_eq!(restorable_url(&long), None);
    }

    #[test]
    fn strips_credentials() {
        assert_eq!(
            restorable_url("https://user:secret@example.com/private").as_deref(),
            Some("https://example.com/private")
        );
        assert_eq!(
            restorable_url("ftp://anonymous@ftp.example.org/").as_deref(),
            Some("ftp://ftp.example.org/")
        );
    }

    #[test]
    fn display_host_drops_www() {
        assert_eq!(
            display_host("https://www.example.com/x").as_deref(),
            Some("example.com")
        );
        assert_eq!(display_host("file:///tmp/a.html"), None);
        assert_eq!(display_host("nonsense"), None);
    }
}
