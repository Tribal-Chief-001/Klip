use serde::{Deserialize, Serialize};

/// ALPN of the clipboard sync protocol (paired peers only).
pub const SYNC_ALPN: &[u8] = b"klip/sync/1";
/// ALPN of the pairing protocol (open only while the host has a pairing window).
pub const PAIR_ALPN: &[u8] = b"klip/pair/1";

/// Upper bound for small control messages (pairing).
pub const MAX_CONTROL_BYTES: usize = 4096;
/// Hard cap on device names to keep UI/log output sane.
pub const MAX_DEVICE_NAME_CHARS: usize = 64;

#[derive(Serialize, Deserialize)]
pub(crate) struct PairRequest {
    pub secret: [u8; 16],
    pub device_name: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) enum PairReply {
    Accepted { device_name: String },
    Rejected { reason: String },
}

/// Strip control characters and limit length of a peer-supplied display name.
pub(crate) fn sanitize_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_DEVICE_NAME_CHARS)
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "Unnamed device".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_control_and_limits() {
        assert_eq!(sanitize_name("Pixel\n\x1b[31m 8"), "Pixel[31m 8");
        assert_eq!(sanitize_name("   "), "Unnamed device");
        assert_eq!(
            sanitize_name(&"x".repeat(500)).chars().count(),
            MAX_DEVICE_NAME_CHARS
        );
    }
}
