//! Test peer for end-to-end runs against the Android emulator: a real daemon engine (own keystore,
//! real network) with an in-memory clipboard, so it never touches the real clipboard or the real
//! daemon. Not part of the product.
//!
//!   KLIP_CONFIG_DIR=<state> cargo run -p klip-daemon --example emulator_peer -- <work-dir>
//!
//! - writes the pairing ticket to `<work-dir>/ticket.txt` and approves every pairing request;
//! - copies `<work-dir>/inject.png` or `inject.webp` to its clipboard when the file appears
//!   (it is renamed to `*.sent`), which sends it to the paired phone;
//! - writes every image that arrives to `<work-dir>/received-<n>.<ext>`.
use klip_daemon::{DaemonConfig, DaemonEngine, RelayMode};
use klip_platform::memory::MemoryClipboardDriver;
use klip_proto::clip::ClipContentType;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let work = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: emulator_peer <work-dir>"),
    );
    std::fs::create_dir_all(&work)?;
    let paths = klip_daemon::user_paths();
    let (_ks, identity) =
        klip_crypto::identity::KeyStore::load_or_create(paths.keystore(), "EmuPeer")?;
    let clipboard = Arc::new(MemoryClipboardDriver::new(identity.node_id));
    let config = DaemonConfig {
        device_name: "EmuPeer".into(),
        relay: RelayMode::N0,
        lan_discovery: false,
        ..DaemonConfig::default()
    };
    let engine = Arc::new(DaemonEngine::init_with(config, paths, Some(clipboard.clone())).await?);
    let runner = Arc::clone(&engine);
    tokio::spawn(async move {
        let _ = runner.run().await;
    });

    let ticket = engine
        .sync
        .net
        .open_pairing(Duration::from_secs(600), false)
        .await;
    engine.sync.mark_pairing_started(600);
    std::fs::write(work.join("ticket.txt"), ticket.encode())?;
    println!("ticket written to {}", work.join("ticket.txt").display());

    let mut received = 0u32;
    let mut last_seen: Option<[u8; 32]> = None;
    loop {
        if engine.sync.poll_pairing().pending.is_some() {
            engine.sync.decide_pairing(true);
            println!("approved a pairing request");
        }
        for (name, ct) in [
            ("inject.png", ClipContentType::ImagePng),
            ("inject.webp", ClipContentType::ImageWebP),
        ] {
            let path = work.join(name);
            if path.exists() {
                let data = std::fs::read(&path)?;
                std::fs::rename(&path, work.join(format!("{name}.sent")))?;
                println!("injecting {name} ({} bytes)", data.len());
                clipboard.user_copy_bytes(ct, data);
            }
        }
        if let Some(clip) = clipboard.current_clip() {
            let ext = match clip.content_type {
                ClipContentType::ImagePng => Some("png"),
                ClipContentType::ImageWebP => Some("webp"),
                _ => None,
            };
            if let Some(ext) = ext {
                // Only what arrived from the phone: what we injected ourselves is `.sent`.
                let ours =
                    work.join("inject.png.sent").exists() || work.join("inject.webp.sent").exists();
                if last_seen != Some(clip.fingerprint) {
                    last_seen = Some(clip.fingerprint);
                    let injected = ours
                        && [work.join("inject.png.sent"), work.join("inject.webp.sent")]
                            .iter()
                            .any(|p| std::fs::read(p).is_ok_and(|d| d == clip.data));
                    if !injected {
                        received += 1;
                        let out = work.join(format!("received-{received}.{ext}"));
                        std::fs::write(&out, &clip.data)?;
                        println!(
                            "received an image: {} ({} bytes)",
                            out.display(),
                            clip.data.len()
                        );
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}
