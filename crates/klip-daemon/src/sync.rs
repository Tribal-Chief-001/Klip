use crate::config::DaemonConfig;
use crate::history::HistoryBuffer;
use klip_crypto::aead::{decrypt_payload_in_place, encrypt_payload_in_place, NonceGenerator};
use klip_crypto::identity::{DeviceIdentity, KeyStore};
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

        // Advance local HLC
        let hlc_stamp = {
            let mut clk = self.clock.lock().unwrap();
            clk.send_event()
        };

        // Cache for differential sync
        let (msg_type, payload_to_encrypt) = {
            let mut cache = self.diff_cache.lock().unwrap();
            if let Some(delta) = cache.create_delta_if_beneficial(&data) {
                flags |= FLAG_DELTA;
                let delta_bytes = bincode::serialize(&delta).unwrap_or_default();
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

                if let Ok(tag) = encrypt_payload_in_place(&sym_key, &nonce, &mut encrypted_payload) {
                    let packet = WirePacket {
                        header: WireHeader {
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
                        },
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

        // Verify sender key from mesh peer registry
        let sym_key = match self.mesh.get_peer_key(&packet.header.sender).await {
            Some(k) => k,
            None => return, // Ignore clips from unauthenticated/unpaired peers
        };

        let mut decrypted_payload = packet.payload;
        if let Err(_) = decrypt_payload_in_place(
            &sym_key,
            &packet.header.nonce,
            &mut decrypted_payload,
            &packet.tag,
        ) {
            return; // Corrupted or tampered
        }

        // HLC resolution
        let remote_hlc = HybridLogicalClock {
            physical_time_ms: packet.header.hlc_physical,
            logical_counter: packet.header.hlc_logical,
            node_id: packet.header.sender,
        };

        let is_winner = {
            let mut clk = self.clock.lock().unwrap();
            let current_local = *clk;
            let _updated = clk.recv_event(&remote_hlc);
            remote_hlc >= current_local
        };

        if !is_winner {
            // Local clip was created more recently; local clip wins LWW
            return;
        }

        // Delta reconstruction if patch
        let final_data = if (packet.header.flags & FLAG_DELTA) != 0 {
            if let Ok(delta) = bincode::deserialize::<klip_diff::delta::DeltaPatch>(&decrypted_payload) {
                let cache = self.diff_cache.lock().unwrap();
                match cache.apply_delta(&delta) {
                    Ok(reconstructed) => reconstructed,
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

        // Inject into host OS clipboard (Echo Shield suppresses echo loop)
        let _ = self.driver.write_clip(&ClipContentType::PlainTextUtf8, &final_data);

        // Record in local history buffer
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
}
