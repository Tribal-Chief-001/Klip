use crate::config::DaemonConfig;
use crate::history::HistoryBuffer;
use klip_crypto::aead::{
    decrypt_payload_in_place_with_aad, encrypt_payload_in_place_with_aad, NonceGenerator,
};
use klip_crypto::identity::{DeviceIdentity, KeyStore, PairedPeer};
use klip_crypto::pairing::PairingPsk;
use klip_diff::delta::RecentClipCache;
use klip_diff::entropy::analyze_snippet;
use klip_platform::driver::ClipboardDriver;
use klip_proto::hlc::HybridLogicalClock;
use klip_proto::wire::{
    ClipContentType, MessageType, WireHeader, WirePacket, CURRENT_VERSION,
    FLAG_DELTA, FLAG_ENCRYPTED, FLAG_SENSITIVE, MAGIC_BYTES,
};
use klip_transport::mesh::MeshNode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

pub struct SyncCoordinator {
    pub config: DaemonConfig,
    pub identity: Arc<DeviceIdentity>,
    pub keystore: Arc<Mutex<KeyStore>>,
    pub driver: Arc<Box<dyn ClipboardDriver>>,
    pub mesh: Arc<MeshNode>,
    pub clock: Arc<std::sync::Mutex<HybridLogicalClock>>,
    pub last_applied_clip_hlc: Arc<std::sync::Mutex<Option<HybridLogicalClock>>>,
    pub pending_pairing: Arc<Mutex<Option<(PairingPsk, Instant)>>>,
    pub nonce_gen: Arc<NonceGenerator>,
    pub diff_cache: Arc<std::sync::Mutex<RecentClipCache>>,
    pub history: HistoryBuffer,
    pub is_paused: AtomicBool,
    pub pause_until_secs: AtomicU64,
    pub total_synced_clips: AtomicU64,
    pub last_sync_instant: Arc<std::sync::Mutex<Option<Instant>>>,
}

impl SyncCoordinator {
    pub fn new(
        config: DaemonConfig,
        identity: Arc<DeviceIdentity>,
        keystore: Arc<Mutex<KeyStore>>,
        driver: Arc<Box<dyn ClipboardDriver>>,
        mesh: Arc<MeshNode>,
    ) -> Self {
        let node_id = identity.node_id;
        let clock = Arc::new(std::sync::Mutex::new(HybridLogicalClock::new(node_id)));
        let last_applied_clip_hlc = Arc::new(std::sync::Mutex::new(None));
        let pending_pairing = Arc::new(Mutex::new(None));
        let nonce_gen = Arc::new(NonceGenerator::new(1));
        let diff_cache = Arc::new(std::sync::Mutex::new(RecentClipCache::default()));
        let history = HistoryBuffer::new(config.max_history_items);

        Self {
            config,
            identity,
            keystore,
            driver,
            mesh,
            clock,
            last_applied_clip_hlc,
            pending_pairing,
            nonce_gen,
            diff_cache,
            history,
            is_paused: AtomicBool::new(false),
            pause_until_secs: AtomicU64::new(0),
            total_synced_clips: AtomicU64::new(0),
            last_sync_instant: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    pub fn pause(&self, duration_secs: u64) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.pause_until_secs.store(now + duration_secs, Ordering::SeqCst);
        self.is_paused.store(true, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.is_paused.store(false, Ordering::SeqCst);
        self.pause_until_secs.store(0, Ordering::SeqCst);
    }

    pub fn check_pause_status(&self) -> bool {
        if self.is_paused.load(Ordering::SeqCst) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let until = self.pause_until_secs.load(Ordering::SeqCst);
            if until > 0 && now >= until {
                self.resume();
                false
            } else {
                true
            }
        } else {
            false
        }
    }

    /// Process locally captured clipboard item and broadcast to mesh
    pub async fn process_local_clip(&self, _content_type: ClipContentType, data: Vec<u8>) {
        if self.check_pause_status() || data.is_empty() {
            return;
        }

        // Entropy / Sensitivity check
        let is_sensitive = if let Ok(text) = std::str::from_utf8(&data) {
            analyze_snippet(text).is_likely_secret
        } else {
            false
        };

        let mut flags = FLAG_ENCRYPTED;
        if is_sensitive {
            flags |= FLAG_SENSITIVE;
        }

        // Advance local HLC and record as active local clip
        let hlc_stamp = {
            let mut clk = self.clock.lock().unwrap();
            let stamp = clk.send_event();
            let mut active = self.last_applied_clip_hlc.lock().unwrap();
            *active = Some(stamp);
            stamp
        };

        // Cache for differential sync
        let (msg_type, payload_to_encrypt) = {
            let mut cache = self.diff_cache.lock().unwrap();
            if let Some(delta) = cache.create_delta_if_beneficial(&data) {
                flags |= FLAG_DELTA;
                let delta_bytes = bincode::serialize(&delta).unwrap_or_default();
                cache.insert(data.clone()); // Cache base for subsequent deltas
                (MessageType::ClipDeltaPatch, delta_bytes)
            } else {
                cache.insert(data.clone());
                (MessageType::ClipFullPayload, data.clone())
            }
        };

        // Record in local history
        self.history.push(
            [0u8; 16],
            self.identity.node_id,
            self.config.device_name.clone(),
            data.clone(),
            is_sensitive,
        );

        // Broadcast to all paired peers with their respective symmetric keys
        let peers_status = self.mesh.peer_statuses().await;
        for peer_info in peers_status {
            if let Some(sym_key) = self.mesh.get_peer_key(&peer_info.node_id).await {
                let nonce = self.nonce_gen.next_nonce();
                let mut encrypted_payload = payload_to_encrypt.clone();

                let header = WireHeader {
                    magic: MAGIC_BYTES,
                    version: CURRENT_VERSION,
                    msg_type,
                    flags,
                    sender: self.identity.node_id,
                    nonce,
                    hlc_physical: hlc_stamp.physical_time_ms,
                    hlc_logical: hlc_stamp.logical_counter,
                    hlc_node_seed: u32::from_be_bytes([
                        self.identity.node_id.0[0],
                        self.identity.node_id.0[1],
                        self.identity.node_id.0[2],
                        self.identity.node_id.0[3],
                    ]),
                    payload_len: encrypted_payload.len() as u32,
                };
                let header_bytes = header.to_bytes();

                if let Ok(tag) = encrypt_payload_in_place_with_aad(
                    &sym_key,
                    &nonce,
                    &header_bytes,
                    &mut encrypted_payload,
                ) {
                    let packet = WirePacket {
                        header,
                        payload: encrypted_payload,
                        tag,
                    };

                    let _ = self.mesh.broadcast_packet(&packet).await;
                }
            }
        }

        self.total_synced_clips.fetch_add(1, Ordering::SeqCst);
        let mut last_sync = self.last_sync_instant.lock().unwrap();
        *last_sync = Some(Instant::now());
    }

    /// Process an inbound packet received from mesh
    pub async fn process_inbound_packet(&self, packet: WirePacket) {
        if self.check_pause_status() {
            return;
        }

        // 1. Handle pairing handshakes
        if packet.header.msg_type == MessageType::Handshake {
            self.handle_inbound_handshake(packet).await;
            return;
        }
        if packet.header.msg_type == MessageType::HandshakeAck {
            self.handle_inbound_handshake_ack(packet).await;
            return;
        }

        // 2. Verify sender key from mesh peer registry
        let sym_key = match self.mesh.get_peer_key(&packet.header.sender).await {
            Some(k) => k,
            None => return, // Ignore clips from unauthenticated/unpaired peers
        };

        let header_bytes = packet.header.to_bytes();
        let mut decrypted_payload = packet.payload;
        if let Err(_) = decrypt_payload_in_place_with_aad(
            &sym_key,
            &packet.header.nonce,
            &header_bytes,
            &mut decrypted_payload,
            &packet.tag,
        ) {
            return; // Corrupted or tampered
        }

        // 3. HLC causality and Last-Write-Wins (LWW) resolution
        let remote_hlc = HybridLogicalClock {
            physical_time_ms: packet.header.hlc_physical,
            logical_counter: packet.header.hlc_logical,
            node_id: packet.header.sender,
        };

        // Monotonically advance node's logical clock
        {
            let mut clk = self.clock.lock().unwrap();
            clk.recv_event(&remote_hlc);
        }

        // Check if remote clip supersedes active local clipboard content
        {
            let mut active_guard = self.last_applied_clip_hlc.lock().unwrap();
            if let Some(active_hlc) = *active_guard {
                if remote_hlc <= active_hlc {
                    // Local active clip was created more recently; local clip wins LWW
                    return;
                }
            }
            *active_guard = Some(remote_hlc);
        }

        // 4. Delta reconstruction if patch
        let final_data = if (packet.header.flags & FLAG_DELTA) != 0 {
            if let Ok(delta) = bincode::deserialize::<klip_diff::delta::DeltaPatch>(&decrypted_payload) {
                let mut cache = self.diff_cache.lock().unwrap();
                match cache.apply_delta(&delta) {
                    Ok(reconstructed) => {
                        cache.insert(reconstructed.clone());
                        reconstructed
                    }
                    Err(_) => return, // Base clip missing, cannot apply
                }
            } else {
                return;
            }
        } else {
            let mut cache = self.diff_cache.lock().unwrap();
            cache.insert(decrypted_payload.clone());
            decrypted_payload
        };

        let is_sensitive = (packet.header.flags & FLAG_SENSITIVE) != 0;

        // 5. Inject into host OS clipboard (Echo Shield suppresses echo loop)
        let _ = self.driver.write_clip(&ClipContentType::PlainTextUtf8, &final_data);

        // 6. Record in local history buffer
        self.history.push(
            [0u8; 16],
            packet.header.sender,
            "Linked Peer".to_string(),
            final_data,
            is_sensitive,
        );

        self.total_synced_clips.fetch_add(1, Ordering::SeqCst);
        let mut last_sync = self.last_sync_instant.lock().unwrap();
        *last_sync = Some(Instant::now());
    }

    async fn handle_inbound_handshake(&self, packet: WirePacket) {
        let (pending_psk, _) = match self.pending_pairing.lock().await.clone() {
            Some(pair) => pair,
            None => return,
        };

        let sym_key = pending_psk.to_symmetric_key();
        let header_bytes = packet.header.to_bytes();
        let mut payload = packet.payload;

        if decrypt_payload_in_place_with_aad(
            &sym_key,
            &packet.header.nonce,
            &header_bytes,
            &mut payload,
            &packet.tag,
        ).is_err() {
            return;
        }

        let peer_name = String::from_utf8(payload).unwrap_or_else(|_| format!("Klip-{}", packet.header.sender.short_id()));
        let peer_node = packet.header.sender;

        // Save to keystore
        {
            let mut ks = self.keystore.lock().await;
            ks.add_peer(PairedPeer::new(peer_node, peer_name.clone(), &sym_key));
            let _ = ks.save(crate::config::get_keystore_path());
        }

        // Add to active mesh
        self.mesh.add_peer(peer_node, peer_name.clone(), sym_key.clone(), None).await;

        // Clear pending pairing
        *self.pending_pairing.lock().await = None;

        // Send HandshakeAck back
        let mut ack_payload = self.config.device_name.clone().into_bytes();
        let nonce = self.nonce_gen.next_nonce();
        let ack_header = WireHeader {
            magic: MAGIC_BYTES,
            version: CURRENT_VERSION,
            msg_type: MessageType::HandshakeAck,
            flags: FLAG_ENCRYPTED,
            sender: self.identity.node_id,
            nonce,
            hlc_physical: 0,
            hlc_logical: 0,
            hlc_node_seed: 0,
            payload_len: ack_payload.len() as u32,
        };
        let ack_header_bytes = ack_header.to_bytes();
        if let Ok(tag) = encrypt_payload_in_place_with_aad(&sym_key, &nonce, &ack_header_bytes, &mut ack_payload) {
            let ack_packet = WirePacket {
                header: ack_header,
                payload: ack_payload,
                tag,
            };
            let _ = self.mesh.broadcast_packet(&ack_packet).await;
        }
    }

    async fn handle_inbound_handshake_ack(&self, packet: WirePacket) {
        if let Some(sym_key) = self.mesh.get_peer_key(&packet.header.sender).await {
            let header_bytes = packet.header.to_bytes();
            let mut payload = packet.payload;
            if decrypt_payload_in_place_with_aad(
                &sym_key,
                &packet.header.nonce,
                &header_bytes,
                &mut payload,
                &packet.tag,
            ).is_ok() {
                let mut peers = self.mesh.peers.lock().await;
                if let Some(peer) = peers.get_mut(&packet.header.sender) {
                    peer.last_seen = tokio::time::Instant::now();
                }
            }
        }
    }
}
