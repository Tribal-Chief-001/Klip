//! Platform-independent Klip engine shared by the desktop daemon and the Android library.
//!
//! It owns the network node, the trusted-device list, pairing state, last-write-wins resolution
//! and clipboard history. The host supplies a [`klip_platform::ClipboardDriver`] (OS clipboard on
//! desktop, a bridge into the app on mobile).

pub mod config;
pub mod engine;
pub mod files;
pub use files::human_size;
pub mod history;
pub mod sync;
mod transfers;

pub use config::{DaemonConfig, KlipPaths, RelayMode};
pub use engine::{CoreEngine, Shutdown};
pub use history::HistoryBuffer;
pub use sync::{
    node_of, offline_label, DeliveryReport, FileReceivedHook, SyncCoordinator, UndeliveredHook,
};
