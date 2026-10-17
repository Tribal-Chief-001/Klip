use klip_core::RelayMode;
use klip_core::Shutdown;
use klip_core::SyncCoordinator;
use klip_net::PairingTicket;
use klip_proto::clip::ClipContentType;
use klip_proto::ipc::{
    deserialize_ipc_msg, max_request_bytes, serialize_ipc_msg, ConfigInfo, HelloInfo, IpcRequest,
    IpcResponse, CAP_CLEAR_HISTORY, CAP_COPY_TO_CLIPBOARD, CAP_FILE_FOLDERS, CAP_FILE_TRANSFER,
    CAP_SHARE_IMAGE, CAP_UNPAIR_ALL, IPC_PROTOCOL_VERSION,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
#[cfg(unix)]
use tokio::net::UnixListener;
use tokio::time::timeout;

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

    /// Create the control pipe (the first instance), before anything else, so a failure is reported.
    #[cfg(windows)]
    pub fn bind_pipe(
        &self,
    ) -> Result<
        (
            crate::ipc_pipe::PipeSecurity,
            tokio::net::windows::named_pipe::NamedPipeServer,
        ),
        std::io::Error,
    > {
        let name = self.sync.paths.ipc_pipe_name();
        let mut security = crate::ipc_pipe::PipeSecurity::owner_only()?;
        let first = security.create(&name, true).map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!("cannot create the control pipe {name}: {e}"),
            )
        })?;
        Ok((security, first))
    }

    /// Serve clients, one pipe instance each: the next instance is created before the connected one
    /// is handed to its handler, so there is always one waiting.
    #[cfg(windows)]
    pub async fn serve_pipe(
        &self,
        mut security: crate::ipc_pipe::PipeSecurity,
        first: tokio::net::windows::named_pipe::NamedPipeServer,
    ) {
        let name = self.sync.paths.ipc_pipe_name();
        let mut server = first;
        loop {
            tokio::select! {
                _ = self.shutdown.wait() => break,
                connected = server.connect() => {
                    // One failed connect or one failed create must not end the control channel for
                    // good (every command would then say "not running"): log it, make a fresh
                    // instance (retrying with a growing pause) and carry on until shutdown.
                    let ready = match connected {
                        Ok(()) => Some(server),
                        Err(e) => {
                            eprintln!("control pipe: a client could not connect ({e})");
                            None
                        }
                    };
                    server = match self.next_instance(&mut security, &name).await {
                        Some(next) => next,
                        None => break,
                    };
                    if let Some(ready) = ready {
                        let sync = Arc::clone(&self.sync);
                        let shutdown = Arc::clone(&self.shutdown);
                        tokio::spawn(async move {
                            Self::handle_client(ready, sync, shutdown).await;
                        });
                    }
                }
            }
        }
    }

    /// The next pipe instance, retried until it exists; `None` when the daemon shuts down first.
    #[cfg(windows)]
    async fn next_instance(
        &self,
        security: &mut crate::ipc_pipe::PipeSecurity,
        name: &str,
    ) -> Option<tokio::net::windows::named_pipe::NamedPipeServer> {
        let mut pause = Duration::from_millis(100);
        loop {
            match security.create(name, false) {
                Ok(next) => return Some(next),
                Err(e) => {
                    eprintln!("control pipe: cannot create the next instance ({e}); retrying")
                }
            }
            tokio::select! {
                _ = self.shutdown.wait() => return None,
                _ = tokio::time::sleep(pause) => {}
            }
            pause = (pause * 2).min(Duration::from_secs(5));
        }
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

    async fn handle_client<S>(mut stream: S, sync: Arc<SyncCoordinator>, shutdown: Arc<Shutdown>)
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let cap = max_request_bytes(sync.config.max_clip_bytes);
        let request = match timeout(IPC_IO_TIMEOUT, Self::read_request(&mut stream, cap)).await {
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

    async fn read_request<S: AsyncRead + Unpin>(
        stream: &mut S,
        cap: usize,
    ) -> Result<IpcRequest, String> {
        let mut len_buf = [0u8; 4];
        stream
            .read_exact(&mut len_buf)
            .await
            .map_err(|e| e.to_string())?;
        let len = u32::from_be_bytes(len_buf) as usize;
        if len > cap {
            return Err(format!(
                "request too large ({len} bytes; this daemon accepts at most {cap})"
            ));
        }
        // Grow the buffer as bytes arrive. A client that announces a huge length and then sends
        // nothing must not make the daemon reserve that much memory.
        let mut buf = Vec::new();
        let got = (&mut *stream)
            .take(len as u64)
            .read_to_end(&mut buf)
            .await
            .map_err(|e| e.to_string())?;
        if got != len {
            return Err("request ended before its announced length".to_string());
        }
        deserialize_ipc_msg(&buf)
    }

    async fn dispatch(
        req: IpcRequest,
        sync: &Arc<SyncCoordinator>,
        shutdown: &Shutdown,
    ) -> IpcResponse {
        match req {
            IpcRequest::GetStatus => IpcResponse::Status(sync.status(
                sync.driver.environment().to_string(),
                get_process_rss_bytes(),
            )),
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
                        if let Err(e) = sync.persist_peer(&rec).await {
                            return IpcResponse::Error(e);
                        }
                        IpcResponse::PairSuccess {
                            peer_name: rec.name,
                            peer_fingerprint: klip_core::node_of(&rec.id).fingerprint(),
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
            IpcRequest::Hello { client_protocol: _ } => {
                let cfg = &sync.config;
                IpcResponse::Hello(HelloInfo {
                    protocol: IPC_PROTOCOL_VERSION,
                    daemon_version: env!("CARGO_PKG_VERSION").to_string(),
                    capabilities: vec![
                        CAP_UNPAIR_ALL.to_string(),
                        CAP_COPY_TO_CLIPBOARD.to_string(),
                        CAP_CLEAR_HISTORY.to_string(),
                        CAP_SHARE_IMAGE.to_string(),
                        CAP_FILE_TRANSFER.to_string(),
                        CAP_FILE_FOLDERS.to_string(),
                    ],
                    config: ConfigInfo {
                        device_name: cfg.device_name.clone(),
                        relay: match cfg.relay {
                            RelayMode::N0 => "n0".to_string(),
                            RelayMode::Lan => "lan".to_string(),
                        },
                        max_history_items: cfg.max_history_items,
                        max_clip_bytes: cfg.max_clip_bytes,
                        lan_discovery: cfg.lan_discovery,
                    },
                    max_request_bytes: max_request_bytes(cfg.max_clip_bytes),
                })
            }
            IpcRequest::UnpairAll => {
                let (removed, failed) = sync.unpair_all().await;
                IpcResponse::UnpairedAll { removed, failed }
            }
            IpcRequest::CopyToClipboard { id } => match sync.copy_history_item(id).await {
                Ok(()) => IpcResponse::Copied,
                Err(e) => IpcResponse::Error(e),
            },
            IpcRequest::ClearHistory => {
                sync.clear_history();
                IpcResponse::HistoryCleared
            }
            IpcRequest::GetImageState => IpcResponse::ImageState(sync.image_state()),
            IpcRequest::SendPendingImage => {
                let size = sync
                    .image_state()
                    .waiting
                    .map(|w| w.size_bytes)
                    .unwrap_or(0);
                match sync.send_pending_image().await {
                    Err(e) => IpcResponse::Error(e),
                    Ok(reports) => {
                        let mut delivered = Vec::new();
                        let mut failed = Vec::new();
                        for r in reports {
                            match r.result {
                                Ok(()) => delivered.push(r.peer_name),
                                Err(e) => failed.push((r.peer_name, e)),
                            }
                        }
                        IpcResponse::Sent {
                            bytes: size,
                            delivered,
                            failed,
                        }
                    }
                }
            }
            IpcRequest::SetAutoImages { enabled } => {
                sync.set_auto_images(enabled);
                // Change only this one setting and keep the rest of the file as it is. A file the
                // person broke by hand (a typo) is left exactly as it is: rewriting it from the running
                // settings would silently throw their edit away.
                let mut config = match klip_core::DaemonConfig::load(&sync.paths) {
                    Ok(config) => config,
                    Err(e) => {
                        return IpcResponse::Error(format!(
                            "the setting is on for now, but config.json has a problem and was left as it is \
                             ({e}); fix it, then switch again"
                        ))
                    }
                };
                config.auto_send_images = enabled;
                match config.save(&sync.paths) {
                    Ok(()) => IpcResponse::ImageState(sync.image_state()),
                    Err(e) => IpcResponse::Error(format!(
                        "the setting is on for now but config.json could not be saved: {e}"
                    )),
                }
            }
            IpcRequest::GetThumbnail { id } => {
                let sync = Arc::clone(sync);
                match tokio::task::spawn_blocking(move || sync.thumbnail(id)).await {
                    Ok(Ok(png)) => IpcResponse::Thumbnail(png),
                    Ok(Err(e)) => IpcResponse::Error(e),
                    Err(e) => IpcResponse::Error(e.to_string()),
                }
            }
            IpcRequest::SendFile { path, to } => {
                let path = std::path::PathBuf::from(path);
                if !path.is_absolute() {
                    IpcResponse::Error(format!(
                        "the file path must be absolute: {}",
                        path.display()
                    ))
                } else {
                    let file_name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let answers = sync.send_file(path, to).await;
                    match klip_core::started_or_error(&file_name, answers) {
                        Ok(started) => IpcResponse::TransfersStarted(started),
                        Err(reasons) => IpcResponse::Error(reasons),
                    }
                }
            }
            IpcRequest::GetTransfers => IpcResponse::Transfers(sync.transfers()),
            IpcRequest::CancelTransfer { id } => {
                IpcResponse::TransferCancelled(sync.cancel_transfer(id))
            }
            IpcRequest::StopDaemon => {
                shutdown.trigger();
                IpcResponse::DaemonStopping
            }
        }
    }

    async fn send_response<S: AsyncWrite + Unpin>(
        stream: &mut S,
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
    #[cfg(target_os = "macos")]
    {
        // SAFETY: proc_pidinfo fills a zeroed proc_taskinfo of exactly the size we pass.
        let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
        let got = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDTASKINFO,
                0,
                &mut info as *mut _ as *mut libc::c_void,
                size,
            )
        };
        if got == size {
            return info.pti_resident_size;
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        // SAFETY: the counters struct is zeroed with its size set, as the API requires.
        let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) } != 0 {
            return counters.WorkingSetSize as u64;
        }
    }
    0
}

#[cfg(test)]
mod framing_tests {
    use super::*;
    use tokio::io::{duplex, AsyncWriteExt};

    async fn read_from(bytes: Vec<u8>, cap: usize) -> Result<IpcRequest, String> {
        let (mut client, mut server) = duplex(64 * 1024);
        client.write_all(&bytes).await.unwrap();
        drop(client); // the end of the stream
        IpcServer::read_request(&mut server, cap).await
    }

    fn frame(req: &IpcRequest) -> Vec<u8> {
        let body = serialize_ipc_msg(req).unwrap();
        let mut out = (body.len() as u32).to_be_bytes().to_vec();
        out.extend(body);
        out
    }

    #[tokio::test]
    async fn a_request_is_read_from_any_stream_not_only_a_unix_socket() {
        let got = read_from(frame(&IpcRequest::GetStatus), 1024)
            .await
            .unwrap();
        assert!(matches!(got, IpcRequest::GetStatus));
    }

    #[tokio::test]
    async fn a_request_over_the_cap_is_refused_before_it_is_read() {
        let mut bytes = (10_000_000u32).to_be_bytes().to_vec();
        bytes.extend([1, 2, 3]);
        let err = read_from(bytes, 1024).await.unwrap_err();
        assert!(err.contains("too large"), "{err}");
    }

    #[tokio::test]
    async fn a_request_that_ends_early_is_an_error_not_a_hang() {
        let mut bytes = (100u32).to_be_bytes().to_vec();
        bytes.extend([7; 10]);
        let err = read_from(bytes, 1024).await.unwrap_err();
        assert!(err.contains("ended before"), "{err}");
    }

    #[tokio::test]
    async fn a_response_is_written_as_length_then_body_to_any_stream() {
        let (mut client, mut server) = duplex(64 * 1024);
        IpcServer::send_response(&mut server, &IpcResponse::Resumed)
            .await
            .unwrap();
        drop(server);
        let mut got = Vec::new();
        client.read_to_end(&mut got).await.unwrap();
        let len = u32::from_be_bytes(got[..4].try_into().unwrap()) as usize;
        assert_eq!(len, got.len() - 4);
        assert!(matches!(
            deserialize_ipc_msg::<IpcResponse>(&got[4..]).unwrap(),
            IpcResponse::Resumed
        ));
    }
}
