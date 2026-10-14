use ed25519_dalek::SigningKey;
use klip_proto::hlc::NodeId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;

/// Keystore format version. v1 stored per-peer symmetric keys for the retired UDP protocol; those
/// entries are meaningless (and untrustworthy) now and are dropped on load.
const KEYSTORE_VERSION: u32 = 2;

/// Long-term device identity. The 32-byte seed is also the QUIC endpoint secret key, so the
/// `node_id` equals the iroh endpoint id.
pub struct DeviceIdentity {
    seed: [u8; 32],
    pub node_id: NodeId,
}

impl DeviceIdentity {
    pub fn generate() -> Self {
        let mut seed = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut seed);
        Self::from_seed(&seed)
    }

    pub fn from_seed(seed: &[u8; 32]) -> Self {
        let signing_key = SigningKey::from_bytes(seed);
        let node_id = NodeId::new(signing_key.verifying_key().to_bytes());
        Self {
            seed: *seed,
            node_id,
        }
    }

    /// Secret seed (the endpoint secret key). Handle with care; never log it.
    pub fn seed(&self) -> &[u8; 32] {
        &self.seed
    }
}

/// A trusted (paired) device.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairedPeer {
    pub node_id: NodeId,
    pub name: String,
    #[serde(default)]
    pub paired_at_secs: u64,
}

impl PairedPeer {
    pub fn new(node_id: NodeId, name: String) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            node_id,
            name,
            paired_at_secs: now,
        }
    }
}

/// Keystore holding the device identity seed and the trusted peers.
#[derive(Default, Serialize, Deserialize)]
pub struct KeyStore {
    #[serde(default)]
    pub version: u32,
    pub device_name: String,
    pub identity_seed_hex: String,
    #[serde(default)]
    pub peers: HashMap<String, PairedPeer>,
}

impl KeyStore {
    pub fn new(device_name: String) -> (Self, DeviceIdentity) {
        let identity = DeviceIdentity::generate();
        let store = Self {
            version: KEYSTORE_VERSION,
            device_name,
            identity_seed_hex: hex_encode(identity.seed()),
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
            let mut store: KeyStore = serde_json::from_str(&data).map_err(|e| e.to_string())?;
            let seed = hex_decode(&store.identity_seed_hex)?;
            let arr: [u8; 32] = seed
                .try_into()
                .map_err(|_| "Corrupted identity seed".to_string())?;
            let identity = DeviceIdentity::from_seed(&arr);
            if store.version < KEYSTORE_VERSION {
                // Legacy peers were bound to a symmetric key of the retired protocol.
                store.peers.clear();
                store.version = KEYSTORE_VERSION;
                store.save(p)?;
            }
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

    pub fn remove_peer(&mut self, node_id: &NodeId) -> bool {
        self.peers.remove(&node_id.to_hex()).is_some()
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

/// Atomically write a secret file that is created with mode 0600 (never briefly world-readable):
/// temp file, fsync, rename. The old file stays untouched when any step fails.
pub fn write_private_file(path: &Path, data: &[u8]) -> Result<(), String> {
    // A name of its own per call: two saves at once (two clients, two threads) must not share a file.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("tmp{}-{n}", std::process::id()));
    let written = (|| {
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
        fs::rename(&tmp, path).map_err(|e| e.to_string())
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn hex_decode(hex: &str) -> Result<Vec<u8>, String> {
    if !hex.len().is_multiple_of(2) {
        return Err("Odd length hex string".to_string());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            hex.get(i..i + 2)
                .ok_or_else(|| "Invalid hex".to_string())
                .and_then(|h| u8::from_str_radix(h, 16).map_err(|e| e.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("klip-ks-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn identity_is_deterministic_from_seed() {
        let id = DeviceIdentity::generate();
        let again = DeviceIdentity::from_seed(id.seed());
        assert_eq!(id.node_id, again.node_id);
        assert_ne!(id.node_id, DeviceIdentity::generate().node_id);
    }

    #[test]
    fn keystore_peer_persistence() {
        let dir = temp_dir("persist");
        let path = dir.join("keystore.json");
        let (mut store, id) = KeyStore::load_or_create(&path, "Laptop").unwrap();
        let peer = NodeId::new([77u8; 32]);
        store.add_peer(PairedPeer::new(peer, "Pixel".into()));
        store.save(&path).unwrap();

        let (loaded, id2) = KeyStore::load_or_create(&path, "ignored").unwrap();
        assert_eq!(id.node_id, id2.node_id);
        assert_eq!(loaded.device_name, "Laptop");
        assert_eq!(loaded.get_peer(&peer).unwrap().name, "Pixel");

        let mut loaded = loaded;
        assert!(loaded.remove_peer(&peer));
        assert!(!loaded.remove_peer(&peer));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_keystore_is_migrated_and_old_peers_dropped() {
        let dir = temp_dir("legacy");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keystore.json");
        let seed_hex = "11".repeat(32);
        let legacy = format!(
            r#"{{"device_name":"Old","identity_seed_hex":"{seed_hex}","peers":{{"aa":{{"node_id":{nid:?},"name":"OldPhone","key_hex":"{key}","paired_at_secs":1}}}}}}"#,
            nid = vec![9u8; 32],
            key = "22".repeat(32),
        );
        fs::write(&path, legacy).unwrap();

        let (store, id) = KeyStore::load_or_create(&path, "x").unwrap();
        assert_eq!(store.version, KEYSTORE_VERSION);
        assert!(
            store.peers.is_empty(),
            "legacy PSK peers must not stay trusted"
        );
        assert_eq!(
            id.node_id,
            DeviceIdentity::from_seed(&[0x11u8; 32]).node_id,
            "identity survives"
        );
        // The migrated file is persisted and private.
        let again = fs::read_to_string(&path).unwrap();
        assert!(again.contains("\"version\": 2"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupted_seed_is_an_error() {
        let dir = temp_dir("corrupt");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keystore.json");
        fs::write(
            &path,
            r#"{"version":2,"device_name":"x","identity_seed_hex":"zz","peers":{}}"#,
        )
        .unwrap();
        assert!(KeyStore::load_or_create(&path, "x").is_err());
        fs::write(
            &path,
            r#"{"version":2,"device_name":"x","identity_seed_hex":"abcd","peers":{}}"#,
        )
        .unwrap();
        assert!(KeyStore::load_or_create(&path, "x").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn test_keystore_is_private_and_tightens_existing_files() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("perm");
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
