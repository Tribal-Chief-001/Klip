//! Measures what an *idle* pair of connected Klip nodes costs (thread wakeups), to find and
//! verify power-hungry timers. Usage: cargo run -p klip-net --example idle_probe -- [keepalive_secs]
use klip_net::{KlipNode, NetConfig, NetEvent, RelayChoice};
use std::time::Duration;

fn thread_wakeups() -> Vec<(String, u64)> {
    let mut out = vec![];
    if let Ok(dir) = std::fs::read_dir("/proc/self/task") {
        for t in dir.flatten() {
            let p = t.path();
            let name = std::fs::read_to_string(p.join("comm"))
                .unwrap_or_default()
                .trim()
                .to_string();
            let sw = std::fs::read_to_string(p.join("status"))
                .unwrap_or_default()
                .lines()
                .find(|l| l.starts_with("voluntary_ctxt_switches:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
                .unwrap_or(0);
            out.push((
                format!("{name}#{}", p.file_name().unwrap().to_string_lossy()),
                sw,
            ));
        }
    }
    out.sort();
    out
}

#[tokio::main]
async fn main() {
    let keep: u64 = std::env::args()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    let cfg = |name: &str| NetConfig {
        device_name: name.into(),
        relay: RelayChoice::Disabled,
        lan_discovery: false,
        keep_alive: Duration::from_secs(keep),
        idle_timeout: Duration::from_secs((keep * 3).max(60)),
        ..NetConfig::default()
    };
    let (a, mut ea) = KlipNode::start(cfg("A"), &[1u8; 32], vec![]).await.unwrap();
    let (b, mut eb) = KlipNode::start(cfg("B"), &[2u8; 32], vec![]).await.unwrap();
    tokio::spawn(async move {
        while let Some(ev) = ea.recv().await {
            if let NetEvent::PairRequest { respond, .. } = ev {
                let _ = respond.send(true);
            }
        }
    });
    tokio::spawn(async move { while eb.recv().await.is_some() {} });
    let ticket = a.open_pairing(Duration::from_secs(60), true).await;
    b.pair_with(&ticket).await.unwrap();
    tokio::time::sleep(Duration::from_secs(8)).await; // let the connection settle
    println!(
        "connected: A sees {:?}",
        a.peer_statuses()
            .iter()
            .map(|p| p.connected)
            .collect::<Vec<_>>()
    );

    let before = thread_wakeups();
    tokio::time::sleep(Duration::from_secs(20)).await;
    let after = thread_wakeups();
    let mut total = 0.0;
    for (name, sw1) in &after {
        let sw0 = before
            .iter()
            .find(|(n, _)| n == name)
            .map(|x| x.1)
            .unwrap_or(0);
        let rate = (*sw1 - sw0) as f64 / 20.0;
        total += rate;
        if rate > 0.3 {
            println!("  {name:<28} {rate:6.1}/s");
        }
    }
    println!("TOTAL wakeups/s (both nodes in one process, keepalive {keep}s): {total:.1}");
}
