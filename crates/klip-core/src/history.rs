use klip_proto::hlc::NodeId;
use klip_proto::ipc::HistoryItem;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

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
            full_content: content,
            size_bytes,
            is_sensitive,
        };

        let mut lock = self.items.lock().unwrap();
        if lock.len() >= self.capacity {
            lock.pop_back();
        }
        lock.push_front(item);
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
}
