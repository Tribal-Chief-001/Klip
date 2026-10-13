use klip_proto::hlc::NodeId;
use klip_proto::ipc::HistoryItem;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// A clip larger than this keeps only its preview in history (the OS clipboard holds the content).
pub const MAX_STORED_ITEM_BYTES: usize = 256 * 1024;
/// Total bytes of stored content across all items; the oldest content is dropped first.
pub const MAX_STORED_TOTAL_BYTES: usize = 8 * 1024 * 1024;

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
        } else if content.starts_with(b"BM") {
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
}
