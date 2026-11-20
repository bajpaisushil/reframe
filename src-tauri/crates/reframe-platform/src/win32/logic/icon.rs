//! Icon pixel conversion and PNG encoding.
//!
//! Icons are rendered by `DrawIconEx` into a zeroed, top-down 32-bit DIB. For icons with an alpha
//! channel that produces *premultiplied* BGRA. Old icons without alpha leave every alpha byte at
//! zero; their transparency lives in the AND mask, which is rendered separately with `DI_MASK`
//! (black = opaque, white = transparent).

/// Smallest and largest icon edge Reframe renders, and the size used when none is requested.
pub const MIN_ICON_SIZE: u32 = 16;
pub const MAX_ICON_SIZE: u32 = 256;
pub const DEFAULT_ICON_SIZE: u32 = 32;

/// Icon edge length to render for a requested size.
pub fn normalized_icon_size(requested: u32) -> u32 {
    if requested == 0 {
        DEFAULT_ICON_SIZE
    } else {
        requested.clamp(MIN_ICON_SIZE, MAX_ICON_SIZE)
    }
}

/// The complete 4-byte pixels of a buffer (a trailing partial pixel is ignored).
fn pixels(buf: &[u8]) -> &[[u8; 4]] {
    buf.as_chunks::<4>().0
}

/// Whether an image the shell returned for a `requested`-px icon is usable: anything smaller
/// than half the requested size is a placeholder (such as a 1x1 stub bitmap), not an icon.
pub fn is_usable_icon_size(width: u32, height: u32, requested: u32) -> bool {
    let minimum = requested.div_ceil(2).max(1);
    width >= minimum && height >= minimum
}

/// `true` if any pixel of a BGRA buffer has a non-zero alpha byte.
pub fn any_alpha(bgra: &[u8]) -> bool {
    pixels(bgra).iter().any(|&[_, _, _, a]| a != 0)
}

fn unpremultiply(channel: u8, alpha: u8) -> u8 {
    let value = (u32::from(channel) * 255 + u32::from(alpha) / 2) / u32::from(alpha);
    u8::try_from(value.min(255)).unwrap_or(u8::MAX)
}

/// Premultiplied BGRA → straight-alpha RGBA.
pub fn premultiplied_bgra_to_rgba(bgra: &[u8]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(bgra.len());
    for &[b, g, r, a] in pixels(bgra) {
        match a {
            0 => rgba.extend_from_slice(&[0, 0, 0, 0]),
            255 => rgba.extend_from_slice(&[r, g, b, 255]),
            _ => rgba.extend_from_slice(&[
                unpremultiply(r, a),
                unpremultiply(g, a),
                unpremultiply(b, a),
                a,
            ]),
        }
    }
    rgba
}

/// Straight-alpha BGRA → RGBA (channel swap only).
pub fn straight_bgra_to_rgba(bgra: &[u8]) -> Vec<u8> {
    pixels(bgra)
        .iter()
        .flat_map(|&[b, g, r, a]| [r, g, b, a])
        .collect()
}

/// Icon without an alpha channel: colours from the `DI_NORMAL` pass, transparency from the AND
/// mask rendered with `DI_MASK` (black = opaque). Transparent pixels become fully transparent
/// black.
pub fn masked_bgra_to_rgba(color: &[u8], mask: &[u8]) -> Vec<u8> {
    pixels(color)
        .iter()
        .zip(pixels(mask))
        .flat_map(|(&[b, g, r, _], &[mb, mg, mr, _])| {
            if mb == 0 && mg == 0 && mr == 0 {
                [r, g, b, 255]
            } else {
                [0, 0, 0, 0]
            }
        })
        .collect()
}

/// Convert the output of a `DrawIconEx` rendering (see module docs) to straight RGBA.
pub fn rendered_icon_to_rgba(color: &[u8], mask: Option<&[u8]>) -> Vec<u8> {
    match mask {
        Some(mask) if !any_alpha(color) => masked_bgra_to_rgba(color, mask),
        _ => premultiplied_bgra_to_rgba(color),
    }
}

/// Convert a 32-bit bitmap produced by the shell (`IShellItemImageFactory`) to RGBA. Such bitmaps
/// may carry premultiplied alpha, straight alpha, or no alpha at all (opaque). Premultiplied data
/// never has a colour channel above its alpha, which tells the two apart.
pub fn shell_bitmap_to_rgba(bgra: &[u8]) -> Vec<u8> {
    if !any_alpha(bgra) {
        return pixels(bgra)
            .iter()
            .flat_map(|&[b, g, r, _]| [r, g, b, 255])
            .collect();
    }
    let premultiplied = pixels(bgra)
        .iter()
        .all(|&[b, g, r, a]| b <= a && g <= a && r <= a);
    if premultiplied {
        premultiplied_bgra_to_rgba(bgra)
    } else {
        straight_bgra_to_rgba(bgra)
    }
}

/// `true` when every pixel is fully transparent (nothing was drawn).
pub fn is_blank(rgba: &[u8]) -> bool {
    pixels(rgba).iter().all(|&[_, _, _, a]| a == 0)
}

/// Encode straight-alpha RGBA pixels as PNG.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let expected = usize::try_from(width)
        .ok()
        .zip(usize::try_from(height).ok())
        .and_then(|(w, h)| w.checked_mul(h))
        .and_then(|pixels| pixels.checked_mul(4));
    if width == 0 || height == 0 || expected != Some(rgba.len()) {
        return Err(format!(
            "pixel buffer of {} bytes does not match a {width}x{height} RGBA image",
            rgba.len()
        ));
    }
    let mut png_bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut png_bytes, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(png_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(png_bytes: &[u8]) -> (u32, u32, Vec<u8>) {
        let decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
        let mut reader = decoder.read_info().expect("valid png");
        let mut buf = vec![0; reader.output_buffer_size().expect("size")];
        let info = reader.next_frame(&mut buf).expect("frame");
        buf.truncate(info.buffer_size());
        (info.width, info.height, buf)
    }

    #[test]
    fn icon_sizes_are_clamped() {
        assert_eq!(normalized_icon_size(0), DEFAULT_ICON_SIZE);
        assert_eq!(normalized_icon_size(1), MIN_ICON_SIZE);
        assert_eq!(normalized_icon_size(48), 48);
        assert_eq!(normalized_icon_size(4096), MAX_ICON_SIZE);
    }

    #[test]
    fn placeholder_images_are_not_icons() {
        assert!(is_usable_icon_size(32, 32, 32));
        assert!(is_usable_icon_size(48, 48, 32)); // larger is fine
        assert!(is_usable_icon_size(16, 16, 32)); // half size is acceptable
        assert!(!is_usable_icon_size(15, 15, 32));
        assert!(!is_usable_icon_size(1, 1, 32));
        assert!(!is_usable_icon_size(32, 1, 32));
        assert!(is_usable_icon_size(8, 8, 16));
        assert!(!is_usable_icon_size(0, 0, 0));
    }

    #[test]
    fn premultiplied_pixels_are_unpremultiplied() {
        let bgra = [
            0, 0, 0, 0, // transparent
            10, 20, 30, 255, // opaque
            64, 32, 128, 128, // half transparent: straight = (255, 64, 128)
            200, 200, 200, 100, // invalid premultiplied data saturates instead of overflowing
        ];
        assert_eq!(
            premultiplied_bgra_to_rgba(&bgra),
            vec![
                0, 0, 0, 0, //
                30, 20, 10, 255, //
                255, 64, 128, 128, //
                255, 255, 255, 100,
            ]
        );
        // Trailing partial pixels are ignored rather than read out of bounds.
        assert_eq!(premultiplied_bgra_to_rgba(&[1, 2, 3]), Vec::<u8>::new());
    }

    #[test]
    fn mask_supplies_transparency_for_alpha_less_icons() {
        let color = [
            255, 0, 0, 0, // blue, opaque per mask
            0, 255, 0, 0, // green, transparent per mask
        ];
        let mask = [
            0, 0, 0, 0, // black: opaque
            255, 255, 255, 0, // white: transparent
        ];
        let expected = vec![0, 0, 255, 255, 0, 0, 0, 0];
        assert_eq!(masked_bgra_to_rgba(&color, &mask), expected);
        assert_eq!(rendered_icon_to_rgba(&color, Some(&mask)), expected);
    }

    #[test]
    fn alpha_icons_ignore_the_mask() {
        let color = [0, 0, 128, 128];
        let mask = [255, 255, 255, 0];
        assert_eq!(
            rendered_icon_to_rgba(&color, Some(&mask)),
            vec![255, 0, 0, 128]
        );
        assert_eq!(rendered_icon_to_rgba(&color, None), vec![255, 0, 0, 128]);
    }

    #[test]
    fn shell_bitmaps_detect_their_alpha_format() {
        // No alpha at all: opaque.
        assert_eq!(shell_bitmap_to_rgba(&[1, 2, 3, 0]), vec![3, 2, 1, 255]);
        // Colour above alpha: straight alpha.
        assert_eq!(
            shell_bitmap_to_rgba(&[200, 100, 50, 128]),
            vec![50, 100, 200, 128]
        );
        // Colour within alpha: premultiplied.
        assert_eq!(
            shell_bitmap_to_rgba(&[64, 32, 128, 128]),
            vec![255, 64, 128, 128]
        );
    }

    #[test]
    fn blank_detection() {
        assert!(is_blank(&[0, 0, 0, 0, 9, 9, 9, 0]));
        assert!(!is_blank(&[0, 0, 0, 0, 9, 9, 9, 1]));
    }

    #[test]
    fn png_round_trip() {
        let rgba: Vec<u8> = (0..2 * 3)
            .flat_map(|i| [i * 10, 255 - i * 10, 7, 255 - i])
            .collect();
        let png_bytes = encode_png(2, 3, &rgba).expect("encodes");
        assert!(png_bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]));
        assert_eq!(decode(&png_bytes), (2, 3, rgba));
    }

    #[test]
    fn png_rejects_mismatched_buffers() {
        assert!(encode_png(2, 2, &[0; 15]).is_err());
        assert!(encode_png(0, 2, &[]).is_err());
        assert!(encode_png(u32::MAX, u32::MAX, &[0; 4]).is_err());
    }
}
