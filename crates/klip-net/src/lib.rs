//! Klip networking on top of iroh (QUIC, mutual authentication by Ed25519 device key).
//!
//! Security model
//! - A device is identified by its Ed25519 public key (`EndpointId`). QUIC/TLS 1.3 with raw
//!   public keys authenticates both sides, so no application-layer re-encryption is needed.
//! - Only *paired* peers may open a sync connection. Anyone else is closed immediately.
//! - Pairing uses a single-use, short-lived, 128-bit secret carried out-of-band (QR/ticket) and
//!   must be approved on the host before the peer is trusted.
//! - Sync is online-only by design: there is no mailbox or store-and-forward.

mod error;
mod node;
mod proto;
mod ticket;

pub use error::NetError;
pub use node::{KlipNode, NetConfig, NetEvent, PairDecision, PeerRecord, PeerStatus, RelayChoice};
pub use proto::{MAX_CONTROL_BYTES, PAIR_ALPN, SYNC_ALPN};
pub use ticket::{PairingTicket, TICKET_PREFIX};

pub use iroh::{EndpointAddr, EndpointId};
