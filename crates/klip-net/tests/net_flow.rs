//! End-to-end tests with real iroh endpoints in one process (relay and discovery disabled,
//! so nothing leaves the machine).

use klip_net::{
    EndpointId, KlipNode, NetConfig, NetError, NetEvent, PairingTicket, PeerRecord, RelayChoice,
};
use klip_proto::hlc::{HybridLogicalClock, NodeId};
use klip_proto::wire::{ClipContentType, ClipPayload};
use std::time::Duration;
use tokio::sync::mpsc;

struct Harness {
    node: KlipNode,
    clips: mpsc::Receiver<(EndpointId, ClipPayload)>,
    paired: mpsc::Receiver<PeerRecord>,
}

fn cfg(name: &str) -> NetConfig {
    NetConfig {
        device_name: name.into(),
        relay: RelayChoice::Disabled,
        ..NetConfig::default()
    }
}

/// Start a node whose pairing requests are answered with `accept_pairs`.
async fn spawn_node(name: &str, accept_pairs: bool, cfg_override: Option<NetConfig>) -> Harness {
    let mut seed = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut seed);
    let (node, mut events) =
        KlipNode::start(cfg_override.unwrap_or_else(|| cfg(name)), &seed, vec![])
            .await
            .expect("start node");
    let (clip_tx, clips) = mpsc::channel(64);
    let (pair_tx, paired) = mpsc::channel(64);
    tokio::spawn(async move {
        while let Some(ev) = events.recv().await {
            match ev {
                NetEvent::Clip { from, payload } => {
                    let _ = clip_tx.send((from, payload)).await;
                }
                NetEvent::PairRequest { respond, .. } => {
                    let _ = respond.send(accept_pairs);
                }
                NetEvent::Paired(rec) => {
                    let _ = pair_tx.send(rec).await;
                }
                _ => {}
            }
        }
    });
    Harness {
        node,
        clips,
        paired,
    }
}

fn clip_from(node: &KlipNode, text: &str) -> ClipPayload {
    let nid = NodeId::new(*node.id().as_bytes());
    let hlc = HybridLogicalClock::new(nid).send_event();
    ClipPayload::new(
        ClipContentType::PlainTextUtf8,
        text.as_bytes().to_vec(),
        nid,
        hlc,
        false,
    )
}

async fn pair(host: &Harness, joiner: &Harness) {
    let ticket = host.node.open_pairing(Duration::from_secs(60)).await;
    // Round-trip through the text form exactly like a user copying/scanning it.
    let ticket = PairingTicket::decode(&ticket.encode()).unwrap();
    joiner
        .node
        .pair_with(&ticket)
        .await
        .expect("pairing succeeds");
}

async fn expect_clip(h: &mut Harness, secs: u64) -> Option<(EndpointId, ClipPayload)> {
    tokio::time::timeout(Duration::from_secs(secs), h.clips.recv())
        .await
        .ok()
        .flatten()
}

#[tokio::test]
async fn pairing_then_bidirectional_sync() {
    let mut host = spawn_node("Laptop", true, None).await;
    let mut phone = spawn_node("Phone", true, None).await;
    pair(&host, &phone).await;

    // Both sides trust each other and know the other's display name.
    assert_eq!(host.node.peers()[0].id, phone.node.id());
    assert_eq!(host.node.peers()[0].name, "Phone");
    assert_eq!(phone.node.peers()[0].id, host.node.id());
    assert_eq!(phone.node.peers()[0].name, "Laptop");
    assert_eq!(host.paired.recv().await.unwrap().id, phone.node.id());
    assert_eq!(phone.paired.recv().await.unwrap().id, host.node.id());
    // The pairing window is single-use and now closed.
    assert!(!host.node.pairing_open());

    // joiner -> host
    let res = phone
        .node
        .send_clip(&clip_from(&phone.node, "hello from phone"))
        .await;
    assert!(res.iter().all(|(_, r)| r.is_ok()), "send failed: {res:?}");
    let (from, got) = expect_clip(&mut host, 10)
        .await
        .expect("host receives clip");
    assert_eq!(from, phone.node.id());
    assert_eq!(got.content, b"hello from phone");

    // host -> joiner (host must be able to dial the joiner it only met via an incoming pairing)
    let res = host
        .node
        .send_clip(&clip_from(&host.node, "hello from laptop"))
        .await;
    assert!(res.iter().all(|(_, r)| r.is_ok()), "send failed: {res:?}");
    let (from, got) = expect_clip(&mut phone, 10)
        .await
        .expect("phone receives clip");
    assert_eq!(from, host.node.id());
    assert_eq!(got.content, b"hello from laptop");

    host.node.shutdown().await;
    phone.node.shutdown().await;
}

#[tokio::test]
async fn wrong_secret_is_rejected_and_ticket_is_single_use() {
    let host = spawn_node("Laptop", true, None).await;
    let good = spawn_node("Phone", true, None).await;
    let evil = spawn_node("Evil", true, None).await;

    let ticket = host.node.open_pairing(Duration::from_secs(60)).await;
    let tampered = PairingTicket::decode(&ticket.encode()).unwrap();
    // Flip the secret by re-encoding a ticket of the same host with a different secret.
    let mut bad = tampered.clone();
    bad.secret[0] ^= 0xff;
    let err = evil.node.pair_with(&bad).await.unwrap_err();
    assert!(matches!(err, NetError::PairingRejected(_)), "{err}");
    assert!(host.node.peers().is_empty());

    // The genuine ticket still works exactly once.
    good.node
        .pair_with(&ticket)
        .await
        .expect("genuine ticket works");
    let err = evil.node.pair_with(&ticket).await.unwrap_err();
    assert!(
        matches!(err, NetError::PairingRejected(_)),
        "reuse must fail: {err}"
    );
    assert_eq!(host.node.peers().len(), 1);
    assert_eq!(host.node.peers()[0].id, good.node.id());
}

#[tokio::test]
async fn too_many_wrong_guesses_lock_the_window() {
    let host = spawn_node("Laptop", true, None).await;
    let evil = spawn_node("Evil", true, None).await;
    let ticket = host.node.open_pairing(Duration::from_secs(60)).await;
    for i in 0..5u8 {
        let mut bad = ticket.clone();
        bad.secret[1] = i.wrapping_add(1);
        bad.secret[2] = 0xAA;
        assert!(evil.node.pair_with(&bad).await.is_err());
    }
    assert!(
        !host.node.pairing_open(),
        "window must close after repeated failures"
    );
    // Even the correct secret no longer works.
    assert!(evil.node.pair_with(&ticket).await.is_err());
    assert!(host.node.peers().is_empty());
}

#[tokio::test]
async fn expired_or_closed_window_is_rejected() {
    let host = spawn_node("Laptop", true, None).await;
    let phone = spawn_node("Phone", true, None).await;

    // Client-side expiry check.
    let ticket = host.node.open_pairing(Duration::from_secs(60)).await;
    let mut old = ticket.clone();
    old.expires_unix = 1;
    assert!(matches!(
        phone.node.pair_with(&old).await,
        Err(NetError::Ticket(_))
    ));

    // Host-side expiry/closing.
    host.node.close_pairing();
    let err = phone.node.pair_with(&ticket).await.unwrap_err();
    assert!(matches!(err, NetError::PairingRejected(_)), "{err}");

    let t = host.node.open_pairing(Duration::from_millis(50)).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let mut t = t;
    t.expires_unix = u64::MAX; // pretend the client clock is wrong; the host must still refuse
    let err = phone.node.pair_with(&t).await.unwrap_err();
    assert!(matches!(err, NetError::PairingRejected(_)), "{err}");
    assert!(host.node.peers().is_empty());
}

#[tokio::test]
async fn host_can_decline_a_valid_request() {
    let host = spawn_node("Laptop", false, None).await; // declines everything
    let phone = spawn_node("Phone", true, None).await;
    let ticket = host.node.open_pairing(Duration::from_secs(60)).await;
    let err = phone.node.pair_with(&ticket).await.unwrap_err();
    assert!(matches!(err, NetError::PairingRejected(_)), "{err}");
    assert!(host.node.peers().is_empty());
    assert!(phone.node.peers().is_empty());
}

#[tokio::test]
async fn unpaired_device_cannot_deliver_clips() {
    let mut host = spawn_node("Laptop", true, None).await;
    let stranger = spawn_node("Stranger", true, None).await;
    // The stranger believes it is paired (e.g. host removed it, or it forged its own list) ...
    let mut seed = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut seed);
    let (rogue, _ev) = KlipNode::start(
        cfg("Rogue"),
        &seed,
        vec![PeerRecord {
            id: host.node.id(),
            name: "Laptop".into(),
        }],
    )
    .await
    .unwrap();
    drop(stranger);
    // ... but the host never trusted it. The host's direct address is only known to a node that
    // has talked to it, so give the rogue an address through the ticket path of another channel:
    let ticket = host.node.open_pairing(Duration::from_secs(5)).await;
    let _ = rogue
        .pair_with(&{
            let mut t = ticket.clone();
            t.secret = [9u8; 16]; // wrong -> rejected, but the rogue learns how to reach the host
            t
        })
        .await;
    let res = rogue.send_clip(&clip_from(&rogue, "malicious")).await;
    assert!(
        res.iter().all(|(_, r)| r.is_err()),
        "unpaired send must fail: {res:?}"
    );
    assert!(
        expect_clip(&mut host, 2).await.is_none(),
        "host must not receive anything"
    );
}

#[tokio::test]
async fn forged_origin_and_oversize_are_dropped() {
    let mut host = spawn_node("Laptop", true, None).await;
    let phone = spawn_node("Phone", true, None).await;
    pair(&host, &phone).await;

    // A paired peer claiming another device's identity as the origin is ignored.
    let other = NodeId::new([42u8; 32]);
    let hlc = HybridLogicalClock::new(other).send_event();
    let forged = ClipPayload::new(
        ClipContentType::PlainTextUtf8,
        b"forged".to_vec(),
        other,
        hlc,
        false,
    );
    let res = phone.node.send_clip(&forged).await;
    assert!(
        res.iter().all(|(_, r)| r.is_err()),
        "sender must learn the clip was rejected: {res:?}"
    );
    assert!(
        expect_clip(&mut host, 2).await.is_none(),
        "forged origin must be dropped"
    );

    // The sender refuses clips above its own limit.
    let small = NetConfig {
        max_clip_bytes: 1024,
        ..cfg("Tiny")
    };
    let tiny = spawn_node("Tiny", true, Some(small)).await;
    pair(&host, &tiny).await;
    let big = clip_from(&tiny.node, &"x".repeat(2048));
    let res = tiny.node.send_clip(&big).await;
    assert!(
        res.iter()
            .all(|(_, r)| matches!(r, Err(NetError::TooLarge { .. }))),
        "{res:?}"
    );

    // A receiver enforces its limit even if the sender's limit is larger.
    let small_rx = NetConfig {
        max_clip_bytes: 1024,
        ..cfg("SmallRx")
    };
    let mut rx_node = spawn_node("SmallRx", true, Some(small_rx)).await;
    pair(&rx_node, &phone).await;
    let big_for_rx = clip_from(&phone.node, &"y".repeat(4096));
    let res = phone.node.send_clip(&big_for_rx).await;
    assert!(
        res.iter()
            .any(|(id, r)| *id == rx_node.node.id() && r.is_err()),
        "{res:?}"
    );
    assert!(
        expect_clip(&mut rx_node, 2).await.is_none(),
        "oversize clip must be dropped by receiver"
    );
}

#[tokio::test]
async fn large_clip_and_unpair() {
    let mut host = spawn_node("Laptop", true, None).await;
    let phone = spawn_node("Phone", true, None).await;
    pair(&host, &phone).await;

    let big = "A".repeat(5 * 1024 * 1024);
    let res = phone.node.send_clip(&clip_from(&phone.node, &big)).await;
    assert!(res.iter().all(|(_, r)| r.is_ok()), "{res:?}");
    let (_, got) = expect_clip(&mut host, 30)
        .await
        .expect("5 MiB clip delivered");
    assert_eq!(got.content.len(), 5 * 1024 * 1024);

    // After the host unpairs the phone, nothing further is delivered.
    assert!(host.node.unpair(&phone.node.id()));
    let _ = phone
        .node
        .send_clip(&clip_from(&phone.node, "after unpair"))
        .await;
    assert!(expect_clip(&mut host, 2).await.is_none());
}
