# Platform support and limitations

Reframe is one codebase with native integrations per operating system. Operating systems do not
offer identical capabilities, and Reframe reports what it can and cannot do instead of pretending.
The app shows the live capability report under **Settings → This computer**, and every
application shows its restore level (**Full restore**, **Partial**, **Launch only**,
**Not supported**).

## Summary

| Capability | Windows 10/11 | macOS 12+ | Linux X11 | Linux Wayland |
|---|---|---|---|---|
| List application windows | ✅ | ✅ (titles need Accessibility) | ✅ | ❌ compositor restriction¹ |
| Read window position, size, state | ✅ | ✅ (needs Accessibility) | ✅ | ❌¹ |
| Move/resize, maximize, minimize | ✅ | ✅ (needs Accessibility; no true maximize) | ✅ (window manager may adjust) | ❌¹ |
| Close windows gracefully | ✅ `WM_CLOSE` | ✅ window close button (Accessibility) | ✅ `_NET_CLOSE_WINDOW` | ❌¹ |
| Monitors (layout, work area, scale) | ✅ | ✅ | ✅ RandR | partial |
| Launch applications | ✅ incl. Store apps | ✅ incl. app bundles | ✅ incl. Flatpak/Snap/AppImage | ✅ |
| Process details (command line, folder) | ✅ (not elevated processes) | ✅ (own user) | ✅ (own user) | ✅ |
| Application icons | ✅ | ✅ | ✅ PNG icons | ✅ (via `.desktop`) |
| Start at login | ✅ | ✅ Launch Agent | ✅ XDG autostart | ✅ |

¹ Wayland compositors do not let applications inspect or arrange other applications' windows.
Under a Wayland session Reframe can still launch every app of a workspace (and restore app state
such as VS Code projects, browser tabs and terminal folders), but cannot list or position windows.
If Reframe runs through XWayland it sees and arranges X11/XWayland windows only.

**Nothing is ever force-quit on any platform.** Closing is always a normal close request; apps can
ask you to save, and windows that stay open are reported.

## Verification status of this release

| Platform | How it was verified |
|---|---|
| Linux X11 | Unit tests plus integration tests against **real windows** (Xvfb + openbox + xterm): listing, moving, maximize/minimize, focus, graceful close, RandR multi-monitor; end-to-end capture → close → restore test |
| Windows | Type-checked and linted for Windows; unit tests and a window smoke test (launch, find, move, close) **run under Wine** — not yet on real Windows hardware |
| macOS | Type-checked and linted for Apple Silicon and Intel; pure logic unit-tested — **not yet run on a real Mac** |

Please report anything that misbehaves on real Windows or macOS hardware.

## Windows

- Coordinates are physical pixels (Reframe is per-monitor DPI aware).
- Only windows on the **current virtual desktop** are listed.
- Windows of programs running **as administrator** cannot be moved or closed by a non-elevated
  Reframe (Windows UIPI); Reframe reports this instead of failing silently.
- Store (MSIX) apps are launched through their AppUserModelID.
- Monitor ids come from the display configuration (stable across reboots) when available.

## macOS

- No permission is needed to list apps and on-screen windows or to reopen apps. The optional
  **Accessibility** permission is required to read window titles and to move, minimize and close
  windows; Reframe shows how to grant it. Reframe never asks for **Screen Recording** (if it was
  granted anyway, its window titles are used).
- Window contents are never captured. Apple Events (**Automation**) are sent only when you save
  a workspace containing Finder, Preview or Skim, or restore a Terminal/iTerm2 profile; just
  opening Explore never triggers an Automation prompt.
- Only windows on the **current Space** are listed (no public API for other Spaces). Windows of
  hidden apps (⌘H) are not listed; minimized windows are listed when Accessibility is granted.
- macOS has no true "maximize": a window that fills the screen's visible area is treated as
  maximized and restored that way. Full screen uses the app's own full-screen mode where supported.
- Focus is best effort (macOS 14+ limits apps activating other apps).
- Apps are launched through Launch Services (`open`); native apps may ignore extra arguments.
- Finder, Preview and Skim integrations use AppleScript and need the **Automation** permission.

## Linux

- **X11** is fully supported with EWMH-compliant window managers. Window managers may round sizes
  (e.g. terminals to character cells) or keep windows on screen.
- X11 does not expose a window's "restore" geometry while it is maximized or minimized.
- **Wayland**: see the note above. **XWayland**: only X11 clients are visible.
- Windows on other virtual desktops are not treated as minimized; Reframe does not switch your
  desktop when arranging windows.
- Icons: PNG icons from the window or the `hicolor` theme; SVG-only apps fall back to initials.

## Application integrations

| Application | Restores | Limits |
|---|---|---|
| VS Code / Insiders / VSCodium | Project folders and `.code-workspace` files; editors, terminals and unsaved changes via VS Code's own session restore | Folder detection uses window titles + VS Code's recent list; custom `window.title` settings or two folders with the same name can confuse it; remote (SSH/WSL/containers) windows are not reopened |
| Chrome, Edge, Brave, Chromium | Windows and tabs (from the browser's local session file — optional, Settings → Browser tabs) | Pinned state, tab groups and back/forward history are not restored; very recent tabs may be missing (browsers save a few seconds after changes); one profile per entry; on Windows the browser may lock its current session file, in which case Reframe says so instead of using stale tabs |
| Terminals | Windows, working directories, profiles; startup commands **only if you add and approve them** | Running programs, history and scrollback are never replayed; multi-window terminals cannot map tabs to windows exactly |
| File managers | Open folders (Explorer via the Shell API; Finder via AppleScript; Linux file managers via window titles) | Linux: folder names in titles must be unambiguous; special locations (This PC, Recycle Bin) are skipped |
| PDF readers | Documents; pages for SumatraPDF, Okular and Acrobat | Pages are saved by readers mostly on close, so they can lag behind |
| Music apps | The app; a chosen playlist for Spotify | Playback position, queue and account state are not accessible |
| Any other app | Launch + window position, size, state; documents passed on its command line | Internal state only if the app restores it itself |

## Import / export

- Exports separate the logical workspace from machine-specific paths and are sanitized
  (credentials in URLs, token-like parameters and secret-looking command-line values are removed).
- Imports are validated strictly (size, structure, limits, references). Imported terminal startup
  commands always arrive switched off, and custom launch arguments are removed for safety.
- Network locations (`\\server\share`, `/net/…`) in imported workspaces are never probed
  automatically — on Windows, merely checking one would send your credentials to that server.
