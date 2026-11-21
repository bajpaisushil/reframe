//! Music players (Spotify, Apple Music, Rhythmbox, Lollypop, Elisa, Strawberry, Clementine,
//! Amarok, foobar2000, MusicBee, iTunes, TIDAL, Deezer): relaunch the player and, for Spotify,
//! open a playlist the user chose.
//!
//! Capture records what was playing as an informational label (`Now playing: Artist - Song`):
//! from the player's MPRIS metadata on Linux ([`mpris`], 2 s budget), else from the window title
//! where the player shows the track (Spotify shows `Artist - Song` while playing and
//! `Spotify Premium` / `Spotify Free` when idle). Playback position, queue and account state are
//! not accessible to other applications and are never captured or restored; the URI starts
//! empty.
//!
//! Users can set a Spotify playlist / album / track link in the workspace (`AppState::Media`
//! `uri`). It is validated strictly ([`spotify::normalize_uri`]) and restored with
//! `spotify --uri=<uri>` on Windows and Linux, or `open <uri>` on macOS (also used when no
//! Spotify executable is known). Invalid links are ignored and the player is launched plainly;
//! other players are only launched.

mod mpris;
pub mod spotify;

use std::time::Duration;

use reframe_platform::{
    LaunchRequest, LaunchTarget, LocateQuery, OsKind, PlatformInfo, ProcessInfo, WindowInfo,
};

use super::file_manager::support::{self, KnownApp};
use super::generic::{self, GenericAdapter};
use super::{
    AppAdapter, AppIdentity, CaptureContext, CaptureInput, CaptureOutput, InstanceModel,
    LaunchContext, LaunchStep,
};
use crate::model::{AdapterCapability, AppState, ApplicationEntry, CapabilityLevel, StateDetail};

/// Time allowed for reading MPRIS metadata during capture.
const MPRIS_BUDGET: Duration = Duration::from_secs(2);
/// Longer labels are cut.
const MAX_LABEL_CHARS: usize = 200;

const APPS: &[KnownApp] = &[
    KnownApp::new("spotify", "Spotify")
        .executables(&["spotify"])
        .locate_names(&["spotify", "Spotify.exe"])
        .bundle_ids(&["com.spotify.client"])
        .flatpak_ids(&["com.spotify.Client"])
        .snap_names(&["spotify"])
        .windows_packages(&["SpotifyAB.SpotifyMusic"])
        .window_classes(&["spotify"])
        .desktop_ids(&[
            "spotify.desktop",
            "com.spotify.Client.desktop",
            "spotify_spotify.desktop",
        ]),
    KnownApp::new("apple-music", "Apple Music")
        .aliases(&["Music"])
        .executables(&["applemusic"])
        .executable_paths(&["Music.app/Contents/MacOS/Music"])
        .locate_names(&["AppleMusic.exe"])
        .bundle_ids(&["com.apple.Music"])
        .windows_packages(&["AppleInc.AppleMusicWin"]),
    KnownApp::new("rhythmbox", "Rhythmbox")
        .executables(&["rhythmbox"])
        .locate_names(&["rhythmbox"])
        .flatpak_ids(&["org.gnome.Rhythmbox3"])
        .window_classes(&["rhythmbox", "org.gnome.Rhythmbox3"])
        .desktop_ids(&["org.gnome.Rhythmbox3.desktop", "rhythmbox.desktop"]),
    KnownApp::new("lollypop", "Lollypop")
        .executables(&["lollypop"])
        .locate_names(&["lollypop"])
        .flatpak_ids(&["org.gnome.Lollypop"])
        .window_classes(&["lollypop", "org.gnome.Lollypop"])
        .desktop_ids(&["org.gnome.Lollypop.desktop"]),
    KnownApp::new("elisa", "Elisa")
        .executables(&["elisa"])
        .locate_names(&["elisa", "elisa.exe"])
        .flatpak_ids(&["org.kde.elisa"])
        .window_classes(&["elisa", "org.kde.elisa"])
        .desktop_ids(&["org.kde.elisa.desktop"]),
    KnownApp::new("strawberry", "Strawberry")
        .executables(&["strawberry"])
        .locate_names(&["strawberry", "strawberry.exe"])
        .bundle_ids(&["org.strawberrymusicplayer.strawberry"])
        .flatpak_ids(&["org.strawberrymusicplayer.strawberry"])
        .window_classes(&["strawberry"])
        .desktop_ids(&["org.strawberrymusicplayer.strawberry.desktop"]),
    KnownApp::new("clementine", "Clementine")
        .executables(&["clementine"])
        .locate_names(&["clementine", "clementine.exe"])
        .flatpak_ids(&["org.clementine_player.Clementine"])
        .window_classes(&["clementine"])
        .desktop_ids(&[
            "org.clementine_player.Clementine.desktop",
            "clementine.desktop",
        ]),
    KnownApp::new("amarok", "Amarok")
        .executables(&["amarok"])
        .locate_names(&["amarok"])
        .flatpak_ids(&["org.kde.amarok"])
        .window_classes(&["amarok"])
        .desktop_ids(&["org.kde.amarok.desktop"]),
    KnownApp::new("foobar2000", "foobar2000")
        .executables(&["foobar2000"])
        .locate_names(&["foobar2000.exe"])
        .bundle_ids(&["com.foobar2000.mac"]),
    KnownApp::new("musicbee", "MusicBee")
        .executables(&["musicbee"])
        .locate_names(&["MusicBee.exe"]),
    KnownApp::new("itunes", "iTunes")
        .executables(&["itunes"])
        .locate_names(&["iTunes.exe"])
        .bundle_ids(&["com.apple.iTunes"])
        .windows_packages(&["AppleInc.iTunes"]),
    KnownApp::new("tidal", "TIDAL")
        .executables(&["tidal", "tidal-hifi"])
        .locate_names(&["TIDAL.exe", "tidal-hifi"])
        .bundle_ids(&["com.tidal.desktop"])
        .flatpak_ids(&["com.mastermindzh.tidal-hifi"])
        .window_classes(&["tidal-hifi"]),
    KnownApp::new("deezer", "Deezer")
        .executables(&["deezer", "deezer-desktop"])
        .locate_names(&["Deezer.exe", "deezer-desktop"])
        .bundle_ids(&["com.deezer.deezer-desktop"])
        .flatpak_ids(&["dev.aunetx.deezer"]),
];

const NOT_ACCESSIBLE_NOTE: &str = "Not accessible to other applications; players keep you signed \
                                   in themselves.";

/// Only Spotify can be told what to open.
fn supports_uri(app_id: &str) -> bool {
    app_id == "spotify"
}

/// Names a player shows in its window title next to (or instead of) the track. `None` for
/// players whose title never shows the track (iTunes, Apple Music, Elisa).
fn title_labels(app_id: &str) -> Option<&'static [&'static str]> {
    Some(match app_id {
        "spotify" => &["Spotify", "Spotify Premium", "Spotify Free"],
        "foobar2000" => &["foobar2000"],
        "amarok" => &["Amarok"],
        "musicbee" => &["MusicBee"],
        "clementine" => &["Clementine"],
        "strawberry" => &["Strawberry Music Player", "Strawberry"],
        "deezer" => &["Deezer"],
        "tidal" => &["TIDAL", "tidal-hifi"],
        "rhythmbox" => &["Rhythmbox"],
        "lollypop" => &["Lollypop"],
        _ => return None,
    })
}

const TRACK_SEPARATORS: &[&str] = &[" - ", " – ", " — "];

/// The track a window title shows (`Artist - Song`) without the player's name; `None` when the
/// title shows no track (idle player, advertisement).
fn track_from_title(app_id: &str, title: &str) -> Option<String> {
    let labels = title_labels(app_id)?;
    let mut text = title.trim();
    // foobar2000: "Artist - Song  [foobar2000]" while playing.
    if text.ends_with(']')
        && let Some(index) = text.rfind('[')
        && text[index..].to_ascii_lowercase().contains("foobar2000")
    {
        text = text[..index].trim_end();
    }
    for label in labels {
        for separator in TRACK_SEPARATORS {
            if let Some(rest) = strip_suffix_ignore_case(text, &format!("{separator}{label}")) {
                text = rest.trim_end();
            }
            if let Some(rest) = strip_prefix_ignore_case(text, &format!("{label}{separator}")) {
                text = rest.trim_start();
            }
        }
    }
    let idle = text.is_empty() || text.eq_ignore_ascii_case("advertisement");
    if idle || !TRACK_SEPARATORS.iter().any(|s| text.contains(s)) {
        return None;
    }
    Some(clean_label(text))
}

fn strip_suffix_ignore_case<'a>(text: &'a str, suffix: &str) -> Option<&'a str> {
    let start = text.len().checked_sub(suffix.len())?;
    let tail = text.get(start..)?;
    tail.eq_ignore_ascii_case(suffix).then(|| &text[..start])
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

/// Labels are one line of bounded length: control characters (line breaks...) become spaces,
/// runs of whitespace collapse and long text is cut with an ellipsis.
fn clean_label(text: &str) -> String {
    let collapsed = text
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if collapsed.chars().count() <= MAX_LABEL_CHARS {
        collapsed
    } else {
        let mut cut: String = collapsed.chars().take(MAX_LABEL_CHARS - 1).collect();
        cut.push('…');
        cut
    }
}

#[derive(Debug)]
pub struct MusicAdapter {
    generic: GenericAdapter,
    /// Ask Linux players for their MPRIS metadata during capture.
    read_mpris: bool,
}

impl MusicAdapter {
    pub fn new() -> Self {
        Self {
            generic: GenericAdapter::new(),
            read_mpris: true,
        }
    }

    /// Deterministic captures in tests (no D-Bus).
    #[cfg(test)]
    fn without_mpris() -> Self {
        Self {
            generic: GenericAdapter::new(),
            read_mpris: false,
        }
    }
}

impl Default for MusicAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl AppAdapter for MusicAdapter {
    fn id(&self) -> &'static str {
        "music"
    }

    fn name(&self) -> &'static str {
        "Music players"
    }

    fn identify(&self, process: &ProcessInfo, windows: &[&WindowInfo]) -> Option<AppIdentity> {
        support::find_app(APPS, process, windows).map(KnownApp::identity)
    }

    fn capability(&self, info: &PlatformInfo) -> AdapterCapability {
        let capability = AdapterCapability::launch_only().feature("Launch application", true, None);
        support::with_window_placement(capability, info)
            .feature(
                "Start a chosen playlist",
                true,
                Some("Spotify only: paste a Spotify playlist, album or track link into the workspace."),
            )
            .feature(
                "Now playing",
                true,
                Some("Shown for reference where the player reveals it; not restored."),
            )
            .feature(
                "Playback position, queue and account state",
                false,
                Some(NOT_ACCESSIBLE_NOTE),
            )
    }

    fn capability_for_app(&self, app_id: &str, info: &PlatformInfo) -> AdapterCapability {
        let spotify = supports_uri(app_id);
        let capability = AdapterCapability {
            level: if spotify {
                CapabilityLevel::Partial
            } else {
                CapabilityLevel::LaunchOnly
            },
            features: Vec::new(),
        }
        .feature("Launch application", true, None);
        let now_playing = title_labels(app_id).is_some()
            || (info.os == OsKind::Linux && !mpris::bus_names(app_id).is_empty());
        support::with_window_placement(capability, info)
            .feature(
                "Start a chosen playlist",
                spotify,
                Some(if spotify {
                    "Paste a Spotify playlist, album or track link into the workspace; Spotify \
                     opens it when it starts (playback may need a click)."
                } else {
                    "Not supported by this player."
                }),
            )
            .feature(
                "Now playing",
                now_playing,
                Some(if now_playing {
                    "Shown for reference; not restored."
                } else {
                    "This player does not reveal the current track."
                }),
            )
            .feature(
                "Playback position, queue and account state",
                false,
                Some(NOT_ACCESSIBLE_NOTE),
            )
    }

    fn capture(&self, _ctx: &CaptureContext<'_>, input: &CaptureInput<'_>) -> CaptureOutput {
        let (launch, warnings) = support::launch_config(input);
        let app_id = input.identity.app_id.as_str();
        let from_mpris = if self.read_mpris {
            mpris::now_playing(
                mpris::bus_names(app_id),
                input.main_process().map(|p| p.pid),
                MPRIS_BUDGET,
            )
        } else {
            None
        };
        let label = from_mpris.or_else(|| {
            input
                .windows
                .iter()
                .find_map(|w| track_from_title(app_id, &w.title))
                .map(|track| format!("Now playing: {track}"))
        });
        let state = if label.is_some() || supports_uri(app_id) {
            AppState::Media { uri: None, label }
        } else {
            AppState::None
        };
        CaptureOutput {
            state,
            launch,
            warnings,
        }
    }

    fn launch_plan(
        &self,
        ctx: &LaunchContext<'_>,
        entry: &ApplicationEntry,
    ) -> Result<Vec<LaunchStep>, String> {
        let configured = match &entry.state {
            AppState::Media { uri: Some(uri), .. } if !uri.trim().is_empty() => Some(uri.as_str()),
            _ => None,
        };
        let start_uri = configured
            .filter(|_| supports_uri(&entry.app_id))
            .and_then(spotify::normalize_uri);
        if let Some(raw) = configured
            && start_uri.is_none()
        {
            let shown: String = raw.chars().take(80).collect();
            log::warn!(
                "{}: ignoring the configured link {shown:?}: not a valid Spotify link for this \
                 application",
                entry.display_name
            );
        }

        let target = generic::launch_target(&entry.launch);
        let (target, args) = match (start_uri, target) {
            // macOS (or no Spotify executable known): the system URL handler starts Spotify.
            (Some(uri), _) if ctx.info.os == OsKind::Macos => {
                (LaunchTarget::Open { target: uri }, Vec::new())
            }
            (Some(uri), None) => (LaunchTarget::Open { target: uri }, Vec::new()),
            (Some(uri), Some(target)) => {
                let mut args = entry.launch.arguments.clone();
                args.push(format!("--uri={uri}"));
                (target, args)
            }
            (None, Some(target)) => (target, entry.launch.arguments.clone()),
            (None, None) => {
                return Err(format!(
                    "No executable is configured for {}.",
                    entry.display_name
                ));
            }
        };
        let working_directory = match target {
            LaunchTarget::Open { .. } => None,
            _ => entry.launch.working_directory.clone(),
        };
        Ok(vec![LaunchStep {
            request: LaunchRequest {
                target,
                args,
                working_directory,
                env: Vec::new(),
            },
            expected_windows: 1,
            title_hint: None,
        }])
    }

    fn instance_model(&self) -> InstanceModel {
        // Spotify and most players forward a second launch to the running instance.
        InstanceModel::SingleInstance
    }

    fn describe(&self, state: &AppState) -> Vec<StateDetail> {
        let AppState::Media { uri, label } = state else {
            return Vec::new();
        };
        let mut details = Vec::new();
        if let Some(label) = label.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
            let value = match label.split_once(": ") {
                Some(("Now playing", track)) => track.to_string(),
                Some(("Paused", track)) => format!("{track} (paused)"),
                _ => label.to_string(),
            };
            details.push(StateDetail::new("Last played", value));
        }
        if let Some(uri) = uri.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
            details.push(match spotify::normalize_uri(uri) {
                Some(uri) => StateDetail::new("Opens", uri),
                None => StateDetail::new("Opens", "Ignored: not a valid Spotify link"),
            });
        }
        details
    }

    fn locate_query(&self, app_id: &str) -> LocateQuery {
        support::app_by_id(APPS, app_id)
            .map(KnownApp::locate_query)
            .unwrap_or_else(|| self.generic.locate_query(app_id))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::support::testing::{platform, process, window};
    use super::*;
    use reframe_platform::{PackageInfo, PackageKind};

    const ID: &str = "37i9dQZF1DXcBWIGoYBM5M";

    fn identify(p: &ProcessInfo, windows: &[&WindowInfo]) -> Option<String> {
        MusicAdapter::new().identify(p, windows).map(|id| id.app_id)
    }

    fn package(kind: PackageKind, id: &str) -> Option<PackageInfo> {
        Some(PackageInfo {
            kind,
            id: id.to_string(),
            path: None,
        })
    }

    #[test]
    fn identifies_players() {
        assert_eq!(
            identify(&process(1, "spotify", Some("/opt/spotify/spotify")), &[]).as_deref(),
            Some("spotify")
        );
        let mut store = process(1, "Spotify.exe", None);
        store.package = package(
            PackageKind::WindowsPackage,
            "SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify",
        );
        assert_eq!(identify(&store, &[]).as_deref(), Some("spotify"));
        let mut flatpak = process(1, "spotify", None);
        flatpak.package = package(PackageKind::Flatpak, "com.spotify.Client");
        assert_eq!(identify(&flatpak, &[]).as_deref(), Some("spotify"));
        let mut music = process(1, "Music", None);
        music.package = package(PackageKind::MacBundle, "com.apple.Music");
        assert_eq!(identify(&music, &[]).as_deref(), Some("apple-music"));
        assert_eq!(
            identify(&process(1, "AppleMusic.exe", None), &[]).as_deref(),
            Some("apple-music")
        );
        let rhythmbox_window = window("1", 1, "Rhythmbox", Some("Rhythmbox"));
        assert_eq!(
            identify(&process(1, "python3", None), &[&rhythmbox_window]).as_deref(),
            Some("rhythmbox")
        );
        assert_eq!(
            identify(&process(1, "foobar2000.exe", None), &[]).as_deref(),
            Some("foobar2000")
        );
        assert_eq!(
            identify(&process(1, "Music", Some("/usr/bin/Music")), &[]),
            None
        );
        assert_eq!(
            identify(&process(1, "vlc", Some("/usr/bin/vlc")), &[]),
            None
        );
    }

    #[test]
    fn reads_tracks_from_titles() {
        let track = |app: &str, title: &str| track_from_title(app, title);
        assert_eq!(
            track("spotify", "Rick Astley - Never Gonna Give You Up").as_deref(),
            Some("Rick Astley - Never Gonna Give You Up")
        );
        for idle in [
            "Spotify",
            "Spotify Premium",
            "Spotify Free",
            "Advertisement",
            "Spotify - Advertisement",
            "",
        ] {
            assert_eq!(track("spotify", idle), None, "{idle:?}");
        }
        assert_eq!(
            track(
                "foobar2000",
                "Daft Punk - [Discovery #01] One More Time  [foobar2000]"
            )
            .as_deref(),
            Some("Daft Punk - [Discovery #01] One More Time")
        );
        assert_eq!(track("foobar2000", "foobar2000 v2.1.4"), None);
        assert_eq!(
            track("amarok", "Artist - Title - Amarok").as_deref(),
            Some("Artist - Title")
        );
        assert_eq!(track("strawberry", "Strawberry Music Player"), None);
        assert_eq!(track("itunes", "Artist - Song"), None);
        assert_eq!(track("elisa", "Artist - Song"), None);
        assert_eq!(
            track("spotify", &format!("{} - x", "é".repeat(300)))
                .unwrap()
                .chars()
                .count(),
            MAX_LABEL_CHARS
        );
    }

    fn capture(app_exe: &str, titles: &[&str]) -> CaptureOutput {
        let adapter = MusicAdapter::without_mpris();
        let (platform, info) = platform(OsKind::Linux);
        let ctx = CaptureContext {
            platform: &platform,
            info: &info,
            browser_tab_capture: false,
            apple_events: false,
        };
        let name = PathBuf::from(app_exe)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let mut p = process(8, &name, Some(app_exe));
        p.command_line = vec![app_exe.to_string(), "--secret-token=abc".into()];
        let windows: Vec<WindowInfo> = titles
            .iter()
            .enumerate()
            .map(|(i, t)| window(&i.to_string(), 8, t, None))
            .collect();
        let refs: Vec<&WindowInfo> = windows.iter().collect();
        let identity = adapter.identify(&p, &refs).unwrap();
        let all = vec![p.clone()];
        let input = CaptureInput {
            identity: &identity,
            processes: vec![&p],
            windows: refs.clone(),
            all_processes: &all,
        };
        adapter.capture(&ctx, &input)
    }

    #[test]
    fn captures_now_playing_labels() {
        let out = capture("/usr/bin/spotify", &["Daft Punk - Digital Love"]);
        assert_eq!(
            out.state,
            AppState::Media {
                uri: None,
                label: Some("Now playing: Daft Punk - Digital Love".into()),
            }
        );
        assert_eq!(
            out.launch.executable,
            Some(PathBuf::from("/usr/bin/spotify"))
        );
        assert!(out.launch.arguments.is_empty());
        assert!(out.warnings.is_empty());

        let idle = capture("/usr/bin/spotify", &["Spotify Premium"]);
        assert_eq!(
            idle.state,
            AppState::Media {
                uri: None,
                label: None
            }
        );
        assert_eq!(
            capture("/usr/bin/rhythmbox", &["Rhythmbox"]).state,
            AppState::None
        );
        assert_eq!(
            capture("/usr/bin/amarok", &["A - B - Amarok"]).state,
            AppState::Media {
                uri: None,
                label: Some("Now playing: A - B".into()),
            }
        );
    }

    fn entry(app_id: &str, uri: Option<&str>) -> ApplicationEntry {
        let mut entry = ApplicationEntry::new(app_id, app_id, "music");
        entry.launch.executable = Some(PathBuf::from(format!("/usr/bin/{app_id}")));
        entry.state = AppState::Media {
            uri: uri.map(str::to_string),
            label: None,
        };
        entry
    }

    fn plan(os: OsKind, entry: &ApplicationEntry) -> Result<Vec<LaunchStep>, String> {
        let (platform, info) = platform(os);
        let ctx = LaunchContext {
            platform: &platform,
            info: &info,
        };
        MusicAdapter::new().launch_plan(&ctx, entry)
    }

    #[test]
    fn spotify_opens_the_configured_link() {
        let uri = format!("spotify:playlist:{ID}");
        let steps = plan(OsKind::Linux, &entry("spotify", Some(&uri))).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(
            steps[0].request.target,
            LaunchTarget::Executable {
                path: PathBuf::from("/usr/bin/spotify")
            }
        );
        assert_eq!(steps[0].request.args, vec![format!("--uri={uri}")]);

        let link = format!("https://open.spotify.com/album/{ID}?si=x");
        let mut e = entry("spotify", Some(&link));
        e.launch.arguments = vec!["--minimized".into()];
        e.launch.executable = None;
        let store = PackageInfo {
            kind: PackageKind::WindowsPackage,
            id: "SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify".into(),
            path: None,
        };
        e.launch.package = Some(store.clone());
        let steps = plan(OsKind::Windows, &e).unwrap();
        assert_eq!(
            steps[0].request.target,
            LaunchTarget::Package { package: store }
        );
        assert_eq!(
            steps[0].request.args,
            vec![
                "--minimized".to_string(),
                format!("--uri=spotify:album:{ID}")
            ]
        );

        let steps = plan(OsKind::Macos, &entry("spotify", Some(&link))).unwrap();
        assert_eq!(
            steps[0].request.target,
            LaunchTarget::Open {
                target: format!("spotify:album:{ID}")
            }
        );
        assert!(steps[0].request.args.is_empty());

        let mut no_exe = entry("spotify", Some(&uri));
        no_exe.launch.executable = None;
        let steps = plan(OsKind::Linux, &no_exe).unwrap();
        assert_eq!(steps[0].request.target, LaunchTarget::Open { target: uri });
    }

    #[test]
    fn invalid_links_and_other_players_launch_plainly() {
        for bad in [
            "spotify:playlist:x --flag",
            "javascript:alert(1)",
            "https://evil.example/playlist/37i9dQZF1DXcBWIGoYBM5M",
        ] {
            let steps = plan(OsKind::Linux, &entry("spotify", Some(bad))).unwrap();
            assert!(steps[0].request.args.is_empty(), "{bad}");
            assert_eq!(
                steps[0].request.target,
                LaunchTarget::Executable {
                    path: PathBuf::from("/usr/bin/spotify")
                }
            );
        }
        let uri = format!("spotify:playlist:{ID}");
        let steps = plan(OsKind::Linux, &entry("rhythmbox", Some(&uri))).unwrap();
        assert!(steps[0].request.args.is_empty());
        let mut none = entry("rhythmbox", None);
        none.launch.executable = None;
        assert!(plan(OsKind::Linux, &none).is_err());
        let mut idle = entry("spotify", None);
        idle.state = AppState::None;
        assert_eq!(plan(OsKind::Linux, &idle).unwrap()[0].expected_windows, 1);
    }

    #[test]
    fn capability_is_partial_for_spotify_only() {
        let adapter = MusicAdapter::new();
        let (_platform, info) = platform(OsKind::Linux);
        let spotify = adapter.capability_for_app("spotify", &info);
        assert_eq!(spotify.level, CapabilityLevel::Partial);
        let playlist = |c: &AdapterCapability| {
            c.features
                .iter()
                .find(|f| f.name == "Start a chosen playlist")
                .unwrap()
                .supported
        };
        assert!(playlist(&spotify));
        let rhythmbox = adapter.capability_for_app("rhythmbox", &info);
        assert_eq!(rhythmbox.level, CapabilityLevel::LaunchOnly);
        assert!(!playlist(&rhythmbox));
        for capability in [&spotify, &rhythmbox, &adapter.capability(&info)] {
            assert!(capability.features.iter().any(|f| {
                f.name == "Playback position, queue and account state" && !f.supported
            }));
        }
        assert_eq!(adapter.capability(&info).level, CapabilityLevel::LaunchOnly);
    }

    #[test]
    fn describes_media_state() {
        let adapter = MusicAdapter::new();
        let state = AppState::Media {
            uri: Some(format!("https://open.spotify.com/playlist/{ID}")),
            label: Some("Now playing: A - B".into()),
        };
        assert_eq!(
            adapter.describe(&state),
            vec![
                StateDetail::new("Last played", "A - B"),
                StateDetail::new("Opens", format!("spotify:playlist:{ID}")),
            ]
        );
        let paused = AppState::Media {
            uri: Some("not a link".into()),
            label: Some("Paused: A - B".into()),
        };
        assert_eq!(
            adapter.describe(&paused),
            vec![
                StateDetail::new("Last played", "A - B (paused)"),
                StateDetail::new("Opens", "Ignored: not a valid Spotify link"),
            ]
        );
        assert!(adapter.describe(&AppState::None).is_empty());
        assert!(
            adapter
                .describe(&AppState::Media {
                    uri: None,
                    label: None
                })
                .is_empty()
        );
    }

    #[test]
    fn locate_queries_and_instance_model() {
        let adapter = MusicAdapter::new();
        let q = adapter.locate_query("spotify");
        assert!(q.executable_names.contains(&"Spotify.exe".to_string()));
        assert_eq!(q.bundle_ids, vec!["com.spotify.client".to_string()]);
        assert!(
            q.desktop_ids
                .contains(&"com.spotify.Client.desktop".to_string())
        );
        assert_eq!(
            adapter.locate_query("apple-music").display_names,
            vec!["Apple Music".to_string(), "Music".to_string()]
        );
        assert_eq!(adapter.instance_model(), InstanceModel::SingleInstance);
    }
}
