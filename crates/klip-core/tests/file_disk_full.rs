//! A disk that fills up in the middle of a file (simulated with a file-size limit for this process,
//! so the write fails the way a full disk does): the transfer fails, the sender is told, and nothing
//! is left in the receive folder. Its own test binary, because the limit applies to the whole process.
#![cfg(unix)]

use klip_core::files::{receive_incoming, FilePolicy, TransferRegistry};
use klip_net::file::{send_file, FileError, FileHeader, IncomingFile};
use klip_proto::ipc::TransferState;
use tokio::io::duplex;

#[tokio::test]
async fn a_write_error_in_the_middle_removes_the_partial_file_and_tells_the_sender() {
    let dir = std::env::temp_dir().join(format!("klip-diskfull-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut policy = FilePolicy::new(dir.clone());
    policy.reserve_bytes = 0;
    let reg = TransferRegistry::new();

    // SAFETY: plain libc calls at the start of the only test in this process.
    unsafe {
        libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
        let limit = libc::rlimit {
            rlim_cur: 100_000,
            rlim_max: 100_000,
        };
        assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &limit), 0);
    }

    let bytes: Vec<u8> = (0..1_000_000).map(|i| (i % 251) as u8).collect();
    let header = FileHeader {
        version: 1,
        id: [3; 16],
        name: "big.bin".into(),
        size: bytes.len() as u64,
    };
    let (s_w, s_r) = duplex(64 * 1024);
    let (r_w, r_r) = duplex(64 * 1024);
    let sender = async move {
        let (mut tx, mut rx) = (s_w, r_r);
        send_file(&header, &bytes[..], &mut tx, &mut rx, &mut |_| {}).await
    };
    let receiver = async {
        let incoming = IncomingFile::open(s_r, r_w).await?;
        receive_incoming(incoming, "Vivo", &policy, &reg).await
    };
    let (sent, got) = tokio::join!(sender, receiver);

    assert!(matches!(got, Err(FileError::Io(_))), "{got:?}");
    assert!(matches!(sent, Err(FileError::Peer(_))), "{sent:?}");
    let left: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
    assert!(left.is_empty(), "nothing is left behind: {left:?}");
    let info = &reg.list()[0];
    assert_eq!(info.state, TransferState::Failed);
    assert!(info.done < 1_000_000);
}
