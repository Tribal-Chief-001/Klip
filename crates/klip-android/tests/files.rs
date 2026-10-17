//! File transfer through the Android-facing API: two engines pair over real iroh endpoints (LAN only,
//! discovery off) and send files from a raw descriptor, as the app does with the Share sheet.

use klip_android::{
    is_valid_pairing_ticket, ClipboardBridge, KlipEngine, TransferStateView, TransferView,
};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// (path, name, size, from, rel_dir)
type Received = (String, String, u64, String, String);

#[derive(Default)]
struct Bridge {
    files: Mutex<Vec<Received>>,
}

impl ClipboardBridge for Bridge {
    fn write_text(&self, _text: String, _sensitive: bool) -> bool {
        true
    }
    fn write_image(&self, _data: Vec<u8>, _mime: String) -> bool {
        true
    }
    fn file_received(&self, path: String, name: String, size: u64, from: String, rel_dir: String) {
        self.files
            .lock()
            .unwrap()
            .push((path, name, size, from, rel_dir));
    }
}

fn data_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("klip-android-files-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.json"), r#"{"lan_discovery": false}"#).unwrap();
    dir
}

fn engine(
    tag: &str,
    name: &str,
    accept: bool,
    max: u64,
) -> (Arc<KlipEngine>, Arc<Bridge>, PathBuf) {
    let bridge = Arc::new(Bridge::default());
    let dir = data_dir(tag);
    let e = KlipEngine::start(
        dir.to_string_lossy().into_owned(),
        name.to_string(),
        false,
        bridge.clone(),
        accept,
        max,
    )
    .expect("engine starts");
    (e, bridge, dir)
}

fn wait_until(secs: u64, mut cond: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    cond()
}

fn paired(
    tag: &str,
    accept: bool,
    max: u64,
) -> (Arc<KlipEngine>, Arc<KlipEngine>, Arc<Bridge>, PathBuf) {
    let (phone, _pb, _pd) = engine(&format!("{tag}-p"), "Phone", true, 2 << 30);
    let (laptop, lb, ld) = engine(&format!("{tag}-l"), "Laptop", accept, max);
    let ticket = laptop.pair_start(60, true).unwrap();
    assert!(is_valid_pairing_ticket(ticket.ticket.clone()));
    let p2 = Arc::clone(&phone);
    let join = std::thread::spawn(move || p2.pair_join(ticket.ticket));
    assert!(wait_until(10, || laptop.pair_poll().pending.is_some()));
    assert!(laptop.pair_decide(true));
    join.join().unwrap().unwrap();
    assert!(wait_until(30, || phone
        .status()
        .peers
        .iter()
        .any(|p| p.online)
        && laptop.status().peers.iter().any(|p| p.online)));
    (phone, laptop, lb, ld)
}

fn sample(tag: &str, bytes: usize) -> (PathBuf, std::fs::File) {
    let path = std::env::temp_dir().join(format!("klip-android-src-{}-{tag}", std::process::id()));
    std::fs::write(
        &path,
        (0..bytes)
            .map(|i| (i * 31 % 251) as u8)
            .collect::<Vec<u8>>(),
    )
    .unwrap();
    let file = std::fs::File::open(&path).unwrap();
    (path, file)
}

fn finished(t: &TransferView) -> bool {
    !matches!(
        t.state,
        TransferStateView::Waiting | TransferStateView::Active
    )
}

#[test]
fn a_file_sent_from_a_descriptor_reaches_the_other_engine_and_the_bridge_is_told() {
    let (phone, _laptop, bridge, dir) = paired("fd", true, 4 << 30);
    let (src, file) = sample("fd", 3_000_000);
    let started = phone
        .send_file_fd(file.as_raw_fd(), "Holiday photo.jpg".into())
        .unwrap();
    drop(file); // the app closes its descriptor as soon as the call returns
    assert_eq!(started.len(), 1);
    assert!(started[0].outgoing && started[0].peer == "Laptop");
    assert!(
        wait_until(30, || !bridge.files.lock().unwrap().is_empty()),
        "bridge told"
    );
    let (path, name, size, from, rel) = bridge.files.lock().unwrap()[0].clone();
    assert_eq!(
        (name.as_str(), size, from.as_str(), rel.as_str()),
        ("Holiday photo.jpg", 3_000_000, "Phone", "")
    );
    assert!(
        path.starts_with(dir.join("incoming").to_str().unwrap()),
        "{path}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), std::fs::read(&src).unwrap());
    assert!(wait_until(10, || phone.transfers().iter().all(finished)));
    assert_eq!(phone.transfers()[0].state, TransferStateView::Done);
}

#[test]
fn receiving_can_be_switched_off_and_says_why() {
    let (phone, _laptop, bridge, _d) = paired("off", false, 4 << 30);
    let (_src, file) = sample("off", 1000);
    phone
        .send_file_fd(file.as_raw_fd(), "a.txt".into())
        .unwrap();
    assert!(wait_until(20, || phone.transfers().iter().all(finished)
        && !phone.transfers().is_empty()));
    let t = &phone.transfers()[0];
    assert_eq!(t.state, TransferStateView::Failed);
    assert!(
        t.error.as_deref().unwrap().contains("not accepting files"),
        "{t:?}"
    );
    assert!(bridge.files.lock().unwrap().is_empty());
}

#[test]
fn the_size_limit_is_enforced() {
    let (phone, _laptop, _bridge, _d) = paired("limit", true, 1 << 20);
    let (_src, file) = sample("limit", 3 << 20);
    phone
        .send_file_fd(file.as_raw_fd(), "big.bin".into())
        .unwrap();
    assert!(wait_until(20, || phone.transfers().iter().any(finished)));
    let t = &phone.transfers()[0];
    assert_eq!(t.state, TransferStateView::Failed);
    assert!(
        t.error.as_deref().unwrap().contains("accepts at most"),
        "{t:?}"
    );
}

#[test]
fn cancel_works_and_an_unknown_id_is_false() {
    let (phone, _laptop, _bridge, _d) = paired("cancel", true, 4 << 30);
    assert!(!phone.cancel_transfer(vec![9; 16]));
    assert!(
        !phone.cancel_transfer(vec![1, 2, 3]),
        "a wrong-length id is just false"
    );
    let path = std::env::temp_dir().join(format!("klip-android-sparse-{}", std::process::id()));
    std::fs::File::create(&path)
        .unwrap()
        .set_len(2 << 30)
        .unwrap();
    let file = std::fs::File::open(&path).unwrap();
    let started = phone
        .send_file_fd(file.as_raw_fd(), "huge.bin".into())
        .unwrap();
    assert!(wait_until(30, || phone
        .transfers()
        .iter()
        .any(|t| t.done > 10 << 20)));
    assert!(phone.cancel_transfer(started[0].id.clone()));
    assert!(wait_until(20, || phone.transfers().iter().all(finished)));
    assert_eq!(phone.transfers()[0].state, TransferStateView::Cancelled);
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_descriptor_that_is_not_a_regular_file_is_refused_with_a_clear_message() {
    let (phone, _laptop, _bridge, _d) = paired("pipe", true, 4 << 30);
    let null = std::fs::File::open("/dev/null").unwrap();
    let err = phone
        .send_file_fd(null.as_raw_fd(), "x".into())
        .unwrap_err()
        .to_string();
    assert!(err.contains("regular"), "{err}");
    assert!(
        phone.send_file_fd(-1, "x".into()).is_err(),
        "a bad descriptor is an error, not a crash"
    );
}
