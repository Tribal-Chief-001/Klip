//! Its own test binary (so no other test allocates concurrently): a client that announces a huge
//! request and then sends nothing must not make the daemon reserve that memory.
#![cfg(target_os = "linux")]

use klip_crypto::identity::KeyStore;
use klip_daemon::{DaemonConfig, DaemonEngine, KlipPaths, RelayMode};
use klip_platform::memory::MemoryClipboardDriver;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

fn vm_size_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find(|l| l.starts_with("VmSize:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .unwrap()
}

#[tokio::test]
async fn a_client_that_announces_a_huge_length_and_sends_nothing_costs_no_memory() {
    let dir = std::env::temp_dir().join(format!("klip-stall-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let paths = KlipPaths::from_dir(&dir);
    let (_ks, identity) = KeyStore::load_or_create(paths.keystore(), "Alpha").unwrap();
    let clipboard = Arc::new(MemoryClipboardDriver::new(identity.node_id));
    // Cap = 256 MiB + 64 KiB, so a 200 MB announcement is within the cap and used to be allocated up front.
    let config = DaemonConfig {
        device_name: "Alpha".into(),
        relay: RelayMode::Lan,
        lan_discovery: false,
        max_clip_bytes: 256 * 1024 * 1024,
        ..DaemonConfig::default()
    };
    let engine = Arc::new(
        DaemonEngine::init_with(config, paths, Some(clipboard))
            .await
            .unwrap(),
    );
    let runner = Arc::clone(&engine);
    let task = tokio::spawn(async move {
        let _ = runner.run().await;
    });
    let sock = dir.join("daemon.sock");
    while !sock.exists() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let before = vm_size_kib();
    let mut held = Vec::new();
    for _ in 0..8 {
        let mut s = UnixStream::connect(&sock).await.unwrap();
        s.write_all(&200_000_000u32.to_be_bytes()).await.unwrap();
        held.push(s); // keep the connections open, send no body
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let grown_mib = vm_size_kib().saturating_sub(before) / 1024;
    assert!(
        grown_mib < 400,
        "8 stalled requests grew the process by {grown_mib} MiB of address space"
    );
    drop(held);
    engine.shutdown.trigger();
    let _ = tokio::time::timeout(Duration::from_secs(10), task).await;
}
