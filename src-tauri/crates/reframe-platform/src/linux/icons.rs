//! Application icons: `_NET_WM_ICON` pixel data to PNG, and PNG lookup in freedesktop icon
//! themes (`hicolor`) and `/usr/share/pixmaps`.
//!
//! Limitations: only PNG files are returned. SVG (scalable) and XPM icons are not rendered, so
//! an application that ships nothing but an SVG icon yields no icon. Only the `hicolor`
//! fallback theme is searched (every application is required to install its icon there),
//! not the user's selected theme.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Icons larger than this edge length in `_NET_WM_ICON` are ignored (bogus data guard).
const MAX_ICON_EDGE: u32 = 1024;
/// PNG files larger than this are not loaded.
const MAX_PNG_FILE: u64 = 8 * 1024 * 1024;
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// One image of a `_NET_WM_ICON` property.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IconImage<'a> {
    pub width: u32,
    pub height: u32,
    /// `width * height` ARGB32 pixels, row by row, alpha in the high byte (not premultiplied).
    pub pixels: &'a [u32],
}

/// Split `_NET_WM_ICON` data (`width, height, pixels..., width, height, pixels...`) into its
/// images. Parsing stops at the first malformed or truncated entry.
pub(crate) fn parse_net_wm_icon(data: &[u32]) -> Vec<IconImage<'_>> {
    let mut images = Vec::new();
    let mut rest = data;
    while rest.len() >= 2 {
        let (width, height) = (rest[0], rest[1]);
        if width == 0 || height == 0 || width > MAX_ICON_EDGE || height > MAX_ICON_EDGE {
            break;
        }
        let count = (width as usize) * (height as usize);
        let Some(pixels) = rest.get(2..2 + count) else {
            break;
        };
        images.push(IconImage {
            width,
            height,
            pixels,
        });
        rest = &rest[2 + count..];
    }
    images
}

/// The image whose size is closest to `size` (by the longer edge); on a tie the larger one.
pub(crate) fn choose_icon<'a>(images: &[IconImage<'a>], size: u32) -> Option<IconImage<'a>> {
    images.iter().copied().min_by_key(|img| {
        let edge = img.width.max(img.height);
        (edge.abs_diff(size), std::cmp::Reverse(edge))
    })
}

/// Encode ARGB32 pixels (alpha in the high byte, as in `_NET_WM_ICON`) as an RGBA PNG.
pub(crate) fn argb_to_png(width: u32, height: u32, pixels: &[u32]) -> Option<Vec<u8>> {
    let expected = (width as usize).checked_mul(height as usize)?;
    if width == 0 || height == 0 || pixels.len() < expected {
        return None;
    }
    let mut rgba = Vec::with_capacity(expected * 4);
    for &p in &pixels[..expected] {
        let [a, r, g, b] = p.to_be_bytes();
        rgba.extend_from_slice(&[r, g, b, a]);
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(&rgba).ok()?;
        writer.finish().ok()?;
    }
    Some(out)
}

/// Read a PNG file (size-limited, signature checked).
pub(crate) fn read_png(path: &Path) -> Option<Vec<u8>> {
    let file = fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > MAX_PNG_FILE {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_PNG_FILE).read_to_end(&mut bytes).ok()?;
    bytes.starts_with(&PNG_SIGNATURE).then_some(bytes)
}

/// Where icon themes and loose icons live, in lookup order.
#[derive(Debug, Clone, Default)]
pub(crate) struct IconDirs {
    /// Base directories containing themes (`<base>/hicolor/...`), e.g. `/usr/share/icons`.
    pub theme_bases: Vec<PathBuf>,
    /// Directories with loose icon files, e.g. `/usr/share/pixmaps`.
    pub pixmaps: Vec<PathBuf>,
}

impl IconDirs {
    /// Standard locations from the XDG base directory specification plus Flatpak and Snap
    /// export directories.
    pub fn standard(xdg: &super::xdg::XdgDirs) -> Self {
        let mut theme_bases = Vec::new();
        let mut push = |p: PathBuf| {
            if !theme_bases.contains(&p) {
                theme_bases.push(p);
            }
        };
        if let Some(home) = &xdg.home {
            push(home.join(".icons"));
        }
        if let Some(data_home) = &xdg.data_home {
            push(data_home.join("icons"));
        }
        for dir in &xdg.data_dirs {
            push(dir.join("icons"));
        }
        if let Some(home) = &xdg.home {
            push(home.join(".local/share/flatpak/exports/share/icons"));
        }
        push(PathBuf::from("/var/lib/flatpak/exports/share/icons"));

        let mut pixmaps = vec![PathBuf::from("/var/lib/snapd/desktop/icons")];
        for dir in &xdg.data_dirs {
            let p = dir.join("pixmaps");
            if !pixmaps.contains(&p) {
                pixmaps.push(p);
            }
        }
        let usr = PathBuf::from("/usr/share/pixmaps");
        if !pixmaps.contains(&usr) {
            pixmaps.push(usr);
        }
        Self {
            theme_bases,
            pixmaps,
        }
    }
}

/// Effective pixel size of an icon theme sub directory name: `48x48` -> 48, `24x24@2` -> 48.
fn size_dir_pixels(name: &str) -> Option<u32> {
    let (dims, scale) = match name.split_once('@') {
        Some((dims, scale)) => (dims, scale.parse::<u32>().ok()?),
        None => (name, 1),
    };
    let (w, h) = dims.split_once('x')?;
    let (w, h) = (w.parse::<u32>().ok()?, h.parse::<u32>().ok()?);
    (w == h && w > 0 && scale > 0).then(|| w.saturating_mul(scale))
}

/// Pick the best size: the smallest size >= `wanted`, else the largest available.
fn better(candidate: u32, current: u32, wanted: u32) -> bool {
    match (candidate >= wanted, current >= wanted) {
        (true, true) => candidate < current,
        (true, false) => true,
        (false, true) => false,
        (false, false) => candidate > current,
    }
}

/// Resolve an `Icon=` value (theme icon name or absolute path) to a PNG file.
pub(crate) fn find_icon_png(icon: &str, size: u32, dirs: &IconDirs) -> Option<PathBuf> {
    let icon = icon.trim();
    if icon.is_empty() {
        return None;
    }
    let path = Path::new(icon);
    if path.is_absolute() {
        let is_png = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("png"));
        return (is_png && path.is_file()).then(|| path.to_path_buf());
    }
    if icon.contains('/') {
        return None;
    }
    // Icon names should not carry an extension, but some desktop files add one.
    let name = ["png", "svg", "xpm"]
        .iter()
        .find_map(|ext| {
            icon.strip_suffix(ext)
                .and_then(|s| s.strip_suffix('.'))
                .filter(|s| !s.is_empty())
        })
        .unwrap_or(icon);
    let file_name = format!("{name}.png");

    let mut best: Option<(u32, PathBuf)> = None;
    for base in &dirs.theme_bases {
        let theme = base.join("hicolor");
        let Ok(entries) = fs::read_dir(&theme) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir_name = entry.file_name();
            let Some(pixels) = dir_name.to_str().and_then(size_dir_pixels) else {
                continue;
            };
            let candidate = entry.path().join("apps").join(&file_name);
            if best
                .as_ref()
                .is_none_or(|(current, _)| better(pixels, *current, size))
                && candidate.is_file()
            {
                best = Some((pixels, candidate));
            }
        }
    }
    if let Some((_, path)) = best {
        return Some(path);
    }
    dirs.theme_bases
        .iter()
        .chain(dirs.pixmaps.iter())
        .map(|dir| dir.join(&file_name))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::testutil::TempDir;

    fn decode(png: &[u8]) -> (u32, u32, Vec<u8>) {
        let decoder = png::Decoder::new(std::io::Cursor::new(png));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        buf.truncate(info.buffer_size());
        assert_eq!(info.color_type, png::ColorType::Rgba);
        (info.width, info.height, buf)
    }

    #[test]
    fn argb_to_png_preserves_pixels() {
        // 2x1: opaque red, half-transparent blue.
        let pixels = [0xFFFF_0000, 0x8000_00FF];
        let png = argb_to_png(2, 1, &pixels).unwrap();
        assert!(png.starts_with(&PNG_SIGNATURE));
        let (w, h, rgba) = decode(&png);
        assert_eq!((w, h), (2, 1));
        assert_eq!(rgba, vec![255, 0, 0, 255, 0, 0, 255, 128]);
    }

    #[test]
    fn argb_to_png_rejects_bad_input() {
        assert!(argb_to_png(0, 1, &[]).is_none());
        assert!(argb_to_png(2, 2, &[0; 3]).is_none());
    }

    #[test]
    fn net_wm_icon_parsing_and_choice() {
        let mut data = vec![2, 2];
        data.extend([0xFF00_0000; 4]);
        data.extend([4, 4]);
        data.extend([0xFFFF_FFFF; 16]);
        data.extend([8, 8]);
        data.extend([0x0000_0000; 64]);
        let images = parse_net_wm_icon(&data);
        assert_eq!(images.len(), 3);
        assert_eq!(choose_icon(&images, 4).unwrap().width, 4);
        assert_eq!(choose_icon(&images, 64).unwrap().width, 8);
        assert_eq!(choose_icon(&images, 1).unwrap().width, 2);
        // 6 is equally far from 4 and 8: prefer the larger.
        assert_eq!(choose_icon(&images, 6).unwrap().width, 8);
        assert!(choose_icon(&[], 32).is_none());
    }

    #[test]
    fn net_wm_icon_truncated_or_bogus_data() {
        // Second image claims 16 pixels but only has 3.
        let data = [1, 1, 0xFFFF_FFFF, 4, 4, 1, 2, 3];
        let images = parse_net_wm_icon(&data);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].pixels, &[0xFFFF_FFFF]);
        assert!(parse_net_wm_icon(&[0, 0, 1, 2]).is_empty());
        assert!(parse_net_wm_icon(&[100_000, 100_000]).is_empty());
        assert!(parse_net_wm_icon(&[5]).is_empty());
    }

    #[test]
    fn size_directory_names() {
        assert_eq!(size_dir_pixels("48x48"), Some(48));
        assert_eq!(size_dir_pixels("24x24@2"), Some(48));
        assert_eq!(size_dir_pixels("scalable"), None);
        assert_eq!(size_dir_pixels("16x32"), None);
        assert_eq!(size_dir_pixels("symbolic"), None);
    }

    fn write_png(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, argb_to_png(1, 1, &[0xFFFF_FFFF]).unwrap()).unwrap();
    }

    #[test]
    fn theme_lookup_picks_nearest_size() {
        let tmp = TempDir::new("icons");
        let base = tmp.path().join("icons");
        for size in ["16x16", "48x48", "128x128"] {
            write_png(&base.join(format!("hicolor/{size}/apps/demo.png")));
        }
        fs::create_dir_all(base.join("hicolor/scalable/apps")).unwrap();
        fs::write(base.join("hicolor/scalable/apps/vector.svg"), "<svg/>").unwrap();
        let pixmaps = tmp.path().join("pixmaps");
        write_png(&pixmaps.join("legacy.png"));
        let dirs = IconDirs {
            theme_bases: vec![base.clone()],
            pixmaps: vec![pixmaps.clone()],
        };

        let pick = |name: &str, size: u32| find_icon_png(name, size, &dirs);
        assert_eq!(
            pick("demo", 48),
            Some(base.join("hicolor/48x48/apps/demo.png"))
        );
        assert_eq!(
            pick("demo", 40),
            Some(base.join("hicolor/48x48/apps/demo.png"))
        );
        assert_eq!(
            pick("demo", 64),
            Some(base.join("hicolor/128x128/apps/demo.png"))
        );
        assert_eq!(
            pick("demo", 512),
            Some(base.join("hicolor/128x128/apps/demo.png"))
        );
        assert_eq!(
            pick("demo", 8),
            Some(base.join("hicolor/16x16/apps/demo.png"))
        );
        assert_eq!(
            pick("demo.png", 48),
            Some(base.join("hicolor/48x48/apps/demo.png"))
        );
        assert_eq!(pick("legacy", 48), Some(pixmaps.join("legacy.png")));
        // SVG-only icons are not rendered.
        assert_eq!(pick("vector", 48), None);
        assert_eq!(pick("missing", 48), None);
        assert_eq!(pick("", 48), None);
        assert_eq!(pick("../escape", 48), None);
        // Absolute paths: PNG only.
        let abs = pixmaps.join("legacy.png");
        assert_eq!(pick(abs.to_str().unwrap(), 48), Some(abs.clone()));
        assert_eq!(pick("/nonexistent/icon.png", 48), None);
        assert_eq!(pick("/usr/share/icons/x.svg", 48), None);

        assert!(read_png(&abs).is_some());
        let not_png = tmp.path().join("fake.png");
        fs::write(&not_png, b"not a png").unwrap();
        assert!(read_png(&not_png).is_none());
    }
}
