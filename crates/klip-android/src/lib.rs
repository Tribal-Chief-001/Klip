//! Klip engine for Android.
//!
//! A thin, blocking API over [`klip_core::CoreEngine`] so the app and the desktop daemon run the
//! exact same pairing / sync / conflict-resolution code. All methods block the calling thread and
//! must be called from a background dispatcher (never the Android main thread).
//!
//! The app owns the system clipboard: it reports local copies with [`KlipEngine::on_local_clip`]
//! and implements [`ClipboardBridge`] so the engine can write remote clips.

use klip_core::{CoreEngine, DaemonConfig, KlipPaths, RelayMode};
use klip_net::PairingTicket;
use klip_platform::driver::{
    CapturedClip, ClipboardDriver, EchoShield, PlatformEnvironment, PlatformError,
};
use klip_proto::clip::{compute_fingerprint, ClipContentType};
use klip_proto::hlc::NodeId;
use std::sync::Arc;
use tokio::runtime::Runtime;

uniffi::setup_scaffolding!();

/// Everything that can go wrong, as a message the UI can show.
#[derive(Debug, uniffi::Error)]
pub enum KlipError {
    // Not named `message`: that would clash with kotlin.Exception.message in the generated bindings.
    Failed { reason: String },
}

impl std::fmt::Display for KlipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed { reason } => write!(f, "{reason}"),
        }
    }
}

impl std::error::Error for KlipError {}

impl KlipEngine {
    fn send(&self, content_type: ClipContentType, data: Vec<u8>) -> Result<SendReport, KlipError> {
        let reports = self
            .rt
            .block_on(self.core.sync.broadcast_local_clip(content_type, data))
            .ok_or_else(|| fail("sync is paused or the clip is empty"))?;
        let mut out = SendReport {
            delivered: vec![],
            failed: vec![],
        };
        for r in reports {
            match r.result {
                Ok(()) => out.delivered.push(r.peer_name),
                Err(reason) => out.failed.push(SendFailure {
                    device: r.peer_name,
                    reason,
                }),
            }
        }
        Ok(out)
    }
}

fn fail<E: std::fmt::Display>(e: E) -> KlipError {
    KlipError::Failed {
        reason: e.to_string(),
    }
}

/// Implemented by the app: writes a remote clip into the Android system clipboard.
#[uniffi::export(with_foreign)]
pub trait ClipboardBridge: Send + Sync {
    /// Return `true` if the text is now on the clipboard. `sensitive` asks the app to set
    /// `ClipDescription.EXTRA_IS_SENSITIVE` so keyboards do not show a preview.
    fn write_text(&self, text: String, sensitive: bool) -> bool;

    /// Return `true` if the image is now on the clipboard. `mime` is `image/png` or `image/webp`;
    /// the engine has already checked the magic bytes and the size in the header.
    fn write_image(&self, data: Vec<u8>, mime: String) -> bool;
}

#[derive(uniffi::Record)]
pub struct PeerInfo {
    pub fingerprint: String,
    pub name: String,
    pub online: bool,
    pub last_error: Option<String>,
    /// How long the device has been gone ("5 min", "2 h"); `None` while connected or under a minute.
    pub offline: Option<String>,
}

#[derive(uniffi::Record)]
pub struct StatusInfo {
    pub device_fingerprint: String,
    pub relay_mode: String,
    pub relay_state: String,
    pub paused: bool,
    pub pairing_open: bool,
    pub peers: Vec<PeerInfo>,
    pub last_sync_secs_ago: Option<u64>,
    pub last_clip_preview: Option<String>,
}

#[derive(uniffi::Record)]
pub struct PairTicketInfo {
    pub ticket: String,
    pub expires_in_secs: u64,
    pub device_fingerprint: String,
}

#[derive(uniffi::Record)]
pub struct PendingPairing {
    pub device_name: String,
    pub fingerprint: String,
}

#[derive(uniffi::Record)]
pub struct PairingProgress {
    pub window_open: bool,
    pub seconds_left: u64,
    pub pending: Option<PendingPairing>,
    pub paired_device: Option<String>,
}

#[derive(uniffi::Record)]
pub struct PairedDevice {
    pub name: String,
    pub fingerprint: String,
}

#[derive(uniffi::Record)]
pub struct SendFailure {
    pub device: String,
    pub reason: String,
}

#[derive(uniffi::Record)]
pub struct SendReport {
    pub delivered: Vec<String>,
    pub failed: Vec<SendFailure>,
}

#[derive(uniffi::Record)]
pub struct HistoryEntry {
    pub from_device: String,
    pub timestamp_ms: u64,
    pub preview: String,
    pub size_bytes: u64,
    pub sensitive: bool,
}

/// Is this text a well-formed Klip pairing ticket? (Cheap check for the QR scanner.) Expiry is
/// deliberately not judged here: only the host's clock decides, and a phone's clock may be wrong.
#[uniffi::export]
pub fn is_valid_pairing_ticket(text: String) -> bool {
    PairingTicket::decode(&text).is_ok()
}

/// Clipboard driver whose "OS clipboard" is the app, reached through [`ClipboardBridge`].
struct BridgeDriver {
    bridge: Arc<dyn ClipboardBridge>,
    echo: EchoShield,
    node: NodeId,
}

impl ClipboardDriver for BridgeDriver {
    fn environment(&self) -> PlatformEnvironment {
        PlatformEnvironment::MobileApp
    }

    // Capture is push-based (the app calls `on_local_clip`), so there is nothing to poll.
    fn read_clip(&self) -> Result<Option<CapturedClip>, PlatformError> {
        Ok(None)
    }

    fn write_clip(&self, content_type: &ClipContentType, data: &[u8]) -> Result<(), PlatformError> {
        self.write_clip_flagged(content_type, data, false)
    }

    fn write_clip_flagged(
        &self,
        content_type: &ClipContentType,
        data: &[u8],
        sensitive: bool,
    ) -> Result<(), PlatformError> {
        let image_mime = match content_type {
            ClipContentType::PlainTextUtf8 | ClipContentType::HtmlContent => None,
            ClipContentType::ImagePng => Some("image/png"),
            ClipContentType::ImageWebP => Some("image/webp"),
            // Custom types are not supported by the Android app.
            ClipContentType::CustomMime(_) => return Err(PlatformError::UnsupportedFormat),
        };
        if let Some(mime) = image_mime {
            self.echo
                .record_injection(compute_fingerprint(data, &self.node));
            return if self.bridge.write_image(data.to_vec(), mime.to_string()) {
                Ok(())
            } else {
                Err(PlatformError::ClipboardBusy)
            };
        }
        let text = String::from_utf8_lossy(data).into_owned();
        // Remember what we are about to write so the clipboard listener does not echo it back.
        self.echo
            .record_injection(compute_fingerprint(text.as_bytes(), &self.node));
        if self.bridge.write_text(text, sensitive) {
            Ok(())
        } else {
            Err(PlatformError::ClipboardBusy)
        }
    }

    fn echo_shield(&self) -> &EchoShield {
        &self.echo
    }
}

#[derive(uniffi::Object)]
pub struct KlipEngine {
    rt: Runtime,
    core: Arc<CoreEngine>,
    driver: Arc<BridgeDriver>,
}

#[uniffi::export]
impl KlipEngine {
    /// Start the engine. `data_dir` must be app-private storage. `use_relays` = use n0's public
    /// relays/discovery (works across networks); `false` = LAN only, no third-party servers.
    #[uniffi::constructor]
    pub fn start(
        data_dir: String,
        device_name: String,
        use_relays: bool,
        bridge: Arc<dyn ClipboardBridge>,
    ) -> Result<Arc<Self>, KlipError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("klip-rt")
            .build()
            .map_err(fail)?;
        let paths = KlipPaths::from_dir(data_dir);
        let mut config = DaemonConfig::load(&paths).map_err(fail)?;
        config.device_name = device_name;
        // Images (stored as files) can be big, text can not (Android's clipboard takes about 0.9 MB):
        // 12 MB keeps a hostile or accidental giant clip from being downloaded over mobile data.
        config.max_clip_bytes = 12_000_000;
        config.relay = if use_relays {
            RelayMode::N0
        } else {
            RelayMode::Lan
        };

        // The driver needs our device id, which the keystore (created on first run) decides.
        let (_ks, identity) =
            klip_crypto::identity::KeyStore::load_or_create(paths.keystore(), &config.device_name)
                .map_err(fail)?;
        let driver = Arc::new(BridgeDriver {
            bridge,
            echo: EchoShield::new(),
            node: identity.node_id,
        });
        let core = rt
            .block_on(CoreEngine::start(
                config,
                paths,
                Arc::clone(&driver) as Arc<dyn ClipboardDriver>,
            ))
            .map_err(fail)?;
        Ok(Arc::new(Self { rt, core, driver }))
    }

    pub fn device_fingerprint(&self) -> String {
        self.core.node_id().fingerprint()
    }

    pub fn status(&self) -> StatusInfo {
        let s = self.core.sync.status("Android".into(), 0);
        let offline: std::collections::HashMap<_, _> = self
            .core
            .sync
            .net
            .peer_statuses()
            .into_iter()
            .filter_map(|p| {
                let label = p.offline_for.and_then(klip_core::offline_label)?;
                Some((klip_core::node_of(&p.id), label))
            })
            .collect();
        StatusInfo {
            device_fingerprint: s.node_id.fingerprint(),
            relay_mode: s.relay_mode,
            relay_state: s.relay_state,
            paused: s.is_paused,
            pairing_open: s.pairing_open,
            peers: s
                .peers
                .into_iter()
                .map(|p| PeerInfo {
                    fingerprint: p.node_id.fingerprint(),
                    offline: offline.get(&p.node_id).cloned(),
                    name: p.name,
                    online: p.is_online,
                    last_error: p.last_error,
                })
                .collect(),
            last_sync_secs_ago: s.last_sync_time_secs_ago,
            last_clip_preview: s.last_clip_preview,
        }
    }

    // ------------------------------------------------------------------------- pairing

    /// Show a ticket (as QR/text) for another device to scan. Single use.
    pub fn pair_start(
        &self,
        ttl_secs: u64,
        include_lan_addrs: bool,
    ) -> Result<PairTicketInfo, KlipError> {
        let ttl = ttl_secs.clamp(30, 600);
        self.core.sync.mark_pairing_started(ttl);
        let ticket = self.rt.block_on(
            self.core
                .sync
                .net
                .open_pairing(std::time::Duration::from_secs(ttl), include_lan_addrs),
        );
        Ok(PairTicketInfo {
            expires_in_secs: ticket.seconds_left(),
            ticket: ticket.encode(),
            device_fingerprint: self.device_fingerprint(),
        })
    }

    pub fn pair_poll(&self) -> PairingProgress {
        let p = self.core.sync.poll_pairing();
        PairingProgress {
            window_open: p.window_open,
            seconds_left: p.seconds_left,
            pending: p.pending.map(|x| PendingPairing {
                device_name: x.device_name,
                fingerprint: x.fingerprint,
            }),
            paired_device: p.paired,
        }
    }

    /// Approve or decline the device that is waiting. `false` if nobody was waiting.
    pub fn pair_decide(&self, accept: bool) -> bool {
        self.core.sync.decide_pairing(accept)
    }

    pub fn pair_cancel(&self) {
        self.core.sync.cancel_pairing();
    }

    /// Redeem a ticket shown by another device. Blocks until the other side approves (up to 90 s).
    pub fn pair_join(&self, ticket: String) -> Result<PairedDevice, KlipError> {
        let ticket = PairingTicket::decode(&ticket).map_err(fail)?;
        let rec = self
            .rt
            .block_on(self.core.sync.net.pair_with(&ticket))
            .map_err(fail)?;
        self.rt
            .block_on(self.core.sync.persist_peer(&rec))
            .map_err(fail)?;
        Ok(PairedDevice {
            name: rec.name,
            fingerprint: klip_core::node_of(&rec.id).fingerprint(),
        })
    }

    pub fn unpair(&self, selector: String) -> Result<String, KlipError> {
        self.rt
            .block_on(self.core.sync.unpair(&selector))
            .map_err(fail)
    }

    // ---------------------------------------------------------------------------- sync

    /// The user copied something on this phone: send it in the background. Echoes of clips the
    /// engine itself just wrote are ignored.
    pub fn on_local_clip(&self, text: String) {
        if text.is_empty() {
            return;
        }
        let fp = compute_fingerprint(text.as_bytes(), &self.core.node_id());
        if self.driver.echo.should_suppress(&fp) {
            return;
        }
        let _guard = self.rt.enter();
        self.core
            .sync
            .spawn_local_clip(ClipContentType::PlainTextUtf8, text.into_bytes());
    }

    /// Send text now and report, per device, what happened (share sheet, tile, "send" button).
    pub fn send_text(&self, text: String) -> Result<SendReport, KlipError> {
        self.send(ClipContentType::PlainTextUtf8, text.into_bytes())
    }

    /// Send a PNG or WebP image now (the app converts everything else first). Checked before sending:
    /// magic bytes and the size in the header, so a hostile file never leaves the phone.
    pub fn send_image(&self, data: Vec<u8>) -> Result<SendReport, KlipError> {
        let (content_type, format) = match klip_diff::detect_image_format(&data) {
            klip_diff::ImageFormat::Png => (ClipContentType::ImagePng, klip_diff::ImageFormat::Png),
            klip_diff::ImageFormat::WebP => {
                (ClipContentType::ImageWebP, klip_diff::ImageFormat::WebP)
            }
            _ => return Err(fail("only PNG and WebP images can be sent")),
        };
        klip_diff::validate_image(format, &data).map_err(fail)?;
        self.send(content_type, data)
    }

    pub fn pause(&self, seconds: u64) {
        self.core.sync.pause(seconds);
    }

    pub fn resume(&self) {
        self.core.sync.resume();
    }

    pub fn history(&self, limit: u32) -> Vec<HistoryEntry> {
        self.core
            .sync
            .history
            .get_recent(limit as usize)
            .into_iter()
            .map(|h| HistoryEntry {
                from_device: h.origin_name,
                timestamp_ms: h.timestamp_ms,
                preview: h.content_preview,
                size_bytes: h.size_bytes as u64,
                sensitive: h.is_sensitive,
            })
            .collect()
    }

    /// Stop networking. The object must not be used afterwards.
    pub fn stop(&self) {
        self.rt.block_on(self.core.stop());
    }
}

/// Hands the Android application context to the networking stack.
///
/// iroh reads the phone's real DNS servers through JNI (`ConnectivityManager`), which needs a JVM and
/// an application context published to `ndk_context`. Without it iroh falls back to public DNS servers
/// (only possible because panics unwind; with `panic = "abort"` the process dies), and those can be
/// blocked on some networks. Kotlin calls this once, before the engine starts
/// (`KlipRuntime.installAndroidContext`).
#[cfg(target_os = "android")]
mod android_context {
    use jni::objects::{JClass, JObject};
    use jni::JNIEnv;
    use std::ffi::c_void;
    use std::sync::Once;

    #[no_mangle]
    pub extern "system" fn Java_com_klip_app_KlipRuntime_installAndroidContext(
        env: JNIEnv,
        _class: JClass,
        context: JObject,
    ) {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let Ok(vm) = env.get_java_vm() else { return };
            let Ok(global) = env.new_global_ref(&context) else {
                return;
            };
            let vm_ptr = vm.get_java_vm_pointer() as *mut c_void;
            let ctx_ptr = global.as_obj().as_raw() as *mut c_void;
            // Both pointers must stay valid until the process exits.
            std::mem::forget(global);
            unsafe { ndk_context::initialize_android_context(vm_ptr, ctx_ptr) };
        });
    }
}
