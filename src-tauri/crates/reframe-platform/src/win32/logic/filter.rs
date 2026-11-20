//! Which top-level windows are "application windows", window id encoding, and UWP host
//! resolution.

/// `WS_CHILD`.
pub const WS_CHILD: u32 = 0x4000_0000;
/// `WS_EX_TOOLWINDOW`: palettes, floating toolbars; not shown in the taskbar.
pub const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
/// `WS_EX_APPWINDOW`: forces a taskbar button even for owned or tool windows.
pub const WS_EX_APPWINDOW: u32 = 0x0004_0000;
/// `WS_EX_NOACTIVATE`: on-screen keyboards, notification toasts and similar overlays.
pub const WS_EX_NOACTIVATE: u32 = 0x0800_0000;

/// Window class of the frame that hosts UWP apps (owned by `ApplicationFrameHost.exe`).
pub const UWP_FRAME_CLASS: &str = "ApplicationFrameWindow";
/// Window class of the UWP app's own window, a child of [`UWP_FRAME_CLASS`] while visible.
pub const UWP_CORE_CLASS: &str = "Windows.UI.Core.CoreWindow";

/// Desktop, taskbar and shell-overlay classes that are never application windows.
const SHELL_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    // Shell overlays (tray overflow, Start/Task View/Search hosts, input switcher). They are
    // usually cloaked or tool windows already; listed so they never leak into layouts.
    "NotifyIconOverflowWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "XamlExplorerHostIslandWindow",
    "Shell_InputSwitchTopLevelWindow",
    "MultitaskingViewFrame",
    "ForegroundStaging",
    "EdgeUiInputTopWndClass",
    // A *top-level* CoreWindow is a shell experience (Start, Search, Action Center); UWP apps
    // show up as their ApplicationFrameWindow host instead.
    UWP_CORE_CLASS,
];

/// Everything the filter needs to know about a top-level window.
#[derive(Debug, Clone, Copy)]
pub struct WindowFacts<'a> {
    pub visible: bool,
    /// `DWMWA_CLOAKED` is non-zero: hidden by the shell (another virtual desktop, suspended UWP
    /// app, ...) although `IsWindowVisible` says otherwise.
    pub cloaked: bool,
    pub style: u32,
    pub ex_style: u32,
    /// `GetWindow(GW_OWNER)` returned a window (dialogs, tool palettes, popups).
    pub has_owner: bool,
    pub title: &'a str,
    pub class_name: &'a str,
    pub width: i32,
    pub height: i32,
}

/// The [`crate::WindowInfo`] filtering contract for Win32: visible, not cloaked, not a tool,
/// owned or no-activate window (unless it forces a taskbar button with `WS_EX_APPWINDOW`), has a
/// title, is not part of the shell, and has a non-zero size.
pub fn is_application_window(facts: &WindowFacts<'_>) -> bool {
    if !facts.visible || facts.cloaked || facts.style & WS_CHILD != 0 {
        return false;
    }
    let forced_app_window = facts.ex_style & WS_EX_APPWINDOW != 0;
    if !forced_app_window
        && (facts.ex_style & (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE) != 0 || facts.has_owner)
    {
        return false;
    }
    if facts.title.trim().is_empty() {
        return false;
    }
    if SHELL_CLASSES
        .iter()
        .any(|class| class.eq_ignore_ascii_case(facts.class_name))
    {
        return false;
    }
    facts.width > 0 && facts.height > 0
}

/// Window ids are the HWND value in lowercase hex with a `0x` prefix, e.g. `0x1a2b3c`.
pub fn format_window_id(handle: isize) -> String {
    format!("0x{:x}", handle as usize)
}

/// Parse a window id produced by [`format_window_id`]. Accepts surrounding whitespace, either
/// prefix case and no prefix at all (the digits are always hexadecimal). Rejects empty, zero,
/// negative, non-hex and out-of-range values.
pub fn parse_window_id(id: &str) -> Option<isize> {
    let text = id.trim();
    let digits = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text);
    if digits.is_empty() || digits.len() > 16 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let value = usize::try_from(u64::from_str_radix(digits, 16).ok()?).ok()?;
    (value != 0).then_some(value as isize)
}

/// The process that really owns a UWP window. `ApplicationFrameWindow` frames belong to
/// `ApplicationFrameHost.exe`; while the app is visible its `Windows.UI.Core.CoreWindow` child
/// belongs to the app process. Falls back to the frame's process when no such child exists
/// (e.g. while the app is minimized and its core window has been re-parented).
pub fn resolve_uwp_pid(frame_pid: u32, children: &[(String, u32)]) -> u32 {
    children
        .iter()
        .find(|(class, pid)| class == UWP_CORE_CLASS && *pid != 0 && *pid != frame_pid)
        .map_or(frame_pid, |(_, pid)| *pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OVERLAPPED_WINDOW: u32 = 0x10CF_0000; // WS_OVERLAPPEDWINDOW | WS_VISIBLE

    fn app() -> WindowFacts<'static> {
        WindowFacts {
            visible: true,
            cloaked: false,
            style: OVERLAPPED_WINDOW,
            ex_style: 0x0000_0100, // WS_EX_WINDOWEDGE
            has_owner: false,
            title: "Untitled - Notepad",
            class_name: "Notepad",
            width: 800,
            height: 600,
        }
    }

    #[test]
    fn ordinary_app_window_is_listed() {
        assert!(is_application_window(&app()));
    }

    #[test]
    fn invisible_cloaked_and_child_windows_are_skipped() {
        assert!(!is_application_window(&WindowFacts {
            visible: false,
            ..app()
        }));
        assert!(!is_application_window(&WindowFacts {
            cloaked: true,
            ..app()
        }));
        assert!(!is_application_window(&WindowFacts {
            style: OVERLAPPED_WINDOW | WS_CHILD,
            ..app()
        }));
    }

    #[test]
    fn tool_owned_and_noactivate_windows_need_appwindow() {
        for facts in [
            WindowFacts {
                ex_style: WS_EX_TOOLWINDOW,
                ..app()
            },
            WindowFacts {
                has_owner: true,
                ..app()
            },
            WindowFacts {
                ex_style: WS_EX_NOACTIVATE,
                ..app()
            },
        ] {
            assert!(!is_application_window(&facts), "{facts:?}");
            assert!(
                is_application_window(&WindowFacts {
                    ex_style: facts.ex_style | WS_EX_APPWINDOW,
                    ..facts
                }),
                "{facts:?} with WS_EX_APPWINDOW"
            );
        }
    }

    #[test]
    fn untitled_shell_and_empty_windows_are_skipped() {
        assert!(!is_application_window(&WindowFacts {
            title: "   ",
            ..app()
        }));
        for class in [
            "Progman",
            "WorkerW",
            "Shell_TrayWnd",
            "shell_secondarytraywnd",
            UWP_CORE_CLASS,
        ] {
            assert!(
                !is_application_window(&WindowFacts {
                    class_name: class,
                    ..app()
                }),
                "{class}"
            );
        }
        assert!(!is_application_window(&WindowFacts { width: 0, ..app() }));
        assert!(!is_application_window(&WindowFacts {
            height: -3,
            ..app()
        }));
        // UWP frames are application windows.
        assert!(is_application_window(&WindowFacts {
            class_name: UWP_FRAME_CLASS,
            title: "Calculator",
            ..app()
        }));
    }

    #[test]
    fn window_ids_round_trip() {
        for handle in [1isize, 0x1a2b3c, 0x7fff_ffff, -1, -0x7fff_0000, isize::MAX] {
            let id = format_window_id(handle);
            assert!(id.starts_with("0x"), "{id}");
            assert_eq!(parse_window_id(&id), Some(handle), "{id}");
        }
        assert_eq!(format_window_id(0x1A2B), "0x1a2b");
    }

    #[test]
    fn window_id_parsing_is_lenient_but_strict_about_content() {
        assert_eq!(parse_window_id(" 0X1A2B \n"), Some(0x1a2b));
        assert_eq!(parse_window_id("1a2b"), Some(0x1a2b));
        for bad in [
            "",
            "0x",
            "0",
            "0x0",
            "-0x10",
            "+10",
            "0xZZ",
            "12 34",
            "0x1_000",
            "0x10000000000000000",
        ] {
            assert_eq!(parse_window_id(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn uwp_pid_points_at_the_app_process() {
        let children = vec![
            ("ApplicationFrameTitleBarWindow".to_string(), 100),
            (UWP_CORE_CLASS.to_string(), 4242),
        ];
        assert_eq!(resolve_uwp_pid(100, &children), 4242);
        // Core window owned by the host itself, or missing: keep the host pid.
        assert_eq!(
            resolve_uwp_pid(100, &[(UWP_CORE_CLASS.to_string(), 100)]),
            100
        );
        assert_eq!(
            resolve_uwp_pid(100, &[(UWP_CORE_CLASS.to_string(), 0)]),
            100
        );
        assert_eq!(resolve_uwp_pid(100, &[]), 100);
    }
}
