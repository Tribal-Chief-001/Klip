//! Exercises the Android-facing API on the host with a mock clipboard bridge: two engines pair
//! through the real ticket/approval flow and sync over real iroh endpoints (LAN mode, no
//! discovery, nothing leaves the machine).

use klip_android::{is_valid_pairing_ticket, ClipboardBridge, KlipEngine};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct MockBridge {
    writes: Mutex<Vec<(String, bool)>>,
    images: Mutex<Vec<(Vec<u8>, String)>>,
    fail: AtomicBool,
}

impl ClipboardBridge for MockBridge {
    fn write_text(&self, text: String, sensitive: bool) -> bool {
        if self.fail.load(Ordering::SeqCst) {
            return false;
        }
        self.writes.lock().unwrap().push((text, sensitive));
        true
    }

    fn write_image(&self, data: Vec<u8>, mime: String) -> bool {
        if self.fail.load(Ordering::SeqCst) {
            return false;
        }
        self.images.lock().unwrap().push((data, mime));
        true
    }
}

impl MockBridge {
    fn written(&self) -> Vec<(String, bool)> {
        self.writes.lock().unwrap().clone()
    }

    fn images(&self) -> Vec<(Vec<u8>, String)> {
        self.images.lock().unwrap().clone()
    }
}

fn data_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("klip-android-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Keep the test hermetic: no mDNS multicast.
    std::fs::write(dir.join("config.json"), r#"{"lan_discovery": false}"#).unwrap();
    dir
}

fn engine(tag: &str, name: &str) -> (Arc<KlipEngine>, Arc<MockBridge>) {
    let bridge = Arc::new(MockBridge::default());
    let e = KlipEngine::start(
        data_dir(tag).to_string_lossy().into_owned(),
        name.to_string(),
        false, // LAN only
        bridge.clone(),
    )
    .expect("engine starts");
    (e, bridge)
}

fn wait_until(secs: u64, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    cond()
}

fn pair(host: &Arc<KlipEngine>, joiner: &Arc<KlipEngine>, approve: bool) -> Result<String, String> {
    let ticket = host.pair_start(60, true).unwrap();
    assert!(is_valid_pairing_ticket(ticket.ticket.clone()));
    let j = Arc::clone(joiner);
    let handle = std::thread::spawn(move || j.pair_join(ticket.ticket).map(|d| d.name));
    assert!(
        wait_until(10, || host.pair_poll().pending.is_some()),
        "host never saw the request"
    );
    assert!(host.pair_decide(approve));
    handle.join().unwrap().map_err(|e| e.to_string())
}

#[test]
fn pair_and_sync_through_the_android_api() {
    let (phone, phone_clip) = engine("p1", "Phone");
    let (laptop, laptop_clip) = engine("l1", "Laptop");

    assert_eq!(pair(&laptop, &phone, true).unwrap(), "Laptop");
    // The host learns about the new device through its asynchronous event pump.
    let mut paired = None;
    assert!(wait_until(10, || {
        paired = laptop.pair_poll().paired_device;
        paired.is_some()
    }));
    assert_eq!(paired.as_deref(), Some("Phone"));
    assert_eq!(phone.status().peers.len(), 1);
    assert_eq!(phone.status().peers[0].name, "Laptop");

    // phone -> laptop (the app reports a local copy)
    phone.on_local_clip("hello from phone".into());
    assert!(wait_until(10, || laptop_clip
        .written()
        .iter()
        .any(|(t, _)| t == "hello from phone")));

    // The laptop's app sees its own clipboard change (caused by Klip) and reports it: ignored.
    laptop.on_local_clip("hello from phone".into());
    std::thread::sleep(Duration::from_millis(800));
    assert!(
        phone_clip.written().is_empty(),
        "an applied clip must not bounce back"
    );

    // laptop -> phone with an explicit send and a per-device report
    let report = laptop.send_text("from laptop".into()).unwrap();
    assert_eq!(report.delivered, vec!["Phone".to_string()]);
    assert!(report.failed.is_empty());
    assert!(wait_until(5, || phone_clip
        .written()
        .iter()
        .any(|(t, _)| t == "from laptop")));

    // Secrets are flagged so the app can set EXTRA_IS_SENSITIVE.
    let secret = format!("ghp_{}", "aB3dE6gH9jK2mN5pQ8sT1vW4yZ7cF0hJ3kL6");
    phone.on_local_clip(secret.clone());
    assert!(wait_until(10, || laptop_clip
        .written()
        .contains(&(secret.clone(), true))));

    // A bridge that cannot write is reported to the sender, honestly.
    laptop_clip.fail.store(true, Ordering::SeqCst);
    let report = phone.send_text("will not land".into()).unwrap();
    assert!(report.delivered.is_empty());
    assert!(
        report.failed[0].reason.contains("clipboard"),
        "{:?}",
        report.failed[0].reason
    );
    laptop_clip.fail.store(false, Ordering::SeqCst);

    // Pause blocks sending; history reflects traffic.
    phone.pause(600);
    assert!(phone.send_text("paused".into()).is_err());
    phone.resume();
    assert!(!phone.history(10).is_empty());

    // Unpair removes the device.
    assert_eq!(phone.unpair("Laptop".into()).unwrap(), "Laptop");
    assert!(phone.status().peers.is_empty());

    phone.stop();
    laptop.stop();
}

#[test]
fn declined_pairing_and_bad_tickets() {
    let (a, _) = engine("p2", "Phone");
    let (b, _) = engine("l2", "Laptop");
    assert!(pair(&b, &a, false).is_err());
    assert!(a.status().peers.is_empty() && b.status().peers.is_empty());

    assert!(!is_valid_pairing_ticket("garbage".into()));
    assert!(!is_valid_pairing_ticket(String::new()));
    assert!(a.pair_join("klip1notaticket".into()).is_err());
    assert!(a.send_text("x".into()).unwrap().delivered.is_empty());
    a.stop();
    b.stop();
}

/// The phone-to-phone flow as the app drives it: the host shows a code, a request arrives, the person
/// reads the fingerprint (seconds), then approves. A request must stay pending until then, and the
/// host must hear about the new device afterwards.
#[test]
fn a_waiting_request_survives_until_the_host_decides_and_the_host_is_told() {
    let (host, _) = engine("p6", "HostPhone");
    let (joiner, _) = engine("p7", "JoinPhone");
    let ticket = host.pair_start(60, true).unwrap();
    let j = Arc::clone(&joiner);
    let t = ticket.ticket.clone();
    let handle = std::thread::spawn(move || j.pair_join(t).map(|d| d.name));
    assert!(wait_until(10, || host.pair_poll().pending.is_some()));

    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        host.pair_poll().pending.is_some(),
        "a request waiting for approval must stay pending"
    );
    assert!(host.pair_decide(true));
    assert_eq!(handle.join().unwrap().unwrap(), "HostPhone");

    let mut told = None;
    assert!(
        wait_until(10, || {
            told = host.pair_poll().paired_device;
            told.is_some()
        }),
        "the host was never told about the new device"
    );
    assert_eq!(told.as_deref(), Some("JoinPhone"));
    host.stop();
    joiner.stop();
}

/// Cancelling pairing (what closing the code sheet does) declines a request that is still waiting.
/// The app must therefore never cancel as a side effect of showing the approval screen.
#[test]
fn cancelling_while_a_request_waits_declines_it() {
    let (host, _) = engine("p8", "HostPhone");
    let (joiner, _) = engine("p9", "JoinPhone");
    let ticket = host.pair_start(60, true).unwrap();
    let j = Arc::clone(&joiner);
    let handle = std::thread::spawn(move || j.pair_join(ticket.ticket).map(|d| d.name));
    assert!(wait_until(10, || host.pair_poll().pending.is_some()));
    host.pair_cancel();
    assert!(
        handle.join().unwrap().is_err(),
        "the waiting request is declined"
    );
    assert!(host.status().peers.is_empty());
    host.stop();
    joiner.stop();
}

fn paired_pair(
    tag: &str,
) -> (
    Arc<KlipEngine>,
    Arc<MockBridge>,
    Arc<KlipEngine>,
    Arc<MockBridge>,
) {
    let (phone, phone_clip) = engine(&format!("{tag}p"), "Phone");
    let (laptop, laptop_clip) = engine(&format!("{tag}l"), "Laptop");
    pair(&laptop, &phone, true).unwrap();
    assert!(wait_until(10, || laptop
        .pair_poll()
        .paired_device
        .is_some()));
    (phone, phone_clip, laptop, laptop_clip)
}

#[test]
fn an_image_sent_by_one_engine_reaches_the_other_bridge_with_its_mime_type() {
    let (phone, phone_clip, laptop, laptop_clip) = paired_pair("img1");
    let png = klip_diff::rgba_to_png(32, 24, &vec![77u8; 32 * 24 * 4]).unwrap();
    let report = phone.send_image(png.clone()).unwrap();
    assert_eq!(
        report.delivered,
        vec!["Laptop".to_string()],
        "{:?}",
        report.failed.first().map(|f| &f.reason)
    );
    assert!(wait_until(5, || !laptop_clip.images().is_empty()));
    assert_eq!(laptop_clip.images(), vec![(png, "image/png".to_string())]);

    let webp = klip_diff::rgba_to_webp(16, 16, &vec![5u8; 16 * 16 * 4]).unwrap();
    let report = laptop.send_image(webp.clone()).unwrap();
    assert_eq!(report.delivered, vec!["Phone".to_string()]);
    assert!(wait_until(5, || !phone_clip.images().is_empty()));
    assert_eq!(phone_clip.images(), vec![(webp, "image/webp".to_string())]);
    phone.stop();
    laptop.stop();
}

#[test]
fn only_real_png_and_webp_images_within_the_limits_are_sent() {
    let (phone, _pc, laptop, laptop_clip) = paired_pair("img2");
    // JPEG (and anything else) is not sent: the app converts to PNG first.
    let jpeg = vec![
        0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0,
    ];
    assert!(phone.send_image(jpeg).is_err());
    assert!(phone.send_image(Vec::new()).is_err());
    assert!(phone.send_image(b"not an image".to_vec()).is_err());

    // Bigger than the 12 MB limit (random pixels do not compress): refused with the limit in the message.
    let mut state = 0x1234_5678_9ABC_DEF0u64;
    let noise: Vec<u8> = (0..2300 * 2300 * 4)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect();
    let big = klip_diff::rgba_to_png(2300, 2300, &noise).unwrap();
    assert!(
        big.len() > 12_000_000,
        "test image is only {} bytes",
        big.len()
    );
    let report = phone.send_image(big).unwrap();
    assert!(report.delivered.is_empty());
    assert!(
        report.failed[0].reason.contains("limit"),
        "{:?}",
        report.failed[0].reason
    );
    assert!(laptop_clip.images().is_empty());
    phone.stop();
    laptop.stop();
}

#[test]
fn a_bridge_that_cannot_store_the_image_is_reported_to_the_sender() {
    let (phone, _pc, laptop, laptop_clip) = paired_pair("img3");
    laptop_clip.fail.store(true, Ordering::SeqCst);
    let png = klip_diff::rgba_to_png(8, 8, &vec![1u8; 8 * 8 * 4]).unwrap();
    let report = phone.send_image(png).unwrap();
    assert!(report.delivered.is_empty());
    assert!(
        report.failed[0].reason.contains("clipboard"),
        "{:?}",
        report.failed[0].reason
    );
    phone.stop();
    laptop.stop();
}

#[test]
fn an_image_whose_header_claims_too_many_pixels_never_leaves_the_phone() {
    let (phone, _pc, laptop, laptop_clip) = paired_pair("img4");
    // A real PNG with its size fields overwritten (CRC fixed): a few bytes that would decode to gigabytes.
    let mut bomb = klip_diff::rgba_to_png(2, 2, &[255u8; 16]).unwrap();
    bomb[16..20].copy_from_slice(&60_000u32.to_be_bytes());
    bomb[20..24].copy_from_slice(&60_000u32.to_be_bytes());
    let mut crc = 0xFFFF_FFFFu32;
    for b in &bomb[12..29] {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    bomb[29..33].copy_from_slice(&(!crc).to_be_bytes());
    let err = phone
        .send_image(bomb)
        .err()
        .expect("a bomb must be refused")
        .to_string();
    assert!(err.contains("pixels"), "{err}");
    std::thread::sleep(Duration::from_millis(300));
    assert!(laptop_clip.images().is_empty());
    phone.stop();
    laptop.stop();
}
