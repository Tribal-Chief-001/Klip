use crate::hlc::NodeId;
use serde::{Deserialize, Serialize};

/// Version of the control protocol. 1 = before `Hello` existed (v0.3.6 and older); 2 adds `Hello`,
/// `UnpairAll`, `CopyToClipboard` and `ClearHistory`. Variants are only ever appended.
pub const IPC_PROTOCOL_VERSION: u32 = 2;

/// Room for the request's own framing and fields on top of the largest clip.
pub const IPC_REQUEST_OVERHEAD: usize = 64 * 1024;

/// Largest request a daemon with this clip limit accepts.
pub fn max_request_bytes(max_clip_bytes: usize) -> usize {
    max_clip_bytes.saturating_add(IPC_REQUEST_OVERHEAD)
}

pub const CAP_UNPAIR_ALL: &str = "unpair_all";
pub const CAP_COPY_TO_CLIPBOARD: &str = "copy_to_clipboard";
pub const CAP_CLEAR_HISTORY: &str = "clear_history";
/// The daemon holds copied images until they are sent (`GetImageState`, `SendPendingImage`, ...).
pub const CAP_SHARE_IMAGE: &str = "share_image";

/// The settings the running daemon loaded (what a restart would re-read from `config.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigInfo {
    pub device_name: String,
    /// `"n0"` or `"lan"`.
    pub relay: String,
    pub max_history_items: usize,
    pub max_clip_bytes: usize,
    pub lan_discovery: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloInfo {
    pub protocol: u32,
    pub daemon_version: String,
    pub capabilities: Vec<String>,
    pub config: ConfigInfo,
    /// Largest request this daemon accepts (`max_request_bytes(config.max_clip_bytes)`).
    pub max_request_bytes: usize,
}

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
    /// First call of protocol 2: who is the daemon, what can it do, which limits apply.
    Hello {
        client_protocol: u32,
    },
    /// Remove every linked device.
    UnpairAll,
    /// Put a history item back on this device's clipboard (not re-broadcast).
    CopyToClipboard {
        id: [u8; 16],
    },
    /// Forget the clips kept in this device's history.
    ClearHistory,
    // Added in v0.6.0 (protocol stays 2): always appended, never inserted.
    /// Is an image waiting to be sent, and are images sent automatically?
    GetImageState,
    /// Send the image that was copied last (see `GetImageState`).
    SendPendingImage,
    /// Switch automatic sending of copied images on or off (also saved in `config.json`).
    SetAutoImages {
        enabled: bool,
    },
    /// A small PNG of the waiting image (`id: None`) or of a stored history image.
    GetThumbnail {
        id: Option<[u8; 16]>,
    },
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
    /// Live state of the relay connection (connected / connecting / unreachable + reason).
    pub relay_state: String,
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

/// The image that was copied last and is waiting to be sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitingImageInfo {
    pub size_bytes: usize,
    pub width: u32,
    pub height: u32,
    pub age_secs: u64,
    /// `"png"` or `"webp"`.
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageStateInfo {
    /// Images are sent as soon as they are copied.
    pub auto: bool,
    pub waiting: Option<WaitingImageInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferDirection {
    Sending,
    Receiving,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferState {
    /// Queued behind another file for the same device, or still connecting.
    Waiting,
    Active,
    Done,
    Failed,
    Cancelled,
}

/// One file transfer as the control channel reports it (kept in memory only, never on disk).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferInfo {
    pub id: [u8; 16],
    /// The other device's name.
    pub peer: String,
    pub direction: TransferDirection,
    pub name: String,
    pub size: u64,
    pub done: u64,
    pub state: TransferState,
    pub error: Option<String>,
    /// Where a received file was saved.
    pub saved_to: Option<String>,
    pub started_ms: u64,
    pub finished_ms: Option<u64>,
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
    Hello(HelloInfo),
    UnpairedAll {
        removed: Vec<String>,
        failed: Vec<(String, String)>,
    },
    Copied,
    HistoryCleared,
    ImageState(ImageStateInfo),
    /// PNG bytes, at most 240 px on the longest side.
    Thumbnail(Vec<u8>),
}

pub fn serialize_ipc_msg<T: Serialize>(val: &T) -> Result<Vec<u8>, String> {
    postcard::to_stdvec(val).map_err(|e| e.to_string())
}

pub fn deserialize_ipc_msg<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, String> {
    postcard::from_bytes(bytes).map_err(|e| e.to_string())
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

#[cfg(test)]
mod robustness {
    use super::*;
    use rand::{rngs::StdRng, Rng, RngCore, SeedableRng};

    #[test]
    fn random_and_mutated_control_messages_never_crash() {
        let mut rng = StdRng::seed_from_u64(0x1BC);
        for _ in 0..50_000 {
            let mut buf = vec![0u8; rng.gen_range(0..200)];
            rng.fill_bytes(&mut buf);
            let _ = deserialize_ipc_msg::<IpcRequest>(&buf);
            let _ = deserialize_ipc_msg::<IpcResponse>(&buf);
        }
        let good = serialize_ipc_msg(&IpcRequest::SendClip {
            text: "hello".into(),
        })
        .unwrap();
        for pos in 0..good.len() {
            for bit in 0..8 {
                let mut bad = good.clone();
                bad[pos] ^= 1 << bit;
                let _ = deserialize_ipc_msg::<IpcRequest>(&bad);
            }
        }
        // Same message, but the text length claims ~4 GiB with 3 bytes behind it.
        let mut huge = vec![good[0], 0xFF, 0xFF, 0xFF, 0xFF, 0x0F];
        huge.extend_from_slice(b"abc");
        assert!(deserialize_ipc_msg::<IpcRequest>(&huge).is_err());
    }
}

#[cfg(test)]
mod compat {
    //! The control protocol is index-encoded by postcard. These tests pin the bytes of every variant
    //! that existed in v0.3.6, so a reordered or inserted variant fails here instead of breaking a
    //! running daemon or an older `klip`.
    use super::*;
    use crate::hlc::NodeId;

    fn req(r: &IpcRequest) -> Vec<u8> {
        serialize_ipc_msg(r).unwrap()
    }
    fn resp(r: &IpcResponse) -> Vec<u8> {
        serialize_ipc_msg(r).unwrap()
    }

    #[test]
    fn existing_request_variants_keep_their_wire_bytes() {
        assert_eq!(req(&IpcRequest::GetStatus), [0]);
        assert_eq!(
            req(&IpcRequest::PairStart {
                ttl_secs: 1,
                include_lan_addrs: false
            })[0],
            1
        );
        assert_eq!(req(&IpcRequest::PairPoll), [2]);
        assert_eq!(req(&IpcRequest::PairDecide { accept: true }), [3, 1]);
        assert_eq!(req(&IpcRequest::PairCancel), [4]);
        assert_eq!(
            req(&IpcRequest::PairJoin { ticket: "k".into() }),
            [5, 1, b'k']
        );
        assert_eq!(
            req(&IpcRequest::Unpair {
                selector: "ab".into()
            }),
            [6, 2, b'a', b'b']
        );
        assert_eq!(
            req(&IpcRequest::Pause { duration_secs: 600 }),
            [7, 0xD8, 0x04]
        );
        assert_eq!(req(&IpcRequest::Resume), [8]);
        assert_eq!(req(&IpcRequest::GetHistory { limit: 10 }), [9, 10]);
        assert_eq!(
            req(&IpcRequest::SendClip { text: "hi".into() }),
            [10, 2, b'h', b'i']
        );
        assert_eq!(req(&IpcRequest::StopDaemon), [11]);
    }

    #[test]
    fn new_request_variants_are_appended_after_the_old_ones() {
        assert_eq!(req(&IpcRequest::Hello { client_protocol: 2 }), [12, 2]);
        assert_eq!(req(&IpcRequest::UnpairAll), [13]);
        assert_eq!(req(&IpcRequest::CopyToClipboard { id: [0; 16] })[0], 14);
        assert_eq!(req(&IpcRequest::ClearHistory), [15]);
    }

    #[test]
    fn existing_response_variants_keep_their_wire_bytes() {
        let status = DaemonStatusInfo {
            is_active: true,
            is_paused: false,
            paused_remaining_secs: 0,
            platform: String::new(),
            memory_rss_bytes: 0,
            node_id: NodeId::ZERO,
            relay_mode: String::new(),
            relay_state: String::new(),
            pairing_open: false,
            peers: vec![],
            total_synced_clips: 0,
            last_sync_time_secs_ago: None,
            last_clip_preview: None,
            last_clip_size_bytes: 0,
        };
        assert_eq!(resp(&IpcResponse::Status(status))[0], 0);
        assert_eq!(
            resp(&IpcResponse::PairTicket {
                ticket: String::new(),
                expires_in_secs: 0,
                host_fingerprint: String::new()
            })[0],
            1
        );
        assert_eq!(resp(&IpcResponse::PairPoll(PairPollInfo::default()))[0], 2);
        assert_eq!(
            resp(&IpcResponse::PairSuccess {
                peer_name: String::new(),
                peer_fingerprint: String::new()
            })[0],
            3
        );
        assert_eq!(resp(&IpcResponse::PairCancelled), [4]);
        assert_eq!(
            resp(&IpcResponse::Unpaired {
                name: String::new()
            })[0],
            5
        );
        assert_eq!(resp(&IpcResponse::Paused { remaining_secs: 1 }), [6, 1]);
        assert_eq!(resp(&IpcResponse::Resumed), [7]);
        assert_eq!(resp(&IpcResponse::History(vec![])), [8, 0]);
        assert_eq!(
            resp(&IpcResponse::Sent {
                bytes: 0,
                delivered: vec![],
                failed: vec![]
            }),
            [9, 0, 0, 0]
        );
        assert_eq!(resp(&IpcResponse::DaemonStopping), [10]);
        assert_eq!(resp(&IpcResponse::Error("x".into())), [11, 1, b'x']);
    }

    #[test]
    fn new_response_variants_are_appended_after_the_old_ones() {
        let hello = HelloInfo {
            protocol: IPC_PROTOCOL_VERSION,
            daemon_version: "0.4.0".into(),
            capabilities: vec![CAP_UNPAIR_ALL.into()],
            config: ConfigInfo {
                device_name: "d".into(),
                relay: "n0".into(),
                max_history_items: 50,
                max_clip_bytes: 1000,
                lan_discovery: true,
            },
            max_request_bytes: max_request_bytes(1000),
        };
        assert_eq!(resp(&IpcResponse::Hello(hello.clone()))[0], 12);
        assert_eq!(
            resp(&IpcResponse::UnpairedAll {
                removed: vec![],
                failed: vec![]
            }),
            [13, 0, 0]
        );
        assert_eq!(resp(&IpcResponse::Copied), [14]);
        assert_eq!(resp(&IpcResponse::HistoryCleared), [15]);
        let bytes = serialize_ipc_msg(&IpcResponse::Hello(hello.clone())).unwrap();
        assert_eq!(
            deserialize_ipc_msg::<IpcResponse>(&bytes).unwrap(),
            IpcResponse::Hello(hello)
        );
    }

    #[test]
    fn image_variants_are_appended_after_protocol_two() {
        // v0.6.0: requests 16.., responses 16.. - the earlier indices above must not move.
        assert_eq!(req(&IpcRequest::GetImageState), [16]);
        assert_eq!(req(&IpcRequest::SendPendingImage), [17]);
        assert_eq!(req(&IpcRequest::SetAutoImages { enabled: true }), [18, 1]);
        assert_eq!(req(&IpcRequest::GetThumbnail { id: None }), [19, 0]);
        let mut with_id = vec![19, 1];
        with_id.extend_from_slice(&[7u8; 16]);
        assert_eq!(
            req(&IpcRequest::GetThumbnail { id: Some([7; 16]) }),
            with_id
        );

        let empty = ImageStateInfo {
            auto: false,
            waiting: None,
        };
        assert_eq!(resp(&IpcResponse::ImageState(empty.clone())), [16, 0, 0]);
        let waiting = ImageStateInfo {
            auto: true,
            waiting: Some(WaitingImageInfo {
                size_bytes: 1,
                width: 2,
                height: 3,
                age_secs: 4,
                kind: "png".into(),
            }),
        };
        assert_eq!(
            resp(&IpcResponse::ImageState(waiting.clone())),
            [16, 1, 1, 1, 2, 3, 4, 3, b'p', b'n', b'g']
        );
        assert_eq!(resp(&IpcResponse::Thumbnail(vec![9, 8])), [17, 2, 9, 8]);
        for r in [
            IpcResponse::ImageState(waiting),
            IpcResponse::Thumbnail(vec![1]),
        ] {
            let bytes = serialize_ipc_msg(&r).unwrap();
            assert_eq!(deserialize_ipc_msg::<IpcResponse>(&bytes).unwrap(), r);
        }
        assert_eq!(CAP_SHARE_IMAGE, "share_image");
    }

    #[test]
    fn an_old_peer_decodes_a_new_request_as_an_error_not_a_panic() {
        // An index far beyond any variant must be a decode error (the daemon turns it into
        // IpcResponse::Error), never a panic.
        assert!(deserialize_ipc_msg::<IpcRequest>(&[200, 1, 2]).is_err());
    }

    #[test]
    fn request_cap_follows_the_clip_limit() {
        assert_eq!(max_request_bytes(1000), 1000 + 64 * 1024);
        assert_eq!(max_request_bytes(usize::MAX), usize::MAX);
    }
}
