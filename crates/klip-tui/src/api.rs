//! The boundary between the TUI and the daemon. `klip-cli` implements [`DaemonApi`] over the control
//! socket; tests use `testkit::FakeDaemon`.
use async_trait::async_trait;
pub use klip_proto::ipc::{
    ConfigInfo, DaemonStatusInfo, HelloInfo, HistoryItem, PairPollInfo, PeerStatusInfo,
    PendingPairInfo,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// No daemon answers on the control socket.
    DaemonDown,
    /// The daemon did not answer within this many seconds.
    Timeout(u64),
    /// The daemon is older than the call (it answered "unknown request").
    Unsupported,
    /// The request is bigger than the daemon accepts.
    TooLarge {
        size: usize,
        max: usize,
    },
    /// The daemon understood and said no, with its own reason.
    Refused(String),
    Io(String),
}

impl ApiError {
    /// One short sentence for a toast or a notice.
    pub fn plain(&self) -> String {
        match self {
            Self::DaemonDown => "Klip is not running".to_string(),
            Self::Timeout(s) => format!("Klip did not answer within {s} s"),
            Self::Unsupported => {
                "the running daemon is older: restart it after upgrading".to_string()
            }
            Self::TooLarge { size, max } => format!(
                "{} is too large; one request can carry at most {}",
                crate::fmt::bytes(*size),
                crate::fmt::bytes(*max)
            ),
            Self::Refused(m) | Self::Io(m) => m.clone(),
        }
    }
}

/// `(removed names, (name, reason) failures)` of an unpair-all.
pub type UnpairReport = (Vec<String>, Vec<(String, String)>);

/// Result of `klip send`: which devices got the text and which did not (with the reason).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendReport {
    pub bytes: usize,
    pub delivered: Vec<String>,
    pub failed: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairTicket {
    pub ticket: String,
    pub expires_in_secs: u64,
    pub host_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paired {
    pub name: String,
    pub fingerprint: String,
}

#[async_trait]
pub trait DaemonApi: Send + Sync {
    /// Protocol-2 handshake. `Err(Unsupported)` on a daemon that predates it.
    async fn hello(&self) -> Result<HelloInfo, ApiError>;
    async fn status(&self) -> Result<DaemonStatusInfo, ApiError>;
    async fn history(&self, limit: usize) -> Result<Vec<HistoryItem>, ApiError>;
    async fn send(&self, text: String) -> Result<SendReport, ApiError>;
    /// Returns the seconds the pause will last.
    async fn pause(&self, secs: u64) -> Result<u64, ApiError>;
    async fn resume(&self) -> Result<(), ApiError>;
    /// `selector` is a name or the start of a fingerprint. Returns the removed device's name.
    async fn unpair(&self, selector: String) -> Result<String, ApiError>;
    async fn unpair_all(&self) -> Result<UnpairReport, ApiError>;
    async fn copy_to_clipboard(&self, id: [u8; 16]) -> Result<(), ApiError>;
    async fn clear_history(&self) -> Result<(), ApiError>;
    async fn pair_start(&self, ttl_secs: u64, lan: bool) -> Result<PairTicket, ApiError>;
    async fn pair_poll(&self) -> Result<PairPollInfo, ApiError>;
    async fn pair_decide(&self, accept: bool) -> Result<PairPollInfo, ApiError>;
    async fn pair_cancel(&self) -> Result<(), ApiError>;
    async fn pair_join(&self, ticket: String) -> Result<Paired, ApiError>;
    async fn stop_daemon(&self) -> Result<(), ApiError>;
    /// Start the daemon in the background; returns its process id.
    async fn start_daemon(&self) -> Result<u32, ApiError>;
    /// Does what `klip install-service` does; returns the message to show.
    async fn install_service(&self) -> Result<String, ApiError>;
}
