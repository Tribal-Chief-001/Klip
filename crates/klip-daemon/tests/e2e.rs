//! Daemon-level end-to-end tests: two complete daemons (real iroh endpoints, real keystores, real
//! Unix control sockets, in-memory clipboards) pair and sync inside one process. LAN mode with
//! discovery off, so nothing leaves the machine.
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

fn synced(d: &TestDaemon) -> u64 {
    d.engine
        .sync
        .total_synced_clips
        .load(std::sync::atomic::Ordering::SeqCst)
}

#[tokio::test]
async fn pair_over_ipc_then_sync_both_ways_without_echo() {
    let a = start_daemon("Alpha", fresh_dir("a1"), false).await;
    let b = start_daemon("Beta", fresh_dir("b1"), false).await;

    match pair(&a, &b, true).await {
        IpcResponse::PairSuccess {
            peer_name,
            peer_fingerprint,
        } => {
            assert_eq!(peer_name, "Alpha");
            assert_eq!(peer_fingerprint, a.engine.node_id().fingerprint());
        }
        other => panic!("pairing failed: {other:?}"),
    }
    // The host hears about the new device through its asynchronous event pump.
    let mut paired = None;
    for _ in 0..100 {
        paired = poll_pair(&a).await.paired;
        if paired.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(paired.as_deref(), Some("Beta"));

    // A -> B
    a.clipboard.user_copy("hello from alpha");
    assert!(
        wait_until(10, || b.clipboard.current_text().as_deref()
            == Some("hello from alpha"))
        .await,
        "B never received A's clip"
    );
    // B -> A
    b.clipboard.user_copy("hello from beta");
    assert!(
        wait_until(10, || a.clipboard.current_text().as_deref()
            == Some("hello from beta"))
        .await,
        "A never received B's clip"
    );

    // No ping-pong: after things settle the counters stop moving.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let before = (synced(&a), synced(&b));
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        before,
        (synced(&a), synced(&b)),
        "clips keep bouncing between the devices"
    );

    // Status reflects reality: one peer each, with a real connection state.
    match a.ipc(IpcRequest::GetStatus).await {
        IpcResponse::Status(s) => {
            assert_eq!(s.peers.len(), 1);
            assert_eq!(s.peers[0].name, "Beta");
            assert!(s.peers[0].is_online);
            assert!(s.memory_rss_bytes > 0);
        }
        other => panic!("unexpected {other:?}"),
    }
    // History keeps both clips.
    match a.ipc(IpcRequest::GetHistory { limit: 10 }).await {
        IpcResponse::History(items) => assert_eq!(items.len(), 2),
        other => panic!("unexpected {other:?}"),
    }
    a.stop().await;
    b.stop().await;
}

#[tokio::test]
async fn declined_pairing_leaves_both_devices_unpaired() {
    let a = start_daemon("Alpha", fresh_dir("a2"), false).await;
    let b = start_daemon("Beta", fresh_dir("b2"), false).await;
    match pair(&a, &b, false).await {
        IpcResponse::Error(_) => {}
        other => panic!("declined pairing must fail, got {other:?}"),
    }
    for d in [&a, &b] {
        match d.ipc(IpcRequest::GetStatus).await {
            IpcResponse::Status(s) => assert!(s.peers.is_empty()),
            other => panic!("unexpected {other:?}"),
        }
    }
    a.clipboard.user_copy("should not travel");
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(b.clipboard.current_text(), None);
    a.stop().await;
    b.stop().await;
}

#[tokio::test]
async fn unpair_stops_sync_and_is_persisted() {
    let dir_a = fresh_dir("a3");
    let a = start_daemon("Alpha", dir_a.clone(), false).await;
    let b = start_daemon("Beta", fresh_dir("b3"), false).await;
    assert!(matches!(
        pair(&a, &b, true).await,
        IpcResponse::PairSuccess { .. }
    ));

    // Selecting by name works, and the removal survives a restart.
    match a
        .ipc(IpcRequest::Unpair {
            selector: "Beta".into(),
        })
        .await
    {
        IpcResponse::Unpaired { name } => assert_eq!(name, "Beta"),
        other => panic!("unexpected {other:?}"),
    }
    assert!(matches!(
        a.ipc(IpcRequest::Unpair {
            selector: "Beta".into()
        })
        .await,
        IpcResponse::Error(_)
    ));
    b.clipboard.user_copy("after unpair");
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        a.clipboard.current_text(),
        None,
        "unpaired device must not deliver clips"
    );

    a.stop().await;
    let a2 = start_daemon("Alpha", dir_a, false).await;
    match a2.ipc(IpcRequest::GetStatus).await {
        IpcResponse::Status(s) => assert!(s.peers.is_empty(), "removal must persist"),
        other => panic!("unexpected {other:?}"),
    }
    a2.stop().await;
    b.stop().await;
}

#[tokio::test]
async fn pause_blocks_sending_and_applying() {
    let a = start_daemon("Alpha", fresh_dir("a4"), false).await;
    let b = start_daemon("Beta", fresh_dir("b4"), false).await;
    assert!(matches!(
        pair(&a, &b, true).await,
        IpcResponse::PairSuccess { .. }
    ));

    a.ipc(IpcRequest::Pause { duration_secs: 600 }).await;
    // Paused sender: nothing leaves.
    a.clipboard.user_copy("while paused (send)");
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(b.clipboard.current_text(), None);
    // Paused receiver: remote clips are not applied.
    b.clipboard.user_copy("while paused (receive)");
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_ne!(
        a.clipboard.current_text().as_deref(),
        Some("while paused (receive)")
    );

    a.ipc(IpcRequest::Resume).await;
    b.clipboard.user_copy("after resume");
    assert!(
        wait_until(10, || a.clipboard.current_text().as_deref()
            == Some("after resume"))
        .await
    );
    a.stop().await;
    b.stop().await;
}

async fn read_response(s: &mut UnixStream) -> IpcResponse {
    let mut len = [0u8; 4];
    s.read_exact(&mut len).await.unwrap();
    let mut buf = vec![0u8; u32::from_be_bytes(len) as usize];
    s.read_exact(&mut buf).await.unwrap();
    deserialize_ipc_msg(&buf).unwrap()
}

#[tokio::test]
async fn control_socket_survives_garbage_and_oversized_requests() {
    let a = start_daemon("Alpha", fresh_dir("a5"), false).await;
    let sock = a.sock();

    // Absurd length prefix: answered with an error, no giant allocation.
    let mut s = UnixStream::connect(&sock).await.unwrap();
    s.write_all(&u32::MAX.to_be_bytes()).await.unwrap();
    assert!(matches!(read_response(&mut s).await, IpcResponse::Error(_)));

    // Garbage body.
    let mut s = UnixStream::connect(&sock).await.unwrap();
    s.write_all(&5u32.to_be_bytes()).await.unwrap();
    s.write_all(&[0xff; 5]).await.unwrap();
    assert!(matches!(read_response(&mut s).await, IpcResponse::Error(_)));

    // Client that connects and says nothing must not wedge the server.
    let _idle = UnixStream::connect(&sock).await.unwrap();
    assert!(matches!(
        a.ipc(IpcRequest::GetStatus).await,
        IpcResponse::Status(_)
    ));

    // The socket is private.
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&sock).unwrap().permissions().mode() & 0o777,
        0o600
    );
    a.stop().await;
}

#[tokio::test]
async fn stop_removes_the_socket_and_ends_the_engine() {
    let a = start_daemon("Alpha", fresh_dir("a6"), false).await;
    let sock = a.sock();
    assert!(matches!(
        a.ipc(IpcRequest::StopDaemon).await,
        IpcResponse::DaemonStopping
    ));
    assert!(
        wait_until(10, || !sock.exists()).await,
        "socket file must be removed on stop"
    );
    tokio::time::timeout(Duration::from_secs(10), a.task)
        .await
        .expect("engine ends")
        .unwrap();
}

#[tokio::test]
async fn device_id_equals_network_id_and_survives_restart() {
    let dir = fresh_dir("a7");
    let a = start_daemon("Alpha", dir.clone(), false).await;
    let id = a.engine.node_id();
    assert_eq!(id.as_bytes(), a.engine.sync.net.id().as_bytes());
    a.stop().await;
    let a2 = start_daemon("Alpha", dir, false).await;
    assert_eq!(
        a2.engine.node_id(),
        id,
        "identity must be stable across restarts"
    );
    a2.stop().await;
}

fn sent_report(resp: IpcResponse) -> (Vec<String>, Vec<(String, String)>) {
    match resp {
        IpcResponse::Sent {
            delivered, failed, ..
        } => (delivered, failed),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn sender_is_told_the_truth_about_what_the_receiver_did() {
    let a = start_daemon("Alpha", fresh_dir("a8"), false).await;
    let b = start_daemon("Beta", fresh_dir("b8"), false).await;
    assert!(matches!(
        pair(&a, &b, true).await,
        IpcResponse::PairSuccess { .. }
    ));

    // Healthy receiver: delivered, and really on its clipboard.
    let (ok, bad) = sent_report(a.ipc(IpcRequest::SendClip { text: "one".into() }).await);
    assert_eq!(
        (ok.as_slice(), bad.len()),
        (["Beta".to_string()].as_slice(), 0)
    );
    assert_eq!(b.clipboard.current_text().as_deref(), Some("one"));

    // Receiver's OS clipboard is unavailable: not "delivered".
    b.clipboard.set_fail_writes(true);
    let (ok, bad) = sent_report(a.ipc(IpcRequest::SendClip { text: "two".into() }).await);
    assert!(ok.is_empty(), "must not claim delivery: {ok:?}");
    assert!(bad[0].1.contains("clipboard"), "{bad:?}");
    assert_eq!(b.clipboard.current_text().as_deref(), Some("one"));
    b.clipboard.set_fail_writes(false);

    // Receiver paused: sender is told so.
    b.ipc(IpcRequest::Pause { duration_secs: 600 }).await;
    let (ok, bad) = sent_report(
        a.ipc(IpcRequest::SendClip {
            text: "three".into(),
        })
        .await,
    );
    assert!(ok.is_empty());
    assert!(bad[0].1.contains("paused"), "{bad:?}");
    b.ipc(IpcRequest::Resume).await;

    // Sender-side status remembers the last failure, then clears it on success.
    let (ok, _) = sent_report(
        a.ipc(IpcRequest::SendClip {
            text: "four".into(),
        })
        .await,
    );
    assert_eq!(ok.len(), 1);
    match a.ipc(IpcRequest::GetStatus).await {
        IpcResponse::Status(s) => assert!(s.peers[0].last_error.is_none()),
        other => panic!("unexpected {other:?}"),
    }
    a.stop().await;
    b.stop().await;
}

#[tokio::test]
async fn a_second_daemon_on_the_same_config_is_refused_and_does_not_steal_the_socket() {
    let dir = fresh_dir("a9");
    let a = start_daemon("Alpha", dir.clone(), false).await;

    // A second instance with the same configuration directory must fail fast ...
    let paths = KlipPaths::from_dir(&dir);
    let config = DaemonConfig {
        device_name: "Alpha".into(),
        relay: RelayMode::Lan,
        lan_discovery: false,
        ..DaemonConfig::default()
    };
    let err = DaemonEngine::init_with(config, paths, None)
        .await
        .err()
        .expect("second daemon must be refused");
    assert!(err.to_string().contains("already running"), "{err}");

    // ... and the first one must still own a working control socket.
    assert!(matches!(
        a.ipc(IpcRequest::GetStatus).await,
        IpcResponse::Status(_)
    ));
    a.stop().await;

    // After a clean stop the directory can be used again.
    let again = start_daemon("Alpha", dir, false).await;
    assert!(matches!(
        again.ipc(IpcRequest::GetStatus).await,
        IpcResponse::Status(_)
    ));
    again.stop().await;
}

#[tokio::test]
async fn a_burst_of_copies_is_coalesced_and_the_last_one_wins() {
    use klip_proto::clip::ClipContentType;
    use std::sync::atomic::Ordering;

    let a = start_daemon("Alpha", fresh_dir("a10"), false).await;
    let b = start_daemon("Beta", fresh_dir("b10"), false).await;
    assert!(matches!(
        pair(&a, &b, true).await,
        IpcResponse::PairSuccess { .. }
    ));

    const BURST: u64 = 200;
    for i in 0..BURST {
        a.engine.sync.spawn_local_clip(
            ClipContentType::PlainTextUtf8,
            format!("burst-{i}").into_bytes(),
        );
    }
    let last = format!("burst-{}", BURST - 1);
    assert!(
        wait_until(15, || b.clipboard.current_text().as_deref()
            == Some(last.as_str()))
        .await,
        "the newest clip of the burst never arrived: {:?}",
        b.clipboard.current_text()
    );
    // Let any in-flight send finish, then check the accounting: every clip was either sent or
    // replaced by a newer one, and far fewer than BURST network sends were made.
    let sync = &a.engine.sync;
    assert!(
        wait_until(10, || {
            sync.sends_started.load(Ordering::SeqCst) + sync.coalesced_clips.load(Ordering::SeqCst)
                == BURST
        })
        .await,
        "sent {} + coalesced {} != {BURST}",
        sync.sends_started.load(Ordering::SeqCst),
        sync.coalesced_clips.load(Ordering::SeqCst)
    );
    let sends = sync.sends_started.load(Ordering::SeqCst);
    assert!((1..BURST / 2).contains(&sends), "sends_started = {sends}");
    // The receiver still shows the newest clip after everything settles (nothing older landed late).
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(b.clipboard.current_text().as_deref(), Some(last.as_str()));

    a.stop().await;
    b.stop().await;
}

#[tokio::test]
async fn a_30_mb_clip_is_delivered_and_an_oversized_one_is_refused_without_harm() {
    let a = start_daemon("Alpha", fresh_dir("a11"), false).await;
    let b = start_daemon("Beta", fresh_dir("b11"), false).await;
    assert!(matches!(
        pair(&a, &b, true).await,
        IpcResponse::PairSuccess { .. }
    ));

    // 30 MB of text through the real capture path (clipboard -> daemon -> QUIC -> clipboard).
    let big = "z".repeat(30 * 1000 * 1000);
    let started = Instant::now();
    a.clipboard.user_copy(&big);
    assert!(
        wait_until(60, || b.clipboard.current_text().map(|t| t.len())
            == Some(big.len()))
        .await,
        "30 MB clip never arrived"
    );
    eprintln!("30 MB delivered in {:?}", started.elapsed());

    // Above the 32 MiB limit: not delivered, nobody crashes, small clips still flow afterwards.
    let too_big = "q".repeat(33 * 1024 * 1024);
    a.clipboard.user_copy(&too_big);
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_ne!(
        b.clipboard.current_text().map(|t| t.len()),
        Some(too_big.len()),
        "an oversized clip must not be applied"
    );
    a.clipboard.user_copy("small again");
    assert!(
        wait_until(15, || b.clipboard.current_text().as_deref()
            == Some("small again"))
        .await,
        "sync must keep working after an oversized clip"
    );
    a.stop().await;
    b.stop().await;
}

/// What a developer actually copies: a long source file with tabs, CRLF and bare CR, trailing spaces,
/// blank lines, emoji, CJK, right-to-left text, combining marks, zero-width characters and a BOM, plus a
/// link with a query string and a very long line. It must arrive byte for byte.
#[tokio::test]
async fn code_links_and_awkward_text_arrive_byte_for_byte() {
    let a = start_daemon("Alpha", fresh_dir("a12"), false).await;
    let b = start_daemon("Beta", fresh_dir("b12"), false).await;
    assert!(matches!(
        pair(&a, &b, true).await,
        IpcResponse::PairSuccess { .. }
    ));

    let mut code = String::from("\u{FEFF}// \u{1F980} Rust \u{65E5}\u{672C}\u{8A9E} \u{05E9}\u{05DC}\u{05D5}\u{05DD} e\u{0301} a\u{200B}b\n");
    for i in 0..10_000 {
        match i % 5 {
            0 => code.push_str(&format!(
                "fn f{i}() {{\n\treturn {i}; // tab-indented   \n}}\n"
            )),
            1 => code.push_str(&format!(
                "    let url = \"https://example.com/a/b?x={i}&y=%20z#frag\";\r\n"
            )),
            2 => code.push_str("\n\n"),
            3 => code.push_str(&format!("  if a {{ b }} else {{ c }} \r{i}\n")),
            _ => code.push_str(&format!("{}\n", "x".repeat(40))),
        }
    }
    code.push_str(&"y".repeat(100_000)); // one very long line
    code.push('\n');
    assert!(code.len() > 400_000, "{}", code.len());

    for sample in [
        code.as_str(),
        "https://example.com/path/to/page?query=1&other=%E2%9C%93#section",
        "  leading and trailing spaces  \n",
        "line1\r\nline2\r\n",
        "\t",
    ] {
        a.clipboard.user_copy(sample);
        assert!(
            wait_until(30, || b.clipboard.current_text().as_deref() == Some(sample)).await,
            "text of {} bytes did not arrive intact (got {:?} bytes)",
            sample.len(),
            b.clipboard.current_text().map(|t| t.len())
        );
    }
    a.stop().await;
    b.stop().await;
}

fn hello_of(r: IpcResponse) -> klip_proto::ipc::HelloInfo {
    match r {
        IpcResponse::Hello(h) => h,
        other => panic!("expected Hello, got {other:?}"),
    }
}

#[tokio::test]
async fn hello_reports_version_capabilities_config_and_the_request_cap() {
    let a = start_daemon_with("Alpha", fresh_dir("h1"), false, |c| c.max_clip_bytes = 5000).await;
    let h = hello_of(a.ipc(IpcRequest::Hello { client_protocol: 2 }).await);
    assert_eq!(h.protocol, klip_proto::ipc::IPC_PROTOCOL_VERSION);
    assert_eq!(h.daemon_version, env!("CARGO_PKG_VERSION"));
    for cap in ["unpair_all", "copy_to_clipboard", "clear_history"] {
        assert!(
            h.capabilities.iter().any(|c| c == cap),
            "missing capability {cap}"
        );
    }
    assert_eq!(h.config.device_name, "Alpha");
    assert_eq!(h.config.relay, "lan");
    assert_eq!(h.config.max_clip_bytes, 5000);
    assert_eq!(h.max_request_bytes, 5000 + 64 * 1024);
    a.stop().await;
}

#[tokio::test]
async fn daemon_version_is_the_release_version_not_the_stale_placeholder() {
    assert_ne!(env!("CARGO_PKG_VERSION"), "0.1.0");
}

#[tokio::test]
async fn unpair_all_removes_every_device_and_reports_the_names() {
    let a = start_daemon("Alpha", fresh_dir("u1a"), false).await;
    let b = start_daemon("Beta", fresh_dir("u1b"), false).await;
    let c = start_daemon("Gamma", fresh_dir("u1c"), false).await;
    assert!(matches!(
        pair(&a, &b, true).await,
        IpcResponse::PairSuccess { .. }
    ));
    assert!(matches!(
        pair(&a, &c, true).await,
        IpcResponse::PairSuccess { .. }
    ));
    match a.ipc(IpcRequest::GetStatus).await {
        IpcResponse::Status(s) => assert_eq!(s.peers.len(), 2),
        other => panic!("{other:?}"),
    }
    match a.ipc(IpcRequest::UnpairAll).await {
        IpcResponse::UnpairedAll {
            mut removed,
            failed,
        } => {
            removed.sort();
            assert_eq!(removed, ["Beta", "Gamma"]);
            assert!(failed.is_empty(), "{failed:?}");
        }
        other => panic!("{other:?}"),
    }
    match a.ipc(IpcRequest::GetStatus).await {
        IpcResponse::Status(s) => assert!(s.peers.is_empty()),
        other => panic!("{other:?}"),
    }
    // Nothing left to remove: still a clean answer.
    assert!(matches!(
        a.ipc(IpcRequest::UnpairAll).await,
        IpcResponse::UnpairedAll { removed, failed } if removed.is_empty() && failed.is_empty()
    ));
    a.stop().await;
    b.stop().await;
    c.stop().await;
}

#[tokio::test]
async fn copy_to_clipboard_restores_an_item_without_sending_it_out_again() {
    let a = start_daemon("Alpha", fresh_dir("c1a"), false).await;
    let b = start_daemon("Beta", fresh_dir("c1b"), false).await;
    assert!(matches!(
        pair(&a, &b, true).await,
        IpcResponse::PairSuccess { .. }
    ));

    a.clipboard.user_copy("first");
    assert!(
        wait_until(10, || b.clipboard.current_text().as_deref()
            == Some("first"))
        .await
    );
    b.clipboard.user_copy("second");
    assert!(
        wait_until(10, || a.clipboard.current_text().as_deref()
            == Some("second"))
        .await
    );

    let id = match b.ipc(IpcRequest::GetHistory { limit: 10 }).await {
        IpcResponse::History(items) => {
            items
                .iter()
                .find(|i| i.full_content == b"first")
                .expect("'first' is in Beta's history")
                .id
        }
        other => panic!("{other:?}"),
    };
    assert!(matches!(
        b.ipc(IpcRequest::CopyToClipboard { id }).await,
        IpcResponse::Copied
    ));
    assert_eq!(b.clipboard.current_text().as_deref(), Some("first"));
    // If the copy had been broadcast as a new local copy, Alpha would now show "first".
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(a.clipboard.current_text().as_deref(), Some("second"));

    // An unknown id is a plain error.
    match b.ipc(IpcRequest::CopyToClipboard { id: [9; 16] }).await {
        IpcResponse::Error(e) => assert!(e.contains("no longer in the history"), "{e}"),
        other => panic!("{other:?}"),
    }
    a.stop().await;
    b.stop().await;
}

#[tokio::test]
async fn clear_history_forgets_the_clips() {
    let a = start_daemon("Alpha", fresh_dir("ch1"), false).await;
    let b = start_daemon("Beta", fresh_dir("ch2"), false).await;
    assert!(matches!(
        pair(&a, &b, true).await,
        IpcResponse::PairSuccess { .. }
    ));
    a.clipboard.user_copy("remember me");
    assert!(
        wait_until(10, || b.clipboard.current_text().as_deref()
            == Some("remember me"))
        .await
    );
    assert!(matches!(
        b.ipc(IpcRequest::GetHistory { limit: 10 }).await,
        IpcResponse::History(items) if !items.is_empty()
    ));
    assert!(matches!(
        b.ipc(IpcRequest::ClearHistory).await,
        IpcResponse::HistoryCleared
    ));
    assert!(matches!(
        b.ipc(IpcRequest::GetHistory { limit: 10 }).await,
        IpcResponse::History(items) if items.is_empty()
    ));
    a.stop().await;
    b.stop().await;
}

#[tokio::test]
async fn a_request_over_the_cap_gets_a_clear_error() {
    let a = start_daemon_with("Alpha", fresh_dir("cap1"), false, |c| {
        c.max_clip_bytes = 1000
    })
    .await;
    let mut s = UnixStream::connect(a.sock()).await.unwrap();
    // Announce 1 MB (cap is 1000 + 64 KiB) and send nothing else: the answer must come anyway.
    s.write_all(&1_000_000u32.to_be_bytes()).await.unwrap();
    let mut len = [0u8; 4];
    s.read_exact(&mut len).await.unwrap();
    let mut buf = vec![0u8; u32::from_be_bytes(len) as usize];
    s.read_exact(&mut buf).await.unwrap();
    match deserialize_ipc_msg::<IpcResponse>(&buf).unwrap() {
        IpcResponse::Error(e) => {
            assert!(
                e.contains("request too large") && e.contains("66536"),
                "{e}"
            )
        }
        other => panic!("{other:?}"),
    }
    // The daemon is still fine.
    assert!(matches!(
        a.ipc(IpcRequest::GetStatus).await,
        IpcResponse::Status(_)
    ));
    a.stop().await;
}

#[tokio::test]
async fn unpair_that_cannot_be_saved_is_refused_and_the_device_stays_listed() {
    use std::os::unix::fs::PermissionsExt;
    let dir_a = fresh_dir("a-unsaved");
    let a = start_daemon("Alpha", dir_a.clone(), false).await;
    let b = start_daemon("Beta", fresh_dir("b-unsaved"), false).await;
    assert!(matches!(
        pair(&a, &b, true).await,
        IpcResponse::PairSuccess { .. }
    ));

    // A read-only directory: the keystore cannot be rewritten.
    std::fs::set_permissions(&dir_a, std::fs::Permissions::from_mode(0o500)).unwrap();
    let reply = a
        .ipc(IpcRequest::Unpair {
            selector: "Beta".into(),
        })
        .await;
    std::fs::set_permissions(&dir_a, std::fs::Permissions::from_mode(0o700)).unwrap();
    match reply {
        IpcResponse::Error(e) => assert!(e.contains("could not remove"), "{e}"),
        other => panic!("an unsaved removal must be an error, got {other:?}"),
    }
    match a.ipc(IpcRequest::GetStatus).await {
        IpcResponse::Status(s) => assert_eq!(s.peers.len(), 1, "device must still be listed"),
        other => panic!("unexpected {other:?}"),
    }
    // With the directory writable again, the same request works.
    assert!(matches!(
        a.ipc(IpcRequest::Unpair {
            selector: "Beta".into()
        })
        .await,
        IpcResponse::Unpaired { .. }
    ));
    a.stop().await;
    b.stop().await;
}

#[tokio::test]
async fn a_retried_clip_counts_as_applied_but_an_older_clip_is_superseded() {
    use klip_net::{ClipOutcome, EndpointId, NetEvent};
    use klip_proto::clip::{ClipContentType, ClipPayload};
    use klip_proto::hlc::HybridLogicalClock;

    let a = start_daemon("Alpha", fresh_dir("a-sup"), false).await;
    let b = start_daemon("Beta", fresh_dir("b-sup"), false).await;
    let from = EndpointId::from_bytes(a.engine.node_id().as_bytes()).unwrap();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let clip = |text: &str, physical_time_ms: u64| {
        let node = a.engine.node_id();
        ClipPayload::new(
            ClipContentType::PlainTextUtf8,
            text.as_bytes().to_vec(),
            node,
            HybridLogicalClock {
                physical_time_ms,
                logical_counter: 0,
                node_id: node,
            },
            false,
        )
    };
    let deliver = |p: ClipPayload| {
        let sync = Arc::clone(&b.engine.sync);
        async move {
            let (ack, answer) = tokio::sync::oneshot::channel();
            sync.handle_net_event(NetEvent::Clip {
                from,
                payload: p,
                ack,
            })
            .await;
            answer.await.unwrap()
        }
    };

    let newer = clip("newer", now_ms + 1_000);
    assert_eq!(deliver(newer.clone()).await, ClipOutcome::Applied);
    // Same clip again (the sender timed out and retried): still a success.
    assert_eq!(deliver(newer).await, ClipOutcome::Applied);
    // A different clip stamped before the active one (sender clock behind): not applied, and said so.
    assert_eq!(
        deliver(clip("older", now_ms - 60_000)).await,
        ClipOutcome::Superseded
    );
    assert_eq!(b.clipboard.current_text().as_deref(), Some("newer"));
    a.stop().await;
    b.stop().await;
}

#[tokio::test]
async fn an_automatic_copy_that_cannot_reach_a_device_alerts_once_per_outage() {
    use std::sync::atomic::Ordering::SeqCst;
    let a = start_daemon("Alpha", fresh_dir("a-alert"), false).await;
    let b = start_daemon("Beta", fresh_dir("b-alert"), false).await;
    let b_id = klip_net::EndpointId::from_bytes(b.engine.node_id().as_bytes()).unwrap();
    assert!(matches!(
        pair(&a, &b, true).await,
        IpcResponse::PairSuccess { .. }
    ));
    let alerts: Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let seen = Arc::clone(&alerts);
    a.engine
        .sync
        .set_undelivered_hook(Arc::new(move |name: &str, _gone: Duration| {
            seen.lock().unwrap().push(name.to_string())
        }));
    assert!(
        wait_until(15, || a
            .engine
            .sync
            .peer_status()
            .iter()
            .all(|p| p.is_online))
        .await
    );
    b.stop().await;
    assert!(
        wait_until(15, || a
            .engine
            .sync
            .peer_status()
            .iter()
            .all(|p| !p.is_online))
        .await
    );

    // An explicit send reports its own error and raises no alert.
    let (ok, bad) = sent_report(
        a.ipc(IpcRequest::SendClip {
            text: "explicit".into(),
        })
        .await,
    );
    assert!(ok.is_empty() && bad.len() == 1, "{ok:?} {bad:?}");
    assert!(alerts.lock().unwrap().is_empty());

    // The first automatic copy alerts, with the device name only.
    a.clipboard.user_copy("one");
    assert!(
        wait_until(40, || !alerts.lock().unwrap().is_empty()).await,
        "no alert for a copy that could not be delivered"
    );
    // A burst of further copies in the same outage stays quiet.
    a.clipboard.user_copy("two");
    assert!(wait_until(40, || a.engine.sync.sends_started.load(SeqCst) >= 2).await);
    a.clipboard.user_copy("three");
    assert!(wait_until(60, || a.engine.sync.sends_started.load(SeqCst) >= 3).await);
    assert!(wait_until(60, || !a.engine.sync.is_sending()).await);
    assert_eq!(*alerts.lock().unwrap(), vec!["Beta".to_string()]);

    // The device comes back (nobody copies meanwhile) and goes away again: the next failed copy
    // is a new outage and must alert again.
    a.engine
        .sync
        .handle_net_event(klip_net::NetEvent::PeerConnected(b_id))
        .await;
    a.clipboard.user_copy("four");
    assert!(
        wait_until(40, || alerts.lock().unwrap().len() == 2).await,
        "a new outage after a reconnect must alert again"
    );
    assert!(wait_until(60, || !a.engine.sync.is_sending()).await);
    a.stop().await;
}

/// A real PNG whose header claims `width` x `height` (CRC fixed): a few bytes that would decode to
/// gigabytes.
fn png_claiming(width: u32, height: u32) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for b in data {
            crc ^= u32::from(*b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    let mut png = klip_diff::rgba_to_png(2, 2, &[255u8; 16]).unwrap();
    png[16..20].copy_from_slice(&width.to_be_bytes());
    png[20..24].copy_from_slice(&height.to_be_bytes());
    let crc = crc32(&png[12..29]);
    png[29..33].copy_from_slice(&crc.to_be_bytes());
    png
}

#[tokio::test]
async fn an_image_copied_on_one_device_arrives_unchanged_and_is_not_echoed() {
    use klip_proto::clip::ClipContentType;
    use std::sync::atomic::Ordering::SeqCst;
    let a = start_daemon("Alpha", fresh_dir("a-img"), false).await;
    let b = start_daemon("Beta", fresh_dir("b-img"), false).await;
    assert!(matches!(
        pair(&a, &b, true).await,
        IpcResponse::PairSuccess { .. }
    ));
    let rgba: Vec<u8> = (0..64 * 48 * 4).map(|i| (i * 7 % 251) as u8).collect();
    let png = klip_diff::rgba_to_png(64, 48, &rgba).unwrap();
    a.clipboard
        .user_copy_bytes(ClipContentType::ImagePng, png.clone());
    assert!(
        wait_until(15, || b
            .clipboard
            .current_clip()
            .is_some_and(|c| c.data == png))
        .await,
        "the image must arrive byte for byte"
    );
    assert_eq!(
        b.clipboard.current_clip().unwrap().content_type,
        ClipContentType::ImagePng
    );
    // Not echoed back: B sent nothing, A sent exactly once.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(b.engine.sync.sends_started.load(SeqCst), 0);
    assert_eq!(a.engine.sync.sends_started.load(SeqCst), 1);
    a.stop().await;
    b.stop().await;
}

#[tokio::test]
async fn a_bomb_and_a_mislabelled_image_are_rejected_and_never_reach_the_clipboard() {
    use klip_net::{ClipOutcome, EndpointId, NetEvent};
    use klip_proto::clip::{ClipContentType, ClipPayload};
    use klip_proto::hlc::HybridLogicalClock;

    let a = start_daemon("Alpha", fresh_dir("a-bomb"), false).await;
    let b = start_daemon("Beta", fresh_dir("b-bomb"), false).await;
    let from = EndpointId::from_bytes(a.engine.node_id().as_bytes()).unwrap();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let mut tick = 0u64;
    let mut clip = |content_type: ClipContentType, bytes: Vec<u8>| {
        tick += 1;
        let node = a.engine.node_id();
        ClipPayload::new(
            content_type,
            bytes,
            node,
            HybridLogicalClock {
                physical_time_ms: now_ms + tick * 10,
                logical_counter: 0,
                node_id: node,
            },
            false,
        )
    };
    let deliver = |p: ClipPayload| {
        let sync = Arc::clone(&b.engine.sync);
        async move {
            let (ack, answer) = tokio::sync::oneshot::channel();
            sync.handle_net_event(NetEvent::Clip {
                from,
                payload: p,
                ack,
            })
            .await;
            answer.await.unwrap()
        }
    };

    let bomb = clip(ClipContentType::ImagePng, png_claiming(60_000, 60_000));
    assert_eq!(deliver(bomb).await, ClipOutcome::Rejected);
    let small = klip_diff::rgba_to_png(4, 4, &[9u8; 64]).unwrap();
    let mislabelled = clip(ClipContentType::ImageWebP, small.clone());
    assert_eq!(deliver(mislabelled).await, ClipOutcome::Rejected);
    assert_eq!(
        deliver(clip(ClipContentType::ImagePng, b"nope".to_vec())).await,
        ClipOutcome::Rejected
    );
    assert!(
        b.clipboard.current_clip().is_none(),
        "nothing may reach the clipboard"
    );

    // A valid image right after is still accepted.
    assert_eq!(
        deliver(clip(ClipContentType::ImagePng, small.clone())).await,
        ClipOutcome::Applied
    );
    assert_eq!(b.clipboard.current_clip().unwrap().data, small);
    a.stop().await;
    b.stop().await;
}
