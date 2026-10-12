use crate::config::{DaemonConfig, KlipPaths, RelayMode};
use crate::ipc_server::IpcServer;
use crate::sync::{node_of, SyncCoordinator};
use klip_crypto::identity::KeyStore;
use klip_net::{KlipNode, NetConfig, NetEvent, PeerRecord, RelayChoice};
use klip_platform::create_default_driver;
use klip_platform::driver::ClipboardDriver;
use klip_proto::hlc::NodeId;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{interval, MissedTickBehavior};

pub struct DaemonEngine {
    pub config: DaemonConfig,
    pub paths: KlipPaths,
    pub sync: Arc<SyncCoordinator>,
    pub shutdown: Arc<Shutdown>,
    net_events: Mutex<mpsc::Receiver<NetEvent>>,
}

/// Cooperative shutdown signal shared by the engine loop and the IPC server.
pub struct Shutdown {
    tx: tokio::sync::watch::Sender<bool>,
}

impl Default for Shutdown {
    fn default() -> Self {
        Self {
            tx: tokio::sync::watch::channel(false).0,
        }
    }
}

impl Shutdown {
    pub fn trigger(&self) {
        self.tx.send_replace(true);
    }

    pub fn is_triggered(&self) -> bool {
        *self.tx.borrow()
    }

    /// Resolves once shutdown was requested (immediately if it already was).
    pub async fn wait(&self) {
        let mut rx = self.tx.subscribe();
        let _ = rx.wait_for(|stop| *stop).await;
    }
}

impl DaemonEngine {
    /// Start with the OS clipboard driver and the user's config directory.
    pub async fn init(config: DaemonConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let paths = KlipPaths::for_user();
        Self::init_with(config, paths, None).await
    }

    /// Full-control constructor (custom directory and/or clipboard driver). Used by tests.
    pub async fn init_with(
        config: DaemonConfig,
        paths: KlipPaths,
        driver: Option<Arc<dyn ClipboardDriver>>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        config.validate()?;
        let (keystore, identity) = KeyStore::load_or_create(paths.keystore(), &config.device_name)
            .map_err(|e| format!("Failed to load keystore: {e}"))?;
        let identity = Arc::new(identity);

        let trusted: Vec<PeerRecord> = keystore
            .peers
            .values()
            .filter_map(|p| {
                klip_net::EndpointId::from_bytes(p.node_id.as_bytes())
                    .ok()
                    .map(|id| PeerRecord {
                        id,
                        name: p.name.clone(),
                    })
            })
            .collect();

        let net_cfg = NetConfig {
            device_name: config.device_name.clone(),
            relay: match config.relay {
                RelayMode::N0 => RelayChoice::N0Default,
                RelayMode::Lan => RelayChoice::Disabled,
            },
            max_clip_bytes: config.max_clip_bytes,
            lan_discovery: config.lan_discovery,
            ..NetConfig::default()
        };
        let (net, net_events) = KlipNode::start(net_cfg, identity.seed(), trusted).await?;
        if node_of(&net.id()) != identity.node_id {
            return Err("internal error: device id and network id differ".into());
        }

        let driver: Arc<dyn ClipboardDriver> = match driver {
            Some(d) => d,
            None => Arc::from(create_default_driver(identity.node_id)),
        };

        let sync = Arc::new(SyncCoordinator::new(
            config.clone(),
            paths.clone(),
            identity,
            Arc::new(Mutex::new(keystore)),
            driver,
            net,
        ));

        Ok(Self {
            config,
            paths,
            sync,
            shutdown: Arc::new(Shutdown::default()),
            net_events: Mutex::new(net_events),
        })
    }

    pub fn node_id(&self) -> NodeId {
        self.sync.identity.node_id
    }

    pub async fn run(self: &Arc<Self>) -> Result<(), Box<dyn std::error::Error>> {
        // IPC server (local control socket).
        let ipc = IpcServer::new(Arc::clone(&self.sync), Arc::clone(&self.shutdown));
        #[cfg(unix)]
        let ipc_task = {
            let listener = ipc.bind()?;
            tokio::spawn(async move { ipc.serve(listener).await })
        };
        #[cfg(not(unix))]
        let ipc_task = tokio::spawn(async move { drop(ipc) });

        let mut events = self.net_events.lock().await;
        let mut poll = interval(Duration::from_millis(self.config.poll_interval_ms));
        poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut stop_signals = StopSignals::new();

        loop {
            tokio::select! {
                _ = poll.tick() => {
                    if let Ok(Some(captured)) = self.sync.driver.read_clip() {
                        self.sync.spawn_local_clip(captured.content_type, captured.data);
                    }
                }
                Some(event) = events.recv() => {
                    self.sync.handle_net_event(event).await;
                }
                _ = self.shutdown.wait() => break,
                _ = stop_signals.recv() => {
                    self.shutdown.trigger();
                    break;
                }
            }
        }

        self.sync.cancel_pairing();
        self.sync.net.shutdown().await;
        let _ = ipc_task.await;
        Ok(())
    }
}

/// Ctrl+C and (on Unix) SIGTERM, which is what `systemctl stop` and launchd send.
struct StopSignals {
    #[cfg(unix)]
    term: Option<tokio::signal::unix::Signal>,
}

impl StopSignals {
    fn new() -> Self {
        Self {
            #[cfg(unix)]
            term: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok(),
        }
    }

    async fn recv(&mut self) {
        #[cfg(unix)]
        {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = async {
                    match self.term.as_mut() {
                        Some(sig) => { sig.recv().await; }
                        None => std::future::pending::<()>().await,
                    }
                } => {}
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}
