use crate::config::get_ipc_socket_path;
use crate::sync::SyncCoordinator;
use klip_crypto::aead::encrypt_payload_in_place_with_aad;
use klip_crypto::identity::PairedPeer;
use klip_crypto::pairing::{compute_sas_invitation, format_sas_pin, PairingInvitation, PairingPsk};
use klip_proto::ipc::{
    deserialize_ipc_msg, serialize_ipc_msg, DaemonStatusInfo, IpcRequest, IpcResponse,
};
use klip_proto::wire::{
    MessageType, WireHeader, WirePacket, CURRENT_VERSION, FLAG_ENCRYPTED, MAGIC_BYTES,
};
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};

pub struct IpcServer {
    sync: Arc<SyncCoordinator>,
    shutdown_flag: Arc<AtomicBool>,
}

impl IpcServer {
    pub fn new(sync: Arc<SyncCoordinator>, shutdown_flag: Arc<AtomicBool>) -> Self {
        Self {
            sync,
            shutdown_flag,
        }
    }

    pub async fn run(&self) -> Result<(), std::io::Error> {
        let sock_path = get_ipc_socket_path();
        #[cfg(unix)]
        {
            let _ = fs::remove_file(&sock_path);
            let listener = UnixListener::bind(&sock_path)?;

            while !self.shutdown_flag.load(Ordering::SeqCst) {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let sync_clone = Arc::clone(&self.sync);
                        let shutdown_clone = Arc::clone(&self.shutdown_flag);
                        tokio::spawn(async move {
                            Self::handle_client(stream, sync_clone, shutdown_clone).await;
                        });
                    }
                    Err(_) => break,
                }
            }
            let _ = fs::remove_file(&sock_path);
        }

        Ok(())
    }

    #[cfg(unix)]
    async fn handle_client(
        mut stream: UnixStream,
        sync: Arc<SyncCoordinator>,
        shutdown: Arc<AtomicBool>,
    ) {
        let mut len_buf = [0u8; 4];
        if stream.read_exact(&mut len_buf).await.is_err() {
            return;
        }
        let msg_len = u32::from_be_bytes(len_buf) as usize;
        let mut payload_buf = vec![0u8; msg_len];
        if stream.read_exact(&mut payload_buf).await.is_err() {
            return;
        }

        let request: IpcRequest = match deserialize_ipc_msg(&payload_buf) {
            Ok(req) => req,
            Err(e) => {
                let resp = IpcResponse::Error(e);
                let _ = Self::send_response(&mut stream, &resp).await;
                return;
            }
        };

        let response = match request {
            IpcRequest::GetStatus => {
                let peers = sync.mesh.peer_statuses().await;
                let last_clip = sync.history.last_item();
                let last_sync_secs = sync.last_sync_instant.lock().unwrap().map(|t| t.elapsed().as_secs());

                let (last_preview, last_size) = match last_clip {
                    Some(c) => (Some(c.content_preview), c.size_bytes),
                    None => (None, 0),
                };

                let remaining_pause = if sync.check_pause_status() {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    sync.pause_until_secs.load(Ordering::SeqCst).saturating_sub(now)
                } else {
                    0
                };

                IpcResponse::Status(DaemonStatusInfo {
                    is_active: true,
                    is_paused: sync.check_pause_status(),
                    paused_remaining_secs: remaining_pause,
                    platform: sync.driver.environment().to_string(),
                    memory_rss_bytes: get_process_rss_bytes(),
                    node_id: sync.identity.node_id,
                    peers,
                    total_synced_clips: sync.total_synced_clips.load(Ordering::SeqCst),
                    last_sync_time_secs_ago: last_sync_secs,
                    last_clip_preview: last_preview,
                    last_clip_size_bytes: last_size,
                })
            }
            IpcRequest::PairInit => {
                let psk = PairingPsk::generate();
                let invitation = PairingInvitation::create(sync.identity.node_id, &psk);
                let token = invitation.to_token();

                // Store pending pairing PSK so incoming handshakes can authenticate
                {
                    let mut lock = sync.pending_pairing.lock().await;
                    *lock = Some((psk.clone(), std::time::Instant::now()));
                }

                // Compute SAS code derived from PSK and host node
                let sas_pin = format_sas_pin(compute_sas_invitation(&psk, &sync.identity.node_id));

                IpcResponse::PairCode {
                    qr_ascii: format!("[ Pairing Token: {} ]", &token[..token.len().min(32)]),
                    sas_code: sas_pin,
                    token,
                }
            }
            IpcRequest::PairConnect { token } => {
                match PairingInvitation::from_token(&token) {
                    Ok(invitation) => match invitation.parse_psk() {
                        Ok(psk) => {
                            let peer_key = psk.to_symmetric_key();
                            let peer_node = invitation.host_node;
                            let peer_name = format!("Klip-{}", peer_node.short_id());

                            // Save to keystore
                            {
                                let mut ks = sync.keystore.lock().await;
                                ks.add_peer(PairedPeer::new(peer_node, peer_name.clone(), &peer_key));
                                let _ = ks.save(crate::config::get_keystore_path());
                            }

                            // Add to active mesh
                            sync.mesh.add_peer(peer_node, peer_name.clone(), peer_key.clone(), None).await;

                            // Send Handshake packet to host node across mesh
                            let mut handshake_payload = sync.config.device_name.clone().into_bytes();
                            let nonce = sync.nonce_gen.next_nonce();
                            let header = WireHeader {
                                magic: MAGIC_BYTES,
                                version: CURRENT_VERSION,
                                msg_type: MessageType::Handshake,
                                flags: FLAG_ENCRYPTED,
                                sender: sync.identity.node_id,
                                nonce,
                                hlc_physical: 0,
                                hlc_logical: 0,
                                hlc_node_seed: 0,
                                payload_len: handshake_payload.len() as u32,
                            };
                            let header_bytes = header.to_bytes();
                            if let Ok(tag) = encrypt_payload_in_place_with_aad(
                                &peer_key,
                                &nonce,
                                &header_bytes,
                                &mut handshake_payload,
                            ) {
                                let packet = WirePacket {
                                    header,
                                    payload: handshake_payload,
                                    tag,
                                };
                                let _ = sync.mesh.broadcast_packet(&packet).await;
                            }

                            IpcResponse::PairSuccess { peer_node, peer_name }
                        }
                        Err(e) => IpcResponse::Error(e),
                    },
                    Err(e) => IpcResponse::Error(e),
                }
            }
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
                let items = sync.history.get_recent(limit);
                IpcResponse::History(items)
            }
            IpcRequest::SendClip { text } => {
                let bytes = text.into_bytes();
                let len = bytes.len();
                sync.process_local_clip(klip_proto::wire::ClipContentType::PlainTextUtf8, bytes).await;
                IpcResponse::Sent { bytes: len }
            }
            IpcRequest::StopDaemon => {
                shutdown.store(true, Ordering::SeqCst);
                IpcResponse::DaemonStopping
            }
        };

        let _ = Self::send_response(&mut stream, &response).await;
    }

    #[cfg(unix)]
    async fn send_response(stream: &mut UnixStream, resp: &IpcResponse) -> Result<(), std::io::Error> {
        let serialized = serialize_ipc_msg(resp).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let len_bytes = (serialized.len() as u32).to_be_bytes();
        stream.write_all(&len_bytes).await?;
        stream.write_all(&serialized).await?;
        stream.flush().await?;
        Ok(())
    }
}

fn get_process_rss_bytes() -> u64 {
    #[cfg(target_os = "linux")]
    {
        if let Ok(statm) = fs::read_to_string("/proc/self/statm") {
            if let Some(rss_pages) = statm.split_whitespace().nth(1) {
                if let Ok(pages) = rss_pages.parse::<u64>() {
                    return pages * 4096;
                }
            }
        }
    }
    4 * 1024 * 1024 // 4 MB baseline fallback estimate
}
