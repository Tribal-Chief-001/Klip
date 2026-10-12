use crate::aead::SymmetricKey;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use klip_proto::hlc::NodeId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Device Identity with Ed25519 keypair.
pub struct DeviceIdentity {
    pub signing_key: SigningKey,
    pub verifying_key: VerifyingKey,
    pub node_id: NodeId,
}

impl DeviceIdentity {
    pub fn generate() -> Self {
        let mut csprng = rand::rngs::OsRng;
        let signing_key = SigningKey::generate(&mut csprng);
        let verifying_key = signing_key.verifying_key();
        let node_id = NodeId::new(verifying_key.to_bytes());
        Self {
            signing_key,
            verifying_key,
            node_id,
        }
    }

    pub fn from_seed(seed: &[u8; 32]) -> Self {
        let signing_key = SigningKey::from_bytes(seed);
        let verifying_key = signing_key.verifying_key();
        let node_id = NodeId::new(verifying_key.to_bytes());
        Self {
            signing_key,
            verifying_key,
            node_id,
        }
    }

    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.signing_key.sign(message).to_bytes()
    }

    pub fn verify(node_id: &NodeId, message: &[u8], signature_bytes: &[u8; 64]) -> bool {
        if let Ok(vk) = VerifyingKey::from_bytes(node_id.as_bytes()) {
            let sig = Signature::from_bytes(signature_bytes);
            vk.verify(message, &sig).is_ok()
        } else {
            false
        }
    }
}

/// Stored entry for a trusted paired peer.
#[derive(Clone, Serialize, Deserialize)]
pub struct PairedPeer {
    pub node_id: NodeId,
    pub name: String,
    pub key_hex: String,
    pub paired_at_secs: u64,
}

impl PairedPeer {
    pub fn new(node_id: NodeId, name: String, key: &SymmetricKey) -> Self {
        let mut key_hex = String::with_capacity(64);
        for b in key.as_bytes() {
            key_hex.push_str(&format!("{:02x}", b));
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            node_id,
            name,
            key_hex,
            paired_at_secs: now,
        }
    }

    pub fn get_symmetric_key(&self) -> Result<SymmetricKey, String> {
        let bytes = hex_decode(&self.key_hex)?;
        if bytes.len() != 32 {
            return Err("Key is not 32 bytes".to_string());
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(SymmetricKey::new(arr))
    }
}

/// Keystore holding node identity and paired peers.
#[derive(Default, Serialize, Deserialize)]
pub struct KeyStore {
    pub device_name: String,
    pub identity_seed_hex: String,
    pub peers: HashMap<String, PairedPeer>,
}

impl KeyStore {
    pub fn new(device_name: String) -> (Self, DeviceIdentity) {
        let identity = DeviceIdentity::generate();
        let mut seed_hex = String::with_capacity(64);
        for b in identity.signing_key.as_bytes() {
            seed_hex.push_str(&format!("{:02x}", b));
        }

        let store = Self {
            device_name,
            identity_seed_hex: seed_hex,
            peers: HashMap::new(),
        };
        (store, identity)
    }

    pub fn load_or_create<P: AsRef<Path>>(
        path: P,
        default_name: &str,
    ) -> Result<(Self, DeviceIdentity), String> {
        let p = path.as_ref();
        if p.exists() {
            let data = fs::read_to_string(p).map_err(|e| e.to_string())?;
            let store: KeyStore = serde_json::from_str(&data).map_err(|e| e.to_string())?;
            let seed = hex_decode(&store.identity_seed_hex)?;
            if seed.len() != 32 {
                return Err("Corrupted identity seed".to_string());
            }
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&seed);
            let identity = DeviceIdentity::from_seed(&arr);
            Ok((store, identity))
        } else {
            if let Some(parent) = p.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let (store, identity) = Self::new(default_name.to_string());
            store.save(p)?;
            Ok((store, identity))
        }
    }

    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(path, json).map_err(|e| e.to_string())
    }

    pub fn add_peer(&mut self, peer: PairedPeer) {
        self.peers.insert(peer.node_id.to_hex(), peer);
    }

    pub fn get_peer(&self, node_id: &NodeId) -> Option<&PairedPeer> {
        self.peers.get(&node_id.to_hex())
    }
}

fn hex_decode(hex: &str) -> Result<Vec<u8>, String> {
    if hex.len() % 2 != 0 {
        return Err("Odd length hex string".to_string());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identity_signature() {
        let identity = DeviceIdentity::generate();
        let message = b"Sign this clipboard update";
        let sig = identity.sign(message);
        assert!(DeviceIdentity::verify(&identity.node_id, message, &sig));

        let wrong_node = NodeId::new([99u8; 32]);
        assert!(!DeviceIdentity::verify(&wrong_node, message, &sig));
    }

    #[test]
    fn test_keystore_peer_persistence() {
        let (mut store, _id) = KeyStore::new("TestMachine".to_string());
        let peer_node = NodeId::new([77u8; 32]);
        let sym_key = SymmetricKey::random();
        let peer = PairedPeer::new(peer_node, "Pixel-8".to_string(), &sym_key);

        store.add_peer(peer);
        assert!(store.get_peer(&peer_node).is_some());

        let loaded_key = store
            .get_peer(&peer_node)
            .unwrap()
            .get_symmetric_key()
            .unwrap();
        assert_eq!(loaded_key.as_bytes(), sym_key.as_bytes());
    }
}
