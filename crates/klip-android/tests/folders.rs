//! Folders sent through the Android-facing API: the app lists a document tree, the engine asks for each file's
//! descriptor when its turn comes.
#![cfg(unix)]

use klip_android::{
    is_valid_pairing_ticket, ClipboardBridge, FolderFile, FolderSource, KlipEngine,
    TransferStateView, TransferView,
};
use std::os::fd::IntoRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// (path, name, size, from, rel_dir)
type Received = (String, String, u64, String, String);

#[derive(Default)]
struct Bridge {
    files: Mutex<Vec<Received>>,
}

impl ClipboardBridge for Bridge {
    fn write_text(&self, _t: String, _s: bool) -> bool {
        true
    }
    fn write_image(&self, _d: Vec<u8>, _m: String) -> bool {
        true
    }
    fn file_received(&self, path: String, name: String, size: u64, from: String, rel_dir: String) {
        self.files
            .lock()
            .unwrap()
            .push((path, name, size, from, rel_dir));
    }
    fn transfers_changed(&self) {}
}

/// Hands out descriptors of files on disk, by index; -1 for the ones listed in `refuse`.
struct Source {
    paths: Vec<PathBuf>,
    refuse: Vec<u32>,
    asked: AtomicUsize,
}

impl FolderSource for Source {
    fn open_fd(&self, index: u32) -> i32 {
        self.asked.fetch_add(1, Ordering::SeqCst);
        if self.refuse.contains(&index) {
            return -1;
        }
        std::fs::File::open(&self.paths[index as usize])
            .map(|f| f.into_raw_fd())
            .unwrap_or(-1)
    }
}

fn engine(tag: &str, name: &str) -> (Arc<KlipEngine>, Arc<Bridge>, PathBuf) {
    let bridge = Arc::new(Bridge::default());
    let dir =
        std::env::temp_dir().join(format!("klip-android-folders-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.json"), r#"{"lan_discovery": false}"#).unwrap();
    let e = KlipEngine::start(
        dir.to_string_lossy().into_owned(),
        name.into(),
        false,
        bridge.clone(),
        true,
        4 << 30,
    )
    .unwrap();
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

fn paired(tag: &str) -> (Arc<KlipEngine>, Arc<KlipEngine>, Arc<Bridge>, PathBuf) {
    let (phone, _pb, _) = engine(&format!("{tag}-p"), "Phone");
    let (laptop, lb, ld) = engine(&format!("{tag}-l"), "Laptop");
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

/// A few files on disk and the list the app would build for them.
fn sample(tag: &str, n: usize) -> (Vec<FolderFile>, Vec<PathBuf>, Vec<Vec<u8>>) {
    let root = std::env::temp_dir().join(format!(
        "klip-android-folders-src-{}-{tag}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let (mut list, mut paths, mut data) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..n {
        let bytes: Vec<u8> = (0..(2000 + i * 111))
            .map(|k| ((k + i) % 251) as u8)
            .collect();
        let path = root.join(format!("src{i}.bin"));
        std::fs::write(&path, &bytes).unwrap();
        let rel = match i % 3 {
            0 => vec![],
            1 => vec!["Day 1".to_string()],
            _ => vec!["Day 1".to_string(), "raw".to_string()],
        };
        list.push(FolderFile {
            rel,
            name: format!("f{i}.bin"),
            size: bytes.len() as u64,
        });
        paths.push(path);
        data.push(bytes);
    }
    (list, paths, data)
}

fn finished(t: &TransferView) -> bool {
    !matches!(
        t.state,
        TransferStateView::Waiting | TransferStateView::Active
    )
}

#[test]
fn a_folder_sent_from_descriptors_arrives_with_its_structure_and_the_bridge_is_told() {
    let (phone, _laptop, bridge, dir) = paired("a");
    let (list, paths, data) = sample("a", 6);
    let source = Arc::new(Source {
        paths,
        refuse: vec![],
        asked: AtomicUsize::new(0),
    });
    let rows = phone
        .send_folder("Trip".into(), list, source.clone())
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].outgoing && rows[0].name == "Trip" && rows[0].peer == "Laptop");
    assert!(wait_until(60, || phone.transfers().iter().all(finished)
        && !phone.transfers().is_empty()));
    assert_eq!(
        phone.transfers()[0].state,
        TransferStateView::Done,
        "{:?}",
        phone.transfers()[0]
    );
    assert_eq!(
        source.asked.load(Ordering::SeqCst),
        6,
        "each file was asked for once"
    );
    assert!(wait_until(20, || bridge.files.lock().unwrap().len() == 6));
    let staging = dir.join("incoming").join("Trip");
    for (i, bytes) in data.iter().enumerate() {
        let rel = match i % 3 {
            0 => format!("f{i}.bin"),
            1 => format!("Day 1/f{i}.bin"),
            _ => format!("Day 1/raw/f{i}.bin"),
        };
        assert_eq!(&std::fs::read(staging.join(&rel)).unwrap(), bytes, "{rel}");
    }
    let rel_dirs: std::collections::BTreeSet<String> = bridge
        .files
        .lock()
        .unwrap()
        .iter()
        .map(|f| f.4.clone())
        .collect();
    assert_eq!(
        rel_dirs.into_iter().collect::<Vec<_>>(),
        ["Trip", "Trip/Day 1", "Trip/Day 1/raw"],
        "the bridge learns each file's folder"
    );
}

#[test]
fn a_source_that_cannot_open_one_file_fails_only_that_file() {
    let (phone, _laptop, bridge, _dir) = paired("b");
    let (list, paths, _data) = sample("b", 5);
    let source = Arc::new(Source {
        paths,
        refuse: vec![2],
        asked: AtomicUsize::new(0),
    });
    phone.send_folder("Trip".into(), list, source).unwrap();
    assert!(wait_until(60, || phone.transfers().iter().all(finished)
        && !phone.transfers().is_empty()));
    let row = &phone.transfers()[0];
    assert_eq!(row.state, TransferStateView::Failed);
    assert!(
        row.error
            .as_deref()
            .unwrap()
            .starts_with("1 of 5 files failed"),
        "{row:?}"
    );
    assert!(wait_until(20, || bridge.files.lock().unwrap().len() == 4));
}

#[test]
fn a_list_over_the_limit_is_an_error_and_no_file_is_asked_for() {
    let (phone, _laptop, _bridge, _dir) = paired("c");
    let many: Vec<FolderFile> = (0..1001)
        .map(|i| FolderFile {
            rel: vec![],
            name: format!("f{i}"),
            size: 1,
        })
        .collect();
    let source = Arc::new(Source {
        paths: vec![],
        refuse: vec![],
        asked: AtomicUsize::new(0),
    });
    let err = phone
        .send_folder("Trip".into(), many, source.clone())
        .unwrap_err()
        .to_string();
    assert!(err.contains("1000"), "{err}");
    assert_eq!(source.asked.load(Ordering::SeqCst), 0);
}

#[test]
fn the_engine_closes_every_descriptor_it_was_handed() {
    let (phone, _laptop, bridge, _dir) = paired("d");
    let (list, paths, _data) = sample("d", 8);
    let open_fds = || {
        std::fs::read_dir("/proc/self/fd")
            .map(|d| d.count())
            .unwrap_or(0)
    };
    let before = open_fds();
    let source = Arc::new(Source {
        paths,
        refuse: vec![],
        asked: AtomicUsize::new(0),
    });
    phone.send_folder("Trip".into(), list, source).unwrap();
    assert!(wait_until(60, || phone.transfers().iter().all(finished)
        && !phone.transfers().is_empty()));
    assert!(wait_until(20, || bridge.files.lock().unwrap().len() == 8));
    std::thread::sleep(Duration::from_millis(500));
    let after = open_fds();
    assert!(
        after <= before + 4,
        "descriptors before {before}, after {after}: the 8 handed over are closed"
    );
}
