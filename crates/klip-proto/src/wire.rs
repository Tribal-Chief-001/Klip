use crate::hlc::{HybridLogicalClock, NodeId};
use serde::{Deserialize, Serialize};
use std::fmt;

pub const MAGIC_BYTES: [u8; 4] = *b"KLIP";
pub const CURRENT_VERSION: u8 = 1;
pub const HEADER_LEN: usize = 4 + 1 + 1 + 2 + 32 + 12 + 16 + 4; // 72 bytes
pub const TAG_LEN: usize = 16;
pub const MAX_PAYLOAD_SIZE: usize = 64 * 1024 * 1024; // 64 MB maximum safe clipboard transfer

// Flags
pub const FLAG_ENCRYPTED: u16 = 0x0001;
pub const FLAG_DELTA: u16 = 0x0002;
pub const FLAG_SENSITIVE: u16 = 0x0004;
pub const FLAG_COMPRESSED: u16 = 0x0008;

// Content Type Flags (bits 4-7)
pub const FLAG_TYPE_TEXT: u16 = 0x0000;
pub const FLAG_TYPE_HTML: u16 = 0x0010;
pub const FLAG_TYPE_IMAGE_PNG: u16 = 0x0020;
pub const FLAG_TYPE_IMAGE_WEBP: u16 = 0x0030;
pub const FLAG_TYPE_MASK: u16 = 0x00F0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    Handshake = 0x01,
    HandshakeAck = 0x02,
    ClipFullPayload = 0x10,
    ClipDeltaPatch = 0x11,
    MailboxQuery = 0x20,
    MailboxDeliver = 0x21,
    MailboxAck = 0x22,
    HeartbeatPing = 0xF0,
    HeartbeatPong = 0xF1,
    ControlCommand = 0xFE,
}

impl TryFrom<u8> for MessageType {
    type Error = ProtoError;

    fn try_from(val: u8) -> Result<Self, Self::Error> {
        match val {
            0x01 => Ok(Self::Handshake),
            0x02 => Ok(Self::HandshakeAck),
            0x10 => Ok(Self::ClipFullPayload),
            0x11 => Ok(Self::ClipDeltaPatch),
            0x20 => Ok(Self::MailboxQuery),
            0x21 => Ok(Self::MailboxDeliver),
            0x22 => Ok(Self::MailboxAck),
            0xF0 => Ok(Self::HeartbeatPing),
            0xF1 => Ok(Self::HeartbeatPong),
            0xFE => Ok(Self::ControlCommand),
            other => Err(ProtoError::InvalidMessageType(other)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipContentType {
    PlainTextUtf8,
    HtmlContent,
    ImageWebP,
    ImagePng,
    CustomMime(String),
}

impl ClipContentType {
    pub fn to_flag(&self) -> u16 {
        match self {
            Self::PlainTextUtf8 => FLAG_TYPE_TEXT,
            Self::HtmlContent => FLAG_TYPE_HTML,
            Self::ImagePng => FLAG_TYPE_IMAGE_PNG,
            Self::ImageWebP => FLAG_TYPE_IMAGE_WEBP,
            Self::CustomMime(_) => FLAG_TYPE_TEXT,
        }
    }

    pub fn from_flags(flags: u16) -> Self {
        match flags & FLAG_TYPE_MASK {
            FLAG_TYPE_HTML => Self::HtmlContent,
            FLAG_TYPE_IMAGE_PNG => Self::ImagePng,
            FLAG_TYPE_IMAGE_WEBP => Self::ImageWebP,
            _ => Self::PlainTextUtf8,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct WireNonce(pub [u8; 12]);

impl WireNonce {
    pub fn new(counter: u64) -> Self {
        let mut bytes = [0u8; 12];
        bytes[4..12].copy_from_slice(&counter.to_be_bytes());
        Self(bytes)
    }

    pub fn counter(&self) -> u64 {
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&self.0[4..12]);
        u64::from_be_bytes(buf)
    }

    pub fn as_bytes(&self) -> &[u8; 12] {
        &self.0
    }
}

impl fmt::Debug for WireNonce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WireNonce(counter={})", self.counter())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireHeader {
    pub magic: [u8; 4],
    pub version: u8,
    pub msg_type: MessageType,
    pub flags: u16,
    pub sender: NodeId,
    pub nonce: WireNonce,
    pub hlc_physical: u64,
    pub hlc_logical: u32,
    pub hlc_node_seed: u32,
    pub payload_len: u32,
}

impl WireHeader {
    pub fn to_bytes(&self) -> [u8; HEADER_LEN] {
        let mut buf = [0u8; HEADER_LEN];
        buf[0..4].copy_from_slice(&self.magic);
        buf[4] = self.version;
        buf[5] = self.msg_type as u8;
        buf[6..8].copy_from_slice(&self.flags.to_be_bytes());
        buf[8..40].copy_from_slice(self.sender.as_bytes());
        buf[40..52].copy_from_slice(self.nonce.as_bytes());
        buf[52..60].copy_from_slice(&self.hlc_physical.to_be_bytes());
        buf[60..64].copy_from_slice(&self.hlc_logical.to_be_bytes());
        buf[64..68].copy_from_slice(&self.hlc_node_seed.to_be_bytes());
        buf[68..72].copy_from_slice(&self.payload_len.to_be_bytes());
        buf
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WirePacket {
    pub header: WireHeader,
    pub payload: Vec<u8>,
    pub tag: [u8; TAG_LEN],
}

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

/// Computes BLAKE3 fingerprint for Echo Loop Shield:
/// Fingerprint = BLAKE3(Content || OriginNodeId)
pub fn compute_fingerprint(content: &[u8], origin: &NodeId) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(content);
    hasher.update(origin.as_bytes());
    *hasher.finalize().as_bytes()
}

#[derive(Debug, PartialEq, Eq)]
pub enum ProtoError {
    InvalidMagic,
    UnsupportedVersion(u8),
    InvalidMessageType(u8),
    PayloadTooLarge(usize),
    BufferTooShort,
    Serialization(String),
}

impl fmt::Display for ProtoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMagic => write!(f, "Invalid KLIP magic header"),
            Self::UnsupportedVersion(v) => write!(f, "Unsupported protocol version: {}", v),
            Self::InvalidMessageType(m) => write!(f, "Unknown message type: {:#04x}", m),
            Self::PayloadTooLarge(s) => write!(f, "Payload exceeds max size: {} bytes", s),
            Self::BufferTooShort => write!(f, "Packet buffer too short for header"),
            Self::Serialization(e) => write!(f, "Serialization error: {}", e),
        }
    }
}

impl std::error::Error for ProtoError {}

impl WirePacket {
    pub fn encode(&self) -> Vec<u8> {
        let total_size = HEADER_LEN + self.payload.len() + TAG_LEN;
        let mut buf = Vec::with_capacity(total_size);

        // Header: 72 bytes
        buf.extend_from_slice(&self.header.to_bytes());

        // Payload
        buf.extend_from_slice(&self.payload);

        // Tag
        buf.extend_from_slice(&self.tag);

        buf
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ProtoError> {
        if bytes.len() < HEADER_LEN + TAG_LEN {
            return Err(ProtoError::BufferTooShort);
        }

        let magic = [bytes[0], bytes[1], bytes[2], bytes[3]];
        if magic != MAGIC_BYTES {
            return Err(ProtoError::InvalidMagic);
        }

        let version = bytes[4];
        if version != CURRENT_VERSION {
            return Err(ProtoError::UnsupportedVersion(version));
        }

        let msg_type = MessageType::try_from(bytes[5])?;
        let flags = u16::from_be_bytes([bytes[6], bytes[7]]);

        let mut sender = [0u8; 32];
        sender.copy_from_slice(&bytes[8..40]);

        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&bytes[40..52]);

        let mut hlc_phys = [0u8; 8];
        hlc_phys.copy_from_slice(&bytes[52..60]);
        let hlc_physical = u64::from_be_bytes(hlc_phys);

        let mut hlc_log = [0u8; 4];
        hlc_log.copy_from_slice(&bytes[60..64]);
        let hlc_logical = u32::from_be_bytes(hlc_log);

        let mut hlc_seed = [0u8; 4];
        hlc_seed.copy_from_slice(&bytes[64..68]);
        let hlc_node_seed = u32::from_be_bytes(hlc_seed);

        let mut p_len = [0u8; 4];
        p_len.copy_from_slice(&bytes[68..72]);
        let payload_len = u32::from_be_bytes(p_len) as usize;

        if payload_len > MAX_PAYLOAD_SIZE {
            return Err(ProtoError::PayloadTooLarge(payload_len));
        }

        let expected_total = HEADER_LEN + payload_len + TAG_LEN;
        if bytes.len() < expected_total {
            return Err(ProtoError::BufferTooShort);
        }

        let payload = bytes[HEADER_LEN..HEADER_LEN + payload_len].to_vec();

        let mut tag = [0u8; TAG_LEN];
        tag.copy_from_slice(&bytes[HEADER_LEN + payload_len..expected_total]);

        Ok(WirePacket {
            header: WireHeader {
                magic,
                version,
                msg_type,
                flags,
                sender: NodeId::new(sender),
                nonce: WireNonce(nonce),
                hlc_physical,
                hlc_logical,
                hlc_node_seed,
                payload_len: payload_len as u32,
            },
            payload,
            tag,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wire_packet_roundtrip() {
        let sender = NodeId::new([42u8; 32]);
        let packet = WirePacket {
            header: WireHeader {
                magic: MAGIC_BYTES,
                version: CURRENT_VERSION,
                msg_type: MessageType::ClipFullPayload,
                flags: FLAG_ENCRYPTED,
                sender,
                nonce: WireNonce::new(1001),
                hlc_physical: 1728000000000,
                hlc_logical: 5,
                hlc_node_seed: 0xDEADBEEF,
                payload_len: 11,
            },
            payload: b"hello world".to_vec(),
            tag: [7u8; TAG_LEN],
        };

        let encoded = packet.encode();
        assert_eq!(encoded.len(), HEADER_LEN + 11 + TAG_LEN);

        let decoded = WirePacket::decode(&encoded).expect("Decode should succeed");
        assert_eq!(decoded.header.magic, MAGIC_BYTES);
        assert_eq!(decoded.header.version, CURRENT_VERSION);
        assert_eq!(decoded.header.msg_type, MessageType::ClipFullPayload);
        assert_eq!(decoded.header.nonce.counter(), 1001);
        assert_eq!(decoded.payload, b"hello world");
        assert_eq!(decoded.tag, [7u8; TAG_LEN]);
    }

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
}
