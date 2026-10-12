use crate::aead::SymmetricKey;
use hmac::{Hmac, Mac};
use klip_proto::hlc::NodeId;
use sha2::Sha256;
use zeroize::{Zeroize, ZeroizeOnDrop};

type HmacSha256 = Hmac<Sha256>;

/// Pairing Pre-Shared Key (PSK) for mutual out-of-band device authentication.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct PairingPsk([u8; 32]);

impl PairingPsk {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn generate() -> Self {
        let mut key = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut key);
        Self(key)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_symmetric_key(&self) -> SymmetricKey {
        SymmetricKey::new(self.0)
    }
}

/// Compute 6-digit Short Authentication String (SAS).
/// SAS = HMAC-SHA256(PSK, NodeId_A || NodeId_B)[0..3] (mod 1_000_000)
pub fn compute_sas(psk: &PairingPsk, node_a: &NodeId, node_b: &NodeId) -> u32 {
    let mut mac =
        HmacSha256::new_from_slice(psk.as_bytes()).expect("HMAC can accept key of any size");

    // Canonical order: sort node IDs so both sides compute identical SAS regardless of who initiated
    if node_a <= node_b {
        mac.update(node_a.as_bytes());
        mac.update(node_b.as_bytes());
    } else {
        mac.update(node_b.as_bytes());
        mac.update(node_a.as_bytes());
    }

    let result = mac.finalize().into_bytes();
    let num = u32::from_be_bytes([result[0], result[1], result[2], result[3]]);
    num % 1_000_000
}

/// Compute SAS from invitation PSK and host node ID (displayed immediately on pairing cards).
pub fn compute_sas_invitation(psk: &PairingPsk, host_node: &NodeId) -> u32 {
    let mut mac =
        HmacSha256::new_from_slice(psk.as_bytes()).expect("HMAC can accept key of any size");
    mac.update(b"KLIP-PAIRING-INVITATION-SAS-V1");
    mac.update(host_node.as_bytes());
    let result = mac.finalize().into_bytes();
    let num = u32::from_be_bytes([result[0], result[1], result[2], result[3]]);
    num % 1_000_000
}

/// Format SAS into aesthetic Charm / Raycast grade 6-digit pill: "742 • 109"
pub fn format_sas_pin(sas: u32) -> String {
    let part1 = (sas / 1000) % 1000;
    let part2 = sas % 1000;
    format!("{:03} • {:03}", part1, part2)
}

/// Compact pairing invitation payload
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct PairingInvitation {
    pub host_node: NodeId,
    pub psk_hex: String,
    pub timestamp: u64,
}

impl PairingInvitation {
    pub fn create(node: NodeId, psk: &PairingPsk) -> Self {
        let mut hex = String::with_capacity(64);
        for b in psk.as_bytes() {
            hex.push_str(&format!("{:02x}", b));
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            host_node: node,
            psk_hex: hex,
            timestamp: now,
        }
    }

    pub fn to_token(&self) -> String {
        let json = serde_json::to_string(self).unwrap_or_default();
        hex::encode(json)
    }

    pub fn from_token(token: &str) -> Result<Self, String> {
        let bytes = hex::decode(token.trim()).map_err(|e| format!("Invalid hex token: {}", e))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid pairing payload: {}", e))
    }

    pub fn parse_psk(&self) -> Result<PairingPsk, String> {
        let bytes = hex::decode(&self.psk_hex).map_err(|e| format!("Invalid PSK hex: {}", e))?;
        if bytes.len() != 32 {
            return Err("PSK must be 32 bytes".to_string());
        }
        let mut psk = [0u8; 32];
        psk.copy_from_slice(&bytes);
        Ok(PairingPsk::new(psk))
    }
}

// Minimal hex helpers
mod hex {
    pub fn encode<T: AsRef<[u8]>>(data: T) -> String {
        let mut s = String::new();
        for b in data.as_ref() {
            s.push_str(&format!("{:02x}", b));
        }
        s
    }

    pub fn decode(hex: &str) -> Result<Vec<u8>, String> {
        if hex.len() % 2 != 0 {
            return Err("Odd length hex string".to_string());
        }
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| e.to_string()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sas_symmetry() {
        let psk = PairingPsk::generate();
        let node_a = NodeId::new([10u8; 32]);
        let node_b = NodeId::new([20u8; 32]);

        let sas_from_a = compute_sas(&psk, &node_a, &node_b);
        let sas_from_b = compute_sas(&psk, &node_b, &node_a);

        assert_eq!(sas_from_a, sas_from_b);
        assert!(sas_from_a < 1_000_000);

        let pin_str = format_sas_pin(sas_from_a);
        assert_eq!(pin_str.chars().count(), 9); // "xxx • xxx"
    }

    #[test]
    fn test_pairing_invitation_token_roundtrip() {
        let psk = PairingPsk::generate();
        let node = NodeId::new([42u8; 32]);

        let invite = PairingInvitation::create(node, &psk);
        let token = invite.to_token();

        let parsed = PairingInvitation::from_token(&token).expect("Token parse must succeed");
        assert_eq!(parsed.host_node, node);

        let parsed_psk = parsed.parse_psk().expect("PSK parse must succeed");
        assert_eq!(parsed_psk.as_bytes(), psk.as_bytes());
    }
}
