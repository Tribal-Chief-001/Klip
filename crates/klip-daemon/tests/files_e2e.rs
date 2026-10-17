//! File transfer between two complete daemons (real iroh endpoints, keystores, control sockets and
//! in-memory clipboards) inside one process. LAN mode with discovery off: nothing leaves the machine.
#![cfg(unix)]

use klip_crypto::identity::KeyStore;
use klip_daemon::{DaemonConfig, DaemonEngine, KlipPaths, RelayMode};
use klip_platform::memory::MemoryClipboardDriver;
use klip_proto::ipc::{
    deserialize_ipc_msg, serialize_ipc_msg, IpcRequest, IpcResponse, PairPollInfo,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::task::JoinHandle;

struct TestDaemon {
    engine: Arc<DaemonEngine>,
    clipboard: Arc<MemoryClipboardDriver>,
    task: JoinHandle<()>,
    dir: PathBuf,
}

impl TestDaemon {
    fn sock(&self) -> PathBuf {
        self.dir.join("daemon.sock")
    }

    async fn ipc(&self, req: IpcRequest) -> IpcResponse {
        ipc(&self.sock(), req).await
    }

    async fn stop(self) {
        let _ = self.ipc(IpcRequest::StopDaemon).await;
        let _ = tokio::time::timeout(Duration::from_secs(10), self.task).await;
    }
}

fn fresh_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("klip-e2e-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

async fn start_daemon(name: &str, dir: PathBuf, lan_discovery: bool) -> TestDaemon {
    start_daemon_with(name, dir, lan_discovery, |_| {}).await
}

async fn start_daemon_with(
    name: &str,
    dir: PathBuf,
    lan_discovery: bool,
    tweak: impl FnOnce(&mut DaemonConfig),
) -> TestDaemon {
    let paths = KlipPaths::from_dir(&dir);
    let (_ks, identity) = KeyStore::load_or_create(paths.keystore(), name).unwrap();
    let clipboard = Arc::new(MemoryClipboardDriver::new(identity.node_id));
    let mut config = DaemonConfig {
        device_name: name.to_string(),
        relay: RelayMode::Lan,
        poll_interval_ms: 20,
        lan_discovery,
        ..DaemonConfig::default()
    };
    tweak(&mut config);
    let engine = Arc::new(
        DaemonEngine::init_with(config, paths, Some(clipboard.clone()))
            .await
            .expect("daemon init"),
    );
    let runner = Arc::clone(&engine);
    let task = tokio::spawn(async move {
        let _ = runner.run().await;
    });
    // Wait for the control socket.
    let sock = dir.join("daemon.sock");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !sock.exists() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(sock.exists(), "control socket did not appear");
    TestDaemon {
        engine,
        clipboard,
        task,
        dir,
    }
}

async fn ipc(sock: &Path, req: IpcRequest) -> IpcResponse {
    let mut s = UnixStream::connect(sock).await.expect("connect ipc");
    let body = serialize_ipc_msg(&req).unwrap();
    s.write_all(&(body.len() as u32).to_be_bytes())
        .await
        .unwrap();
    s.write_all(&body).await.unwrap();
    let mut len = [0u8; 4];
    s.read_exact(&mut len).await.unwrap();
    let mut buf = vec![0u8; u32::from_be_bytes(len) as usize];
    s.read_exact(&mut buf).await.unwrap();
    deserialize_ipc_msg(&buf).unwrap()
}

async fn wait_until<F: FnMut() -> bool>(secs: u64, mut cond: F) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    cond()
}

async fn poll_pair(d: &TestDaemon) -> PairPollInfo {
    match d.ipc(IpcRequest::PairPoll).await {
        IpcResponse::PairPoll(i) => i,
        other => panic!("unexpected {other:?}"),
    }
}

/// Run the full pairing ceremony: `host` shows a ticket, `joiner` redeems it, host approves.
async fn pair(host: &TestDaemon, joiner: &TestDaemon, approve: bool) -> IpcResponse {
    let ticket = match host
        .ipc(IpcRequest::PairStart {
            ttl_secs: 60,
            include_lan_addrs: true,
        })
        .await
    {
        IpcResponse::PairTicket { ticket, .. } => ticket,
        other => panic!("unexpected {other:?}"),
    };
    let joiner_sock = joiner.sock();
    let join =
        tokio::spawn(async move { ipc(&joiner_sock, IpcRequest::PairJoin { ticket }).await });

    // The host sees the request and decides.
    let mut pending = None;
    for _ in 0..200 {
        let info = poll_pair(host).await;
        if info.pending.is_some() {
            pending = info.pending;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let pending = pending.expect("host should see a pairing request");
    assert_eq!(pending.device_name, joiner.engine.config.device_name);
    assert_eq!(pending.fingerprint, joiner.engine.node_id().fingerprint());
    host.ipc(IpcRequest::PairDecide { accept: approve }).await;
    join.await.unwrap()
}

use klip_proto::ipc::{TransferDirection, TransferInfo, TransferState};

/// A started pair of daemons: `a` sends, `b` receives into `b_dir/incoming` (with `tweak_b` applied).
async fn two_paired(
    tag: &str,
    tweak_b: impl FnOnce(&mut DaemonConfig),
) -> (TestDaemon, TestDaemon, PathBuf) {
    let a = start_daemon("Alpha", fresh_dir(&format!("{tag}-a")), false).await;
    let b_dir = fresh_dir(&format!("{tag}-b"));
    let incoming = b_dir.join("incoming");
    let incoming_for_config = incoming.display().to_string();
    let b = start_daemon_with("Beta", b_dir, false, move |c| {
        c.receive_dir = Some(incoming_for_config);
        tweak_b(c);
    })
    .await;
    assert!(matches!(
        pair(&a, &b, true).await,
        IpcResponse::PairSuccess { .. }
    ));
    // Both ends must hold the other's connection before a file goes (the sender checks it).
    let (a2, b2) = (a.engine.clone(), b.engine.clone());
    assert!(
        wait_until(20, || {
            let up = |e: &Arc<DaemonEngine>| e.sync.net.peer_statuses().iter().any(|s| s.connected);
            up(&a2) && up(&b2)
        })
        .await,
        "the devices connect"
    );
    (a, b, incoming)
}

fn make_file(dir: &Path, name: &str, bytes: usize) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    let data: Vec<u8> = (0..bytes)
        .map(|i| (i.wrapping_mul(131) % 251) as u8)
        .collect();
    std::fs::write(&path, data).unwrap();
    path
}

/// A big file that costs no disk space (reads as zeros).
fn make_sparse(dir: &Path, name: &str, bytes: u64) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::File::create(&path)
        .unwrap()
        .set_len(bytes)
        .unwrap();
    path
}

async fn wait_transfer(
    d: &TestDaemon,
    secs: u64,
    what: &str,
    cond: impl Fn(&TransferInfo) -> bool,
) -> TransferInfo {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(t) = d
            .engine
            .sync
            .transfers()
            .into_iter()
            .rev()
            .find(|t| cond(t))
        {
            return t;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {:?}",
            d.engine.sync.transfers()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn finished(t: &TransferInfo) -> bool {
    !matches!(t.state, TransferState::Waiting | TransferState::Active)
}

async fn send(a: &TestDaemon, path: &Path) -> TransferInfo {
    let mut answers = a.engine.sync.send_file(path.to_path_buf(), None).await;
    assert_eq!(answers.len(), 1, "{answers:?}");
    answers.remove(0).expect("the transfer starts")
}

fn files_in(dir: &Path) -> Vec<String> {
    match std::fs::read_dir(dir) {
        Ok(rd) => {
            let mut v: Vec<String> = rd
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        }
        Err(_) => Vec::new(),
    }
}

#[tokio::test]
async fn a_file_goes_between_two_daemons_byte_for_byte() {
    let (a, b, b_incoming) = two_paired("both", |_| {}).await;
    let src = a.dir.join("out");
    let big = make_file(&src, "holiday photo (1).jpg", 24 * 1024 * 1024);
    let started = send(&a, &big).await;
    assert_eq!(
        (started.direction, started.peer.as_str()),
        (TransferDirection::Sending, "Beta")
    );
    let done = wait_transfer(&a, 60, "the send to finish", finished).await;
    assert_eq!(done.state, TransferState::Done, "{done:?}");
    assert_eq!(done.done, 24 * 1024 * 1024);
    let received = b_incoming.join("holiday photo (1).jpg");
    assert_eq!(
        std::fs::read(&received).unwrap(),
        std::fs::read(&big).unwrap()
    );
    let theirs = wait_transfer(&b, 10, "Beta's record", finished).await;
    assert_eq!(
        (theirs.state, theirs.direction),
        (TransferState::Done, TransferDirection::Receiving)
    );
    assert_eq!(theirs.saved_to.as_deref(), Some(received.to_str().unwrap()));
    assert_eq!(
        files_in(&b_incoming),
        ["holiday photo (1).jpg"],
        "no partial file is left"
    );

    // The same name again: a second copy, the first untouched.
    send(&a, &big).await;
    wait_transfer(&a, 60, "the second send", |t| {
        finished(t) && t.id != done.id
    })
    .await;
    assert_eq!(
        files_in(&b_incoming),
        ["holiday photo (1) (2).jpg", "holiday photo (1).jpg"]
    );
}

#[tokio::test]
async fn a_device_that_does_not_accept_files_says_why() {
    let (a, b, incoming) = two_paired("off", |c| c.accept_files = false).await;
    let file = make_file(&a.dir.join("out"), "a.txt", 5000);
    send(&a, &file).await;
    let t = wait_transfer(&a, 20, "the refusal", finished).await;
    assert_eq!(t.state, TransferState::Failed);
    assert!(t.error.unwrap().contains("not accepting files"));
    assert!(files_in(&incoming).is_empty());
    drop(b);
}

#[tokio::test]
async fn a_file_over_the_receivers_limit_is_refused_before_it_is_sent() {
    let (a, _b, incoming) = two_paired("limit", |c| c.max_file_bytes = 1024 * 1024).await;
    let file = make_file(&a.dir.join("out"), "big.bin", 3 * 1024 * 1024);
    send(&a, &file).await;
    let t = wait_transfer(&a, 20, "the refusal", finished).await;
    assert_eq!(t.state, TransferState::Failed);
    assert!(
        t.error.as_deref().unwrap().contains("accepts at most"),
        "{t:?}"
    );
    assert_eq!(t.done, 0, "not a byte of it was sent");
    assert!(files_in(&incoming).is_empty());
}

#[tokio::test]
async fn cancelling_stops_a_running_transfer_and_leaves_nothing_behind() {
    let (a, b, incoming) = two_paired("cancel", |_| {}).await;
    let huge = make_sparse(&a.dir.join("out"), "huge.bin", 2 * 1024 * 1024 * 1024);
    let started = send(&a, &huge).await;
    wait_transfer(&a, 30, "bytes to move", |t| t.done > 20 * 1024 * 1024).await;
    assert!(a.engine.sync.cancel_transfer(started.id));
    let mine = wait_transfer(&a, 20, "the cancel", finished).await;
    assert_eq!(mine.state, TransferState::Cancelled, "{mine:?}");
    let theirs = wait_transfer(&b, 20, "the receiver to notice", finished).await;
    assert!(
        matches!(
            theirs.state,
            TransferState::Failed | TransferState::Cancelled
        ),
        "{theirs:?}"
    );
    assert!(
        wait_until(10, || files_in(&incoming).is_empty()).await,
        "{:?}",
        files_in(&incoming)
    );
}

#[tokio::test]
async fn unpairing_in_the_middle_stops_the_transfer() {
    let (a, b, incoming) = two_paired("unpair", |_| {}).await;
    let huge = make_sparse(&a.dir.join("out"), "huge.bin", 2 * 1024 * 1024 * 1024);
    send(&a, &huge).await;
    wait_transfer(&a, 30, "bytes to move", |t| t.done > 20 * 1024 * 1024).await;
    a.engine
        .sync
        .unpair(&b.engine.node_id().fingerprint())
        .await
        .unwrap();
    let mine = wait_transfer(&a, 20, "the transfer to stop", finished).await;
    assert!(
        matches!(mine.state, TransferState::Cancelled | TransferState::Failed),
        "{mine:?}"
    );
    assert!(
        wait_until(15, || files_in(&incoming).is_empty()).await,
        "{:?}",
        files_in(&incoming)
    );
}

#[tokio::test]
async fn clips_still_arrive_while_a_big_file_is_moving() {
    let (a, b, _incoming) = two_paired("clips", |_| {}).await;
    let huge = make_sparse(&a.dir.join("out"), "huge.bin", 2 * 1024 * 1024 * 1024);
    let started = send(&a, &huge).await;
    wait_transfer(&a, 30, "bytes to move", |t| t.done > 20 * 1024 * 1024).await;
    a.clipboard.user_copy("copied while a file is moving");
    assert!(
        wait_until(5, || b.clipboard.current_text().as_deref()
            == Some("copied while a file is moving"))
        .await,
        "the clip must not wait for the file"
    );
    assert!(a.engine.sync.cancel_transfer(started.id));
    wait_transfer(&a, 20, "the cancel", finished).await;
}

#[tokio::test]
async fn a_hostile_name_and_a_lying_header_from_a_raw_stream_do_no_harm() {
    use klip_net::file::{send_file, FileHeader};
    let (a, b, incoming) = two_paired("hostile", |_| {}).await;
    let b_id = a.engine.sync.net.peers()[0].id;

    // A hostile name with honest data: it lands inside the folder under a safe name.
    let mut streams = a.engine.sync.net.open_file_stream(b_id).await.unwrap();
    let data = vec![7u8; 4096];
    let header = FileHeader {
        version: 1,
        id: [1; 16],
        name: "../../evil.sh".into(),
        size: 4096,
    };
    send_file(
        &header,
        &data[..],
        &mut streams.tx,
        &mut streams.rx,
        &mut |_| {},
    )
    .await
    .unwrap();
    streams.close();
    assert_eq!(files_in(&incoming), ["evil.sh"]);
    assert!(!b.dir.join("evil.sh").exists() && !b.dir.parent().unwrap().join("evil.sh").exists());

    // A header that promises 5 MB and delivers 1 KB: nothing is kept.
    let mut streams = a.engine.sync.net.open_file_stream(b_id).await.unwrap();
    let header = FileHeader {
        version: 1,
        id: [2; 16],
        name: "liar.bin".into(),
        size: 5_000_000,
    };
    let r = send_file(
        &header,
        &vec![1u8; 1000][..],
        &mut streams.tx,
        &mut streams.rx,
        &mut |_| {},
    )
    .await;
    assert!(r.is_err());
    streams.close();
    assert!(
        wait_until(10, || files_in(&incoming) == ["evil.sh"]).await,
        "{:?}",
        files_in(&incoming)
    );
    let liar = wait_transfer(&b, 10, "the liar's record", |t| {
        t.name == "liar.bin" && finished(t)
    })
    .await;
    assert_eq!(liar.state, TransferState::Failed);
}

#[tokio::test]
async fn two_files_to_one_device_go_one_after_the_other() {
    let (a, b, incoming) = two_paired("two", |_| {}).await;
    let src = a.dir.join("out");
    let one = make_file(&src, "one.bin", 3_000_000);
    let two = make_file(&src, "two.bin", 3_000_000);
    let first = send(&a, &one).await;
    let second = send(&a, &two).await;
    assert_ne!(first.id, second.id);
    wait_transfer(&a, 60, "both sends", |t| t.id == second.id && finished(t)).await;
    wait_transfer(&a, 60, "the first send", |t| {
        t.id == first.id && finished(t)
    })
    .await;
    assert_eq!(files_in(&incoming), ["one.bin", "two.bin"]);
    drop(b);
}

#[tokio::test]
async fn an_offline_device_a_missing_file_and_an_unknown_name_are_reported_not_queued() {
    let (a, b, _incoming) = two_paired("errors", |_| {}).await;
    let missing = a.engine.sync.send_file(a.dir.join("nope.txt"), None).await;
    assert!(
        missing[0].as_ref().unwrap_err().contains("cannot send"),
        "{missing:?}"
    );
    let file = make_file(&a.dir.join("out"), "x.txt", 10);
    let nobody = a
        .engine
        .sync
        .send_file(file.clone(), Some("Nobody".into()))
        .await;
    assert!(
        nobody[0].as_ref().unwrap_err().contains("no paired device"),
        "{nobody:?}"
    );
    let dir = a.engine.sync.send_file(a.dir.clone(), None).await;
    assert!(dir[0].is_err(), "a folder cannot be sent: {dir:?}");
    b.stop().await;
    assert!(
        wait_until(30, || !a
            .engine
            .sync
            .net
            .peer_statuses()
            .iter()
            .any(|s| s.connected))
        .await,
        "Alpha notices Beta is gone"
    );
    let offline = a.engine.sync.send_file(file, None).await;
    assert!(
        offline[0].as_ref().unwrap_err().contains("not connected"),
        "{offline:?}"
    );
    assert!(
        a.engine.sync.transfers().iter().all(|t| t.name != "x.txt"),
        "nothing was queued"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn memory_stays_flat_while_a_gigabyte_moves() {
    fn rss_kib() -> u64 {
        std::fs::read_to_string("/proc/self/status")
            .unwrap()
            .lines()
            .find(|l| l.starts_with("VmRSS:"))
            .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
            .unwrap()
    }
    let (a, _b, incoming) = two_paired("memory", |_| {}).await;
    let big = make_sparse(&a.dir.join("out"), "gig.bin", 1024 * 1024 * 1024);
    let before = rss_kib();
    send(&a, &big).await;
    let mut peak = before;
    loop {
        peak = peak.max(rss_kib());
        if a.engine.sync.transfers().iter().all(finished) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(a.engine.sync.transfers()[0].state, TransferState::Done);
    assert_eq!(
        std::fs::metadata(incoming.join("gig.bin")).unwrap().len(),
        1024 * 1024 * 1024
    );
    let grew_mib = (peak - before) / 1024;
    assert!(
        grew_mib < 64,
        "the process grew by {grew_mib} MiB while a 1 GiB file moved"
    );
}
