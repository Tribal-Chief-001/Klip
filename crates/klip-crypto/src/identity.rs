use crate::aead::SymmetricKey;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use klip_proto::hlc::NodeId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
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
            // Older versions wrote this file world/group readable; tighten on every load.
            restrict_to_owner(p, 0o600);
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
                restrict_to_owner(parent, 0o700);
            }
            let (store, identity) = Self::new(default_name.to_string());
            store.save(p)?;
            Ok((store, identity))
        }
    }

    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        write_private_file(path.as_ref(), json.as_bytes())
    }

    pub fn add_peer(&mut self, peer: PairedPeer) {
        self.peers.insert(peer.node_id.to_hex(), peer);
    }

    pub fn get_peer(&self, node_id: &NodeId) -> Option<&PairedPeer> {
        self.peers.get(&node_id.to_hex())
    }
}

/// Restrict a file or directory to the owner (no-op on non-Unix targets).
pub fn restrict_to_owner(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
}

/// Atomically write a secret file that is created with mode 0600 (never briefly world-readable).
fn write_private_file(path: &Path, data: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp).map_err(|e| e.to_string())?;
        // A pre-existing temp file keeps its old mode; force it.
        restrict_to_owner(&tmp, 0o600);
        f.write_all(data).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
    }
    fs::rename(&tmp, path).map_err(|e| e.to_string())
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

    #[cfg(unix)]
    #[test]
    fn test_keystore_is_private_and_tightens_existing_files() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("klip-ks-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("keystore.json");

        // Fresh store: file 0600, directory 0700.
        let (store, _id) = KeyStore::load_or_create(&path, "T").unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&dir), 0o700);

        // Simulate the legacy world/group-readable file; loading must repair it.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o664)).unwrap();
        let (_s2, _i2) = KeyStore::load_or_create(&path, "T").unwrap();
        assert_eq!(mode(&path), 0o600);

        // Saving again must keep it private and leave no temp file behind.
        store.save(&path).unwrap();
        assert_eq!(mode(&path), 0o600);
        assert!(!path.with_extension("tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
