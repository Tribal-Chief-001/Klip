//! The TUI's view of the daemon, over the control socket.
use crate::client::{ClientError, DaemonClient, OLD_DAEMON_REQUEST_CAP};
use crate::ops::{log_tail, request_cap, spawn_daemon_process, unpair_all_compat, Spawned};
use async_trait::async_trait;
use klip_daemon::KlipPaths;
use klip_proto::ipc::{IpcRequest, IpcResponse, IPC_PROTOCOL_VERSION};
use klip_tui::api::*;
use std::time::Duration;

pub struct IpcDaemonApi {
    client: DaemonClient,
    paths: KlipPaths,
}

impl IpcDaemonApi {
    pub fn new(client: DaemonClient, paths: KlipPaths) -> Self {
        Self { client, paths }
    }

    /// One call; the daemon's own refusal becomes `ApiError::Refused`.
    async fn call(&self, req: IpcRequest) -> Result<IpcResponse, ApiError> {
        match self
            .client
            .request(&req, OLD_DAEMON_REQUEST_CAP)
            .await
            .map_err(map_err)?
        {
            IpcResponse::Error(e) => Err(ApiError::Refused(e)),
            other => Ok(other),
        }
    }
}

fn map_err(e: ClientError) -> ApiError {
    match e {
        ClientError::Down(_) => ApiError::DaemonDown,
        ClientError::Timeout(s) => ApiError::Timeout(s),
        ClientError::TooLarge { size, max } => ApiError::TooLarge { size, max },
        ClientError::Io(m) => ApiError::Io(m),
    }
}

fn unexpected(r: IpcResponse) -> ApiError {
    ApiError::Io(format!("unexpected reply: {r:?}"))
}

#[async_trait]
impl DaemonApi for IpcDaemonApi {
    async fn hello(&self) -> Result<HelloInfo, ApiError> {
        match self
            .call(IpcRequest::Hello {
                client_protocol: IPC_PROTOCOL_VERSION,
            })
            .await
        {
            Ok(IpcResponse::Hello(h)) => Ok(h),
            Ok(other) => Err(unexpected(other)),
            // A daemon that predates protocol 2 answers an unknown request with an error.
            Err(ApiError::Refused(_)) => Err(ApiError::Unsupported),
            Err(e) => Err(e),
        }
    }

    async fn status(&self) -> Result<DaemonStatusInfo, ApiError> {
        match self.call(IpcRequest::GetStatus).await? {
            IpcResponse::Status(s) => Ok(s),
            other => Err(unexpected(other)),
        }
    }

    async fn history(&self, limit: usize) -> Result<Vec<HistoryItem>, ApiError> {
        match self.call(IpcRequest::GetHistory { limit }).await? {
            IpcResponse::History(items) => Ok(items),
            other => Err(unexpected(other)),
        }
    }

    async fn send(&self, text: String) -> Result<SendReport, ApiError> {
        let cap = request_cap(&self.client).await;
        match self
            .client
            .request(&IpcRequest::SendClip { text }, cap)
            .await
            .map_err(map_err)?
        {
            IpcResponse::Sent {
                bytes,
                delivered,
                failed,
            } => Ok(SendReport {
                bytes,
                delivered,
                failed,
            }),
            IpcResponse::Error(e) => Err(ApiError::Refused(e)),
            other => Err(unexpected(other)),
        }
    }

    async fn pause(&self, secs: u64) -> Result<u64, ApiError> {
        match self
            .call(IpcRequest::Pause {
                duration_secs: secs,
            })
            .await?
        {
            IpcResponse::Paused { remaining_secs } => Ok(remaining_secs),
            other => Err(unexpected(other)),
        }
    }

    async fn resume(&self) -> Result<(), ApiError> {
        match self.call(IpcRequest::Resume).await? {
            IpcResponse::Resumed => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    async fn unpair(&self, selector: String) -> Result<String, ApiError> {
        match self.call(IpcRequest::Unpair { selector }).await? {
            IpcResponse::Unpaired { name } => Ok(name),
            other => Err(unexpected(other)),
        }
    }

    async fn unpair_all(&self) -> Result<UnpairReport, ApiError> {
        unpair_all_compat(&self.client).await.map_err(map_err)
    }

    async fn copy_to_clipboard(&self, id: [u8; 16]) -> Result<(), ApiError> {
        match self.call(IpcRequest::CopyToClipboard { id }).await? {
            IpcResponse::Copied => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    async fn clear_history(&self) -> Result<(), ApiError> {
        match self.call(IpcRequest::ClearHistory).await? {
            IpcResponse::HistoryCleared => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    async fn pair_start(&self, ttl_secs: u64, lan: bool) -> Result<PairTicket, ApiError> {
        match self
            .call(IpcRequest::PairStart {
                ttl_secs,
                include_lan_addrs: lan,
            })
            .await?
        {
            IpcResponse::PairTicket {
                ticket,
                expires_in_secs,
                host_fingerprint,
            } => Ok(PairTicket {
                ticket,
                expires_in_secs,
                host_fingerprint,
            }),
            other => Err(unexpected(other)),
        }
    }

    async fn pair_poll(&self) -> Result<PairPollInfo, ApiError> {
        match self.call(IpcRequest::PairPoll).await? {
            IpcResponse::PairPoll(i) => Ok(i),
            other => Err(unexpected(other)),
        }
    }

    async fn pair_decide(&self, accept: bool) -> Result<PairPollInfo, ApiError> {
        match self.call(IpcRequest::PairDecide { accept }).await? {
            IpcResponse::PairPoll(i) => Ok(i),
            other => Err(unexpected(other)),
        }
    }

    async fn pair_cancel(&self) -> Result<(), ApiError> {
        match self.call(IpcRequest::PairCancel).await? {
            IpcResponse::PairCancelled => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    async fn pair_join(&self, ticket: String) -> Result<Paired, ApiError> {
        match self.call(IpcRequest::PairJoin { ticket }).await? {
            IpcResponse::PairSuccess {
                peer_name,
                peer_fingerprint,
            } => Ok(Paired {
                name: peer_name,
                fingerprint: peer_fingerprint,
            }),
            other => Err(unexpected(other)),
        }
    }

    async fn stop_daemon(&self) -> Result<(), ApiError> {
        match self.call(IpcRequest::StopDaemon).await? {
            IpcResponse::DaemonStopping => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    async fn start_daemon(&self) -> Result<u32, ApiError> {
        if self.status().await.is_ok() {
            return Err(ApiError::Refused("Klip is already running".to_string()));
        }
        let Spawned {
            mut child,
            log_path,
        } = spawn_daemon_process(&self.paths).map_err(ApiError::Io)?;
        let pid = child.id();
        for _ in 0..120 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if self.status().await.is_ok() {
                return Ok(pid);
            }
            if let Ok(Some(status)) = child.try_wait() {
                return Err(ApiError::Refused(format!(
                    "Klip stopped right after starting ({status}). Last log lines:\n{}",
                    log_tail(&log_path, 4)
                )));
            }
        }
        Err(ApiError::Refused(format!(
            "Klip did not come up within 30 s. See the log: {}",
            log_path.display()
        )))
    }

    async fn install_service(&self) -> Result<String, ApiError> {
        match tokio::task::spawn_blocking(crate::ops::install_service).await {
            Ok(Ok(Some(msg))) => Ok(msg),
            Ok(Ok(None)) => Err(ApiError::Refused(
                "this system has no auto-start installer".to_string(),
            )),
            Ok(Err(e)) => Err(ApiError::Io(e)),
            Err(e) => Err(ApiError::Io(e.to_string())),
        }
    }
}
