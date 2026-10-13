//! The TUI's own loop (Harness) driving real daemons: real iroh endpoints, real keystores, real
//! control sockets, in-memory clipboards, LAN only with discovery off (nothing leaves the machine).
#![cfg(unix)]
use crossterm::event::{KeyCode, KeyModifiers};
use klip_cli::api::IpcDaemonApi;
use klip_cli::client::DaemonClient;
use klip_crypto::identity::KeyStore;
use klip_daemon::{DaemonConfig, DaemonEngine, KlipPaths, RelayMode};
use klip_platform::memory::MemoryClipboardDriver;
use klip_proto::ipc::{deserialize_ipc_msg, serialize_ipc_msg, IpcRequest, IpcResponse};
use klip_tui::testkit::Harness;
use klip_tui::theme::TuiEnv;
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
    async fn stop(self) {
        let _ = ipc(&self.sock(), IpcRequest::StopDaemon).await;
        let _ = tokio::time::timeout(Duration::from_secs(10), self.task).await;
    }
}

fn fresh(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("klip-tuie2e-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

async fn start_daemon_with(
    name: &str,
    dir: PathBuf,
    tweak: impl FnOnce(&mut DaemonConfig),
) -> TestDaemon {
    let paths = KlipPaths::from_dir(&dir);
    let (_ks, identity) = KeyStore::load_or_create(paths.keystore(), name).unwrap();
    let clipboard = Arc::new(MemoryClipboardDriver::new(identity.node_id));
    let mut config = DaemonConfig {
        device_name: name.to_string(),
        relay: RelayMode::Lan,
        poll_interval_ms: 20,
        lan_discovery: false,
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

async fn start_daemon(name: &str, dir: PathBuf) -> TestDaemon {
    start_daemon_with(name, dir, |_| {}).await
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
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    cond()
}

/// A TUI for `d`, wired exactly like the real binary does it.
fn tui_for(d: &TestDaemon) -> Harness {
    let paths = KlipPaths::from_dir(&d.dir);
    let api = IpcDaemonApi::new(DaemonClient::new(d.sock()), paths.clone());
    let env = TuiEnv::from_vars(paths, |k| (k == "NO_COLOR").then(|| "1".to_string()));
    Harness::new(Arc::new(api), env, 110, 40)
}

fn ctrl(h: &mut Harness, c: char) {
    h.key_mod(KeyCode::Char(c), KeyModifiers::CONTROL);
}
fn alt(h: &mut Harness, c: char) {
    h.key_mod(KeyCode::Char(c), KeyModifiers::ALT);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_real_daemons_pair_sync_copy_back_and_unpair_through_the_tui() {
    let a = start_daemon("Alpha", fresh("a")).await;
    let b = start_daemon("Beta", fresh("b")).await;
    let mut h = tui_for(&a);
    assert!(h.wait_for("ACTIVE", 10_000).await, "{}", h.screen());

    // Show a code with LAN addresses (these daemons have discovery off) and let Beta use it.
    h.text("3");
    assert!(
        h.wait_for("Ticket (single use):", 10_000).await,
        "{}",
        h.screen()
    );
    h.text("l");
    assert!(
        h.wait_for("LAN addresses included", 10_000).await,
        "{}",
        h.screen()
    );
    assert!(
        h.wait_for("Ticket (single use):", 10_000).await,
        "the new code replaces the old one: {}",
        h.screen()
    );
    let ticket =
        h.rt.app
            .pair
            .ticket
            .as_ref()
            .expect("a ticket")
            .ticket
            .clone();
    let beta_sock = b.sock();
    let join = tokio::spawn(async move { ipc(&beta_sock, IpcRequest::PairJoin { ticket }).await });

    // Beta's request appears as a modal with its fingerprint; nothing happens until we answer.
    let asked = h.wait_for("asking to pair", 15_000).await;
    if !asked && join.is_finished() {
        eprintln!("DIAG join finished early: {:?}", join.await);
        panic!(
            "Beta's join ended before Alpha showed a request: {}",
            h.screen()
        );
    }
    assert!(asked, "{}", h.screen());
    assert!(
        h.screen().contains(&b.engine.node_id().fingerprint()),
        "{}",
        h.screen()
    );
    h.text("y");
    match tokio::time::timeout(Duration::from_secs(20), join)
        .await
        .expect("join in time")
        .unwrap()
    {
        IpcResponse::PairSuccess { peer_name, .. } => assert_eq!(peer_name, "Alpha"),
        other => panic!("Beta could not join: {other:?}"),
    }
    assert!(
        h.wait_for("Linked with Beta", 15_000).await,
        "{}",
        h.screen()
    );

    // Devices screen lists Beta.
    h.text("2");
    assert!(h.wait_for("Devices (1)", 10_000).await, "{}", h.screen());
    assert!(h.screen().contains("Beta"));

    // Send text typed in the TUI: it must land in Beta's clipboard, byte for byte.
    h.text("5");
    h.key(KeyCode::Enter);
    h.text("hello from the tui\n\tsecond line 日本語");
    ctrl(&mut h, 's');
    assert!(h.wait_for("✓ Beta", 20_000).await, "{}", h.screen());
    assert!(
        wait_until(10, || b.clipboard.current_text().as_deref()
            == Some("hello from the tui\n\tsecond line 日本語"))
        .await,
        "Beta's clipboard: {:?}",
        b.clipboard.current_text()
    );

    // History shows it; Enter puts it back on Alpha's own clipboard.
    alt(&mut h, '4');
    assert!(
        h.wait_for("hello from the tui", 10_000).await,
        "{}",
        h.screen()
    );
    h.key(KeyCode::Enter);
    assert!(
        h.wait_for("Copied to this computer", 10_000).await,
        "{}",
        h.screen()
    );
    assert_eq!(
        a.clipboard.current_text().as_deref(),
        Some("hello from the tui\n\tsecond line 日本語")
    );

    // Pause from the menu, then unpair everything.
    h.key(KeyCode::Char('p'));
    h.key(KeyCode::Enter);
    assert!(h.wait_for("PAUSED", 10_000).await, "{}", h.screen());
    h.key(KeyCode::Char('p'));
    h.key(KeyCode::Enter); // "Resume now"
    assert!(h.wait_for("ACTIVE", 10_000).await, "{}", h.screen());
    h.text("2");
    h.text("a");
    assert!(h.screen().contains("Unpair ALL"), "{}", h.screen());
    h.text("y");
    assert!(h.wait_for("No devices yet", 10_000).await, "{}", h.screen());
    match ipc(&a.sock(), IpcRequest::GetStatus).await {
        IpcResponse::Status(s) => assert!(s.peers.is_empty()),
        other => panic!("{other:?}"),
    }
    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_tui_survives_the_daemon_stopping_and_coming_back() {
    let dir = fresh("restart");
    let a = start_daemon("Alpha", dir.clone()).await;
    let mut h = tui_for(&a);
    assert!(h.wait_for("ACTIVE", 10_000).await, "{}", h.screen());
    a.stop().await;
    assert!(
        h.wait_for("Klip is not running", 8_000).await,
        "{}",
        h.screen()
    );
    assert!(h.screen().contains("STOPPED"));
    let a2 = start_daemon("Alpha", dir).await;
    assert!(h.wait_for("ACTIVE", 15_000).await, "{}", h.screen());
    let mut asked_again = false;
    for _ in 0..80 {
        if h.rt.app.has_capability("copy_to_clipboard") {
            asked_again = true;
            break;
        }
        h.pump(50).await;
    }
    assert!(asked_again, "Hello must be asked again after reconnecting");
    a2.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pairing_request_waiting_for_an_answer_is_replaced_by_a_notice_when_the_daemon_dies() {
    let a = start_daemon("Alpha", fresh("die-a")).await;
    let b = start_daemon("Beta", fresh("die-b")).await;
    let mut h = tui_for(&a);
    assert!(h.wait_for("ACTIVE", 10_000).await);
    h.text("3");
    assert!(h.wait_for("Ticket (single use):", 10_000).await);
    h.text("l");
    assert!(h.wait_for("LAN addresses included", 10_000).await);
    assert!(h.wait_for("Ticket (single use):", 10_000).await);
    let ticket = h.rt.app.pair.ticket.as_ref().unwrap().ticket.clone();
    let beta_sock = b.sock();
    let _join = tokio::spawn(async move { ipc(&beta_sock, IpcRequest::PairJoin { ticket }).await });
    assert!(h.wait_for("asking to pair", 15_000).await, "{}", h.screen());
    a.stop().await;
    assert!(
        h.wait_for("Klip stopped answering", 10_000).await,
        "{}",
        h.screen()
    );
    assert!(h.screen().contains("STOPPED"));
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_huge_text_from_the_editor_is_refused_clearly_when_it_exceeds_the_daemons_limit() {
    let a = start_daemon_with("Alpha", fresh("limit"), |c| c.max_clip_bytes = 2000).await;
    let mut h = tui_for(&a);
    assert!(h.wait_for("ACTIVE", 10_000).await);
    h.text("5");
    h.key(KeyCode::Enter);
    h.paste(&"x".repeat(5000));
    ctrl(&mut h, 's');
    assert!(
        h.wait_for("is over the limit of", 5_000).await,
        "{}",
        h.screen()
    );
    assert!(
        h.screen().contains("1.9 KiB") || h.screen().contains("2,000 B"),
        "the limit is shown: {}",
        h.screen()
    );
    a.stop().await;
}
