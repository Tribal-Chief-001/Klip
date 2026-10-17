//! Files between real iroh endpoints in one process (relay and discovery off, nothing leaves the machine).

use klip_net::file::{
    send_file, send_file_v2, FileError, FileHeader, FileHeaderV2, IncomingFile, RejectCode,
};
use klip_net::{
    EndpointId, FileConnection, KlipNode, NetConfig, NetError, NetEvent, PairingTicket, PeerRecord,
    RecvStream, RelayChoice, SendStream,
};
use klip_proto::clip::{ClipContentType, ClipPayload};
use klip_proto::hlc::{HybridLogicalClock, NodeId};
use std::time::Duration;
use tokio::sync::mpsc;

type Incoming = (EndpointId, SendStream, RecvStream, FileConnection, u8);

struct Harness {
    node: KlipNode,
    files: mpsc::Receiver<Incoming>,
}

fn cfg(name: &str, file_transfer: bool, file_v2: bool) -> NetConfig {
    NetConfig {
        device_name: name.into(),
        relay: RelayChoice::Disabled,
        lan_discovery: false,
        file_transfer,
        file_v2,
        ..NetConfig::default()
    }
}

async fn spawn_node(name: &str, file_transfer: bool, known: Vec<PeerRecord>) -> Harness {
    spawn_node_v(name, file_transfer, true, known).await
}

async fn spawn_node_v(
    name: &str,
    file_transfer: bool,
    file_v2: bool,
    known: Vec<PeerRecord>,
) -> Harness {
    let mut seed = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut seed);
    let (node, mut events) = KlipNode::start(cfg(name, file_transfer, file_v2), &seed, known)
        .await
        .expect("start node");
    let (file_tx, files) = mpsc::channel(8);
    tokio::spawn(async move {
        while let Some(ev) = events.recv().await {
            match ev {
                NetEvent::Clip { ack, .. } => {
                    let _ = ack.send(klip_net::ClipOutcome::Applied);
                }
                NetEvent::PairRequest { respond, .. } => {
                    let _ = respond.send(true);
                }
                NetEvent::File {
                    from,
                    tx,
                    rx,
                    conn,
                    version,
                } => {
                    let _ = file_tx.send((from, tx, rx, conn, version)).await;
                }
                _ => {}
            }
        }
    });
    Harness { node, files }
}

async fn pair(host: &Harness, joiner: &Harness) {
    let ticket = host.node.open_pairing(Duration::from_secs(60), true).await;
    let ticket = PairingTicket::decode(&ticket.encode()).unwrap();
    joiner
        .node
        .pair_with(&ticket)
        .await
        .expect("pairing succeeds");
}

fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i.wrapping_mul(13) % 251) as u8).collect()
}

fn header(name: &str, size: usize) -> FileHeader {
    FileHeader {
        version: 1,
        id: [4; 16],
        name: name.into(),
        size: size as u64,
    }
}

#[tokio::test]
async fn a_paired_device_sends_a_file_and_the_other_one_receives_it() {
    let mut laptop = spawn_node("Laptop", true, vec![]).await;
    let phone = spawn_node("Phone", true, vec![]).await;
    pair(&laptop, &phone).await;

    let data = pattern(3_000_000);
    let expected = data.clone();
    let receiver = tokio::spawn(async move {
        let (from, tx, rx, conn, _version) = laptop.files.recv().await.expect("a file arrives");
        let incoming = IncomingFile::open(rx, tx).await.unwrap();
        assert_eq!(incoming.header().name, "holiday.mp4");
        let mut sink = Vec::new();
        let received = incoming.accept(&mut sink, &mut |_| {}).await.unwrap();
        received.ok().await;
        conn.finished().await;
        (from, sink)
    });

    let mut streams = phone
        .node
        .open_file_stream(laptop_id(&receiver, &phone))
        .await
        .expect("the file stream opens");
    send_file(
        &header("holiday.mp4", data.len()),
        &data[..],
        &mut streams.tx,
        &mut streams.rx,
        &mut |_| {},
    )
    .await
    .expect("the transfer succeeds");
    streams.close();
    let (from, got) = receiver.await.unwrap();
    assert_eq!(from, phone.node.id());
    assert_eq!(got, expected);
}

/// The id of the other device: the phone's only paired peer is the laptop.
fn laptop_id<T>(_keep: &T, phone: &Harness) -> EndpointId {
    phone.node.peers()[0].id
}

#[tokio::test]
async fn a_device_that_is_not_paired_cannot_send_a_file() {
    let mut host = spawn_node("Laptop", true, vec![]).await;
    let mut seed = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut seed);
    // The rogue believes it is paired with the host; the host never trusted it.
    let (rogue, _ev) = KlipNode::start(
        cfg("Rogue", true, true),
        &seed,
        vec![PeerRecord {
            id: host.node.id(),
            name: "Laptop".into(),
        }],
    )
    .await
    .unwrap();
    // Let the rogue learn how to reach the host (a failed pairing attempt gives it the address).
    let ticket = host.node.open_pairing(Duration::from_secs(5), true).await;
    let mut wrong = ticket.clone();
    wrong.secret = [9u8; 16];
    let _ = rogue.pair_with(&wrong).await;

    let attempt = async {
        let mut streams = rogue.open_file_stream(host.node.id()).await?;
        let data = pattern(1000);
        send_file(
            &header("evil.sh", 1000),
            &data[..],
            &mut streams.tx,
            &mut streams.rx,
            &mut |_| {},
        )
        .await
        .map_err(|e| NetError::Protocol(e.to_string()))
    };
    let result = tokio::time::timeout(Duration::from_secs(10), attempt)
        .await
        .expect("the attempt ends");
    assert!(
        result.is_err(),
        "an unpaired device must not get a file through"
    );
    let nothing = tokio::time::timeout(Duration::from_secs(1), host.files.recv()).await;
    assert!(nothing.is_err(), "the host must not see a file event");
}

#[tokio::test]
async fn a_device_without_file_support_is_reported_as_such_not_as_offline() {
    let phone = spawn_node("Phone", true, vec![]).await;
    let old_laptop = spawn_node("Old laptop", false, vec![]).await;
    pair(&old_laptop, &phone).await;
    // The clip connection is up (a clip goes through) ...
    let nid = NodeId::new(*phone.node.id().as_bytes());
    let clip = ClipPayload::new(
        ClipContentType::PlainTextUtf8,
        b"hello".to_vec(),
        nid,
        HybridLogicalClock::new(nid).send_event(),
        false,
    );
    let sent = phone.node.send_clip(&clip).await;
    assert!(sent.iter().all(|(_, r)| r.is_ok()), "{sent:?}");
    // ... so a failure to open the file connection means "does not support it".
    let err = phone
        .node
        .open_file_stream(old_laptop.node.id())
        .await
        .err()
        .expect("no file connection");
    assert!(matches!(err, NetError::FileUnsupported), "{err}");
    assert!(err.to_string().to_lowercase().contains("update klip"));
}

#[tokio::test]
async fn a_refusal_from_the_other_side_comes_back_through_the_streams() {
    let mut laptop = spawn_node("Laptop", true, vec![]).await;
    let phone = spawn_node("Phone", true, vec![]).await;
    pair(&laptop, &phone).await;
    tokio::spawn(async move {
        let (_from, tx, rx, conn, _v) = laptop.files.recv().await.unwrap();
        let incoming = IncomingFile::open(rx, tx).await.unwrap();
        incoming.reject(RejectCode::NoSpace, "no room").await;
        conn.finished().await;
    });
    let id = phone.node.peers()[0].id;
    let mut streams = phone.node.open_file_stream(id).await.unwrap();
    let data = pattern(100);
    let err = send_file(
        &header("a.bin", 100),
        &data[..],
        &mut streams.tx,
        &mut streams.rx,
        &mut |_| {},
    )
    .await
    .unwrap_err();
    assert!(
        matches!(
            err,
            FileError::Rejected {
                code: RejectCode::NoSpace,
                ..
            }
        ),
        "{err:?}"
    );
    streams.close();
}

#[tokio::test]
async fn two_nodes_that_speak_both_versions_use_version_two_and_can_resume() {
    let mut laptop = spawn_node("Laptop", true, vec![]).await;
    let phone = spawn_node("Phone", true, vec![]).await;
    pair(&laptop, &phone).await;
    let laptop_id = phone.node.peers()[0].id;

    let data = pattern(2_000_000);
    let have = 700_000usize;
    let want = data[have..].to_vec();
    let prefix_data = data[..have].to_vec();
    let receiver = tokio::spawn(async move {
        let (_from, tx, rx, conn, version) = laptop.files.recv().await.expect("a file arrives");
        assert_eq!(version, 2);
        let incoming = IncomingFile::open_v2(rx, tx).await.unwrap();
        assert_eq!(incoming.v2().unwrap().resume_key, [8; 32]);
        let mut prefix = blake3::Hasher::new();
        prefix.update(&prefix_data);
        let mut sink = Vec::new();
        let received = incoming
            .accept_from(have as u64, prefix, &mut sink, &mut |_| {})
            .await
            .unwrap();
        received.ok().await;
        conn.finished().await;
        sink
    });
    let mut streams = phone.node.open_file_stream(laptop_id).await.unwrap();
    assert_eq!(streams.version(), 2);
    let header = FileHeaderV2 {
        version: 2,
        id: [4; 16],
        name: "big.bin".into(),
        size: data.len() as u64,
        resume_key: [8; 32],
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
    .expect("the resumed transfer works over real QUIC");
    streams.close();
    assert_eq!(receiver.await.unwrap(), want);
}

#[tokio::test]
async fn a_peer_that_only_speaks_version_one_gets_version_one() {
    let mut laptop = spawn_node_v("Laptop", true, false, vec![]).await;
    let phone = spawn_node("Phone", true, vec![]).await;
    pair(&laptop, &phone).await;
    let laptop_id = phone.node.peers()[0].id;

    let receiver = tokio::spawn(async move {
        let (_from, tx, rx, conn, version) = laptop.files.recv().await.expect("a file arrives");
        assert_eq!(version, 1);
        let incoming = IncomingFile::open(rx, tx).await.unwrap();
        let mut sink = Vec::new();
        let received = incoming.accept(&mut sink, &mut |_| {}).await.unwrap();
        received.ok().await;
        conn.finished().await;
        sink.len()
    });
    let mut streams = phone.node.open_file_stream(laptop_id).await.unwrap();
    assert_eq!(
        streams.version(),
        1,
        "the dial fell back to the version both speak"
    );
    let data = pattern(100_000);
    send_file(
        &header("old.bin", data.len()),
        &data[..],
        &mut streams.tx,
        &mut streams.rx,
        &mut |_| {},
    )
    .await
    .unwrap();
    streams.close();
    assert_eq!(receiver.await.unwrap(), 100_000);
}
