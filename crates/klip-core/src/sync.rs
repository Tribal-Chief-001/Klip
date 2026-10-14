use crate::config::{DaemonConfig, KlipPaths, RelayMode};
use crate::history::HistoryBuffer;
use klip_crypto::identity::{DeviceIdentity, KeyStore, PairedPeer};
use klip_diff::entropy::analyze_snippet;
use klip_net::{ClipOutcome, EndpointId, KlipNode, NetError, NetEvent, PairDecision, PeerRecord};
use klip_platform::driver::ClipboardDriver;
use klip_proto::clip::{ClipContentType, ClipPayload};
use klip_proto::hlc::{HybridLogicalClock, NodeId};
use klip_proto::ipc::{
    DaemonStatusInfo, ImageStateInfo, PairPollInfo, PeerStatusInfo, PendingPairInfo,
    WaitingImageInfo,
};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Remote clocks further ahead than this are refused (see `HybridLogicalClock::recv_event_bounded`).
const MAX_CLOCK_DRIFT_MS: u64 = 5 * 60 * 1000;

pub fn node_of(id: &EndpointId) -> NodeId {
    NodeId::new(*id.as_bytes())
}

/// Transcode (images), classify (secrets), wrap and record in history. Pure CPU work.
fn build_payload(
    content_type: ClipContentType,
    data: Vec<u8>,
    hlc: HybridLogicalClock,
    node: NodeId,
    device_name: String,
    history: &HistoryBuffer,
) -> ClipPayload {
    // Compress uncompressed bitmaps; keep the label in sync with the bytes we actually send.
    let (content_type, data) = match content_type {
        ClipContentType::ImagePng | ClipContentType::ImageWebP => {
            match klip_diff::transcode_image_for_sync(&data) {
                Ok((klip_diff::ImageFormat::WebP, bytes)) => (ClipContentType::ImageWebP, bytes),
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

    let payload = ClipPayload::new(content_type, data, node, hlc, is_sensitive);
    history.push(
        payload.id,
        node,
        device_name,
        payload.content.clone(),
        is_sensitive,
    );
    payload
}

/// Called, outside any lock, when an automatic copy could not reach a device: its name and how long
/// it has been gone. Only the name and a duration: never clip content.
pub type UndeliveredHook = Arc<dyn Fn(&str, Duration) + Send + Sync>;

/// "5 min" / "2 h" for how long a device has been gone; `None` under a minute.
pub fn offline_label(gone: Duration) -> Option<String> {
    let mins = gone.as_secs() / 60;
    match mins {
        0 => None,
        1..=119 => Some(format!("{mins} min")),
        _ => Some(format!("{} h", mins / 60)),
    }
}

fn image_format_of(t: &ClipContentType) -> Option<klip_diff::ImageFormat> {
    match t {
        ClipContentType::ImagePng => Some(klip_diff::ImageFormat::Png),
        ClipContentType::ImageWebP => Some(klip_diff::ImageFormat::WebP),
        _ => None,
    }
}

fn is_unreachable(e: &NetError) -> bool {
    matches!(e, NetError::Connect(_) | NetError::Stream(_))
}

/// One alert per outage: true for the first unreachable failure since the last success (or ever).
fn first_failure_of_outage(
    alerted: &mut HashSet<NodeId>,
    node: NodeId,
    result: &Result<(), NetError>,
) -> bool {
    match result {
        Ok(()) => {
            alerted.remove(&node);
            false
        }
        Err(e) if is_unreachable(e) => alerted.insert(node),
        Err(_) => false,
    }
}

/// How long a copied image waits to be sent on purpose.
const PENDING_IMAGE_TTL: Duration = Duration::from_secs(10 * 60);
/// Longest side of a preview picture.
const THUMBNAIL_SIDE: u32 = 240;

/// A copied image waiting for the person to send it (see `DaemonConfig::auto_send_images`).
struct PendingImage {
    content_type: ClipContentType,
    data: Vec<u8>,
    width: u32,
    height: u32,
    since: Instant,
    thumb: Option<Vec<u8>>,
}

/// The one held image, with its expiry rules. Pure (the clock is a parameter) so it can be tested.
#[derive(Default)]
struct PendingSlot {
    image: Option<PendingImage>,
}

impl PendingSlot {
    /// A newer image replaces the older one.
    fn hold(&mut self, image: PendingImage) {
        self.image = Some(image);
    }

    fn expire(&mut self, now: Instant) {
        if let Some(i) = &self.image {
            if now.saturating_duration_since(i.since) >= PENDING_IMAGE_TTL {
                self.image = None;
            }
        }
    }

    fn fresh(&mut self, now: Instant) -> Option<&PendingImage> {
        self.expire(now);
        self.image.as_ref()
    }

    fn take(&mut self, now: Instant) -> Option<PendingImage> {
        self.expire(now);
        self.image.take()
    }

    /// Put back an image whose send did not reach anybody, unless a newer one was copied meanwhile.
    fn restore(&mut self, image: PendingImage) {
        if self.image.is_none() {
            self.image = Some(image);
        }
    }
}

/// `(width, height)` of an image that may be held: PNG or WebP, really that format, within the pixel limit.
fn held_image_info(content_type: &ClipContentType, data: &[u8]) -> Result<(u32, u32), String> {
    match image_format_of(content_type) {
        Some(format) => klip_diff::validate_image(format, data),
        None => Err("only PNG and WebP images can be held".to_string()),
    }
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
    /// Newest clip waiting to be sent. Only the latest matters (last write wins), so a burst of
    /// copies collapses to "the clip being sent now" plus "the newest one after it".
    outbox: Mutex<Option<ClipPayload>>,
    /// True while a drain task owns the outbox.
    sending: AtomicBool,
    /// How many network sends were started (a send goes to all peers).
    pub sends_started: AtomicU64,
    /// Local clips that were never sent because a newer one replaced them.
    pub coalesced_clips: AtomicU64,
    undelivered_hook: Mutex<Option<UndeliveredHook>>,
    /// Copied images are sent at once (`true`) or held until sent on purpose (`false`).
    auto_images: AtomicBool,
    pending_image: Mutex<PendingSlot>,
    /// Devices already alerted about in their current outage.
    alerted: Mutex<HashSet<NodeId>>,
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
        let history = HistoryBuffer::with_max_age(
            config.max_history_items,
            config.history_ttl_hours.saturating_mul(3_600_000),
        );
        let auto_images = config.auto_send_images;
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
            outbox: Mutex::new(None),
            sending: AtomicBool::new(false),
            sends_started: AtomicU64::new(0),
            coalesced_clips: AtomicU64::new(0),
            undelivered_hook: Mutex::new(None),
            auto_images: AtomicBool::new(auto_images),
            pending_image: Mutex::new(PendingSlot::default()),
            alerted: Mutex::new(HashSet::new()),
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

    // ------------------------------------------------------------------ held images

    pub fn auto_images(&self) -> bool {
        self.auto_images.load(Ordering::SeqCst)
    }

    /// Memory only; the daemon also saves `config.json`.
    pub fn set_auto_images(&self, on: bool) {
        self.auto_images.store(on, Ordering::SeqCst);
    }

    /// Keep a copied image until it is sent on purpose; the newest one wins. Not in the history yet.
    pub fn hold_image(&self, content_type: ClipContentType, data: Vec<u8>) -> Result<(), String> {
        let (width, height) = held_image_info(&content_type, &data)?;
        self.pending_image.lock().unwrap().hold(PendingImage {
            content_type,
            data,
            width,
            height,
            since: Instant::now(),
            thumb: None,
        });
        Ok(())
    }

    pub fn image_state(&self) -> ImageStateInfo {
        let now = Instant::now();
        let mut slot = self.pending_image.lock().unwrap();
        ImageStateInfo {
            auto: self.auto_images(),
            waiting: slot.fresh(now).map(|i| WaitingImageInfo {
                size_bytes: i.data.len(),
                width: i.width,
                height: i.height,
                age_secs: now.saturating_duration_since(i.since).as_secs(),
                kind: match i.content_type {
                    ClipContentType::ImageWebP => "webp",
                    _ => "png",
                }
                .to_string(),
            }),
        }
    }

    /// Send the held image to every device and report per device. The image stays held when nobody
    /// got it (no device, all unreachable, sync paused), so the person can try again.
    pub async fn send_pending_image(&self) -> Result<Vec<DeliveryReport>, String> {
        let image = self
            .pending_image
            .lock()
            .unwrap()
            .take(Instant::now())
            .ok_or_else(|| {
                "no image is waiting to be sent (an image you copy waits for 10 minutes)"
                    .to_string()
            })?;
        let sent = self
            .broadcast_local_clip(image.content_type.clone(), image.data.clone())
            .await;
        match sent {
            None => {
                self.pending_image.lock().unwrap().restore(image);
                Err("sync is paused".to_string())
            }
            Some(reports) => {
                if !reports.iter().any(|r| r.result.is_ok()) {
                    self.pending_image.lock().unwrap().restore(image);
                }
                Ok(reports)
            }
        }
    }

    /// A small PNG of the held image (`None`) or of a stored history image. Blocking (decodes the
    /// image): call it off the async workers.
    pub fn thumbnail(&self, id: Option<[u8; 16]>) -> Result<Vec<u8>, String> {
        match id {
            Some(id) => {
                let item = self
                    .history
                    .get_by_id(id)
                    .ok_or_else(|| "that clip is no longer in the history".to_string())?;
                if item.full_content.is_empty() {
                    return Err("that clip was too large to keep in the history".to_string());
                }
                klip_diff::thumbnail_png(&item.full_content, THUMBNAIL_SIDE)
            }
            None => {
                let (data, since) = {
                    let mut slot = self.pending_image.lock().unwrap();
                    let image = slot
                        .fresh(Instant::now())
                        .ok_or_else(|| "no image is waiting".to_string())?;
                    if let Some(t) = &image.thumb {
                        return Ok(t.clone());
                    }
                    (image.data.clone(), image.since)
                };
                let thumb = klip_diff::thumbnail_png(&data, THUMBNAIL_SIDE)?;
                let mut slot = self.pending_image.lock().unwrap();
                if let Some(image) = slot.image.as_mut().filter(|i| i.since == since) {
                    image.thumb = Some(thumb.clone());
                }
                Ok(thumb)
            }
        }
    }

    pub fn set_undelivered_hook(&self, hook: UndeliveredHook) {
        *self.undelivered_hook.lock().unwrap() = Some(hook);
    }

    /// True while a send of an automatic copy is in flight.
    pub fn is_sending(&self) -> bool {
        self.sending.load(Ordering::SeqCst)
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
    /// unreachable peer can never stall clipboard capture or inbound processing. Copies that
    /// arrive while a send is in flight are coalesced: only the newest is sent next.
    pub fn spawn_local_clip(self: &Arc<Self>, content_type: ClipContentType, data: Vec<u8>) {
        if self.check_pause_status() || data.is_empty() {
            return;
        }
        // Stamp now, in copy order; the heavy preparation below may finish out of order.
        let hlc = self.stamp_local_clip();
        let this = Arc::clone(self);
        tokio::spawn(async move {
            if let Some(payload) = this.prepare_off_thread(content_type, data, hlc).await {
                this.queue_for_sending(payload);
            }
        });
    }

    /// A copy captured from the OS clipboard (desktop): an image waits to be sent on purpose unless
    /// automatic image sending is on; everything else is sent at once.
    pub fn on_local_capture(self: &Arc<Self>, content_type: ClipContentType, data: Vec<u8>) {
        if image_format_of(&content_type).is_some() && !self.auto_images() {
            if let Err(why) = self.hold_image(content_type, data) {
                eprintln!("klip: an image was not held: {why}");
            }
            return;
        }
        self.spawn_local_clip(content_type, data);
    }

    /// Prepare and send a local clip, waiting for each peer's answer. `None` when ignored
    /// (paused / empty). Used for explicit sends (`klip send`), which are never coalesced.
    pub async fn broadcast_local_clip(
        &self,
        content_type: ClipContentType,
        data: Vec<u8>,
    ) -> Option<Vec<DeliveryReport>> {
        if self.check_pause_status() || data.is_empty() {
            return None;
        }
        let hlc = self.stamp_local_clip();
        let payload = self.prepare_off_thread(content_type, data, hlc).await?;
        Some(self.deliver(&payload, false).await)
    }

    fn stamp_local_clip(&self) -> HybridLogicalClock {
        let mut clock = self.clock.lock().unwrap();
        let stamp = clock.send_event();
        *self.last_applied_clip_hlc.lock().unwrap() = Some(stamp);
        stamp
    }

    /// Image transcoding and secret analysis are CPU work; keep them off the async workers.
    async fn prepare_off_thread(
        &self,
        content_type: ClipContentType,
        data: Vec<u8>,
        hlc: HybridLogicalClock,
    ) -> Option<ClipPayload> {
        let node = self.identity.node_id;
        let device = self.config.device_name.clone();
        let history = self.history.clone();
        tokio::task::spawn_blocking(move || {
            build_payload(content_type, data, hlc, node, device, &history)
        })
        .await
        .ok()
    }

    fn queue_for_sending(self: &Arc<Self>, payload: ClipPayload) {
        {
            let mut outbox = self.outbox.lock().unwrap();
            match outbox.as_ref() {
                // Preparation finished out of order: a newer clip is already waiting.
                Some(waiting) if waiting.hlc >= payload.hlc => {
                    self.coalesced_clips.fetch_add(1, Ordering::SeqCst);
                    return;
                }
                Some(_) => {
                    self.coalesced_clips.fetch_add(1, Ordering::SeqCst);
                }
                None => {}
            }
            *outbox = Some(payload);
        }
        if self.sending.swap(true, Ordering::SeqCst) {
            return; // the running drain task will pick it up
        }
        let this = Arc::clone(self);
        tokio::spawn(async move { this.drain_outbox().await });
    }

    async fn drain_outbox(self: Arc<Self>) {
        loop {
            let next = self.outbox.lock().unwrap().take();
            let Some(payload) = next else {
                self.sending.store(false, Ordering::SeqCst);
                // A clip may have been queued between the take and the store.
                if self.outbox.lock().unwrap().is_some()
                    && !self.sending.swap(true, Ordering::SeqCst)
                {
                    continue;
                }
                return;
            };
            // A newer clip (local or remote) became active while this one waited: skip it.
            let stale = self
                .last_applied_clip_hlc
                .lock()
                .unwrap()
                .map(|active| active > payload.hlc)
                .unwrap_or(false);
            if stale {
                self.coalesced_clips.fetch_add(1, Ordering::SeqCst);
                continue;
            }
            self.sends_started.fetch_add(1, Ordering::SeqCst);
            self.deliver(&payload, true).await;
        }
    }

    /// Send one prepared clip to every peer and record the per-peer outcome. `notify` is true for
    /// automatic copies: an unreachable device then raises the undelivered hook (once per outage).
    async fn deliver(&self, payload: &ClipPayload, notify: bool) -> Vec<DeliveryReport> {
        let results = self.net.send_clip(payload).await;
        let names: HashMap<NodeId, String> = self
            .net
            .peers()
            .into_iter()
            .map(|p| (node_of(&p.id), p.name))
            .collect();
        let gone: HashMap<NodeId, Duration> = if notify {
            self.net
                .peer_statuses()
                .into_iter()
                .filter_map(|s| s.offline_for.map(|d| (node_of(&s.id), d)))
                .collect()
        } else {
            HashMap::new()
        };

        let mut to_alert: Vec<(String, Duration)> = Vec::new();
        let mut errors = self.last_errors.lock().unwrap();
        let mut reports = Vec::new();
        for (id, res) in results {
            let node = node_of(&id);
            let peer_name = names.get(&node).cloned().unwrap_or_else(|| node.short_id());
            {
                let mut alerted = self.alerted.lock().unwrap();
                if notify {
                    if first_failure_of_outage(&mut alerted, node, &res) {
                        let off = gone.get(&node).copied().unwrap_or_default();
                        to_alert.push((peer_name.clone(), off));
                    }
                } else if res.is_ok() {
                    alerted.remove(&node); // an explicit send that worked also ends the outage
                }
            }
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
        let hook = self.undelivered_hook.lock().unwrap().clone();
        if let Some(hook) = hook {
            for (name, off) in to_alert {
                hook(&name, off);
            }
        }
        reports
    }

    // ----------------------------------------------------------------------- peers -> local

    /// Handle one event from the network layer.
    pub async fn handle_net_event(&self, event: NetEvent) {
        match event {
            NetEvent::Clip { from, payload, ack } => {
                let outcome = self.apply_remote_clip(&from, payload).await;
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
                if let Err(e) = self.persist_peer(&rec).await {
                    eprintln!("klip: {e}");
                }
                self.pairing.lock().unwrap().paired_name = Some(rec.name);
            }
            NetEvent::PeerConnected(id) => {
                // The device is back: the next failure is a new outage and must alert again, even
                // if nobody copied anything in between.
                self.alerted.lock().unwrap().remove(&node_of(&id));
            }
            NetEvent::PeerDisconnected(_) => {}
        }
    }

    async fn apply_remote_clip(&self, from: &EndpointId, payload: ClipPayload) -> ClipOutcome {
        if self.check_pause_status() {
            return ClipOutcome::Paused;
        }
        let sender = node_of(from);
        // The transport already guarantees origin == sender; the clock owner must match too.
        if payload.hlc.node_id != sender || payload.origin_node != sender {
            return ClipOutcome::Rejected;
        }
        // An image from a paired device is still untrusted input: check its magic bytes and the size
        // in its header (no decoding) before anything is written or our clock moves.
        if let Some(format) = image_format_of(&payload.content_type) {
            if let Err(why) = klip_diff::validate_image(format, &payload.content) {
                eprintln!("klip: refused an image from {}: {why}", sender.short_id());
                return ClipOutcome::Rejected;
            }
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
        // A retry of a clip we already applied (the sender timed out waiting for our answer) is a
        // success for the sender, not "superseded".
        if self.history.get_by_id(payload.id).is_some() {
            return ClipOutcome::Applied;
        }
        if let Some(current) = *self.last_applied_clip_hlc.lock().unwrap() {
            if remote_hlc <= current {
                return ClipOutcome::Superseded; // a newer clip is already active (LWW)
            }
        }

        // The platform write can block (X11 selection handshake, JNI); keep it off the async
        // workers. The payload moves into the closure and comes back for the history entry.
        let driver = Arc::clone(&self.driver);
        let written = tokio::task::spawn_blocking(move || {
            let ok = driver
                .write_clip_flagged(
                    &payload.content_type,
                    &payload.content,
                    payload.is_sensitive,
                )
                .is_ok();
            (ok, payload)
        })
        .await;
        let payload = match written {
            Ok((true, payload)) => payload,
            // Not applied, so the "active clip" bookkeeping must not claim it was.
            _ => return ClipOutcome::ClipboardUnavailable,
        };
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

    /// Save a newly paired device. An error means it is trusted now but would be forgotten on restart.
    pub async fn persist_peer(&self, rec: &PeerRecord) -> Result<(), String> {
        let mut ks = self.keystore.lock().await;
        ks.add_peer(PairedPeer::new(node_of(&rec.id), rec.name.clone()));
        ks.save(self.paths.keystore()).map_err(|e| {
            format!(
                "{} is paired but the device list could not be saved: {e}",
                rec.name
            )
        })
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
                // Save first: if the file cannot be written the device would be trusted again after
                // a restart, so it must not be reported (or treated) as removed.
                let node = node_of(&one.id);
                let mut ks = self.keystore.lock().await;
                let old = ks.get_peer(&node).cloned();
                ks.remove_peer(&node);
                if let Err(e) = ks.save(self.paths.keystore()) {
                    if let Some(old) = old {
                        ks.add_peer(old);
                    }
                    return Err(format!(
                        "could not remove {}: the device list could not be saved ({e})",
                        one.name
                    ));
                }
                drop(ks);
                self.net.unpair(&one.id);
                self.alerted.lock().unwrap().remove(&node);
                Ok(one.name.clone())
            }
            many => Err(format!(
                "'{wanted}' is ambiguous ({} devices); use more of the fingerprint",
                many.len()
            )),
        }
    }

    // ----------------------------------------------------------------------------- status

    /// Remove every linked device. Continues after a failure and reports both lists
    /// (`removed` names, and `(name, reason)` for each failure).
    pub async fn unpair_all(&self) -> (Vec<String>, Vec<(String, String)>) {
        let mut removed = Vec::new();
        let mut failed = Vec::new();
        for peer in self.net.peers() {
            let selector = node_of(&peer.id).fingerprint();
            match self.unpair(&selector).await {
                Ok(name) => removed.push(name),
                Err(reason) => failed.push((peer.name, reason)),
            }
        }
        (removed, failed)
    }

    /// Put a history item back on this device's clipboard. It goes through the same driver call as a
    /// received clip, which records it in the echo shield, so it is not sent out again as a new copy.
    pub async fn copy_history_item(&self, id: [u8; 16]) -> Result<(), String> {
        let item = self
            .history
            .get_by_id(id)
            .ok_or_else(|| "that clip is no longer in the history".to_string())?;
        if item.full_content.is_empty() {
            return Err(
                "that clip was too large to keep in the history; copy it again on the device it came from"
                    .to_string(),
            );
        }
        let content_type = crate::history::sniff_content_type(&item.full_content);
        // The history keeps whatever arrived, including bytes a peer labelled as text; check before the
        // driver decodes them as an image.
        if let Some(format) = image_format_of(&content_type) {
            klip_diff::validate_image(format, &item.full_content)?;
        }
        let driver = Arc::clone(&self.driver);
        let sensitive = item.is_sensitive;
        let data = item.full_content;
        tokio::task::spawn_blocking(move || {
            driver
                .write_clip_flagged(&content_type, &data, sensitive)
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }

    pub fn clear_history(&self) {
        self.history.clear();
    }

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
                    last_error: errors.get(&node).cloned().or(p.problem),
                }
            })
            .collect()
    }

    /// Everything the UIs show on their status screen. The host supplies the values only it knows.
    pub fn status(&self, platform: String, memory_rss_bytes: u64) -> DaemonStatusInfo {
        let last_clip = self.history.last_item();
        let (last_clip_preview, last_clip_size_bytes) = match last_clip {
            Some(c) => (Some(c.content_preview), c.size_bytes),
            None => (None, 0),
        };
        DaemonStatusInfo {
            is_active: true,
            is_paused: self.check_pause_status(),
            paused_remaining_secs: self.pause_remaining_secs(),
            platform,
            memory_rss_bytes,
            node_id: self.identity.node_id,
            relay_mode: self.relay_mode_label().to_string(),
            relay_state: self.net.relay_state_label(),
            pairing_open: self.net.pairing_open(),
            peers: self.peer_status(),
            total_synced_clips: self.total_synced_clips.load(Ordering::SeqCst),
            last_sync_time_secs_ago: self
                .last_sync_instant
                .lock()
                .unwrap()
                .map(|t| t.elapsed().as_secs()),
            last_clip_preview,
            last_clip_size_bytes,
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(kind: ClipContentType, tag: u8, since: Instant) -> PendingImage {
        PendingImage {
            content_type: kind,
            data: vec![tag; 4],
            width: 10,
            height: 5,
            since,
            thumb: None,
        }
    }

    #[test]
    fn a_held_image_is_replaced_by_a_newer_one_and_expires_after_ten_minutes() {
        let now = Instant::now();
        let mut slot = PendingSlot::default();
        assert!(slot.fresh(now).is_none());
        slot.hold(pending(ClipContentType::ImagePng, 1, now));
        slot.hold(pending(ClipContentType::ImageWebP, 2, now));
        assert_eq!(
            slot.fresh(now).unwrap().data,
            vec![2; 4],
            "the newer image wins"
        );

        let nine = now + Duration::from_secs(9 * 60);
        assert!(slot.fresh(nine).is_some());
        let eleven = now + Duration::from_secs(11 * 60);
        assert!(slot.fresh(eleven).is_none(), "gone after ten minutes");
        assert!(
            slot.take(eleven).is_none(),
            "and it cannot be taken any more"
        );
    }

    #[test]
    fn taking_a_held_image_empties_the_slot_and_putting_it_back_keeps_its_age() {
        let t0 = Instant::now();
        let mut slot = PendingSlot::default();
        slot.hold(pending(ClipContentType::ImagePng, 3, t0));
        let image = slot.take(t0 + Duration::from_secs(60)).unwrap();
        assert!(slot.fresh(t0).is_none());
        slot.restore(image);
        let again = slot.fresh(t0 + Duration::from_secs(120)).unwrap();
        assert_eq!(again.since, t0, "the original capture time is kept");
        // A newer image copied meanwhile must not be overwritten by the restored older one.
        let newer = pending(ClipContentType::ImagePng, 9, t0 + Duration::from_secs(130));
        let mut slot = PendingSlot::default();
        slot.hold(newer);
        slot.restore(pending(ClipContentType::ImagePng, 3, t0));
        assert_eq!(
            slot.fresh(t0 + Duration::from_secs(140)).unwrap().data,
            vec![9; 4]
        );
    }

    #[test]
    fn only_png_and_webp_can_be_held_and_they_must_be_real_images() {
        let png = klip_diff::rgba_to_png(6, 4, &[9u8; 96]).unwrap();
        assert_eq!(
            held_image_info(&ClipContentType::ImagePng, &png),
            Ok((6, 4))
        );
        assert!(held_image_info(&ClipContentType::PlainTextUtf8, &png).is_err());
        assert!(held_image_info(&ClipContentType::ImageWebP, &png).is_err());
        assert!(held_image_info(&ClipContentType::ImagePng, b"nope").is_err());
    }

    #[test]
    fn offline_label_is_blank_under_a_minute_then_minutes_then_hours() {
        assert_eq!(offline_label(Duration::from_secs(59)), None);
        assert_eq!(
            offline_label(Duration::from_secs(60)).as_deref(),
            Some("1 min")
        );
        assert_eq!(
            offline_label(Duration::from_secs(119 * 60)).as_deref(),
            Some("119 min")
        );
        assert_eq!(
            offline_label(Duration::from_secs(120 * 60)).as_deref(),
            Some("2 h")
        );
    }

    #[test]
    fn only_could_not_reach_errors_count_as_unreachable() {
        assert!(is_unreachable(&NetError::Connect("x".into())));
        assert!(is_unreachable(&NetError::Stream("x".into())));
        assert!(!is_unreachable(&NetError::PeerRefused("paused".into())));
        assert!(!is_unreachable(&NetError::UnknownPeer));
        assert!(!is_unreachable(&NetError::TooLarge { size: 2, limit: 1 }));
    }

    #[test]
    fn one_alert_per_outage_and_a_success_rearms_it() {
        let node = NodeId::new([9u8; 32]);
        let mut alerted = HashSet::new();
        let fail: Result<(), NetError> = Err(NetError::Connect("down".into()));
        let ok: Result<(), NetError> = Ok(());
        let refused: Result<(), NetError> = Err(NetError::PeerRefused("paused".into()));
        assert!(first_failure_of_outage(&mut alerted, node, &fail));
        assert!(!first_failure_of_outage(&mut alerted, node, &fail));
        assert!(!first_failure_of_outage(&mut alerted, node, &refused));
        assert!(!first_failure_of_outage(&mut alerted, node, &ok));
        assert!(first_failure_of_outage(&mut alerted, node, &fail));
    }
}
