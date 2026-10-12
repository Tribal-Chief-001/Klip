use crate::mailbox::EphemeralMailbox;
use crate::relay::StatelessRelayRouter;
use klip_crypto::aead::SymmetricKey;
use klip_proto::hlc::NodeId;
use klip_proto::ipc::PeerStatusInfo;
use klip_proto::wire::WirePacket;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{Duration, Instant};

pub const DEFAULT_KLIP_PORT: u16 = 7123;
pub const BROADCAST_PORT: u16 = 7124;

#[derive(Debug, Clone)]
pub struct ActivePeer {
    pub node_id: NodeId,
    pub name: String,
    pub addr: Option<SocketAddr>,
    pub sym_key: SymmetricKey,
    pub is_direct: bool,
    pub latency_ms: u32,
    pub last_seen: Instant,
}

pub struct MeshNode {
    pub local_node: NodeId,
    pub socket: Arc<UdpSocket>,
    pub peers: Arc<Mutex<HashMap<NodeId, ActivePeer>>>,
    pub mailbox: EphemeralMailbox,
    pub relay: StatelessRelayRouter,
    pub inbound_tx: mpsc::Sender<WirePacket>,
}

impl MeshNode {
    pub async fn bind(
        local_node: NodeId,
        bind_addr: &str,
        inbound_tx: mpsc::Sender<WirePacket>,
    ) -> Result<Arc<Self>, std::io::Error> {
        let socket = match UdpSocket::bind(bind_addr).await {
            Ok(s) => s,
            Err(_) => {
                // Fallback to ephemeral port if preferred port in use
                UdpSocket::bind("0.0.0.0:0").await?
            }
        };

        let node = Arc::new(Self {
            local_node,
            socket: Arc::new(socket),
            peers: Arc::new(Mutex::new(HashMap::new())),
            mailbox: EphemeralMailbox::new(),
            relay: StatelessRelayRouter::new(),
            inbound_tx,
        });

        // Spawn packet receiver loop
        let node_clone = Arc::clone(&node);
        tokio::spawn(async move {
            node_clone.receive_loop().await;
        });

        Ok(node)
    }

    pub async fn add_peer(&self, node_id: NodeId, name: String, sym_key: SymmetricKey, addr: Option<SocketAddr>) {
        let mut lock = self.peers.lock().await;
        lock.insert(
            node_id,
            ActivePeer {
                node_id,
                name,
                addr,
                sym_key,
                is_direct: addr.is_some(),
                latency_ms: if addr.is_some() { 15 } else { 45 },
                last_seen: Instant::now(),
            },
        );
    }

    pub async fn get_peer_key(&self, node_id: &NodeId) -> Option<SymmetricKey> {
        let lock = self.peers.lock().await;
        lock.get(node_id).map(|p| p.sym_key.clone())
    }

    pub async fn peer_statuses(&self) -> Vec<PeerStatusInfo> {
        let lock = self.peers.lock().await;
        lock.values()
            .map(|p| PeerStatusInfo {
                node_id: p.node_id,
                name: p.name.clone(),
                connection_type: if p.is_direct {
                    "Direct (P2P)".to_string()
                } else {
                    "Relay (ZK)".to_string()
                },
                latency_ms: p.latency_ms,
                is_online: p.last_seen.elapsed().as_secs() < 60,
            })
            .collect()
    }

    /// Broadcast an encoded wire packet to all linked peers
    pub async fn broadcast_packet(&self, packet: &WirePacket) -> Result<usize, std::io::Error> {
        let encoded = packet.encode();
        let peers_snapshot: Vec<ActivePeer> = {
            let lock = self.peers.lock().await;
            lock.values().cloned().collect()
        };

        let mut sent_count = 0;
        for peer in peers_snapshot {
            if let Some(addr) = peer.addr {
                if let Ok(_) = self.socket.send_to(&encoded, addr).await {
                    sent_count += 1;
                }
            } else {
                // If offline / direct address unknown, store into ephemeral mailbox
                self.mailbox.deposit(
                    [0u8; 32],
                    peer.node_id,
                    encoded.clone(),
                    crate::mailbox::DEFAULT_MAILBOX_TTL_SECS,
                );
            }
        }

        Ok(sent_count)
    }

    async fn receive_loop(&self) {
        let mut buf = vec![0u8; 65535];
        loop {
            match self.socket.recv_from(&mut buf).await {
                Ok((len, remote_addr)) => {
                    let packet_bytes = &buf[..len];
                    if let Ok(packet) = WirePacket::decode(packet_bytes) {
                        // Update peer address and last seen
                        {
                            let mut peers = self.peers.lock().await;
                            if let Some(peer) = peers.get_mut(&packet.header.sender) {
                                peer.addr = Some(remote_addr);
                                peer.is_direct = true;
                                peer.last_seen = Instant::now();
                            }
                        }

                        // Forward packet to daemon inbound channel
                        let _ = self.inbound_tx.send(packet).await;
                    }
                }
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }
}
