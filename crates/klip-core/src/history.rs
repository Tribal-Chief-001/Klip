use klip_proto::clip::ClipContentType;
use klip_proto::hlc::NodeId;
use klip_proto::ipc::HistoryItem;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// A clip larger than this keeps only its preview in history (the OS clipboard holds the content).
pub const MAX_STORED_ITEM_BYTES: usize = 256 * 1024;
/// Total bytes of stored content across all items; the oldest content is dropped first.
pub const MAX_STORED_TOTAL_BYTES: usize = 8 * 1024 * 1024;

/// What a stored clip is, judged from its bytes (the history keeps no content type). Text is the
/// default; images are recognised only by their real magic bytes.
pub fn sniff_content_type(content: &[u8]) -> ClipContentType {
    if content.starts_with(b"\x89PNG\r\n\x1a\n") {
        ClipContentType::ImagePng
    } else if content.len() >= 12 && &content[0..4] == b"RIFF" && &content[8..12] == b"WEBP" {
        ClipContentType::ImageWebP
    } else {
        ClipContentType::PlainTextUtf8
    }
}

/// A Windows bitmap: `BM`, then a 14-byte file header, then a DIB header whose size field (u32 LE at
/// offset 14) is one of the known header sizes. Text that merely starts with "BM" does not match.
fn is_bitmap(content: &[u8]) -> bool {
    content.len() >= 26
        && content.starts_with(b"BM")
        && matches!(
            u32::from_le_bytes([content[14], content[15], content[16], content[17]]),
            12 | 40 | 52 | 56 | 64 | 108 | 124
        )
}

#[derive(Clone)]
pub struct HistoryBuffer {
    items: Arc<Mutex<VecDeque<HistoryItem>>>,
    capacity: usize,
}

impl HistoryBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            items: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
            capacity,
        }
    }

    pub fn push(
        &self,
        id: [u8; 16],
        origin_node: NodeId,
        origin_name: String,
        content: Vec<u8>,
        is_sensitive: bool,
    ) {
        let size_bytes = content.len();
        let preview = if is_sensitive {
            "•••••••••••••••• (Sensitive Token / Key)".to_string()
        } else if content.starts_with(b"\x89PNG\r\n\x1a\n") {
            format!("[Image: PNG ({} B)]", size_bytes)
        } else if content.len() >= 12 && &content[0..4] == b"RIFF" && &content[8..12] == b"WEBP" {
            format!("[Image: WebP ({} B)]", size_bytes)
        } else if is_bitmap(&content) {
            format!("[Image: Bitmap ({} B)]", size_bytes)
        } else {
            let s = String::from_utf8_lossy(&content);
            let mut preview_str = s.chars().take(80).collect::<String>();
            if s.chars().count() > 80 {
                preview_str.push_str("...");
            }
            preview_str
        };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let item = HistoryItem {
            id,
            origin_node,
            origin_name,
            timestamp_ms: now,
            content_preview: preview,
            full_content: if size_bytes <= MAX_STORED_ITEM_BYTES {
                content
            } else {
                Vec::new()
            },
            size_bytes,
            is_sensitive,
        };

        let mut lock = self.items.lock().unwrap();
        if lock.len() >= self.capacity {
            lock.pop_back();
        }
        lock.push_front(item);
        // Bound the memory history holds: strip the oldest stored content until under budget.
        let mut total: usize = lock.iter().map(|i| i.full_content.len()).sum();
        while total > MAX_STORED_TOTAL_BYTES {
            match lock.iter_mut().rev().find(|i| !i.full_content.is_empty()) {
                Some(oldest) => {
                    total -= oldest.full_content.len();
                    oldest.full_content = Vec::new();
                }
                None => break,
            }
        }
    }

    pub fn get_recent(&self, limit: usize) -> Vec<HistoryItem> {
        let lock = self.items.lock().unwrap();
        lock.iter().take(limit).cloned().collect()
    }

    pub fn get_by_id(&self, id: [u8; 16]) -> Option<HistoryItem> {
        let lock = self.items.lock().unwrap();
        lock.iter().find(|i| i.id == id).cloned()
    }

    /// Forget everything kept in history (local only; nothing is sent to other devices).
    pub fn clear(&self) {
        self.items.lock().unwrap().clear();
    }

    pub fn last_item(&self) -> Option<HistoryItem> {
        let lock = self.items.lock().unwrap();
        lock.front().cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_history_buffer_ring_capacity() {
        let buf = HistoryBuffer::new(3);
        let node = NodeId::new([1u8; 32]);

        buf.push([1; 16], node, "Node1".into(), b"clip 1".to_vec(), false);
        buf.push([2; 16], node, "Node1".into(), b"clip 2".to_vec(), false);
        buf.push([3; 16], node, "Node1".into(), b"clip 3".to_vec(), false);
        buf.push([4; 16], node, "Node1".into(), b"clip 4".to_vec(), false);

        let items = buf.get_recent(10);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].full_content, b"clip 4");
        assert_eq!(items[1].full_content, b"clip 3");
        assert_eq!(items[2].full_content, b"clip 2");
    }

    #[test]
    fn memory_is_bounded_even_for_huge_clips() {
        let node = NodeId::new([1u8; 32]);
        // 40 clips of 5 MiB: 200 MiB of input must not stay in memory.
        let buf = HistoryBuffer::new(50);
        for i in 0..40u8 {
            buf.push(
                [i; 16],
                node,
                "N".into(),
                vec![b'x'; 5 * 1024 * 1024],
                false,
            );
        }
        let items = buf.get_recent(100);
        assert_eq!(items.len(), 40, "every clip keeps its history line");
        assert!(items.iter().all(|i| i.full_content.is_empty()));
        assert!(items.iter().all(|i| i.size_bytes == 5 * 1024 * 1024));

        // Many mid-size clips: stored content stays under budget, newest kept longest.
        let buf = HistoryBuffer::new(50);
        for i in 0..50u8 {
            buf.push([i; 16], node, "N".into(), vec![b'y'; 200 * 1024], false);
        }
        let items = buf.get_recent(100);
        let held: usize = items.iter().map(|i| i.full_content.len()).sum();
        assert!(held <= MAX_STORED_TOTAL_BYTES, "{held}");
        assert!(
            !items[0].full_content.is_empty(),
            "newest keeps its content"
        );
        assert!(
            items[49].full_content.is_empty(),
            "oldest content evicted first"
        );
    }

    #[test]
    fn text_that_starts_with_bm_is_text_not_a_bitmap() {
        let buf = HistoryBuffer::new(3);
        let node = NodeId::new([1u8; 32]);
        buf.push(
            [1; 16],
            node,
            "n".into(),
            b"BMW M3 is a fast car indeed".to_vec(),
            false,
        );
        let items = buf.get_recent(1);
        assert_eq!(items[0].content_preview, "BMW M3 is a fast car indeed");
        assert_eq!(
            sniff_content_type(b"BMW M3 is a fast car indeed"),
            klip_proto::clip::ClipContentType::PlainTextUtf8
        );
    }

    #[test]
    fn a_real_bitmap_header_is_still_recognised() {
        let mut bmp = vec![b'B', b'M'];
        bmp.extend_from_slice(&[54, 0, 0, 0]); // file size
        bmp.extend_from_slice(&[0, 0, 0, 0]); // reserved
        bmp.extend_from_slice(&[54, 0, 0, 0]); // pixel data offset
        bmp.extend_from_slice(&[40, 0, 0, 0]); // DIB header size = BITMAPINFOHEADER
        bmp.extend_from_slice(&[0u8; 36]);
        let buf = HistoryBuffer::new(3);
        buf.push([2; 16], NodeId::new([1u8; 32]), "n".into(), bmp, false);
        assert!(buf.get_recent(1)[0]
            .content_preview
            .starts_with("[Image: Bitmap ("));
    }

    #[test]
    fn items_can_be_found_by_id_and_history_can_be_cleared() {
        let buf = HistoryBuffer::new(3);
        let node = NodeId::new([1u8; 32]);
        buf.push([7; 16], node, "n".into(), b"seven".to_vec(), false);
        buf.push([8; 16], node, "n".into(), b"eight".to_vec(), false);
        assert_eq!(buf.get_by_id([7; 16]).unwrap().full_content, b"seven");
        assert!(buf.get_by_id([9; 16]).is_none());
        buf.clear();
        assert!(buf.get_recent(10).is_empty());
        assert!(buf.get_by_id([7; 16]).is_none());
    }

    #[test]
    fn sniffing_recognises_png_and_webp_only_by_their_magic_bytes() {
        use klip_proto::clip::ClipContentType as T;
        assert_eq!(sniff_content_type(b"\x89PNG\r\n\x1a\nrest"), T::ImagePng);
        assert_eq!(
            sniff_content_type(b"RIFF\x00\x00\x00\x00WEBPVP8 "),
            T::ImageWebP
        );
        assert_eq!(sniff_content_type(b"RIFF is also a word"), T::PlainTextUtf8);
        assert_eq!(sniff_content_type(b""), T::PlainTextUtf8);
    }
}
