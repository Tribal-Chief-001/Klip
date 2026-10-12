use directories::ProjectDirs;
use std::path::PathBuf;

pub use klip_core::config::{DaemonConfig, KlipPaths, RelayMode};

/// The current user's Klip directory. `KLIP_CONFIG_DIR` overrides the OS default, which makes it
/// possible to run several isolated instances on one machine (development, tests).
pub fn user_paths() -> KlipPaths {
    if let Some(over) = std::env::var_os("KLIP_CONFIG_DIR") {
        return KlipPaths::from_dir(PathBuf::from(over));
    }
    if let Some(proj) = ProjectDirs::from("com", "klip", "klip") {
        return KlipPaths::from_dir(proj.config_dir());
    }
    KlipPaths::from_dir(".klip")
}

// Convenience wrappers used by the CLI.
pub fn get_klip_dir() -> PathBuf {
    user_paths().dir().to_path_buf()
}

pub fn get_ipc_socket_path() -> PathBuf {
    user_paths().ipc_socket()
}

pub fn get_keystore_path() -> PathBuf {
    user_paths().keystore()
}
