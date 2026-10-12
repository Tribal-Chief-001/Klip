use crate::config::{DaemonConfig, KlipPaths, RelayMode};
use crate::history::HistoryBuffer;
use klip_crypto::identity::{DeviceIdentity, KeyStore, PairedPeer};
use klip_diff::entropy::analyze_snippet;
use klip_net::{ClipOutcome, EndpointId, KlipNode, NetEvent, PairDecision, PeerRecord};
use klip_platform::driver::ClipboardDriver;
use klip_proto::clip::{ClipContentType, ClipPayload};
use klip_proto::hlc::{HybridLogicalClock, NodeId};
use klip_proto::ipc::{PairPollInfo, PeerStatusInfo, PendingPairInfo};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Remote clocks further ahead than this are refused (see `HybridLogicalClock::recv_event_bounded`).
const MAX_CLOCK_DRIFT_MS: u64 = 5 * 60 * 1000;

pub fn node_of(id: &EndpointId) -> NodeId {
    NodeId::new(*id.as_bytes())
}

struct PendingPair {
    peer: NodeId,
    device_name: String,
    respond: PairDecision,
}

#[derive(Default)]
struct PairingUi {
    pending: Option<PendingPair>,
    paired_name: Option<String>,
    expires_at: Option<Instant>,
}

/// Outcome of sending one clip to one peer.
pub struct DeliveryReport {
    pub peer_name: String,
    pub result: Result<(), String>,
}

pub struct SyncCoordinator {
    pub config: DaemonConfig,
    pub paths: KlipPaths,
    pub identity: Arc<DeviceIdentity>,
    pub keystore: Arc<tokio::sync::Mutex<KeyStore>>,
    pub driver: Arc<dyn ClipboardDriver>,
    pub net: KlipNode,
    pub history: HistoryBuffer,
    clock: Mutex<HybridLogicalClock>,
    last_applied_clip_hlc: Mutex<Option<HybridLogicalClock>>,
    pairing: Mutex<PairingUi>,
    last_errors: Mutex<HashMap<NodeId, String>>,
    is_paused: AtomicBool,
    pause_until_secs: AtomicU64,
    pub total_synced_clips: AtomicU64,
    pub last_sync_instant: Mutex<Option<Instant>>,
}

impl SyncCoordinator {
    pub fn new(
        config: DaemonConfig,
        paths: KlipPaths,
        identity: Arc<DeviceIdentity>,
        keystore: Arc<tokio::sync::Mutex<KeyStore>>,
        driver: Arc<dyn ClipboardDriver>,
        net: KlipNode,
    ) -> Self {
        let history = HistoryBuffer::new(config.max_history_items);
        let clock = Mutex::new(HybridLogicalClock::new(identity.node_id));
        Self {
            config,
            paths,
            identity,
            keystore,
            driver,
            net,
            history,
            clock,
            last_applied_clip_hlc: Mutex::new(None),
            pairing: Mutex::new(PairingUi::default()),
            last_errors: Mutex::new(HashMap::new()),
            is_paused: AtomicBool::new(false),
            pause_until_secs: AtomicU64::new(0),
            total_synced_clips: AtomicU64::new(0),
            last_sync_instant: Mutex::new(None),
        }
    }

    // ------------------------------------------------------------------------------ pause

    pub fn pause(&self, duration_secs: u64) {
        self.pause_until_secs
            .store(now_secs().saturating_add(duration_secs), Ordering::SeqCst);
        self.is_paused.store(true, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.is_paused.store(false, Ordering::SeqCst);
        self.pause_until_secs.store(0, Ordering::SeqCst);
    }

    pub fn check_pause_status(&self) -> bool {
        if !self.is_paused.load(Ordering::SeqCst) {
            return false;
        }
        let until = self.pause_until_secs.load(Ordering::SeqCst);
        if until > 0 && now_secs() >= until {
            self.resume();
            false
        } else {
            true
        }
    }

    pub fn pause_remaining_secs(&self) -> u64 {
        if self.check_pause_status() {
            self.pause_until_secs
                .load(Ordering::SeqCst)
                .saturating_sub(now_secs())
        } else {
            0
        }
    }

    // ----------------------------------------------------------------------- local -> peers

    /// Record a locally copied item and send it to all peers in the background, so a slow or
    /// unreachable peer can never stall clipboard polling or inbound processing.
    pub fn spawn_local_clip(self: &Arc<Self>, content_type: ClipContentType, data: Vec<u8>) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            this.broadcast_local_clip(content_type, data).await;
        });
    }

    /// Prepare and send a local clip, waiting for each peer's answer. `None` when ignored
    /// (paused / empty).
    pub async fn broadcast_local_clip(
        &self,
        content_type: ClipContentType,
        data: Vec<u8>,
    ) -> Option<Vec<DeliveryReport>> {
        let payload = self.prepare_local_clip(content_type, data)?;
        let results = self.net.send_clip(&payload).await;
        let names: HashMap<NodeId, String> = self
            .net
            .peers()
            .into_iter()
            .map(|p| (node_of(&p.id), p.name))
            .collect();

        let mut errors = self.last_errors.lock().unwrap();
        let mut reports = Vec::new();
        for (id, res) in results {
            let node = node_of(&id);
            let peer_name = names.get(&node).cloned().unwrap_or_else(|| node.short_id());
            match res {
                Ok(()) => {
                    errors.remove(&node);
                    reports.push(DeliveryReport {
                        peer_name,
                        result: Ok(()),
                    });
                }
                Err(e) => {
                    let msg = e.to_string();
                    errors.insert(node, msg.clone());
                    reports.push(DeliveryReport {
                        peer_name,
                        result: Err(msg),
                    });
                }
            }
        }
        drop(errors);
        // "Last sync" means a peer really accepted something, not that we merely saw a local copy.
        if reports.iter().any(|r| r.result.is_ok()) {
            self.record_sync();
        }
        Some(reports)
    }

    fn prepare_local_clip(
        &self,
        content_type: ClipContentType,
        data: Vec<u8>,
    ) -> Option<ClipPayload> {
        if self.check_pause_status() || data.is_empty() {
            return None;
        }

        // Compress uncompressed bitmaps; keep the label in sync with the bytes we actually send.
        let (content_type, data) = match content_type {
            ClipContentType::ImagePng | ClipContentType::ImageWebP => {
                match klip_diff::transcode_image_for_sync(&data) {
                    Ok((klip_diff::ImageFormat::WebP, bytes)) => {
                        (ClipContentType::ImageWebP, bytes)
                    }
                    Ok((klip_diff::ImageFormat::Png, bytes)) => (ClipContentType::ImagePng, bytes),
                    _ => (content_type, data),
                }
            }
            other => (other, data),
        };

        let is_sensitive = match (&content_type, std::str::from_utf8(&data)) {
            (ClipContentType::PlainTextUtf8, Ok(text)) => analyze_snippet(text).is_likely_secret,
            _ => false,
        };

        let hlc = {
            let mut clock = self.clock.lock().unwrap();
            let stamp = clock.send_event();
            *self.last_applied_clip_hlc.lock().unwrap() = Some(stamp);
            stamp
        };

        let payload =
            ClipPayload::new(content_type, data, self.identity.node_id, hlc, is_sensitive);
        self.history.push(
            payload.id,
            self.identity.node_id,
            self.config.device_name.clone(),
            payload.content.clone(),
            is_sensitive,
        );
        Some(payload)
    }

    // ----------------------------------------------------------------------- peers -> local

    /// Handle one event from the network layer.
    pub async fn handle_net_event(&self, event: NetEvent) {
        match event {
            NetEvent::Clip { from, payload, ack } => {
                let outcome = self.apply_remote_clip(&from, payload);
                let _ = ack.send(outcome);
            }
            NetEvent::PairRequest {
                peer,
                device_name,
                respond,
            } => {
                let mut ui = self.pairing.lock().unwrap();
                if ui.pending.is_some() {
                    // One decision at a time; a second request is declined, not queued.
                    let _ = respond.send(false);
                } else {
                    ui.pending = Some(PendingPair {
                        peer: node_of(&peer),
                        device_name,
                        respond,
                    });
                }
            }
            NetEvent::Paired(rec) => {
                self.persist_peer(&rec).await;
                self.pairing.lock().unwrap().paired_name = Some(rec.name);
            }
            NetEvent::PeerConnected(_) | NetEvent::PeerDisconnected(_) => {}
        }
    }

    fn apply_remote_clip(&self, from: &EndpointId, payload: ClipPayload) -> ClipOutcome {
        if self.check_pause_status() {
            return ClipOutcome::Paused;
        }
        let sender = node_of(from);
        // The transport already guarantees origin == sender; the clock owner must match too.
        if payload.hlc.node_id != sender || payload.origin_node != sender {
            return ClipOutcome::Rejected;
        }
        let remote_hlc = payload.hlc;
        {
            let mut clock = self.clock.lock().unwrap();
            if clock
                .recv_event_bounded(&remote_hlc, MAX_CLOCK_DRIFT_MS)
                .is_err()
            {
                return ClipOutcome::Rejected; // absurd timestamp: keep our clock sane
            }
        }
        if let Some(current) = *self.last_applied_clip_hlc.lock().unwrap() {
            if remote_hlc <= current {
                return ClipOutcome::Superseded; // a newer clip is already active (LWW)
            }
        }

        if self
            .driver
            .write_clip(&payload.content_type, &payload.content)
            .is_err()
        {
            // Not applied, so the "active clip" bookkeeping must not claim it was.
            return ClipOutcome::ClipboardUnavailable;
        }
        {
            let mut active = self.last_applied_clip_hlc.lock().unwrap();
            if active.map(|cur| remote_hlc > cur).unwrap_or(true) {
                *active = Some(remote_hlc);
            }
        }
        let name = self
            .net
            .peers()
            .into_iter()
            .find(|p| p.id == *from)
            .map(|p| p.name)
            .unwrap_or_else(|| sender.short_id());
        self.history.push(
            payload.id,
            sender,
            name,
            payload.content,
            payload.is_sensitive,
        );
        self.record_sync();
        ClipOutcome::Applied
    }

    fn record_sync(&self) {
        self.total_synced_clips.fetch_add(1, Ordering::SeqCst);
        *self.last_sync_instant.lock().unwrap() = Some(Instant::now());
    }

    // ---------------------------------------------------------------------------- pairing

    pub async fn persist_peer(&self, rec: &PeerRecord) {
        let mut ks = self.keystore.lock().await;
        ks.add_peer(PairedPeer::new(node_of(&rec.id), rec.name.clone()));
        let _ = ks.save(self.paths.keystore());
    }

    pub fn mark_pairing_started(&self, ttl_secs: u64) {
        let mut ui = self.pairing.lock().unwrap();
        ui.expires_at = Some(Instant::now() + std::time::Duration::from_secs(ttl_secs));
        ui.paired_name = None;
        if let Some(old) = ui.pending.take() {
            let _ = old.respond.send(false);
        }
    }

    pub fn cancel_pairing(&self) {
        self.net.close_pairing();
        let mut ui = self.pairing.lock().unwrap();
        ui.expires_at = None;
        if let Some(old) = ui.pending.take() {
            let _ = old.respond.send(false);
        }
    }

    pub fn poll_pairing(&self) -> PairPollInfo {
        let mut ui = self.pairing.lock().unwrap();
        let pending = ui.pending.as_ref().map(|p| PendingPairInfo {
            device_name: p.device_name.clone(),
            fingerprint: p.peer.fingerprint(),
        });
        let seconds_left = ui
            .expires_at
            .map(|t| t.saturating_duration_since(Instant::now()).as_secs())
            .unwrap_or(0);
        PairPollInfo {
            window_open: self.net.pairing_open() || pending.is_some(),
            seconds_left,
            pending,
            paired: ui.paired_name.take(),
        }
    }

    /// Answer the pending pairing request. Returns false if none was pending.
    pub fn decide_pairing(&self, accept: bool) -> bool {
        match self.pairing.lock().unwrap().pending.take() {
            Some(p) => {
                let _ = p.respond.send(accept);
                true
            }
            None => false,
        }
    }

    /// Remove a paired device selected by exact name or fingerprint prefix.
    pub async fn unpair(&self, selector: &str) -> Result<String, String> {
        let wanted = selector.trim();
        if wanted.is_empty() {
            return Err("tell me which device to remove".into());
        }
        let norm = |s: &str| s.replace('-', "").to_uppercase();
        let matches: Vec<PeerRecord> = self
            .net
            .peers()
            .into_iter()
            .filter(|p| {
                p.name == wanted
                    || (norm(wanted).len() >= 4
                        && norm(&node_of(&p.id).fingerprint()).starts_with(&norm(wanted)))
            })
            .collect();
        match matches.as_slice() {
            [] => Err(format!("no paired device matches '{wanted}'")),
            [one] => {
                self.net.unpair(&one.id);
                let mut ks = self.keystore.lock().await;
                ks.remove_peer(&node_of(&one.id));
                let _ = ks.save(self.paths.keystore());
                Ok(one.name.clone())
            }
            many => Err(format!(
                "'{wanted}' is ambiguous ({} devices); use more of the fingerprint",
                many.len()
            )),
        }
    }

    // ----------------------------------------------------------------------------- status

    pub fn peer_status(&self) -> Vec<PeerStatusInfo> {
        let errors = self.last_errors.lock().unwrap();
        self.net
            .peer_statuses()
            .into_iter()
            .map(|p| {
                let node = node_of(&p.id);
                PeerStatusInfo {
                    node_id: node,
                    name: p.name,
                    is_online: p.connected,
                    last_error: errors.get(&node).cloned(),
                }
            })
            .collect()
    }

    pub fn relay_mode_label(&self) -> &'static str {
        match self.config.relay {
            RelayMode::N0 => "n0 public relays + discovery",
            RelayMode::Lan => "LAN only (no third-party servers)",
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
