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
/// Version 2: resume and folders. A device offers it first and falls back to version 1 with an older peer.
pub const FILE_ALPN_V2: &[u8] = b"klip/file/2";
/// A folder path holds at most this many components (each at most `MAX_NAME_CHARS` characters).
pub const MAX_FOLDER_DEPTH: usize = 16;
/// Largest header, reply or verdict frame.
pub const MAX_FILE_FRAME: usize = 2048;
/// Size of the buffers file bytes move through (memory per transfer is about this).
pub const CHUNK: usize = 256 * 1024;
/// No byte for this long ends the transfer.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// The receiver may need a while to flush and move a big file before it can say "done".
pub const DONE_TIMEOUT: Duration = Duration::from_secs(300);
pub const MAX_NAME_CHARS: usize = 255;
/// What a receiver says when the bytes arrived but the hash did not match. A sender that resumed treats exactly this
/// verdict as "the kept part was not the same file any more" and sends the file again from the start.
pub const DAMAGED: &str = "the file arrived damaged";
const VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileHeader {
    pub version: u8,
    pub id: [u8; 16],
    pub name: String,
    pub size: u64,
}

/// Where a file of a folder goes: the folder's name, a random id shared by every file of one send, and the
/// folders between the top one and the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderPlace {
    pub batch: [u8; 16],
    pub top: String,
    pub rel: Vec<String>,
}

/// The version 2 header: the version 1 fields, then what resume and folders need.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileHeaderV2 {
    pub version: u8,
    pub id: [u8; 16],
    pub name: String,
    pub size: u64,
    /// Chosen by the sender from the file's name, size, modification time and first bytes: the same file sent
    /// again gets the same key, so the receiver can find the partial it kept.
    pub resume_key: [u8; 32],
    pub folder: Option<FolderPlace>,
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
    Reject {
        code: RejectCode,
        text: String,
    },
    /// Version 2 only: "I already hold the first `offset` bytes, send the rest". Appended after the v1 variants.
    AcceptFrom {
        offset: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileDone {
    Ok,
    Bad { text: String },
}

#[derive(Debug)]
pub enum FileError {
    Io(String),
    /// This device could not write the bytes (full disk, permissions): not worth keeping a partial file for.
    Disk(String),
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
            Self::Io(e) | Self::Disk(e) => write!(f, "{e}"),
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

fn plain_part(text: &str) -> bool {
    !text.is_empty() && text.chars().count() <= MAX_NAME_CHARS && !text.contains('\0')
}

/// A version 2 header, checked like the version 1 one, and its folder place if it has one.
pub fn parse_header_v2(bytes: &[u8]) -> Result<FileHeaderV2, FileError> {
    let header: FileHeaderV2 =
        postcard::from_bytes(bytes).map_err(|e| FileError::Protocol(e.to_string()))?;
    if header.version != 2 {
        return Err(FileError::Protocol(
            "unsupported file transfer version".into(),
        ));
    }
    if !plain_part(&header.name) {
        return Err(FileError::Protocol("unusable file name".into()));
    }
    if let Some(place) = &header.folder {
        if !plain_part(&place.top)
            || place.rel.len() > MAX_FOLDER_DEPTH
            || !place.rel.iter().all(|c| plain_part(c))
        {
            return Err(FileError::Protocol("unusable folder".into()));
        }
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

/// A transfer that moves fewer than this many bytes in a whole window is dropped: one byte every 29 seconds
/// would otherwise hold a receive slot (and a file handle) for as long as a paired device likes.
const PACE_WINDOW: Duration = Duration::from_secs(60);
const PACE_MIN_BYTES: u64 = 1024;

pub(crate) struct Pace {
    start: tokio::time::Instant,
    moved: u64,
}

impl Pace {
    pub(crate) fn new(now: tokio::time::Instant) -> Self {
        Self {
            start: now,
            moved: 0,
        }
    }

    /// `n` more bytes moved at `now`. `false`: the window ended and too little moved in it.
    pub(crate) fn tick(&mut self, now: tokio::time::Instant, n: u64) -> bool {
        self.moved += n;
        if now.duration_since(self.start) >= PACE_WINDOW {
            if self.moved < PACE_MIN_BYTES {
                return false;
            }
            self.start = now;
            self.moved = 0;
        }
        true
    }
}

fn short(text: &str) -> String {
    text.chars().take(200).collect()
}

/// Send one file. `source` must hold exactly `header.size` bytes; `progress` gets the running total.
pub async fn send_file<S, W, R>(
    header: &FileHeader,
    source: S,
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
    match read_reply(rx).await? {
        FileReply::Accept => send_body(header.size, 0, source, tx, rx, progress).await,
        FileReply::Reject { code, text } => Err(FileError::Rejected { code, text }),
        FileReply::AcceptFrom { .. } => Err(FileError::Protocol("unexpected resume offer".into())),
    }
}

/// Version 2: like `send_file`, but the receiver may already hold the first bytes. Those are read from `source`
/// and hashed here (not sent), so the trailer still covers the whole file.
pub async fn send_file_v2<S, W, R>(
    header: &FileHeaderV2,
    source: S,
    tx: &mut W,
    rx: &mut R,
    progress: &mut (dyn FnMut(u64) + Send),
) -> Result<(), FileError>
where
    S: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    send_file_v2_offset(header, source, tx, rx, progress, &mut 0).await
}

/// `send_file_v2`, and `resumed_from` is set to the offset the receiver asked to continue from (0 for a full send).
pub async fn send_file_v2_offset<S, W, R>(
    header: &FileHeaderV2,
    source: S,
    tx: &mut W,
    rx: &mut R,
    progress: &mut (dyn FnMut(u64) + Send),
    resumed_from: &mut u64,
) -> Result<(), FileError>
where
    S: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    write_frame(tx, header).await?;
    match read_reply(rx).await? {
        FileReply::Accept => send_body(header.size, 0, source, tx, rx, progress).await,
        FileReply::AcceptFrom { offset } if offset <= header.size => {
            *resumed_from = offset;
            send_body(header.size, offset, source, tx, rx, progress).await
        }
        FileReply::AcceptFrom { .. } => {
            Err(FileError::Protocol("resume offset past the end".into()))
        }
        FileReply::Reject { code, text } => Err(FileError::Rejected { code, text }),
    }
}

async fn read_reply<R: AsyncRead + Unpin>(rx: &mut R) -> Result<FileReply, FileError> {
    postcard::from_bytes(&read_frame(rx, IDLE_TIMEOUT).await?)
        .map_err(|e| FileError::Protocol(e.to_string()))
}

/// The bytes after the reply: skip (and hash) the first `offset`, send the rest, the trailer, read the verdict.
async fn send_body<S, W, R>(
    size: u64,
    offset: u64,
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
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; CHUNK];
    let mut skipped = 0u64;
    while skipped < offset {
        let want = (offset - skipped).min(CHUNK as u64) as usize;
        let n = source.read(&mut buf[..want]).await.map_err(io_err)?;
        if n == 0 {
            return Err(FileError::TooShort);
        }
        hasher.update(&buf[..n]);
        skipped += n as u64;
    }
    if offset > 0 {
        progress(offset);
    }
    let mut sent = offset;
    let mut pace = Pace::new(tokio::time::Instant::now());
    while sent < size {
        let want = (size - sent).min(CHUNK as u64) as usize;
        let n = source.read(&mut buf[..want]).await.map_err(io_err)?;
        if n == 0 {
            return Err(FileError::TooShort);
        }
        hasher.update(&buf[..n]);
        // The receiver may still be hashing the part it kept: the first chunk after a resume waits longer.
        let wait = if offset > 0 && sent == offset {
            DONE_TIMEOUT
        } else {
            IDLE_TIMEOUT
        };
        match timeout(wait, tx.write_all(&buf[..n])).await {
            Err(_) => return Err(FileError::Timeout),
            // The receiver stopped reading: it usually said why just before it hung up.
            Ok(Err(e)) => return Err(peer_verdict(rx, io_err(e)).await),
            Ok(Ok(())) => {}
        }
        sent += n as u64;
        progress(sent);
        if !pace.tick(tokio::time::Instant::now(), n as u64) {
            return Err(FileError::Timeout);
        }
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
    /// The full version 2 header when this is a version 2 transfer.
    v2: Option<FileHeaderV2>,
    rx: R,
    tx: W,
}

impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> IncomingFile<R, W> {
    pub async fn open(mut rx: R, tx: W) -> Result<Self, FileError> {
        let header = parse_header(&read_frame(&mut rx, IDLE_TIMEOUT).await?)?;
        Ok(Self {
            header,
            v2: None,
            rx,
            tx,
        })
    }

    /// The first frame of a version 2 connection. `header()` has the common fields; `v2()` the rest.
    pub async fn open_v2(mut rx: R, tx: W) -> Result<Self, FileError> {
        let v2 = parse_header_v2(&read_frame(&mut rx, IDLE_TIMEOUT).await?)?;
        let header = FileHeader {
            version: v2.version,
            id: v2.id,
            name: v2.name.clone(),
            size: v2.size,
        };
        Ok(Self {
            header,
            v2: Some(v2),
            rx,
            tx,
        })
    }

    pub fn v2(&self) -> Option<&FileHeaderV2> {
        self.v2.as_ref()
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
        self,
        sink: &mut K,
        progress: &mut (dyn FnMut(u64) + Send),
    ) -> Result<Received<W>, FileError> {
        self.take(
            FileReply::Accept,
            0,
            std::future::ready(Ok(blake3::Hasher::new())),
            sink,
            progress,
        )
        .await
    }

    /// Version 2: say yes, but the first `offset` bytes are already here. `prefix` is a hasher the caller fed
    /// with exactly those bytes; `sink` gets only the rest. The trailer is checked against the whole file.
    pub async fn accept_from<K, P>(
        mut self,
        offset: u64,
        prefix: P,
        sink: &mut K,
        progress: &mut (dyn FnMut(u64) + Send),
    ) -> Result<Received<W>, FileError>
    where
        K: AsyncWrite + Unpin,
        P: std::future::Future<Output = std::io::Result<blake3::Hasher>>,
    {
        if offset > self.header.size {
            self.bad("the offset is past the end of the file").await;
            return Err(FileError::Protocol("resume offset past the end".into()));
        }
        let reply = if offset == 0 {
            FileReply::Accept
        } else {
            FileReply::AcceptFrom { offset }
        };
        self.take(reply, offset, prefix, sink, progress).await
    }

    async fn take<K, P>(
        mut self,
        reply: FileReply,
        offset: u64,
        prefix: P,
        sink: &mut K,
        progress: &mut (dyn FnMut(u64) + Send),
    ) -> Result<Received<W>, FileError>
    where
        K: AsyncWrite + Unpin,
        P: std::future::Future<Output = std::io::Result<blake3::Hasher>>,
    {
        write_frame(&mut self.tx, &reply).await?;
        // The reply is out: hashing what is already here happens now, while the sender hashes its side too.
        let mut hasher = match prefix.await {
            Ok(h) => h,
            Err(e) => {
                self.bad("the kept part could not be read").await;
                return Err(FileError::Disk(e.to_string()));
            }
        };
        let size = self.header.size;
        let mut buf = vec![0u8; CHUNK];
        let mut got = offset;
        if offset > 0 {
            progress(offset);
        }
        let mut pace = Pace::new(tokio::time::Instant::now());
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
                return Err(FileError::Disk(e.to_string()));
            }
            hasher.update(&buf[..n]);
            got += n as u64;
            progress(got);
            if !pace.tick(tokio::time::Instant::now(), n as u64) {
                self.bad("the transfer was too slow").await;
                return Err(FileError::Timeout);
            }
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
            self.bad(DAMAGED).await;
            return Err(FileError::HashMismatch);
        }
        if let Err(e) = sink.flush().await {
            self.bad("the file could not be saved").await;
            return Err(FileError::Disk(e.to_string()));
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
        assert!(
            matches!(&sent, Err(FileError::Peer(t)) if t == DAMAGED),
            "{sent:?}"
        );
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

    fn header_v2(size: u64, folder: Option<FolderPlace>) -> FileHeaderV2 {
        FileHeaderV2 {
            version: 2,
            id: [7; 16],
            name: "report.pdf".into(),
            size,
            resume_key: [9; 32],
            folder,
        }
    }

    #[test]
    fn a_v2_header_round_trips_and_the_reply_has_the_new_variant_last() {
        let place = FolderPlace {
            batch: [5; 16],
            top: "Trip".into(),
            rel: vec!["Day 1".into(), "raw".into()],
        };
        let h = header_v2(10, Some(place));
        assert_eq!(
            parse_header_v2(&postcard::to_stdvec(&h).unwrap()).unwrap(),
            h
        );
        // v1 replies keep their bytes; AcceptFrom is appended after them.
        assert_eq!(postcard::to_stdvec(&FileReply::Accept).unwrap(), [0]);
        assert_eq!(
            postcard::to_stdvec(&FileReply::Reject {
                code: RejectCode::Disabled,
                text: String::new()
            })
            .unwrap(),
            [1, 0, 0]
        );
        assert_eq!(
            postcard::to_stdvec(&FileReply::AcceptFrom { offset: 5 }).unwrap(),
            [2, 5]
        );
    }

    #[test]
    fn v2_headers_with_bad_folder_places_are_refused() {
        let ok = |p: FolderPlace| {
            parse_header_v2(&postcard::to_stdvec(&header_v2(1, Some(p))).unwrap()).is_ok()
        };
        let base = || FolderPlace {
            batch: [1; 16],
            top: "Top".into(),
            rel: vec!["a".into()],
        };
        assert!(ok(base()));
        assert!(!ok(FolderPlace {
            top: String::new(),
            ..base()
        }));
        assert!(!ok(FolderPlace {
            top: "a\0b".into(),
            ..base()
        }));
        assert!(!ok(FolderPlace {
            top: "x".repeat(256),
            ..base()
        }));
        assert!(!ok(FolderPlace {
            rel: vec![String::new()],
            ..base()
        }));
        assert!(!ok(FolderPlace {
            rel: vec!["a\0".into()],
            ..base()
        }));
        assert!(!ok(FolderPlace {
            rel: vec!["x".repeat(256)],
            ..base()
        }));
        assert!(ok(FolderPlace {
            rel: vec!["d".into(); 16],
            ..base()
        }));
        assert!(!ok(FolderPlace {
            rel: vec!["d".into(); 17],
            ..base()
        }));
        let mut v1 = header_v2(1, None);
        v1.version = 1;
        assert!(
            parse_header_v2(&postcard::to_stdvec(&v1).unwrap()).is_err(),
            "a v2 parser wants version 2"
        );
    }

    /// Run a v2 transfer where the receiver already holds `have` bytes whose hash it computed itself.
    async fn resumed(
        data: &[u8],
        have: usize,
        prefix_is_really: &[u8],
    ) -> (Result<(), FileError>, Result<Vec<u8>, FileError>, Vec<u64>) {
        let (s_w, r_r) = duplex(64 * 1024);
        let (r_w, s_r) = duplex(64 * 1024);
        let h = header_v2(data.len() as u64, None);
        let (mut tx, mut rx) = (s_w, s_r);
        let src = data.to_vec();
        let sender = tokio::spawn(async move {
            let mut seen = Vec::new();
            let r = send_file_v2(&h, &src[..], &mut tx, &mut rx, &mut |n| seen.push(n)).await;
            (r, seen)
        });
        let mut prefix = blake3::Hasher::new();
        prefix.update(&prefix_is_really[..have]);
        let incoming = IncomingFile::open_v2(r_r, r_w).await.unwrap();
        let mut sink = Vec::new();
        let got = incoming
            .accept_from(
                have as u64,
                std::future::ready(Ok(prefix)),
                &mut sink,
                &mut |_| {},
            )
            .await;
        let got = match got {
            Ok(received) => {
                received.ok().await;
                Ok(sink)
            }
            Err(e) => Err(e),
        };
        let (sent, seen) = sender.await.unwrap();
        (sent, got, seen)
    }

    #[tokio::test]
    async fn a_resumed_transfer_sends_only_the_missing_bytes_and_checks_the_whole_hash() {
        let data = pattern(600_000);
        let (sent, got, seen) = resumed(&data, 250_000, &data).await;
        sent.unwrap();
        assert_eq!(got.unwrap(), data[250_000..]);
        assert_eq!(
            seen.first(),
            Some(&250_000),
            "progress starts at the offset"
        );
        assert_eq!(seen.last(), Some(&600_000));
    }

    #[tokio::test]
    async fn a_wrong_prefix_ends_in_a_hash_failure_on_both_sides() {
        let data = pattern(600_000);
        let mut other = data.clone();
        other[100] ^= 0xFF;
        let (sent, got, _) = resumed(&data, 250_000, &other).await;
        assert!(matches!(got, Err(FileError::HashMismatch)), "{got:?}");
        assert!(matches!(sent, Err(FileError::Peer(_))), "{sent:?}");
    }

    #[tokio::test]
    async fn offset_zero_and_offset_equal_to_size_both_work() {
        let data = pattern(70_000);
        let (sent, got, _) = resumed(&data, 0, &data).await;
        sent.unwrap();
        assert_eq!(got.unwrap(), data);
        let (sent, got, _) = resumed(&data, 70_000, &data).await;
        sent.unwrap();
        assert!(got.unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_offset_past_the_end_is_a_protocol_error_for_the_sender() {
        let (mut client_tx, mut srv_rx) = duplex(64 * 1024);
        let (mut srv_tx, mut client_rx) = duplex(64 * 1024);
        let data = pattern(100);
        let h = header_v2(100, None);
        let sender = tokio::spawn(async move {
            send_file_v2(&h, &data[..], &mut client_tx, &mut client_rx, &mut |_| {}).await
        });
        let _ = read_frame(&mut srv_rx, Duration::from_secs(5))
            .await
            .unwrap();
        write_frame(&mut srv_tx, &FileReply::AcceptFrom { offset: 101 })
            .await
            .unwrap();
        assert!(matches!(sender.await.unwrap(), Err(FileError::Protocol(_))));
    }

    #[tokio::test]
    async fn a_v1_sender_refuses_a_resume_offer_it_never_asked_for() {
        let (mut client_tx, mut srv_rx) = duplex(64 * 1024);
        let (mut srv_tx, mut client_rx) = duplex(64 * 1024);
        let data = pattern(100);
        let sender = tokio::spawn(async move {
            send_file(
                &header(100),
                &data[..],
                &mut client_tx,
                &mut client_rx,
                &mut |_| {},
            )
            .await
        });
        let _ = read_frame(&mut srv_rx, Duration::from_secs(5))
            .await
            .unwrap();
        write_frame(&mut srv_tx, &FileReply::AcceptFrom { offset: 10 })
            .await
            .unwrap();
        assert!(matches!(sender.await.unwrap(), Err(FileError::Protocol(_))));
    }

    #[test]
    fn pace_lets_a_slow_but_moving_transfer_live_and_drops_a_crawling_one() {
        let t0 = tokio::time::Instant::now();
        let s = Duration::from_secs;
        let mut slow = Pace::new(t0);
        assert!(slow.tick(t0 + s(10), 500));
        assert!(
            slow.tick(t0 + s(61), 600),
            "1100 bytes in the first window is enough"
        );
        assert!(slow.tick(t0 + s(121), 2000), "and the window starts over");
        let mut crawl = Pace::new(t0);
        assert!(crawl.tick(t0 + s(29), 1));
        assert!(crawl.tick(t0 + s(58), 1));
        assert!(!crawl.tick(t0 + s(87), 1), "3 bytes in 87 seconds");
    }

    #[tokio::test(start_paused = true)]
    async fn a_sender_that_sends_one_byte_every_29_seconds_is_dropped() {
        let (mut client, server) = duplex(64 * 1024);
        let (reply_tx, mut reply_rx) = duplex(64 * 1024);
        let writer = tokio::spawn(async move {
            write_frame(&mut client, &header(1_000_000)).await.unwrap();
            let mut drip = 0u32;
            loop {
                tokio::time::sleep(Duration::from_secs(29)).await;
                if client.write_all(&[1u8]).await.is_err() {
                    return drip;
                }
                drip += 1;
            }
        });
        let incoming = IncomingFile::open(server, reply_tx).await.unwrap();
        let mut sink = tokio::io::sink();
        let started = tokio::time::Instant::now();
        let r = incoming.accept(&mut sink, &mut |_| {}).await;
        assert!(matches!(r, Err(FileError::Timeout)), "{:?}", r.err());
        assert!(
            started.elapsed() < Duration::from_secs(120),
            "dropped within two windows, not after the whole file"
        );
        let _ = read_frame(&mut reply_rx, Duration::from_secs(1)).await; // Accept
        writer.abort();
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
        assert!(matches!(got, Err(FileError::Disk(_))), "{got:?}");
        match sender.await.unwrap() {
            Err(FileError::Peer(text)) => assert!(text.contains("could not be saved"), "{text}"),
            other => panic!("the sender should learn why: {other:?}"),
        }
    }

    // `AsyncRead` is only named so the test helpers above compile against the same traits.
    #[allow(dead_code)]
    fn _traits<R: AsyncRead>() {}
}
