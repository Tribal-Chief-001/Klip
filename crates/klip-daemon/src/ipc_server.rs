use crate::engine::Shutdown;
use crate::sync::SyncCoordinator;
use klip_net::PairingTicket;
use klip_proto::clip::ClipContentType;
use klip_proto::ipc::{
    deserialize_ipc_msg, serialize_ipc_msg, DaemonStatusInfo, IpcRequest, IpcResponse,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
use tokio::time::timeout;

/// Largest request the control socket accepts (a `klip send` of a big text is the worst case).
pub const MAX_IPC_REQUEST_BYTES: usize = 8 * 1024 * 1024;
const IPC_IO_TIMEOUT: Duration = Duration::from_secs(10);
const MIN_PAIR_TTL_SECS: u64 = 30;
const MAX_PAIR_TTL_SECS: u64 = 600;

pub struct IpcServer {
    sync: Arc<SyncCoordinator>,
    shutdown: Arc<Shutdown>,
}

impl IpcServer {
    pub fn new(sync: Arc<SyncCoordinator>, shutdown: Arc<Shutdown>) -> Self {
        Self { sync, shutdown }
    }

    /// Bind the control socket. Done before anything else so a failure is reported, never hidden.
    #[cfg(unix)]
    pub fn bind(&self) -> Result<UnixListener, std::io::Error> {
        let sock_path = self.sync.paths.ipc_socket();
        let _ = std::fs::remove_file(&sock_path);
        let listener = UnixListener::bind(&sock_path).map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!("cannot create control socket {}: {e}", sock_path.display()),
            )
        })?;
        // The socket can read clipboard history and stop the daemon: owner only.
        klip_crypto::identity::restrict_to_owner(&sock_path, 0o600);
        Ok(listener)
    }

    #[cfg(unix)]
    pub async fn serve(&self, listener: UnixListener) {
        loop {
            tokio::select! {
                _ = self.shutdown.wait() => break,
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        let sync = Arc::clone(&self.sync);
                        let shutdown = Arc::clone(&self.shutdown);
                        tokio::spawn(async move {
                            Self::handle_client(stream, sync, shutdown).await;
                        });
                    }
                    Err(_) => break,
                }
            }
        }
        let _ = std::fs::remove_file(self.sync.paths.ipc_socket());
    }

    #[cfg(unix)]
    async fn handle_client(
        mut stream: UnixStream,
        sync: Arc<SyncCoordinator>,
        shutdown: Arc<Shutdown>,
    ) {
        let request = match timeout(IPC_IO_TIMEOUT, Self::read_request(&mut stream)).await {
            Ok(Ok(req)) => req,
            Ok(Err(msg)) => {
                let _ = Self::send_response(&mut stream, &IpcResponse::Error(msg)).await;
                return;
            }
            Err(_) => return,
        };
        let response = Self::dispatch(request, &sync, &shutdown).await;
        let _ = timeout(IPC_IO_TIMEOUT, Self::send_response(&mut stream, &response)).await;
    }

    #[cfg(unix)]
    async fn read_request(stream: &mut UnixStream) -> Result<IpcRequest, String> {
        let mut len_buf = [0u8; 4];
        stream
            .read_exact(&mut len_buf)
            .await
            .map_err(|e| e.to_string())?;
        let len = u32::from_be_bytes(len_buf) as usize;
        if len > MAX_IPC_REQUEST_BYTES {
            return Err(format!("request too large ({len} bytes)"));
        }
        let mut buf = vec![0u8; len];
        stream
            .read_exact(&mut buf)
            .await
            .map_err(|e| e.to_string())?;
        deserialize_ipc_msg(&buf)
    }

    async fn dispatch(
        req: IpcRequest,
        sync: &Arc<SyncCoordinator>,
        shutdown: &Shutdown,
    ) -> IpcResponse {
        match req {
            IpcRequest::GetStatus => {
                let last_clip = sync.history.last_item();
                let last_sync_secs = sync
                    .last_sync_instant
                    .lock()
                    .unwrap()
                    .map(|t| t.elapsed().as_secs());
                let (preview, size) = match last_clip {
                    Some(c) => (Some(c.content_preview), c.size_bytes),
                    None => (None, 0),
                };
                IpcResponse::Status(DaemonStatusInfo {
                    is_active: true,
                    is_paused: sync.check_pause_status(),
                    paused_remaining_secs: sync.pause_remaining_secs(),
                    platform: sync.driver.environment().to_string(),
                    memory_rss_bytes: get_process_rss_bytes(),
                    node_id: sync.identity.node_id,
                    relay_mode: sync.relay_mode_label().to_string(),
                    pairing_open: sync.net.pairing_open(),
                    peers: sync.peer_status(),
                    total_synced_clips: sync
                        .total_synced_clips
                        .load(std::sync::atomic::Ordering::SeqCst),
                    last_sync_time_secs_ago: last_sync_secs,
                    last_clip_preview: preview,
                    last_clip_size_bytes: size,
                })
            }
            IpcRequest::PairStart {
                ttl_secs,
                include_lan_addrs,
            } => {
                let ttl = ttl_secs.clamp(MIN_PAIR_TTL_SECS, MAX_PAIR_TTL_SECS);
                sync.mark_pairing_started(ttl);
                let ticket = sync
                    .net
                    .open_pairing(Duration::from_secs(ttl), include_lan_addrs)
                    .await;
                IpcResponse::PairTicket {
                    ticket: ticket.encode(),
                    expires_in_secs: ticket.seconds_left(),
                    host_fingerprint: sync.identity.node_id.fingerprint(),
                }
            }
            IpcRequest::PairPoll => IpcResponse::PairPoll(sync.poll_pairing()),
            IpcRequest::PairDecide { accept } => {
                if sync.decide_pairing(accept) {
                    IpcResponse::PairPoll(sync.poll_pairing())
                } else {
                    IpcResponse::Error("no pairing request is waiting".into())
                }
            }
            IpcRequest::PairCancel => {
                sync.cancel_pairing();
                IpcResponse::PairCancelled
            }
            IpcRequest::PairJoin { ticket } => {
                let ticket = match PairingTicket::decode(&ticket) {
                    Ok(t) => t,
                    Err(e) => return IpcResponse::Error(e.to_string()),
                };
                match sync.net.pair_with(&ticket).await {
                    Ok(rec) => {
                        sync.persist_peer(&rec).await;
                        IpcResponse::PairSuccess {
                            peer_name: rec.name,
                            peer_fingerprint: crate::sync::node_of(&rec.id).fingerprint(),
                        }
                    }
                    Err(e) => IpcResponse::Error(e.to_string()),
                }
            }
            IpcRequest::Unpair { selector } => match sync.unpair(&selector).await {
                Ok(name) => IpcResponse::Unpaired { name },
                Err(e) => IpcResponse::Error(e),
            },
            IpcRequest::Pause { duration_secs } => {
                sync.pause(duration_secs);
                IpcResponse::Paused {
                    remaining_secs: duration_secs,
                }
            }
            IpcRequest::Resume => {
                sync.resume();
                IpcResponse::Resumed
            }
            IpcRequest::GetHistory { limit } => {
                IpcResponse::History(sync.history.get_recent(limit))
            }
            IpcRequest::SendClip { text } => {
                let bytes = text.into_bytes();
                let len = bytes.len();
                if len > sync.config.max_clip_bytes {
                    return IpcResponse::Error(format!(
                        "text is {len} bytes; the limit is {} bytes",
                        sync.config.max_clip_bytes
                    ));
                }
                match sync
                    .broadcast_local_clip(ClipContentType::PlainTextUtf8, bytes)
                    .await
                {
                    None => IpcResponse::Error("sync is paused or the text is empty".into()),
                    Some(reports) => {
                        let mut delivered = Vec::new();
                        let mut failed = Vec::new();
                        for r in reports {
                            match r.result {
                                Ok(()) => delivered.push(r.peer_name),
                                Err(e) => failed.push((r.peer_name, e)),
                            }
                        }
                        IpcResponse::Sent {
                            bytes: len,
                            delivered,
                            failed,
                        }
                    }
                }
            }
            IpcRequest::StopDaemon => {
                shutdown.trigger();
                IpcResponse::DaemonStopping
            }
        }
    }

    #[cfg(unix)]
    async fn send_response(
        stream: &mut UnixStream,
        resp: &IpcResponse,
    ) -> Result<(), std::io::Error> {
        let serialized = serialize_ipc_msg(resp).map_err(std::io::Error::other)?;
        stream
            .write_all(&(serialized.len() as u32).to_be_bytes())
            .await?;
        stream.write_all(&serialized).await?;
        stream.flush().await
    }
}

fn get_process_rss_bytes() -> u64 {
    // /proc/self/status reports VmRSS in kB, which avoids guessing the page size.
    #[cfg(target_os = "linux")]
    {
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            if let Some(kb) = status
                .lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<u64>().ok())
            {
                return kb * 1024;
            }
        }
    }
    0
}
