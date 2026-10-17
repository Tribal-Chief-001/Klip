//! `klip send-file` and `klip transfers` against two real, isolated, paired daemons (the real binary,
//! real iroh endpoints on the loopback network, in-memory clipboards, nothing leaves the machine).
#![cfg(unix)]
mod common;
use common::{text, Isolated};
use klip_daemon::KlipPaths;
use klip_proto::ipc::{deserialize_ipc_msg, serialize_ipc_msg, IpcRequest, IpcResponse};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

async fn ipc(dir: &Path, req: IpcRequest) -> IpcResponse {
    let mut s = UnixStream::connect(KlipPaths::from_dir(dir).ipc_socket())
        .await
        .unwrap();
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

/// Two daemons, `Alpha` (sends) and `Beta` (receives into the returned folder), paired and connected.
fn two_daemons(tag: &str, beta_extra: &str) -> (Isolated, Isolated, PathBuf) {
    let alpha = Isolated::start(
        &format!("{tag}-alpha"),
        r#"{"relay":"lan","lan_discovery":false,"device_name":"Alpha"}"#,
    );
    let beta = Isolated::create(&format!("{tag}-beta"), "{}");
    let incoming = beta.dir.join("incoming");
    std::fs::write(
        beta.dir.join("config.json"),
        format!(
            r#"{{"relay":"lan","lan_discovery":false,"device_name":"Beta","receive_dir":"{}"{beta_extra}}}"#,
            incoming.display()
        ),
    )
    .unwrap();
    let out = beta.run(&["start"], None);
    assert!(out.status.success(), "{}", text(&out));

    rt().block_on(async {
        let IpcResponse::PairTicket { ticket, .. } = ipc(
            &beta.dir,
            IpcRequest::PairStart {
                ttl_secs: 60,
                include_lan_addrs: true,
            },
        )
        .await
        else {
            panic!("ticket")
        };
        let alpha_dir = alpha.dir.clone();
        let join =
            tokio::spawn(async move { ipc(&alpha_dir, IpcRequest::PairJoin { ticket }).await });
        let end = Instant::now() + Duration::from_secs(20);
        loop {
            let IpcResponse::PairPoll(p) = ipc(&beta.dir, IpcRequest::PairPoll).await else {
                panic!()
            };
            if p.pending.is_some() {
                break;
            }
            assert!(Instant::now() < end, "no pairing request arrived");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        ipc(&beta.dir, IpcRequest::PairDecide { accept: true }).await;
        assert!(matches!(
            join.await.unwrap(),
            IpcResponse::PairSuccess { .. }
        ));
        let end = Instant::now() + Duration::from_secs(30);
        loop {
            let IpcResponse::Status(s) = ipc(&alpha.dir, IpcRequest::GetStatus).await else {
                panic!()
            };
            if s.peers.iter().any(|p| p.is_online) {
                break;
            }
            assert!(Instant::now() < end, "Alpha never saw Beta online");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    (alpha, beta, incoming)
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn run_in(d: &Isolated, cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_klip"))
        .args(args)
        .current_dir(cwd)
        .env("KLIP_CONFIG_DIR", &d.dir)
        .env("KLIP_CLIPBOARD", "memory")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn sample(dir: &Path, name: &str, bytes: usize) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let p = dir.join(name);
    std::fs::write(
        &p,
        (0..bytes).map(|i| (i * 7 % 253) as u8).collect::<Vec<u8>>(),
    )
    .unwrap();
    p
}

#[test]
fn send_file_delivers_files_with_equal_content_and_transfers_lists_them() {
    let (alpha, _beta, incoming) = two_daemons("deliver", "");
    let src = alpha.dir.join("out");
    let a = sample(&src, "report.pdf", 5_000_000);
    let b = sample(&src, "notes ü.txt", 1234);
    // A relative path works too: the CLI makes it absolute before the daemon sees it.
    let out = run_in(
        &alpha,
        &src,
        &["send-file", "report.pdf", "notes ü.txt", "--to", "Beta"],
    );
    assert!(out.status.success(), "{}", text(&out));
    let shown = text(&out);
    assert!(
        shown.contains("report.pdf") && shown.contains("Beta"),
        "{shown}"
    );
    assert_eq!(
        std::fs::read(incoming.join("report.pdf")).unwrap(),
        std::fs::read(a).unwrap()
    );
    assert_eq!(
        std::fs::read(incoming.join("notes ü.txt")).unwrap(),
        std::fs::read(b).unwrap()
    );

    let listed = text(&alpha.run(&["transfers"], None));
    assert!(
        listed.contains("report.pdf") && listed.contains("done"),
        "{listed}"
    );
    let theirs = text(&_beta.run(&["transfers"], None));
    assert!(theirs.contains("←") && theirs.contains("Alpha"), "{theirs}");
}

#[test]
fn send_file_with_a_missing_file_fails_clearly_and_still_sends_the_others() {
    let (alpha, _beta, incoming) = two_daemons("missing", "");
    let ok = sample(&alpha.dir.join("out"), "ok.txt", 100);
    let out = alpha.run(
        &["send-file", "/no/such/file.bin", ok.to_str().unwrap()],
        None,
    );
    assert!(!out.status.success());
    let shown = text(&out);
    assert!(shown.contains("/no/such/file.bin"), "{shown}");
    assert_eq!(files_in(&incoming), ["ok.txt"]);
}

#[test]
fn send_file_says_why_when_the_other_device_does_not_accept_files() {
    let (alpha, _beta, incoming) = two_daemons("refuse", r#","accept_files":false"#);
    let f = sample(&alpha.dir.join("out"), "x.txt", 100);
    let out = alpha.run(&["send-file", f.to_str().unwrap()], None);
    assert!(!out.status.success());
    assert!(text(&out).contains("not accepting files"), "{}", text(&out));
    assert!(files_in(&incoming).is_empty());
}

#[test]
fn send_file_to_a_name_nobody_has_fails() {
    let (alpha, _beta, _incoming) = two_daemons("names", "");
    let f = sample(&alpha.dir.join("out"), "x.txt", 10);
    let out = alpha.run(&["send-file", f.to_str().unwrap(), "--to", "Nobody"], None);
    assert!(!out.status.success());
    assert!(text(&out).contains("no paired device"), "{}", text(&out));
}

#[test]
fn cancel_stops_a_running_transfer() {
    let (alpha, _beta, incoming) = two_daemons("cancel", "");
    let huge = alpha.dir.join("huge.bin");
    std::fs::File::create(&huge)
        .unwrap()
        .set_len(2 * 1024 * 1024 * 1024)
        .unwrap();
    let out = alpha.run(&["send-file", "--no-wait", huge.to_str().unwrap()], None);
    assert!(out.status.success(), "{}", text(&out));
    let end = Instant::now() + Duration::from_secs(30);
    let id = loop {
        let listed = text(&alpha.run(&["transfers"], None));
        if let Some(line) = listed
            .lines()
            .find(|l| l.contains("huge.bin") && l.contains('%') && !l.contains(" 0%"))
        {
            break line.split_whitespace().next().unwrap().to_string();
        }
        assert!(Instant::now() < end, "never started: {listed}");
        std::thread::sleep(Duration::from_millis(100));
    };
    let out = alpha.run(&["transfers", "--cancel", &id], None);
    assert!(out.status.success(), "{}", text(&out));
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        let listed = text(&alpha.run(&["transfers"], None));
        if listed.contains("stopped") {
            break;
        }
        assert!(Instant::now() < end, "not stopped: {listed}");
        std::thread::sleep(Duration::from_millis(100));
    }
    // A cut transfer leaves a hidden partial for a later resume, never a file under its real name.
    std::thread::sleep(Duration::from_millis(500));
    let shown: Vec<String> = files_in(&incoming)
        .into_iter()
        .filter(|n| !n.starts_with(".klip-"))
        .collect();
    assert!(shown.is_empty(), "left behind: {shown:?}");
    let end = Instant::now() + Duration::from_secs(0);
    while Instant::now() < end {
        std::thread::sleep(Duration::from_millis(100));
    }
    let again = alpha.run(&["transfers", "--cancel", "zzzz"], None);
    assert!(!again.status.success());
}

#[test]
fn a_file_command_without_a_running_daemon_says_so() {
    let d = Isolated::create("nodaemon", common::LAN_ONLY);
    let f = sample(&d.dir, "a.txt", 5);
    let out = d.run(&["send-file", f.to_str().unwrap()], None);
    assert!(!out.status.success());
    assert!(
        text(&out).to_lowercase().contains("not running") || text(&out).contains("klip start"),
        "{}",
        text(&out)
    );
    assert!(!d.run(&["transfers"], None).status.success());
}

#[test]
fn send_file_sends_a_folder_into_one_new_folder_and_an_empty_one_is_refused() {
    let (alpha, _beta, incoming) = two_daemons("folder", "");
    let top = alpha.dir.join("out").join("Trip");
    for (rel, n) in [
        ("a.txt", 10usize),
        ("Day 1/b.jpg", 2000),
        ("Day 1/raw/c.cr2", 70_000),
    ] {
        let p = top.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, vec![n as u8; n]).unwrap();
    }
    let out = alpha.run(&["send-file", top.to_str().unwrap()], None);
    assert!(out.status.success(), "{}", text(&out));
    for (rel, n) in [
        ("a.txt", 10usize),
        ("Day 1/b.jpg", 2000),
        ("Day 1/raw/c.cr2", 70_000),
    ] {
        assert_eq!(
            std::fs::read(incoming.join("Trip").join(rel)).unwrap(),
            vec![n as u8; n],
            "{rel}"
        );
    }
    let empty = alpha.dir.join("out").join("Empty");
    std::fs::create_dir_all(&empty).unwrap();
    let out = alpha.run(&["send-file", empty.to_str().unwrap()], None);
    assert!(!out.status.success());
    assert!(text(&out).contains("no files"), "{}", text(&out));
}
