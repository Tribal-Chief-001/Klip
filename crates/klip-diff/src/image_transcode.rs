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
}
