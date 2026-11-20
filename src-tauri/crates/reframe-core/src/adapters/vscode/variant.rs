//! The VS Code family — Visual Studio Code, VS Code Insiders and VSCodium — and how each one
//! is recognised (executables, bundle/package ids) and found (locate queries).

use reframe_platform::{LocateQuery, OsKind, PackageInfo, PackageKind, ProcessInfo, WindowInfo};

use super::paths;
use crate::adapters::AppIdentity;

/// One member of the VS Code family. Each variant is a separate application with its own
/// executable, user-data directory and window-title product name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Variant {
    /// Visual Studio Code.
    Stable,
    /// Visual Studio Code - Insiders.
    Insiders,
    /// VSCodium.
    Codium,
}

impl Variant {
    pub const ALL: [Variant; 3] = [Variant::Stable, Variant::Insiders, Variant::Codium];

    /// Logical application id stored in workspaces.
    pub fn app_id(self) -> &'static str {
        match self {
            Variant::Stable => "vscode",
            Variant::Insiders => "vscode-insiders",
            Variant::Codium => "vscodium",
        }
    }

    pub fn from_app_id(app_id: &str) -> Option<Self> {
        let app_id = app_id.trim();
        Self::ALL
            .into_iter()
            .find(|v| v.app_id().eq_ignore_ascii_case(app_id))
    }

    /// Product name (`nameLong` in VS Code's product.json): shown to users and used as
    /// `${appName}` in window titles.
    pub fn display_name(self) -> &'static str {
        match self {
            Variant::Stable => "Visual Studio Code",
            Variant::Insiders => "Visual Studio Code - Insiders",
            Variant::Codium => "VSCodium",
        }
    }

    /// Short name for messages.
    pub fn short_name(self) -> &'static str {
        match self {
            Variant::Stable => "VS Code",
            Variant::Insiders => "VS Code Insiders",
            Variant::Codium => "VSCodium",
        }
    }

    /// `nameShort`: the name of the user-data folder (`~/.config/Code`).
    pub fn data_folder_name(self) -> &'static str {
        match self {
            Variant::Stable => "Code",
            Variant::Insiders => "Code - Insiders",
            Variant::Codium => "VSCodium",
        }
    }

    /// `applicationName`: the CLI name, also used for the macOS portable-data folder.
    pub fn application_name(self) -> &'static str {
        match self {
            Variant::Stable => "code",
            Variant::Insiders => "code-insiders",
            Variant::Codium => "codium",
        }
    }

    /// Main executable file name on Windows.
    pub fn windows_executable(self) -> &'static str {
        match self {
            Variant::Stable => "Code.exe",
            Variant::Insiders => "Code - Insiders.exe",
            Variant::Codium => "VSCodium.exe",
        }
    }

    /// Main executable file names on Linux and other Unix systems.
    pub fn unix_executables(self) -> &'static [&'static str] {
        match self {
            Variant::Stable => &["code"],
            Variant::Insiders => &["code-insiders"],
            Variant::Codium => &["codium", "vscodium"],
        }
    }

    /// Current macOS bundle identifier.
    pub fn bundle_id(self) -> &'static str {
        match self {
            Variant::Stable => "com.microsoft.VSCode",
            Variant::Insiders => "com.microsoft.VSCodeInsiders",
            Variant::Codium => "com.vscodium",
        }
    }

    /// macOS bundle identifiers (the first is current; VSCodium used the second before 2020).
    pub fn bundle_ids(self) -> &'static [&'static str] {
        match self {
            Variant::Stable => &["com.microsoft.VSCode"],
            Variant::Insiders => &["com.microsoft.VSCodeInsiders"],
            Variant::Codium => &["com.vscodium", "com.visualstudio.code.oss"],
        }
    }

    /// Default macOS application bundle name.
    pub fn mac_app_name(self) -> &'static str {
        match self {
            Variant::Stable => "Visual Studio Code.app",
            Variant::Insiders => "Visual Studio Code - Insiders.app",
            Variant::Codium => "VSCodium.app",
        }
    }

    pub fn flatpak_ids(self) -> &'static [&'static str] {
        match self {
            Variant::Stable => &["com.visualstudio.code"],
            Variant::Insiders => &["com.visualstudio.code.insiders"],
            Variant::Codium => &["com.vscodium.codium"],
        }
    }

    pub fn snap_names(self) -> &'static [&'static str] {
        match self {
            Variant::Stable => &["code"],
            Variant::Insiders => &["code-insiders"],
            Variant::Codium => &["codium"],
        }
    }

    /// Linux desktop entry ids: distribution packages, Flatpak, Snap and AUR builds.
    pub fn desktop_ids(self) -> &'static [&'static str] {
        match self {
            Variant::Stable => &[
                "code.desktop",
                "com.visualstudio.code.desktop",
                "code_code.desktop",
                "visual-studio-code.desktop",
            ],
            Variant::Insiders => &[
                "code-insiders.desktop",
                "com.visualstudio.code.insiders.desktop",
                "code-insiders_code-insiders.desktop",
                "visual-studio-code-insiders.desktop",
            ],
            Variant::Codium => &[
                "codium.desktop",
                "com.vscodium.codium.desktop",
                "codium_codium.desktop",
                "vscodium.desktop",
                "vscodium-bin.desktop",
            ],
        }
    }

    /// Directory names the variant is installed into on Linux (`/usr/share/code`,
    /// `/opt/visual-studio-code`...). Used to recognise helper executables such as
    /// `chrome_crashpad_handler` that live next to the main binary.
    fn linux_install_dirs(self) -> &'static [&'static str] {
        match self {
            Variant::Stable => &["code", "visual-studio-code", "vscode"],
            Variant::Insiders => &["code-insiders", "visual-studio-code-insiders"],
            Variant::Codium => &["codium", "vscodium", "vscodium-bin"],
        }
    }

    pub fn identity(self) -> AppIdentity {
        AppIdentity {
            app_id: self.app_id().to_string(),
            display_name: self.display_name().to_string(),
        }
    }
}

/// Which VS Code variant `process` belongs to, if any.
///
/// Identification uses, in order: the package identity reported by the platform (macOS
/// bundle id, Flatpak id, Snap name), the executable path, and — only when the executable
/// path is not readable — the process name. Helper processes (renderer, GPU, utility,
/// crashpad) run the same executable on Windows and Linux, or live inside the application
/// bundle on macOS, so they are attributed to the same variant even though they own no
/// windows. On macOS the windows' bundle identifier is a last resort.
pub fn identify(process: &ProcessInfo, windows: &[&WindowInfo], os: OsKind) -> Option<Variant> {
    if let Some(variant) = process.package.as_ref().and_then(from_package) {
        return Some(variant);
    }
    let by_process = match &process.executable_path {
        Some(exe) => from_executable(&exe.to_string_lossy(), os),
        None => from_process_name(&process.name, os),
    };
    by_process.or_else(|| {
        if os != OsKind::Macos {
            return None;
        }
        windows.iter().find_map(|w| {
            w.class_name
                .as_deref()
                .and_then(from_bundle_id)
                .or_else(|| w.app_id_hint.as_deref().and_then(from_bundle_id))
        })
    })
}

fn from_package(package: &PackageInfo) -> Option<Variant> {
    let id = package.id.trim();
    match package.kind {
        PackageKind::MacBundle => from_bundle_id(id),
        PackageKind::Flatpak => Variant::ALL.into_iter().find(|v| {
            v.flatpak_ids()
                .iter()
                .any(|known| known.eq_ignore_ascii_case(id))
        }),
        PackageKind::Snap => Variant::ALL
            .into_iter()
            .find(|v| v.snap_names().contains(&id)),
        // Identified by their executable instead.
        PackageKind::WindowsPackage | PackageKind::AppImage => None,
    }
}

/// Bundle id of the application or of one of its helpers (`com.microsoft.VSCode.helper.GPU`).
fn from_bundle_id(id: &str) -> Option<Variant> {
    let id = id.trim().as_bytes();
    Variant::ALL.into_iter().find(|v| {
        v.bundle_ids().iter().any(|known| {
            let known = known.as_bytes();
            id.len() >= known.len()
                && id[..known.len()].eq_ignore_ascii_case(known)
                && (id.len() == known.len() || id[known.len()] == b'.')
        })
    })
}

fn from_executable(path: &str, os: OsKind) -> Option<Variant> {
    match os {
        OsKind::Windows => {
            let name = paths::file_name(path, os);
            Variant::ALL
                .into_iter()
                .find(|v| name.eq_ignore_ascii_case(v.windows_executable()))
        }
        OsKind::Macos => {
            // Anything inside the application bundle: the main executable
            // (`Contents/MacOS/Electron`) as well as helper apps and crashpad.
            let lower = path.to_lowercase();
            [Variant::Insiders, Variant::Stable, Variant::Codium]
                .into_iter()
                .find(|v| lower.contains(&format!("/{}/", v.mac_app_name().to_lowercase())))
        }
        OsKind::Linux | OsKind::Other => {
            let name = paths::file_name(path, os);
            if let Some(variant) = Variant::ALL
                .into_iter()
                .find(|v| v.unix_executables().contains(&name))
            {
                return Some(variant);
            }
            if name == "chrome_crashpad_handler" {
                let dir = paths::file_name(paths::parent(path, os)?, os);
                return Variant::ALL
                    .into_iter()
                    .find(|v| v.linux_install_dirs().contains(&dir));
            }
            None
        }
    }
}

fn from_process_name(name: &str, os: OsKind) -> Option<Variant> {
    let name = name.trim();
    match os {
        OsKind::Windows => Variant::ALL
            .into_iter()
            .find(|v| name.eq_ignore_ascii_case(v.windows_executable())),
        OsKind::Linux | OsKind::Other => Variant::ALL
            .into_iter()
            .find(|v| v.unix_executables().contains(&name)),
        // The main process is called "Electron" or "Code" on macOS: not distinctive.
        OsKind::Macos => None,
    }
}

/// How to find `variant` on a machine running `os`.
pub fn locate_query(variant: Variant, os: OsKind) -> LocateQuery {
    let executable_names = match os {
        OsKind::Windows => vec![variant.windows_executable().to_string()],
        OsKind::Macos | OsKind::Linux | OsKind::Other => variant
            .unix_executables()
            .iter()
            .map(|s| s.to_string())
            .collect(),
    };
    LocateQuery {
        executable_names,
        bundle_ids: variant.bundle_ids().iter().map(|s| s.to_string()).collect(),
        desktop_ids: variant
            .desktop_ids()
            .iter()
            .map(|s| s.to_string())
            .collect(),
        display_names: vec![variant.display_name().to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reframe_platform::{Rect, WindowId, WindowState};

    fn process(exe: Option<&str>, name: &str, package: Option<PackageInfo>) -> ProcessInfo {
        ProcessInfo {
            pid: 7,
            parent_pid: None,
            name: name.into(),
            product_name: None,
            executable_path: exe.map(Into::into),
            command_line: Vec::new(),
            working_directory: None,
            start_time: None,
            package,
        }
    }

    fn package(kind: PackageKind, id: &str) -> Option<PackageInfo> {
        Some(PackageInfo {
            kind,
            id: id.into(),
            path: None,
        })
    }

    fn exe(path: &str, os: OsKind) -> Option<Variant> {
        identify(&process(Some(path), "", None), &[], os)
    }

    #[test]
    fn app_ids_round_trip() {
        for v in Variant::ALL {
            assert_eq!(Variant::from_app_id(v.app_id()), Some(v));
        }
        assert_eq!(Variant::from_app_id(" VSCode "), Some(Variant::Stable));
        assert_eq!(Variant::from_app_id("exe:code"), None);
        assert_eq!(
            Variant::Insiders.identity(),
            AppIdentity {
                app_id: "vscode-insiders".into(),
                display_name: "Visual Studio Code - Insiders".into()
            }
        );
    }

    #[test]
    fn identifies_windows_executables() {
        let w = OsKind::Windows;
        assert_eq!(
            exe(
                r"C:\Users\me\AppData\Local\Programs\Microsoft VS Code\Code.exe",
                w
            ),
            Some(Variant::Stable)
        );
        assert_eq!(
            exe(r"C:\Program Files\Microsoft VS Code\code.EXE", w),
            Some(Variant::Stable)
        );
        assert_eq!(
            exe(
                r"C:\Program Files\Microsoft VS Code Insiders\Code - Insiders.exe",
                w
            ),
            Some(Variant::Insiders)
        );
        assert_eq!(
            exe(r"C:\Program Files\VSCodium\VSCodium.exe", w),
            Some(Variant::Codium)
        );
        assert_eq!(
            exe(r"\\?\D:\Portable\VSCode\Code.exe", w),
            Some(Variant::Stable)
        );
        assert_eq!(exe(r"C:\Program Files\cursor\Cursor.exe", w), None);
        assert_eq!(
            exe(r"C:\Program Files\Microsoft VS Code\bin\code-tunnel.exe", w),
            None
        );
        // Helpers (renderer, GPU, crashpad) run Code.exe with --type=...: same application.
        let mut helper = process(
            Some(r"C:\Program Files\Microsoft VS Code\Code.exe"),
            "Code.exe",
            None,
        );
        helper.command_line = vec!["Code.exe".into(), "--type=crashpad-handler".into()];
        assert_eq!(identify(&helper, &[], w), Some(Variant::Stable));
        // Executable not readable: fall back to the image name.
        assert_eq!(
            identify(&process(None, "Code - Insiders.exe", None), &[], w),
            Some(Variant::Insiders)
        );
    }

    #[test]
    fn identifies_linux_executables_and_packages() {
        let l = OsKind::Linux;
        for (path, variant) in [
            ("/usr/share/code/code", Variant::Stable),
            ("/opt/visual-studio-code/code", Variant::Stable),
            ("/snap/code/176/usr/share/code/code", Variant::Stable),
            ("/usr/share/code-insiders/code-insiders", Variant::Insiders),
            ("/usr/share/codium/codium", Variant::Codium),
            ("/opt/vscodium-bin/codium", Variant::Codium),
            (
                "/tmp/.mount_VSCodiAbc/usr/share/codium/codium",
                Variant::Codium,
            ),
            ("/usr/share/code/chrome_crashpad_handler", Variant::Stable),
            ("/opt/vscodium-bin/chrome_crashpad_handler", Variant::Codium),
        ] {
            assert_eq!(exe(path, l), Some(variant), "{path}");
        }
        for path in [
            "/usr/lib/chromium/chrome_crashpad_handler",
            "/usr/bin/codex",
            "/home/me/.vscode-server/bin/abc123/node",
            "/usr/share/code/bin/code-tunnel",
        ] {
            assert_eq!(exe(path, l), None, "{path}");
        }
        // Flatpak and Snap identities win over the binary name inside the sandbox.
        assert_eq!(
            identify(
                &process(
                    Some("/app/extra/vscode/code"),
                    "code",
                    package(PackageKind::Flatpak, "com.visualstudio.code")
                ),
                &[],
                l
            ),
            Some(Variant::Stable)
        );
        assert_eq!(
            identify(
                &process(
                    Some("/app/share/codium/codium"),
                    "codium",
                    package(PackageKind::Flatpak, "com.vscodium.codium")
                ),
                &[],
                l
            ),
            Some(Variant::Codium)
        );
        assert_eq!(
            identify(
                &process(None, "code", package(PackageKind::Snap, "code-insiders")),
                &[],
                l
            ),
            Some(Variant::Insiders)
        );
        // A readable executable is authoritative: a script whose process is named "code"
        // is not VS Code.
        assert_eq!(
            identify(&process(Some("/usr/bin/python3"), "code", None), &[], l),
            None
        );
        assert_eq!(
            identify(&process(None, "code", None), &[], l),
            Some(Variant::Stable)
        );
    }

    #[test]
    fn identifies_macos_bundles() {
        let m = OsKind::Macos;
        for (id, variant) in [
            ("com.microsoft.VSCode", Some(Variant::Stable)),
            (
                "com.microsoft.VSCode.helper.Renderer",
                Some(Variant::Stable),
            ),
            ("com.microsoft.VSCodeInsiders", Some(Variant::Insiders)),
            ("com.vscodium", Some(Variant::Codium)),
            ("com.vscodium.helper", Some(Variant::Codium)),
            ("com.microsoft.VSCodeX", None),
            ("com.apple.Terminal", None),
        ] {
            assert_eq!(
                identify(
                    &process(None, "Electron", package(PackageKind::MacBundle, id)),
                    &[],
                    m
                ),
                variant,
                "{id}"
            );
        }
        for (path, variant) in [
            (
                "/Applications/Visual Studio Code.app/Contents/MacOS/Electron",
                Some(Variant::Stable),
            ),
            (
                "/Applications/Visual Studio Code - Insiders.app/Contents/MacOS/Electron",
                Some(Variant::Insiders),
            ),
            (
                "/Users/me/Applications/VSCodium.app/Contents/Frameworks/Code Helper (GPU).app/Contents/MacOS/Code Helper (GPU)",
                Some(Variant::Codium),
            ),
            (
                "/Applications/Visual Studio Code.app/Contents/Frameworks/Electron Framework.framework/Helpers/chrome_crashpad_handler",
                Some(Variant::Stable),
            ),
            ("/Applications/Xcode.app/Contents/MacOS/Xcode", None),
        ] {
            assert_eq!(exe(path, m), variant, "{path}");
        }
        // No readable process details: the window's bundle identifier decides.
        let window = WindowInfo {
            id: WindowId::new("1"),
            pid: 7,
            title: "reframe".into(),
            class_name: Some("com.microsoft.VSCode".into()),
            app_id_hint: None,
            bounds: Rect::new(0, 0, 10, 10),
            normal_bounds: None,
            state: WindowState::Normal,
            monitor_id: None,
            desktop: None,
            is_focused: false,
            z_order: 0,
        };
        assert_eq!(
            identify(&process(None, "Electron", None), &[&window], m),
            Some(Variant::Stable)
        );
        assert_eq!(
            identify(&process(None, "Electron", None), &[&window], OsKind::Linux),
            None
        );
    }

    #[test]
    fn locate_queries_are_os_specific() {
        let q = locate_query(Variant::Stable, OsKind::Windows);
        assert_eq!(q.executable_names, vec!["Code.exe".to_string()]);
        assert_eq!(q.bundle_ids, vec!["com.microsoft.VSCode".to_string()]);
        assert!(q.desktop_ids.contains(&"code.desktop".to_string()));
        assert!(
            q.desktop_ids
                .contains(&"com.visualstudio.code.desktop".to_string())
        );
        assert_eq!(q.display_names, vec!["Visual Studio Code".to_string()]);

        let q = locate_query(Variant::Insiders, OsKind::Linux);
        assert_eq!(q.executable_names, vec!["code-insiders".to_string()]);
        assert!(q.desktop_ids.contains(&"code-insiders.desktop".to_string()));

        let q = locate_query(Variant::Codium, OsKind::Linux);
        assert_eq!(q.executable_names, vec!["codium", "vscodium"]);
        assert!(q.desktop_ids.contains(&"codium.desktop".to_string()));
        assert_eq!(q.bundle_ids[0], "com.vscodium");
        assert_eq!(q.display_names, vec!["VSCodium".to_string()]);
    }
}
