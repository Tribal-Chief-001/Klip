//! The coordinator's side of file transfer: take what paired devices send, send what the person asks.
use crate::files::{
    default_receive_dir, finish_send, prepare_send, receive_incoming_from, send_attempt,
    FilePolicy, PreparedFile,
};
use crate::sync::{node_of, FileReceivedHook, SyncCoordinator};
use klip_net::file::{FileError, IncomingFile, RejectCode};
use klip_net::{EndpointId, FileConnection, RecvStream, SendStream};
use klip_proto::hlc::NodeId;
use klip_proto::ipc::{TransferDirection, TransferInfo, TransferState};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Partial files older than this are removed at start: a transfer that died with the daemon can still be
/// resumed by sending the same file again for this long.
/// Files waiting for their turn each hold an open descriptor: more than this are refused.
const MAX_WAITING_SENDS: usize = 64;

pub(crate) const PARTIAL_MAX_AGE: Duration = Duration::from_secs(24 * 3600);

/// The answers of a send as one result: `Err` (the reasons joined) when no device was started, otherwise the
/// started transfers followed by an already-failed entry for each device that could not be reached, so the
/// person sees them side by side.
pub fn started_or_error(
    file_name: &str,
    answers: Vec<Result<TransferInfo, String>>,
) -> Result<Vec<TransferInfo>, String> {
    let (mut started, mut errors) = (Vec::new(), Vec::new());
    for answer in answers {
        match answer {
            Ok(t) => started.push(t),
            Err(e) => errors.push(e),
        }
    }
    if started.is_empty() {
        return Err(errors.join("; "));
    }
    for (i, error) in errors.into_iter().enumerate() {
        started.push(TransferInfo {
            id: [0xFF; 16].map(|b| b ^ i as u8),
            peer: "device".into(),
            direction: TransferDirection::Sending,
            name: file_name.to_string(),
            size: 0,
            done: 0,
            state: TransferState::Failed,
            error: Some(error),
            saved_to: None,
            started_ms: 0,
            finished_ms: Some(0),
        });
    }
    Ok(started)
}

/// A resumed send that the receiver judged damaged (the kept part was not the same file any more) is sent again
/// from the start, once. Any other verdict (the person cancelled over there, the disk is full, too slow) is final.
fn retry_from_zero(resumed: u64, fresh_key: bool, result: &Result<(), FileError>) -> bool {
    resumed > 0
        && !fresh_key
        && matches!(result, Err(FileError::Peer(text)) if text == klip_net::file::DAMAGED)
}

/// Why one file of a folder did not go: it alone (the others go on), or the whole send must stop.
enum FolderFail {
    File(FileError),
    Stop(String),
}

/// What a send reads from: a path (opened once per receiving device) or a file somebody opened.
enum Source {
    Path(PathBuf),
    Open {
        file: std::fs::File,
        name: String,
        serial: Arc<tokio::sync::Mutex<()>>,
    },
}

impl Source {
    async fn prepare(&self) -> Result<PreparedFile, String> {
        match self {
            Source::Path(path) => prepare_send(path)
                .await
                .map_err(|e| format!("cannot send {}: {e}", path.display())),
            Source::Open { file, name, serial } => file
                .try_clone()
                .and_then(|f| PreparedFile::from_file(f, name, Some(Arc::clone(serial))))
                .map_err(|e| format!("cannot send {name}: {e}")),
        }
    }
}

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

    /// Called whenever a transfer begins or ends (never for progress), from whatever thread did it.
    pub fn set_transfers_changed_hook(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        self.transfers.set_on_change(hook);
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
        version: u8,
    ) {
        let me = Arc::clone(self);
        tokio::spawn(async move {
            let opened = if version >= 2 {
                IncomingFile::open_v2(rx, tx).await
            } else {
                IncomingFile::open(rx, tx).await
            };
            if let Ok(incoming) = opened {
                // The header can take a while to arrive: the device may have been unpaired meanwhile.
                if !me.net.is_paired(&from) {
                    incoming
                        .reject(
                            RejectCode::Other,
                            "this device is not linked with you any more",
                        )
                        .await;
                    conn.finished().await;
                    return;
                }
                let peer = me.peer_name(&from);
                let policy = me.file_policy();
                // Kept partial files older than a day go whenever a transfer starts, not only when the daemon does.
                let sweep_dir = policy.dir.clone();
                tokio::task::spawn_blocking(move || {
                    crate::files::sweep_partials(&sweep_dir, PARTIAL_MAX_AGE)
                });
                let node = node_of(&from);
                if let Ok(path) =
                    receive_incoming_from(incoming, &peer, Some(node), &policy, &me.transfers).await
                {
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
        self.send_to_targets(Source::Path(path), to).await
    }

    /// Like `send_file`, for a file somebody else opened (an Android descriptor). The caller may close
    /// its own handle once this returns; each send works on a duplicate.
    pub async fn send_open_file(
        self: &Arc<Self>,
        file: std::fs::File,
        name: String,
        to: Option<String>,
    ) -> Vec<Result<TransferInfo, String>> {
        let serial = Arc::new(tokio::sync::Mutex::new(()));
        self.send_to_targets(Source::Open { file, name, serial }, to)
            .await
    }

    /// The devices a send goes to (all of them, or the one `to` names: a name or the start of its fingerprint) and
    /// which of them are connected right now.
    fn select_targets(
        &self,
        to: &Option<String>,
    ) -> Result<
        (
            Vec<klip_net::PeerRecord>,
            std::collections::HashSet<EndpointId>,
        ),
        String,
    > {
        let peers = self.net.peers();
        if peers.is_empty() {
            return Err("No linked devices. Run `klip pair` first.".to_string());
        }
        let norm = |s: &str| s.replace('-', "").to_uppercase();
        let targets: Vec<_> = match to {
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
                    return Err(format!("no paired device matches '{wanted}'"));
                }
                hits
            }
        };
        let connected = self
            .net
            .peer_statuses()
            .into_iter()
            .filter(|s| s.connected)
            .map(|s| s.id)
            .collect();
        Ok((targets, connected))
    }

    /// A folder: one row per device, files sent one after another (each opened only when its turn comes), a failing
    /// file does not stop the others, a cancel stops it between (or inside) files.
    async fn send_folder(
        self: &Arc<Self>,
        path: PathBuf,
        to: Option<String>,
    ) -> Vec<Result<TransferInfo, String>> {
        let root = path.clone();
        let walked = match tokio::task::spawn_blocking(move || crate::folders::walk(&root)).await {
            Ok(Ok(w)) => Arc::new(w),
            Ok(Err(why)) => return vec![Err(format!("cannot send {}: {why}", path.display()))],
            Err(e) => return vec![Err(e.to_string())],
        };
        self.start_folder_job(walked, to).await
    }

    /// A folder somebody else lists (the phone, from a document tree): the files are opened one at a time, when their
    /// turn comes, by `opener(index)`. Same rows, limits, stop rules and resume as a folder of this computer.
    pub async fn send_folder_entries(
        self: &Arc<Self>,
        top: String,
        entries: Vec<crate::folders::ExternalEntry>,
        opener: crate::folders::Opener,
        to: Option<String>,
    ) -> Vec<Result<TransferInfo, String>> {
        match crate::folders::from_entries(&top, entries, opener) {
            Ok(walked) => self.start_folder_job(Arc::new(walked), to).await,
            Err(why) => vec![Err(format!("cannot send the folder: {why}"))],
        }
    }

    async fn start_folder_job(
        self: &Arc<Self>,
        walked: Arc<crate::folders::Walked>,
        to: Option<String>,
    ) -> Vec<Result<TransferInfo, String>> {
        let (targets, connected) = match self.select_targets(&to) {
            Ok(selected) => selected,
            Err(why) => return vec![Err(why)],
        };
        let mut answers = Vec::new();
        for peer in targets {
            if !connected.contains(&peer.id) {
                answers.push(Err(format!("{} is not connected right now", peer.name)));
                continue;
            }
            let mut id = [0u8; 16];
            rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut id);
            let mut token = self.transfers.begin(
                &peer.name,
                TransferDirection::Sending,
                id,
                &walked.top,
                walked.total,
                TransferState::Waiting,
            );
            self.transfers.tag_node(id, node_of(&peer.id));
            let (me, lock, walked) = (
                Arc::clone(self),
                self.send_lock(node_of(&peer.id)),
                Arc::clone(&walked),
            );
            let (peer_id, node_peer) = (peer.id, peer.name.clone());
            tokio::spawn(async move {
                let _turn = tokio::select! {
                    turn = lock.lock() => turn,
                    _ = token.cancelled() => {
                        me.transfers.finish(id, TransferState::Cancelled, Some("cancelled".into()), None);
                        return;
                    }
                };
                me.run_folder(peer_id, &node_peer, id, &walked, &mut token)
                    .await;
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

    async fn run_folder(
        &self,
        peer_id: EndpointId,
        node_peer: &str,
        id: [u8; 16],
        walked: &crate::folders::Walked,
        token: &mut crate::files::CancelToken,
    ) {
        let mut batch = [0u8; 16];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut batch);
        let (mut failed, mut sent, mut first_error, mut base) =
            (0usize, 0usize, None::<String>, 0u64);
        for entry in &walked.entries {
            if token.is_cancelled() {
                self.transfers
                    .finish(id, TransferState::Cancelled, Some("cancelled".into()), None);
                return;
            }
            let place = klip_net::file::FolderPlace {
                batch,
                top: walked.top.clone(),
                rel: entry.rel.clone(),
            };
            let outcome = self
                .send_one_of_folder(peer_id, id, entry, &place, base, token)
                .await;
            match outcome {
                Ok(()) => {
                    base += entry.size;
                    sent += 1;
                }
                Err(FolderFail::Stop(why)) => {
                    let total = walked.entries.len();
                    self.transfers.finish(
                        id,
                        TransferState::Failed,
                        Some(format!(
                            "{node_peer}: stopped after {sent} of {total} files: {why}"
                        )),
                        None,
                    );
                    return;
                }
                Err(FolderFail::File(FileError::Cancelled)) => {
                    self.transfers.finish(
                        id,
                        TransferState::Cancelled,
                        Some("cancelled".into()),
                        None,
                    );
                    return;
                }
                Err(FolderFail::File(e)) => {
                    failed += 1;
                    base += entry.size; // keep the bar honest: this file's share is accounted for
                    self.transfers.progress(id, base);
                    first_error.get_or_insert_with(|| e.to_string());
                }
            }
        }
        if failed == 0 {
            self.transfers.finish(id, TransferState::Done, None, None);
        } else {
            let total = walked.entries.len();
            let why = first_error.unwrap_or_default();
            self.transfers.finish(
                id,
                TransferState::Failed,
                Some(format!("{failed} of {total} files failed: {why}")),
                None,
            );
        }
    }

    /// One file of a folder, with the same retry from zero as a single file. Needs a version 2 device.
    async fn send_one_of_folder(
        &self,
        peer_id: EndpointId,
        row: [u8; 16],
        entry: &crate::folders::FolderEntry,
        place: &klip_net::file::FolderPlace,
        base: u64,
        token: &mut crate::files::CancelToken,
    ) -> Result<(), FolderFail> {
        let prepared = match &entry.open {
            // The phone's way: the owner of the file hands over an open descriptor when asked.
            Some(opener) => {
                let (opener, index) = (Arc::clone(opener), entry.index);
                let name = entry.name.clone();
                tokio::task::spawn_blocking(move || {
                    opener(index).and_then(|file| PreparedFile::from_file(file, &name, None))
                })
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()))
                .map_err(|e| format!("cannot read {}: {e}", entry.name))
            }
            None => prepare_send(&entry.path)
                .await
                .map_err(|e| format!("cannot read {}: {e}", entry.path.display())),
        };
        let mut prepared = prepared.map_err(|e| FolderFail::File(FileError::Io(e)))?;
        let mut fresh_key = false;
        let mut busy_tries = 0;
        loop {
            // The device is gone or too old: every other file would fail the same way (after a long wait).
            let mut streams = self
                .net
                .open_file_stream(peer_id)
                .await
                .map_err(|e| FolderFail::Stop(e.to_string()))?;
            if streams.version() < 2 {
                streams.close();
                return Err(FolderFail::Stop(
                    "the other device cannot receive a folder: update Klip on it".into(),
                ));
            }
            let (result, resumed) = send_attempt(
                &mut prepared,
                row,
                2,
                fresh_key,
                Some(place),
                base,
                &mut streams.tx,
                &mut streams.rx,
                token,
                &self.transfers,
            )
            .await;
            if matches!(result, Err(FileError::Cancelled)) && !self.transfers.is_stopping() {
                streams.cancel().await;
            } else {
                streams.close();
            }
            if retry_from_zero(resumed, fresh_key, &result) {
                fresh_key = true;
                continue;
            }
            match result {
                // The device as a whole says no: asking again for the other files only wastes time.
                Err(FileError::Rejected {
                    code: RejectCode::Disabled | RejectCode::Paused,
                    text,
                }) => return Err(FolderFail::Stop(text)),
                // Busy is usually the receiver still holding the part of a transfer that just broke off.
                Err(FileError::Rejected {
                    code: RejectCode::Busy,
                    ..
                }) if busy_tries < 4 => {
                    busy_tries += 1;
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(3)) => {}
                        _ = token.cancelled() => return Err(FolderFail::File(FileError::Cancelled)),
                    }
                    continue;
                }
                other => return other.map_err(FolderFail::File),
            }
        }
    }

    async fn send_to_targets(
        self: &Arc<Self>,
        source: Source,
        to: Option<String>,
    ) -> Vec<Result<TransferInfo, String>> {
        if let Source::Path(path) = &source {
            if tokio::fs::metadata(path).await.is_ok_and(|m| m.is_dir()) {
                return self.send_folder(path.clone(), to).await;
            }
        }
        let (targets, connected) = match self.select_targets(&to) {
            Ok(selected) => selected,
            Err(why) => return vec![Err(why)],
        };
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
            let mut prepared = match source.prepare().await {
                Ok(p) => p,
                Err(e) => {
                    answers.push(Err(e));
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
            self.transfers.tag_node(id, node_of(&peer.id));
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
                // A resume that ends "damaged" (the kept partial was not the same bytes any more) is tried once more
                // from zero with a key nobody has seen.
                let mut fresh_key = false;
                loop {
                    match me.net.open_file_stream(peer_id).await {
                        Ok(mut streams) => {
                            let (result, resumed) = send_attempt(
                                &mut prepared,
                                id,
                                streams.version(),
                                fresh_key,
                                None,
                                0,
                                &mut streams.tx,
                                &mut streams.rx,
                                &mut token,
                                &me.transfers,
                            )
                            .await;
                            // A cancel by the person is told to the receiver (so it deletes the part); the daemon
                            // stopping is not a decision and stays a cut.
                            if matches!(result, Err(FileError::Cancelled))
                                && !me.transfers.is_stopping()
                            {
                                streams.cancel().await;
                            } else {
                                streams.close();
                            }
                            if retry_from_zero(resumed, fresh_key, &result) {
                                fresh_key = true;
                                me.transfers.progress(id, 0);
                                continue;
                            }
                            finish_send(&me.transfers, id, &result);
                        }
                        Err(e) => me.transfers.finish(
                            id,
                            TransferState::Failed,
                            Some(format!("{node_peer}: {e}")),
                            None,
                        ),
                    }
                    break;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_damaged_verdict_after_a_resume_sends_the_file_again_from_the_start() {
        let damaged = Err(FileError::Peer(klip_net::file::DAMAGED.to_string()));
        assert!(retry_from_zero(5, false, &damaged));
        assert!(!retry_from_zero(0, false, &damaged), "nothing was resumed");
        assert!(!retry_from_zero(5, true, &damaged), "only once");
        for other in [
            FileError::Peer("the transfer was cancelled".into()),
            FileError::Peer("the file could not be saved".into()),
            FileError::Peer("the transfer was too slow".into()),
            FileError::Timeout,
            FileError::Cancelled,
        ] {
            assert!(!retry_from_zero(5, false, &Err(other)));
        }
        assert!(!retry_from_zero(5, false, &Ok(())));
    }
}
