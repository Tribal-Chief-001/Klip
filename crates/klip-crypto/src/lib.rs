//! Device identity and the on-disk keystore.
//!
//! Transport security (key exchange, encryption, mutual authentication) is provided by QUIC/TLS in
//! `klip-net`; this crate only owns the long-term Ed25519 device key and the list of trusted peers.

pub mod identity;

pub use identity::{restrict_to_owner, DeviceIdentity, KeyStore, PairedPeer};
