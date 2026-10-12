#![allow(clippy::manual_is_multiple_of)]

pub mod aead;
pub mod hybrid;
pub mod identity;
pub mod mem;
pub mod pairing;

pub use aead::{
    decrypt_payload_in_place, encrypt_payload_in_place, CryptoError, NonceGenerator, SymmetricKey,
};
pub use hybrid::HybridSharedSecret;
pub use identity::{DeviceIdentity, KeyStore, PairedPeer};
pub use mem::PinnedBuffer;
pub use pairing::{compute_sas, format_sas_pin, PairingInvitation, PairingPsk};
