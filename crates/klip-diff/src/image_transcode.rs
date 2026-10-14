//! Image format detection, lossless transcoding, and RGBA conversion pipeline.
//! Compresses heavy uncompressed screen captures (BMP/DIB) to compact representations (< 14ms)
//! and provides RGBA <-> PNG codec conversions for host clipboard APIs.

use image::{DynamicImage, ImageFormat as ImgFormat, RgbaImage};
use std::io::Cursor;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    WebP,
    Bmp,
    Unknown,
}

/// Detect image encoding format from magic header bytes.
pub fn detect_image_format(bytes: &[u8]) -> ImageFormat {
    if bytes.len() >= 8 && &bytes[0..8] == b"\x89PNG\r\n\x1a\n" {
        ImageFormat::Png
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        ImageFormat::WebP
    } else if bytes.len() >= 2 && &bytes[0..2] == b"BM" {
        ImageFormat::Bmp
    } else {
        ImageFormat::Unknown
    }
}

/// Check if the image buffer is in an uncompressed format (such as Windows BMP/CF_DIB)
pub fn is_uncompressed_image(bytes: &[u8]) -> bool {
    matches!(detect_image_format(bytes), ImageFormat::Bmp)
}

/// Largest image Klip accepts or sends, in pixels (about 8K). 36 Mpx is 144 MB once decoded to RGBA.
pub const MAX_IMAGE_PIXELS: u64 = 36_000_000;

/// True when a `width` x `height` image is above [`MAX_IMAGE_PIXELS`] (no overflow for any input).
pub fn too_many_pixels(width: u32, height: u32) -> bool {
    u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS
}

/// Check an incoming image *without decoding it*: the magic bytes must match `expected` (PNG or
/// WebP) and the header's size must be within [`MAX_IMAGE_PIXELS`]. A decompression bomb is a few
/// bytes of header and is refused here. Returns `(width, height)`.
pub fn validate_image(expected: ImageFormat, bytes: &[u8]) -> Result<(u32, u32), String> {
    let (format, name) = match expected {
        ImageFormat::Png => (ImgFormat::Png, "PNG"),
        ImageFormat::WebP => (ImgFormat::WebP, "WebP"),
        _ => return Err("only PNG and WebP images are accepted".to_string()),
    };
    if detect_image_format(bytes) != expected {
        return Err(format!("the data is not a {name} image"));
    }
    let (width, height) = image::ImageReader::with_format(Cursor::new(bytes), format)
        .into_dimensions()
        .map_err(|e| format!("unreadable {name} header: {e}"))?;
    if too_many_pixels(width, height) {
        return Err(format!(
            "the image is {width} x {height} pixels; the limit is {MAX_IMAGE_PIXELS} pixels"
        ));
    }
    Ok((width, height))
}

/// A PNG preview of an image, at most `max_side` pixels on the longest side (never enlarged). The
/// pixel limit is checked from the header before anything is decoded.
pub fn thumbnail_png(bytes: &[u8], max_side: u32) -> Result<Vec<u8>, String> {
    if max_side == 0 {
        return Err("the thumbnail size must be at least 1 pixel".to_string());
    }
    let (width, height, rgba) = png_to_rgba(bytes)?;
    let img = RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| "Failed to construct RgbaImage from buffer".to_string())?;
    let dynamic = DynamicImage::ImageRgba8(img);
    let dynamic = if width > max_side || height > max_side {
        dynamic.thumbnail(max_side, max_side)
    } else {
        dynamic
    };
    let mut out = Vec::new();
    dynamic
        .write_to(&mut Cursor::new(&mut out), ImgFormat::Png)
        .map_err(|e| format!("PNG encoding failed: {e}"))?;
    Ok(out)
}

/// Encodes raw RGBA pixel data (e.g. from arboard::ImageData) into standard compressed PNG container bytes.
pub fn rgba_to_png(width: u32, height: u32, rgba_bytes: &[u8]) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 {
        return Err("Image dimensions cannot be zero".to_string());
    }
    let expected_len = (width as usize) * (height as usize) * 4;
    if rgba_bytes.len() < expected_len {
        return Err(format!(
            "Buffer length {} insufficient for {}x{} RGBA (expected {})",
            rgba_bytes.len(),
            width,
            height,
            expected_len
        ));
    }

    let img_buffer = RgbaImage::from_raw(width, height, rgba_bytes[..expected_len].to_vec())
        .ok_or_else(|| "Failed to construct RgbaImage from buffer".to_string())?;

    let dynamic = DynamicImage::ImageRgba8(img_buffer);
    let mut out = Vec::new();
    let mut cursor = Cursor::new(&mut out);

    dynamic
        .write_to(&mut cursor, ImgFormat::Png)
        .map_err(|e| format!("PNG encoding failed: {}", e))?;

    Ok(out)
}

/// Decodes compressed PNG (or other supported format) bytes into raw (width, height, RGBA) pixels
/// for setting into the host clipboard.
pub fn png_to_rgba(png_bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    // Read the size from the header first: a few bytes can describe gigabytes of pixels.
    let (width, height) = image::ImageReader::new(Cursor::new(png_bytes))
        .with_guessed_format()
        .map_err(|e| format!("Failed to read image header: {e}"))?
        .into_dimensions()
        .map_err(|e| format!("Failed to read image header: {e}"))?;
    if too_many_pixels(width, height) {
        return Err(format!(
            "the image is {width} x {height} pixels; the limit is {MAX_IMAGE_PIXELS} pixels"
        ));
    }
    let dynamic =
        image::load_from_memory(png_bytes).map_err(|e| format!("Failed to decode image: {}", e))?;

    let rgba = dynamic.to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok((width, height, rgba.into_raw()))
}

/// Transcodes uncompressed BMP/DIB screenshot into a compressed PNG container.
/// Achieves typically 85-96% bandwidth reduction.
pub fn transcode_uncompressed_bmp_to_png(bmp_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let dynamic = image::load_from_memory_with_format(bmp_bytes, ImgFormat::Bmp)
        .map_err(|e| format!("Failed to load BMP buffer: {}", e))?;

    let mut out = Vec::new();
    let mut cursor = Cursor::new(&mut out);
    dynamic
        .write_to(&mut cursor, ImgFormat::Png)
        .map_err(|e| format!("PNG transcoding failed: {}", e))?;

    Ok(out)
}

/// Converts PNG to BMP for host platforms requiring standard bitmap representation (e.g. CF_DIB on Windows).
pub fn png_to_bmp(png_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let dynamic =
        image::load_from_memory(png_bytes).map_err(|e| format!("Failed to decode PNG: {}", e))?;

    let mut out = Vec::new();
    let mut cursor = Cursor::new(&mut out);
    dynamic
        .write_to(&mut cursor, ImgFormat::Bmp)
        .map_err(|e| format!("BMP encoding failed: {}", e))?;

    Ok(out)
}

/// Encodes raw RGBA pixel data into compressed WebP container bytes.
pub fn rgba_to_webp(width: u32, height: u32, rgba_bytes: &[u8]) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 {
        return Err("Image dimensions cannot be zero".to_string());
    }
    let expected_len = (width as usize) * (height as usize) * 4;
    if rgba_bytes.len() < expected_len {
        return Err(format!(
            "Buffer length {} insufficient for {}x{} RGBA (expected {})",
            rgba_bytes.len(),
            width,
            height,
            expected_len
        ));
    }

    let img_buffer = RgbaImage::from_raw(width, height, rgba_bytes[..expected_len].to_vec())
        .ok_or_else(|| "Failed to construct RgbaImage from buffer".to_string())?;

    let dynamic = DynamicImage::ImageRgba8(img_buffer);
    let mut out = Vec::new();
    let mut cursor = Cursor::new(&mut out);

    dynamic
        .write_to(&mut cursor, ImgFormat::WebP)
        .map_err(|e| format!("WebP encoding failed: {}", e))?;

    Ok(out)
}

/// Transcodes uncompressed BMP/DIB screenshot into a compressed WebP container.
pub fn transcode_uncompressed_bmp_to_webp(bmp_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let dynamic = image::load_from_memory_with_format(bmp_bytes, ImgFormat::Bmp)
        .map_err(|e| format!("Failed to load BMP buffer: {}", e))?;

    let mut out = Vec::new();
    let mut cursor = Cursor::new(&mut out);
    dynamic
        .write_to(&mut cursor, ImgFormat::WebP)
        .map_err(|e| format!("WebP transcoding failed: {}", e))?;

    Ok(out)
}

/// Optimizes an image buffer before transmitting over the wire:
/// - If uncompressed BMP, compresses to WebP (< 14ms) or PNG.
/// - If already compressed PNG or WebP, retains as-is.
pub fn transcode_image_for_sync(bytes: &[u8]) -> Result<(ImageFormat, Vec<u8>), String> {
    match detect_image_format(bytes) {
        ImageFormat::Bmp => {
            if let Ok(webp_bytes) = transcode_uncompressed_bmp_to_webp(bytes) {
                Ok((ImageFormat::WebP, webp_bytes))
            } else {
                let compressed = transcode_uncompressed_bmp_to_png(bytes)?;
                Ok((ImageFormat::Png, compressed))
            }
        }
        ImageFormat::Png => Ok((ImageFormat::Png, bytes.to_vec())),
        ImageFormat::WebP => Ok((ImageFormat::WebP, bytes.to_vec())),
        ImageFormat::Unknown => {
            // Attempt generic decode fallback
            if let Ok(dynamic) = image::load_from_memory(bytes) {
                let mut out = Vec::new();
                let mut cursor = Cursor::new(&mut out);
                dynamic
                    .write_to(&mut cursor, ImgFormat::Png)
                    .map_err(|e| format!("Fallback PNG encoding failed: {}", e))?;
                Ok((ImageFormat::Png, out))
            } else {
                Err("Unrecognized image format".to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_format_detection() {
        let png_header = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";
        assert_eq!(detect_image_format(png_header), ImageFormat::Png);

        let webp_header = b"RIFF\x20\x00\x00\x00WEBPVP8 ";
        assert_eq!(detect_image_format(webp_header), ImageFormat::WebP);

        let bmp_header = b"BM\x36\x00\x00\x00\x00\x00";
        assert_eq!(detect_image_format(bmp_header), ImageFormat::Bmp);
        assert!(is_uncompressed_image(bmp_header));
    }

    #[test]
    fn test_rgba_to_png_roundtrip() {
        // Create 4x4 red RGBA image
        let width = 4;
        let height = 4;
        let mut rgba = Vec::with_capacity(width * height * 4);
        for _ in 0..(width * height) {
            rgba.extend_from_slice(&[255, 0, 0, 255]);
        }

        let png_bytes = rgba_to_png(width as u32, height as u32, &rgba).expect("Encode PNG");
        assert_eq!(detect_image_format(&png_bytes), ImageFormat::Png);

        let (dec_w, dec_h, dec_rgba) = png_to_rgba(&png_bytes).expect("Decode PNG");
        assert_eq!(dec_w, width as u32);
        assert_eq!(dec_h, height as u32);
        assert_eq!(dec_rgba, rgba);
    }

    #[test]
    fn test_bmp_transcode_compression() {
        // Create 16x16 gradient image and export to BMP
        let width = 16;
        let height = 16;
        let mut rgba = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                rgba.extend_from_slice(&[(x * 15) as u8, (y * 15) as u8, 128, 255]);
            }
        }
        let png_bytes = rgba_to_png(width as u32, height as u32, &rgba).unwrap();
        let bmp_bytes = png_to_bmp(&png_bytes).expect("Encode BMP");

        assert_eq!(detect_image_format(&bmp_bytes), ImageFormat::Bmp);
        assert!(is_uncompressed_image(&bmp_bytes));

        // Transcode BMP -> WebP/PNG for sync
        let (fmt, compressed) = transcode_image_for_sync(&bmp_bytes).expect("Transcode for sync");
        assert!(fmt == ImageFormat::WebP || fmt == ImageFormat::Png);
        assert!(compressed.len() < bmp_bytes.len());

        // Verify content integrity
        let (w, h, roundtrip_rgba) = png_to_rgba(&compressed).expect("Decode transcode result");
        assert_eq!(w, width as u32);
        assert_eq!(h, height as u32);
        assert_eq!(roundtrip_rgba, rgba);
    }

    /// The Linux driver remembers `rgba_to_png(png_to_rgba(incoming))` as the echo fingerprint and later
    /// sees `rgba_to_png(clipboard pixels)`; that only works if re-encoding the same pixels is stable.
    #[test]
    fn reencoding_decoded_pixels_is_byte_stable_even_from_webp() {
        let rgba: Vec<u8> = (0..16 * 16 * 4).map(|i| (i * 7 % 251) as u8).collect();
        let webp = rgba_to_webp(16, 16, &rgba).unwrap();
        let (w, h, px) = png_to_rgba(&webp).unwrap();
        let first = rgba_to_png(w, h, &px).unwrap();
        let (w2, h2, px2) = png_to_rgba(&first).unwrap();
        assert_eq!(rgba_to_png(w2, h2, &px2).unwrap(), first);
        assert_ne!(
            first, webp,
            "the incoming bytes are not what read_clip sees"
        );
    }

    #[test]
    fn test_webp_roundtrip() {
        let width = 8;
        let height = 8;
        let mut rgba = Vec::with_capacity(width * height * 4);
        for _ in 0..(width * height) {
            rgba.extend_from_slice(&[0, 255, 128, 255]);
        }

        let webp_bytes = rgba_to_webp(width as u32, height as u32, &rgba).expect("Encode WebP");
        assert_eq!(detect_image_format(&webp_bytes), ImageFormat::WebP);

        let (dec_w, dec_h, dec_rgba) = png_to_rgba(&webp_bytes).expect("Decode WebP to RGBA");
        assert_eq!(dec_w, width as u32);
        assert_eq!(dec_h, height as u32);
        assert_eq!(dec_rgba, rgba);
    }

    /// A real PNG whose width/height fields are overwritten (CRC fixed), to describe huge images
    /// without making them.
    fn png_claiming(width: u32, height: u32) -> Vec<u8> {
        fn crc32(data: &[u8]) -> u32 {
            let mut crc = 0xFFFF_FFFFu32;
            for b in data {
                crc ^= u32::from(*b);
                for _ in 0..8 {
                    crc = if crc & 1 != 0 {
                        (crc >> 1) ^ 0xEDB8_8320
                    } else {
                        crc >> 1
                    };
                }
            }
            !crc
        }
        let mut png = rgba_to_png(2, 2, &[255u8; 16]).unwrap();
        png[16..20].copy_from_slice(&width.to_be_bytes());
        png[20..24].copy_from_slice(&height.to_be_bytes());
        let crc = crc32(&png[12..29]); // "IHDR" + 13 data bytes
        png[29..33].copy_from_slice(&crc.to_be_bytes());
        png
    }

    /// A real lossless WebP whose VP8L header (14-bit width-1, 14-bit height-1) is overwritten.
    fn webp_claiming(width: u32, height: u32) -> Vec<u8> {
        let mut webp = rgba_to_webp(2, 2, &[255u8; 16]).unwrap();
        let at = webp
            .windows(4)
            .position(|w| w == b"VP8L")
            .expect("lossless webp")
            + 8;
        assert_eq!(webp[at], 0x2f, "VP8L signature");
        let bits = (width - 1) | ((height - 1) << 14);
        let old = u32::from_le_bytes([webp[at + 1], webp[at + 2], webp[at + 3], webp[at + 4]]);
        let new = (old & 0xF000_0000) | bits; // keep alpha + version bits
        webp[at + 1..at + 5].copy_from_slice(&new.to_le_bytes());
        webp
    }

    #[test]
    fn a_webp_bomb_header_and_the_exact_webp_limit_are_handled_like_png() {
        assert_eq!(
            validate_image(ImageFormat::WebP, &webp_claiming(6000, 6000)),
            Ok((6000, 6000))
        );
        assert!(validate_image(ImageFormat::WebP, &webp_claiming(6000, 6001)).is_err());
        let err = validate_image(ImageFormat::WebP, &webp_claiming(16000, 16000)).unwrap_err();
        assert!(err.contains("16000"), "{err}");
    }

    #[test]
    fn decoding_refuses_a_huge_image_from_its_header_before_allocating() {
        let err = png_to_rgba(&png_claiming(8000, 8000)).unwrap_err();
        assert!(
            err.contains("limit"),
            "must be refused by the pixel limit, got: {err}"
        );
        let err = png_to_rgba(&webp_claiming(16000, 16000)).unwrap_err();
        assert!(err.contains("limit"), "{err}");
        // A normal image still decodes.
        let png = rgba_to_png(3, 2, &[7u8; 24]).unwrap();
        assert_eq!(png_to_rgba(&png).unwrap().0, 3);
    }

    #[test]
    fn a_thumbnail_keeps_the_aspect_ratio_and_never_upscales() {
        let big = rgba_to_png(800, 600, &vec![90u8; 800 * 600 * 4]).unwrap();
        let t = thumbnail_png(&big, 240).unwrap();
        assert_eq!(detect_image_format(&t), ImageFormat::Png);
        let (w, h, _) = png_to_rgba(&t).unwrap();
        assert_eq!((w, h), (240, 180));

        let tall = rgba_to_png(100, 400, &vec![5u8; 100 * 400 * 4]).unwrap();
        let (w, h, _) = png_to_rgba(&thumbnail_png(&tall, 240).unwrap()).unwrap();
        assert_eq!((w, h), (60, 240));

        let small = rgba_to_png(100, 50, &vec![7u8; 100 * 50 * 4]).unwrap();
        let (w, h, _) = png_to_rgba(&thumbnail_png(&small, 240).unwrap()).unwrap();
        assert_eq!((w, h), (100, 50), "a small image stays as it is");
    }

    #[test]
    fn a_thumbnail_of_a_bomb_or_garbage_is_refused() {
        let err = thumbnail_png(&png_claiming(60_000, 60_000), 240).unwrap_err();
        assert!(err.contains("limit"), "{err}");
        assert!(thumbnail_png(b"not an image", 240).is_err());
        assert!(thumbnail_png(b"", 240).is_err());
        assert!(thumbnail_png(&rgba_to_png(2, 2, &[1u8; 16]).unwrap(), 0).is_err());
    }

    #[test]
    fn real_png_and_webp_pass_validation_with_their_size() {
        let rgba = vec![200u8; 8 * 4 * 4];
        let png = rgba_to_png(8, 4, &rgba).unwrap();
        assert_eq!(validate_image(ImageFormat::Png, &png), Ok((8, 4)));
        let webp = rgba_to_webp(8, 4, &rgba).unwrap();
        assert_eq!(validate_image(ImageFormat::WebP, &webp), Ok((8, 4)));
    }

    #[test]
    fn a_mislabelled_or_broken_image_is_refused() {
        let rgba = vec![1u8; 4 * 4 * 4];
        let png = rgba_to_png(4, 4, &rgba).unwrap();
        let webp = rgba_to_webp(4, 4, &rgba).unwrap();
        assert!(validate_image(ImageFormat::WebP, &png).is_err());
        assert!(validate_image(ImageFormat::Png, &webp).is_err());
        assert!(validate_image(ImageFormat::Bmp, &png).is_err());
        assert!(validate_image(ImageFormat::Png, b"").is_err());
        assert!(validate_image(ImageFormat::Png, b"not an image at all").is_err());
        assert!(
            validate_image(ImageFormat::Png, &png[..20]).is_err(),
            "truncated header"
        );
    }

    #[test]
    fn the_pixel_limit_is_exact_and_a_decompression_bomb_header_is_refused() {
        assert_eq!(
            validate_image(ImageFormat::Png, &png_claiming(6000, 6000)),
            Ok((6000, 6000)),
            "exactly the limit is allowed"
        );
        assert!(validate_image(ImageFormat::Png, &png_claiming(6000, 6001)).is_err());
        let bomb = png_claiming(60_000, 60_000);
        assert!(bomb.len() < 100, "the bomb is a few bytes");
        let err = validate_image(ImageFormat::Png, &bomb).unwrap_err();
        assert!(err.contains("60000"), "{err}");
        assert!(!too_many_pixels(6000, 6000) && too_many_pixels(6000, 6001));
        assert!(too_many_pixels(u32::MAX, u32::MAX), "no overflow");
    }
}
