//! Linux MPRIS (`org.mpris.MediaPlayer2`): the track a player is playing, read through
//! `gdbus` on the session bus.
//!
//! `gdbus call ... org.freedesktop.DBus.Properties.Get org.mpris.MediaPlayer2.Player Metadata`
//! prints a GVariant in text form, e.g.
//! `(<{'xesam:title': <'Song'>, 'xesam:artist': <['Artist']>, ...}>,)`; `PlaybackStatus` is
//! read the same way. Everything here is optional: when `gdbus` is missing, the player does not
//! implement MPRIS, there is no session bus or the calls exceed their small time budget, capture
//! simply has no MPRIS information. Only the title and artists are read.

use std::path::Path;
use std::time::{Duration, Instant};

use super::clean_label;
use crate::adapters::file_manager::support;

/// MPRIS bus-name suffixes per application (`org.mpris.MediaPlayer2.<name>`).
pub(super) fn bus_names(app_id: &str) -> &'static [&'static str] {
    match app_id {
        "spotify" => &["spotify"],
        "rhythmbox" => &["rhythmbox"],
        "lollypop" => &["Lollypop", "org.gnome.Lollypop"],
        "elisa" => &["elisa", "org.kde.elisa"],
        "strawberry" => &["strawberry"],
        "clementine" => &["clementine"],
        "amarok" => &["amarok"],
        "tidal" => &["tidal-hifi"],
        _ => &[],
    }
}

/// `Now playing: Artist - Title` / `Paused: …` from the player's MPRIS metadata, within
/// `budget`. `None` when unavailable or stopped. Linux only.
pub(super) fn now_playing(names: &[&str], pid: Option<u32>, budget: Duration) -> Option<String> {
    if !cfg!(target_os = "linux") || names.is_empty() || !session_bus_available() {
        return None;
    }
    let deadline = Instant::now() + budget;
    let mut buses: Vec<String> = names
        .iter()
        .map(|n| format!("org.mpris.MediaPlayer2.{n}"))
        .collect();
    // Some players register one name per process.
    if let Some(pid) = pid {
        buses.extend(
            names
                .iter()
                .map(|n| format!("org.mpris.MediaPlayer2.{n}.instance{pid}")),
        );
    }
    for bus in buses {
        let Some(metadata) = get_property(&bus, "Metadata", deadline) else {
            continue;
        };
        let track = parse_metadata(&metadata)?;
        let status =
            get_property(&bus, "PlaybackStatus", deadline).and_then(|raw| first_string(&raw));
        return label(&track, status.as_deref());
    }
    None
}

/// Whether a session bus is configured. Without one no player is reachable, and `gdbus` must
/// not be started: it would try to autolaunch a new bus (through X11 when `DISPLAY` is set).
fn session_bus_available() -> bool {
    std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some_and(|address| !address.is_empty())
        || std::env::var_os("XDG_RUNTIME_DIR")
            .is_some_and(|dir| !dir.is_empty() && Path::new(&dir).join("bus").exists())
}

/// `gdbus call` for one `org.mpris.MediaPlayer2.Player` property, bounded by `deadline`.
fn get_property(bus: &str, property: &str, deadline: Instant) -> Option<String> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining < Duration::from_millis(50) {
        return None;
    }
    let seconds = remaining.as_secs().clamp(1, 2).to_string();
    let output = support::command_output(
        "gdbus",
        &[
            "call",
            "--session",
            "--timeout",
            &seconds,
            "--dest",
            bus,
            "--object-path",
            "/org/mpris/MediaPlayer2",
            "--method",
            "org.freedesktop.DBus.Properties.Get",
            "org.mpris.MediaPlayer2.Player",
            property,
        ],
        remaining,
    );
    match output {
        Ok(text) => Some(text),
        Err(error) => {
            log::debug!("MPRIS {bus} {property}: {error}");
            None
        }
    }
}

/// Title and artists of the current track.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct Track {
    pub title: Option<String>,
    pub artists: Vec<String>,
}

/// Build the label shown in Reframe.
pub(super) fn label(track: &Track, status: Option<&str>) -> Option<String> {
    let title = track
        .title
        .as_deref()
        .map(clean_label)
        .filter(|t| !t.is_empty())?;
    let artists: Vec<String> = track
        .artists
        .iter()
        .map(|a| clean_label(a))
        .filter(|a| !a.is_empty())
        .collect();
    let text = if artists.is_empty() {
        title
    } else {
        format!("{} - {title}", artists.join(", "))
    };
    let prefix = match status {
        Some("Stopped") => return None,
        Some("Paused") => "Paused",
        _ => "Now playing",
    };
    Some(clean_label(&format!("{prefix}: {text}")))
}

/// Parse `Metadata` output. `None` when it contains neither a title nor artists.
pub(super) fn parse_metadata(text: &str) -> Option<Track> {
    let chars: Vec<char> = text.chars().collect();
    let mut track = Track::default();
    let mut found = false;
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '\'' && chars[i] != '"' {
            i += 1;
            continue;
        }
        let (key, next) = parse_string(&chars, i)?;
        let after = skip_whitespace(&chars, next);
        if chars.get(after) != Some(&':') {
            i = next;
            continue;
        }
        let value = skip_whitespace(&chars, after + 1);
        i = match key.as_str() {
            "xesam:title" => match parse_variant_string(&chars, value) {
                Some((title, end)) => {
                    track.title = Some(title);
                    found = true;
                    end
                }
                None => value,
            },
            "xesam:artist" => match parse_variant_strings(&chars, value) {
                Some((artists, end)) => {
                    track.artists = artists;
                    found = true;
                    end
                }
                None => value,
            },
            _ => value,
        };
    }
    found.then_some(track)
}

/// The first string literal in `text` (e.g. `(<'Playing'>,)`).
pub(super) fn first_string(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let start = chars.iter().position(|c| *c == '\'' || *c == '"')?;
    parse_string(&chars, start).map(|(value, _)| value)
}

fn skip_whitespace(chars: &[char], mut i: usize) -> usize {
    while chars.get(i).is_some_and(|c| c.is_whitespace()) {
        i += 1;
    }
    i
}

/// A GVariant string literal starting at `start` (quote character); returns the value and the
/// index after the closing quote.
fn parse_string(chars: &[char], start: usize) -> Option<(String, usize)> {
    let quote = *chars.get(start)?;
    let mut out = String::new();
    let mut i = start + 1;
    while let Some(&c) = chars.get(i) {
        match c {
            '\\' => {
                let escaped = *chars.get(i + 1)?;
                i += 2;
                match escaped {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    'a' => out.push('\u{7}'),
                    'b' => out.push('\u{8}'),
                    'f' => out.push('\u{c}'),
                    'v' => out.push('\u{b}'),
                    'u' | 'U' => {
                        let len = if escaped == 'u' { 4 } else { 8 };
                        let hex: String = chars.get(i..i + len)?.iter().collect();
                        let ch = u32::from_str_radix(&hex, 16)
                            .ok()
                            .and_then(char::from_u32)?;
                        out.push(ch);
                        i += len;
                    }
                    other => out.push(other),
                }
            }
            c if c == quote => return Some((out, i + 1)),
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    None
}

/// Skip `<`, whitespace and a type annotation (`@s`, `@as`, `string`, `objectpath`).
fn variant_body(chars: &[char], start: usize) -> Option<usize> {
    if chars.get(start) != Some(&'<') {
        return None;
    }
    let mut i = skip_whitespace(chars, start + 1);
    if chars.get(i) == Some(&'@') || chars.get(i).is_some_and(|c| c.is_ascii_alphabetic()) {
        while chars
            .get(i)
            .is_some_and(|c| !c.is_whitespace() && *c != '[')
        {
            i += 1;
        }
        i = skip_whitespace(chars, i);
    }
    Some(i)
}

fn close_variant(chars: &[char], i: usize) -> usize {
    let i = skip_whitespace(chars, i);
    if chars.get(i) == Some(&'>') { i + 1 } else { i }
}

/// `<'text'>`.
fn parse_variant_string(chars: &[char], start: usize) -> Option<(String, usize)> {
    let i = variant_body(chars, start)?;
    let (value, end) = parse_string(chars, i)?;
    Some((value, close_variant(chars, end)))
}

/// `<['a', 'b']>` (a single string is accepted from non-conforming players).
fn parse_variant_strings(chars: &[char], start: usize) -> Option<(Vec<String>, usize)> {
    let mut i = variant_body(chars, start)?;
    if matches!(chars.get(i), Some('\'' | '"')) {
        let (value, end) = parse_string(chars, i)?;
        return Some((vec![value], close_variant(chars, end)));
    }
    if chars.get(i) != Some(&'[') {
        return None;
    }
    i += 1;
    let mut values = Vec::new();
    loop {
        i = skip_whitespace(chars, i);
        match chars.get(i)? {
            ']' => return Some((values, close_variant(chars, i + 1))),
            ',' => i += 1,
            '\'' | '"' => {
                let (value, end) = parse_string(chars, i)?;
                values.push(value);
                i = end;
            }
            _ => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPOTIFY: &str = "(<{'mpris:trackid': <'/com/spotify/track/4uLU6hMCjMI75M1A2tKUQC'>, \
        'mpris:length': <uint64 213573000>, 'mpris:artUrl': <'https://i.scdn.co/image/ab67'>, \
        'xesam:album': <'Whenever You Need Somebody'>, 'xesam:albumArtist': <['Rick Astley']>, \
        'xesam:artist': <['Rick Astley']>, 'xesam:autoRating': <0.80000000000000004>, \
        'xesam:discNumber': <1>, 'xesam:title': <'Never Gonna Give You Up'>, \
        'xesam:trackNumber': <1>, 'xesam:url': <'https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC'>},>,)\n";

    #[test]
    fn parses_spotify_metadata() {
        assert_eq!(
            parse_metadata(SPOTIFY),
            Some(Track {
                title: Some("Never Gonna Give You Up".into()),
                artists: vec!["Rick Astley".into()],
            })
        );
    }

    #[test]
    fn parses_escapes_quotes_and_annotations() {
        let text = r#"(<{'xesam:title': <"Don't Stop Me Now">, 'xesam:artist': <@as ['Queen', 'A \'B\' é\U0001F3B5']>, 'mpris:trackid': <objectpath '/org/x'>},>,)"#;
        assert_eq!(
            parse_metadata(text),
            Some(Track {
                title: Some("Don't Stop Me Now".into()),
                artists: vec!["Queen".into(), "A 'B' é🎵".into()],
            })
        );
        // A title that *contains* a key is still just a title.
        let tricky = "(<{'xesam:title': <'xesam:artist: fake'>, 'xesam:artist': <@as []>},>,)";
        assert_eq!(
            parse_metadata(tricky),
            Some(Track {
                title: Some("xesam:artist: fake".into()),
                artists: Vec::new(),
            })
        );
        let single = "(<{'xesam:title': <@s 'Solo'>, 'xesam:artist': <'One'>},>,)";
        assert_eq!(
            parse_metadata(single).unwrap().artists,
            vec!["One".to_string()]
        );
    }

    #[test]
    fn malformed_metadata_is_rejected_without_panicking() {
        for text in [
            "",
            "(<@a{sv} {}>,)",
            "(<{'xesam:title': <'unterminated",
            "(<{'xesam:title': <'bad escape \\u12'>}>,)",
            "(<{'xesam:artist': <[1, 2]>}>,)",
            "'",
            "Error: GDBus.Error:org.freedesktop.DBus.Error.ServiceUnknown",
        ] {
            assert_eq!(parse_metadata(text), None, "{text:?}");
        }
    }

    #[test]
    fn reads_playback_status() {
        assert_eq!(first_string("(<'Playing'>,)\n").as_deref(), Some("Playing"));
        assert_eq!(first_string("()"), None);
    }

    #[test]
    fn labels() {
        let track = Track {
            title: Some("Song".into()),
            artists: vec!["A".into(), " ".into(), "B".into()],
        };
        assert_eq!(
            label(&track, Some("Playing")).as_deref(),
            Some("Now playing: A, B - Song")
        );
        assert_eq!(
            label(&track, None).as_deref(),
            Some("Now playing: A, B - Song")
        );
        assert_eq!(
            label(&track, Some("Paused")).as_deref(),
            Some("Paused: A, B - Song")
        );
        assert_eq!(label(&track, Some("Stopped")), None);
        assert_eq!(label(&Track::default(), Some("Playing")), None);
        let long = Track {
            title: Some("x".repeat(500)),
            artists: Vec::new(),
        };
        assert_eq!(
            label(&long, None).unwrap().chars().count(),
            super::super::MAX_LABEL_CHARS
        );
        let control = Track {
            title: Some("Line\nbreak\u{1}".into()),
            artists: vec!["\tA".into()],
        };
        assert_eq!(
            label(&control, None).as_deref(),
            Some("Now playing: A - Line break")
        );
    }

    #[test]
    fn parses_metadata_as_printed_by_glib() {
        // Output of GLib's g_variant_print (what `gdbus call` prints) for a Properties.Get
        // reply, generated with GLib itself: double-quoted strings when they contain a single
        // quote, `\"` and `\uXXXX` escapes, type annotations.
        let text = r#"(<{'mpris:trackid': <objectpath '/com/spotify/track/4uLU6hMCjMI75M1A2tKUQC'>, 'mpris:length': <uint64 213573000>, 'xesam:artist': <['Queen', "A \"B\" it's"]>, 'xesam:title': <"Don't Stop Me Now é♫\n\u0001">, 'xesam:album': <'Jazz'>, 'xesam:autoRating': <0.80000000000000004>}>,)"#;
        assert_eq!(
            parse_metadata(text),
            Some(Track {
                title: Some("Don't Stop Me Now é♫\n\u{1}".into()),
                artists: vec!["Queen".into(), "A \"B\" it's".into()],
            })
        );
        assert_eq!(parse_metadata("(<@a{sv} {}>,)"), None);
        assert_eq!(
            parse_metadata("(<{'xesam:artist': <@as []>}>,)"),
            Some(Track::default())
        );
    }

    #[test]
    fn bus_names_per_app() {
        assert_eq!(bus_names("spotify"), &["spotify"]);
        assert!(bus_names("itunes").is_empty());
        assert_eq!(now_playing(&[], Some(1), Duration::from_secs(1)), None);
    }

    #[test]
    fn unknown_players_answer_nothing_within_the_budget() {
        // No such player is registered (and there may be no session bus or gdbus at all; then
        // gdbus is not even started): the lookup must give up quietly and quickly.
        let start = Instant::now();
        let result = now_playing(
            &["reframe-test-no-such-player"],
            Some(std::process::id()),
            Duration::from_millis(1500),
        );
        assert_eq!(result, None);
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
