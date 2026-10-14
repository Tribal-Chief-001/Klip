use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Where an instance keeps its files (keystore, config, control socket).
#[derive(Debug, Clone)]
pub struct KlipPaths {
    dir: PathBuf,
}

impl KlipPaths {
    pub fn from_dir(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let _ = fs::create_dir_all(&dir);
        klip_crypto::identity::restrict_to_owner(&dir, 0o700);
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
    pub fn keystore(&self) -> PathBuf {
        self.dir.join("keystore.json")
    }
    /// Control socket path. Unix socket paths are limited to ~108 bytes, so a very deep config
    /// directory falls back to a short, stable path derived from it.
    pub fn ipc_socket(&self) -> PathBuf {
        let preferred = self.dir.join("daemon.sock");
        if preferred.as_os_str().len() < MAX_SOCKET_PATH_BYTES {
            return preferred;
        }
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        base.join(format!(
            "klip-{:016x}.sock",
            fnv1a(self.dir.as_os_str().as_encoded_bytes())
        ))
    }
    pub fn config_file(&self) -> PathBuf {
        self.dir.join("config.json")
    }
}

/// Conservative bound below the platform `sun_path` limit (104 on macOS/BSD, 108 on Linux).
const MAX_SOCKET_PATH_BYTES: usize = 100;

/// FNV-1a: tiny and, unlike `DefaultHasher`, guaranteed identical across builds, so the CLI and the
/// daemon always agree on the fallback socket path.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// How devices find each other across networks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RelayMode {
    /// n0's public relays and discovery (works across networks; n0 sees metadata, never content).
    N0,
    /// No third-party servers; devices must reach each other directly (same LAN).
    Lan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DaemonConfig {
    pub device_name: String,
    pub relay: RelayMode,
    pub max_history_items: usize,
    /// Clips older than this many hours are forgotten from the history; 0 keeps them until the
    /// item and size limits push them out.
    pub history_ttl_hours: u64,
    pub poll_interval_ms: u64,
    /// Largest clip accepted or sent, in bytes.
    pub max_clip_bytes: usize,
    /// Announce this device on the LAN via mDNS so paired devices find it without internet.
    pub lan_discovery: bool,
    /// Send copied images to the other devices at once. Off (the default): a copied image waits until
    /// it is sent on purpose (`klip share`, the tray). Text is always sent automatically.
    pub auto_send_images: bool,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            device_name: default_device_name(),
            relay: RelayMode::N0,
            max_history_items: 50,
            history_ttl_hours: 24,
            poll_interval_ms: 100,
            max_clip_bytes: 32 * 1024 * 1024,
            lan_discovery: true,
            auto_send_images: false,
        }
    }
}

impl DaemonConfig {
    /// Load `config.json` if present (missing keys fall back to defaults). A malformed file is an
    /// error rather than being silently ignored, so a typo never changes privacy settings.
    pub fn load(paths: &KlipPaths) -> Result<Self, String> {
        let file = paths.config_file();
        if !file.exists() {
            return Ok(Self::default());
        }
        let data = fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let cfg: Self =
            serde_json::from_str(&data).map_err(|e| format!("{}: {e}", file.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Validate, then write `config.json` atomically (mode 0600); the old file stays when anything fails.
    pub fn save(&self, paths: &KlipPaths) -> Result<(), String> {
        self.validate()?;
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())? + "\n";
        klip_crypto::identity::write_private_file(&paths.config_file(), json.as_bytes())
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.poll_interval_ms < 20 {
            return Err("poll_interval_ms must be at least 20".into());
        }
        if self.max_clip_bytes == 0 || self.max_clip_bytes > 256 * 1024 * 1024 {
            return Err("max_clip_bytes must be between 1 and 268435456".into());
        }
        if self.history_ttl_hours > 24 * 365 {
            return Err("history_ttl_hours must be at most 8760 (one year); 0 keeps clips until the limits push them out".into());
        }
        if self.max_history_items == 0 {
            return Err("max_history_items must be at least 1".into());
        }
        Ok(())
    }
}

fn default_device_name() -> String {
    let name = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_default();
    if name.trim().is_empty() {
        "Klip device".to_string()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> KlipPaths {
        let d = std::env::temp_dir().join(format!("klip-cfg-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        KlipPaths::from_dir(d)
    }

    #[test]
    fn deep_config_dirs_get_a_short_stable_socket_path() {
        let short = KlipPaths::from_dir(
            std::env::temp_dir().join(format!("klip-sock-{}", std::process::id())),
        );
        assert_eq!(short.ipc_socket(), short.dir().join("daemon.sock"));

        let deep_dir = std::env::temp_dir()
            .join("x".repeat(60))
            .join("y".repeat(60));
        let deep = KlipPaths::from_dir(&deep_dir);
        let sock = deep.ipc_socket();
        assert!(sock.as_os_str().len() < MAX_SOCKET_PATH_BYTES, "{sock:?}");
        assert_eq!(
            sock,
            KlipPaths::from_dir(&deep_dir).ipc_socket(),
            "must be stable"
        );
        let other = KlipPaths::from_dir(
            std::env::temp_dir()
                .join("z".repeat(70))
                .join("w".repeat(40)),
        );
        assert_ne!(sock, other.ipc_socket());
        let _ = fs::remove_dir_all(short.dir());
        let _ = fs::remove_dir_all(std::env::temp_dir().join("x".repeat(60)));
        let _ = fs::remove_dir_all(std::env::temp_dir().join("z".repeat(70)));
    }

    #[test]
    fn images_are_manual_by_default_and_the_choice_round_trips_through_save() {
        let p = tmp("images");
        assert!(!DaemonConfig::load(&p).unwrap().auto_send_images);
        fs::write(
            p.config_file(),
            r#"{"auto_send_images": true, "device_name": "Desk"}"#,
        )
        .unwrap();
        let mut cfg = DaemonConfig::load(&p).unwrap();
        assert!(cfg.auto_send_images);
        cfg.auto_send_images = false;
        cfg.max_history_items = 7;
        cfg.save(&p).unwrap();
        let back = DaemonConfig::load(&p).unwrap();
        assert!(!back.auto_send_images);
        assert_eq!(
            (back.device_name.as_str(), back.max_history_items),
            ("Desk", 7)
        );
        let _ = fs::remove_dir_all(p.dir());
    }

    #[test]
    fn save_refuses_an_invalid_config_and_keeps_the_old_file() {
        let p = tmp("save-invalid");
        DaemonConfig::default().save(&p).unwrap();
        let before = fs::read_to_string(p.config_file()).unwrap();
        let bad = DaemonConfig {
            max_history_items: 0,
            ..DaemonConfig::default()
        };
        assert!(bad.save(&p).is_err());
        assert_eq!(fs::read_to_string(p.config_file()).unwrap(), before);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(p.config_file()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = fs::remove_dir_all(p.dir());
    }

    #[test]
    fn defaults_when_missing_and_partial_override() {
        let p = tmp("partial");
        let cfg = DaemonConfig::load(&p).unwrap();
        assert_eq!(cfg.relay, RelayMode::N0);
        fs::write(p.config_file(), r#"{"relay":"lan","device_name":"Desk"}"#).unwrap();
        let cfg = DaemonConfig::load(&p).unwrap();
        assert_eq!(cfg.relay, RelayMode::Lan);
        assert_eq!(cfg.device_name, "Desk");
        assert_eq!(cfg.poll_interval_ms, 100, "unset keys keep defaults");
        let _ = fs::remove_dir_all(p.dir());
    }

    #[test]
    fn malformed_or_invalid_config_is_an_error() {
        let p = tmp("bad");
        fs::write(p.config_file(), "{ not json").unwrap();
        assert!(DaemonConfig::load(&p).is_err());
        fs::write(p.config_file(), r#"{"relay":"wan"}"#).unwrap();
        assert!(
            DaemonConfig::load(&p).is_err(),
            "unknown relay mode must not silently default"
        );
        fs::write(p.config_file(), r#"{"poll_interval_ms":1}"#).unwrap();
        assert!(DaemonConfig::load(&p).is_err());
        // A misspelled key must not be silently ignored (it could be a privacy setting).
        fs::write(p.config_file(), r#"{"relays":"lan"}"#).unwrap();
        let err = DaemonConfig::load(&p).unwrap_err();
        assert!(err.contains("relays"), "{err}");
        let _ = fs::remove_dir_all(p.dir());
    }
}
