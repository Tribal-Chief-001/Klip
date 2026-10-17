//! The coordinator's side of file transfer: take what paired devices send, send what the person asks.
use crate::files::{
    default_receive_dir, prepare_send, receive_incoming, send_prepared, FilePolicy,
};
use crate::sync::{node_of, FileReceivedHook, SyncCoordinator};
use klip_net::file::IncomingFile;
use klip_net::{EndpointId, FileConnection, RecvStream, SendStream};
use klip_proto::hlc::NodeId;
use klip_proto::ipc::{TransferDirection, TransferInfo, TransferState};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Stale `.klip-*.part` files (a transfer that died with the daemon) older than this are removed at
/// start; nothing of this daemon is running then, so only a clock skew needs the margin.
/// Files waiting for their turn each hold an open descriptor: more than this are refused.
const MAX_WAITING_SENDS: usize = 64;

pub(crate) const PARTIAL_MAX_AGE: Duration = Duration::from_secs(3600);

impl SyncCoordinator {
    /// What this device accepts, from its settings.
    pub fn file_policy(&self) -> FilePolicy {
        let dir = self
            .config
            .receive_dir
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(default_receive_dir);
        let mut policy = FilePolicy::new(dir);
        policy.accept = self.config.accept_files;
        policy.max_bytes = self.config.max_file_bytes;
        policy.space_factor = self.config.file_space_factor;
        policy
    }

    pub fn set_file_received_hook(&self, hook: FileReceivedHook) {
        *self.file_hook.lock().unwrap() = Some(hook);
    }

    /// The transfers that are running and the last 50 that finished.
    pub fn transfers(&self) -> Vec<TransferInfo> {
        self.transfers.list()
    }

    /// Stop a running transfer. `false`: no running transfer has that id.
    pub fn cancel_transfer(&self, id: [u8; 16]) -> bool {
        self.transfers.cancel(id)
    }

    fn peer_name(&self, id: &EndpointId) -> String {
        self.net
            .peers()
            .into_iter()
            .find(|p| &p.id == id)
            .map(|p| p.name)
            .unwrap_or_else(|| "a device".to_string())
    }

    /// A paired device opened a file stream: take the file on a task of its own (the event queue is
    /// shared with clips and must never wait for a transfer).
    pub(crate) fn spawn_incoming_file(
        self: &Arc<Self>,
        from: EndpointId,
        tx: SendStream,
        rx: RecvStream,
        conn: FileConnection,
    ) {
        let me = Arc::clone(self);
        tokio::spawn(async move {
            if let Ok(incoming) = IncomingFile::open(rx, tx).await {
                let peer = me.peer_name(&from);
                let policy = me.file_policy();
                if let Ok(path) = receive_incoming(incoming, &peer, &policy, &me.transfers).await {
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    let hook = me.file_hook.lock().unwrap().clone();
                    if let Some(hook) = hook {
                        hook(&peer, &path, size);
                    }
                }
            }
            conn.finished().await;
        });
    }

    fn send_lock(&self, node: NodeId) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(
            self.file_send_locks
                .lock()
                .unwrap()
                .entry(node)
                .or_default(),
        )
    }

    /// Send a file to every connected paired device, or to the one called `to` (a name or the start of
    /// its fingerprint). One answer per device: a transfer that has started, or why it could not.
    /// The transfers run on their own tasks; `transfers()` shows how they are doing.
    pub async fn send_file(
        self: &Arc<Self>,
        path: PathBuf,
        to: Option<String>,
    ) -> Vec<Result<TransferInfo, String>> {
        let peers = self.net.peers();
        if peers.is_empty() {
            return vec![Err("No linked devices. Run `klip pair` first.".to_string())];
        }
        let norm = |s: &str| s.replace('-', "").to_uppercase();
        let targets: Vec<_> = match &to {
            None => peers,
            Some(wanted) => {
                let wanted = wanted.trim();
                let hits: Vec<_> = peers
                    .into_iter()
                    .filter(|p| {
                        p.name == wanted
                            || (norm(wanted).len() >= 4
                                && norm(&node_of(&p.id).fingerprint()).starts_with(&norm(wanted)))
                    })
                    .collect();
                if hits.is_empty() {
                    return vec![Err(format!("no paired device matches '{wanted}'"))];
                }
                hits
            }
        };
        let connected: std::collections::HashSet<EndpointId> = self
            .net
            .peer_statuses()
            .into_iter()
            .filter(|s| s.connected)
            .map(|s| s.id)
            .collect();
        let mut answers = Vec::new();
        for peer in targets {
            if !connected.contains(&peer.id) {
                answers.push(Err(format!("{} is not connected right now", peer.name)));
                continue;
            }
            if self
                .transfers
                .list()
                .iter()
                .filter(|t| {
                    t.direction == TransferDirection::Sending && t.state == TransferState::Waiting
                })
                .count()
                >= MAX_WAITING_SENDS
            {
                answers.push(Err(format!(
                    "{MAX_WAITING_SENDS} files are already waiting to be sent; let some finish first"
                )));
                continue;
            }
            // One open file per device: a file is read once per receiver, never shared.
            let prepared = match prepare_send(&path).await {
                Ok(p) => p,
                Err(e) => {
                    answers.push(Err(format!("cannot send {}: {e}", path.display())));
                    break;
                }
            };
            let mut id = [0u8; 16];
            rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut id);
            let token = self.transfers.begin(
                &peer.name,
                TransferDirection::Sending,
                id,
                &prepared.name,
                prepared.size,
                TransferState::Waiting,
            );
            let lock = self.send_lock(node_of(&peer.id));
            let me = Arc::clone(self);
            let (peer_id, node_peer) = (peer.id, peer.name.clone());
            let mut token = token;
            tokio::spawn(async move {
                // A cancel while waiting ends the wait at once and frees the open file.
                let _turn = tokio::select! {
                    turn = lock.lock() => turn,
                    _ = token.cancelled() => {
                        me.transfers.finish(
                            id,
                            TransferState::Cancelled,
                            Some("cancelled".into()),
                            None,
                        );
                        return;
                    }
                };
                if token.is_cancelled() {
                    me.transfers.finish(
                        id,
                        TransferState::Cancelled,
                        Some("cancelled".into()),
                        None,
                    );
                    return;
                }
                match me.net.open_file_stream(peer_id).await {
                    Ok(mut streams) => {
                        let _ = send_prepared(
                            prepared,
                            id,
                            &mut streams.tx,
                            &mut streams.rx,
                            token,
                            &me.transfers,
                        )
                        .await;
                        streams.close();
                    }
                    Err(e) => me.transfers.finish(
                        id,
                        TransferState::Failed,
                        Some(format!("{node_peer}: {e}")),
                        None,
                    ),
                }
            });
            answers.push(
                self.transfers
                    .list()
                    .into_iter()
                    .rev()
                    .find(|t| t.id == id)
                    .ok_or_else(|| "the transfer could not be started".to_string()),
            );
        }
        answers
    }
}
