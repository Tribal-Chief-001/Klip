pub mod hlc;
pub mod ipc;
pub mod wire;

pub use hlc::{HybridLogicalClock, NodeId};
pub use ipc::{
    deserialize_ipc_msg, serialize_ipc_msg, DaemonStatusInfo, HistoryItem, IpcRequest, IpcResponse,
    PeerStatusInfo,
};
pub use wire::{
    compute_fingerprint, ClipContentType, ClipPayload, MessageType, ProtoError, WireHeader,
    WireNonce, WirePacket, CURRENT_VERSION, FLAG_COMPRESSED, FLAG_DELTA, FLAG_ENCRYPTED,
    FLAG_SENSITIVE, FLAG_TYPE_HTML, FLAG_TYPE_IMAGE_PNG, FLAG_TYPE_IMAGE_WEBP, FLAG_TYPE_MASK,
    FLAG_TYPE_TEXT, HEADER_LEN, MAGIC_BYTES, TAG_LEN,
};
