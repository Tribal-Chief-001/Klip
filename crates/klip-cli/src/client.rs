use klip_daemon::config::get_ipc_socket_path;
use klip_proto::ipc::{deserialize_ipc_msg, serialize_ipc_msg, IpcRequest, IpcResponse};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(unix)]
use tokio::net::UnixStream;

/// Most requests are answered in milliseconds; a hung daemon must not hang the CLI forever.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
/// Joining a device waits for the other side's human to approve it.
const PAIR_JOIN_TIMEOUT: Duration = Duration::from_secs(120);
/// What daemons that predate the `Hello` call accept in one request.
pub const OLD_DAEMON_REQUEST_CAP: usize = 8 * 1024 * 1024;
/// Answers bigger than this are refused as implausible (history holds at most ~8 MiB of content).
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    /// No socket, or nobody answers on it. The text is the message the CLI has always shown.
    Down(String),
    Timeout(u64),
    TooLarge {
        size: usize,
        max: usize,
    },
    Io(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Down(m) | Self::Io(m) => write!(f, "{m}"),
            Self::Timeout(s) => write!(
                f,
                "Klip did not answer within {s} s; it may be stuck. Try `klip stop`, or stop the process."
            ),
            Self::TooLarge { size, max } => write!(
                f,
                "the text is {size} bytes but one request can carry at most {max} bytes"
            ),
        }
    }
}

impl std::error::Error for ClientError {}

pub struct DaemonClient {
    sock: PathBuf,
}

/// A big clip is delivered to every device before the daemon answers `klip send`: allow time for it.
fn timeout_for(req: &IpcRequest) -> Duration {
    match req {
        IpcRequest::PairJoin { .. } => PAIR_JOIN_TIMEOUT,
        IpcRequest::SendClip { text } => {
            DEFAULT_TIMEOUT + Duration::from_secs((text.len() / (256 * 1024)) as u64)
        }
        _ => DEFAULT_TIMEOUT,
    }
}

impl DaemonClient {
    pub fn new(sock: impl Into<PathBuf>) -> Self {
        Self { sock: sock.into() }
    }

    pub fn for_current_user() -> Self {
        Self::new(get_ipc_socket_path())
    }

    /// Existing callers: the current user's daemon, the 8 MiB request cap of older daemons.
    pub async fn send_request(req: &IpcRequest) -> Result<IpcResponse, String> {
        Self::for_current_user()
            .request(req, OLD_DAEMON_REQUEST_CAP)
            .await
            .map_err(|e| e.to_string())
    }

    /// Send one request. `max_request_bytes` is what the daemon accepts (see `ops::request_cap`);
    /// a bigger request is refused here, before a single byte is written.
    pub async fn request(
        &self,
        req: &IpcRequest,
        max_request_bytes: usize,
    ) -> Result<IpcResponse, ClientError> {
        let limit = timeout_for(req);
        tokio::time::timeout(limit, self.exchange(req, max_request_bytes))
            .await
            .map_err(|_| ClientError::Timeout(limit.as_secs()))?
    }

    #[cfg(unix)]
    async fn exchange(
        &self,
        req: &IpcRequest,
        max_request_bytes: usize,
    ) -> Result<IpcResponse, ClientError> {
        if !self.sock.exists() {
            return Err(ClientError::Down(
                "Klip daemon is not running. Launch it with `klip start`.".to_string(),
            ));
        }
        let mut stream = UnixStream::connect(&self.sock).await.map_err(|_| {
            ClientError::Down(
                "Klip daemon is not running or not responding. Launch it with `klip start`."
                    .to_string(),
            )
        })?;

        let serialized = serialize_ipc_msg(req).map_err(ClientError::Io)?;
        if serialized.len() > max_request_bytes {
            return Err(ClientError::TooLarge {
                size: serialized.len(),
                max: max_request_bytes,
            });
        }
        let written = async {
            stream
                .write_all(&(serialized.len() as u32).to_be_bytes())
                .await?;
            stream.write_all(&serialized).await?;
            stream.flush().await
        }
        .await;
        if let Err(e) = written {
            // The daemon may have answered (for example "request too large") before it closed the
            // connection: that answer is more useful than "Broken pipe".
            return match Self::read_response(&mut stream).await {
                Ok(resp) => Ok(resp),
                Err(_) => Err(ClientError::Io(e.to_string())),
            };
        }
        Self::read_response(&mut stream).await
    }

    #[cfg(unix)]
    async fn read_response(stream: &mut UnixStream) -> Result<IpcResponse, ClientError> {
        let mut resp_len_buf = [0u8; 4];
        stream
            .read_exact(&mut resp_len_buf)
            .await
            .map_err(|e| ClientError::Io(format!("Failed to read response length: {e}")))?;
        let resp_len = u32::from_be_bytes(resp_len_buf) as usize;
        if resp_len > MAX_RESPONSE_BYTES {
            return Err(ClientError::Io(
                "daemon sent an implausible response size".to_string(),
            ));
        }
        let mut resp_buf = vec![0u8; resp_len];
        stream
            .read_exact(&mut resp_buf)
            .await
            .map_err(|e| ClientError::Io(format!("Failed to read response data: {e}")))?;
        deserialize_ipc_msg(&resp_buf).map_err(ClientError::Io)
    }

    #[cfg(not(unix))]
    async fn exchange(
        &self,
        _req: &IpcRequest,
        _max_request_bytes: usize,
    ) -> Result<IpcResponse, ClientError> {
        Err(ClientError::Io(
            "Unix socket not supported on this platform".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn big_sends_get_more_time_and_other_requests_keep_the_old_limits() {
        assert_eq!(timeout_for(&IpcRequest::GetStatus), DEFAULT_TIMEOUT);
        assert_eq!(
            timeout_for(&IpcRequest::PairJoin {
                ticket: String::new()
            }),
            PAIR_JOIN_TIMEOUT
        );
        let big = IpcRequest::SendClip {
            text: "x".repeat(30 * 1024 * 1024),
        };
        assert_eq!(
            timeout_for(&big),
            DEFAULT_TIMEOUT + Duration::from_secs(120)
        );
    }

    #[test]
    fn messages_for_the_daemon_being_down_are_unchanged() {
        assert_eq!(
            ClientError::Timeout(15).to_string(),
            "Klip did not answer within 15 s; it may be stuck. Try `klip stop`, or stop the process."
        );
    }
}
