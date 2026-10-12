use crate::hlc::NodeId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IpcRequest {
    GetStatus,
    PairInit,
    PairConnect { token: String },
    Pause { duration_secs: u64 },
    Resume,
    GetHistory { limit: usize },
    SendClip { text: String },
    StopDaemon,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerStatusInfo {
    pub node_id: NodeId,
    pub name: String,
    pub connection_type: String, // "Direct (P2P)" or "Relay (ZK)"
    pub latency_ms: u32,
    pub is_online: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStatusInfo {
    pub is_active: bool,
    pub is_paused: bool,
    pub paused_remaining_secs: u64,
    pub platform: String,
    pub memory_rss_bytes: u64,
    pub node_id: NodeId,
    pub peers: Vec<PeerStatusInfo>,
    pub total_synced_clips: u64,
    pub last_sync_time_secs_ago: Option<u64>,
    pub last_clip_preview: Option<String>,
    pub last_clip_size_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryItem {
    pub id: [u8; 16],
    pub origin_node: NodeId,
    pub origin_name: String,
    pub timestamp_ms: u64,
    pub content_preview: String,
    pub full_content: Vec<u8>,
    pub size_bytes: usize,
    pub is_sensitive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IpcResponse {
    Status(DaemonStatusInfo),
    PairCode {
        qr_ascii: String,
        sas_code: String,
        token: String,
    },
    PairSuccess { peer_node: NodeId, peer_name: String },
    Paused { remaining_secs: u64 },
    Resumed,
    History(Vec<HistoryItem>),
    Sent { bytes: usize },
    DaemonStopping,
    Error(String),
}

pub fn serialize_ipc_msg<T: Serialize>(val: &T) -> Result<Vec<u8>, String> {
    bincode::serialize(val).map_err(|e| e.to_string())
}

pub fn deserialize_ipc_msg<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, String> {
    bincode::deserialize(bytes).map_err(|e| e.to_string())
}
