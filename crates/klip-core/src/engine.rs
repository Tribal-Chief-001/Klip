use crate::config::{DaemonConfig, KlipPaths, RelayMode};
use crate::sync::{node_of, SyncCoordinator};
use klip_crypto::identity::KeyStore;
use klip_net::{KlipNode, NetConfig, NetEvent, PeerRecord, RelayChoice};
use klip_platform::driver::ClipboardDriver;
use klip_proto::hlc::NodeId;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;

/// Cooperative shutdown signal shared by long-running tasks.
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

/// Network node + sync logic with its event pump running. The host adds whatever drives local
/// clipboard capture (a polling loop on desktop, push calls from the app on mobile).
pub struct CoreEngine {
    pub config: DaemonConfig,
    pub paths: KlipPaths,
    pub sync: Arc<SyncCoordinator>,
    pub shutdown: Arc<Shutdown>,
    pump: Mutex<Option<JoinHandle<()>>>,
}

impl CoreEngine {
    pub async fn start(
        config: DaemonConfig,
        paths: KlipPaths,
        driver: Arc<dyn ClipboardDriver>,
    ) -> Result<Arc<Self>, Box<dyn std::error::Error + Send + Sync>> {
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
            file_v2: config.file_protocol_v2,
            ..NetConfig::default()
        };
        let (net, net_events) = KlipNode::start(net_cfg, identity.seed(), trusted).await?;
        if node_of(&net.id()) != identity.node_id {
            return Err("internal error: device id and network id differ".into());
        }

        let sync = Arc::new(SyncCoordinator::new(
            config.clone(),
            paths.clone(),
            identity,
            Arc::new(Mutex::new(keystore)),
            driver,
            net,
        ));
        let shutdown = Arc::new(Shutdown::default());
        // A transfer that died with the previous daemon leaves a hidden partial file: clear old ones.
        let receive_dir = sync.file_policy().dir;
        tokio::task::spawn_blocking(move || {
            crate::files::sweep_partials(&receive_dir, crate::transfers::PARTIAL_MAX_AGE)
        });
        let pump = spawn_event_pump(Arc::clone(&sync), net_events, Arc::clone(&shutdown));

        Ok(Arc::new(Self {
            config,
            paths,
            sync,
            shutdown,
            pump: Mutex::new(Some(pump)),
        }))
    }

    pub fn node_id(&self) -> NodeId {
        self.sync.identity.node_id
    }

    /// Stop pairing, close the network endpoint and end the event pump. Safe to call twice.
    pub async fn stop(&self) {
        self.shutdown.trigger();
        self.sync.cancel_pairing();
        // Let running transfers remove their partial files before the process can end.
        self.sync.transfers.cancel_all();
        for _ in 0..40 {
            if self.sync.transfers.running() == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        self.sync.net.shutdown().await;
        if let Some(pump) = self.pump.lock().await.take() {
            pump.abort();
        }
    }
}

fn spawn_event_pump(
    sync: Arc<SyncCoordinator>,
    mut events: mpsc::Receiver<NetEvent>,
    shutdown: Arc<Shutdown>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown.wait() => break,
                event = events.recv() => match event {
                    Some(ev) => sync.handle_net_event(ev).await,
                    None => break,
                },
            }
        }
    })
}
