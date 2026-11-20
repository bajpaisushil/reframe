//! Spotify links: strict validation of the playlist / album / track a user configures.
//!
//! Accepted forms (anything else is rejected):
//! - `spotify:<kind>:<id>` with `<kind>` one of playlist, album, track, artist, show, episode;
//! - the legacy `spotify:user:<name>:playlist:<id>`;
//! - `https://open.spotify.com/[intl-xx/][embed/]<kind>/<id>[?…][#…]` (also `http://` and the
//!   legacy `/user/<name>/playlist/<id>`), converted to the URI form.
//!
//! Ids must be exactly 22 base-62 characters and user names `[A-Za-z0-9._-]`, so the result
//! only ever contains `[A-Za-z0-9:._-]`: it is safe as a single command-line argument
//! (`--uri=…`) and can only ever reach the `spotify:` URL handler.

const MAX_INPUT_LEN: usize = 256;
const KINDS: &[&str] = &["playlist", "album", "track", "artist", "show", "episode"];
const ID_LEN: usize = 22;

/// Canonical `spotify:` URI for a Spotify URI or `open.spotify.com` link; `None` for anything
/// else.
pub fn normalize_uri(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty()
        || input.len() > MAX_INPUT_LEN
        || !input.chars().all(|c| c.is_ascii_graphic())
    {
        return None;
    }
    if let Some(rest) = strip_prefix_ignore_case(input, "spotify:") {
        return from_parts(&rest.split(':').collect::<Vec<_>>());
    }
    let rest = strip_prefix_ignore_case(input, "https://")
        .or_else(|| strip_prefix_ignore_case(input, "http://"))?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    // Exactly the web player host: no credentials, port or look-alike host.
    if !authority.eq_ignore_ascii_case("open.spotify.com") {
        return None;
    }
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let mut segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segments.first().is_some_and(|s| is_locale_segment(s)) {
        segments.remove(0);
    }
    if segments.first() == Some(&"embed") {
        segments.remove(0);
    }
    from_parts(&segments)
}

fn from_parts(parts: &[&str]) -> Option<String> {
    match parts {
        [kind, id] if KINDS.contains(kind) && is_id(id) => Some(format!("spotify:{kind}:{id}")),
        ["user", user, "playlist", id] if is_user(user) && is_id(id) => {
            Some(format!("spotify:user:{user}:playlist:{id}"))
        }
        _ => None,
    }
}

fn is_id(text: &str) -> bool {
    text.len() == ID_LEN && text.chars().all(|c| c.is_ascii_alphanumeric())
}

fn is_user(text: &str) -> bool {
    (1..=64).contains(&text.len())
        && !text.starts_with(['.', '-'])
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// `intl-de`, `intl-pt-br`.
fn is_locale_segment(text: &str) -> bool {
    text.strip_prefix("intl-").is_some_and(|locale| {
        (2..=8).contains(&locale.len())
            && locale.chars().all(|c| c.is_ascii_alphabetic() || c == '-')
    })
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "37i9dQZF1DXcBWIGoYBM5M";

    #[test]
    fn accepts_uris_and_web_links() {
        let expect = |input: &str, uri: &str| {
            assert_eq!(normalize_uri(input).as_deref(), Some(uri), "{input}");
        };
        expect(
            &format!("spotify:playlist:{ID}"),
            &format!("spotify:playlist:{ID}"),
        );
        expect(
            &format!("  Spotify:album:{ID}\n"),
            &format!("spotify:album:{ID}"),
        );
        expect(
            &format!("spotify:track:{ID}"),
            &format!("spotify:track:{ID}"),
        );
        expect(
            &format!("spotify:user:spotify:playlist:{ID}"),
            &format!("spotify:user:spotify:playlist:{ID}"),
        );
        expect(
            &format!("https://open.spotify.com/playlist/{ID}?si=abc123&utm=x"),
            &format!("spotify:playlist:{ID}"),
        );
        expect(
            &format!("https://OPEN.SPOTIFY.COM/intl-de/album/{ID}"),
            &format!("spotify:album:{ID}"),
        );
        expect(
            &format!("http://open.spotify.com/embed/show/{ID}#top"),
            &format!("spotify:show:{ID}"),
        );
        expect(
            &format!("https://open.spotify.com/user/some.user_1/playlist/{ID}"),
            &format!("spotify:user:some.user_1:playlist:{ID}"),
        );
    }

    #[test]
    fn rejects_hostile_and_malformed_input() {
        let hostile = [
            String::new(),
            "   ".into(),
            format!("spotify:playlist:{ID} --disable-gpu"),
            format!("spotify:playlist:{ID};rm -rf ~"),
            format!("spotify:playlist:{ID}\n--uri=spotify:track:{ID}"),
            format!("spotify:playlist:{ID}\0"),
            format!("spotify:playlist:{ID}?si=1"),
            format!("spotify:playlist:{ID}:extra"),
            format!("spotify:Playlist:{ID}"),
            format!("spotify:podcast:{ID}"),
            "spotify:playlist:../../etc/passwd".into(),
            "spotify:playlist:short".into(),
            format!("spotify:playlist:{ID}A"),
            format!("spotify:playlist:{}", "é".repeat(22)),
            format!("spоtify:playlist:{ID}"), // Cyrillic "о"
            format!("spotify:user:-rf:playlist:{ID}"),
            format!("spotify:user:a/b:playlist:{ID}"),
            "javascript:alert(1)".into(),
            "file:///etc/passwd".into(),
            format!("https://evil.example/open.spotify.com/playlist/{ID}"),
            format!("https://open.spotify.com.evil.example/playlist/{ID}"),
            format!("https://user:pass@open.spotify.com/playlist/{ID}"),
            format!("https://open.spotify.com:8080/playlist/{ID}"),
            format!("https://open.spotify.com/playlist/%2e%2e/{ID}"),
            format!("https://open.spotify.com/playlist/{ID}/extra"),
            format!("https://open.spotify.com/intl-de/intl-fr/album/{ID}"),
            format!("ftp://open.spotify.com/playlist/{ID}"),
            format!("spotify:playlist:{ID}{}", "x".repeat(300)),
            format!("https://open.spotify.com/playlist/{ID} --flag"),
        ];
        for input in hostile {
            assert_eq!(normalize_uri(&input), None, "{input:?}");
        }
    }

    #[test]
    fn output_only_contains_safe_characters() {
        let uri = normalize_uri(&format!(
            "https://open.spotify.com/user/a-b.c/playlist/{ID}"
        ))
        .unwrap();
        assert!(
            uri.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '.' | '_' | '-'))
        );
    }
}
