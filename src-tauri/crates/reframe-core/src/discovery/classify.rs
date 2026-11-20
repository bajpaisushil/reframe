//! Classification of discovered applications into [`AppCategory`]:
//!
//! - **System** — OS shell and desktop components (taskbars, panels, docks, launchers, lock
//!   screens, notification daemons, input-method popups, GPU overlays). Never suggested.
//! - **Utility** — known helper processes that only show transient windows (tray applets,
//!   password/permission prompts, "open with" pickers, crash reporters), and applications whose
//!   every window is tiny (smaller than 200×120 in its normal state). Listed, not suggested.
//! - **Application** — everything else. The tables are deliberately conservative: when a process
//!   might be something the user wants to restore, it stays an application.
//!
//! Matching uses lower-cased process names, executable stems and package ids; on Linux also
//! `WM_CLASS` values and on macOS bundle identifiers (reported as window classes), so windows
//! whose process is unknown can still be classified. Platform implementations already filter
//! desktop, dock and panel *window types*; these tables catch shell components that still show
//! ordinary-looking windows. They cannot recognise renamed or unknown shell components.

use reframe_platform::{OsKind, ProcessInfo, WindowInfo, WindowState};

use super::names;
use crate::model::AppCategory;

/// Windows narrower than this *and* shorter than [`TINY_HEIGHT`] are helper windows.
pub const TINY_WIDTH: i32 = 200;
/// See [`TINY_WIDTH`].
pub const TINY_HEIGHT: i32 = 120;

/// Patterns are lower-case; a trailing `*` matches any suffix.
struct Tables {
    system: &'static [&'static str],
    utility: &'static [&'static str],
}

const WINDOWS_SYSTEM: &[&str] = &[
    // Shell experience hosts (Start, search, touch keyboard, emoji panel, lock screen, widgets...)
    "textinputhost",
    "windowsinternal.composableshell.experiences.textinput.inputapp",
    "shellexperiencehost",
    "startmenuexperiencehost",
    "searchhost",
    "searchapp",
    "searchui",
    "minisearchhost",
    "lockapp",
    "logonui",
    "shellhost",
    "widgets",
    "widgetboard",
    "widgetservice",
    "peopleexperiencehost",
    "screenclippinghost",
    "crossdeviceresume",
    "tabtip",
    "ctfmon",
    "dwm",
    "sihost",
    "csrss",
    "winlogon",
    // Xbox Game Bar overlay
    "gamebar",
    "gamebarftserver",
    "gamebarpresencewriter",
    // GPU vendor overlays and helpers
    "nvidia share",
    "nvidia overlay",
    "nvidia web helper",
    "nvsphelper",
    "nvsphelper64",
    "nvcontainer",
    "amdow",
    "amdrsserv",
    "amdrssrcext",
    // Shell packages (package name of the AppUserModelID)
    "microsoft.windows.shellexperiencehost",
    "microsoft.windows.startmenuexperiencehost",
    "microsoft.windows.search",
    "microsoft.windows.cortana",
    "microsoft.windows.peopleexperiencehost",
    "microsoft.windows.contentdeliverymanager",
    "microsoft.lockapp",
    "microsoftwindows.client.webexperience",
    "microsoft.xboxgamingoverlay",
];

const WINDOWS_UTILITY: &[&str] = &[
    "securityhealthsystray",
    "onedrive",
    "openwith",
    "pickerhost",
    "credentialuibroker",
    "consent",
    "smartscreen",
    "werfault",
    "wermgr",
    "rundll32",
    "runtimebroker",
    "dataexchangehost",
    "sndvol",
    "systemsettingsadminflows",
    "msiexec",
    "taskhostw",
    "gameoverlayui",
    "microsoft.windows.apprep.chxapp",
    "microsoft.windows.capturepicker",
    "microsoft.windows.xgpuejectdialog",
];

const LINUX_SYSTEM: &[&str] = &[
    // GNOME, Cinnamon, MATE, Budgie, Unity, Pantheon
    "gnome-shell",
    "gnome-shell-calendar-server",
    "gnome-flashback",
    "gnome-panel",
    "gnome-screensaver",
    "nautilus-desktop",
    "nemo-desktop",
    "cinnamon",
    "cinnamon-screensaver",
    "mate-panel",
    "mate-screensaver",
    "mate-notification-daemon",
    "budgie-panel",
    "budgie-wm",
    "budgie-daemon",
    "unity-panel-service",
    "gala",
    "wingpanel",
    "gsd-*",
    // KDE Plasma
    "plasmashell",
    "krunner",
    "kwin",
    "kwin_x11",
    "kwin_wayland",
    "ksmserver",
    "ksmserver-logout-greeter",
    "kscreenlocker_greet",
    "ksplashqml",
    "kded5",
    "kded6",
    "xembedsniproxy",
    "lattedock",
    "latte-dock",
    // Xfce
    "xfce4-panel",
    "xfdesktop",
    "xfwm4",
    "xfce4-notifyd",
    "xfce4-screensaver",
    "wrapper-1.0",
    "wrapper-2.0",
    // LXDE / LXQt
    "lxpanel",
    "lxqt-panel",
    "lxqt-notificationd",
    "lxqt-runner",
    "lxsession",
    // Window managers, compositors and display servers
    "mutter",
    "muffin",
    "marco",
    "metacity",
    "openbox",
    "compiz",
    "fluxbox",
    "icewm",
    "jwm",
    "awesome",
    "i3",
    "i3bar",
    "sway",
    "swaybar",
    "swaybg",
    "bspwm",
    "herbstluftwm",
    "xmonad",
    "qtile",
    "enlightenment",
    "picom",
    "compton",
    "xcompmgr",
    "xwayland",
    "xorg",
    // Bars, docks and desktop widgets
    "waybar",
    "polybar",
    "tint2",
    "plank",
    "docky",
    "cairo-dock",
    "conky",
    "lemonbar",
    "xmobar",
    "dzen2",
    "trayer",
    "stalonetray",
    "eww",
    "nwg-panel",
    "nwg-dock",
    "wf-panel",
    // Notification daemons, screen lockers, input methods
    "dunst",
    "mako",
    "swaync",
    "notify-osd",
    "notification-daemon",
    "xscreensaver",
    "light-locker",
    "i3lock",
    "swaylock",
    "slock",
    "ibus-ui-gtk3",
    "ibus-x11",
    "ibus-extension-gtk3",
    "fcitx",
    "fcitx5",
];

const LINUX_UTILITY: &[&str] = &[
    // Tray applets
    "nm-applet",
    "blueman-applet",
    "blueman-tray",
    "pasystray",
    "volumeicon",
    "cbatticon",
    "xfce4-power-manager",
    "redshift-gtk",
    "gammastep-indicator",
    "xfce4-clipman",
    "parcellite",
    "diodon",
    "indicator-*",
    "update-notifier",
    "evolution-alarm-notify",
    // Authentication and passphrase prompts
    "polkit-gnome-authentication-agent-1",
    "polkit-kde-authentication-agent-1",
    "polkit-mate-authentication-agent-1",
    "lxpolkit",
    "lxqt-policykit-agent",
    "xfce-polkit",
    "budgie-polkit-dialog",
    "gcr-prompter",
    "pinentry*",
    "ssh-askpass*",
    "ksshaskpass",
    // Dialog helpers and portals (file choosers opened for sandboxed apps)
    "zenity",
    "yad",
    "kdialog",
    "xmessage",
    "gxmessage",
    "xdg-desktop-portal*",
    // On-screen keyboard
    "onboard",
];

const MACOS_SYSTEM: &[&str] = &[
    "dock",
    "systemuiserver",
    "controlcenter",
    "notificationcenter",
    "windowserver",
    "spotlight",
    "loginwindow",
    "windowmanager",
    "textinputmenuagent",
    "textinputswitcher",
    "airplayuiagent",
    "siri",
    "osduihelper",
    "cursoruiviewservice",
    "screencaptureui",
    "screensaverengine",
    "legacyscreensaver",
    "wallpaperagent",
    "controlstrip",
    "universalcontrol",
    "axvisualsupportagent",
    // Bundle identifiers
    "com.apple.dock",
    "com.apple.systemuiserver",
    "com.apple.controlcenter",
    "com.apple.notificationcenterui",
    "com.apple.spotlight",
    "com.apple.loginwindow",
    "com.apple.windowmanager",
    "com.apple.textinputmenuagent",
    "com.apple.textinputswitcher",
    "com.apple.airplayuiagent",
    "com.apple.siri",
    "com.apple.osduihelper",
    "com.apple.screencaptureui",
    "com.apple.screensaver.engine",
    "com.apple.controlstrip",
    "com.apple.universalcontrol",
    "com.apple.accessibility.axvisualsupportagent",
];

const MACOS_UTILITY: &[&str] = &[
    "securityagent",
    "coreautha",
    "coreservicesuiagent",
    "usernotificationcenter",
    "quicklookuiservice",
    "universalaccessauthwarn",
    "problem reporter",
    "pinentry-mac",
    "ssh-askpass",
    "com.apple.securityagent",
    "com.apple.coreservices.uiagent",
    "com.apple.usernotificationcenter",
    "com.apple.problemreporter",
    "com.apple.universalaccessauthwarn",
];

fn tables(os: OsKind) -> Tables {
    match os {
        OsKind::Windows => Tables {
            system: WINDOWS_SYSTEM,
            utility: WINDOWS_UTILITY,
        },
        OsKind::Linux => Tables {
            system: LINUX_SYSTEM,
            utility: LINUX_UTILITY,
        },
        OsKind::Macos => Tables {
            system: MACOS_SYSTEM,
            utility: MACOS_UTILITY,
        },
        OsKind::Other => Tables {
            system: &[],
            utility: &[],
        },
    }
}

fn matches_table(table: &[&str], signal: &str) -> bool {
    !signal.is_empty()
        && table.iter().any(|pattern| match pattern.strip_suffix('*') {
            Some(prefix) => signal.starts_with(prefix),
            None => signal == *pattern,
        })
}

/// Lower-cased names identifying a process: process name, executable stem and package ids.
fn process_signals(os: OsKind, process: &ProcessInfo, out: &mut Vec<String>) {
    out.push(names::process_name(&process.name));
    if let Some(exe) = process.executable_path.as_deref() {
        out.push(names::exe_stem(exe, os));
    }
    if let Some(package) = &process.package {
        out.extend(names::package_names(package));
    }
}

/// Lower-cased names identifying the application a window belongs to. Windows class names name
/// window *types* rather than applications, so on Windows only the AppUserModelID is used.
fn window_signals(os: OsKind, window: &WindowInfo, out: &mut Vec<String>) {
    let class = window.class_name.as_deref().map(str::trim);
    let hint = window.app_id_hint.as_deref().map(str::trim);
    match os {
        OsKind::Windows => {
            if let Some(aumid) = hint.filter(|h| !h.is_empty()) {
                out.push(aumid.to_lowercase());
                out.push(names::aumid_package_name(aumid));
            }
        }
        OsKind::Linux | OsKind::Macos => {
            out.extend(
                [class, hint]
                    .into_iter()
                    .flatten()
                    .filter(|s| !s.is_empty())
                    .map(str::to_lowercase),
            );
        }
        OsKind::Other => {}
    }
}

/// True when `process` is a known OS shell / desktop component on `os`.
pub fn is_system_process(os: OsKind, process: &ProcessInfo) -> bool {
    let mut signals = Vec::new();
    process_signals(os, process, &mut signals);
    let system = tables(os).system;
    signals.iter().any(|s| matches_table(system, s))
}

/// True when a window is too small to be an application window: narrower than
/// [`TINY_WIDTH`] and shorter than [`TINY_HEIGHT`] in its normal (restored) state. Maximized and
/// fullscreen windows are never tiny; minimized windows count only when the platform reports
/// their restored bounds (a minimized window's current bounds are meaningless on Windows).
pub fn is_tiny_window(window: &WindowInfo) -> bool {
    let bounds = match window.state {
        WindowState::Maximized | WindowState::Fullscreen => return false,
        WindowState::Minimized => match window.normal_bounds {
            Some(bounds) => bounds,
            None => return false,
        },
        WindowState::Normal => window.restore_bounds(),
    };
    bounds.width < TINY_WIDTH && bounds.height < TINY_HEIGHT
}

/// Category of an application from its owning processes and windows (see module docs).
pub fn classify(os: OsKind, processes: &[&ProcessInfo], windows: &[&WindowInfo]) -> AppCategory {
    let mut signals = Vec::new();
    for process in processes {
        process_signals(os, process, &mut signals);
    }
    for window in windows {
        window_signals(os, window, &mut signals);
    }
    let tables = tables(os);
    let known_helper = || signals.iter().any(|s| matches_table(tables.utility, s));
    let only_tiny_windows = || !windows.is_empty() && windows.iter().all(|w| is_tiny_window(w));
    if signals.iter().any(|s| matches_table(tables.system, s)) {
        AppCategory::System
    } else if known_helper() || only_tiny_windows() {
        AppCategory::Utility
    } else {
        AppCategory::Application
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reframe_platform::{PackageInfo, PackageKind, Rect, WindowId};

    fn process(name: &str, exe: Option<&str>) -> ProcessInfo {
        ProcessInfo {
            pid: 1,
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

    fn window(class: Option<&str>, width: i32, height: i32) -> WindowInfo {
        WindowInfo {
            id: WindowId::new("w"),
            pid: 1,
            title: "t".into(),
            class_name: class.map(Into::into),
            app_id_hint: None,
            bounds: Rect::new(0, 0, width, height),
            normal_bounds: None,
            state: WindowState::Normal,
            monitor_id: None,
            desktop: None,
            is_focused: false,
            z_order: 0,
        }
    }

    fn category(os: OsKind, p: &ProcessInfo, w: &WindowInfo) -> AppCategory {
        classify(os, &[p], &[w])
    }

    #[test]
    fn shell_components_are_system_per_os() {
        let big = window(None, 800, 600);
        let cases = [
            (OsKind::Windows, process("TextInputHost.exe", None)),
            (
                OsKind::Windows,
                process(
                    "x",
                    Some(
                        r"C:\Windows\SystemApps\ShellExperienceHost_cw5n1h2txyewy\ShellExperienceHost.exe",
                    ),
                ),
            ),
            (OsKind::Windows, process("NVIDIA Share.exe", None)),
            (
                OsKind::Linux,
                process("gnome-shell", Some("/usr/bin/gnome-shell")),
            ),
            (OsKind::Linux, process("xfce4-panel", None)),
            (OsKind::Linux, process("gsd-media-keys", None)),
            (OsKind::Macos, process("Dock", None)),
            (OsKind::Macos, process("SystemUIServer", None)),
        ];
        for (os, p) in cases {
            assert_eq!(
                category(os, &p, &big),
                AppCategory::System,
                "{os:?} {}",
                p.name
            );
        }
    }

    #[test]
    fn tables_are_per_os() {
        let big = window(None, 800, 600);
        // `dock` is a macOS shell component, not a Linux one.
        assert_eq!(
            category(OsKind::Linux, &process("Dock", None), &big),
            AppCategory::Application
        );
        assert_eq!(
            category(OsKind::Other, &process("gnome-shell", None), &big),
            AppCategory::Application
        );
    }

    #[test]
    fn file_managers_and_hosts_stay_applications() {
        let big = window(Some("CabinetWClass"), 800, 600);
        for name in [
            "explorer.exe",
            "ApplicationFrameHost.exe",
            "SystemSettings.exe",
        ] {
            assert_eq!(
                category(OsKind::Windows, &process(name, None), &big),
                AppCategory::Application,
                "{name}"
            );
        }
        assert_eq!(
            category(OsKind::Macos, &process("Finder", None), &big),
            AppCategory::Application
        );
        assert_eq!(
            category(OsKind::Linux, &process("nautilus", None), &big),
            AppCategory::Application
        );
    }

    #[test]
    fn window_classes_and_bundle_ids_classify_unknown_processes() {
        let stand_in = process("unknown", None);
        assert_eq!(
            category(OsKind::Linux, &stand_in, &window(Some("Conky"), 300, 300)),
            AppCategory::System
        );
        assert_eq!(
            category(
                OsKind::Macos,
                &stand_in,
                &window(Some("com.apple.notificationcenterui"), 300, 300)
            ),
            AppCategory::System
        );
        // Window class names are ignored on Windows (they name window types, not apps).
        assert_eq!(
            category(OsKind::Windows, &stand_in, &window(Some("dwm"), 300, 300)),
            AppCategory::Application
        );
        let mut aumid = window(None, 300, 300);
        aumid.app_id_hint = Some("Microsoft.LockApp_cw5n1h2txyewy!WindowsDefaultLockScreen".into());
        assert_eq!(
            category(OsKind::Windows, &stand_in, &aumid),
            AppCategory::System
        );
    }

    #[test]
    fn packages_are_matched_by_package_name() {
        let mut p = process("Widgets.exe", None);
        p.name = "something".into();
        p.package = Some(PackageInfo {
            kind: PackageKind::WindowsPackage,
            id: "MicrosoftWindows.Client.WebExperience_cw5n1h2txyewy!Widgets".into(),
            path: None,
        });
        assert_eq!(
            category(OsKind::Windows, &p, &window(None, 800, 600)),
            AppCategory::System
        );
    }

    #[test]
    fn helpers_are_utilities() {
        let big = window(None, 400, 300);
        assert_eq!(
            category(OsKind::Linux, &process("nm-applet", None), &big),
            AppCategory::Utility
        );
        assert_eq!(
            category(OsKind::Linux, &process("pinentry-gnome3", None), &big),
            AppCategory::Utility
        );
        assert_eq!(
            category(
                OsKind::Linux,
                &process(
                    "polkit-gnome-au",
                    Some("/usr/libexec/polkit-gnome-authentication-agent-1")
                ),
                &big
            ),
            AppCategory::Utility
        );
        assert_eq!(
            category(OsKind::Windows, &process("OpenWith.exe", None), &big),
            AppCategory::Utility
        );
        assert_eq!(
            category(OsKind::Macos, &process("SecurityAgent", None), &big),
            AppCategory::Utility
        );
    }

    #[test]
    fn tiny_windows() {
        assert!(is_tiny_window(&window(None, 150, 100)));
        assert!(is_tiny_window(&window(None, 1, 1)));
        assert!(!is_tiny_window(&window(None, 150, 500)));
        assert!(!is_tiny_window(&window(None, 800, 100)));
        assert!(!is_tiny_window(&window(None, 200, 100)));

        // Minimized on Windows: the current bounds are a 160x28 placeholder.
        let mut minimized = window(None, 160, 28);
        minimized.state = WindowState::Minimized;
        assert!(!is_tiny_window(&minimized));
        minimized.normal_bounds = Some(Rect::new(0, 0, 900, 700));
        assert!(!is_tiny_window(&minimized));
        minimized.normal_bounds = Some(Rect::new(0, 0, 100, 50));
        assert!(is_tiny_window(&minimized));

        let mut maximized = window(None, 150, 100);
        maximized.state = WindowState::Maximized;
        assert!(!is_tiny_window(&maximized));
    }

    #[test]
    fn utility_when_every_window_is_tiny() {
        let p = process("xeyes", None);
        let tiny = window(None, 150, 100);
        let big = window(None, 800, 600);
        assert_eq!(
            classify(OsKind::Linux, &[&p], &[&tiny]),
            AppCategory::Utility
        );
        assert_eq!(
            classify(OsKind::Linux, &[&p], &[&tiny, &big]),
            AppCategory::Application
        );
        assert_eq!(
            classify(OsKind::Linux, &[&p], &[]),
            AppCategory::Application
        );
    }

    #[test]
    fn is_system_process_matches_tables() {
        assert!(is_system_process(
            OsKind::Linux,
            &process("plasmashell", None)
        ));
        assert!(!is_system_process(OsKind::Linux, &process("firefox", None)));
        assert!(is_system_process(
            OsKind::Windows,
            &process("SearchHost.exe", None)
        ));
    }
}
