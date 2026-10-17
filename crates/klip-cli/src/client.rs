use klip_daemon::config::get_ipc_socket_path;
use klip_proto::ipc::{deserialize_ipc_msg, serialize_ipc_msg, IpcRequest, IpcResponse};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
#[cfg(unix)]
use tokio::net::UnixStream;

/// Most requests are answered in milliseconds; a hung daemon must not hang the CLI forever.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
/// Joining a device waits for the other side's human to approve it.
const PAIR_JOIN_TIMEOUT: Duration = Duration::from_secs(120);
/// The daemon takes the held image out of its slot and sends it before it answers: a big image over a
/// slow link needs time, and giving up early would report a failure for something still going through.
const SHARE_IMAGE_TIMEOUT: Duration = Duration::from_secs(120);
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
        IpcRequest::SendPendingImage => SHARE_IMAGE_TIMEOUT,
        IpcRequest::SendClip { text } => {
            DEFAULT_TIMEOUT + Duration::from_secs((text.len() / (256 * 1024)) as u64)
        }
        _ => DEFAULT_TIMEOUT,
    }
}

/// What failing to open the control pipe means. Only a pipe that does not exist is "the daemon is not
/// running"; a busy pipe or a refused one must not send the person off to start a second daemon.
#[cfg_attr(not(windows), allow(dead_code))]
fn pipe_open_error(code: Option<i32>, busy_retries: u32) -> ClientError {
    const ERROR_FILE_NOT_FOUND: i32 = 2;
    const ERROR_PATH_NOT_FOUND: i32 = 3;
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_PIPE_BUSY: i32 = 231;
    match code {
        Some(ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) => ClientError::Down(
            "Klip daemon is not running or not responding. Launch it with `klip start`."
                .to_string(),
        ),
        Some(ERROR_ACCESS_DENIED) => ClientError::Io(
            "Windows refused access to the Klip control pipe (is Klip running as another user?)"
                .to_string(),
        ),
        Some(ERROR_PIPE_BUSY) => ClientError::Io(format!(
            "the Klip daemon is busy (the control pipe stayed busy for {} s); try again",
            u64::from(busy_retries) * 50 / 1000
        )),
        Some(other) => {
            ClientError::Io(format!("cannot open the Klip control pipe (error {other})"))
        }
        None => ClientError::Io("cannot open the Klip control pipe".to_string()),
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
        let stream = UnixStream::connect(&self.sock).await.map_err(|_| {
            ClientError::Down(
                "Klip daemon is not running or not responding. Launch it with `klip start`."
                    .to_string(),
            )
        })?;
        Self::talk(stream, req, max_request_bytes).await
    }

    /// Windows: the control channel is a named pipe. A busy pipe (every instance serving someone)
    /// is retried for a second; a missing one means the daemon is not running.
    #[cfg(windows)]
    async fn exchange(
        &self,
        req: &IpcRequest,
        max_request_bytes: usize,
    ) -> Result<IpcResponse, ClientError> {
        use tokio::net::windows::named_pipe::ClientOptions;
        const ERROR_PIPE_BUSY: i32 = 231;
        let mut tries = 0;
        let stream = loop {
            // SECURITY_IDENTIFICATION: whoever owns the pipe may learn who we are but never act as us.
            match ClientOptions::new()
                .security_qos_flags(0x0001_0000)
                .open(self.sock.as_os_str())
            {
                Ok(stream) => break stream,
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && tries < 20 => {
                    tries += 1;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) => return Err(pipe_open_error(e.raw_os_error(), tries)),
            }
        };
        Self::talk(stream, req, max_request_bytes).await
    }

    /// One framed request and its framed answer over any stream (a Unix socket, a named pipe).
    async fn talk<S: AsyncRead + AsyncWrite + Unpin>(
        mut stream: S,
        req: &IpcRequest,
        max_request_bytes: usize,
    ) -> Result<IpcResponse, ClientError> {
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

    async fn read_response<S: AsyncRead + Unpin>(
        stream: &mut S,
    ) -> Result<IpcResponse, ClientError> {
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

    #[cfg(not(any(unix, windows)))]
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
    #[test]
    fn sharing_an_image_gets_time_to_send_a_big_one() {
        // The daemon takes the held image out of its slot and then sends it: a client that gave up
        // after 15 s would report a failure for something that is still going through.
        assert!(timeout_for(&IpcRequest::SendPendingImage) >= Duration::from_secs(120));
        assert!(timeout_for(&IpcRequest::GetStatus) <= Duration::from_secs(20));
    }

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

    use tokio::io::duplex;

    /// A stand-in for the daemon: reads one framed request, answers with `reply` (or hangs up).
    async fn fake_daemon(mut stream: tokio::io::DuplexStream, reply: Option<IpcResponse>) {
        let mut len = [0u8; 4];
        stream.read_exact(&mut len).await.unwrap();
        let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
        stream.read_exact(&mut body).await.unwrap();
        let _ = deserialize_ipc_msg::<IpcRequest>(&body).unwrap();
        if let Some(reply) = reply {
            let bytes = serialize_ipc_msg(&reply).unwrap();
            stream
                .write_all(&(bytes.len() as u32).to_be_bytes())
                .await
                .unwrap();
            stream.write_all(&bytes).await.unwrap();
            stream.flush().await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_request_and_its_answer_travel_over_any_stream() {
        let (client, server) = duplex(64 * 1024);
        let daemon = tokio::spawn(fake_daemon(server, Some(IpcResponse::Resumed)));
        let got = DaemonClient::talk(client, &IpcRequest::Resume, 1024)
            .await
            .unwrap();
        assert!(matches!(got, IpcResponse::Resumed));
        daemon.await.unwrap();
    }

    #[tokio::test]
    async fn a_request_over_the_daemons_cap_is_refused_before_a_byte_is_written() {
        let (client, mut server) = duplex(64 * 1024);
        let err = DaemonClient::talk(
            client,
            &IpcRequest::SendClip {
                text: "x".repeat(5000),
            },
            100,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, ClientError::TooLarge { max: 100, .. }),
            "{err:?}"
        );
        let mut rest = Vec::new();
        server.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty(), "nothing was sent");
    }

    #[tokio::test]
    async fn a_daemon_that_hangs_up_without_answering_is_an_error() {
        let (client, server) = duplex(64 * 1024);
        let daemon = tokio::spawn(fake_daemon(server, None));
        let err = DaemonClient::talk(client, &IpcRequest::Resume, 1024)
            .await
            .unwrap_err();
        assert!(matches!(err, ClientError::Io(_)), "{err:?}");
        daemon.await.unwrap();
    }

    #[test]
    fn only_a_missing_pipe_means_the_daemon_is_not_running() {
        assert!(
            matches!(pipe_open_error(Some(2), 0), ClientError::Down(_)),
            "file not found"
        );
        assert!(
            matches!(pipe_open_error(Some(3), 0), ClientError::Down(_)),
            "path not found"
        );
        match pipe_open_error(Some(5), 0) {
            ClientError::Io(m) => assert!(m.to_lowercase().contains("access"), "{m}"),
            other => panic!("access denied is not 'down': {other:?}"),
        }
        match pipe_open_error(Some(231), 20) {
            ClientError::Io(m) => assert!(m.to_lowercase().contains("busy"), "{m}"),
            other => panic!("a busy pipe is not 'down': {other:?}"),
        }
        assert!(matches!(pipe_open_error(None, 0), ClientError::Io(_)));
    }
}
