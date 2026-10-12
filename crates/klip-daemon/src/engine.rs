use crate::config::{user_paths, DaemonConfig, KlipPaths};
use crate::ipc_server::IpcServer;
use klip_core::{CoreEngine, Shutdown, SyncCoordinator};
use klip_platform::create_default_driver;
use klip_platform::driver::ClipboardDriver;
use klip_proto::hlc::NodeId;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{interval, MissedTickBehavior};

/// The desktop daemon: the shared core engine plus clipboard polling and the local control socket.
pub struct DaemonEngine {
    pub config: DaemonConfig,
    pub paths: KlipPaths,
    pub core: Arc<CoreEngine>,
    pub sync: Arc<SyncCoordinator>,
    pub shutdown: Arc<Shutdown>,
}

impl DaemonEngine {
    /// Start with the OS clipboard driver and the user's config directory.
    pub async fn init(config: DaemonConfig) -> Result<Self, Box<dyn std::error::Error>> {
        Self::init_with(config, user_paths(), None).await
    }

    /// Full-control constructor (custom directory and/or clipboard driver). Used by tests.
    pub async fn init_with(
        config: DaemonConfig,
        paths: KlipPaths,
        driver: Option<Arc<dyn ClipboardDriver>>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        config.validate()?;
        let driver: Arc<dyn ClipboardDriver> = match driver {
            Some(d) => d,
            None => {
                // The driver needs the device id, which lives in the keystore the core creates.
                let (_ks, identity) = klip_crypto::identity::KeyStore::load_or_create(
                    paths.keystore(),
                    &config.device_name,
                )
                .map_err(|e| format!("Failed to load keystore: {e}"))?;
                Arc::from(create_default_driver(identity.node_id))
            }
        };
        let core = CoreEngine::start(config.clone(), paths.clone(), driver)
            .await
            .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?;
        Ok(Self {
            config,
            paths,
            sync: Arc::clone(&core.sync),
            shutdown: Arc::clone(&core.shutdown),
            core,
        })
    }

    pub fn node_id(&self) -> NodeId {
        self.core.node_id()
    }

    pub async fn run(self: &Arc<Self>) -> Result<(), Box<dyn std::error::Error>> {
        // Control socket first, so a failure is reported instead of leaving a deaf daemon.
        let ipc = IpcServer::new(Arc::clone(&self.sync), Arc::clone(&self.shutdown));
        #[cfg(unix)]
        let ipc_task = {
            let listener = ipc.bind()?;
            tokio::spawn(async move { ipc.serve(listener).await })
        };
        #[cfg(not(unix))]
        let ipc_task = tokio::spawn(async move { drop(ipc) });

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
                _ = self.shutdown.wait() => break,
                _ = stop_signals.recv() => {
                    self.shutdown.trigger();
                    break;
                }
            }
        }

        self.core.stop().await;
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
