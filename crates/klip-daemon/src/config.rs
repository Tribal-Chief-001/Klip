use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    pub device_name: String,
    pub listen_port: u16,
    pub auto_clear_secrets: bool, // strictly false by default per user non-negotiable
    pub max_history_items: usize,
    pub poll_interval_ms: u64,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        let hostname = gethostname::gethostname().to_string_lossy().to_string();

        Self {
            device_name: if hostname.is_empty() {
                "KlipNode".to_string()
            } else {
                hostname
            },
            listen_port: 7123,
            auto_clear_secrets: false, // strictly false! Do not wipe CopyQ or system history
            max_history_items: 50,
            poll_interval_ms: 100,
        }
    }
}

// Minimal hostname fallback
mod gethostname {
    use std::ffi::OsString;
    pub fn gethostname() -> OsString {
        std::env::var_os("HOSTNAME")
            .or_else(|| std::env::var_os("HOST"))
            .unwrap_or_else(|| OsString::from("KlipDevice"))
    }
}

pub fn get_klip_dir() -> PathBuf {
    if let Some(proj_dirs) = ProjectDirs::from("com", "klip", "klip") {
        let config_dir = proj_dirs.config_dir().to_path_buf();
        let _ = fs::create_dir_all(&config_dir);
        config_dir
    } else {
        let fallback = PathBuf::from(".klip");
        let _ = fs::create_dir_all(&fallback);
        fallback
    }
}

pub fn get_ipc_socket_path() -> PathBuf {
    get_klip_dir().join("daemon.sock")
}

pub fn get_keystore_path() -> PathBuf {
    get_klip_dir().join("keystore.json")
}
