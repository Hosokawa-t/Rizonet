//! Image → `.ico` conversion.
//!
//! Accepts any aspect ratio and any format supported by [`image`] (PNG,
//! JPEG, BMP, GIF, WebP, TIFF, ICO, …) and produces a Windows-compatible
//! multi-resolution `.ico` file by:
//!
//! 1. Decoding the source image.
//! 2. Letterboxing it onto a transparent square canvas (no stretch —
//!    preserves the original aspect ratio).
//! 3. Resampling to the standard set of icon sizes (16, 24, 32, 48, 64,
//!    128, 256) with Lanczos3.
//! 4. Packaging every size into a single `.ico` with embedded PNG data.
//!
//! The module is gated behind the `iconify` cargo feature so the core
//! runtime does not pay for the `image` dependency at runtime.

use std::fs;
use std::path::Path;

use image::imageops::FilterType;
use image::{GenericImageView, ImageReader, Rgba, RgbaImage};

use crate::error::{Result, RizonetError};

/// The icon sizes embedded into every generated `.ico`. Windows picks the
/// closest match at display time, so covering the common DPI buckets is
/// enough to look crisp on any monitor.
pub const ICON_SIZES: &[u32] = &[16, 24, 32, 48, 64, 128, 256];

/// Convert any image on disk into a multi-resolution `.ico` and write it
/// to `dst`. Returns the number of bytes written.
pub fn image_to_ico(src: &Path, dst: &Path) -> Result<usize> {
    let bytes = encode_ico_from_path(src)?;
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(dst, &bytes)?;
    Ok(bytes.len())
}

/// Convert any image on disk into an in-memory `.ico` blob.
pub fn encode_ico_from_path(src: &Path) -> Result<Vec<u8>> {
    let img = ImageReader::open(src)
        .map_err(|e| RizonetError::Other(format!("failed to open icon source {:?}: {e}", src)))?
        .with_guessed_format()
        .map_err(|e| RizonetError::Other(format!("failed to sniff icon format: {e}")))?
        .decode()
        .map_err(|e| RizonetError::Other(format!("failed to decode icon: {e}")))?;
    encode_ico_from_rgba(&img.to_rgba8(), img.dimensions())
}

/// Convert an already-decoded RGBA image into a multi-resolution `.ico`.
pub fn encode_ico_from_rgba(src: &RgbaImage, dims: (u32, u32)) -> Result<Vec<u8>> {
    let square = letterbox_square(src, dims);

    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    for &size in ICON_SIZES {
        let resized = image::imageops::resize(&square, size, size, FilterType::Lanczos3);
        let img = ico::IconImage::from_rgba_data(size, size, resized.into_raw());
        let entry = ico::IconDirEntry::encode(&img)
            .map_err(|e| RizonetError::Other(format!("ico encode failed at {size}px: {e}")))?;
        dir.add_entry(entry);
    }
    let mut out = Vec::new();
    dir.write(&mut out)
        .map_err(|e| RizonetError::Other(format!("ico serialize failed: {e}")))?;
    Ok(out)
}

/// Pad `src` with fully-transparent pixels onto a square canvas whose side
/// equals `max(width, height)`. The original image is centred and **not
/// stretched**, which is what users almost always want when they hand us
/// a wide banner or a tall portrait photo.
fn letterbox_square(src: &RgbaImage, (w, h): (u32, u32)) -> RgbaImage {
    let side = w.max(h).max(1);
    if w == h {
        return src.clone();
    }
    let mut canvas = RgbaImage::from_pixel(side, side, Rgba([0, 0, 0, 0]));
    let ox = (side - w) / 2;
    let oy = (side - h) / 2;
    image::imageops::overlay(&mut canvas, src, ox as i64, oy as i64);
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn non_square_input_is_letterboxed() {
        // A 4:1 blue strip — exercises the padding path.
        let mut img = RgbaImage::new(40, 10);
        for p in img.pixels_mut() {
            *p = Rgba([10, 20, 200, 255]);
        }
        let bytes = encode_ico_from_rgba(&img, (40, 10)).expect("encode ok");
        assert!(bytes.len() > 64, "ico must contain data, got {}", bytes.len());
        // The header reports the number of embedded images.
        let count = u16::from_le_bytes([bytes[4], bytes[5]]);
        assert_eq!(count as usize, ICON_SIZES.len());
    }
}
