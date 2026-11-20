//! Detect how a process was packaged (Flatpak, Snap, AppImage) from `/proc`.
//!
//! Limitations: `/proc/<pid>/root` and `/proc/<pid>/environ` are only readable for the
//! current user's own processes, so packages of other users' processes are not detected.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::{PackageInfo, PackageKind};

/// `/proc/<pid>/environ` is read up to this many bytes.
const MAX_ENVIRON: u64 = 256 * 1024;
/// `.flatpak-info` is read up to this many bytes.
const MAX_FLATPAK_INFO: u64 = 64 * 1024;

/// Snaps that provide runtimes rather than applications; a process running from one of them
/// belongs to the snap named in its environment instead.
fn is_base_snap(name: &str) -> bool {
    name == "core"
        || name == "snapd"
        || name == "bare"
        || name
            .strip_prefix("core")
            .is_some_and(|v| v.bytes().all(|b| b.is_ascii_digit()))
}

/// The application id from the `[Application]` group of a `.flatpak-info` file.
pub(crate) fn flatpak_info_app_id(content: &str) -> Option<String> {
    let mut in_application = false;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_application = line == "[Application]";
            continue;
        }
        if in_application
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "name"
        {
            let value = value.trim();
            return (!value.is_empty()).then(|| value.to_string());
        }
    }
    None
}

/// The snap name for an executable under `/snap/<name>/<revision>/...`.
pub(crate) fn snap_from_executable(path: &Path) -> Option<String> {
    let rest = path.strip_prefix("/snap").ok()?;
    let mut components = rest.components();
    let name = components.next()?.as_os_str().to_str()?;
    // `/snap/bin/<name>` launchers are not snap contents.
    if name == "bin" || name.is_empty() || components.next().is_none() {
        return None;
    }
    Some(name.to_string())
}

/// Whether an executable looks like it runs from an AppImage (mounted `squashfs` under
/// `$TMPDIR/.mount_*`, or an `--appimage-extract-and-run` directory), or is an `.AppImage`.
pub(crate) fn looks_like_appimage(executable: Option<&Path>, argv0: Option<&str>) -> bool {
    let is_appimage_name = |s: &str| s.to_ascii_lowercase().ends_with(".appimage");
    executable.is_some_and(|p| {
        let s = p.to_string_lossy();
        s.contains("/.mount_") || s.contains("/appimage_extracted_") || is_appimage_name(&s)
    }) || argv0.is_some_and(is_appimage_name)
}

/// Value of `key` in a NUL-separated environment block.
pub(crate) fn environ_value(environ: &[u8], key: &str) -> Option<String> {
    environ.split(|&b| b == 0).find_map(|entry| {
        let (k, v) = entry.split_at(entry.iter().position(|&b| b == b'=')?);
        (k == key.as_bytes())
            .then(|| String::from_utf8_lossy(&v[1..]).into_owned())
            .filter(|v| !v.is_empty())
    })
}

fn read_limited_result(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    let file = fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn read_limited(path: &Path, limit: u64) -> Option<Vec<u8>> {
    read_limited_result(path, limit).ok()
}

/// Reads per-process information from a `/proc`-like directory (a parameter for tests).
pub(crate) struct PackageDetector {
    proc_root: PathBuf,
}

impl Default for PackageDetector {
    fn default() -> Self {
        Self::new("/proc")
    }
}

impl PackageDetector {
    pub fn new(proc_root: impl Into<PathBuf>) -> Self {
        Self {
            proc_root: proc_root.into(),
        }
    }

    fn environ(&self, pid: u32) -> Option<Vec<u8>> {
        read_limited(
            &self.proc_root.join(pid.to_string()).join("environ"),
            MAX_ENVIRON,
        )
    }

    /// Detect the package of one process.
    pub fn detect(
        &self,
        pid: u32,
        executable: Option<&Path>,
        command_line: &[String],
    ) -> Option<PackageInfo> {
        if executable.is_none() && command_line.is_empty() {
            return None; // kernel thread
        }
        let mut environ: Option<Option<Vec<u8>>> = None;
        let mut env_value = |key: &str| -> Option<String> {
            environ
                .get_or_insert_with(|| self.environ(pid))
                .as_deref()
                .and_then(|env| environ_value(env, key))
        };

        // Flatpak: the sandbox root carries /.flatpak-info.
        let info_path = self
            .proc_root
            .join(pid.to_string())
            .join("root/.flatpak-info");
        let flatpak_id = match read_limited_result(&info_path, MAX_FLATPAK_INFO) {
            Ok(bytes) => flatpak_info_app_id(&String::from_utf8_lossy(&bytes)),
            // Not a sandbox: only an executable from /app hints otherwise.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            // The sandbox root could not be read; the environment of a sandboxed process
            // names the app as well (readable for own processes).
            Err(_) => env_value("FLATPAK_ID"),
        }
        .or_else(|| {
            executable
                .is_some_and(|p| p.starts_with("/app"))
                .then(|| env_value("FLATPAK_ID"))
                .flatten()
        });
        if let Some(id) = flatpak_id {
            return Some(PackageInfo {
                kind: PackageKind::Flatpak,
                id,
                path: None,
            });
        }

        // Snap: executables live under /snap/<name>/<revision>/.
        if let Some(name) = executable.and_then(snap_from_executable) {
            let name = if is_base_snap(&name) {
                env_value("SNAP_INSTANCE_NAME").or_else(|| env_value("SNAP_NAME"))?
            } else {
                env_value("SNAP_INSTANCE_NAME").unwrap_or(name)
            };
            return Some(PackageInfo {
                kind: PackageKind::Snap,
                id: name,
                path: None,
            });
        }

        // AppImage: the runtime exports the image path as $APPIMAGE.
        let argv0 = command_line.first().map(String::as_str);
        if looks_like_appimage(executable, argv0) {
            let path = env_value("APPIMAGE")
                .map(PathBuf::from)
                .or_else(|| {
                    argv0
                        .map(PathBuf::from)
                        .filter(|p| p.is_absolute() && looks_like_appimage(Some(p), None))
                })
                .or_else(|| {
                    executable
                        .filter(|p| {
                            looks_like_appimage(Some(p), None)
                                && !p.to_string_lossy().contains("/.mount_")
                        })
                        .map(Path::to_path_buf)
                })?;
            let id = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .filter(|s| !s.is_empty())?;
            return Some(PackageInfo {
                kind: PackageKind::AppImage,
                id,
                path: Some(path),
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::testutil::TempDir;

    #[test]
    fn flatpak_info_parsing() {
        let info = "[Application]\nname=org.gnome.Calculator\nruntime=runtime/org.gnome.Platform/x86_64/46\n\n[Instance]\ninstance-id=123\n";
        assert_eq!(
            flatpak_info_app_id(info).as_deref(),
            Some("org.gnome.Calculator")
        );
        assert_eq!(flatpak_info_app_id("[Runtime]\nname=org.x\n"), None);
        assert_eq!(flatpak_info_app_id("[Application]\nname=\n"), None);
        assert_eq!(flatpak_info_app_id(""), None);
    }

    #[test]
    fn snap_names_from_paths() {
        assert_eq!(
            snap_from_executable(Path::new("/snap/firefox/4848/usr/lib/firefox/firefox"))
                .as_deref(),
            Some("firefox")
        );
        assert_eq!(snap_from_executable(Path::new("/snap/bin/firefox")), None);
        assert_eq!(snap_from_executable(Path::new("/snap/firefox")), None);
        assert_eq!(snap_from_executable(Path::new("/usr/bin/firefox")), None);
        assert!(is_base_snap("core22"));
        assert!(is_base_snap("core"));
        assert!(!is_base_snap("corecpu"));
        assert!(!is_base_snap("firefox"));
    }

    #[test]
    fn environ_parsing() {
        let env = b"HOME=/home/u\0APPIMAGE=/opt/App-1.2.AppImage\0EMPTY=\0EQ=a=b\0";
        assert_eq!(
            environ_value(env, "APPIMAGE").as_deref(),
            Some("/opt/App-1.2.AppImage")
        );
        assert_eq!(environ_value(env, "EQ").as_deref(), Some("a=b"));
        assert_eq!(environ_value(env, "EMPTY"), None);
        assert_eq!(environ_value(env, "HOM"), None);
        assert_eq!(environ_value(b"", "X"), None);
    }

    #[test]
    fn appimage_heuristics() {
        assert!(looks_like_appimage(
            Some(Path::new("/tmp/.mount_ObsidiXYZ/obsidian")),
            None
        ));
        assert!(looks_like_appimage(
            None,
            Some("/home/u/Apps/Tool.AppImage")
        ));
        assert!(!looks_like_appimage(
            Some(Path::new("/usr/bin/vim")),
            Some("vim")
        ));
    }

    #[test]
    fn detects_packages_from_a_fake_proc() {
        let tmp = TempDir::new("proc");
        tmp.write(
            "100/root/.flatpak-info",
            "[Application]\nname=org.gimp.GIMP\n",
        );
        tmp.write("101/environ", "SNAP_NAME=firefox\0");
        tmp.write(
            "102/environ",
            "SNAP_INSTANCE_NAME=mytool_beta\0SNAP_NAME=mytool\0",
        );
        tmp.write(
            "103/environ",
            "APPIMAGE=/home/u/Apps/Obsidian-1.4.16.AppImage\0",
        );
        tmp.write("104/environ", "FLATPAK_ID=org.example.App\0");
        let d = PackageDetector::new(tmp.path());
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();

        let flatpak = d
            .detect(100, Some(Path::new("/app/bin/gimp")), &[])
            .unwrap();
        assert_eq!(flatpak.kind, PackageKind::Flatpak);
        assert_eq!(flatpak.id, "org.gimp.GIMP");

        let snap = d
            .detect(101, Some(Path::new("/snap/firefox/1/firefox")), &[])
            .unwrap();
        assert_eq!(
            (snap.kind, snap.id.as_str()),
            (PackageKind::Snap, "firefox")
        );
        // A program from a base snap belongs to the snap named in the environment.
        let base = d
            .detect(102, Some(Path::new("/snap/core22/1/usr/bin/python3")), &[])
            .unwrap();
        assert_eq!(base.id, "mytool_beta");
        assert!(
            d.detect(999, Some(Path::new("/snap/core22/1/usr/bin/python3")), &[])
                .is_none()
        );

        let appimage = d
            .detect(
                103,
                Some(Path::new("/tmp/.mount_Obsid1/obsidian")),
                &s(&["obsidian"]),
            )
            .unwrap();
        assert_eq!(appimage.kind, PackageKind::AppImage);
        assert_eq!(appimage.id, "Obsidian-1.4.16");
        assert_eq!(
            appimage.path.as_deref(),
            Some(Path::new("/home/u/Apps/Obsidian-1.4.16.AppImage"))
        );

        let env_flatpak = d.detect(104, Some(Path::new("/app/bin/app")), &[]).unwrap();
        assert_eq!(env_flatpak.id, "org.example.App");

        assert!(
            d.detect(105, Some(Path::new("/usr/bin/vim")), &s(&["vim"]))
                .is_none()
        );
        assert!(d.detect(106, None, &[]).is_none());
    }
}
