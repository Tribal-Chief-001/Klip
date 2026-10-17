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
    // These tests move gigabytes and kept partials stay on disk: clear what earlier runs left behind
    // (another process id in the name) so /tmp does not fill up, once per run.
    static CLEANED: std::sync::Once = std::sync::Once::new();
    CLEANED.call_once(|| {
        let mine = format!("klip-e2e-{}-", std::process::id());
        if let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with("klip-e2e-") && !name.starts_with(&mine) {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
    });
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

/// Names in `dir` that are not hidden partial files: what a person would see.
fn final_files_in(dir: &Path) -> Vec<String> {
    files_in(dir)
        .into_iter()
        .filter(|n| !n.starts_with(".klip-"))
        .collect()
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
    // A cut transfer leaves a hidden partial for a later resume, never a file under its real name.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        final_files_in(&incoming).is_empty(),
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
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        final_files_in(&incoming).is_empty(),
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
    use klip_net::file::{send_file_v2, FileHeaderV2};
    let (a, b, incoming) = two_paired("hostile", |_| {}).await;
    let b_id = a.engine.sync.net.peers()[0].id;

    // A hostile name with honest data: it lands inside the folder under a safe name.
    let mut streams = a.engine.sync.net.open_file_stream(b_id).await.unwrap();
    let data = vec![7u8; 4096];
    let header = FileHeaderV2 {
        version: 2,
        id: [1; 16],
        name: "../../evil.sh".into(),
        size: 4096,
        resume_key: [0; 32],
        folder: None,
    };
    send_file_v2(
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
    let header = FileHeaderV2 {
        version: 2,
        id: [2; 16],
        name: "liar.bin".into(),
        size: 5_000_000,
        resume_key: [0; 32],
        folder: None,
    };
    let r = send_file_v2(
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
    let empty = a.dir.join("empty-folder");
    std::fs::create_dir_all(&empty).unwrap();
    let dir = a.engine.sync.send_file(empty, None).await;
    assert!(
        dir[0].as_ref().unwrap_err().contains("no files"),
        "an empty folder is refused: {dir:?}"
    );
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

#[tokio::test]
async fn the_control_socket_lists_the_capability_and_starts_lists_and_cancels_transfers() {
    let (a, _b, incoming) = two_paired("ipc", |_| {}).await;
    let IpcResponse::Hello(hello) = a.ipc(IpcRequest::Hello { client_protocol: 2 }).await else {
        panic!("Hello")
    };
    assert!(hello.capabilities.iter().any(|c| c == "file_transfer"));
    assert_eq!(
        a.ipc(IpcRequest::GetTransfers).await,
        IpcResponse::Transfers(vec![])
    );

    let relative = a
        .ipc(IpcRequest::SendFile {
            path: "x.txt".into(),
            to: None,
        })
        .await;
    assert!(
        matches!(&relative, IpcResponse::Error(e) if e.contains("absolute")),
        "{relative:?}"
    );
    let missing = a
        .ipc(IpcRequest::SendFile {
            path: a.dir.join("nope").display().to_string(),
            to: None,
        })
        .await;
    assert!(
        matches!(&missing, IpcResponse::Error(e) if e.contains("cannot send")),
        "{missing:?}"
    );

    let file = make_file(&a.dir.join("out"), "viaipc.bin", 100_000);
    let IpcResponse::TransfersStarted(started) = a
        .ipc(IpcRequest::SendFile {
            path: file.display().to_string(),
            to: Some("Beta".into()),
        })
        .await
    else {
        panic!("TransfersStarted")
    };
    assert_eq!(started.len(), 1);
    wait_transfer(&a, 20, "the send", finished).await;
    let IpcResponse::Transfers(list) = a.ipc(IpcRequest::GetTransfers).await else {
        panic!()
    };
    assert_eq!((list.len(), list[0].state), (1, TransferState::Done));
    assert_eq!(
        std::fs::read(incoming.join("viaipc.bin")).unwrap(),
        std::fs::read(&file).unwrap()
    );
    assert_eq!(
        a.ipc(IpcRequest::CancelTransfer { id: [9; 16] }).await,
        IpcResponse::TransferCancelled(false)
    );
}

#[tokio::test]
async fn cancelling_a_file_that_is_still_waiting_ends_it_at_once() {
    let (a, _b, _incoming) = two_paired("waitcancel", |_| {}).await;
    let huge = make_sparse(&a.dir.join("out"), "huge.bin", 2 * 1024 * 1024 * 1024);
    let first = send(&a, &huge).await;
    wait_transfer(&a, 30, "bytes to move", |t| t.done > 20 * 1024 * 1024).await;
    let small = make_file(&a.dir.join("out"), "small.bin", 1000);
    let second = send(&a, &small).await;
    assert_eq!(second.state, TransferState::Waiting);
    assert!(a.engine.sync.cancel_transfer(second.id));
    wait_transfer(&a, 3, "the waiting one to be cancelled", |t| {
        t.id == second.id && t.state == TransferState::Cancelled
    })
    .await;
    assert!(a.engine.sync.cancel_transfer(first.id));
    wait_transfer(&a, 20, "the first cancel", |t| {
        t.id == first.id && finished(t)
    })
    .await;
}

#[tokio::test]
async fn too_many_waiting_files_are_refused_not_held_open() {
    let (a, _b, _incoming) = two_paired("cap", |_| {}).await;
    let huge = make_sparse(&a.dir.join("out"), "huge.bin", 2 * 1024 * 1024 * 1024);
    let first = send(&a, &huge).await;
    wait_transfer(&a, 30, "bytes to move", |t| t.done > 1024 * 1024).await;
    let small = make_file(&a.dir.join("out"), "s.bin", 10);
    let mut refused = 0;
    for _ in 0..70 {
        if a.engine.sync.send_file(small.clone(), None).await[0].is_err() {
            refused += 1;
        }
    }
    assert_eq!(refused, 6, "64 may wait, the other 6 are told why");
    a.engine.sync.cancel_transfer(first.id);
    for t in a.engine.sync.transfers() {
        a.engine.sync.cancel_transfer(t.id);
    }
}

#[tokio::test]
async fn a_device_that_cannot_be_reached_is_listed_next_to_the_ones_that_started() {
    let (a, b, _incoming) = two_paired("partial", |_| {}).await;
    let c = start_daemon("Gamma", fresh_dir("partial-c"), false).await;
    assert!(matches!(
        pair(&a, &c, true).await,
        IpcResponse::PairSuccess { .. }
    ));
    c.stop().await;
    assert!(
        wait_until(30, || {
            a.engine
                .sync
                .net
                .peer_statuses()
                .iter()
                .filter(|s| s.connected)
                .count()
                == 1
        })
        .await
    );
    let file = make_file(&a.dir.join("out"), "p.bin", 1000);
    let IpcResponse::TransfersStarted(list) = a
        .ipc(IpcRequest::SendFile {
            path: file.display().to_string(),
            to: None,
        })
        .await
    else {
        panic!("TransfersStarted")
    };
    assert_eq!(list.len(), 2, "{list:?}");
    let failed: Vec<_> = list
        .iter()
        .filter(|t| t.state == TransferState::Failed)
        .collect();
    assert_eq!(failed.len(), 1);
    assert!(
        failed[0]
            .error
            .as_deref()
            .unwrap()
            .contains("Gamma is not connected"),
        "{failed:?}"
    );
    drop(b);
}

#[tokio::test]
async fn stopping_the_receiving_daemon_keeps_a_resumable_partial_and_no_final_file() {
    let (a, b, incoming) = two_paired("stoprecv", |_| {}).await;
    let huge = make_sparse(&a.dir.join("out"), "huge.bin", 2 * 1024 * 1024 * 1024);
    send(&a, &huge).await;
    wait_transfer(&b, 30, "bytes to arrive", |t| t.done > 20 * 1024 * 1024).await;
    b.engine.core.stop().await;
    assert!(
        final_files_in(&incoming).is_empty(),
        "{:?}",
        files_in(&incoming)
    );
    assert_eq!(
        files_in(&incoming)
            .iter()
            .filter(|n| n.ends_with(".part"))
            .count(),
        1,
        "the daemon stopping is not the person's decision: the partial stays for a resume"
    );
    let mine = wait_transfer(&a, 30, "the sender to give up", finished).await;
    assert_eq!(mine.state, TransferState::Failed, "{mine:?}");
}

async fn send_open(
    a: &TestDaemon,
    file: std::fs::File,
    name: &str,
) -> Vec<Result<TransferInfo, String>> {
    a.engine
        .sync
        .send_open_file(file, name.to_string(), None)
        .await
}

#[tokio::test]
async fn an_open_file_is_sent_and_arrives_after_the_original_is_closed() {
    let (a, _b, incoming) = two_paired("openfile", |_| {}).await;
    let path = make_file(&a.dir.join("out"), "open.bin", 24 * 1024 * 1024);
    let f = std::fs::File::open(&path).unwrap();
    let answers = send_open(&a, f, "from-descriptor.bin").await; // `f` is moved in and dropped by the caller side
    assert!(answers[0].is_ok(), "{answers:?}");
    let done = wait_transfer(&a, 60, "the send", finished).await;
    assert_eq!(done.state, TransferState::Done, "{done:?}");
    assert_eq!(
        std::fs::read(incoming.join("from-descriptor.bin")).unwrap(),
        std::fs::read(&path).unwrap()
    );
}

#[tokio::test]
async fn one_open_file_sent_to_two_devices_arrives_intact_at_both() {
    let a = start_daemon("Alpha", fresh_dir("open2-a"), false).await;
    let mut receivers = Vec::new();
    for name in ["Beta", "Gamma"] {
        let dir = fresh_dir(&format!("open2-{name}"));
        let incoming = dir.join("incoming");
        let cfg = incoming.display().to_string();
        let d = start_daemon_with(name, dir, false, move |c| c.receive_dir = Some(cfg)).await;
        assert!(matches!(
            pair(&a, &d, true).await,
            IpcResponse::PairSuccess { .. }
        ));
        receivers.push((d, incoming));
    }
    let a2 = a.engine.clone();
    assert!(
        wait_until(30, || a2
            .sync
            .net
            .peer_statuses()
            .iter()
            .filter(|s| s.connected)
            .count()
            == 2)
        .await,
        "both devices connect"
    );
    let path = make_file(&a.dir.join("out"), "shared.bin", 12 * 1024 * 1024);
    let answers = send_open(&a, std::fs::File::open(&path).unwrap(), "shared.bin").await;
    assert_eq!(
        answers.iter().filter(|r| r.is_ok()).count(),
        2,
        "{answers:?}"
    );
    for (d, incoming) in &receivers {
        wait_transfer(d, 60, "the file", finished).await;
        assert_eq!(
            std::fs::read(incoming.join("shared.bin")).unwrap(),
            std::fs::read(&path).unwrap(),
            "{}",
            d.engine.config.device_name
        );
    }
}

#[tokio::test]
async fn a_non_regular_open_file_is_refused() {
    let (a, _b, incoming) = two_paired("openrefuse", |_| {}).await;
    let answers = send_open(&a, std::fs::File::open("/dev/null").unwrap(), "null").await;
    assert!(
        answers[0].as_ref().unwrap_err().contains("regular"),
        "{answers:?}"
    );
    assert!(files_in(&incoming).is_empty() && a.engine.sync.transfers().is_empty());
}

#[tokio::test]
async fn a_file_that_shrinks_while_sent_fails_and_leaves_nothing() {
    let (a, _b, incoming) = two_paired("openshrink", |_| {}).await;
    let huge = make_sparse(&a.dir.join("out"), "huge.bin", 2 * 1024 * 1024 * 1024);
    let started = send_open(&a, std::fs::File::open(&huge).unwrap(), "huge.bin").await;
    assert!(started[0].is_ok());
    wait_transfer(&a, 30, "bytes to move", |t| t.done > 20 * 1024 * 1024).await;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&huge)
        .unwrap()
        .set_len(1024 * 1024)
        .unwrap();
    let t = wait_transfer(&a, 30, "the failure", finished).await;
    assert_eq!(t.state, TransferState::Failed, "{t:?}");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        final_files_in(&incoming).is_empty(),
        "{:?}",
        files_in(&incoming)
    );
}

#[tokio::test]
async fn a_hostile_display_name_becomes_a_safe_name() {
    let (a, _b, incoming) = two_paired("openname", |_| {}).await;
    let path = make_file(&a.dir.join("out"), "n.bin", 100);
    for raw in ["../../evil.sh", "", &"x".repeat(400)] {
        let ans = send_open(&a, std::fs::File::open(&path).unwrap(), raw).await;
        assert!(ans[0].is_ok(), "{raw:?}: {ans:?}");
    }
    assert!(
        wait_until(30, || files_in(&incoming).len() == 3).await,
        "{:?}",
        files_in(&incoming)
    );
    for name in files_in(&incoming) {
        assert!(
            !name.contains('/') && !name.is_empty() && name.chars().count() <= 255,
            "{name}"
        );
    }
    assert!(files_in(&incoming).contains(&"evil.sh".to_string()));
    assert!(!a.dir.parent().unwrap().join("evil.sh").exists());
}

#[tokio::test]
async fn a_device_unpaired_after_it_connected_cannot_deliver_its_file() {
    use klip_net::file::{send_file_v2, FileError, FileHeaderV2};
    let (a, b, incoming) = two_paired("unpaired-late", |_| {}).await;
    let b_id = a.engine.sync.net.peers()[0].id;
    // A opens the file stream while it is still paired, then B unpairs A before the header goes.
    let mut streams = a.engine.sync.net.open_file_stream(b_id).await.unwrap();
    b.engine
        .sync
        .unpair(&a.engine.node_id().fingerprint())
        .await
        .unwrap();
    let data = vec![3u8; 10_000];
    let header = FileHeaderV2 {
        version: 2,
        id: [4; 16],
        name: "late.bin".into(),
        size: 10_000,
        resume_key: [0; 32],
        folder: None,
    };
    let r = send_file_v2(
        &header,
        &data[..],
        &mut streams.tx,
        &mut streams.rx,
        &mut |_| {},
    )
    .await;
    assert!(
        matches!(
            r,
            Err(FileError::Rejected { .. }) | Err(FileError::Io(_)) | Err(FileError::Peer(_))
        ),
        "{r:?}"
    );
    streams.close();
    assert!(files_in(&incoming).is_empty(), "{:?}", files_in(&incoming));
}

#[tokio::test]
async fn cutting_a_transfer_then_sending_the_same_file_again_resumes_and_arrives_equal() {
    let (a, b, incoming) = two_paired("resume", |_| {}).await;
    let path = make_file(&a.dir.join("out"), "movie.bin", 160 * 1024 * 1024);
    let first = send(&a, &path).await;
    let cut = wait_transfer(&a, 60, "bytes to move", |t| {
        t.id == first.id && t.done > 40 * 1024 * 1024
    })
    .await
    .done;
    assert!(a.engine.sync.cancel_transfer(first.id));
    wait_transfer(&a, 20, "the cut", |t| t.id == first.id && finished(t)).await;
    assert!(
        wait_until(15, || files_in(&incoming)
            .iter()
            .any(|n| n.starts_with(".klip-") && n.ends_with(".part")))
        .await,
        "the receiver keeps what arrived: {:?}",
        files_in(&incoming)
    );

    let second = send(&a, &path).await;
    // The first progress seen on the new transfer is the offset the receiver asked to continue from.
    let mut first_seen = 0;
    let end = Instant::now() + Duration::from_secs(30);
    while first_seen == 0 {
        first_seen = a
            .engine
            .sync
            .transfers()
            .iter()
            .find(|t| t.id == second.id)
            .map_or(0, |t| t.done);
        assert!(
            Instant::now() < end,
            "the second transfer never started moving"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(
        first_seen >= cut * 9 / 10,
        "resumed from {first_seen}, the cut was at {cut}"
    );
    let done = wait_transfer(&a, 90, "the second send", |t| {
        t.id == second.id && finished(t)
    })
    .await;
    assert_eq!(done.state, TransferState::Done, "{done:?}");
    assert_eq!(
        std::fs::read(incoming.join("movie.bin")).unwrap(),
        std::fs::read(&path).unwrap()
    );
    assert_eq!(files_in(&incoming), ["movie.bin"], "no partial is left");
    drop(b);
}

#[tokio::test]
async fn a_resume_whose_kept_partial_no_longer_matches_is_retried_from_zero_and_still_arrives_equal(
) {
    use std::io::{Seek, SeekFrom, Write};
    let (a, _b, incoming) = two_paired("resume-bad", |_| {}).await;
    let path = make_file(&a.dir.join("out"), "movie.bin", 120 * 1024 * 1024);
    let first = send(&a, &path).await;
    wait_transfer(&a, 60, "bytes to move", |t| {
        t.id == first.id && t.done > 30 * 1024 * 1024
    })
    .await;
    assert!(a.engine.sync.cancel_transfer(first.id));
    wait_transfer(&a, 20, "the cut", |t| t.id == first.id && finished(t)).await;
    assert!(
        wait_until(15, || files_in(&incoming)
            .iter()
            .any(|n| n.ends_with(".part")))
        .await
    );

    // Change a byte inside the part the receiver already holds, and put the modification time back: to the
    // sender it is still "the same file", but the kept bytes are now wrong.
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();
    let mut f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    f.seek(SeekFrom::Start(5 * 1024 * 1024)).unwrap();
    f.write_all(&[0xAB]).unwrap();
    f.set_modified(before).unwrap();
    drop(f);

    let second = send(&a, &path).await;
    let done = wait_transfer(&a, 120, "the second send", |t| {
        t.id == second.id && finished(t)
    })
    .await;
    assert_eq!(done.state, TransferState::Done, "{done:?}");
    assert_eq!(
        std::fs::read(incoming.join("movie.bin")).unwrap(),
        std::fs::read(&path).unwrap(),
        "the file that arrived is the changed one, complete"
    );
    assert_eq!(files_in(&incoming), ["movie.bin"]);
}

#[tokio::test]
async fn cancelling_on_the_receiving_side_deletes_the_partial() {
    let (a, b, incoming) = two_paired("recvcancel", |_| {}).await;
    let huge = make_sparse(&a.dir.join("out"), "huge.bin", 2 * 1024 * 1024 * 1024);
    send(&a, &huge).await;
    let theirs = wait_transfer(&b, 30, "bytes to arrive", |t| t.done > 20 * 1024 * 1024).await;
    assert!(b.engine.sync.cancel_transfer(theirs.id));
    assert!(
        wait_until(15, || files_in(&incoming).is_empty()).await,
        "the receiver's own cancel is the person's decision: nothing is kept ({:?})",
        files_in(&incoming)
    );
    wait_transfer(&a, 20, "the sender to give up", finished).await;
}

#[tokio::test]
async fn unpairing_the_sender_on_the_receiving_side_deletes_what_it_left() {
    let (a, b, incoming) = two_paired("unpair-kept", |_| {}).await;
    let huge = make_sparse(&a.dir.join("out"), "huge.bin", 2 * 1024 * 1024 * 1024);
    let started = send(&a, &huge).await;
    wait_transfer(&a, 30, "bytes to move", |t| {
        t.id == started.id && t.done > 20 * 1024 * 1024
    })
    .await;
    assert!(a.engine.sync.cancel_transfer(started.id));
    assert!(
        wait_until(15, || files_in(&incoming)
            .iter()
            .any(|n| n.ends_with(".part")))
        .await
    );
    b.engine
        .sync
        .unpair(&a.engine.node_id().fingerprint())
        .await
        .unwrap();
    assert!(
        wait_until(10, || files_in(&incoming).is_empty()).await,
        "{:?}",
        files_in(&incoming)
    );
}

/// A folder `Trip` with nested files; returns its path and the (relative path, bytes) of every file.
fn make_folder(root: &Path, files: usize) -> (PathBuf, Vec<(String, Vec<u8>)>) {
    let top = root.join("Trip");
    let mut all = Vec::new();
    for i in 0..files {
        let rel = match i % 3 {
            0 => format!("file{i}.txt"),
            1 => format!("Day {}/img{i}.jpg", i % 5),
            _ => format!("Day {}/raw/shot{i}.cr2", i % 5),
        };
        let bytes: Vec<u8> = (0..(1000 + i * 37))
            .map(|n| ((n * 7 + i) % 251) as u8)
            .collect();
        let path = top.join(&rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        all.push((rel, bytes));
    }
    (top, all)
}

#[tokio::test]
async fn a_folder_arrives_with_its_structure_in_one_new_folder_and_again_as_a_second_one() {
    let (a, _b, incoming) = two_paired("folder1", |_| {}).await;
    let (top, files) = make_folder(&a.dir.join("out"), 30);
    let total: u64 = files.iter().map(|(_, b)| b.len() as u64).sum();
    let mut answers = a.engine.sync.send_file(top.clone(), None).await;
    let row = answers.remove(0).expect("the folder send starts");
    assert_eq!((row.name.as_str(), row.size), ("Trip", total));
    let done = wait_transfer(&a, 90, "the folder", |t| t.id == row.id && finished(t)).await;
    assert_eq!(done.state, TransferState::Done, "{done:?}");
    assert_eq!(done.done, total);
    for (rel, bytes) in &files {
        assert_eq!(
            &std::fs::read(incoming.join("Trip").join(rel)).unwrap(),
            bytes,
            "{rel}"
        );
    }
    assert!(
        files_in(&incoming).iter().all(|n| !n.starts_with(".klip-")),
        "{:?}",
        files_in(&incoming)
    );

    let again = a.engine.sync.send_file(top, None).await.remove(0).unwrap();
    wait_transfer(&a, 90, "the second folder", |t| {
        t.id == again.id && finished(t)
    })
    .await;
    assert_eq!(files_in(&incoming), ["Trip", "Trip (2)"]);
    for (rel, bytes) in &files {
        assert_eq!(
            &std::fs::read(incoming.join("Trip (2)").join(rel)).unwrap(),
            bytes
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn one_unreadable_file_fails_only_that_file_and_the_row_says_so() {
    use std::os::unix::fs::PermissionsExt;
    let (a, _b, incoming) = two_paired("folder2", |_| {}).await;
    let (top, files) = make_folder(&a.dir.join("out"), 9);
    let bad = top.join(&files[4].0);
    std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o000)).unwrap();
    let row = a.engine.sync.send_file(top, None).await.remove(0).unwrap();
    let done = wait_transfer(&a, 60, "the folder", |t| t.id == row.id && finished(t)).await;
    assert_eq!(done.state, TransferState::Failed, "{done:?}");
    assert!(
        done.error
            .as_deref()
            .unwrap()
            .starts_with("1 of 9 files failed"),
        "{done:?}"
    );
    for (i, (rel, bytes)) in files.iter().enumerate() {
        let path = incoming.join("Trip").join(rel);
        if i == 4 {
            assert!(!path.exists());
        } else {
            assert_eq!(&std::fs::read(&path).unwrap(), bytes, "{rel}");
        }
    }
}

#[tokio::test]
async fn cancelling_a_folder_stops_it_between_files() {
    let (a, _b, incoming) = two_paired("folder3", |_| {}).await;
    let (top, _files) = make_folder(&a.dir.join("out"), 6);
    make_sparse(&top, "zzz-huge.bin", 2 * 1024 * 1024 * 1024); // sorted last: the small ones go first
    let row = a.engine.sync.send_file(top, None).await.remove(0).unwrap();
    wait_transfer(&a, 60, "the huge file to move", |t| {
        t.id == row.id && t.done > 3_000_000 + 20 * 1024 * 1024
    })
    .await;
    assert!(a.engine.sync.cancel_transfer(row.id));
    let done = wait_transfer(&a, 30, "the cancel", |t| t.id == row.id && finished(t)).await;
    assert_eq!(done.state, TransferState::Cancelled, "{done:?}");
    assert!(
        incoming.join("Trip").join("file0.txt").exists(),
        "what had arrived stays"
    );
    assert!(!incoming.join("Trip").join("zzz-huge.bin").exists());
}

#[tokio::test]
async fn a_hostile_folder_place_from_a_raw_stream_stays_inside_the_top_folder() {
    use klip_net::file::{send_file_v2, FileHeaderV2, FolderPlace};
    let (a, _b, incoming) = two_paired("folder4", |_| {}).await;
    let b_id = a.engine.sync.net.peers()[0].id;
    let mut streams = a.engine.sync.net.open_file_stream(b_id).await.unwrap();
    let place = FolderPlace {
        batch: [7; 16],
        top: "../../Escape".into(),
        rel: vec!["..".into(), "..".into(), "x/../y".into()],
    };
    let header = FileHeaderV2 {
        version: 2,
        id: [1; 16],
        name: "../../evil.txt".into(),
        size: 100,
        resume_key: [0; 32],
        folder: Some(place),
    };
    send_file_v2(
        &header,
        &[1u8; 100][..],
        &mut streams.tx,
        &mut streams.rx,
        &mut |_| {},
    )
    .await
    .unwrap();
    streams.close();
    let mut found = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            if e.path().is_dir() {
                walk(&e.path(), out)
            } else {
                out.push(e.path())
            }
        }
    }
    walk(incoming.parent().unwrap(), &mut found);
    let landed: Vec<_> = found
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n == "evil.txt"))
        .collect();
    assert_eq!(landed.len(), 1, "{found:?}");
    assert!(landed[0].starts_with(&incoming), "{:?}", landed[0]);
}
