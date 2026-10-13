//! A stub that behaves like a v0.3.6 daemon: it does not know protocol 2, caps requests at 8 MiB.
#![cfg(unix)]
use klip_cli::client::{ClientError, DaemonClient, OLD_DAEMON_REQUEST_CAP};
use klip_cli::ops::{request_cap, unpair_all_compat};
use klip_proto::hlc::NodeId;
use klip_proto::ipc::{
    deserialize_ipc_msg, serialize_ipc_msg, DaemonStatusInfo, IpcRequest, IpcResponse,
    PeerStatusInfo,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;

fn status_with(names: &[&str]) -> DaemonStatusInfo {
    DaemonStatusInfo {
        is_active: true,
        is_paused: false,
        paused_remaining_secs: 0,
        platform: "stub".into(),
        memory_rss_bytes: 0,
        node_id: NodeId::ZERO,
        relay_mode: String::new(),
        relay_state: String::new(),
        pairing_open: false,
        peers: names
            .iter()
            .enumerate()
            .map(|(i, n)| PeerStatusInfo {
                node_id: NodeId::new([i as u8 + 1; 32]),
                name: n.to_string(),
                is_online: false,
                last_error: None,
            })
            .collect(),
        total_synced_clips: 0,
        last_sync_time_secs_ago: None,
        last_clip_preview: None,
        last_clip_size_bytes: 0,
    }
}

/// Serves requests like an old daemon: unknown variants are decode errors answered with Error.
fn spawn_old_daemon(dir: &Path, unpaired: Arc<Mutex<Vec<String>>>) {
    let sock = dir.join("old.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = listener.accept().await.unwrap();
            let unpaired = Arc::clone(&unpaired);
            tokio::spawn(async move {
                let mut len = [0u8; 4];
                if s.read_exact(&mut len).await.is_err() {
                    return;
                }
                let mut buf = vec![0u8; u32::from_be_bytes(len) as usize];
                if s.read_exact(&mut buf).await.is_err() {
                    return;
                }
                let reply = match deserialize_ipc_msg::<IpcRequest>(&buf) {
                    // The old daemon's enum ends at StopDaemon (index 11): 12.. are decode errors.
                    Ok(IpcRequest::Hello { .. })
                    | Ok(IpcRequest::UnpairAll)
                    | Ok(IpcRequest::CopyToClipboard { .. })
                    | Ok(IpcRequest::ClearHistory) => {
                        IpcResponse::Error("Found a variant that does not exist".into())
                    }
                    Ok(IpcRequest::GetStatus) => {
                        IpcResponse::Status(status_with(&["Beta", "Beta"]))
                    }
                    Ok(IpcRequest::Unpair { selector }) => {
                        unpaired.lock().unwrap().push(selector.clone());
                        IpcResponse::Unpaired {
                            name: "Beta".into(),
                        }
                    }
                    _ => IpcResponse::Error("unsupported in the stub".into()),
                };
                let out = serialize_ipc_msg(&reply).unwrap();
                let _ = s.write_all(&(out.len() as u32).to_be_bytes()).await;
                let _ = s.write_all(&out).await;
            });
        }
    });
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("klip-old-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[tokio::test]
async fn an_old_daemon_gets_the_8_mib_cap_and_oversize_text_is_refused_before_writing() {
    let dir = temp("cap");
    spawn_old_daemon(&dir, Arc::default());
    let client = DaemonClient::new(dir.join("old.sock"));
    assert_eq!(request_cap(&client).await, OLD_DAEMON_REQUEST_CAP);
    let big = IpcRequest::SendClip {
        text: "a".repeat(9_000_000),
    };
    match client.request(&big, OLD_DAEMON_REQUEST_CAP).await {
        Err(ClientError::TooLarge { size, max }) => {
            assert!(size > 9_000_000);
            assert_eq!(max, OLD_DAEMON_REQUEST_CAP);
        }
        other => panic!("expected TooLarge, got {other:?}"),
    }
}

#[tokio::test]
async fn unpair_all_falls_back_to_one_call_per_device_on_an_old_daemon() {
    let dir = temp("unpair");
    let log = Arc::new(Mutex::new(Vec::new()));
    spawn_old_daemon(&dir, Arc::clone(&log));
    let client = DaemonClient::new(dir.join("old.sock"));
    let (removed, failed) = unpair_all_compat(&client).await.unwrap();
    assert_eq!(removed, ["Beta", "Beta"]);
    assert!(failed.is_empty());
    assert_eq!(log.lock().unwrap().len(), 2, "one Unpair per device");
}

#[tokio::test]
async fn a_missing_socket_is_reported_as_the_daemon_being_down() {
    let client = DaemonClient::new(temp("down").join("none.sock"));
    match client
        .request(&IpcRequest::GetStatus, OLD_DAEMON_REQUEST_CAP)
        .await
    {
        Err(ClientError::Down(m)) => assert!(m.contains("not running"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn the_tui_api_reports_an_old_daemon_as_unsupported_and_still_unpairs_everyone() {
    use klip_cli::api::IpcDaemonApi;
    use klip_tui::api::{ApiError, DaemonApi};
    let dir = temp("api");
    let log = Arc::new(Mutex::new(Vec::new()));
    spawn_old_daemon(&dir, Arc::clone(&log));
    let api = IpcDaemonApi::new(
        DaemonClient::new(dir.join("old.sock")),
        klip_daemon::KlipPaths::from_dir(dir.join("cfg")),
    );
    assert_eq!(api.hello().await.unwrap_err(), ApiError::Unsupported);
    assert!(api.status().await.is_ok());
    let (removed, failed) = api.unpair_all().await.unwrap();
    assert_eq!((removed.len(), failed.len()), (2, 0));
}
