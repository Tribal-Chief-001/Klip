use crate::hlc::{HybridLogicalClock, NodeId};
use serde::{Deserialize, Serialize};

/// Clipboard content kinds that can travel between devices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipContentType {
    PlainTextUtf8,
    HtmlContent,
    ImageWebP,
    ImagePng,
    CustomMime(String),
}

/// One clipboard item as exchanged between paired devices (serialized with postcard inside an
/// authenticated QUIC stream).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipPayload {
    pub id: [u8; 16],
    pub content_type: ClipContentType,
    pub content: Vec<u8>,
    pub origin_node: NodeId,
    pub hlc: HybridLogicalClock,
    pub fingerprint: [u8; 32],
    pub is_sensitive: bool,
}

impl ClipPayload {
    pub fn new(
        content_type: ClipContentType,
        content: Vec<u8>,
        origin_node: NodeId,
        hlc: HybridLogicalClock,
        is_sensitive: bool,
    ) -> Self {
        let mut id = [0u8; 16];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut id);
        let fingerprint = compute_fingerprint(&content, &origin_node);
        Self {
            id,
            content_type,
            content,
            origin_node,
            hlc,
            fingerprint,
            is_sensitive,
        }
    }
}

/// BLAKE3 fingerprint used by the echo-loop shield: `BLAKE3(content || origin_node)`.
pub fn compute_fingerprint(content: &[u8], origin: &NodeId) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(content);
    hasher.update(origin.as_bytes());
    *hasher.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fingerprint_deterministic() {
        let node = NodeId::new([1u8; 32]);
        let fp1 = compute_fingerprint(b"clipboard secret", &node);
        let fp2 = compute_fingerprint(b"clipboard secret", &node);
        assert_eq!(fp1, fp2);

        let other_node = NodeId::new([2u8; 32]);
        let fp3 = compute_fingerprint(b"clipboard secret", &other_node);
        assert_ne!(fp1, fp3);
    }

    #[test]
    fn payload_serde_roundtrip() {
        let node = NodeId::new([5u8; 32]);
        let hlc = HybridLogicalClock::new(node);
        let p = ClipPayload::new(
            ClipContentType::PlainTextUtf8,
            b"hi".to_vec(),
            node,
            hlc,
            true,
        );
        // serde_json is only a convenient symmetric check of the derive set (wire format is postcard)
        let json = serde_json::to_string(&p).unwrap();
        let back: ClipPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }
}
