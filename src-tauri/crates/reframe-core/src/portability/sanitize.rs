//! Removing sensitive data (credentials, tokens, secret-looking parameters) before export.
//!
//! - **URLs** (browser tabs, URL arguments, media links): only `http`, `https`, `file` and `ftp`
//!   survive (`javascript:`, `data:`, `blob:`, `chrome:`, `about:`, extension pages... are
//!   dropped). User names and passwords are removed, as are query, fragment and `;matrix`
//!   parameters whose names look secret — tokens, API keys, passwords, OAuth `code` + `state`,
//!   session ids, signatures, cloud presigned-URL parameters; names are also checked decoded
//!   twice, split at separators only, and as bare credentials (`?ghp_…`) — or whose values have
//!   a known credential format or carry a nested URL or parameter list with secrets of its own
//!   (`?next=https://user:pass@…`, `?redirect_uri=…%3Fcode%3D…`). Fragments with tokens
//!   (`#access_token=…`, `#/magic/eyJ…`) are dropped. Everything else is kept exactly as
//!   written (percent-encoding included). URLs are dropped entirely when their *path* holds a
//!   credential (a known format, a token-shaped segment after `reset`, `verify`, `invite`,
//!   `webhooks`..., an embedded `user:pass@` URL), when they are webhook endpoints (Slack,
//!   Teams, Discord, Zapier), or when longer than [`MAX_URL_LEN`].
//! - **Launch arguments**: options with secret-looking names are dropped together with their
//!   value; `--user name:password` is dropped; credential-shaped values and values with an
//!   embedded known credential are dropped; URLs are sanitized as above.
//! - **Terminal startup commands** are kept (the user wrote them) with secrets replaced by
//!   `<redacted>` (see `command`), and always switched off (`startup_command_approved =
//!   false`). Terminal window titles, which often show the running command, are redacted the
//!   same way.
//! - **Titles and labels**: URLs inside them are sanitized and known credential formats
//!   replaced.
//! - **Media links** go through a scheme whitelist (`spotify:`, web/file URLs, Apple Music,
//!   Deezer, Tidal); OS protocol handlers that could run code are dropped.
//! - **`Custom` adapter data** is dropped entirely: its meaning is unknown, so it cannot be
//!   vetted (an adapter may export a vetted version through its own `sanitize_for_export`).
//! - Filesystem paths are kept: imports remap them.
//!
//! Every function here is idempotent: sanitizing its own output changes nothing.
//!
//! Limitations: detection is heuristic (see `secrets`). Secrets stored under innocent names
//! with ordinary-looking values, positional passwords (`tool user hunter2`), short options
//! whose meaning depends on the program (`-p`), short or word-like tokens in URL paths, and
//! secrets embedded in URL paths in unknown formats are not recognised.

use url::Url;

use super::command::redact_command;
use super::limits::{
    MAX_BROWSER_WINDOWS, MAX_COMMAND_LEN, MAX_STATE_ITEMS, MAX_TABS_PER_WINDOW, MAX_TEXT_LEN,
    MAX_URL_LEN,
};
use super::paths::looks_like_absolute_path;
use super::secrets::{
    NameKind, is_auth_code_name, is_known_credential_format, is_secret_name, looks_like_credential,
    looks_like_token_in_context, name_words, scrub_known_credentials,
};
use crate::model::{
    AppState, BrowserTab, BrowserWindow, DocumentRef, FolderRef, ProjectRef, TerminalSession,
};

/// Schemes that survive in browser tabs and URL arguments.
pub(crate) const WEB_SCHEMES: &[&str] = &["http", "https", "file", "ftp"];
/// URL schemes accepted for media links (besides `spotify:` URIs).
const MEDIA_URL_SCHEMES: &[&str] = &[
    "http", "https", "file", "ftp", "music", "itms", "itmss", "deezer", "tidal",
];
/// Inputs longer than this are rejected without parsing.
const MAX_URL_INPUT_LEN: usize = 64 * 1024;
/// URLs and parameter lists nested in parameter values are examined this many levels deep.
const MAX_NESTING: u8 = 3;
/// Sanitizing a value is repeated until nothing changes, at most this many times (removing an
/// unparseable part can leave a URL that parses, and is then normalized); a value that is
/// still changing is dropped.
const MAX_PASSES: usize = 4;
/// Fragments containing one of these (case-insensitive, percent-decoded) are dropped.
const TOKEN_FRAGMENT_MARKERS: &[&str] = &["access_token", "id_token", "refresh_token", "token="];
/// Parameters that only make sense next to a URL signature (AWS SigV2/SigV4, Google Cloud
/// Storage, Azure SAS, CloudFront). They are dropped together with the signature.
const SIGNED_URL_PARAMETERS: &[&str] = &[
    "expires",
    "keypairid",
    "policy",
    "googleaccessid",
    "awsaccesskeyid",
    "signedheaders",
    "sv",
    "ss",
    "srt",
    "sp",
    "se",
    "st",
    "spr",
    "sr",
    "si",
    "sip",
    "skoid",
    "sktid",
    "skt",
    "ske",
    "sks",
    "skv",
    "sdd",
];

/// Result of [`check_url`].
#[derive(Debug)]
pub(crate) enum UrlCheck {
    /// Allowed, nothing had to be removed.
    Clean(Url),
    /// Allowed after removing credentials or secret parameters.
    Sanitized(Url),
    Rejected(Rejection),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rejection {
    /// Not a parseable absolute URL.
    Invalid,
    /// The scheme is not allowed.
    Scheme,
    /// Longer than [`MAX_URL_LEN`] even after sanitizing.
    TooLong,
    /// A path segment is a credential, which cannot be removed without breaking the URL.
    CredentialInPath,
}

/// Parse `raw` and remove everything secret from it; see the module docs.
pub(crate) fn check_url(raw: &str, schemes: &[&str]) -> UrlCheck {
    check_url_at(raw, schemes, 0)
}

/// [`check_url`] for a URL nested `depth` levels deep in parameter values.
fn check_url_at(raw: &str, schemes: &[&str], depth: u8) -> UrlCheck {
    let raw = raw.trim();
    if raw.is_empty() {
        return UrlCheck::Rejected(Rejection::Invalid);
    }
    if raw.len() > MAX_URL_INPUT_LEN {
        return UrlCheck::Rejected(Rejection::TooLong);
    }
    let Ok(mut url) = Url::parse(raw) else {
        return UrlCheck::Rejected(Rejection::Invalid);
    };
    if !schemes.contains(&url.scheme()) {
        return UrlCheck::Rejected(Rejection::Scheme);
    }
    if is_secret_endpoint(&url) {
        return UrlCheck::Rejected(Rejection::CredentialInPath);
    }
    let mut changed = false;
    if !url.username().is_empty() || url.password().is_some() {
        // These only fail for URLs that cannot carry credentials, which then have none.
        let _ = url.set_password(None);
        let _ = url.set_username("");
        changed = true;
    }
    if url.path().contains(';')
        && let Some(path) = filter_matrix_params(url.path())
    {
        url.set_path(&path);
        changed = true;
    }
    let segments: Vec<String> = url.path().split('/').map(percent_decode).collect();
    if segments_have_credential(segments.iter().map(String::as_str))
        || has_embedded_userinfo(&percent_decode(url.path()))
    {
        return UrlCheck::Rejected(Rejection::CredentialInPath);
    }
    if let Some(query) = url.query()
        && let Some(filtered) = filter_query_at(query, depth)
    {
        url.set_query(filtered.as_deref());
        changed = true;
    }
    if let Some(fragment) = url.fragment()
        && let Some(filtered) = filter_fragment(fragment, depth)
    {
        url.set_fragment(filtered.as_deref());
        changed = true;
    }
    if url.as_str().len() > MAX_URL_LEN {
        return UrlCheck::Rejected(Rejection::TooLong);
    }
    if changed {
        UrlCheck::Sanitized(url)
    } else {
        UrlCheck::Clean(url)
    }
}

/// Sanitize a URL for export: `None` when it must not be exported at all (scheme other than
/// `http`/`https`/`file`/`ftp`, unparseable, credential in the path, longer than
/// [`MAX_URL_LEN`]). The result is the normalized URL (lower-case scheme and host,
/// `https://example.com` → `https://example.com/`) without user info and secret parameters;
/// everything else, including percent-encoded characters, is kept as written.
pub fn sanitize_url(url: &str) -> Option<String> {
    match check_url(url, WEB_SCHEMES) {
        UrlCheck::Clean(url) | UrlCheck::Sanitized(url) => Some(url.into()),
        UrlCheck::Rejected(_) => None,
    }
}

/// Whether `url` is an absolute `http`/`https`/`file`/`ftp` URL of acceptable length (import
/// validation; nothing is removed).
pub(crate) fn is_allowed_web_url(url: &str) -> bool {
    url.len() <= MAX_URL_LEN
        && Url::parse(url.trim()).is_ok_and(|u| WEB_SCHEMES.contains(&u.scheme()))
}

/// Sanitize a media link: `spotify:` URIs (`spotify:playlist:37i9dQZF1DXcBWIGoYBM5M`) and
/// `http`/`https`/`file`/`ftp`/`music`/`itms`/`itmss`/`deezer`/`tidal` URLs (sanitized like
/// [`sanitize_url`]). Anything else — notably OS protocol handlers that can run code — yields
/// `None`.
pub fn sanitize_media_uri(uri: &str) -> Option<String> {
    let uri = uri.trim();
    if uri.is_empty() || uri.len() > MAX_URL_LEN {
        return None;
    }
    if uri
        .get(..8)
        .is_some_and(|p| p.eq_ignore_ascii_case("spotify:"))
    {
        let rest = &uri[8..];
        let valid = !rest.is_empty()
            && rest.split(':').all(|segment| {
                !segment.is_empty()
                    && segment
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '%'))
            });
        return valid.then(|| format!("spotify:{rest}"));
    }
    match check_url(uri, MEDIA_URL_SCHEMES) {
        UrlCheck::Clean(url) | UrlCheck::Sanitized(url) => Some(url.into()),
        UrlCheck::Rejected(_) => None,
    }
}

/// Remove secret parameters from a raw (still percent-encoded) query string, keeping every
/// other parameter exactly as written. `None` when nothing was removed, `Some(None)` when
/// nothing is left.
pub(crate) fn filter_query(query: &str) -> Option<Option<String>> {
    filter_query_at(query, 0)
}

/// [`filter_query`] for a query nested `depth` levels deep in parameter values.
///
/// Pairs are separated by `&`; each pair is also examined piece by piece at `;` (an old
/// separator some servers still accept), and is dropped as a whole when any piece is secret.
fn filter_query_at(query: &str, depth: u8) -> Option<Option<String>> {
    let pairs: Vec<(&str, Vec<(String, String)>)> = query
        .split('&')
        .map(|raw| {
            let pieces = raw
                .split(';')
                .filter_map(|piece| {
                    url::form_urlencoded::parse(piece.as_bytes())
                        .next()
                        .map(|(n, v)| (n.into_owned(), v.into_owned()))
                })
                .collect();
            (raw, pieces)
        })
        .collect();
    let names = || pairs.iter().flat_map(|(_, pieces)| pieces).map(|(n, _)| n);
    let auth_code = names().any(|name| is_auth_code_name(name));
    let signed = names().any(|name| is_signature_name(name));
    let mut removed = false;
    let mut kept = Vec::with_capacity(pairs.len());
    for (raw, pieces) in &pairs {
        if raw.is_empty() {
            continue;
        }
        let secret = pieces
            .iter()
            .any(|(name, value)| is_secret_parameter(name, value, auth_code, signed, depth));
        if secret {
            removed = true;
        } else {
            kept.push(*raw);
        }
    }
    if !removed {
        return None;
    }
    Some((!kept.is_empty()).then(|| kept.join("&")))
}

fn is_secret_parameter(name: &str, value: &str, auth_code: bool, signed: bool, depth: u8) -> bool {
    if is_secret_parameter_name(name) || is_known_credential_format(value) {
        return true;
    }
    let words = name_words(name);
    let joined = words.concat();
    let companion = (auth_code && matches!(joined.as_str(), "state" | "sessionstate"))
        || (signed
            && (SIGNED_URL_PARAMETERS.contains(&joined.as_str())
                || joined.starts_with("xamz")
                || joined.starts_with("xgoog")
                || words.first().is_some_and(|w| w == "oauth")));
    companion || nested_value_has_secret(value, depth)
}

/// A secret-looking parameter name, a bare credential used as a name (`?ghp_…`), or a name
/// that only reveals itself after decoding once more (`%2574oken` → `%74oken` → `token`).
fn is_secret_parameter_name(name: &str) -> bool {
    let mut current = name.to_string();
    for _ in 0..3 {
        if is_secret_name(&current, NameKind::Parameter) || is_known_credential_format(&current) {
            return true;
        }
        if !current.contains('%') {
            break;
        }
        let decoded = percent_decode(&current);
        if decoded == current {
            break;
        }
        current = decoded;
    }
    false
}

/// Whether a (decoded) parameter value carries a URL or a parameter list with secrets of its
/// own: `?next=https://user:pass@host/`, `?redirect=https%3A%2F%2Fapp%2Fcb%3Fcode%3D…`,
/// `?params=token%3Dabc`. Such parameters are dropped as a whole. Values nested deeper than
/// [`MAX_NESTING`] levels are not examined.
fn nested_value_has_secret(value: &str, depth: u8) -> bool {
    if depth >= MAX_NESTING || value.len() < 3 {
        return false;
    }
    if nested_text_has_secret(value, depth) {
        return true;
    }
    // Encoded twice (`https%3A%2F%2F…`): look at it decoded once more.
    value.contains('%') && {
        let decoded = percent_decode(value);
        decoded != value && nested_text_has_secret(&decoded, depth)
    }
}

fn nested_text_has_secret(value: &str, depth: u8) -> bool {
    let value = value.trim();
    if has_embedded_userinfo(value) {
        return true;
    }
    if url_scheme(value).is_some() {
        match check_url_at(value, WEB_SCHEMES, depth + 1) {
            UrlCheck::Clean(_) => {}
            UrlCheck::Sanitized(_) | UrlCheck::Rejected(Rejection::CredentialInPath) => {
                return true;
            }
            UrlCheck::Rejected(_) => {
                if redact_generic_url(value).is_some() {
                    return true;
                }
            }
        }
    }
    value.contains('=') && filter_query_at(value, depth + 1).is_some()
}

/// Whether `text` embeds a URL with a password anywhere: `…/https://user:pass@host/` in an
/// archive link's path, `#https://u:p@host` in a fragment.
fn has_embedded_userinfo(text: &str) -> bool {
    text.match_indices("://").any(|(i, _)| {
        let rest = &text[i + 3..];
        let authority = &rest[..rest
            .find(|c: char| matches!(c, '/' | '?' | '#' | '\\') || c.is_whitespace())
            .unwrap_or(rest.len())];
        authority
            .rsplit_once('@')
            .is_some_and(|(userinfo, _)| userinfo.contains(':'))
    })
}

/// Endpoints whose URLs are secrets in their entirety: incoming webhooks (Slack, Microsoft
/// Teams, Discord, Zapier).
fn is_secret_endpoint(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let path = url.path();
    host == "hooks.slack.com"
        || host == "hooks.zapier.com"
        || host.ends_with(".webhook.office.com")
        || (host == "outlook.office.com" && path.starts_with("/webhook"))
        || (matches!(
            host,
            "discord.com" | "discordapp.com" | "canary.discord.com" | "ptb.discord.com"
        ) && path.starts_with("/api/webhooks/"))
}

/// Words that make the following path segments (or fragment pieces) suspicious: a random-looking
/// value after `reset`, `verify`, `magic`, `invite`, `webhooks`... is a one-time credential.
const SENSITIVE_PATH_WORDS: &[&str] = &[
    "reset",
    "magic",
    "verify",
    "verification",
    "confirm",
    "confirmation",
    "activate",
    "activation",
    "invite",
    "invites",
    "invitation",
    "invitations",
    "login",
    "signin",
    "signup",
    "auth",
    "oauth",
    "authorize",
    "password",
    "passwords",
    "passwd",
    "token",
    "tokens",
    "secret",
    "secrets",
    "otp",
    "hooks",
    "key",
    "keys",
    "apikey",
    "apikeys",
    "session",
    "sessions",
    "unsubscribe",
];

fn is_sensitive_path_word(segment: &str) -> bool {
    name_words(segment)
        .iter()
        .any(|w| SENSITIVE_PATH_WORDS.contains(&w.as_str()) || w.starts_with("webhook"))
}

/// Whether one of the (decoded) path segments or fragment pieces is a credential: a known
/// credential format anywhere, or a token-shaped value after a sensitive word.
fn segments_have_credential<'a>(segments: impl IntoIterator<Item = &'a str>) -> bool {
    let mut sensitive = false;
    for segment in segments {
        if segment.is_empty() {
            continue;
        }
        if is_known_credential_format(segment)
            || (sensitive && looks_like_token_in_context(segment))
        {
            return true;
        }
        sensitive = sensitive || is_sensitive_path_word(segment);
    }
    false
}

fn is_signature_name(name: &str) -> bool {
    let words = name_words(name);
    let joined = words.concat();
    matches!(
        joined.as_str(),
        "signature" | "sig" | "xamzsignature" | "xgoogsignature" | "oauthsignature" | "hmac"
    ) || words.last().is_some_and(|w| w == "signature")
}

/// Fragments: drop token-bearing ones (`#access_token=…`, `#/magic/eyJ…`, `#/reset/8f14e4…`);
/// filter `#/route?a=1&token=…` and `#a=1&token=…` like a query. The part before the first
/// `?` is filtered too when it carries parameters, so the result never exposes new parameters
/// to a second pass (sanitizing is idempotent).
fn filter_fragment(fragment: &str, depth: u8) -> Option<Option<String>> {
    let decoded = percent_decode(fragment);
    let lower = decoded.to_ascii_lowercase();
    if TOKEN_FRAGMENT_MARKERS.iter().any(|m| lower.contains(m))
        || segments_have_credential(decoded.split(['/', '?', '&', '=', ';', '#', ',']))
        || has_embedded_userinfo(&decoded)
    {
        return Some(None);
    }
    let (route, query) = match fragment.find('?') {
        Some(i) => (&fragment[..i], Some(&fragment[i + 1..])),
        None => (fragment, None),
    };
    let new_route = if route.contains('=') {
        filter_query_at(route, depth)
    } else {
        None
    };
    let new_query = query.and_then(|q| filter_query_at(q, depth));
    if new_route.is_none() && new_query.is_none() {
        return None;
    }
    let route = match new_route {
        Some(filtered) => filtered.unwrap_or_default(),
        None => route.to_string(),
    };
    let query = match new_query {
        Some(filtered) => filtered,
        None => query.map(str::to_string),
    };
    Some(match query {
        Some(query) => Some(format!("{route}?{query}")),
        None if route.is_empty() => None,
        None => Some(route),
    })
}

/// Remove secret `;name=value` parameters from path segments (`/app;jsessionid=…/page`).
fn filter_matrix_params(path: &str) -> Option<String> {
    let mut removed = false;
    let segments: Vec<String> = path
        .split('/')
        .map(|segment| {
            let mut parts = segment.split(';');
            let mut kept = vec![parts.next().unwrap_or_default()];
            for param in parts {
                let (name, value) = param.split_once('=').unwrap_or((param, ""));
                let secret = is_secret_parameter_name(&percent_decode(name))
                    || is_known_credential_format(&percent_decode(value));
                if secret {
                    removed = true;
                } else {
                    kept.push(param);
                }
            }
            kept.join(";")
        })
        .collect();
    removed.then(|| segments.join("/"))
}

/// Lossy percent-decoding (invalid escapes are kept as written).
pub(crate) fn percent_decode(text: &str) -> String {
    fn hex(b: u8) -> Option<u8> {
        (b as char).to_digit(16).and_then(|d| u8::try_from(d).ok())
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((high << 4) | low);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The scheme when `value` starts with `scheme:` (two or more characters, so Windows drive
/// letters are not schemes).
pub(crate) fn url_scheme(value: &str) -> Option<&str> {
    let scheme = &value[..value.find(':')?];
    let mut chars = scheme.chars();
    let valid = scheme.len() >= 2
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    valid.then_some(scheme)
}

/// For URL-like values of any scheme (`postgres://user:pass@host/db`): remove a password (and
/// a credential-looking user name) from the authority, and secret query parameters, editing
/// the text in place so nothing else changes. `None` when there is nothing to remove.
pub(crate) fn redact_generic_url(value: &str) -> Option<String> {
    let authority_start = value.find("://")? + 3;
    let after = &value[authority_start..];
    let authority_len = after.find(['/', '?', '#']).unwrap_or(after.len());
    let authority = &after[..authority_len];
    let mut changed = false;
    let mut out = String::with_capacity(value.len());
    out.push_str(&value[..authority_start]);
    match authority.rfind('@') {
        Some(at) => {
            let userinfo = &authority[..at];
            let (user, password) = match userinfo.split_once(':') {
                Some((user, password)) => (user, Some(password)),
                None => (userinfo, None),
            };
            let user_is_secret = looks_like_credential(&percent_decode(user));
            if password.is_some() || user_is_secret {
                changed = true;
                if !user_is_secret && !user.is_empty() {
                    out.push_str(user);
                    out.push('@');
                }
            } else {
                out.push_str(userinfo);
                out.push('@');
            }
            out.push_str(&authority[at + 1..]);
        }
        None => out.push_str(authority),
    }
    let rest = &after[authority_len..];
    match rest.find('?') {
        Some(q) => {
            out.push_str(&rest[..q]);
            let query_and_fragment = &rest[q + 1..];
            let (query, fragment) = match query_and_fragment.find('#') {
                Some(h) => (&query_and_fragment[..h], &query_and_fragment[h..]),
                None => (query_and_fragment, ""),
            };
            match filter_query(query) {
                Some(filtered) => {
                    changed = true;
                    if let Some(filtered) = filtered {
                        out.push('?');
                        out.push_str(&filtered);
                    }
                }
                None => {
                    out.push('?');
                    out.push_str(query);
                }
            }
            out.push_str(fragment);
        }
        None => out.push_str(rest),
    }
    changed.then_some(out)
}

/// Sanitize launch arguments for export:
/// - options with secret-looking names are dropped with their value: `--token=x`,
///   `--password x`, `-Dapp.secret=x`, `/password:x` (Windows). A value that is an absolute
///   path is kept (`--key /home/a/.ssh/id_ed25519`, `--session ~/work.kitty`): it refers to a
///   file whose content is not exported;
/// - boolean switches about secrets are kept (`--no-password`, `--password-store=basic`);
/// - `--user`/`--proxy-user` values of the form `name:password` are dropped (`uid:gid` stays);
/// - credential-shaped values (known token formats, JWTs, long random hex/base64) are dropped;
/// - URLs lose user info and secret parameters (`--proxy-server=http://u:p@proxy:8080`);
/// - after `--`, arguments are positional: only the value rules apply.
///
/// Limitation: one-letter options (`-p`) are ambiguous across programs and are not treated as
/// secret, and positional secrets that look ordinary cannot be recognised.
pub fn sanitize_arguments(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut options_ended = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if !options_ended {
            if arg == "--" {
                options_ended = true;
                out.push(arg.clone());
                continue;
            }
            if let Some(option) = parse_option(arg) {
                let secret = is_secret_name(option.name, NameKind::Flag);
                match option.value {
                    Some(value) => {
                        if (secret && !is_file_reference(value))
                            || is_user_password(option.name, value)
                        {
                            continue;
                        }
                        match sanitize_argument_value(value) {
                            ArgumentValue::Keep => out.push(arg.clone()),
                            ArgumentValue::Replace(value) => {
                                out.push(format!("{}{value}", option.head));
                            }
                            ArgumentValue::Drop => {}
                        }
                    }
                    None if secret => {
                        // `--password hunter2`: the next argument is the value (dropped with
                        // the option), unless it is another option. A clean file reference
                        // (`--key /home/a/.ssh/id_ed25519`) is kept together with the option.
                        match args.get(i) {
                            Some(next) if is_file_reference(next) => {
                                i += 1;
                                if matches!(sanitize_argument_value(next), ArgumentValue::Keep) {
                                    out.push(arg.clone());
                                    out.push(next.clone());
                                }
                            }
                            Some(next) if !next.starts_with('-') => i += 1,
                            _ => {}
                        }
                    }
                    None if is_user_option(option.name) => match args.get(i) {
                        Some(next) if is_user_password(option.name, next) => i += 1,
                        _ => out.push(arg.clone()),
                    },
                    None => out.push(arg.clone()),
                }
                continue;
            }
        }
        match sanitize_argument_value(arg) {
            ArgumentValue::Keep => out.push(arg.clone()),
            ArgumentValue::Replace(value) => out.push(value),
            ArgumentValue::Drop => {}
        }
    }
    out
}

/// An option argument: `--name`, `--name=value`, `-name=value`, `/name:value`.
struct ParsedOption<'a> {
    name: &'a str,
    /// Everything up to and including the separator before the value.
    head: &'a str,
    value: Option<&'a str>,
}

fn parse_option(arg: &str) -> Option<ParsedOption<'_>> {
    let (prefix_len, windows) = if arg.starts_with("--") {
        (2, false)
    } else if arg.starts_with('-') {
        (1, false)
    } else if arg.starts_with('/') {
        (1, true)
    } else {
        return None;
    };
    let rest = &arg[prefix_len..];
    if windows {
        // `/name:value` or `/name=value` only: a bare `/name` is more likely a Unix path.
        let separator = rest.find([':', '='])?;
        let name = &rest[..separator];
        let valid = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
        return valid.then(|| ParsedOption {
            name,
            head: &arg[..prefix_len + separator + 1],
            value: Some(&rest[separator + 1..]),
        });
    }
    // `-` alone, `-5` (a negative number) and `--` are not options.
    if !rest.chars().next().is_some_and(char::is_alphabetic) {
        return None;
    }
    Some(match rest.find('=') {
        Some(separator) => ParsedOption {
            name: &rest[..separator],
            head: &arg[..prefix_len + separator + 1],
            value: Some(&rest[separator + 1..]),
        },
        None => ParsedOption {
            name: rest,
            head: arg,
            value: None,
        },
    })
}

/// An absolute path (`/…`, `~/…`, `C:\…`, `\\server\…`) that is not shaped like a credential:
/// a base64 token can start with `/` too, so values with `+` or `=` (base64 alphabet and
/// padding) or with a credential-shaped component do not count as paths.
pub(crate) fn is_file_reference(value: &str) -> bool {
    looks_like_absolute_path(value)
        && !value.contains(['+', '='])
        && !looks_like_credential(value.trim_start_matches(['/', '\\', '~']))
        && !value.split(['/', '\\']).any(looks_like_credential)
}

/// `--user`, `--proxy-user`, `--username`: options that may carry `name:password`.
fn is_user_option(name: &str) -> bool {
    matches!(
        name_words(name).concat().as_str(),
        "user" | "proxyuser" | "username" | "userpass" | "httpuser"
    )
}

/// `--user=alice:hunter2` (but not `--user=1000:1000`, a uid:gid pair).
fn is_user_password(name: &str, value: &str) -> bool {
    is_user_option(name)
        && value.split_once(':').is_some_and(|(_, password)| {
            !password.is_empty() && !password.bytes().all(|b| b.is_ascii_digit())
        })
}

enum ArgumentValue {
    Keep,
    Replace(String),
    Drop,
}

/// [`sanitize_argument_value_once`] until nothing changes, so the result is stable (sanitizing
/// an exported argument again changes nothing).
fn sanitize_argument_value(value: &str) -> ArgumentValue {
    let mut current = match sanitize_argument_value_once(value) {
        ArgumentValue::Replace(next) => next,
        other => return other,
    };
    for _ in 1..MAX_PASSES {
        match sanitize_argument_value_once(&current) {
            ArgumentValue::Keep => return ArgumentValue::Replace(current),
            ArgumentValue::Replace(next) => current = next,
            ArgumentValue::Drop => return ArgumentValue::Drop,
        }
    }
    ArgumentValue::Drop
}

fn sanitize_argument_value_once(value: &str) -> ArgumentValue {
    // A credential in a known format anywhere in the value (`Authorization:ghp_…`).
    if scrub_known_credentials(value).is_some() {
        return ArgumentValue::Drop;
    }
    if let Some(scheme) = url_scheme(value) {
        if WEB_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str()) {
            return match check_url(value, WEB_SCHEMES) {
                UrlCheck::Clean(_) => ArgumentValue::Keep,
                UrlCheck::Sanitized(url) => ArgumentValue::Replace(url.into()),
                UrlCheck::Rejected(Rejection::CredentialInPath) => ArgumentValue::Drop,
                UrlCheck::Rejected(_) => match redact_generic_url(value) {
                    Some(redacted) => ArgumentValue::Replace(redacted),
                    None => ArgumentValue::Keep,
                },
            };
        }
        if let Some(redacted) = redact_generic_url(value) {
            return ArgumentValue::Replace(redacted);
        }
    }
    if looks_like_credential(value) {
        ArgumentValue::Drop
    } else {
        ArgumentValue::Keep
    }
}

/// Sanitize adapter state for export (the default `AppAdapter::sanitize_for_export`):
/// - `Browser`: tab URLs through [`sanitize_url`] (tabs that become `None` are dropped, then
///   windows without tabs; the active tab follows its tab or the one that took its place),
///   at most [`MAX_BROWSER_WINDOWS`] windows of [`MAX_TABS_PER_WINDOW`] tabs;
/// - `Terminal`: startup commands kept with secrets redacted, always switched off; commands
///   longer than [`MAX_COMMAND_LEN`] are dropped (never truncated);
/// - `Media`: the link through [`sanitize_media_uri`];
/// - `Custom`: becomes `AppState::None` (unknown data may hold secrets);
/// - paths are kept; titles, labels and profile names are cut to [`MAX_TEXT_LEN`] characters
///   (NUL characters removed), and URLs inside titles and labels are sanitized too (a page
///   without a `<title>` shows its address, query string included); lists are cut to
///   [`MAX_STATE_ITEMS`] entries.
pub fn sanitize_state(state: &AppState) -> AppState {
    match state {
        AppState::None => AppState::None,
        AppState::Documents { documents } => AppState::Documents {
            documents: documents
                .iter()
                .take(MAX_STATE_ITEMS)
                .map(|d| DocumentRef {
                    path: d.path.clone(),
                    page: d.page,
                })
                .collect(),
        },
        AppState::Projects { projects } => AppState::Projects {
            projects: projects
                .iter()
                .take(MAX_STATE_ITEMS)
                .map(|p| ProjectRef {
                    path: p.path.clone(),
                    is_workspace_file: p.is_workspace_file,
                    window_title: clean_optional_title(&p.window_title),
                })
                .collect(),
        },
        AppState::Browser { windows, profile } => AppState::Browser {
            windows: windows
                .iter()
                .filter_map(sanitize_browser_window)
                .take(MAX_BROWSER_WINDOWS)
                .collect(),
            profile: clean_optional_text(profile),
        },
        AppState::Terminal { sessions } => AppState::Terminal {
            sessions: sessions
                .iter()
                .take(MAX_STATE_ITEMS)
                .map(sanitize_terminal_session)
                .collect(),
        },
        AppState::Folders { folders } => AppState::Folders {
            folders: folders
                .iter()
                .take(MAX_STATE_ITEMS)
                .map(|f| FolderRef {
                    path: f.path.clone(),
                    window_title: clean_optional_title(&f.window_title),
                })
                .collect(),
        },
        AppState::Media { uri, label } => AppState::Media {
            uri: uri.as_deref().and_then(sanitize_media_uri),
            label: clean_optional_title(label),
        },
        AppState::Custom { .. } => AppState::None,
    }
}

fn sanitize_browser_window(window: &BrowserWindow) -> Option<BrowserWindow> {
    let mut tabs = Vec::new();
    let mut kept_indices = Vec::new();
    for (index, tab) in window.tabs.iter().enumerate() {
        if tabs.len() == MAX_TABS_PER_WINDOW {
            break;
        }
        if let Some(url) = sanitize_url(&tab.url) {
            kept_indices.push(index);
            tabs.push(BrowserTab {
                url,
                title: clean_title(&tab.title, MAX_TEXT_LEN),
                pinned: tab.pinned,
            });
        }
    }
    if tabs.is_empty() {
        return None;
    }
    let active_tab = window.active_tab.map(|active| {
        let active = usize::try_from(active).unwrap_or(usize::MAX);
        let position = kept_indices
            .iter()
            .position(|&i| i >= active)
            .unwrap_or(tabs.len() - 1);
        u32::try_from(position).unwrap_or(0)
    });
    Some(BrowserWindow {
        tabs,
        active_tab,
        window_title: clean_optional_title(&window.window_title),
    })
}

fn sanitize_terminal_session(session: &TerminalSession) -> TerminalSession {
    TerminalSession {
        working_directory: session.working_directory.clone(),
        profile: clean_optional_text(&session.profile),
        startup_command: session
            .startup_command
            .as_deref()
            .map(|command| redact_command(&command.replace('\0', "")))
            .filter(|command| {
                !command.trim().is_empty() && command.chars().count() <= MAX_COMMAND_LEN
            }),
        startup_command_approved: false,
        window_title: session
            .window_title
            .as_deref()
            .map(|title| clean_terminal_title(title, MAX_TEXT_LEN)),
    }
}

/// `text` without NUL characters, cut to `max_chars` characters.
pub(crate) fn clean_text(text: &str, max_chars: usize) -> String {
    text.chars()
        .filter(|c| *c != '\0')
        .take(max_chars)
        .collect()
}

fn clean_optional_text(text: &Option<String>) -> Option<String> {
    text.as_deref().map(|t| clean_text(t, MAX_TEXT_LEN))
}

/// Titles and labels: NUL characters removed, URLs inside them sanitized like
/// [`sanitize_url`] (browsers show a page's address as its title when it has none, query
/// string included), credentials in a known format replaced, cut to `max_chars` characters.
pub(crate) fn clean_title(text: &str, max_chars: usize) -> String {
    let text: String = text
        .chars()
        .filter(|c| *c != '\0')
        .take(MAX_URL_LEN)
        .collect();
    let text = redact_urls_in_text(&text);
    let text = scrub_known_credentials(&text).unwrap_or(text);
    text.chars().take(max_chars).collect()
}

fn clean_optional_title(text: &Option<String>) -> Option<String> {
    text.as_deref().map(|t| clean_title(t, MAX_TEXT_LEN))
}

/// Terminal titles often show the running command line (`mysql -u root -phunter2`): secrets
/// are redacted as in startup commands, then the title is cleaned like any other.
pub(crate) fn clean_terminal_title(text: &str, max_chars: usize) -> String {
    let text: String = text
        .chars()
        .filter(|c| *c != '\0')
        .take(MAX_URL_LEN)
        .collect();
    clean_title(&redact_command(&text), max_chars)
}

/// Sanitize URL-like words (`https://…`, `example.com/cb?token=…`) inside free text.
fn redact_urls_in_text(text: &str) -> String {
    if !text.contains("://") && !text.contains('=') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    for piece in text.split_inclusive(char::is_whitespace) {
        let word = piece.trim_end_matches(char::is_whitespace);
        match redact_url_word(word) {
            Some(redacted) => out.push_str(&redacted),
            None => out.push_str(word),
        }
        out.push_str(&piece[word.len()..]);
    }
    out
}

/// [`redact_url_word_once`] until nothing changes; a word that keeps changing becomes `…`.
fn redact_url_word(word: &str) -> Option<String> {
    let mut current = redact_url_word_once(word)?;
    for _ in 1..MAX_PASSES {
        match redact_url_word_once(&current) {
            None => return Some(current),
            Some(next) => current = next,
        }
    }
    Some("…".to_string())
}

fn redact_url_word_once(word: &str) -> Option<String> {
    let has_scheme = word.contains("://");
    let has_parameters = word
        .find(['?', '#', ';'])
        .is_some_and(|i| word[i..].contains('='));
    if !has_scheme && !has_parameters {
        return None;
    }
    let candidate = if has_scheme {
        word.to_string()
    } else {
        format!("https://{word}")
    };
    match check_url(&candidate, WEB_SCHEMES) {
        UrlCheck::Clean(_) => None,
        UrlCheck::Sanitized(url) => {
            let url: String = url.into();
            Some(if has_scheme {
                url
            } else {
                url.strip_prefix("https://").unwrap_or(&url).to_string()
            })
        }
        UrlCheck::Rejected(Rejection::CredentialInPath) => Some("…".to_string()),
        UrlCheck::Rejected(_) => redact_generic_url(word),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn s(url: &str) -> Option<String> {
        sanitize_url(url)
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn safe_urls_are_untouched() {
        for url in [
            "https://example.com/",
            "https://www.google.com/search?q=rust+lang&oq=rust&sourceid=chrome&ie=UTF-8",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=42s",
            "https://en.wikipedia.org/wiki/Rust_(programming_language)#History",
            "https://example.com/a%20b/caf%C3%A9?q=%E2%9C%93&path=%2Fusr%2Fbin&x=a+b",
            "https://mail.google.com/mail/u/0/?authuser=0#inbox",
            "https://github.com/rust-lang/rust/commit/0023e1fdbd363b88a3dca4cd0cdb4b0fbdb056d1",
            "https://shop.example.com/?state=CA&country_code=US&keyword=boots&author=me",
            "file:///home/alice/Documents/report.pdf",
            "ftp://ftp.example.com/pub/file.tar.gz",
            "http://localhost:3000/dashboard?tab=settings",
            "https://app.example.com/#/settings/api-keys",
        ] {
            assert_eq!(s(url).as_deref(), Some(url), "{url}");
        }
        // Normalization only.
        assert_eq!(
            s("  HTTPS://Example.COM  ").as_deref(),
            Some("https://example.com/")
        );
    }

    #[test]
    fn user_info_is_removed() {
        assert_eq!(
            s("https://alice:hunter2@example.com/private").as_deref(),
            Some("https://example.com/private")
        );
        assert_eq!(
            s("https://ghp_0123456789abcdefghijABCDEFGHIJ012345@github.com/org/repo").as_deref(),
            Some("https://github.com/org/repo")
        );
        assert_eq!(
            s("ftp://anonymous:me%40example.com@ftp.example.com/").as_deref(),
            Some("ftp://ftp.example.com/")
        );
        assert_eq!(
            s("https://:secret@example.com/").as_deref(),
            Some("https://example.com/")
        );
    }

    #[test]
    fn secret_query_parameters_are_removed() {
        let cases = [
            (
                "https://example.com/cb?access_token=abc&page=2",
                "https://example.com/cb?page=2",
            ),
            (
                "https://example.com/?API_KEY=abc&apiKey=def&x-api-key=ghi&q=1",
                "https://example.com/?q=1",
            ),
            ("https://example.com/?token=abc", "https://example.com/"),
            (
                "https://example.com/?user=al&password=hunter2&pwd=x&passwd=y",
                "https://example.com/?user=al",
            ),
            (
                "https://example.com/?sessionid=1&sid=2&PHPSESSID=3&session=4&lang=en",
                "https://example.com/?lang=en",
            ),
            (
                "https://example.com/?client_secret=a&client_id=b&refresh_token=c&id_token=d",
                "https://example.com/?client_id=b",
            ),
            (
                "https://example.com/?jwt=a&otp=1&sig=x&signature=y&auth=z&key=k&secret=s",
                "https://example.com/",
            ),
            // A JWT under an innocent name.
            (
                "https://example.com/view?data=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghijk&mode=a",
                "https://example.com/view?mode=a",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(s(input).as_deref(), Some(expected), "{input}");
        }
    }

    #[test]
    fn oauth_code_takes_state_with_it() {
        assert_eq!(
            s("https://app.example.com/callback?code=SplxlOBeZQQYbYS6WxSbIA&state=xyz&lang=en")
                .as_deref(),
            Some("https://app.example.com/callback?lang=en")
        );
        // `state` alone is an ordinary parameter.
        assert_eq!(
            s("https://maps.example.com/?state=CA").as_deref(),
            Some("https://maps.example.com/?state=CA")
        );
        assert_eq!(
            s("https://app.example.com/#/callback?code=abc&state=xyz").as_deref(),
            Some("https://app.example.com/#/callback")
        );
    }

    #[test]
    fn presigned_urls_lose_their_signature() {
        let aws_v4 = "https://bucket.s3.amazonaws.com/report.pdf?response-content-disposition=inline\
            &X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20240101%2Fus-east-1%2Fs3%2Faws4_request\
            &X-Amz-Date=20240101T000000Z&X-Amz-Expires=3600&X-Amz-SignedHeaders=host\
            &X-Amz-Security-Token=FwoGZXIvYXdzEJr&X-Amz-Signature=fe5f80f77d5fa3beca038a248ff027d0445342fe2855ddc963176630326f1024";
        assert_eq!(
            s(aws_v4).as_deref(),
            Some("https://bucket.s3.amazonaws.com/report.pdf?response-content-disposition=inline")
        );
        let aws_v2 = "https://bucket.s3.amazonaws.com/a.png?AWSAccessKeyId=AKIAIOSFODNN7EXAMPLE&Expires=1700000000&Signature=vjbyPxybdZaNmGa%2ByT272YEAiv4%3D";
        assert_eq!(
            s(aws_v2).as_deref(),
            Some("https://bucket.s3.amazonaws.com/a.png")
        );
        let azure = "https://acct.blob.core.windows.net/c/b.txt?sv=2022-11-02&ss=b&srt=o&sp=r&se=2024-01-01T00:00:00Z&spr=https&sig=abc%3D";
        assert_eq!(
            s(azure).as_deref(),
            Some("https://acct.blob.core.windows.net/c/b.txt")
        );
        // `Expires` without a signature is ordinary.
        assert_eq!(
            s("https://example.com/?expires=2025-01-01").as_deref(),
            Some("https://example.com/?expires=2025-01-01")
        );
    }

    #[test]
    fn token_fragments_and_session_paths_are_removed() {
        assert_eq!(
            s("https://app.example.com/cb#access_token=abc&token_type=bearer&expires_in=3600")
                .as_deref(),
            Some("https://app.example.com/cb")
        );
        assert_eq!(
            s("https://app.example.com/#id_token=abc").as_deref(),
            Some("https://app.example.com/")
        );
        assert_eq!(
            s("https://app.example.com/page#token=abc").as_deref(),
            Some("https://app.example.com/page")
        );
        assert_eq!(
            s("https://shop.example.com/cart;jsessionid=1A2B3C4D?item=7").as_deref(),
            Some("https://shop.example.com/cart?item=7")
        );
        // A credential inside the path cannot be removed: the URL is dropped.
        assert_eq!(
            s(
                "https://example.com/magic/eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghijk"
            ),
            None
        );
    }

    #[test]
    fn unsafe_schemes_and_garbage_are_rejected() {
        for url in [
            "javascript:alert(1)",
            "JavaScript:alert(document.cookie)",
            "data:text/html;base64,PHNjcmlwdD4=",
            "blob:https://example.com/550e8400-e29b-41d4-a716-446655440000",
            "chrome-extension://abcdefghijklmnop/popup.html",
            "chrome://settings/passwords",
            "edge://settings",
            "about:blank",
            "view-source:https://example.com",
            "mailto:alice@example.com",
            "ws://example.com/socket",
            "ms-msdt:/id PCWDiagnostic",
            "not a url",
            "/relative/path",
            "",
        ] {
            assert_eq!(s(url), None, "{url}");
        }
    }

    #[test]
    fn long_urls_are_rejected() {
        let long = format!("https://example.com/?q={}", "a".repeat(5000));
        assert_eq!(s(&long), None);
        let huge = format!("https://example.com/?q={}", "a".repeat(100_000));
        assert_eq!(s(&huge), None);
        // Removing a long secret can bring a URL back under the limit.
        let secret = format!("https://example.com/?token={}&q=1", "a".repeat(5000));
        assert_eq!(s(&secret).as_deref(), Some("https://example.com/?q=1"));
        let exactly = format!("https://example.com/{}", "a".repeat(4096 - 20));
        assert_eq!(exactly.len(), 4096);
        assert_eq!(s(&exactly).as_deref(), Some(exactly.as_str()));
    }

    #[test]
    fn media_uris_go_through_a_whitelist() {
        assert_eq!(
            sanitize_media_uri("spotify:playlist:37i9dQZF1DXcBWIGoYBM5M").as_deref(),
            Some("spotify:playlist:37i9dQZF1DXcBWIGoYBM5M")
        );
        assert_eq!(
            sanitize_media_uri("https://open.spotify.com/playlist/37i9?si=abc123&utm=x").as_deref(),
            Some("https://open.spotify.com/playlist/37i9?si=abc123&utm=x")
        );
        assert_eq!(
            sanitize_media_uri("https://music.example.com/p?token=abc").as_deref(),
            Some("https://music.example.com/p")
        );
        assert_eq!(
            sanitize_media_uri("music://music.apple.com/us/album/123").as_deref(),
            Some("music://music.apple.com/us/album/123")
        );
        for uri in [
            "spotify:",
            "spotify:playlist:abc?token=x",
            "spotify:user:a b",
            "ms-msdt:/id PCWDiagnostic",
            "search-ms:query=x",
            "vscode://file/etc/passwd",
            "javascript:alert(1)",
        ] {
            assert_eq!(sanitize_media_uri(uri), None, "{uri}");
        }
    }

    #[test]
    fn secret_options_are_dropped_with_their_values() {
        assert_eq!(
            sanitize_arguments(&args(&[
                "--new-window",
                "--token=abc",
                "--password",
                "hunter2",
                "--profile-directory=Default",
                "--api-key",
                "k",
                "/home/alice/project",
            ])),
            args(&[
                "--new-window",
                "--profile-directory=Default",
                "/home/alice/project"
            ])
        );
        assert_eq!(
            sanitize_arguments(&args(&[
                "-Djavax.net.ssl.keyStorePassword=changeit",
                "/password:hunter2",
                "/user:alice",
                "-jar",
                "app.jar",
            ])),
            args(&["/user:alice", "-jar", "app.jar"])
        );
        // A secret option followed by another option has no value to drop.
        assert_eq!(
            sanitize_arguments(&args(&["--password", "--verbose", "x"])),
            args(&["--verbose", "x"])
        );
        // A secret option at the end.
        assert_eq!(
            sanitize_arguments(&args(&["--token"])),
            Vec::<String>::new()
        );
    }

    #[test]
    fn switches_and_file_references_are_kept() {
        let kept = args(&[
            "--password-store=basic",
            "--no-password",
            "--restore-last-session",
            "--key",
            "/home/alice/.ssh/id_ed25519",
            "--session=~/work.session",
            "--auth-server-allowlist=*.example.com",
            "--",
            "--token-looking-file.txt",
            "-5",
            "-",
        ]);
        assert_eq!(sanitize_arguments(&kept), kept);
    }

    #[test]
    fn argument_values_are_checked() {
        assert_eq!(
            sanitize_arguments(&args(&[
                "--proxy-server=http://user:pass@proxy.local:8080",
                "https://example.com/cb?access_token=abc&x=1",
                "https://example.com",
                "postgres://app:secret@db.local/shop",
                "javascript:alert(1)",
                "ghp_0123456789abcdefghijABCDEFGHIJ012345",
                "q7XvT2mR9kLp4ZsW8nBc3HdF6jGy1KtN",
                "880f130c5e469263039877524e2b9774",
                "61c54bbd-c2c6-5271-96e7-009a87ff44bf",
                "C:\\Users\\alice\\notes.txt",
                "--enable-features=OverlayScrollbar,WebUIDarkMode",
            ])),
            args(&[
                "--proxy-server=http://proxy.local:8080/",
                "https://example.com/cb?x=1",
                "https://example.com",
                "postgres://app@db.local/shop",
                "javascript:alert(1)",
                "61c54bbd-c2c6-5271-96e7-009a87ff44bf",
                "C:\\Users\\alice\\notes.txt",
                "--enable-features=OverlayScrollbar,WebUIDarkMode",
            ])
        );
        // After `--` values are still checked, options are not interpreted.
        assert_eq!(
            sanitize_arguments(&args(&[
                "--",
                "--token=abc",
                "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.abcdefghijklmnop"
            ])),
            args(&["--", "--token=abc"])
        );
        assert!(sanitize_arguments(&[]).is_empty());
    }

    fn tab(url: &str, title: &str) -> BrowserTab {
        BrowserTab {
            url: url.into(),
            title: title.into(),
            pinned: false,
        }
    }

    #[test]
    fn browser_state_is_sanitized() {
        let state = AppState::Browser {
            windows: vec![
                BrowserWindow {
                    tabs: vec![
                        tab("https://example.com/a", "A"),
                        tab("chrome://settings", "Settings"),
                        tab("https://example.com/b?token=x", &"T".repeat(2000)),
                    ],
                    active_tab: Some(1),
                    window_title: Some("Window\0".into()),
                },
                BrowserWindow {
                    tabs: vec![tab("about:blank", ""), tab("javascript:void(0)", "")],
                    active_tab: Some(0),
                    window_title: None,
                },
                BrowserWindow {
                    tabs: (0..600)
                        .map(|i| tab(&format!("https://example.com/{i}"), "x"))
                        .collect(),
                    active_tab: Some(599),
                    window_title: None,
                },
            ],
            profile: Some("Profile 1".into()),
        };
        let AppState::Browser { windows, profile } = sanitize_state(&state) else {
            panic!("expected browser state");
        };
        assert_eq!(profile.as_deref(), Some("Profile 1"));
        assert_eq!(
            windows.len(),
            2,
            "the window with only unsafe tabs is dropped"
        );
        let first = &windows[0];
        assert_eq!(first.tabs.len(), 2);
        assert_eq!(first.tabs[1].url, "https://example.com/b");
        assert_eq!(first.tabs[1].title.chars().count(), MAX_TEXT_LEN);
        // The active tab was dropped: the tab that took its place becomes active.
        assert_eq!(first.active_tab, Some(1));
        assert_eq!(first.window_title.as_deref(), Some("Window"));
        let second = &windows[1];
        assert_eq!(second.tabs.len(), MAX_TABS_PER_WINDOW);
        assert_eq!(
            second.active_tab,
            Some(u32::try_from(MAX_TABS_PER_WINDOW - 1).unwrap())
        );
    }

    #[test]
    fn terminal_commands_are_redacted_and_switched_off() {
        let state = AppState::Terminal {
            sessions: vec![
                TerminalSession {
                    working_directory: Some(PathBuf::from("/home/alice/code")),
                    profile: Some("Ubuntu".into()),
                    startup_command: Some("export GITHUB_TOKEN=ghp_abc && npm run dev".into()),
                    startup_command_approved: true,
                    window_title: Some("dev".into()),
                },
                TerminalSession {
                    working_directory: None,
                    profile: None,
                    startup_command: Some(format!("echo {}", "x".repeat(MAX_COMMAND_LEN))),
                    startup_command_approved: true,
                    window_title: None,
                },
                TerminalSession {
                    working_directory: None,
                    profile: None,
                    startup_command: Some("   ".into()),
                    startup_command_approved: false,
                    window_title: None,
                },
            ],
        };
        let AppState::Terminal { sessions } = sanitize_state(&state) else {
            panic!("expected terminal state");
        };
        assert_eq!(sessions.len(), 3);
        assert_eq!(
            sessions[0].startup_command.as_deref(),
            Some("export GITHUB_TOKEN=\"<redacted>\" && npm run dev")
        );
        assert_eq!(
            sessions[0].working_directory,
            Some(PathBuf::from("/home/alice/code"))
        );
        assert_eq!(sessions[0].profile.as_deref(), Some("Ubuntu"));
        assert_eq!(
            sessions[1].startup_command, None,
            "too long: dropped, not truncated"
        );
        assert_eq!(sessions[2].startup_command, None);
        assert!(sessions.iter().all(|s| !s.startup_command_approved));
    }

    #[test]
    fn other_states() {
        let custom = AppState::Custom {
            data: serde_json::json!({ "apiKey": "secret", "nested": { "token": "x" } }),
        };
        assert_eq!(sanitize_state(&custom), AppState::None);
        assert_eq!(sanitize_state(&AppState::None), AppState::None);

        let docs = AppState::Documents {
            documents: vec![DocumentRef {
                path: PathBuf::from("C:\\Users\\alice\\report.pdf"),
                page: Some(3),
            }],
        };
        assert_eq!(sanitize_state(&docs), docs);
        let folders = AppState::Folders {
            folders: vec![FolderRef {
                path: PathBuf::from("/home/alice/Downloads"),
                window_title: Some("Downloads".into()),
            }],
        };
        assert_eq!(sanitize_state(&folders), folders);
        let projects = AppState::Projects {
            projects: vec![ProjectRef {
                path: PathBuf::from("/home/alice/code/app.code-workspace"),
                is_workspace_file: true,
                window_title: None,
            }],
        };
        assert_eq!(sanitize_state(&projects), projects);

        let media = AppState::Media {
            uri: Some("ms-msdt:/id PCWDiagnostic".into()),
            label: Some("Focus".into()),
        };
        assert_eq!(
            sanitize_state(&media),
            AppState::Media {
                uri: None,
                label: Some("Focus".into())
            }
        );
    }

    #[test]
    fn urls_inside_titles_are_sanitized() {
        assert_eq!(
            clean_title(
                "app.example.com/cb?access_token=abc123&lang=en - Google Chrome",
                MAX_TEXT_LEN
            ),
            "app.example.com/cb?lang=en - Google Chrome"
        );
        assert_eq!(
            clean_title(
                "Clone https://alice:hunter2@git.example.com/repo",
                MAX_TEXT_LEN
            ),
            "Clone https://git.example.com/repo"
        );
        assert_eq!(
            clean_title("psql postgres://app:secret@db/shop\0", MAX_TEXT_LEN),
            "psql postgres://app@db/shop"
        );
        for title in [
            "serde - Rust",
            "x = y + 1 — notes.txt",
            "Inbox (3) - alice@example.com - Mail",
            "chrome://settings",
            "https://docs.rs/serde/latest/serde/",
        ] {
            assert_eq!(clean_title(title, MAX_TEXT_LEN), title);
        }
        // Truncation happens after sanitizing.
        let long = format!("{} https://example.com/?token=abc", "a".repeat(10));
        assert_eq!(clean_title(&long, 12), "aaaaaaaaaa h");

        let state = AppState::Browser {
            windows: vec![BrowserWindow {
                tabs: vec![BrowserTab {
                    url: "https://example.com/reset?token=abcdef".into(),
                    title: "example.com/reset?token=abcdef".into(),
                    pinned: true,
                }],
                active_tab: None,
                window_title: Some("example.com/reset?token=abcdef - Chromium".into()),
            }],
            profile: None,
        };
        let json = serde_json::to_string(&sanitize_state(&state)).unwrap();
        assert!(!json.contains("abcdef"), "{json}");
    }

    #[test]
    fn disguised_parameter_names_are_caught() {
        for (input, expected) in [
            // Encoded once (decoded by the parser) and twice.
            (
                "https://example.com/?%74oken=abc&q=1",
                "https://example.com/?q=1",
            ),
            (
                "https://example.com/?%2574oken=abc&q=1",
                "https://example.com/?q=1",
            ),
            (
                "https://example.com/?access%5Ftoken=abc&q=1",
                "https://example.com/?q=1",
            ),
            // Odd capitalisation splits differently at case changes.
            (
                "https://example.com/?ToKeN=abc&q=1",
                "https://example.com/?q=1",
            ),
            (
                "https://example.com/?toKen_x=abc&q=1",
                "https://example.com/?q=1",
            ),
            (
                "https://example.com/?ACCESS_TOKEN=abc&q=1",
                "https://example.com/?q=1",
            ),
            // A bare credential as a parameter name.
            (
                "https://example.com/?ghp_0123456789abcdefghijABCDEFGHIJ012345&q=1",
                "https://example.com/?q=1",
            ),
            // `;` as a separator inside a pair: the whole pair goes.
            (
                "https://example.com/?a=1;token=abc&q=1",
                "https://example.com/?q=1",
            ),
            (
                "https://example.com/?DB_PASS=x&q=1",
                "https://example.com/?q=1",
            ),
            (
                "https://example.com/?licensekey=ABCD&q=1",
                "https://example.com/?q=1",
            ),
            (
                "https://zoom.us/j/123456789?pwd=AbCdEfGh1234",
                "https://zoom.us/j/123456789",
            ),
        ] {
            assert_eq!(s(input).as_deref(), Some(expected), "{input}");
        }
        // Ordinary text that merely mentions secrets is kept.
        let search = "https://example.com/search?q=password+reset+token";
        assert_eq!(s(search).as_deref(), Some(search));
    }

    #[test]
    fn nested_urls_with_secrets_take_their_parameter_with_them() {
        for input in [
            "https://example.com/?next=https://user:pass@other.com/",
            "https://example.com/?next=https%3A%2F%2Fuser%3Apass%40other.com%2F",
            "https://example.com/?redirect_uri=https%3A%2F%2Fapp%2Fcb%3Fcode%3Dabc%26state%3Dx",
            "https://example.com/?params=token%3Dabc%26x%3D1",
            "https://example.com/?u=https%253A%252F%252Fh%252F%253Faccess_token%253Dx",
            "https://example.com/?db=postgres%3A%2F%2Fapp%3Asecret%40db%2Fshop",
        ] {
            assert_eq!(s(input).as_deref(), Some("https://example.com/"), "{input}");
        }
        let safe = "https://example.com/?url=https%3A%2F%2Fexample.org%2F%3Fq%3D1";
        assert_eq!(s(safe).as_deref(), Some(safe));
        let relative = "https://example.com/login?next=/dashboard";
        assert_eq!(s(relative).as_deref(), Some(relative));
    }

    #[test]
    fn tokens_in_paths_and_fragments_are_removed() {
        // One-time tokens after a sensitive word drop the URL (they cannot be cut out).
        for input in [
            "https://example.com/reset-password/8f14e45fceea167a5a36dedd4bea2543",
            "https://example.com/auth/magic/AbCdEf0123456789xyzQRSTuv",
            "https://example.com/invite/550e8400-e29b-41d4-a716-446655440000",
            "https://example.com/api/keys/0023e1fdbd363b88a3dca4cd0cdb4b0fbdb056d1",
            "https://web.archive.org/web/2020/https://user:pass@example.com/",
            "https://api.telegram.org/bot123456789:AAEhBP0av28Hjlk3LnXm4b2Zqz0R-xyzABC/getMe",
        ] {
            assert_eq!(s(input), None, "{input}");
        }
        // In fragments, the fragment goes.
        for (input, expected) in [
            (
                "https://example.com/#/reset/eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.abcdefghijk",
                "https://example.com/",
            ),
            (
                "https://example.com/#/reset-password/8f14e45fceea167a5a36dedd4bea2543",
                "https://example.com/",
            ),
            (
                "https://example.com/#https://u:p@host/",
                "https://example.com/",
            ),
            (
                "https://example.com/?x=1#/cb?to=https://u:p@h/",
                "https://example.com/?x=1",
            ),
            (
                "https://example.com/cb#AcCeSs_ToKeN=abc",
                "https://example.com/cb",
            ),
            (
                "https://example.com/cb#accessToken%3Dabc",
                "https://example.com/cb",
            ),
        ] {
            assert_eq!(s(input).as_deref(), Some(expected), "{input}");
        }
        // Random-looking ids without such context are ordinary.
        for url in [
            "https://github.com/rust-lang/rust/commit/0023e1fdbd363b88a3dca4cd0cdb4b0fbdb056d1",
            "https://docs.google.com/document/d/1BxiMVs0XRA5nFMdKvBdBZjgmUUqptlbs74OgvE2upms/edit",
            "https://example.com/#/settings/profile",
            "https://example.com/@user/https://x.com/@y",
            "https://example.com/auth/callback",
        ] {
            assert_eq!(s(url).as_deref(), Some(url), "{url}");
        }
    }

    #[test]
    fn webhook_endpoints_are_dropped() {
        for url in [
            concat!("https://hooks.slack.com/services/", "T00000000/B00000000/XXXXXXXXXXXXXXXXXXXXXXXX"),
            "https://discord.com/api/webhooks/123456789012345678/AbCdEfGhIjKlMnOpQrStUvWx",
            "https://contoso.webhook.office.com/webhookb2/abc@def/IncomingWebhook/123/456",
            "https://hooks.zapier.com/hooks/catch/123/abc/",
        ] {
            assert_eq!(s(url), None, "{url}");
        }
        assert!(s("https://discord.com/channels/123/456").is_some());
    }

    #[test]
    fn more_presigned_formats_lose_their_signature() {
        let gcs = "https://storage.googleapis.com/b/o?X-Goog-Algorithm=GOOG4-RSA-SHA256\
            &X-Goog-Credential=x&X-Goog-Date=1&X-Goog-Expires=900&X-Goog-SignedHeaders=host\
            &X-Goog-Signature=abc";
        assert_eq!(
            s(gcs).as_deref(),
            Some("https://storage.googleapis.com/b/o")
        );
        let cloudfront =
            "https://d111.cloudfront.net/image.jpg?Expires=1&Signature=abc&Key-Pair-Id=K2&w=5";
        assert_eq!(
            s(cloudfront).as_deref(),
            Some("https://d111.cloudfront.net/image.jpg?w=5")
        );
        let lower = "https://b.s3.amazonaws.com/k?x-amz-signature=abc&x-amz-date=1&a=b";
        assert_eq!(
            s(lower).as_deref(),
            Some("https://b.s3.amazonaws.com/k?a=b")
        );
    }

    #[test]
    fn argument_edge_cases() {
        // `name:password` for user options; uid:gid pairs stay.
        assert_eq!(
            sanitize_arguments(&args(&[
                "--user=alice:hunter2",
                "--user",
                "bob:pw",
                "--user=1000:1000",
                "--user",
                "carol",
            ])),
            args(&["--user=1000:1000", "--user", "carol"])
        );
        // A base64 token starting with `/` is not a file reference.
        assert_eq!(
            sanitize_arguments(&args(&[
                "--token=/abc+def/ghiJKLmnoPQRstu0123456789abc==",
                "--secret",
                "/q7XvT2mR9kLp4ZsW8nBc3HdF6jGy1KtN",
                "--key",
                "/home/a/.ssh/id_ed25519",
            ])),
            args(&["--key", "/home/a/.ssh/id_ed25519"])
        );
        // Short words that are secrets at the end of a name.
        assert_eq!(
            sanitize_arguments(&args(&[
                "--DB-PASS=x",
                "--admin_pw",
                "y",
                "--pin",
                "1234",
                "--pass-through",
                "z",
            ])),
            args(&["--pass-through", "z"])
        );
        // Found by fuzzing: a secret option and its value stay or go together.
        let pair = args(&[
            "--authorization",
            "\\\\?\\ghp_0123456789abcdefghijABCDEFGHIJ012345,",
        ]);
        assert!(sanitize_arguments(&pair).is_empty());
        // Found by fuzzing: removing an unparseable password leaves a URL that parses; the
        // result must be stable, and the credentials in the fragment's nested URL go too.
        let tricky = "http://u:p@\\\\srv\\share{@9&state=/|€]?#9spotify:\0/home/a9http://u:p@";
        let once = sanitize_arguments(&args(&[tricky]));
        assert_eq!(once, args(&["http://9&state=/|%E2%82%AC]?"]));
        assert_eq!(sanitize_arguments(&once), once);
    }

    #[test]
    fn terminal_titles_are_redacted_like_commands() {
        let state = AppState::Terminal {
            sessions: vec![TerminalSession {
                working_directory: None,
                profile: None,
                startup_command: None,
                startup_command_approved: false,
                window_title: Some("mysql -u root -phunter2 — Terminal".into()),
            }],
        };
        let AppState::Terminal { sessions } = sanitize_state(&state) else {
            panic!("expected terminal state");
        };
        assert_eq!(
            sessions[0].window_title.as_deref(),
            Some("mysql -u root -p\"<redacted>\" — Terminal")
        );
        assert_eq!(clean_terminal_title("vim notes.txt", 1024), "vim notes.txt");
        assert_eq!(
            clean_terminal_title("alice@host: ~/code", 1024),
            "alice@host: ~/code"
        );
    }

    #[test]
    fn sanitizing_is_idempotent() {
        let state = AppState::Browser {
            windows: vec![BrowserWindow {
                tabs: vec![
                    tab(
                        "https://u:p@example.com/?code=1&state=2&q=3#access_token=x",
                        "x",
                    ),
                    tab("https://example.com/a;jsessionid=1", "y"),
                ],
                active_tab: Some(0),
                window_title: None,
            }],
            profile: None,
        };
        let once = sanitize_state(&state);
        assert_eq!(sanitize_state(&once), once);
        let list = args(&["--token", "x", "https://u:p@h/?token=1", "--flag"]);
        let once = sanitize_arguments(&list);
        assert_eq!(sanitize_arguments(&once), once);

        // Found by fuzzing: the part before `?` in a fragment carries parameters too.
        let url = "file:///#Bearer &state=x?/home/a--password.Authorization:";
        let once = s(url).unwrap();
        assert_eq!(once, "file:///#state=x");
        assert_eq!(s(&once).as_deref(), Some(once.as_str()));
        assert_eq!(
            s("https://example.com/#?a=1&token_x=2").as_deref(),
            Some("https://example.com/#?a=1")
        );
    }
}
