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
        #[cfg(windows)]
        return PathBuf::from(self.ipc_pipe_name());
        #[cfg(not(windows))]
        self.ipc_socket_path()
    }

    #[cfg(not(windows))]
    fn ipc_socket_path(&self) -> PathBuf {
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

    /// The Windows control channel: a named pipe whose name comes from the Klip directory, so two
    /// configurations (or two users) never share one, and the CLI and the daemon always agree.
    /// Windows paths are case-insensitive, so the directory is lower-cased there.
    pub fn ipc_pipe_name(&self) -> String {
        let dir = self.dir.to_string_lossy();
        #[cfg(windows)]
        let dir = dir.to_lowercase();
        format!(r"\\.\pipe\klip-{:016x}", fnv1a(dir.as_bytes()))
    }
}

/// Conservative bound below the platform `sun_path` limit (104 on macOS/BSD, 108 on Linux).
#[cfg(not(windows))]
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
    /// Take files that paired devices send (into `receive_dir`). Off: every file is refused.
    pub accept_files: bool,
    /// Where received files go (an absolute path). `None`: `<Downloads>/Klip`.
    pub receive_dir: Option<String>,
    /// Largest file accepted, in bytes.
    pub max_file_bytes: u64,
    /// Free space needed per byte of a received file (1; 2 on Android, which copies the file again
    /// into shared storage). Not meant to be edited by hand.
    pub file_space_factor: u64,
    /// Speak file transfer version 2 (resume, folders). On unless you are chasing a problem: off makes this device
    /// behave like one that only has version 1 (whole files, no folders).
    #[serde(skip_serializing_if = "is_true")]
    pub file_protocol_v2: bool,
}

fn is_true(v: &bool) -> bool {
    *v
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
            accept_files: true,
            receive_dir: None,
            max_file_bytes: 4 * 1024 * 1024 * 1024,
            file_space_factor: 1,
            file_protocol_v2: true,
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
        if self.max_file_bytes == 0 || self.max_file_bytes > 64 * 1024 * 1024 * 1024 {
            return Err("max_file_bytes must be between 1 and 68719476736 (64 GiB)".into());
        }
        if let Some(dir) = &self.receive_dir {
            if dir.is_empty() || !std::path::Path::new(dir).is_absolute() {
                return Err(
                    "receive_dir must be an absolute path (for example /home/me/Downloads/Klip)"
                        .into(),
                );
            }
        }
        if self.file_space_factor == 0 || self.file_space_factor > 8 {
            return Err("file_space_factor must be between 1 and 8".into());
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
    #[test]
    fn the_switch_is_not_written_while_it_is_on_so_an_older_klip_still_reads_the_file() {
        use super::*;
        let json = serde_json::to_string(&DaemonConfig::default()).unwrap();
        assert!(!json.contains("file_protocol_v2"), "{json}");
        let off = DaemonConfig {
            file_protocol_v2: false,
            ..DaemonConfig::default()
        };
        assert!(serde_json::to_string(&off)
            .unwrap()
            .contains("\"file_protocol_v2\":false"));
    }

    #[test]
    fn the_file_protocol_switch_defaults_on_is_read_from_json_and_unknown_keys_still_fail() {
        use super::*;
        assert!(DaemonConfig::default().file_protocol_v2);
        let off: DaemonConfig = serde_json::from_str(r#"{"file_protocol_v2": false}"#).unwrap();
        assert!(!off.file_protocol_v2);
        assert!(serde_json::from_str::<DaemonConfig>(r#"{"file_protocol_v3": true}"#).is_err());
    }

    #[test]
    fn the_file_keys_have_safe_defaults_and_are_validated() {
        use super::*;
        let d = DaemonConfig::default();
        assert!(d.accept_files);
        assert_eq!(d.receive_dir, None);
        assert_eq!(d.max_file_bytes, 4 * 1024 * 1024 * 1024);
        assert_eq!(d.file_space_factor, 1);
        assert!(d.validate().is_ok());

        for max in [0, 65 * 1024 * 1024 * 1024] {
            let bad = DaemonConfig {
                max_file_bytes: max,
                ..DaemonConfig::default()
            };
            assert!(
                bad.validate().unwrap_err().contains("max_file_bytes"),
                "{max}"
            );
        }
        for dir in ["", "relative/folder", "~/Downloads"] {
            let c = DaemonConfig {
                receive_dir: Some(dir.into()),
                ..DaemonConfig::default()
            };
            assert!(c.validate().unwrap_err().contains("receive_dir"), "{dir:?}");
        }
        let ok = DaemonConfig {
            receive_dir: Some(std::env::temp_dir().display().to_string()),
            ..DaemonConfig::default()
        };
        assert!(ok.validate().is_ok());
        let zero = DaemonConfig {
            file_space_factor: 0,
            ..ok
        };
        assert!(zero.validate().unwrap_err().contains("file_space_factor"));
    }

    #[test]
    fn an_old_config_file_without_the_file_keys_still_loads_with_defaults() {
        use super::*;
        let dir = std::env::temp_dir().join(format!("klip-cfg-files-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let paths = KlipPaths::from_dir(&dir);
        fs::write(
            paths.config_file(),
            r#"{"device_name":"Old","max_history_items":10}"#,
        )
        .unwrap();
        let cfg = DaemonConfig::load(&paths).unwrap();
        assert_eq!(cfg.device_name, "Old");
        assert!(cfg.accept_files && cfg.receive_dir.is_none());
        let mut changed = cfg.clone();
        changed.accept_files = false;
        changed.receive_dir = Some(std::env::temp_dir().display().to_string());
        changed.save(&paths).unwrap();
        let back = DaemonConfig::load(&paths).unwrap();
        assert!(!back.accept_files);
        assert_eq!(back.receive_dir, changed.receive_dir);
    }

    #[test]
    fn the_windows_pipe_name_is_stable_per_directory_and_differs_between_directories() {
        use super::*;
        let dir =
            |n: &str| std::env::temp_dir().join(format!("klip-pipe-{}-{n}", std::process::id()));
        let (a, a2, b) = (
            KlipPaths::from_dir(dir("a")),
            KlipPaths::from_dir(dir("a")),
            KlipPaths::from_dir(dir("b")),
        );
        assert_eq!(a.ipc_pipe_name(), a2.ipc_pipe_name());
        assert_ne!(a.ipc_pipe_name(), b.ipc_pipe_name());
        let name = a.ipc_pipe_name();
        let hex = name.strip_prefix(r"\\.\pipe\klip-").expect(&name);
        assert_eq!(hex.len(), 16, "{name}");
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()), "{name}");
    }
    use super::*;

    fn tmp(tag: &str) -> KlipPaths {
        let d = std::env::temp_dir().join(format!("klip-cfg-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        KlipPaths::from_dir(d)
    }

    #[cfg(not(windows))]
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
