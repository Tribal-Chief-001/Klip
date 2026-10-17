//! Test peer for end-to-end runs against the Android emulator: a real daemon engine (own keystore,
//! real network) with an in-memory clipboard, so it never touches the real clipboard or the real
//! daemon. Not part of the product.
//!
//!   KLIP_CONFIG_DIR=<state> cargo run -p klip-daemon --example emulator_peer -- <work-dir>
//!
//! - writes the pairing ticket to `<work-dir>/ticket.txt` and approves every pairing request (not
//!   with `PEER_MANUAL=1`: then it does neither, and a controller such as the tray decides);
//! - `PEER_RELAY=lan` keeps everything on this machine; `PEER_NAME` sets the device name;
//! - copies `<work-dir>/inject.png` or `inject.webp` to its clipboard when the file appears
//!   (it is renamed to `*.sent`), which sends it to the paired phone;
//! - writes every image that arrives to `<work-dir>/received-<n>.<ext>`;
//! - receives files into `<work-dir>/files/`; every path listed (one per line) in `<work-dir>/send-file.txt`
//!   is sent to the paired phone (the file is renamed to `send-file.txt.sent`); every finished transfer is
//!   appended to `<work-dir>/files.log` as `<received|sent> <state> <size> <name>`.
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
        device_name: std::env::var("PEER_NAME").unwrap_or_else(|_| "EmuPeer".into()),
        relay: if std::env::var("PEER_RELAY").as_deref() == Ok("lan") {
            RelayMode::Lan
        } else {
            RelayMode::N0
        },
        lan_discovery: false,
        receive_dir: Some(work.join("files").display().to_string()),
        ..DaemonConfig::default()
    };
    let engine = Arc::new(DaemonEngine::init_with(config, paths, Some(clipboard.clone())).await?);
    let runner = Arc::clone(&engine);
    tokio::spawn(async move {
        let _ = runner.run().await;
    });

    let manual = std::env::var_os("PEER_MANUAL").is_some();
    if !manual {
        let ticket = engine
            .sync
            .net
            .open_pairing(Duration::from_secs(600), false)
            .await;
        engine.sync.mark_pairing_started(600);
        std::fs::write(work.join("ticket.txt"), ticket.encode())?;
        println!("ticket written to {}", work.join("ticket.txt").display());
    }
    println!("ready");

    let mut logged: std::collections::HashSet<[u8; 16]> = std::collections::HashSet::new();
    let mut received = 0u32;
    let mut last_seen: Option<[u8; 32]> = None;
    loop {
        if !manual && engine.sync.poll_pairing().pending.is_some() {
            engine.sync.decide_pairing(true);
            println!("approved a pairing request");
        }
        let list = work.join("send-file.txt");
        if list.exists() {
            let text = std::fs::read_to_string(&list)?;
            std::fs::rename(&list, work.join("send-file.txt.sent"))?;
            for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
                let answers = engine.sync.send_file(PathBuf::from(line), None).await;
                println!("send-file {line}: {answers:?}");
            }
        }
        for t in engine.sync.transfers() {
            use klip_proto::ipc::{TransferDirection, TransferState};
            if matches!(t.state, TransferState::Waiting | TransferState::Active)
                || !logged.insert(t.id)
            {
                continue;
            }
            let dir = if t.direction == TransferDirection::Sending {
                "sent"
            } else {
                "received"
            };
            let line = format!("{dir} {:?} {} {}\n", t.state, t.size, t.name);
            use std::io::Write as _;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(work.join("files.log"))?
                .write_all(line.as_bytes())?;
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
