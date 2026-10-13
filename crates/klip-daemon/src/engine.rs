use crate::config::{user_paths, DaemonConfig, KlipPaths};
use crate::instance_lock::InstanceLock;
use crate::ipc_server::IpcServer;
use klip_core::{CoreEngine, Shutdown, SyncCoordinator};
use klip_platform::create_default_driver;
use klip_platform::driver::ClipboardDriver;
use klip_proto::hlc::NodeId;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// Held for the life of the daemon: only one instance per configuration directory.
    _lock: InstanceLock,
    capture: Arc<CaptureGate>,
}

/// Coordinates clipboard reads: one at a time, with one more queued if a change arrives meanwhile.
/// Separate from `DaemonEngine` so a read in flight never keeps the instance lock alive.
#[derive(Default)]
struct CaptureGate {
    capturing: AtomicBool,
    recapture: AtomicBool,
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
        let lock = InstanceLock::acquire(&paths)?;
        let driver: Arc<dyn ClipboardDriver> = match driver {
            Some(d) => d,
            None => {
                // The driver needs the device id, which lives in the keystore the core creates.
                let (_ks, identity) = klip_crypto::identity::KeyStore::load_or_create(
                    paths.keystore(),
                    &config.device_name,
                )
                .map_err(|e| format!("Failed to load keystore: {e}"))?;
                // KLIP_CLIPBOARD=memory runs without touching the OS clipboard (headless servers,
                // demos, tests of the real binary): clips are still synced and kept in history.
                if std::env::var("KLIP_CLIPBOARD").as_deref() == Ok("memory") {
                    Arc::new(klip_platform::memory::MemoryClipboardDriver::new(
                        identity.node_id,
                    ))
                } else {
                    Arc::from(create_default_driver(identity.node_id))
                }
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
            _lock: lock,
            capture: Arc::default(),
        })
    }

    /// Read the clipboard off the event loop (an X11 selection read can block on a slow owner).
    /// A change that arrives while a read is running triggers exactly one more read afterwards.
    fn capture_clipboard(&self) {
        let gate = Arc::clone(&self.capture);
        if gate.capturing.swap(true, Ordering::SeqCst) {
            gate.recapture.store(true, Ordering::SeqCst);
            return;
        }
        let sync = Arc::clone(&self.sync);
        tokio::spawn(async move {
            loop {
                gate.recapture.store(false, Ordering::SeqCst);
                let driver = Arc::clone(&sync.driver);
                if let Ok(Ok(Some(captured))) =
                    tokio::task::spawn_blocking(move || driver.read_clip()).await
                {
                    sync.spawn_local_clip(captured.content_type, captured.data);
                }
                if gate.recapture.load(Ordering::SeqCst) {
                    continue;
                }
                gate.capturing.store(false, Ordering::SeqCst);
                // A change signalled between the check and the store must not be lost.
                if gate.recapture.load(Ordering::SeqCst)
                    && !gate.capturing.swap(true, Ordering::SeqCst)
                {
                    continue;
                }
                break;
            }
        });
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

        // With an OS change notification (X11 XFIXES) the clipboard is read only when it changes;
        // the slow timer is just a safety net. Without one, poll at the configured interval.
        let mut changes = self.sync.driver.change_signal();
        let poll_period = if changes.is_some() {
            Duration::from_secs(30)
        } else {
            Duration::from_millis(self.config.poll_interval_ms)
        };
        let mut poll = interval(poll_period);
        poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
        // Clips older than the configured age are forgotten even when nobody looks at the history.
        let mut prune = interval(Duration::from_secs(600));
        prune.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut stop_signals = StopSignals::new();

        loop {
            let notified = async {
                match changes.as_mut() {
                    Some(rx) => {
                        if rx.changed().await.is_err() {
                            std::future::pending::<()>().await; // watcher died: timer still polls
                        }
                    }
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::select! {
                _ = poll.tick() => self.capture_clipboard(),
                _ = notified => self.capture_clipboard(),
                _ = prune.tick() => self.sync.history.prune_expired(),
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
