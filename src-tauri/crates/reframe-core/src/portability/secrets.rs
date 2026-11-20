//! Recognising credentials: secret-looking *names* (URL parameters, command-line options,
//! environment variables, HTTP headers) and credential-shaped *values*.
//!
//! The name rules are intentionally broad — dropping a harmless parameter costs little,
//! exporting a token is a leak — while the value rules are conservative so that ordinary
//! paths, identifiers, UUIDs and words survive.
//!
//! Limitations: this is heuristic. A secret stored under an innocent name *and* with an
//! ordinary-looking value (a short password, a dictionary word) cannot be recognised, and
//! roughly one in four random 32+ character tokens without a known prefix slips past the
//! value heuristics (they are tuned to avoid eating CamelCase identifiers and paths).

/// How a name is used. Command-line options get extra exceptions for boolean switches such as
/// `--no-password` or `--disable-auth`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NameKind {
    /// URL query, fragment or matrix parameter; HTTP header.
    Parameter,
    /// Command-line option (`--api-key`, `-Dfoo.password`, `/password:`).
    Flag,
    /// Environment or shell variable (`GITHUB_TOKEN`).
    Variable,
}

/// Complete names (separators removed, lower-cased) that always hold secrets.
const SECRET_NAMES: &[&str] = &[
    "code",
    "sid",
    "sig",
    "sign",
    "pass",
    "pw",
    "key",
    "auth",
    "session",
    "jwt",
    "otp",
    "hmac",
    "phpsessid",
    "jsessionid",
    "aspnetsessionid",
    "connectsid",
    "awsaccesskeyid",
    "googleaccessid",
    "samlresponse",
    "samlart",
    "sshpass",
    "passin",
    "passout",
];

/// Words that make a name secret wherever they appear (`client_secret`, `X-Api-Key`,
/// `X-Amz-Security-Token`, `AWSAccessKeyId`).
const SECRET_WORDS: &[&str] = &[
    "token",
    "tokens",
    "password",
    "passwords",
    "passwd",
    "pwd",
    "passphrase",
    "passcode",
    "secret",
    "secrets",
    "signature",
    "sig",
    "jwt",
    "apikey",
    "apikeys",
    "credential",
    "credentials",
    "creds",
    "session",
    "sessionid",
    "sessid",
    "otp",
    "totp",
    "hotp",
    "bearer",
    "privatekey",
    "secretkey",
    "accesskey",
    "authkey",
    "auth",
    "authorization",
    "key",
    "verifier",
    "cookie",
    "cookies",
    "csrf",
    "xsrf",
    "hmac",
    "jsessionid",
    "phpsessid",
];

/// Single words (or whole names) that are or end with one of these are secret too:
/// `accesstoken`, `clientsecret`, `newpassword`, `xamzsignature`, `licensekey`.
const SECRET_SUFFIXES: &[&str] = &[
    "token",
    "password",
    "passwd",
    "pwd",
    "secret",
    "apikey",
    "sessionid",
    "sessid",
    "signature",
    "credential",
    "credentials",
    "privatekey",
    "secretkey",
    "accesskey",
    "authkey",
    "licensekey",
    "masterkey",
    "signingkey",
    "encryptionkey",
    "subscriptionkey",
    "sessionkey",
];

/// Short words that hold a secret when they end a name (`DB_PASS`, `--admin-pw`, `SIM_PIN`)
/// but are ordinary elsewhere (`pass_through`, `PW_TEST_REPORT`, `pin_memory`).
const SECRET_LAST_WORDS: &[&str] = &["pass", "pw", "pin"];

/// A name whose *last* word is one of these describes or configures a secret rather than
/// holding one: `password_file`, `token_type`, `auth_url`, `session_timeout`,
/// `--password-store`, `--auth-server-allowlist`.
const DESCRIPTIVE_SUFFIXES: &[&str] = &[
    "file",
    "files",
    "path",
    "paths",
    "dir",
    "directory",
    "folder",
    "store",
    "storage",
    "stdin",
    "env",
    "prompt",
    "name",
    "names",
    "length",
    "len",
    "size",
    "type",
    "types",
    "mode",
    "format",
    "strength",
    "required",
    "manager",
    "helper",
    "command",
    "cmd",
    "url",
    "uri",
    "endpoint",
    "server",
    "host",
    "port",
    "realm",
    "domain",
    "scheme",
    "schemes",
    "method",
    "methods",
    "provider",
    "providers",
    "expiry",
    "expires",
    "expiration",
    "ttl",
    "timeout",
    "lifetime",
    "policy",
    "hint",
    "whitelist",
    "allowlist",
    "blacklist",
    "blocklist",
    "delegate",
    "negotiate",
    "enabled",
    "disabled",
    "count",
    "limit",
    "version",
    "delay",
    "interval",
    "layout",
    "binding",
    "bindings",
    "through",
    "thru",
];

/// Command-line options starting with one of these words are boolean switches about a secret,
/// not carriers of one: `--no-password`, `--disable-auth`, `--restore-last-session`.
const SWITCH_PREFIXES: &[&str] = &[
    "no", "disable", "disabled", "skip", "without", "ask", "prompt", "require", "allow", "enable",
    "use", "save", "remember", "show", "hide", "forget", "ignore", "restore", "clear", "keep",
    "persist",
];

/// Words that turn a `code` parameter into a one-time credential: `verification_code`,
/// `auth_code`, `otp_code`, `device_code`.
const AUTH_CODE_COMPANIONS: &[&str] = &[
    "auth",
    "authorization",
    "oauth",
    "verification",
    "verify",
    "access",
    "security",
    "otp",
    "mfa",
    "2fa",
    "twofa",
    "tfa",
    "sms",
    "reset",
    "activation",
    "activate",
    "device",
    "login",
    "signin",
    "confirm",
    "confirmation",
    "recovery",
    "backup",
    "magic",
    "passcode",
];

/// Split an identifier into lower-cased words at separators and case changes:
/// `X-Amz-Signature` → `[x, amz, signature]`, `accessToken` → `[access, token]`,
/// `AWSAccessKeyId` → `[aws, access, key, id]`, `GITHUB_TOKEN` → `[github, token]`.
pub(crate) fn name_words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    for part in name.split(|c: char| !c.is_alphanumeric()) {
        let chars: Vec<char> = part.chars().collect();
        let mut start = 0;
        for i in 1..chars.len() {
            let (prev, cur) = (chars[i - 1], chars[i]);
            let next_is_lower = chars.get(i + 1).is_some_and(|c| c.is_lowercase());
            let boundary = ((prev.is_lowercase() || prev.is_ascii_digit()) && cur.is_uppercase())
                || (prev.is_uppercase() && cur.is_uppercase() && next_is_lower);
            if boundary {
                words.push(chars[start..i].iter().collect::<String>().to_lowercase());
                start = i;
            }
        }
        if start < chars.len() {
            words.push(chars[start..].iter().collect::<String>().to_lowercase());
        }
    }
    words
}

/// Whether a parameter, option or variable called `name` probably holds a secret.
///
/// The name is split into words twice — at separators and case changes (`accessToken`), and at
/// separators only (`toKen_x`, `GiThUb_ToKeN`) — and is secret when either split says so.
pub(crate) fn is_secret_name(name: &str, kind: NameKind) -> bool {
    let words = name_words(name);
    if is_secret_words(&words, kind) {
        return true;
    }
    let plain = separator_words(name);
    plain != words && is_secret_words(&plain, kind)
}

/// Lower-cased words between separators, ignoring capitalisation.
fn separator_words(name: &str) -> Vec<String> {
    name.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn is_secret_words(words: &[String], kind: NameKind) -> bool {
    let Some(last) = words.last() else {
        return false;
    };
    let joined: String = words.concat();
    if SECRET_NAMES.contains(&joined.as_str()) {
        return true;
    }
    if DESCRIPTIVE_SUFFIXES.contains(&last.as_str()) {
        return false;
    }
    if kind == NameKind::Flag && words.len() > 1 && SWITCH_PREFIXES.contains(&words[0].as_str()) {
        return false;
    }
    let ends_with_secret = |w: &str| SECRET_SUFFIXES.iter().any(|s| w.ends_with(s));
    SECRET_LAST_WORDS.contains(&last.as_str())
        || words
            .iter()
            .any(|w| SECRET_WORDS.contains(&w.as_str()) || ends_with_secret(w))
        || SECRET_WORDS.contains(&joined.as_str())
        || ends_with_secret(&joined)
        || is_auth_code_words(words)
}

/// Whether `name` is an OAuth authorization code or another one-time code parameter
/// (`code`, `auth_code`, `verificationCode`). Its companion `state` parameter is removed too.
pub(crate) fn is_auth_code_name(name: &str) -> bool {
    is_auth_code_words(&name_words(name))
}

fn is_auth_code_words(words: &[String]) -> bool {
    if words.concat() == "code" {
        return true;
    }
    let has_code = words.iter().any(|w| w == "code");
    let has_companion = words
        .iter()
        .any(|w| AUTH_CODE_COMPANIONS.contains(&w.as_str()));
    let glued = words.iter().any(|w| {
        w.len() > 4
            && w.ends_with("code")
            && AUTH_CODE_COMPANIONS
                .iter()
                .any(|c| w.len() == c.len() + 4 && w.starts_with(c))
    });
    (has_code && has_companion) || glued
}

/// Known token formats with their minimum total length.
const KNOWN_TOKEN_PREFIXES: &[(&str, usize)] = &[
    // GitHub
    ("ghp_", 40),
    ("gho_", 40),
    ("ghu_", 40),
    ("ghs_", 40),
    ("ghr_", 40),
    ("github_pat_", 40),
    // GitLab
    ("glpat-", 26),
    ("gldt-", 25),
    ("glrt-", 25),
    ("glptt-", 26),
    // Slack
    ("xoxb-", 24),
    ("xoxp-", 24),
    ("xoxa-", 24),
    ("xoxr-", 24),
    ("xoxs-", 24),
    ("xapp-", 24),
    // Stripe
    ("sk_live_", 30),
    ("sk_test_", 30),
    ("rk_live_", 30),
    ("rk_test_", 30),
    ("whsec_", 30),
    // OpenAI / Anthropic style secret keys
    ("sk-", 40),
    // Google API keys and OAuth access tokens
    ("AIza", 39),
    ("ya29.", 40),
    // Package registries and hubs
    ("npm_", 40),
    ("pypi-", 80),
    ("hf_", 37),
    // Shopify
    ("shpat_", 38),
    ("shpss_", 38),
    ("shpca_", 38),
    ("shppa_", 38),
    // DigitalOcean
    ("dop_v1_", 71),
    ("doo_v1_", 71),
    // Grafana service accounts
    ("glsa_", 40),
    // SendGrid
    ("SG.", 60),
    // age secret keys
    ("AGE-SECRET-KEY-", 60),
    // HashiCorp Vault (service, batch and recovery tokens)
    ("hvs.", 40),
    ("hvb.", 40),
    ("hvr.", 40),
    // Docker Hub, Sentry, Grafana Cloud, Figma, Linear, Netlify, RubyGems, Contentful
    ("dckr_pat_", 30),
    ("sntrys_", 40),
    ("glc_", 40),
    ("figd_", 40),
    ("lin_api_", 40),
    ("nfp_", 40),
    ("rubygems_", 40),
    ("CFPAT-", 40),
    // Atlassian API tokens
    ("ATATT3", 100),
    // Slack refresh tokens, Square
    ("xoxe.xoxp-", 24),
    ("xoxe-", 24),
    ("sq0atp-", 29),
    ("sq0csp-", 40),
];

/// AWS access key id prefixes (`AKIA…`, `ASIA…`: 4 + 16 upper-case alphanumerics).
const AWS_KEY_ID_PREFIXES: &[&str] = &[
    "AKIA", "ASIA", "ABIA", "ACCA", "AGPA", "AIDA", "AIPA", "ANPA", "ANVA", "APKA", "AROA", "ASCA",
];

/// High-confidence credential formats: provider token prefixes (GitHub, GitLab, Slack, Stripe,
/// Google, AWS key ids...), JSON Web Tokens and PEM private keys. Safe to apply to any value,
/// including URL parameters with innocent names.
pub(crate) fn is_known_credential_format(value: &str) -> bool {
    let v = value.trim();
    if v.len() < 20 {
        return false;
    }
    if v.contains("PRIVATE KEY-----")
        || is_jwt(v)
        || is_aws_access_key_id(v)
        || is_telegram_bot_token(v)
    {
        return true;
    }
    KNOWN_TOKEN_PREFIXES.iter().any(|(prefix, min_len)| {
        v.len() >= *min_len
            && v.starts_with(prefix)
            && v[prefix.len()..]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    })
}

fn is_jwt(v: &str) -> bool {
    if v.len() < 30 || !v.starts_with("eyJ") {
        return false;
    }
    let parts: Vec<&str> = v.split('.').collect();
    (3..=5).contains(&parts.len())
        && !parts[1].is_empty()
        && parts.iter().all(|p| {
            p.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '='))
        })
}

fn is_aws_access_key_id(v: &str) -> bool {
    v.len() == 20
        && AWS_KEY_ID_PREFIXES.iter().any(|p| v.starts_with(p))
        && v.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

/// `123456789:AAE…` (35 characters after the colon), optionally prefixed with `bot` as in Bot
/// API URLs (`/bot123456789:AAE…/getMe`).
fn is_telegram_bot_token(v: &str) -> bool {
    let v = v.strip_prefix("bot").unwrap_or(v);
    let Some((id, secret)) = v.split_once(':') else {
        return false;
    };
    (6..=12).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_digit())
        && secret.len() == 35
        && secret
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

pub(crate) fn is_uuid(v: &str) -> bool {
    let bytes = v.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => *b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

/// [`is_known_credential_format`] plus conservative shape heuristics for long random-looking
/// tokens:
/// - hexadecimal strings of 32+ characters that mix digits and letters (API keys, secrets);
/// - base64/base64url strings of 32+ characters mixing upper case, lower case and digits whose
///   statistics look random: under 25% vowels among the letters (random text has ≈19%,
///   identifiers and paths made of words ≈35-40%) with digits or capitals scattered around.
///
/// Paths (`/…`, `~…`, `.…`), options (`-…`), UUIDs and anything with spaces or other
/// punctuation are never considered credentials by the shape rules.
pub(crate) fn looks_like_credential(value: &str) -> bool {
    let v = value.trim();
    if is_known_credential_format(v) {
        return true;
    }
    if v.len() < 32 || v.starts_with(['-', '/', '.', '~']) || is_uuid(v) {
        return false;
    }
    if !v
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'_' | b'-'))
    {
        return false;
    }
    let has_digit = v.bytes().any(|b| b.is_ascii_digit());
    if v.bytes().all(|b| b.is_ascii_hexdigit()) {
        return has_digit && v.bytes().any(|b| b.is_ascii_alphabetic());
    }
    let has_lower = v.bytes().any(|b| b.is_ascii_lowercase());
    let has_upper = v.bytes().any(|b| b.is_ascii_uppercase());
    if !(has_digit && has_lower && has_upper) {
        return false;
    }
    let letters = v.bytes().filter(u8::is_ascii_alphabetic).count();
    let vowels = v.bytes().filter(|b| b"aeiouAEIOU".contains(b)).count();
    let bytes = v.as_bytes();
    let digit_runs = bytes
        .iter()
        .enumerate()
        .filter(|(i, b)| b.is_ascii_digit() && (*i == 0 || !bytes[i - 1].is_ascii_digit()))
        .count();
    let upper_pairs = bytes
        .windows(2)
        .filter(|w| w[0].is_ascii_uppercase() && w[1].is_ascii_uppercase())
        .count();
    vowels * 4 < letters && (upper_pairs >= 2 || digit_runs >= 2)
}

/// Replace credentials in a known format ([`is_known_credential_format`]) wherever they appear
/// in `text` — inside a header line, after a colon, in the middle of a sentence. A PEM private
/// key anywhere redacts the whole text. `None` when nothing was found.
pub(crate) fn scrub_known_credentials(text: &str) -> Option<String> {
    const REDACTED: &str = "<redacted>";
    if text.contains("PRIVATE KEY-----") {
        return (text != REDACTED).then(|| REDACTED.to_string());
    }
    let is_token_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':');
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    let mut rest = text;
    while let Some(start) = rest.find(is_token_char) {
        out.push_str(&rest[..start]);
        let run_len = rest[start..]
            .find(|c: char| !is_token_char(c))
            .unwrap_or(rest.len() - start);
        let run = &rest[start..start + run_len];
        if is_known_credential_format(run) {
            out.push_str(REDACTED);
            changed = true;
        } else if run.contains(':') && run.split(':').any(is_known_credential_format) {
            // `Authorization:ghp_…`, `user:ghp_…`
            let pieces: Vec<&str> = run
                .split(':')
                .map(|piece| {
                    if is_known_credential_format(piece) {
                        REDACTED
                    } else {
                        piece
                    }
                })
                .collect();
            out.push_str(&pieces.join(":"));
            changed = true;
        } else {
            out.push_str(run);
        }
        rest = &rest[start + run_len..];
    }
    out.push_str(rest);
    changed.then_some(out)
}

/// A value that is a token *given its context* — a URL path segment after `reset`, `verify`,
/// `webhooks`... where a random-looking value is almost certainly a one-time credential:
/// [`looks_like_credential`], a UUID, or a run of 20+ letters and digits that mixes both.
/// Too eager for values without such context (it would eat commit hashes and document ids).
pub(crate) fn looks_like_token_in_context(value: &str) -> bool {
    let v = value.trim();
    if looks_like_credential(v) || is_uuid(v) {
        return true;
    }
    let token_chars = v
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'~' | b'+' | b'='));
    let longest_run = v
        .split(|c: char| !c.is_ascii_alphanumeric())
        .map(str::len)
        .max()
        .unwrap_or(0);
    token_chars
        && longest_run >= 20
        && v.bytes().any(|b| b.is_ascii_digit())
        && v.bytes().any(|b| b.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_names_into_words() {
        assert_eq!(name_words("X-Amz-Signature"), ["x", "amz", "signature"]);
        assert_eq!(name_words("accessToken"), ["access", "token"]);
        assert_eq!(name_words("AWSAccessKeyId"), ["aws", "access", "key", "id"]);
        assert_eq!(name_words("GITHUB_TOKEN"), ["github", "token"]);
        assert_eq!(name_words("APIKey"), ["api", "key"]);
        assert_eq!(name_words("oauth2Token"), ["oauth2", "token"]);
        assert_eq!(name_words("JSESSIONID"), ["jsessionid"]);
        assert_eq!(name_words("--api-key"), ["api", "key"]);
        assert_eq!(name_words("user[password]"), ["user", "password"]);
        assert!(name_words("").is_empty());
        assert!(name_words("--").is_empty());
    }

    #[test]
    fn secret_parameter_names() {
        let secret = [
            "token",
            "access_token",
            "refresh_token",
            "id_token",
            "auth",
            "Authorization",
            "code",
            "session",
            "sessionid",
            "sid",
            "key",
            "apikey",
            "api_key",
            "x-api-key",
            "apiKey",
            "password",
            "passwd",
            "pwd",
            "secret",
            "client_secret",
            "signature",
            "sig",
            "jwt",
            "otp",
            "X-Amz-Signature",
            "X-Amz-Credential",
            "X-Amz-Security-Token",
            "AWSAccessKeyId",
            "Signature",
            "accesstoken",
            "authToken",
            "my_token",
            "oauth_token",
            "oauth_verifier",
            "code_verifier",
            "verification_code",
            "authcode",
            "JSESSIONID",
            "PHPSESSID",
            "csrf_token",
            "_csrf",
            "user[password]",
            "pass_word",
            "subscription-key",
        ];
        for name in secret {
            assert!(
                is_secret_name(name, NameKind::Parameter),
                "{name} should be secret"
            );
        }
        let harmless = [
            "q",
            "page",
            "author",
            "authuser",
            "keyword",
            "keywords",
            "monkey",
            "sourceid",
            "state",
            "token_type",
            "access_type",
            "response_type",
            "redirect_uri",
            "client_id",
            "code_challenge",
            "country_code",
            "zip_code",
            "promo_code",
            "expires",
            "X-Amz-Expires",
            "X-Amz-Date",
            "lang",
            "utm_source",
            "v",
            "t",
            "sessions",
            "passwordless",
            "password_reset_url",
        ];
        for name in harmless {
            assert!(
                !is_secret_name(name, NameKind::Parameter),
                "{name} should not be secret"
            );
        }
    }

    #[test]
    fn flag_names_allow_boolean_switches_and_descriptions() {
        for flag in [
            "token",
            "password",
            "api-key",
            "apiKey",
            "client-secret",
            "auth-token",
            "Djavax.net.ssl.keyStorePassword",
            "session",
            "pw",
        ] {
            assert!(is_secret_name(flag, NameKind::Flag), "{flag}");
        }
        for flag in [
            "password-store",
            "password-stdin",
            "password-file",
            "token-file",
            "no-password",
            "disable-auth",
            "restore-last-session",
            "auth-server-allowlist",
            "auth-schemes",
            "new-window",
            "profile-directory",
            "user-data-dir",
        ] {
            assert!(!is_secret_name(flag, NameKind::Flag), "{flag}");
        }
        // The boolean exception only applies to options.
        assert!(is_secret_name("no_auth", NameKind::Parameter));
    }

    #[test]
    fn unusual_capitalisation_and_short_suffixes() {
        for name in [
            "toKen_x",
            "GiThUb_ToKeN",
            "DB_PASS",
            "MYSQL_ROOT_PASS",
            "admin_pw",
            "SIM_PIN",
            "SSHPASS",
            "passin",
            "licensekey",
            "masterKey",
            "mysqlpwd",
        ] {
            assert!(is_secret_name(name, NameKind::Variable), "{name}");
        }
        for name in [
            "pass_through",
            "passthrough",
            "PW_TEST_HTML_REPORT_OPEN",
            "pin_memory",
            "bypass",
            "compass",
            "keyboard_layout",
        ] {
            assert!(!is_secret_name(name, NameKind::Variable), "{name}");
        }
    }

    #[test]
    fn tokens_in_context_are_recognised() {
        for value in [
            "8f14e45fceea167a5a36dedd4bea2543",
            "550e8400-e29b-41d4-a716-446655440000",
            "AbCdEf0123456789xyzQRSTuv",
            "XXXXXXXXXXXX0XXXXXXXXXXX",
        ] {
            assert!(looks_like_token_in_context(value), "{value}");
        }
        for value in [
            "email-confirmation-step2",
            "callback",
            "v1",
            "12345678901234567890",
        ] {
            assert!(!looks_like_token_in_context(value), "{value}");
        }
        assert!(is_known_credential_format(
            "bot123456789:AAEhBP0av28Hjlk3LnXm4b2Zqz0R-xyzABC"
        ));
        assert!(is_known_credential_format(
            "hvs.CAESIJlU0eDpnU2pM1hR7dxh9e2fEXAMPLEexampleEXAMPLE"
        ));
        assert!(!is_known_credential_format("123456789:short"));
    }

    #[test]
    fn auth_codes_need_a_companion_word() {
        assert!(is_auth_code_name("code"));
        assert!(is_auth_code_name("auth_code"));
        assert!(is_auth_code_name("verificationCode"));
        assert!(is_auth_code_name("otpcode"));
        assert!(!is_auth_code_name("country_code"));
        assert!(!is_auth_code_name("postcode"));
        assert!(!is_auth_code_name("encode"));
    }

    #[test]
    fn known_credential_formats() {
        let known = [
            concat!("gh", "p_0123456789abcdefghijABCDEFGHIJ012345"),
            concat!("github_", "pat_11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyz"),
            concat!("glp", "at-0123456789abcdefghij"),
            concat!("xo", "xb-123456789012-1234567890123-AbCdEfGhIjKlMnOp"),
            concat!("sk_", "live_0123456789abcdefghijklmnop"),
            concat!("sk-", "proj-0123456789abcdefghijklmnopqrstuvwxyzABCD"),
            concat!("AI", "zaSyA0123456789abcdefghijklmnopqrstuv"),
            "AKIAIOSFODNN7EXAMPLE",
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U",
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEow...",
        ];
        for value in known {
            assert!(is_known_credential_format(value), "{value}");
        }
        for value in [
            "ghp_short",
            "sk-SK",
            "eyJ-not-a-token",
            "AKIA",
            "https://github.com/rust-lang/rust",
            "/home/alice/projects/reframe",
            "hello world",
        ] {
            assert!(!is_known_credential_format(value), "{value}");
        }
    }

    #[test]
    fn credential_shapes_are_conservative() {
        let tokens = [
            // random base64url / alphanumeric secrets
            "q7XvT2mR9kLp4ZsW8nBc3HdF6jGy1KtN",
            "Zx8Kq2Lr9TfW4mBnY7cH3vJ6sD1gP5aQ",
            "wJalrXUtnFqMI/K7MDENG/bPxRfiCYzZ9kLmQ2w+",
            // 32+ hex characters
            "880f130c5e469263039877524e2b9774",
            "0023e1fdbd363b88a3dca4cd0cdb4b0fbdb056d1",
        ];
        for value in tokens {
            assert!(looks_like_credential(value), "{value}");
        }
        let ordinary = [
            "--new-window",
            "/home/alice/Projects/MyApp2024/SomeModule/Main",
            "Projects/MyApp2024/SomeModule/Main",
            "ThisIsAVeryLongClassNameForTesting2",
            "MyApplicationConfigurationManager2024",
            "my-very-long-project-name-2024-final-v2",
            "61c54bbd-c2c6-5271-96e7-009a87ff44bf",
            "61C54BBD-C2C6-5271-96E7-009A87FF44BF",
            "12345678901234567890123456789012345",
            "Default",
            "Profile 1",
            "C:\\Users\\alice\\AppData\\Local\\Programs\\Microsoft VS Code",
            "https://example.com/aVeryLongPathSegmentWith0123456789",
            "",
        ];
        for value in ordinary {
            assert!(!looks_like_credential(value), "{value}");
        }
    }
}
