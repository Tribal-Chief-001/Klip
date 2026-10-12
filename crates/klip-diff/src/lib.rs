pub mod cf_html;
pub mod delta;
pub mod entropy;
pub mod image_transcode;

pub use cf_html::{cf_html_to_utf8, utf8_to_cf_html};
pub use delta::{
    chunk_with_fastcdc, ContentChunk, DeltaPatch, RecentClipCache, MIN_DIFF_PAYLOAD_SIZE,
    RECENT_CACHE_CAPACITY,
};
pub use entropy::{
    analyze_snippet, calculate_shannon_entropy, EntropyAnalysis, ENTROPY_THRESHOLD,
    MIN_SENSITIVE_LENGTH,
};
pub use image_transcode::{
    detect_image_format, is_uncompressed_image, png_to_bmp, png_to_rgba, rgba_to_png, rgba_to_webp,
    transcode_image_for_sync, transcode_uncompressed_bmp_to_png,
    transcode_uncompressed_bmp_to_webp, ImageFormat,
};
