//! The file transfer protocol: one file per bidirectional stream, generic over any async byte pipe.
//!
//! sender -> receiver: a header frame; receiver -> sender: accept or reject; then exactly `size` raw
//! bytes, the 32-byte BLAKE3 of those bytes, and finally the receiver's verdict. A frame is a big-endian
//! `u32` length and a postcard body, and the length is checked before the body is read, so a hostile peer
//! cannot make us allocate. Every wait has an idle timeout. Policy (where the bytes go, how much room is
//! left, which names are allowed) is the caller's: this module only moves and verifies bytes.
use serde::{Deserialize, Serialize};
use std::time::Duration;
use subtle::ConstantTimeEq;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::timeout;

/// ALPN of the file transfer protocol (paired peers only). The clip protocol is not touched.
pub const FILE_ALPN: &[u8] = b"klip/file/1";
/// Largest header, reply or verdict frame.
pub const MAX_FILE_FRAME: usize = 2048;
/// Size of the buffers file bytes move through (memory per transfer is about this).
pub const CHUNK: usize = 256 * 1024;
/// No byte for this long ends the transfer.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// The receiver may need a while to flush and move a big file before it can say "done".
pub const DONE_TIMEOUT: Duration = Duration::from_secs(300);
pub const MAX_NAME_CHARS: usize = 255;
const VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileHeader {
    pub version: u8,
    pub id: [u8; 16],
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RejectCode {
    Disabled,
    TooLarge,
    NoSpace,
    Busy,
    Paused,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileReply {
    Accept,
    Reject { code: RejectCode, text: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileDone {
    Ok,
    Bad { text: String },
}

#[derive(Debug)]
pub enum FileError {
    Io(String),
    Protocol(String),
    Rejected {
        code: RejectCode,
        text: String,
    },
    /// The data ended before the announced size.
    TooShort,
    /// The source has more bytes than the announced size (it grew while being sent).
    TooLong,
    HashMismatch,
    Timeout,
    /// The other side says the file did not arrive intact or could not be saved.
    Peer(String),
    /// Somebody cancelled the transfer here.
    Cancelled,
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Protocol(e) => write!(f, "the other device sent something unexpected ({e})"),
            Self::Rejected { text, .. } => write!(f, "{text}"),
            Self::TooShort => write!(f, "the file ended before its announced size"),
            Self::TooLong => write!(f, "the file grew while it was being sent"),
            Self::HashMismatch => write!(f, "the file arrived damaged"),
            Self::Timeout => write!(f, "the other device stopped answering"),
            Self::Peer(text) => write!(f, "{text}"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::error::Error for FileError {}

fn io_err(e: std::io::Error) -> FileError {
    if e.kind() == std::io::ErrorKind::UnexpectedEof {
        FileError::TooShort
    } else {
        let text = e.to_string();
        if text.contains("stopped by peer") || text.contains("reset by peer") {
            FileError::Peer("the other device stopped the transfer".into())
        } else {
            FileError::Io(text)
        }
    }
}

/// The header of a frame body, checked: the version, and a name that is non-empty, short and has no NUL.
pub fn parse_header(bytes: &[u8]) -> Result<FileHeader, FileError> {
    let header: FileHeader =
        postcard::from_bytes(bytes).map_err(|e| FileError::Protocol(e.to_string()))?;
    if header.version != VERSION {
        return Err(FileError::Protocol(
            "unsupported file transfer version".into(),
        ));
    }
    if header.name.is_empty()
        || header.name.chars().count() > MAX_NAME_CHARS
        || header.name.contains('\0')
    {
        return Err(FileError::Protocol("unusable file name".into()));
    }
    Ok(header)
}

async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(
    w: &mut W,
    value: &T,
) -> Result<(), FileError> {
    let body = postcard::to_stdvec(value).map_err(|e| FileError::Protocol(e.to_string()))?;
    if body.len() > MAX_FILE_FRAME {
        return Err(FileError::Protocol("frame too large".into()));
    }
    timeout(IDLE_TIMEOUT, async {
        w.write_all(&(body.len() as u32).to_be_bytes()).await?;
        w.write_all(&body).await?;
        w.flush().await
    })
    .await
    .map_err(|_| FileError::Timeout)?
    .map_err(io_err)
}

async fn read_frame<R: AsyncRead + Unpin>(r: &mut R, wait: Duration) -> Result<Vec<u8>, FileError> {
    let mut len = [0u8; 4];
    timeout(wait, r.read_exact(&mut len))
        .await
        .map_err(|_| FileError::Timeout)?
        .map_err(io_err)?;
    let n = u32::from_be_bytes(len) as usize;
    if n == 0 || n > MAX_FILE_FRAME {
        return Err(FileError::Protocol("frame size out of range".into()));
    }
    let mut body = vec![0u8; n];
    timeout(wait, r.read_exact(&mut body))
        .await
        .map_err(|_| FileError::Timeout)?
        .map_err(io_err)?;
    Ok(body)
}

/// The reason the receiver gave (its `Bad` verdict) if one is waiting, otherwise `fallback`.
async fn peer_verdict<R: AsyncRead + Unpin>(rx: &mut R, fallback: FileError) -> FileError {
    match timeout(
        Duration::from_secs(2),
        read_frame(rx, Duration::from_secs(2)),
    )
    .await
    {
        Ok(Ok(body)) => match postcard::from_bytes::<FileDone>(&body) {
            Ok(FileDone::Bad { text }) => FileError::Peer(text),
            _ => fallback,
        },
        _ => fallback,
    }
}

fn short(text: &str) -> String {
    text.chars().take(200).collect()
}

/// Send one file. `source` must hold exactly `header.size` bytes; `progress` gets the running total.
pub async fn send_file<S, W, R>(
    header: &FileHeader,
    mut source: S,
    tx: &mut W,
    rx: &mut R,
    progress: &mut (dyn FnMut(u64) + Send),
) -> Result<(), FileError>
where
    S: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    write_frame(tx, header).await?;
    let reply: FileReply = postcard::from_bytes(&read_frame(rx, IDLE_TIMEOUT).await?)
        .map_err(|e| FileError::Protocol(e.to_string()))?;
    if let FileReply::Reject { code, text } = reply {
        return Err(FileError::Rejected { code, text });
    }
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; CHUNK];
    let mut sent = 0u64;
    while sent < header.size {
        let want = (header.size - sent).min(CHUNK as u64) as usize;
        let n = source.read(&mut buf[..want]).await.map_err(io_err)?;
        if n == 0 {
            return Err(FileError::TooShort);
        }
        hasher.update(&buf[..n]);
        match timeout(IDLE_TIMEOUT, tx.write_all(&buf[..n])).await {
            Err(_) => return Err(FileError::Timeout),
            // The receiver stopped reading: it usually said why just before it hung up.
            Ok(Err(e)) => return Err(peer_verdict(rx, io_err(e)).await),
            Ok(Ok(())) => {}
        }
        sent += n as u64;
        progress(sent);
    }
    let mut probe = [0u8; 1];
    if source.read(&mut probe).await.map_err(io_err)? != 0 {
        return Err(FileError::TooLong);
    }
    timeout(IDLE_TIMEOUT, async {
        tx.write_all(hasher.finalize().as_bytes()).await?;
        tx.flush().await?;
        tx.shutdown().await
    })
    .await
    .map_err(|_| FileError::Timeout)?
    .map_err(io_err)?;
    let done: FileDone = postcard::from_bytes(&read_frame(rx, DONE_TIMEOUT).await?)
        .map_err(|e| FileError::Protocol(e.to_string()))?;
    match done {
        FileDone::Ok => Ok(()),
        FileDone::Bad { text } => Err(FileError::Peer(text)),
    }
}

/// A file offered by the other device: its header has been read and checked, nothing else yet.
pub struct IncomingFile<R, W> {
    header: FileHeader,
    rx: R,
    tx: W,
}

impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> IncomingFile<R, W> {
    pub async fn open(mut rx: R, tx: W) -> Result<Self, FileError> {
        let header = parse_header(&read_frame(&mut rx, IDLE_TIMEOUT).await?)?;
        Ok(Self { header, rx, tx })
    }

    pub fn header(&self) -> &FileHeader {
        &self.header
    }

    /// Say no, with a reason the sender can show.
    pub async fn reject(mut self, code: RejectCode, text: &str) {
        let reply = FileReply::Reject {
            code,
            text: short(text),
        };
        let _ = write_frame(&mut self.tx, &reply).await;
        let _ = self.tx.shutdown().await;
    }

    async fn bad(&mut self, text: &str) {
        let _ = write_frame(&mut self.tx, &FileDone::Bad { text: short(text) }).await;
        let _ = self.tx.shutdown().await;
    }

    /// Say yes and take the bytes into `sink`. Returns only when exactly `size` bytes arrived and
    /// their hash matches; the caller then makes the file permanent and answers with `ok()` or `bad()`.
    pub async fn accept<K: AsyncWrite + Unpin>(
        mut self,
        sink: &mut K,
        progress: &mut (dyn FnMut(u64) + Send),
    ) -> Result<Received<W>, FileError> {
        write_frame(&mut self.tx, &FileReply::Accept).await?;
        let size = self.header.size;
        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; CHUNK];
        let mut got = 0u64;
        while got < size {
            let want = (size - got).min(CHUNK as u64) as usize;
            let n = match timeout(IDLE_TIMEOUT, self.rx.read(&mut buf[..want])).await {
                Err(_) => {
                    self.bad("the sender stopped sending").await;
                    return Err(FileError::Timeout);
                }
                Ok(Err(e)) => {
                    self.bad("the connection failed").await;
                    return Err(io_err(e));
                }
                Ok(Ok(n)) => n,
            };
            if n == 0 {
                self.bad("the file ended early").await;
                return Err(FileError::TooShort);
            }
            if let Err(e) = sink.write_all(&buf[..n]).await {
                self.bad("the file could not be saved").await;
                return Err(FileError::Io(e.to_string()));
            }
            hasher.update(&buf[..n]);
            got += n as u64;
            progress(got);
        }
        let mut trailer = [0u8; 32];
        match timeout(IDLE_TIMEOUT, self.rx.read_exact(&mut trailer)).await {
            Err(_) => {
                self.bad("the sender stopped sending").await;
                return Err(FileError::Timeout);
            }
            Ok(Err(e)) => {
                self.bad("the file ended early").await;
                return Err(io_err(e));
            }
            Ok(Ok(_)) => {}
        }
        if !bool::from(trailer.ct_eq(hasher.finalize().as_bytes())) {
            self.bad("the file arrived damaged").await;
            return Err(FileError::HashMismatch);
        }
        if let Err(e) = sink.flush().await {
            self.bad("the file could not be saved").await;
            return Err(FileError::Io(e.to_string()));
        }
        Ok(Received {
            bytes: got,
            tx: self.tx,
        })
    }
}

/// The bytes arrived intact; the verdict is still to be sent.
pub struct Received<W> {
    pub bytes: u64,
    tx: W,
}

impl<W: AsyncWrite + Unpin> Received<W> {
    /// The file is saved: tell the sender.
    pub async fn ok(mut self) {
        let _ = write_frame(&mut self.tx, &FileDone::Ok).await;
        let _ = self.tx.shutdown().await;
    }

    /// The file could not be made permanent after all: tell the sender why.
    pub async fn bad(mut self, text: &str) {
        let _ = write_frame(&mut self.tx, &FileDone::Bad { text: short(text) }).await;
        let _ = self.tx.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_stream_the_other_side_stopped_says_so_in_plain_words() {
        let e = super::io_err(std::io::Error::other("sending stopped by peer: error 0"));
        assert_eq!(e.to_string(), "the other device stopped the transfer");
        let e = super::io_err(std::io::Error::other("connection reset by peer"));
        assert_eq!(e.to_string(), "the other device stopped the transfer");
        let e = super::io_err(std::io::Error::other("disk on fire"));
        assert_eq!(e.to_string(), "disk on fire");
    }

    use super::*;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::{duplex, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

    fn pattern(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i.wrapping_mul(31) % 251) as u8).collect()
    }

    fn header(size: u64) -> FileHeader {
        FileHeader {
            version: 1,
            id: [7; 16],
            name: "report.pdf".into(),
            size,
        }
    }

    /// A writer that flips every bit of the byte at absolute stream position `at`.
    struct Flip<W> {
        inner: W,
        at: usize,
        seen: usize,
    }

    impl<W: AsyncWrite + Unpin> AsyncWrite for Flip<W> {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            let start = self.seen;
            let mut data = buf.to_vec();
            if self.at >= start && self.at < start + data.len() {
                data[self.at - start] ^= 0xFF;
            }
            match Pin::new(&mut self.inner).poll_write(cx, &data) {
                Poll::Ready(Ok(n)) => {
                    self.seen += n;
                    Poll::Ready(Ok(n))
                }
                other => other,
            }
        }
        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_flush(cx)
        }
        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }

    type Outcome = (Result<(), FileError>, Result<Vec<u8>, FileError>, u64, u64);

    /// A sender and a receiver wired together over two in-memory pipes. `flip_at` damages the data in
    /// transit; `reject` makes the receiver refuse; `source` is what the sender really has.
    async fn run(
        header: FileHeader,
        source: Vec<u8>,
        flip_at: Option<usize>,
        reject: Option<(RejectCode, &'static str)>,
    ) -> Outcome {
        let (s_to_r_w, s_to_r_r) = duplex(64 * 1024);
        let (r_to_s_w, r_to_s_r) = duplex(64 * 1024);
        let sender = tokio::spawn(async move {
            let mut tx = Flip {
                inner: s_to_r_w,
                at: flip_at.unwrap_or(usize::MAX),
                seen: 0,
            };
            let mut rx = r_to_s_r;
            let mut last = 0u64;
            let r = send_file(&header, &source[..], &mut tx, &mut rx, &mut |n| last = n).await;
            (r, last)
        });
        let receiver = tokio::spawn(async move {
            let incoming = IncomingFile::open(s_to_r_r, r_to_s_w).await?;
            if let Some((code, text)) = reject {
                incoming.reject(code, text).await;
                return Ok((Vec::new(), 0));
            }
            let mut sink = Vec::new();
            let mut last = 0u64;
            let received = incoming.accept(&mut sink, &mut |n| last = n).await?;
            received.ok().await;
            Ok::<_, FileError>((sink, last))
        });
        let (sent, sent_progress) = sender.await.unwrap();
        let got = receiver.await.unwrap();
        match got {
            Ok((sink, progress)) => (sent, Ok(sink), sent_progress, progress),
            Err(e) => (sent, Err(e), sent_progress, 0),
        }
    }

    #[tokio::test]
    async fn a_file_goes_through_with_its_hash_checked() {
        let data = pattern(700_000);
        let (sent, got, sent_progress, got_progress) =
            run(header(data.len() as u64), data.clone(), None, None).await;
        sent.unwrap();
        assert_eq!(got.unwrap(), data);
        assert_eq!((sent_progress, got_progress), (700_000, 700_000));
    }

    #[tokio::test]
    async fn zero_bytes_and_exactly_one_chunk_work() {
        for n in [0usize, 1, CHUNK - 1, CHUNK, CHUNK + 1] {
            let data = pattern(n);
            let (sent, got, _, _) = run(header(n as u64), data.clone(), None, None).await;
            sent.unwrap_or_else(|e| panic!("{n}: {e:?}"));
            assert_eq!(got.unwrap(), data, "{n}");
        }
    }

    #[tokio::test]
    async fn a_flipped_bit_is_caught_by_the_hash_and_the_sender_is_told() {
        let data = pattern(700_000);
        let (sent, got, _, _) = run(header(data.len() as u64), data, Some(1000), None).await;
        assert!(matches!(got, Err(FileError::HashMismatch)), "{got:?}");
        assert!(matches!(sent, Err(FileError::Peer(_))), "{sent:?}");
    }

    #[tokio::test]
    async fn data_shorter_than_the_header_says_is_an_error_not_a_hang() {
        let (sent, got, _, _) = run(header(1000), pattern(500), None, None).await;
        assert!(matches!(sent, Err(FileError::TooShort)), "{sent:?}");
        assert!(matches!(got, Err(FileError::TooShort)), "{got:?}");
    }

    #[tokio::test]
    async fn a_source_with_more_than_size_bytes_is_refused_and_nothing_extra_is_sent() {
        let (sent, got, _, _) = run(header(1000), pattern(1500), None, None).await;
        assert!(matches!(sent, Err(FileError::TooLong)), "{sent:?}");
        assert!(got.is_err(), "the receiver never got a valid trailer");
    }

    #[tokio::test]
    async fn a_reject_reaches_the_sender_with_its_code_and_text() {
        let (sent, _, _, _) = run(
            header(10),
            pattern(10),
            None,
            Some((RejectCode::NoSpace, "not enough free space")),
        )
        .await;
        match sent {
            Err(FileError::Rejected { code, text }) => {
                assert_eq!(code, RejectCode::NoSpace);
                assert_eq!(text, "not enough free space");
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_peer_times_out() {
        let (mut tx, _keep_open) = duplex(1024);
        let (_never_written, mut rx) = duplex(1024);
        let data = pattern(10);
        let r = send_file(&header(10), &data[..], &mut tx, &mut rx, &mut |_| {}).await;
        assert!(matches!(r, Err(FileError::Timeout)), "{r:?}");
    }

    #[tokio::test]
    async fn a_frame_over_the_cap_is_refused_before_it_is_read() {
        let (mut client, server) = duplex(1024);
        let (reply, _keep) = duplex(1024);
        client
            .write_all(&100_000_000u32.to_be_bytes())
            .await
            .unwrap();
        // The body never comes: the answer must not wait for 100 MB.
        let r = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            IncomingFile::open(server, reply),
        )
        .await
        .expect("refused at once");
        assert!(matches!(r, Err(FileError::Protocol(_))), "{:?}", r.err());
    }

    #[test]
    fn a_header_with_a_bad_version_or_name_is_refused() {
        let good = header(5);
        assert!(parse_header(&postcard::to_stdvec(&good).unwrap()).is_ok());
        let mut v2 = good.clone();
        v2.version = 2;
        let mut empty = good.clone();
        empty.name = String::new();
        let mut nul = good.clone();
        nul.name = "a\0b".into();
        let mut long = good.clone();
        long.name = "x".repeat(MAX_NAME_CHARS + 1);
        for bad in [v2, empty, nul, long] {
            assert!(
                parse_header(&postcard::to_stdvec(&bad).unwrap()).is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn random_bytes_never_panic_as_a_header() {
        use rand::{rngs::StdRng, Rng, RngCore, SeedableRng};
        let mut rng = StdRng::seed_from_u64(0xF11E);
        for _ in 0..20_000 {
            let mut buf = vec![0u8; rng.gen_range(0..300)];
            rng.fill_bytes(&mut buf);
            let _ = parse_header(&buf);
        }
    }

    #[tokio::test]
    async fn random_streams_never_panic_or_hang_the_receiver() {
        use rand::{rngs::StdRng, Rng, RngCore, SeedableRng};
        let mut rng = StdRng::seed_from_u64(0xBEEF);
        for _ in 0..300 {
            let mut buf = vec![0u8; rng.gen_range(0..400)];
            rng.fill_bytes(&mut buf);
            let (mut client, server) = duplex(4096);
            let (reply, mut seen) = duplex(4096);
            client.write_all(&buf).await.unwrap();
            drop(client);
            let r = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                IncomingFile::open(server, reply),
            )
            .await
            .expect("no hang");
            if let Ok(incoming) = r {
                let mut sink = Vec::new();
                let _ = incoming.accept(&mut sink, &mut |_| {}).await;
            }
            let mut sink = Vec::new();
            let _ = seen.read_to_end(&mut sink).await;
        }
    }

    /// A sink that fails after `limit` bytes (a full disk).
    struct FullAfter {
        limit: usize,
        written: usize,
    }

    impl AsyncWrite for FullAfter {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            if self.written + buf.len() > self.limit {
                return Poll::Ready(Err(std::io::Error::other("no space left on device")));
            }
            self.written += buf.len();
            Poll::Ready(Ok(buf.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn a_receiver_that_cannot_save_tells_the_sender_why_not_just_broken_pipe() {
        let data = pattern(2_000_000);
        let (s_to_r_w, s_to_r_r) = duplex(64 * 1024);
        let (r_to_s_w, r_to_s_r) = duplex(64 * 1024);
        let head = header(data.len() as u64);
        let sender = tokio::spawn(async move {
            let (mut tx, mut rx) = (s_to_r_w, r_to_s_r);
            send_file(&head, &data[..], &mut tx, &mut rx, &mut |_| {}).await
        });
        let receiver = tokio::spawn(async move {
            let incoming = IncomingFile::open(s_to_r_r, r_to_s_w).await?;
            let mut sink = FullAfter {
                limit: 300_000,
                written: 0,
            };
            incoming.accept(&mut sink, &mut |_| {}).await.map(|_| ())
        });
        let got = receiver.await.unwrap();
        assert!(matches!(got, Err(FileError::Io(_))), "{got:?}");
        match sender.await.unwrap() {
            Err(FileError::Peer(text)) => assert!(text.contains("could not be saved"), "{text}"),
            other => panic!("the sender should learn why: {other:?}"),
        }
    }

    // `AsyncRead` is only named so the test helpers above compile against the same traits.
    #[allow(dead_code)]
    fn _traits<R: AsyncRead>() {}
}
