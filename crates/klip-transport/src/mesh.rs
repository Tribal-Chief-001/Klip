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

/// Detects local network interface candidates and loopback for peer connection hints.
pub fn detect_local_endpoints(port: u16) -> Vec<String> {
    let mut candidates = Vec::new();
    if let Ok(temp_sock) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if temp_sock.connect("8.8.8.8:80").is_ok() {
            if let Ok(local) = temp_sock.local_addr() {
                candidates.push(format!("{}:{}", local.ip(), port));
            }
        }
    }
    candidates.push(format!("127.0.0.1:{}", port));
    candidates
}

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

        // Enable UDP broadcast for LAN peer discovery
        let _ = socket.set_broadcast(true);

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

        // Spawn periodic heartbeat keepalive loop (every 20s) to keep NAT pinholes open
        let hb_node = Arc::clone(&node);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(20));
            loop {
                interval.tick().await;
                let peers_snapshot: Vec<(Option<SocketAddr>, NodeId)> = {
                    let lock = hb_node.peers.lock().await;
                    lock.values().map(|p| (p.addr, p.node_id)).collect()
                };

                for (addr_opt, _) in peers_snapshot {
                    let ping = Self::create_heartbeat_packet(hb_node.local_node, false);
                    let encoded = ping.encode();
                    if let Some(addr) = addr_opt {
                        let _ = hb_node.socket.send_to(&encoded, addr).await;
                    } else if let Ok(bcast_addr) =
                        format!("255.255.255.255:{}", DEFAULT_KLIP_PORT).parse::<SocketAddr>()
                    {
                        let _ = hb_node.socket.send_to(&encoded, bcast_addr).await;
                    }
                }
            }
        });

        Ok(node)
    }

    pub fn create_heartbeat_packet(local_node: NodeId, is_pong: bool) -> WirePacket {
        let header = klip_proto::wire::WireHeader {
            magic: klip_proto::wire::MAGIC_BYTES,
            version: klip_proto::wire::CURRENT_VERSION,
            msg_type: if is_pong {
                klip_proto::wire::MessageType::HeartbeatPong
            } else {
                klip_proto::wire::MessageType::HeartbeatPing
            },
            flags: 0,
            sender: local_node,
            nonce: klip_proto::wire::WireNonce::new(0),
            hlc_physical: 0,
            hlc_logical: 0,
            hlc_node_seed: 0,
            payload_len: 0,
        };
        WirePacket {
            header,
            payload: Vec::new(),
            tag: [0u8; 16],
        }
    }

    pub async fn add_peer(
        &self,
        node_id: NodeId,
        name: String,
        sym_key: SymmetricKey,
        addr: Option<SocketAddr>,
    ) {
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

    pub async fn send_packet_direct(
        &self,
        packet: &WirePacket,
        addr: SocketAddr,
    ) -> Result<(), std::io::Error> {
        let encoded = packet.encode();
        self.socket.send_to(&encoded, addr).await?;
        Ok(())
    }

    /// Broadcast an encoded wire packet to all linked peers
    pub async fn broadcast_packet(&self, packet: &WirePacket) -> Result<usize, std::io::Error> {
        let encoded = packet.encode();
        let peers_snapshot: Vec<ActivePeer> = {
            let lock = self.peers.lock().await;
            lock.values().cloned().collect()
        };

        let mut sent_count = 0;
        let mut had_unresolved = false;
        for peer in peers_snapshot {
            if let Some(addr) = peer.addr {
                if self.socket.send_to(&encoded, addr).await.is_ok() {
                    sent_count += 1;
                }
            } else {
                had_unresolved = true;
                // If offline / direct address unknown, store into ephemeral mailbox
                self.mailbox.deposit(
                    [0u8; 32],
                    peer.node_id,
                    encoded.clone(),
                    crate::mailbox::DEFAULT_MAILBOX_TTL_SECS,
                );
            }
        }

        // If any peer has unresolved address, broadcast to LAN and loopback
        if had_unresolved {
            if let Ok(bcast_addr) =
                format!("255.255.255.255:{}", DEFAULT_KLIP_PORT).parse::<SocketAddr>()
            {
                let _ = self.socket.send_to(&encoded, bcast_addr).await;
            }
            if let Ok(loopback_addr) =
                format!("127.0.0.1:{}", DEFAULT_KLIP_PORT).parse::<SocketAddr>()
            {
                let _ = self.socket.send_to(&encoded, loopback_addr).await;
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
                        // Handle HeartbeatPing / HeartbeatPong
                        if packet.header.msg_type == klip_proto::wire::MessageType::HeartbeatPing {
                            {
                                let mut peers = self.peers.lock().await;
                                if let Some(peer) = peers.get_mut(&packet.header.sender) {
                                    peer.addr = Some(remote_addr);
                                    peer.is_direct = true;
                                    peer.last_seen = Instant::now();
                                }
                            }
                            let pong = Self::create_heartbeat_packet(self.local_node, true);
                            let _ = self.socket.send_to(&pong.encode(), remote_addr).await;
                            continue;
                        }
                        if packet.header.msg_type == klip_proto::wire::MessageType::HeartbeatPong {
                            let mut peers = self.peers.lock().await;
                            if let Some(peer) = peers.get_mut(&packet.header.sender) {
                                peer.addr = Some(remote_addr);
                                peer.is_direct = true;
                                peer.last_seen = Instant::now();
                            }
                            continue;
                        }

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
                        let _ = self.inbound_tx.send(packet.clone()).await;

                        // Drain pending mailbox items for this peer now that we have their address
                        let pending_items = self.mailbox.fetch_and_drain(&packet.header.sender);
                        for item in pending_items {
                            let _ = self.socket.send_to(&item.encrypted_blob, remote_addr).await;
                        }
                    }
                }
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }
}
