use crate::error::NetError;
use crate::proto::{
    sanitize_name, PairReply, PairRequest, MAX_CONTROL_BYTES, PAIR_ALPN, SYNC_ALPN,
};
use crate::ticket::{now_unix, PairingTicket};
use iroh::endpoint::{presets, Connection, IdleTimeout, Incoming, QuicTransportConfig};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey};
use klip_proto::clip::ClipPayload;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tokio::time::timeout;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(20);
const PAIR_DECISION_TIMEOUT: Duration = Duration::from_secs(60);
const PAIR_CLIENT_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_PAIR_FAILURES: u8 = 5;
/// Slack on top of the clip size limit for the serialized envelope (ids, hlc, mime string).
const ENVELOPE_SLACK: usize = 4096;
const ACK_APPLIED: u8 = 1;
const ACK_REJECTED: u8 = 0;
const ACK_PAUSED: u8 = 2;
const ACK_NO_CLIPBOARD: u8 = 3;
/// How long the receiving application may take to apply a clip before we report a refusal.
const APPLY_TIMEOUT: Duration = Duration::from_secs(5);
const CODE_UNPAIRED: u32 = 1;
const CODE_DONE: u32 = 0;

/// How the endpoint finds and reaches peers beyond the local network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelayChoice {
    /// n0's public relays + DNS address lookup (needed for connections across networks).
    /// Relays see metadata (device ids, IPs, timing, volume) but never clip contents.
    N0Default,
    /// No relay, no discovery: direct addresses only (LAN / tests).
    Disabled,
}

#[derive(Clone, Debug)]
pub struct NetConfig {
    pub device_name: String,
    pub relay: RelayChoice,
    pub keep_alive: Duration,
    pub idle_timeout: Duration,
    pub max_clip_bytes: usize,
    /// Announce this device on the local network (mDNS) and find paired devices there, so sync
    /// works on the same Wi-Fi even without internet. Advertises the device id to the LAN.
    pub lan_discovery: bool,
}

impl Default for NetConfig {
    fn default() -> Self {
        Self {
            device_name: "Klip device".into(),
            relay: RelayChoice::N0Default,
            keep_alive: Duration::from_secs(20),
            idle_timeout: Duration::from_secs(60),
            max_clip_bytes: 32 * 1024 * 1024,
            lan_discovery: true,
        }
    }
}

/// A trusted (paired) device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerRecord {
    pub id: EndpointId,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerStatus {
    pub id: EndpointId,
    pub name: String,
    pub connected: bool,
}

/// What the receiving application did with a clip; this is what the sender is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipOutcome {
    /// Written to the clipboard.
    Applied,
    /// A newer clip was already active (last-write-wins); nothing to do, not an error.
    Superseded,
    /// The receiving device has sync paused.
    Paused,
    /// The OS clipboard could not be written (no display, locked, unsupported type).
    ClipboardUnavailable,
    /// Invalid or unacceptable clip.
    Rejected,
}

/// Reply channel for the application's decision on a received clip.
pub type ClipAck = oneshot::Sender<ClipOutcome>;

/// Reply channel for a pairing request: send `true` to accept, `false` to decline.
pub type PairDecision = oneshot::Sender<bool>;

pub enum NetEvent {
    /// A clip arrived from a paired peer (already authenticated and origin-checked).
    Clip {
        from: EndpointId,
        payload: ClipPayload,
        /// Must be answered; the sender waits for it (5 s) before reporting a refusal.
        ack: ClipAck,
    },
    /// A device holding a valid ticket wants to pair; the host must decide.
    PairRequest {
        peer: EndpointId,
        device_name: String,
        respond: PairDecision,
    },
    /// A device was added to the trusted set (host or joiner side).
    Paired(PeerRecord),
    PeerConnected(EndpointId),
    PeerDisconnected(EndpointId),
}

struct PairingWindow {
    secret: [u8; 16],
    expires: Instant,
    failures: u8,
}

struct Inner {
    cfg: NetConfig,
    peers: Mutex<HashMap<EndpointId, PeerRecord>>,
    conns: Mutex<HashMap<EndpointId, Connection>>,
    pairing: Mutex<Option<PairingWindow>>,
    events: mpsc::Sender<NetEvent>,
}

#[derive(Clone)]
pub struct KlipNode {
    endpoint: Endpoint,
    inner: Arc<Inner>,
}

fn secret_from_seed(seed: &[u8; 32]) -> SecretKey {
    SecretKey::from_bytes(seed)
}

impl KlipNode {
    /// Start the endpoint. `seed` is the device's Ed25519 secret (its public half is the device id).
    pub async fn start(
        cfg: NetConfig,
        seed: &[u8; 32],
        peers: Vec<PeerRecord>,
    ) -> Result<(Self, mpsc::Receiver<NetEvent>), NetError> {
        let idle = IdleTimeout::try_from(cfg.idle_timeout)
            .map_err(|e| NetError::Bind(format!("bad idle timeout: {e}")))?;
        let transport = QuicTransportConfig::builder()
            .keep_alive_interval(cfg.keep_alive)
            .max_idle_timeout(Some(idle))
            .build();

        let builder = match cfg.relay {
            RelayChoice::N0Default => Endpoint::builder(presets::N0),
            RelayChoice::Disabled => {
                Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled)
            }
        };
        let endpoint = builder
            .secret_key(secret_from_seed(seed))
            .alpns(vec![SYNC_ALPN.to_vec(), PAIR_ALPN.to_vec()])
            .transport_config(transport)
            .bind()
            .await
            .map_err(|e| NetError::Bind(e.to_string()))?;

        if cfg.lan_discovery {
            // Failure here (no multicast-capable interface, sandbox) only costs LAN discovery.
            if let Ok(mdns) =
                iroh_mdns_address_lookup::MdnsAddressLookup::builder().build(endpoint.id())
            {
                if let Ok(services) = endpoint.address_lookup() {
                    services.add(mdns);
                }
            }
        }

        let (tx, rx) = mpsc::channel(256);
        let inner = Arc::new(Inner {
            cfg,
            peers: Mutex::new(peers.into_iter().map(|p| (p.id, p)).collect()),
            conns: Mutex::new(HashMap::new()),
            pairing: Mutex::new(None),
            events: tx,
        });
        let node = Self { endpoint, inner };
        node.spawn_accept_loop();
        Ok((node, rx))
    }

    pub fn id(&self) -> EndpointId {
        self.endpoint.id()
    }

    pub fn addr(&self) -> EndpointAddr {
        self.endpoint.addr()
    }

    pub fn peers(&self) -> Vec<PeerRecord> {
        let mut v: Vec<_> = self.inner.peers.lock().unwrap().values().cloned().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    pub fn peer_statuses(&self) -> Vec<PeerStatus> {
        let conns = self.inner.conns.lock().unwrap();
        self.peers()
            .into_iter()
            .map(|p| PeerStatus {
                connected: conns
                    .get(&p.id)
                    .map(|c| c.close_reason().is_none())
                    .unwrap_or(false),
                id: p.id,
                name: p.name,
            })
            .collect()
    }

    pub fn is_paired(&self, id: &EndpointId) -> bool {
        self.inner.peers.lock().unwrap().contains_key(id)
    }

    /// Remove a device from the trusted set and drop its connection.
    pub fn unpair(&self, id: &EndpointId) -> bool {
        let removed = self.inner.peers.lock().unwrap().remove(id).is_some();
        if let Some(conn) = self.inner.conns.lock().unwrap().remove(id) {
            conn.close(CODE_UNPAIRED.into(), b"unpaired");
        }
        removed
    }

    /// Gracefully close the endpoint.
    pub async fn shutdown(&self) {
        self.endpoint.close().await;
    }

    // ---------------------------------------------------------------- pairing (host side)

    /// Open a pairing window and return the ticket to show as text/QR.
    /// Single use: the secret is consumed by the first request that presents it.
    ///
    /// `include_direct_addrs = false` keeps the ticket (and its QR code) small by carrying only the
    /// relay hint; peers then find each other through the relay/discovery. Direct addresses are
    /// always included when relays are disabled, because there is no other way to reach the host.
    pub async fn open_pairing(&self, ttl: Duration, include_direct_addrs: bool) -> PairingTicket {
        if self.inner.cfg.relay == RelayChoice::N0Default {
            // Give the endpoint a moment to learn its home relay so the ticket can carry it.
            let _ = timeout(Duration::from_secs(3), self.endpoint.online()).await;
        }
        let mut secret = [0u8; 16];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut secret);
        *self.inner.pairing.lock().unwrap() = Some(PairingWindow {
            secret,
            expires: Instant::now() + ttl,
            failures: 0,
        });
        let mut addr = self.endpoint.addr();
        if !include_direct_addrs && self.inner.cfg.relay != RelayChoice::Disabled {
            addr.addrs.retain(|a| a.is_relay());
        }
        PairingTicket::new(addr, secret, now_unix() + ttl.as_secs())
    }

    pub fn close_pairing(&self) {
        *self.inner.pairing.lock().unwrap() = None;
    }

    pub fn pairing_open(&self) -> bool {
        matches!(&*self.inner.pairing.lock().unwrap(), Some(w) if Instant::now() < w.expires)
    }

    // --------------------------------------------------------------- pairing (joiner side)

    /// Redeem a ticket: connect to the host, present the secret, wait for the host's approval.
    pub async fn pair_with(&self, ticket: &PairingTicket) -> Result<PeerRecord, NetError> {
        if ticket.is_expired() {
            return Err(NetError::Ticket("ticket has expired".into()));
        }
        let host_id = ticket.addr.id;
        if host_id == self.id() {
            return Err(NetError::Ticket(
                "this ticket belongs to this device".into(),
            ));
        }
        let conn = timeout(
            CONNECT_TIMEOUT,
            self.endpoint.connect(ticket.addr.clone(), PAIR_ALPN),
        )
        .await
        .map_err(|_| NetError::Connect("timed out reaching the host".into()))?
        .map_err(|e| NetError::Connect(e.to_string()))?;

        let result = timeout(PAIR_CLIENT_TIMEOUT, async {
            let (mut tx, mut rx) = conn
                .open_bi()
                .await
                .map_err(|e| NetError::Stream(e.to_string()))?;
            let req = PairRequest {
                secret: ticket.secret,
                device_name: sanitize_name(&self.inner.cfg.device_name),
            };
            let bytes = postcard::to_stdvec(&req).map_err(|e| NetError::Protocol(e.to_string()))?;
            tx.write_all(&bytes)
                .await
                .map_err(|e| NetError::Stream(e.to_string()))?;
            tx.finish().map_err(|e| NetError::Stream(e.to_string()))?;
            let reply = rx
                .read_to_end(MAX_CONTROL_BYTES)
                .await
                .map_err(|e| NetError::Stream(e.to_string()))?;
            postcard::from_bytes::<PairReply>(&reply).map_err(|e| NetError::Protocol(e.to_string()))
        })
        .await
        .map_err(|_| NetError::PairingRejected("host did not answer in time".into()))?;

        conn.close(CODE_DONE.into(), b"done");
        match result? {
            PairReply::Accepted { device_name } => {
                let rec = PeerRecord {
                    id: host_id,
                    name: sanitize_name(&device_name),
                };
                self.inner
                    .peers
                    .lock()
                    .unwrap()
                    .insert(host_id, rec.clone());
                let _ = self.inner.events.send(NetEvent::Paired(rec.clone())).await;
                Ok(rec)
            }
            PairReply::Rejected { reason } => Err(NetError::PairingRejected(reason)),
        }
    }

    // ------------------------------------------------------------------------- sync (send)

    /// Send one clip to every paired peer. Returns the per-peer outcome.
    /// Peers that are unreachable simply fail: there is no offline queue by design.
    pub async fn send_clip(
        &self,
        payload: &ClipPayload,
    ) -> Vec<(EndpointId, Result<(), NetError>)> {
        if payload.content.len() > self.inner.cfg.max_clip_bytes {
            let err = || NetError::TooLarge {
                size: payload.content.len(),
                limit: self.inner.cfg.max_clip_bytes,
            };
            return self
                .peers()
                .into_iter()
                .map(|p| (p.id, Err(err())))
                .collect();
        }
        let bytes: Arc<Vec<u8>> = match postcard::to_stdvec(payload) {
            Ok(b) => Arc::new(b),
            Err(e) => {
                let msg = e.to_string();
                return self
                    .peers()
                    .into_iter()
                    .map(|p| (p.id, Err(NetError::Protocol(msg.clone()))))
                    .collect();
            }
        };
        let mut set = JoinSet::new();
        for peer in self.peers() {
            let node = self.clone();
            let bytes = Arc::clone(&bytes);
            set.spawn(async move { (peer.id, node.send_to(peer.id, &bytes).await) });
        }
        let mut out = Vec::new();
        while let Some(res) = set.join_next().await {
            if let Ok(item) = res {
                out.push(item);
            }
        }
        out
    }

    async fn send_to(&self, id: EndpointId, bytes: &[u8]) -> Result<(), NetError> {
        // Retry once with a fresh connection, but only for transport problems (a cached
        // connection can be dead). A peer that answered "no" has given its final answer: sending
        // the same clip again would, at best, be reported as a stale duplicate.
        match self.try_send(id, bytes, false).await {
            Err(NetError::PeerRefused(why)) => Err(NetError::PeerRefused(why)),
            Err(_) => self.try_send(id, bytes, true).await,
            ok => ok,
        }
    }

    async fn try_send(
        &self,
        id: EndpointId,
        bytes: &[u8],
        force_new: bool,
    ) -> Result<(), NetError> {
        let conn = self.connection_to(id, force_new).await?;
        timeout(WRITE_TIMEOUT, async {
            let (mut tx, mut rx) = conn
                .open_bi()
                .await
                .map_err(|e| NetError::Stream(e.to_string()))?;
            tx.write_all(bytes)
                .await
                .map_err(|e| NetError::Stream(e.to_string()))?;
            tx.finish().map_err(|e| NetError::Stream(e.to_string()))?;
            // "Ok" means the peer's application accepted the clip, not merely that bytes left us.
            let ack = rx
                .read_to_end(1)
                .await
                .map_err(|e| NetError::Stream(e.to_string()))?;
            match ack.as_slice() {
                [ACK_APPLIED] => Ok(()),
                [ACK_PAUSED] => Err(NetError::PeerRefused(
                    "the other device has sync paused".into(),
                )),
                [ACK_NO_CLIPBOARD] => Err(NetError::PeerRefused(
                    "the other device could not write its clipboard".into(),
                )),
                _ => Err(NetError::PeerRefused(
                    "the other device rejected the clip".into(),
                )),
            }
        })
        .await
        .map_err(|_| NetError::Stream("send timed out".into()))?
    }

    async fn connection_to(&self, id: EndpointId, force_new: bool) -> Result<Connection, NetError> {
        if !self.is_paired(&id) {
            return Err(NetError::UnknownPeer);
        }
        if !force_new {
            if let Some(c) = self.inner.conns.lock().unwrap().get(&id) {
                if c.close_reason().is_none() {
                    return Ok(c.clone());
                }
            }
        }
        let conn = timeout(
            CONNECT_TIMEOUT,
            self.endpoint.connect(EndpointAddr::new(id), SYNC_ALPN),
        )
        .await
        .map_err(|_| NetError::Connect("timed out".into()))?
        .map_err(|e| NetError::Connect(e.to_string()))?;
        self.register_connection(conn.clone());
        Ok(conn)
    }

    // ---------------------------------------------------------------------- sync (receive)

    fn spawn_accept_loop(&self) {
        let node = self.clone();
        tokio::spawn(async move {
            while let Some(incoming) = node.endpoint.accept().await {
                let node = node.clone();
                tokio::spawn(async move {
                    node.handle_incoming(incoming).await;
                });
            }
        });
    }

    async fn handle_incoming(&self, incoming: Incoming) {
        let Ok(accepting) = incoming.accept() else {
            return;
        };
        let Ok(conn) = accepting.await else {
            return;
        };
        let remote = conn.remote_id();
        match conn.alpn() {
            a if a == PAIR_ALPN => self.handle_pair(conn, remote).await,
            a if a == SYNC_ALPN => {
                if self.is_paired(&remote) {
                    self.register_connection(conn);
                } else {
                    conn.close(CODE_UNPAIRED.into(), b"not paired");
                }
            }
            _ => conn.close(CODE_UNPAIRED.into(), b"unsupported protocol"),
        }
    }

    /// Remember the connection and spawn its reader. Called for dialed and accepted connections.
    fn register_connection(&self, conn: Connection) {
        let remote = conn.remote_id();
        let stable = conn.stable_id();
        let previous = self
            .inner
            .conns
            .lock()
            .unwrap()
            .insert(remote, conn.clone());
        if previous.map(|p| p.close_reason().is_some()).unwrap_or(true) {
            let _ = self.inner.events.try_send(NetEvent::PeerConnected(remote));
        }
        let node = self.clone();
        tokio::spawn(async move {
            node.read_loop(conn.clone(), remote).await;
            let mut conns = node.inner.conns.lock().unwrap();
            let still_current = conns.get(&remote).map(|c| c.stable_id() == stable) == Some(true);
            if still_current {
                conns.remove(&remote);
                drop(conns);
                let _ = node
                    .inner
                    .events
                    .try_send(NetEvent::PeerDisconnected(remote));
            }
        });
    }

    async fn read_loop(&self, conn: Connection, remote: EndpointId) {
        let limit = self.inner.cfg.max_clip_bytes + ENVELOPE_SLACK;
        loop {
            let Ok((mut tx, mut rx)) = conn.accept_bi().await else {
                return;
            };
            if !self.is_paired(&remote) {
                conn.close(CODE_UNPAIRED.into(), b"unpaired");
                return;
            }
            let outcome = match rx.read_to_end(limit).await {
                // Oversized or broken stream: reject this clip, keep the connection.
                Err(_) => ClipOutcome::Rejected,
                Ok(bytes) => match postcard::from_bytes::<ClipPayload>(&bytes) {
                    Err(_) => ClipOutcome::Rejected,
                    Ok(payload) => {
                        // A peer may only speak for itself: forged origins are dropped.
                        let origin_ok = payload.origin_node.as_bytes() == remote.as_bytes();
                        let size_ok = payload.content.len() <= self.inner.cfg.max_clip_bytes;
                        if origin_ok && size_ok {
                            let (ack_tx, ack_rx) = oneshot::channel();
                            let sent = self
                                .inner
                                .events
                                .send(NetEvent::Clip {
                                    from: remote,
                                    payload,
                                    ack: ack_tx,
                                })
                                .await
                                .is_ok();
                            if sent {
                                timeout(APPLY_TIMEOUT, ack_rx)
                                    .await
                                    .ok()
                                    .and_then(|r| r.ok())
                                    .unwrap_or(ClipOutcome::Rejected)
                            } else {
                                ClipOutcome::Rejected
                            }
                        } else {
                            ClipOutcome::Rejected
                        }
                    }
                },
            };
            let ack = match outcome {
                ClipOutcome::Applied | ClipOutcome::Superseded => ACK_APPLIED,
                ClipOutcome::Paused => ACK_PAUSED,
                ClipOutcome::ClipboardUnavailable => ACK_NO_CLIPBOARD,
                ClipOutcome::Rejected => ACK_REJECTED,
            };
            let _ = tx.write_all(&[ack]).await;
            let _ = tx.finish();
        }
    }

    // ------------------------------------------------------------------- pairing (host loop)

    async fn handle_pair(&self, conn: Connection, remote: EndpointId) {
        let Ok((mut tx, mut rx)) = conn.accept_bi().await else {
            return;
        };
        let Ok(bytes) = rx.read_to_end(MAX_CONTROL_BYTES).await else {
            return;
        };
        let Ok(req) = postcard::from_bytes::<PairRequest>(&bytes) else {
            conn.close(CODE_UNPAIRED.into(), b"bad request");
            return;
        };

        let reply = match self.consume_pairing_secret(&req.secret) {
            Err(reason) => PairReply::Rejected { reason },
            Ok(()) => {
                let name = sanitize_name(&req.device_name);
                let (respond, decision) = oneshot::channel();
                let asked = self
                    .inner
                    .events
                    .send(NetEvent::PairRequest {
                        peer: remote,
                        device_name: name.clone(),
                        respond,
                    })
                    .await
                    .is_ok();
                let accepted =
                    asked && matches!(timeout(PAIR_DECISION_TIMEOUT, decision).await, Ok(Ok(true)));
                if accepted {
                    let rec = PeerRecord { id: remote, name };
                    self.inner.peers.lock().unwrap().insert(remote, rec.clone());
                    let _ = self.inner.events.send(NetEvent::Paired(rec)).await;
                    PairReply::Accepted {
                        device_name: sanitize_name(&self.inner.cfg.device_name),
                    }
                } else {
                    PairReply::Rejected {
                        reason: "the host declined the request".into(),
                    }
                }
            }
        };

        if let Ok(bytes) = postcard::to_stdvec(&reply) {
            let _ = tx.write_all(&bytes).await;
            let _ = tx.finish();
            let _ = timeout(Duration::from_secs(5), tx.stopped()).await;
        }
        conn.close(CODE_DONE.into(), b"done");
    }

    /// Validate and consume the one-time secret. Wrong secrets count towards a lockout.
    fn consume_pairing_secret(&self, presented: &[u8; 16]) -> Result<(), String> {
        let mut guard = self.inner.pairing.lock().unwrap();
        let Some(window) = guard.as_mut() else {
            return Err("no pairing in progress on the host".into());
        };
        if Instant::now() >= window.expires {
            *guard = None;
            return Err("the pairing code has expired".into());
        }
        if window.secret.ct_eq(presented).into() {
            *guard = None; // single use
            Ok(())
        } else {
            window.failures += 1;
            if window.failures >= MAX_PAIR_FAILURES {
                *guard = None;
            }
            Err("invalid pairing code".into())
        }
    }
}
