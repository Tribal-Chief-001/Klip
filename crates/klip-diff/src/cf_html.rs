/// Bidirectional CF_HTML <-> UTF-8 Translation.
/// Windows uses CF_HTML with byte offset headers:
/// Version:0.9
/// StartHTML:0000000000
/// EndHTML:0000000000
/// StartFragment:0000000000
/// EndFragment:0000000000

pub const CF_HTML_HEADER_PREFIX: &str = "Version:0.9\r\n";
pub const FRAGMENT_START_MARKER: &str = "<!--StartFragment-->";
pub const FRAGMENT_END_MARKER: &str = "<!--EndFragment-->";

/// Extracts clean UTF-8 HTML from Windows CF_HTML clipboard format.
pub fn cf_html_to_utf8(cf_html: &str) -> String {
    // Attempt byte-offset based extraction if markers match
    if let (Some(start_idx), Some(end_idx)) = (
        cf_html.find(FRAGMENT_START_MARKER),
        cf_html.find(FRAGMENT_END_MARKER),
    ) {
        let content_start = start_idx + FRAGMENT_START_MARKER.len();
        if content_start <= end_idx {
            if let Some(slice) = cf_html.get(content_start..end_idx) {
                return slice.to_string();
            }
        }
    }

    // Fallback: parse StartFragment header value
    for line in cf_html.lines() {
        if line.starts_with("StartFragment:") {
            if let Ok(offset) = line["StartFragment:".len()..].trim().parse::<usize>() {
                if let Some(end_offset_line) = cf_html.lines().find(|l| l.starts_with("EndFragment:")) {
                    if let Ok(end) = end_offset_line["EndFragment:".len()..].trim().parse::<usize>() {
                        if offset <= end {
                            if let Some(slice) = cf_html.get(offset..end) {
                                return slice.to_string();
                            }
                        }
                    }
                }
                if let Some(slice) = cf_html.get(offset..) {
                    return slice.to_string();
                }
            }
        }
    }

    // Return as-is if no CF_HTML envelope detected
    cf_html.to_string()
}

/// Converts clean UTF-8 HTML into Windows CF_HTML format with calculated byte offsets.
pub fn utf8_to_cf_html(html: &str) -> String {
    let dummy_header = "Version:0.9\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n";
    let prefix = "<html><body><!--StartFragment-->";
    let suffix = "<!--EndFragment--></body></html>";

    let header_len = dummy_header.len();
    let start_html = header_len;
    let start_fragment = header_len + prefix.len();
    let end_fragment = start_fragment + html.len();
    let end_html = end_fragment + suffix.len();

    format!(
        "Version:0.9\r\nStartHTML:{:010}\r\nEndHTML:{:010}\r\nStartFragment:{:010}\r\nEndFragment:{:010}\r\n{}{}{}",
        start_html, end_html, start_fragment, end_fragment, prefix, html, suffix
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cf_html_roundtrip() {
        let original_html = "<b>Bold Klip Architecture</b> <a href=\"https://github.com\">Link</a>";
        let cf_format = utf8_to_cf_html(original_html);

        assert!(cf_format.contains("Version:0.9"));
        assert!(cf_format.contains("StartFragment:"));

        let extracted = cf_html_to_utf8(&cf_format);
        assert_eq!(extracted, original_html);
    }
}
