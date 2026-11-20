//! Graphical session detection from the environment (X11, Wayland, none).
//!
//! Only environment variables are inspected here; whether an X server is actually reachable
//! is decided later by trying to connect (see [`super::x11`]).

use std::ffi::OsString;

/// The session-related environment variables Reframe looks at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SessionEnv {
    /// `XDG_SESSION_TYPE` (`x11`, `wayland`, `tty`, ...).
    pub session_type: Option<String>,
    /// `WAYLAND_DISPLAY`.
    pub wayland_display: Option<String>,
    /// `DISPLAY` (the X11 display, also set for XWayland).
    pub display: Option<String>,
    /// `XDG_CURRENT_DESKTOP` (colon separated, e.g. `ubuntu:GNOME`).
    pub current_desktop: Option<String>,
    /// `XDG_SESSION_DESKTOP`.
    pub session_desktop: Option<String>,
    /// `DESKTOP_SESSION` (sometimes a path to a session file).
    pub desktop_session: Option<String>,
}

fn non_empty(value: Option<OsString>) -> Option<String> {
    value
        .map(|v| v.to_string_lossy().trim().to_string())
        .filter(|v| !v.is_empty())
}

impl SessionEnv {
    /// Read the variables from the process environment.
    pub fn from_process() -> Self {
        Self {
            session_type: non_empty(std::env::var_os("XDG_SESSION_TYPE")),
            wayland_display: non_empty(std::env::var_os("WAYLAND_DISPLAY")),
            display: non_empty(std::env::var_os("DISPLAY")),
            current_desktop: non_empty(std::env::var_os("XDG_CURRENT_DESKTOP")),
            session_desktop: non_empty(std::env::var_os("XDG_SESSION_DESKTOP")),
            desktop_session: non_empty(std::env::var_os("DESKTOP_SESSION")),
        }
    }
}

/// What kind of graphical session the environment describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionKind {
    /// An X11 session, or no declared session type but `DISPLAY` is set (e.g. SSH X forwarding).
    X11,
    /// A Wayland session. X11 clients (and Reframe's window management) may still be reachable
    /// through XWayland when `DISPLAY` is set.
    Wayland,
    /// Neither `DISPLAY` nor `WAYLAND_DISPLAY` is set: no graphical session at all.
    Headless,
}

/// Classify the session. An explicit `XDG_SESSION_TYPE` of `x11` or `wayland` wins; otherwise
/// the presence of `WAYLAND_DISPLAY` / `DISPLAY` decides.
pub(crate) fn classify(env: &SessionEnv) -> SessionKind {
    let declared = env.session_type.as_deref().map(str::to_ascii_lowercase);
    match declared.as_deref() {
        Some("wayland") => SessionKind::Wayland,
        Some("x11") => SessionKind::X11,
        _ if env.wayland_display.is_some() => SessionKind::Wayland,
        _ if env.display.is_some() => SessionKind::X11,
        _ => SessionKind::Headless,
    }
}

/// Canonical spelling of well-known desktop environment names (matched case-insensitively).
const KNOWN_DESKTOPS: &[&str] = &[
    "GNOME",
    "KDE",
    "XFCE",
    "LXQt",
    "LXDE",
    "MATE",
    "Cinnamon",
    "Unity",
    "Budgie",
    "Pantheon",
    "Deepin",
    "DDE",
    "COSMIC",
    "Enlightenment",
    "Trinity",
    "UKUI",
    "Lumina",
    "i3",
    "sway",
    "Hyprland",
    "Openbox",
    "Fluxbox",
    "IceWM",
    "awesome",
    "bspwm",
];

fn canonical(component: &str) -> Option<&'static str> {
    let name = component.strip_prefix("X-").unwrap_or(component);
    KNOWN_DESKTOPS
        .iter()
        .copied()
        .find(|known| known.eq_ignore_ascii_case(name))
}

/// Normalise one desktop variable: the first well-known component of a colon separated list
/// (`ubuntu:GNOME` -> `GNOME`, `X-Cinnamon` -> `Cinnamon`), else the first component as is.
fn normalise(value: &str) -> Option<String> {
    // DESKTOP_SESSION is sometimes a path such as /usr/share/xsessions/plasma.
    let value = value.rsplit('/').next().unwrap_or(value);
    let components: Vec<&str> = value
        .split(':')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .collect();
    if let Some(known) = components.iter().find_map(|c| canonical(c)) {
        return Some(known.to_string());
    }
    components
        .first()
        .map(|c| c.strip_prefix("X-").unwrap_or(c).to_string())
        .filter(|c| !c.is_empty())
}

/// Desktop environment name from `XDG_CURRENT_DESKTOP`, falling back to
/// `XDG_SESSION_DESKTOP` and `DESKTOP_SESSION`.
pub(crate) fn desktop_environment(env: &SessionEnv) -> Option<String> {
    [
        env.current_desktop.as_deref(),
        env.session_desktop.as_deref(),
        env.desktop_session.as_deref(),
    ]
    .into_iter()
    .flatten()
    .find_map(normalise)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(session_type: Option<&str>, wayland: Option<&str>, display: Option<&str>) -> SessionEnv {
        SessionEnv {
            session_type: session_type.map(str::to_string),
            wayland_display: wayland.map(str::to_string),
            display: display.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn declared_session_type_wins() {
        assert_eq!(
            classify(&env(Some("wayland"), None, Some(":0"))),
            SessionKind::Wayland
        );
        assert_eq!(
            classify(&env(Some("x11"), Some("wayland-0"), Some(":0"))),
            SessionKind::X11
        );
        assert_eq!(
            classify(&env(Some("Wayland"), None, None)),
            SessionKind::Wayland
        );
    }

    #[test]
    fn falls_back_to_display_variables() {
        assert_eq!(
            classify(&env(Some("tty"), Some("wayland-0"), None)),
            SessionKind::Wayland
        );
        assert_eq!(
            classify(&env(None, None, Some("localhost:10.0"))),
            SessionKind::X11
        );
        assert_eq!(
            classify(&env(Some("tty"), None, None)),
            SessionKind::Headless
        );
        assert_eq!(classify(&SessionEnv::default()), SessionKind::Headless);
    }

    #[test]
    fn desktop_environment_is_normalised() {
        let mut e = SessionEnv {
            current_desktop: Some("ubuntu:GNOME".into()),
            ..Default::default()
        };
        assert_eq!(desktop_environment(&e).as_deref(), Some("GNOME"));
        e.current_desktop = Some("X-Cinnamon".into());
        assert_eq!(desktop_environment(&e).as_deref(), Some("Cinnamon"));
        e.current_desktop = Some("kde".into());
        assert_eq!(desktop_environment(&e).as_deref(), Some("KDE"));
        e.current_desktop = Some("Budgie:GNOME".into());
        assert_eq!(desktop_environment(&e).as_deref(), Some("Budgie"));
        e.current_desktop = Some("MyShell".into());
        assert_eq!(desktop_environment(&e).as_deref(), Some("MyShell"));
    }

    #[test]
    fn desktop_environment_fallbacks() {
        let e = SessionEnv {
            desktop_session: Some("/usr/share/xsessions/xfce".into()),
            ..Default::default()
        };
        assert_eq!(desktop_environment(&e).as_deref(), Some("XFCE"));
        let e = SessionEnv {
            session_desktop: Some("lxqt".into()),
            desktop_session: Some("other".into()),
            ..Default::default()
        };
        assert_eq!(desktop_environment(&e).as_deref(), Some("LXQt"));
        assert_eq!(desktop_environment(&SessionEnv::default()), None);
    }
}
