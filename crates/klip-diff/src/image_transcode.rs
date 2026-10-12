/// Image format detection and compression pipeline.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    WebP,
    Bmp,
    Unknown,
}

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

pub fn is_uncompressed_image(bytes: &[u8]) -> bool {
    matches!(detect_image_format(bytes), ImageFormat::Bmp)
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
}
