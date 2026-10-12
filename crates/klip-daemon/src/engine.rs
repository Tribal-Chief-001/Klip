use crate::config::{get_keystore_path, DaemonConfig};
use crate::ipc_server::IpcServer;
use crate::sync::SyncCoordinator;
use klip_crypto::identity::KeyStore;
use klip_platform::create_default_driver;
use klip_proto::wire::WirePacket;
use klip_transport::mesh::MeshNode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tokio::time::interval;

pub struct DaemonEngine {
    pub config: DaemonConfig,
    pub sync: Arc<SyncCoordinator>,
    pub shutdown_flag: Arc<AtomicBool>,
    pub inbound_rx: Mutex<mpsc::Receiver<WirePacket>>,
}

impl DaemonEngine {
    pub async fn init(config: DaemonConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let keystore_path = get_keystore_path();
        let (keystore, identity) = KeyStore::load_or_create(&keystore_path, &config.device_name)
            .map_err(|e| format!("Failed to load keystore: {}", e))?;

        let identity = Arc::new(identity);
        let keystore = Arc::new(Mutex::new(keystore));
        let driver = Arc::new(create_default_driver(identity.node_id));

        let (inbound_tx, inbound_rx) = mpsc::channel(256);
        let bind_addr = format!("0.0.0.0:{}", config.listen_port);
        let mesh = MeshNode::bind(identity.node_id, &bind_addr, inbound_tx).await?;

        // Populate active peers from keystore
        {
            let ks = keystore.lock().await;
            for peer in ks.peers.values() {
                if let Ok(sym_key) = peer.get_symmetric_key() {
                    mesh.add_peer(peer.node_id, peer.name.clone(), sym_key, None).await;
                }
            }
        }

        let sync = Arc::new(SyncCoordinator::new(
            config.clone(),
            identity,
            keystore,
            driver,
            mesh,
        ));

        let shutdown_flag = Arc::new(AtomicBool::new(false));

        Ok(Self {
            config,
            sync,
            shutdown_flag,
            inbound_rx: Mutex::new(inbound_rx),
        })
    }

    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        let shutdown = Arc::clone(&self.shutdown_flag);

        // 1. Spawn IPC server
        let ipc_server = IpcServer::new(Arc::clone(&self.sync), Arc::clone(&shutdown));
        tokio::spawn(async move {
            let _ = ipc_server.run().await;
        });

        // 2. Inbound packet processing loop
        let mut inbound_rx = self.inbound_rx.lock().await;

        // 3. Local clipboard polling interval (100ms)
        let mut poll_timer = interval(Duration::from_millis(self.config.poll_interval_ms));

        while !shutdown.load(Ordering::SeqCst) {
            tokio::select! {
                _ = poll_timer.tick() => {
                    if let Ok(Some(captured)) = self.sync.driver.read_clip() {
                        self.sync.process_local_clip(captured.content_type, captured.data).await;
                    }
                }
                Some(packet) = inbound_rx.recv() => {
                    self.sync.process_inbound_packet(packet).await;
                }
                _ = tokio::signal::ctrl_c() => {
                    shutdown.store(true, Ordering::SeqCst);
                    break;
                }
            }
        }

        Ok(())
    }
}
