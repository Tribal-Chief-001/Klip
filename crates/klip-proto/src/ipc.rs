use crate::hlc::NodeId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IpcRequest {
    GetStatus,
    /// Open a pairing window on this device and return a ticket to show as text/QR.
    PairStart {
        ttl_secs: u64,
        /// Also put this device's LAN addresses into the ticket (bigger QR, works without internet).
        include_lan_addrs: bool,
    },
    /// Poll the pairing window (is a device asking to pair? did someone pair?).
    PairPoll,
    /// Answer the pending pairing request.
    PairDecide {
        accept: bool,
    },
    PairCancel,
    /// Redeem a ticket produced by another device.
    PairJoin {
        ticket: String,
    },
    /// Remove a paired device by (a prefix of) its fingerprint or its exact name.
    Unpair {
        selector: String,
    },
    Pause {
        duration_secs: u64,
    },
    Resume,
    GetHistory {
        limit: usize,
    },
    SendClip {
        text: String,
    },
    StopDaemon,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerStatusInfo {
    pub node_id: NodeId,
    pub name: String,
    pub is_online: bool,
    /// Most recent delivery problem, if any (cleared by the next success).
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStatusInfo {
    pub is_active: bool,
    pub is_paused: bool,
    pub paused_remaining_secs: u64,
    pub platform: String,
    pub memory_rss_bytes: u64,
    pub node_id: NodeId,
    pub relay_mode: String,
    pub pairing_open: bool,
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
pub struct PendingPairInfo {
    pub device_name: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PairPollInfo {
    pub window_open: bool,
    pub seconds_left: u64,
    pub pending: Option<PendingPairInfo>,
    /// Name of a device that finished pairing since the window was opened.
    pub paired: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IpcResponse {
    Status(DaemonStatusInfo),
    PairTicket {
        ticket: String,
        expires_in_secs: u64,
        host_fingerprint: String,
    },
    PairPoll(PairPollInfo),
    PairSuccess {
        peer_name: String,
        peer_fingerprint: String,
    },
    PairCancelled,
    Unpaired {
        name: String,
    },
    Paused {
        remaining_secs: u64,
    },
    Resumed,
    History(Vec<HistoryItem>),
    Sent {
        bytes: usize,
        delivered: Vec<String>,
        failed: Vec<(String, String)>,
    },
    DaemonStopping,
    Error(String),
}

pub fn serialize_ipc_msg<T: Serialize>(val: &T) -> Result<Vec<u8>, String> {
    bincode::serialize(val).map_err(|e| e.to_string())
}

pub fn deserialize_ipc_msg<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, String> {
    bincode::deserialize(bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_roundtrip() {
        let req = IpcRequest::PairJoin {
            ticket: "klip1abc".into(),
        };
        let bytes = serialize_ipc_msg(&req).unwrap();
        assert_eq!(deserialize_ipc_msg::<IpcRequest>(&bytes).unwrap(), req);
        let resp = IpcResponse::PairPoll(PairPollInfo {
            window_open: true,
            seconds_left: 30,
            pending: Some(PendingPairInfo {
                device_name: "Pixel".into(),
                fingerprint: "AAAA-BBBB-CCCC".into(),
            }),
            paired: None,
        });
        let bytes = serialize_ipc_msg(&resp).unwrap();
        assert_eq!(deserialize_ipc_msg::<IpcResponse>(&bytes).unwrap(), resp);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(deserialize_ipc_msg::<IpcRequest>(&[0xff; 7]).is_err());
        assert!(deserialize_ipc_msg::<IpcRequest>(&[]).is_err());
    }
}
