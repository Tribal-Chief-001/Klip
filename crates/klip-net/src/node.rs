use crate::error::NetError;
use crate::file::{FILE_ALPN, FILE_ALPN_V2};
use crate::proto::{
    sanitize_name, PairReply, PairRequest, MAX_CONTROL_BYTES, PAIR_ALPN, SYNC_ALPN,
};
use crate::ticket::{now_unix, PairingTicket};
use iroh::endpoint::{
    presets, Connection, ConnectionError, IdleTimeout, Incoming, QuicTransportConfig, RecvStream,
    SendStream, Side, VarInt,
};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey, Watcher};
use klip_proto::clip::ClipPayload;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;
use tokio::sync::{mpsc, oneshot, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;
use tokio::time::timeout;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Base time for a clip exchange plus time proportional to its size (assumes at least 64 KiB/s),
/// so a 30 MB clip over a slow link is not killed after 10 s.
const WRITE_TIMEOUT_BASE: Duration = Duration::from_secs(10);
/// A clip up to this size is re-sent once on a fresh connection after a timeout (the cached
/// connection may be dead: peer crashed, laptop slept, network changed). Larger clips are not
/// re-sent automatically because that would double a big transfer.
const RETRY_AFTER_TIMEOUT_MAX_BYTES: usize = 256 * 1024;
const MIN_THROUGHPUT_BYTES_PER_SEC: u64 = 64 * 1024;
/// A first attempt that fails faster than this was a dead cached connection and is retried once.
const FAST_FAILURE: Duration = Duration::from_secs(3);
/// Only a handful of unauthenticated pairing handshakes may run at the same time.
const MAX_CONCURRENT_PAIR_HANDLERS: usize = 4;
/// File connections that may be waiting for the application at once (the application limits transfers).
const MAX_CONCURRENT_FILE_HANDLERS: usize = 8;
/// A file connection must open its one stream within this long.
const FILE_STREAM_TIMEOUT: Duration = Duration::from_secs(15);
/// How long the receiving side waits for the sender to close after the verdict.
const FILE_CLOSE_WAIT: Duration = Duration::from_secs(5);
const PAIR_STREAM_TIMEOUT: Duration = Duration::from_secs(10);
/// Received clips wait here for the application; kept small because each can be tens of MB.
const EVENT_QUEUE: usize = 8;
/// Streams a peer may have open at once (each can hold a whole clip in memory).
const MAX_BIDI_STREAMS: u32 = 4;
const MAINTAIN_TICK: Duration = Duration::from_secs(5);
const RECONNECT_MIN: Duration = Duration::from_secs(5);
const RECONNECT_MAX: Duration = Duration::from_secs(60);
const PAIR_DECISION_TIMEOUT: Duration = Duration::from_secs(60);
const PAIR_CLIENT_TIMEOUT: Duration = Duration::from_secs(90);
/// Slack on top of the clip size limit for the serialized envelope (ids, hlc, mime string).
const ENVELOPE_SLACK: usize = 4096;
const ACK_APPLIED: u8 = 1;
const ACK_REJECTED: u8 = 0;
const ACK_PAUSED: u8 = 2;
const ACK_NO_CLIPBOARD: u8 = 3;
/// The receiver already has a newer clip (last-write-wins), so this one was not applied.
const ACK_SUPERSEDED: u8 = 4;
/// How long the receiving application may take to apply a clip before we report a refusal.
const APPLY_TIMEOUT: Duration = Duration::from_secs(5);
const CODE_UNPAIRED: u32 = 1;
const CODE_DONE: u32 = 0;
/// Closed because a duplicate connection to the same peer won the tie-break.
const CODE_SUPERSEDED: u32 = 2;
/// Shown in peer status when the other device closed with `CODE_UNPAIRED` on a sync connection.
const REMOVED_BY_PEER: &str = "unpaired by the other device; pair again";

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
    /// Speak the file transfer protocol (an extra ALPN). Always on in Klip itself; off only in tests
    /// that need a device that predates files.
    pub file_transfer: bool,
    /// Also speak file transfer version 2 (resume, folders). Always on in Klip itself; off only in tests that
    /// need a device that has version 1 only.
    pub file_v2: bool,
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
            file_transfer: true,
            file_v2: true,
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
    /// A standing problem with this peer that retrying cannot fix (for example, it unpaired us).
    pub problem: Option<String>,
    /// How long since this peer was last seen connected; `None` while it is connected.
    pub offline_for: Option<Duration>,
}

/// What the receiving application did with a clip; this is what the sender is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipOutcome {
    /// Written to the clipboard.
    Applied,
    /// A newer clip was already active (last-write-wins): this one was not applied. The sender is told.
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
    /// A paired device opened a file stream. Handle it on its own task at once: this queue is shared
    /// with clips. Dropping `conn` ends the connection.
    File {
        from: EndpointId,
        tx: SendStream,
        rx: RecvStream,
        /// 1 or 2: which file protocol this connection speaks.
        version: u8,
        conn: FileConnection,
    },
    /// A device was added to the trusted set (host or joiner side).
    Paired(PeerRecord),
    PeerConnected(EndpointId),
    PeerDisconnected(EndpointId),
}

/// The connection a file arrives on, kept open until the transfer is over.
pub struct FileConnection {
    conn: Connection,
    _slot: OwnedSemaphorePermit,
}

impl FileConnection {
    /// The verdict has been sent: wait (briefly) for the sender to close, so the last bytes are delivered.
    pub async fn finished(self) {
        let _ = timeout(FILE_CLOSE_WAIT, self.conn.closed()).await;
    }
}

/// The streams of a file connection this device opened.
pub struct FileStreams {
    pub tx: SendStream,
    pub rx: RecvStream,
    conn: Connection,
}

impl FileStreams {
    /// The file protocol the two devices agreed on: 2 when both speak it, else 1.
    pub fn version(&self) -> u8 {
        if self.conn.alpn() == FILE_ALPN_V2 {
            2
        } else {
            1
        }
    }

    /// The transfer is over (verdict read): close the connection.
    pub fn close(self) {
        self.conn.close(CODE_DONE.into(), b"done");
    }

    /// The person cancelled: tell the receiver it is a cancel and not a cut (a reset with `CANCEL_CODE`), give the
    /// frame a moment to leave, then close. Best effort: when the signal is lost the receiver treats it as a cut.
    pub async fn cancel(mut self) {
        let _ = self.tx.reset(VarInt::from_u32(crate::file::CANCEL_CODE));
        tokio::time::sleep(Duration::from_millis(200)).await;
        self.conn.close(CODE_DONE.into(), b"cancelled");
    }
}

struct PairingWindow {
    secret: [u8; 16],
    expires: Instant,
}

struct Inner {
    cfg: NetConfig,
    peers: Mutex<HashMap<EndpointId, PeerRecord>>,
    conns: Mutex<HashMap<EndpointId, Connection>>,
    problems: Mutex<HashMap<EndpointId, String>>,
    /// When each peer was last seen connected (refreshed whenever `peer_statuses` runs, which the
    /// 5 s maintainer tick does). Connection state is used, not `PeerDisconnected` events:
    /// `drop_connection` removes a connection without raising one.
    last_seen: Mutex<HashMap<EndpointId, Instant>>,
    /// Connections with a running reader task (diagnostics and leak tests).
    readers: std::sync::atomic::AtomicUsize,
    pairing: Mutex<Option<PairingWindow>>,
    events: mpsc::Sender<NetEvent>,
    pair_handlers: Semaphore,
    file_handlers: Arc<Semaphore>,
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
            // Klip only uses bidirectional streams, a few at a time; bounding them bounds memory.
            .max_concurrent_bidi_streams(VarInt::from_u32(MAX_BIDI_STREAMS))
            .max_concurrent_uni_streams(VarInt::from_u32(0))
            .build();

        let mut alpns = vec![SYNC_ALPN.to_vec(), PAIR_ALPN.to_vec()];
        if cfg.file_transfer {
            // The server's order decides when both versions fit: version 2 must come first.
            if cfg.file_v2 {
                alpns.push(FILE_ALPN_V2.to_vec());
            }
            alpns.push(FILE_ALPN.to_vec());
        }
        let builder = match cfg.relay {
            RelayChoice::N0Default => Endpoint::builder(presets::N0),
            RelayChoice::Disabled => {
                Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled)
            }
        };
        let endpoint = builder
            .secret_key(secret_from_seed(seed))
            .alpns(alpns)
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

        let (tx, rx) = mpsc::channel(EVENT_QUEUE);
        let inner = Arc::new(Inner {
            cfg,
            peers: Mutex::new(peers.into_iter().map(|p| (p.id, p)).collect()),
            conns: Mutex::new(HashMap::new()),
            problems: Mutex::new(HashMap::new()),
            last_seen: Mutex::new(HashMap::new()),
            readers: std::sync::atomic::AtomicUsize::new(0),
            pairing: Mutex::new(None),
            events: tx,
            pair_handlers: Semaphore::new(MAX_CONCURRENT_PAIR_HANDLERS),
            file_handlers: Arc::new(Semaphore::new(MAX_CONCURRENT_FILE_HANDLERS)),
        });
        let node = Self { endpoint, inner };
        node.spawn_accept_loop();
        node.spawn_connection_maintainer();
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
        let problems = self.inner.problems.lock().unwrap();
        let mut seen = self.inner.last_seen.lock().unwrap();
        let now = Instant::now();
        self.peers()
            .into_iter()
            .map(|p| {
                let connected = conns
                    .get(&p.id)
                    .map(|c| c.close_reason().is_none())
                    .unwrap_or(false);
                // A peer never seen yet (daemon start, fresh pairing) counts as seen now.
                let last = seen.entry(p.id).or_insert(now);
                if connected {
                    *last = now;
                }
                PeerStatus {
                    connected,
                    offline_for: (!connected).then(|| now.duration_since(*last)),
                    problem: problems.get(&p.id).cloned(),
                    id: p.id,
                    name: p.name,
                }
            })
            .collect()
    }

    /// One-line, human-readable state of the connection to the relay server, for status screens.
    /// This is what to look at first when two devices on different networks cannot find each other.
    pub fn relay_state_label(&self) -> String {
        if self.inner.cfg.relay == RelayChoice::Disabled {
            return "relays off (LAN only)".to_string();
        }
        let statuses = self.endpoint.home_relay_status().get();
        match statuses.first() {
            None => "choosing a relay…".to_string(),
            Some(r) if r.is_connected() => format!("relay connected ({})", r.url()),
            Some(r) => match r.last_error() {
                Some(e) => format!("relay unreachable: {e}"),
                None => "connecting to relay…".to_string(),
            },
        }
    }

    /// Number of sync connections currently being read (should be at most one per paired peer).
    pub fn open_connection_count(&self) -> usize {
        self.inner.readers.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn is_paired(&self, id: &EndpointId) -> bool {
        self.inner.peers.lock().unwrap().contains_key(id)
    }

    /// Remove a device from the trusted set and drop its connection.
    pub fn unpair(&self, id: &EndpointId) -> bool {
        let removed = self.inner.peers.lock().unwrap().remove(id).is_some();
        self.inner.problems.lock().unwrap().remove(id);
        self.inner.last_seen.lock().unwrap().remove(id);
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
        // Expiry is enforced by the host with its own clock. Judging it here with *this* device's
        // clock would reject valid tickets on a phone whose date/time is wrong.
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
        // A failed connection is never reused: drop it so the next attempt dials fresh. Retry once
        // on a fresh connection when the failure was fast (a dead cached connection is refused at
        // once) or when a small clip timed out (the cached connection was silently dead). A peer
        // that answered "no" has given its final answer; connect failures are not retried at once.
        // A duplicate delivery is harmless: the receiver answers "superseded" for a clip it has.
        let started = Instant::now();
        match self.try_send(id, bytes, false).await {
            Ok(()) => Ok(()),
            Err(NetError::PeerRefused(why)) => Err(NetError::PeerRefused(why)),
            Err(e @ (NetError::UnknownPeer | NetError::Connect(_))) => Err(e),
            Err(e) => {
                self.drop_connection(&id);
                let fast = started.elapsed() <= FAST_FAILURE;
                if fast || bytes.len() <= RETRY_AFTER_TIMEOUT_MAX_BYTES {
                    self.try_send(id, bytes, true).await
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Forget and close the cached connection to a peer (it just failed us).
    fn drop_connection(&self, id: &EndpointId) {
        if let Some(conn) = self.inner.conns.lock().unwrap().remove(id) {
            conn.close(CODE_DONE.into(), b"abandoned");
        }
    }

    async fn try_send(
        &self,
        id: EndpointId,
        bytes: &[u8],
        force_new: bool,
    ) -> Result<(), NetError> {
        let conn = self.connection_to(id, force_new).await?;
        let limit = WRITE_TIMEOUT_BASE
            + Duration::from_secs(bytes.len() as u64 / MIN_THROUGHPUT_BYTES_PER_SEC);
        timeout(limit, async {
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
                [ACK_SUPERSEDED] => Err(NetError::PeerRefused(
                    "the other device already has a newer clip; this one was not applied".into(),
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
        Ok(self.register_connection(conn))
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

    /// Keep a connection open to every paired peer (reconnecting with exponential backoff), so
    /// "connected" in status screens is true and the first clip does not pay connection setup.
    fn spawn_connection_maintainer(&self) {
        let node = self.clone();
        tokio::spawn(async move {
            let mut retry: HashMap<EndpointId, (Instant, Duration)> = HashMap::new();
            let mut tick = tokio::time::interval(MAINTAIN_TICK);
            loop {
                tokio::select! {
                    _ = node.endpoint.closed() => return,
                    _ = tick.tick() => {}
                }
                let live: Vec<EndpointId> = node
                    .peer_statuses()
                    .into_iter()
                    .filter(|p| p.connected)
                    .map(|p| p.id)
                    .collect();
                let paired: Vec<EndpointId> = node.peers().into_iter().map(|p| p.id).collect();
                retry.retain(|id, _| paired.contains(id));
                for id in paired {
                    if live.contains(&id) {
                        retry.remove(&id);
                        continue;
                    }
                    if matches!(retry.get(&id), Some((next, _)) if Instant::now() < *next) {
                        continue;
                    }
                    match node.connection_to(id, true).await {
                        Ok(_) => {
                            retry.remove(&id);
                        }
                        Err(_) => {
                            let delay = retry
                                .get(&id)
                                .map(|(_, d)| (*d * 2).min(RECONNECT_MAX))
                                .unwrap_or(RECONNECT_MIN);
                            retry.insert(id, (Instant::now() + delay, delay));
                        }
                    }
                }
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
                    let _ = self.register_connection(conn);
                } else {
                    conn.close(CODE_UNPAIRED.into(), b"not paired");
                }
            }
            a if a == FILE_ALPN => self.handle_file(conn, remote, 1).await,
            a if a == FILE_ALPN_V2 => self.handle_file(conn, remote, 2).await,
            _ => conn.close(CODE_UNPAIRED.into(), b"unsupported protocol"),
        }
    }

    /// A file connection: paired devices only, one stream, handed to the application.
    async fn handle_file(&self, conn: Connection, remote: EndpointId, version: u8) {
        if !self.is_paired(&remote) {
            conn.close(CODE_UNPAIRED.into(), b"not paired");
            return;
        }
        let Ok(slot) = Arc::clone(&self.inner.file_handlers).try_acquire_owned() else {
            conn.close(CODE_DONE.into(), b"busy");
            return;
        };
        let Ok(Ok((tx, rx))) = timeout(FILE_STREAM_TIMEOUT, conn.accept_bi()).await else {
            conn.close(CODE_DONE.into(), b"no stream");
            return;
        };
        let event = NetEvent::File {
            from: remote,
            tx,
            rx,
            version,
            conn: FileConnection {
                conn: conn.clone(),
                _slot: slot,
            },
        };
        if self.inner.events.send(event).await.is_err() {
            conn.close(CODE_DONE.into(), b"shutting down");
        }
    }

    /// Dial a paired device for one file: a new connection (its own ALPN, so clips are never held up)
    /// and one stream. If the clip connection to that device is up but this one cannot be made, the
    /// device is an older Klip.
    pub async fn open_file_stream(&self, id: EndpointId) -> Result<FileStreams, NetError> {
        if !self.is_paired(&id) {
            return Err(NetError::UnknownPeer);
        }
        // Offer version 2 and accept version 1: an older device answers with the one it knows.
        let attempt = async {
            let (primary, extra) = if self.inner.cfg.file_v2 {
                (FILE_ALPN_V2, vec![FILE_ALPN.to_vec()])
            } else {
                (FILE_ALPN, Vec::new())
            };
            let connecting = self
                .endpoint
                .connect_with_opts(
                    EndpointAddr::new(id),
                    primary,
                    iroh::endpoint::ConnectOptions::new().with_additional_alpns(extra),
                )
                .await
                .map_err(|e| e.to_string())?;
            connecting.await.map_err(|e| e.to_string())
        };
        let dialled = timeout(CONNECT_TIMEOUT, attempt).await;
        let conn = match dialled {
            Err(_) => return Err(NetError::Connect("timed out".into())),
            Ok(Err(e)) => {
                let clip_connection_up = self
                    .inner
                    .conns
                    .lock()
                    .unwrap()
                    .get(&id)
                    .is_some_and(|c| c.close_reason().is_none());
                return Err(if clip_connection_up {
                    NetError::FileUnsupported
                } else {
                    NetError::Connect(e.to_string())
                });
            }
            Ok(Ok(conn)) => conn,
        };
        let (tx, rx) = conn
            .open_bi()
            .await
            .map_err(|e| NetError::Stream(e.to_string()))?;
        Ok(FileStreams { tx, rx, conn })
    }

    /// Remember the connection and spawn its reader. Called for dialed and accepted connections.
    ///
    /// When both devices dial each other at the same moment there are two connections. They would
    /// both stay open (double keep-alive traffic, and each side sending on a different one), so
    /// both ends apply the same rule: keep the connection dialed by the device with the lower id.
    /// Returns the connection to use, which is the other one if this one lost.
    fn register_connection(&self, conn: Connection) -> Connection {
        let remote = conn.remote_id();
        let stable = conn.stable_id();
        let i_am_preferred_dialer = self.endpoint.id().as_bytes() < remote.as_bytes();
        let dialed_by_me = conn.side() == Side::Client;

        let mut conns = self.inner.conns.lock().unwrap();
        let live_previous = conns.get(&remote).filter(|c| c.close_reason().is_none());
        if let Some(old) = live_previous {
            let old_dialed_by_me = old.side() == Side::Client;
            let new_wins = old_dialed_by_me == dialed_by_me // same dialer: the newer one is fresher
                || dialed_by_me == i_am_preferred_dialer;
            if !new_wins {
                let winner = old.clone();
                drop(conns);
                conn.close(CODE_SUPERSEDED.into(), b"duplicate connection");
                return winner;
            }
            old.close(CODE_SUPERSEDED.into(), b"duplicate connection");
        }
        let was_connected = live_previous.is_some();
        conns.insert(remote, conn.clone());
        drop(conns);
        // A fresh attempt may succeed where the last one was refused; a refusal sets it again.
        self.inner.problems.lock().unwrap().remove(&remote);
        if !was_connected {
            let _ = self.inner.events.try_send(NetEvent::PeerConnected(remote));
        }
        let node = self.clone();
        let reader = conn.clone();
        self.inner
            .readers
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        tokio::spawn(async move {
            node.read_loop(reader, remote).await;
            node.inner
                .readers
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
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
        conn
    }

    async fn read_loop(&self, conn: Connection, remote: EndpointId) {
        let limit = self.inner.cfg.max_clip_bytes + ENVELOPE_SLACK;
        loop {
            let (mut tx, mut rx) = match conn.accept_bi().await {
                Ok(streams) => streams,
                Err(ConnectionError::ApplicationClosed(close))
                    if close.error_code == VarInt::from_u32(CODE_UNPAIRED) =>
                {
                    // The other device deliberately closed on us: it does not have us paired.
                    // Reconnecting cannot fix that, so say so instead of failing silently.
                    if self.is_paired(&remote) {
                        self.inner
                            .problems
                            .lock()
                            .unwrap()
                            .insert(remote, REMOVED_BY_PEER.to_string());
                    }
                    return;
                }
                Err(_) => return,
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
                ClipOutcome::Applied => ACK_APPLIED,
                ClipOutcome::Superseded => ACK_SUPERSEDED,
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
        // Pairing connections come from *unauthenticated* devices: cap how many run at once, bound
        // the request size, and never wait forever. A stranger with no open window still gets a
        // clear "no pairing in progress" answer, which costs us one tiny read.
        let Ok(_permit) = self.inner.pair_handlers.try_acquire() else {
            conn.close(CODE_UNPAIRED.into(), b"busy");
            return;
        };
        let Ok(Ok((mut tx, mut rx))) = timeout(PAIR_STREAM_TIMEOUT, conn.accept_bi()).await else {
            conn.close(CODE_UNPAIRED.into(), b"timeout");
            return;
        };
        let Ok(Ok(bytes)) = timeout(PAIR_STREAM_TIMEOUT, rx.read_to_end(MAX_CONTROL_BYTES)).await
        else {
            conn.close(CODE_UNPAIRED.into(), b"timeout");
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
        // A wrong secret does not close the window: the secret is 128 bits (not guessable), and a
        // lockout would let any stranger who knows the device id cancel a pairing in progress.
        if window.secret.ct_eq(presented).into() {
            *guard = None; // single use
            Ok(())
        } else {
            Err("invalid pairing code".into())
        }
    }
}
