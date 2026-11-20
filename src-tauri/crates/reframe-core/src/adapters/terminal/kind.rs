//! The terminal emulators Reframe knows, and how to recognise and locate each of them.

use reframe_platform::{LaunchTarget, LocateQuery, OsKind, ProcessInfo, WindowInfo};

use super::shell::normalize_name;

/// Linux reports process names truncated to 15 bytes (`TASK_COMM_LEN - 1`).
const COMM_LEN: usize = 15;

/// A terminal emulator with a dedicated integration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerminalKind {
    WindowsTerminal,
    /// Classic console windows (conhost / OpenConsole) running cmd, Windows PowerShell or pwsh.
    WindowsConsole,
    GnomeTerminal,
    /// GNOME Console (`kgx`).
    GnomeConsole,
    Ptyxis,
    Konsole,
    Xfce4Terminal,
    MateTerminal,
    LxTerminal,
    QTerminal,
    Tilix,
    Terminator,
    Alacritty,
    Kitty,
    WezTerm,
    Foot,
    Xterm,
    /// rxvt-unicode (`urxvt`, `urxvtd`).
    Urxvt,
    DeepinTerminal,
    Terminology,
    Ghostty,
    /// macOS Terminal.app.
    MacTerminal,
    ITerm2,
    Warp,
}

impl TerminalKind {
    pub const ALL: [TerminalKind; 24] = [
        Self::WindowsTerminal,
        Self::WindowsConsole,
        Self::GnomeTerminal,
        Self::GnomeConsole,
        Self::Ptyxis,
        Self::Konsole,
        Self::Xfce4Terminal,
        Self::MateTerminal,
        Self::LxTerminal,
        Self::QTerminal,
        Self::Tilix,
        Self::Terminator,
        Self::Alacritty,
        Self::Kitty,
        Self::WezTerm,
        Self::Foot,
        Self::Xterm,
        Self::Urxvt,
        Self::DeepinTerminal,
        Self::Terminology,
        Self::Ghostty,
        Self::MacTerminal,
        Self::ITerm2,
        Self::Warp,
    ];

    /// Machine-independent application id stored in workspaces.
    pub fn app_id(self) -> &'static str {
        match self {
            Self::WindowsTerminal => "windows-terminal",
            Self::WindowsConsole => "windows-console",
            Self::GnomeTerminal => "gnome-terminal",
            Self::GnomeConsole => "gnome-console",
            Self::Ptyxis => "ptyxis",
            Self::Konsole => "konsole",
            Self::Xfce4Terminal => "xfce4-terminal",
            Self::MateTerminal => "mate-terminal",
            Self::LxTerminal => "lxterminal",
            Self::QTerminal => "qterminal",
            Self::Tilix => "tilix",
            Self::Terminator => "terminator",
            Self::Alacritty => "alacritty",
            Self::Kitty => "kitty",
            Self::WezTerm => "wezterm",
            Self::Foot => "foot",
            Self::Xterm => "xterm",
            Self::Urxvt => "urxvt",
            Self::DeepinTerminal => "deepin-terminal",
            Self::Terminology => "terminology",
            Self::Ghostty => "ghostty",
            Self::MacTerminal => "macos-terminal",
            Self::ITerm2 => "iterm2",
            Self::Warp => "warp",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::WindowsTerminal => "Windows Terminal",
            Self::WindowsConsole => "Windows Console",
            Self::GnomeTerminal => "GNOME Terminal",
            Self::GnomeConsole => "GNOME Console",
            Self::Ptyxis => "Ptyxis",
            Self::Konsole => "Konsole",
            Self::Xfce4Terminal => "Xfce Terminal",
            Self::MateTerminal => "MATE Terminal",
            Self::LxTerminal => "LXTerminal",
            Self::QTerminal => "QTerminal",
            Self::Tilix => "Tilix",
            Self::Terminator => "Terminator",
            Self::Alacritty => "Alacritty",
            Self::Kitty => "kitty",
            Self::WezTerm => "WezTerm",
            Self::Foot => "foot",
            Self::Xterm => "XTerm",
            Self::Urxvt => "rxvt-unicode",
            Self::DeepinTerminal => "Deepin Terminal",
            Self::Terminology => "Terminology",
            Self::Ghostty => "Ghostty",
            Self::MacTerminal => "Terminal",
            Self::ITerm2 => "iTerm2",
            Self::Warp => "Warp",
        }
    }

    pub fn from_app_id(app_id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.app_id() == app_id)
    }

    /// Operating systems the terminal exists on (`Other` covers the BSDs).
    pub fn runs_on(self, os: OsKind) -> bool {
        match self {
            Self::WindowsTerminal | Self::WindowsConsole => os == OsKind::Windows,
            Self::MacTerminal | Self::ITerm2 | Self::Warp => os == OsKind::Macos,
            Self::Alacritty | Self::WezTerm => true,
            Self::Kitty | Self::Ghostty => os != OsKind::Windows,
            _ => matches!(os, OsKind::Linux | OsKind::Other),
        }
    }

    /// Normalised (lower-case, no `.exe`) names of the processes that own the terminal's
    /// windows. Windows consoles are recognised separately (see [`identify_kind`]).
    fn process_names(self) -> &'static [&'static str] {
        match self {
            Self::WindowsTerminal => &["windowsterminal"],
            Self::WindowsConsole => &[],
            Self::GnomeTerminal => &["gnome-terminal-server", "gnome-terminal"],
            Self::GnomeConsole => &["kgx", "gnome-console"],
            Self::Ptyxis => &["ptyxis"],
            Self::Konsole => &["konsole"],
            Self::Xfce4Terminal => &["xfce4-terminal"],
            Self::MateTerminal => &["mate-terminal"],
            Self::LxTerminal => &["lxterminal"],
            Self::QTerminal => &["qterminal"],
            Self::Tilix => &["tilix"],
            Self::Terminator => &["terminator"],
            Self::Alacritty => &["alacritty"],
            Self::Kitty => &["kitty"],
            Self::WezTerm => &["wezterm-gui", "wezterm"],
            Self::Foot => &["foot"],
            Self::Xterm => &["xterm"],
            Self::Urxvt => &["urxvt", "rxvt-unicode", "urxvtd"],
            Self::DeepinTerminal => &["deepin-terminal"],
            Self::Terminology => &["terminology"],
            Self::Ghostty => &["ghostty"],
            Self::MacTerminal => &["terminal"],
            Self::ITerm2 => &["iterm2"],
            // Warp's executable is called "stable": too generic, recognised by bundle id only.
            Self::Warp => &[],
        }
    }

    /// macOS bundle identifiers, the one used for launching first.
    pub fn mac_bundle_ids(self) -> &'static [&'static str] {
        match self {
            Self::MacTerminal => &["com.apple.Terminal"],
            Self::ITerm2 => &["com.googlecode.iterm2"],
            Self::Warp => &["dev.warp.Warp-Stable", "dev.warp.Warp-Preview"],
            Self::Ghostty => &["com.mitchellh.ghostty"],
            Self::Alacritty => &["org.alacritty"],
            Self::Kitty => &["net.kovidgoyal.kitty"],
            Self::WezTerm => &["com.github.wez.wezterm"],
            _ => &[],
        }
    }

    /// The macOS bundle identifier used to launch the terminal through Launch Services.
    pub fn bundle_id(self) -> Option<&'static str> {
        self.mac_bundle_ids().first().copied()
    }

    /// Flatpak application ids.
    fn flatpak_ids(self) -> &'static [&'static str] {
        match self {
            Self::GnomeTerminal => &["org.gnome.Terminal"],
            Self::GnomeConsole => &["org.gnome.Console"],
            Self::Ptyxis => &[
                "org.gnome.Ptyxis",
                "org.gnome.Ptyxis.Devel",
                "app.devsuite.Ptyxis",
            ],
            Self::Konsole => &["org.kde.konsole"],
            Self::Tilix => &["com.gexperts.Tilix"],
            Self::Alacritty => &["org.alacritty.Alacritty"],
            Self::Kitty => &["net.kovidgoyal.kitty"],
            Self::WezTerm => &["org.wezfurlong.wezterm"],
            Self::Foot => &["org.codeberg.dnkl.foot"],
            Self::Ghostty => &["com.mitchellh.ghostty"],
            _ => &[],
        }
    }

    /// Lower-case window class / application-id hints (X11 `WM_CLASS`, `_GTK_APPLICATION_ID`,
    /// Win32 window classes, macOS bundle ids).
    fn window_hints(self) -> &'static [&'static str] {
        match self {
            Self::WindowsTerminal => &["cascadia_hosting_window_class"],
            Self::WindowsConsole => &[],
            Self::GnomeTerminal => &[
                "org.gnome.terminal",
                "gnome-terminal",
                "gnome-terminal-server",
            ],
            Self::GnomeConsole => &["org.gnome.console", "kgx"],
            Self::Ptyxis => &["org.gnome.ptyxis", "org.gnome.ptyxis.devel", "ptyxis"],
            Self::Konsole => &["org.kde.konsole", "konsole"],
            Self::Xfce4Terminal => &["xfce4-terminal"],
            Self::MateTerminal => &["mate-terminal"],
            Self::LxTerminal => &["lxterminal"],
            Self::QTerminal => &["qterminal"],
            Self::Tilix => &["com.gexperts.tilix", "tilix"],
            Self::Terminator => &["terminator"],
            Self::Alacritty => &["alacritty", "org.alacritty"],
            Self::Kitty => &["kitty", "net.kovidgoyal.kitty"],
            Self::WezTerm => &["org.wezfurlong.wezterm", "com.github.wez.wezterm"],
            Self::Foot => &["foot", "footclient"],
            Self::Xterm => &["xterm", "uxterm"],
            Self::Urxvt => &["urxvt", "rxvt-unicode"],
            Self::DeepinTerminal => &["deepin-terminal"],
            Self::Terminology => &["terminology"],
            Self::Ghostty => &["com.mitchellh.ghostty"],
            Self::MacTerminal => &["com.apple.terminal"],
            Self::ITerm2 => &["com.googlecode.iterm2"],
            Self::Warp => &[
                "dev.warp.warp-stable",
                "dev.warp.warp-preview",
                "dev.warp.warp",
            ],
        }
    }

    /// Linux desktop entry ids.
    pub fn desktop_ids(self) -> &'static [&'static str] {
        match self {
            Self::GnomeTerminal => &["org.gnome.Terminal.desktop"],
            Self::GnomeConsole => &["org.gnome.Console.desktop"],
            Self::Ptyxis => &["org.gnome.Ptyxis.desktop"],
            Self::Konsole => &["org.kde.konsole.desktop"],
            Self::Xfce4Terminal => &["xfce4-terminal.desktop"],
            Self::MateTerminal => &["mate-terminal.desktop"],
            Self::LxTerminal => &["lxterminal.desktop"],
            Self::QTerminal => &["qterminal.desktop"],
            Self::Tilix => &["com.gexperts.Tilix.desktop"],
            Self::Terminator => &["terminator.desktop"],
            Self::Alacritty => &["Alacritty.desktop", "org.alacritty.Alacritty.desktop"],
            Self::Kitty => &["kitty.desktop"],
            Self::WezTerm => &["org.wezfurlong.wezterm.desktop"],
            Self::Foot => &["foot.desktop", "org.codeberg.dnkl.foot.desktop"],
            Self::Xterm => &["xterm.desktop", "debian-xterm.desktop"],
            Self::Urxvt => &["rxvt-unicode.desktop"],
            Self::DeepinTerminal => &["deepin-terminal.desktop", "org.deepin.terminal.desktop"],
            Self::Terminology => &["terminology.desktop"],
            Self::Ghostty => &["com.mitchellh.ghostty.desktop"],
            _ => &[],
        }
    }

    /// Executables (normalised names) that open new terminal windows. The process owning the
    /// windows is not always one of them: `gnome-terminal-server`, `urxvtd` and Windows
    /// Terminal's `WindowsTerminal.exe` are started through their launchers.
    pub fn launcher_names(self) -> &'static [&'static str] {
        match self {
            Self::WindowsTerminal => &["wt"],
            Self::WindowsConsole => &["cmd", "powershell", "pwsh"],
            Self::GnomeTerminal => &["gnome-terminal"],
            Self::GnomeConsole => &["kgx"],
            Self::Urxvt => &["urxvt", "rxvt-unicode", "urxvtc"],
            Self::WezTerm => &["wezterm", "wezterm-gui"],
            Self::MacTerminal | Self::ITerm2 | Self::Warp => &[],
            other => other.process_names(),
        }
    }

    /// Names shown when locating the application (Start menu, `.app` bundles).
    fn locate_display_names(self) -> &'static [&'static str] {
        match self {
            Self::WindowsConsole => &["Command Prompt", "Windows PowerShell", "PowerShell"],
            Self::GnomeTerminal => &["GNOME Terminal", "Terminal"],
            Self::GnomeConsole => &["GNOME Console", "Console"],
            Self::ITerm2 => &["iTerm2", "iTerm"],
            Self::Urxvt => &["rxvt-unicode", "URxvt"],
            _ => &[],
        }
    }

    /// Whether `name` (normalised) is one of the processes that own the terminal's windows.
    pub fn process_names_include(self, name: &str) -> bool {
        self.process_names().contains(&name)
    }

    fn matches_process_name(self, name: &str) -> bool {
        self.process_names().iter().any(|known| {
            *known == name
                || (name.len() == COMM_LEN && known.len() > COMM_LEN && known.starts_with(name))
        })
    }
}

fn kind_for_package(id: &str) -> Option<TerminalKind> {
    if id.to_lowercase().starts_with("microsoft.windowsterminal") {
        return Some(TerminalKind::WindowsTerminal);
    }
    TerminalKind::ALL.into_iter().find(|k| {
        k.mac_bundle_ids()
            .iter()
            .chain(k.flatpak_ids())
            .any(|known| known.eq_ignore_ascii_case(id))
    })
}

fn kind_for_hint(hint: &str) -> Option<TerminalKind> {
    let hint = hint.trim().to_lowercase();
    if hint.is_empty() {
        return None;
    }
    if hint.starts_with("microsoft.windowsterminal") {
        return Some(TerminalKind::WindowsTerminal);
    }
    TerminalKind::ALL
        .into_iter()
        .find(|k| k.window_hints().contains(&hint.as_str()))
}

/// The terminal a launch target starts, when it is one of the known launchers or packages
/// (`/usr/bin/alacritty`, `wt.exe`, the `com.apple.Terminal` bundle...).
pub fn kind_for_target(target: &LaunchTarget) -> Option<TerminalKind> {
    match target {
        LaunchTarget::Executable { path } => {
            let name = normalize_name(&path.to_string_lossy());
            TerminalKind::ALL
                .into_iter()
                .find(|k| k.launcher_names().contains(&name.as_str()))
        }
        LaunchTarget::Package { package } => kind_for_package(&package.id),
        LaunchTarget::Open { .. } => None,
    }
}

/// The Win32 class of classic console windows.
pub const CONSOLE_WINDOW_CLASS: &str = "ConsoleWindowClass";

/// Shells that own classic console windows, by normalised process name.
pub fn is_console_shell(normalized_name: &str) -> bool {
    matches!(normalized_name, "cmd" | "powershell" | "pwsh")
}

/// Console hosts: `conhost.exe`, and `OpenConsole.exe` when it is the default console host.
pub fn is_console_host(normalized_name: &str) -> bool {
    matches!(normalized_name, "conhost" | "openconsole")
}

/// Which terminal `process` (owning `windows`) is, on `os`.
///
/// Order: package id (bundle / Flatpak / Windows package), Windows console ownership, process
/// name or executable name (tolerating Linux's 15-byte name truncation), window class hints.
///
/// Classic Windows consoles: Win32 reports a console window as owned by the console's client
/// process (conhost sets the owner to the shell that created the console), so the owner is
/// usually `cmd.exe`, `powershell.exe` or `pwsh.exe`; some enumeration paths report
/// `conhost.exe` / `OpenConsole.exe` instead. Both are recognised, but a shell is only claimed
/// when its windows are console windows: `powershell.exe` showing a WinForms/WPF window is a GUI
/// application, not a terminal.
pub fn identify_kind(
    os: OsKind,
    process: &ProcessInfo,
    windows: &[&WindowInfo],
) -> Option<TerminalKind> {
    if let Some(kind) = process
        .package
        .as_ref()
        .and_then(|p| kind_for_package(&p.id))
        .filter(|k| k.runs_on(os))
    {
        return Some(kind);
    }

    let name = normalize_name(&process.name);
    if os == OsKind::Windows && (is_console_shell(&name) || is_console_host(&name)) {
        let console_windows = windows.is_empty()
            || windows.iter().any(|w| {
                w.class_name
                    .as_deref()
                    .is_none_or(|c| c.eq_ignore_ascii_case(CONSOLE_WINDOW_CLASS))
            });
        return console_windows.then_some(TerminalKind::WindowsConsole);
    }

    let mut names = vec![name];
    if let Some(exe) = process
        .executable_path
        .as_deref()
        .and_then(|p| p.file_name())
    {
        names.push(normalize_name(&exe.to_string_lossy()));
    }
    if let Some(kind) = TerminalKind::ALL
        .into_iter()
        .filter(|k| k.runs_on(os))
        .find(|k| names.iter().any(|n| k.matches_process_name(n)))
    {
        return Some(kind);
    }

    windows
        .iter()
        .flat_map(|w| [w.app_id_hint.as_deref(), w.class_name.as_deref()])
        .flatten()
        .filter_map(kind_for_hint)
        .find(|k| k.runs_on(os))
}

/// How to find `kind` installed on `os`.
pub fn locate_query_for(kind: TerminalKind, os: OsKind) -> LocateQuery {
    let mut query = LocateQuery::default();
    let windows = os == OsKind::Windows;
    let mut exe_names: Vec<&str> = kind.launcher_names().to_vec();
    if kind == TerminalKind::WindowsTerminal {
        exe_names.push("WindowsTerminal");
    }
    for name in exe_names {
        if windows {
            query.executable_names.push(format!("{name}.exe"));
        }
        query.executable_names.push(name.to_string());
    }
    if os == OsKind::Macos {
        query.bundle_ids = kind
            .mac_bundle_ids()
            .iter()
            .map(|b| b.to_string())
            .collect();
    }
    if matches!(os, OsKind::Linux | OsKind::Other) {
        query.desktop_ids = kind.desktop_ids().iter().map(|d| d.to_string()).collect();
    }
    query.display_names.push(kind.display_name().to_string());
    for name in kind.locate_display_names() {
        if !query.display_names.iter().any(|n| n == name) {
            query.display_names.push(name.to_string());
        }
    }
    query
}

#[cfg(test)]
mod tests {
    use super::*;
    use reframe_platform::{PackageInfo, PackageKind, Rect, WindowId, WindowState};

    fn process(name: &str, exe: Option<&str>) -> ProcessInfo {
        ProcessInfo {
            pid: 10,
            parent_pid: None,
            name: name.into(),
            product_name: None,
            executable_path: exe.map(Into::into),
            command_line: Vec::new(),
            working_directory: None,
            start_time: None,
            package: None,
        }
    }

    fn window(class: Option<&str>, hint: Option<&str>) -> WindowInfo {
        WindowInfo {
            id: WindowId::new("w"),
            pid: 10,
            title: "t".into(),
            class_name: class.map(Into::into),
            app_id_hint: hint.map(Into::into),
            bounds: Rect::new(0, 0, 10, 10),
            normal_bounds: None,
            state: WindowState::Normal,
            monitor_id: None,
            desktop: None,
            is_focused: false,
            z_order: 0,
        }
    }

    fn id(os: OsKind, p: &ProcessInfo, windows: &[&WindowInfo]) -> Option<&'static str> {
        identify_kind(os, p, windows).map(TerminalKind::app_id)
    }

    #[test]
    fn app_ids_are_unique_and_round_trip() {
        let mut seen = std::collections::HashSet::new();
        for kind in TerminalKind::ALL {
            assert!(seen.insert(kind.app_id()), "duplicate {}", kind.app_id());
            assert_eq!(TerminalKind::from_app_id(kind.app_id()), Some(kind));
            assert!(!kind.display_name().is_empty());
        }
        assert_eq!(TerminalKind::from_app_id("exe:bash"), None);
    }

    #[test]
    fn identifies_linux_terminals_by_process_name() {
        let cases: &[(&str, Option<&str>, &str)] = &[
            (
                "gnome-terminal-",
                Some("/usr/libexec/gnome-terminal-server"),
                "gnome-terminal",
            ),
            // Truncated name without executable path.
            ("gnome-terminal-", None, "gnome-terminal"),
            ("kgx", Some("/usr/bin/kgx"), "gnome-console"),
            ("ptyxis", None, "ptyxis"),
            ("konsole", None, "konsole"),
            ("xfce4-terminal", None, "xfce4-terminal"),
            ("mate-terminal", None, "mate-terminal"),
            ("lxterminal", None, "lxterminal"),
            ("qterminal", None, "qterminal"),
            ("tilix", None, "tilix"),
            // Terminator is a Python script: comm is the script name, exe is python.
            ("terminator", Some("/usr/bin/python3.12"), "terminator"),
            ("alacritty", None, "alacritty"),
            ("kitty", None, "kitty"),
            ("wezterm-gui", None, "wezterm"),
            ("foot", None, "foot"),
            ("xterm", None, "xterm"),
            ("urxvt", None, "urxvt"),
            ("urxvtd", None, "urxvt"),
            ("deepin-terminal", None, "deepin-terminal"),
            ("terminology", None, "terminology"),
            ("ghostty", None, "ghostty"),
        ];
        for (name, exe, expected) in cases {
            assert_eq!(
                id(OsKind::Linux, &process(name, *exe), &[]),
                Some(*expected),
                "{name}"
            );
        }
        for name in [
            "bash",
            "firefox",
            "code",
            "gnome-shell",
            "terminal",
            "WindowsTerminal.exe",
            "pwsh",
        ] {
            assert_eq!(id(OsKind::Linux, &process(name, None), &[]), None, "{name}");
        }
    }

    #[test]
    fn identifies_windows_terminals_and_consoles() {
        let console = window(Some("ConsoleWindowClass"), None);
        let form = window(Some("WindowsForms10.Window.8.app.0.1"), None);
        let cascadia = window(Some("CASCADIA_HOSTING_WINDOW_CLASS"), None);
        let win = OsKind::Windows;
        assert_eq!(
            id(win, &process("WindowsTerminal.exe", None), &[&cascadia]),
            Some("windows-terminal")
        );
        for owner in [
            "cmd.exe",
            "powershell.exe",
            "pwsh.exe",
            "conhost.exe",
            "OpenConsole.exe",
        ] {
            assert_eq!(
                id(win, &process(owner, None), &[&console]),
                Some("windows-console"),
                "{owner}"
            );
        }
        // PowerShell running a WinForms script is a GUI app, not a terminal.
        assert_eq!(id(win, &process("powershell.exe", None), &[&form]), None);
        assert_eq!(
            id(win, &process("wezterm-gui.exe", None), &[]),
            Some("wezterm")
        );
        assert_eq!(
            id(win, &process("alacritty.exe", None), &[]),
            Some("alacritty")
        );
        // Linux-only terminals are not claimed on Windows.
        assert_eq!(id(win, &process("konsole.exe", None), &[]), None);
        // Packaged Windows Terminal.
        let mut packaged = process("WindowsTerminal.exe", None);
        packaged.package = Some(PackageInfo {
            kind: PackageKind::WindowsPackage,
            id: "Microsoft.WindowsTerminal_8wekyb3d8bbwe!App".into(),
            path: None,
        });
        assert_eq!(id(win, &packaged, &[]), Some("windows-terminal"));
    }

    #[test]
    fn identifies_macos_terminals_by_bundle_or_name() {
        let mac = OsKind::Macos;
        let bundle = |name: &str, bundle_id: &str| {
            let mut p = process(name, None);
            p.package = Some(PackageInfo {
                kind: PackageKind::MacBundle,
                id: bundle_id.into(),
                path: None,
            });
            p
        };
        for (name, bundle_id, expected) in [
            ("Terminal", "com.apple.Terminal", "macos-terminal"),
            ("iTerm2", "com.googlecode.iterm2", "iterm2"),
            ("alacritty", "org.alacritty", "alacritty"),
            ("kitty", "net.kovidgoyal.kitty", "kitty"),
            ("wezterm-gui", "com.github.wez.wezterm", "wezterm"),
            ("ghostty", "com.mitchellh.ghostty", "ghostty"),
            ("stable", "dev.warp.Warp-Stable", "warp"),
        ] {
            assert_eq!(
                id(mac, &bundle(name, bundle_id), &[]),
                Some(expected),
                "{bundle_id}"
            );
        }
        assert_eq!(
            id(mac, &process("Terminal", None), &[]),
            Some("macos-terminal")
        );
        assert_eq!(id(mac, &process("iTerm2", None), &[]), Some("iterm2"));
        // Warp without package info: recognised through the window's bundle id.
        let warp = window(Some("dev.warp.Warp-Stable"), Some("dev.warp.Warp-Stable"));
        assert_eq!(id(mac, &process("stable", None), &[&warp]), Some("warp"));
        assert_eq!(id(mac, &process("stable", None), &[]), None);
        // Terminal.app's name means nothing on Linux.
        assert_eq!(id(OsKind::Linux, &process("Terminal", None), &[]), None);
    }

    #[test]
    fn identifies_by_flatpak_id_and_window_hints() {
        let mut p = process("ptyxis-agent-ish", None);
        p.package = Some(PackageInfo {
            kind: PackageKind::Flatpak,
            id: "org.gnome.Ptyxis".into(),
            path: None,
        });
        assert_eq!(id(OsKind::Linux, &p, &[]), Some("ptyxis"));
        let gtk = window(Some("Gnome-terminal"), Some("org.gnome.Terminal"));
        assert_eq!(
            id(OsKind::Linux, &process("unknown", None), &[&gtk]),
            Some("gnome-terminal")
        );
        let other = window(Some("Firefox"), Some("firefox"));
        assert_eq!(
            id(OsKind::Linux, &process("unknown", None), &[&other]),
            None
        );
    }

    #[test]
    fn kinds_from_launch_targets() {
        let exe = |p: &str| LaunchTarget::Executable { path: p.into() };
        assert_eq!(
            kind_for_target(&exe("/usr/bin/gnome-terminal")),
            Some(TerminalKind::GnomeTerminal)
        );
        assert_eq!(
            kind_for_target(&exe(
                r"C:\Users\u\AppData\Local\Microsoft\WindowsApps\wt.exe"
            )),
            Some(TerminalKind::WindowsTerminal)
        );
        assert_eq!(
            kind_for_target(&exe(r"C:\Windows\System32\cmd.exe")),
            Some(TerminalKind::WindowsConsole)
        );
        // Servers and helpers are not launchers.
        assert_eq!(
            kind_for_target(&exe("/usr/libexec/gnome-terminal-server")),
            None
        );
        assert_eq!(kind_for_target(&exe("/usr/bin/open")), None);
        let terminal_app = LaunchTarget::Package {
            package: PackageInfo {
                kind: PackageKind::MacBundle,
                id: "com.apple.Terminal".into(),
                path: None,
            },
        };
        assert_eq!(
            kind_for_target(&terminal_app),
            Some(TerminalKind::MacTerminal)
        );
        assert_eq!(
            kind_for_target(&LaunchTarget::Open {
                target: "/tmp".into()
            }),
            None
        );
    }

    #[test]
    fn locate_queries() {
        let q = locate_query_for(TerminalKind::GnomeTerminal, OsKind::Linux);
        assert_eq!(q.executable_names, vec!["gnome-terminal"]);
        assert_eq!(q.desktop_ids, vec!["org.gnome.Terminal.desktop"]);
        assert!(q.bundle_ids.is_empty());
        let q = locate_query_for(TerminalKind::Konsole, OsKind::Linux);
        assert_eq!(q.desktop_ids, vec!["org.kde.konsole.desktop"]);
        let q = locate_query_for(TerminalKind::WindowsTerminal, OsKind::Windows);
        assert_eq!(
            q.executable_names,
            vec!["wt.exe", "wt", "WindowsTerminal.exe", "WindowsTerminal"]
        );
        let q = locate_query_for(TerminalKind::ITerm2, OsKind::Macos);
        assert_eq!(q.bundle_ids, vec!["com.googlecode.iterm2"]);
        assert!(q.executable_names.is_empty());
        assert!(q.display_names.contains(&"iTerm".to_string()));
        let q = locate_query_for(TerminalKind::MacTerminal, OsKind::Macos);
        assert_eq!(q.bundle_ids, vec!["com.apple.Terminal"]);
        assert_eq!(q.display_names, vec!["Terminal"]);
        let q = locate_query_for(TerminalKind::Warp, OsKind::Macos);
        assert_eq!(
            q.bundle_ids,
            vec!["dev.warp.Warp-Stable", "dev.warp.Warp-Preview"]
        );
        let q = locate_query_for(TerminalKind::WezTerm, OsKind::Macos);
        assert_eq!(q.bundle_ids[0], "com.github.wez.wezterm");
        assert_eq!(q.executable_names, vec!["wezterm", "wezterm-gui"]);
    }
}
