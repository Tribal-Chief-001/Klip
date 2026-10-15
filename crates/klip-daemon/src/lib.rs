pub mod config;
pub mod engine;
pub mod instance_lock;
#[cfg(windows)]
pub mod ipc_pipe;
pub mod ipc_server;
#[cfg(target_os = "linux")]
pub mod notify;
pub mod services;

pub use config::{
    get_ipc_socket_path, get_keystore_path, get_klip_dir, user_paths, DaemonConfig, KlipPaths,
    RelayMode,
};
pub use engine::DaemonEngine;
pub use instance_lock::InstanceLock;
pub use klip_core::{HistoryBuffer, Shutdown, SyncCoordinator};
pub use services::{
    generate_launchd_plist_file, generate_systemd_unit_file, generate_windows_service_script,
};
