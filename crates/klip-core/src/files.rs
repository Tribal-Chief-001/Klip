//! Receiving and sending files, the parts that touch the disk: safe names, unique paths that never
//! replace anything, free space, and partial files that become permanent only when complete.
use klip_net::file::{send_file, FileError, FileHeader, IncomingFile, RejectCode};
use klip_proto::ipc::{TransferDirection, TransferInfo, TransferState};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::watch;

const MAX_NAME_BYTES: usize = 200;
/// Names Windows treats as devices; harmless elsewhere, so they are avoided on every OS (a file made
/// on Linux must still be usable when it is copied to a Windows machine).
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

fn is_format_char(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}' | '\u{2028}' | '\u{2029}' | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
    )
}

/// A received name is data, not a path: the last component only, nothing a shell, a file system or a
/// reader's eyes could misread (control and direction-changing characters), no leading dots (no hidden
/// files), no trailing dots or spaces, no Windows device names, at most 200 bytes, never empty.
pub fn sanitize_file_name(raw: &str) -> String {
    let last = raw
        .rsplit(['/', '\\'])
        .find(|part| !part.trim().is_empty())
        .unwrap_or("");
    let cleaned: String = last
        .chars()
        .filter(|c| !c.is_control() && !is_format_char(*c))
        .map(|c| if ":*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    let trimmed = cleaned
        .trim()
        .trim_start_matches('.')
        .trim_end_matches(['.', ' ']);
    let stem = trimmed.split('.').next().unwrap_or("");
    let mut name = if RESERVED
        .iter()
        .any(|r| r.eq_ignore_ascii_case(stem.trim_end()))
    {
        format!("_{trimmed}")
    } else {
        trimmed.to_string()
    };
    if name.len() > MAX_NAME_BYTES {
        let (stem, ext) = split_ext(&name);
        let ext = ext.to_string();
        let budget = MAX_NAME_BYTES.saturating_sub(ext.len());
        let mut end = 0;
        for (i, c) in stem.char_indices() {
            if i + c.len_utf8() > budget {
                break;
            }
            end = i + c.len_utf8();
        }
        name = format!("{}{ext}", stem[..end].trim_end_matches(['.', ' ']));
    }
    if name.is_empty() {
        "file".to_string()
    } else {
        name
    }
}

/// `("report", ".pdf")`; a leading dot or a very long "extension" is part of the name.
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 && name.len() - i <= 17 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Owner only, and never an execute bit: a received file is data, not a program.
        options.mode(0o600);
    }
    options
}

/// Create a new file called `name` in `dir`, or `name (2).ext`, `name (3).ext` ... when it exists.
/// `create_new` reserves the name atomically, so nothing is ever replaced, not even by a race.
pub fn unique_path(dir: &Path, name: &str) -> io::Result<(File, PathBuf)> {
    let (stem, ext) = split_ext(name);
    for n in 1..=10_000u32 {
        let candidate = if n == 1 {
            name.to_string()
        } else {
            format!("{stem} ({n}){ext}")
        };
        let path = dir.join(&candidate);
        match private_options().open(&path) {
            Ok(file) => return Ok((file, path)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "too many files with that name",
    ))
}

/// Bytes available to this user on the volume that holds `dir` (or the nearest folder that exists).
pub fn free_space(dir: &Path) -> io::Result<u64> {
    let mut probe = dir;
    while !probe.exists() {
        probe = probe
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such folder"))?;
    }
    platform_free_space(probe)
}

#[cfg(unix)]
#[allow(clippy::unnecessary_cast)] // the field types differ between platforms (u32 on some)
fn platform_free_space(path: &Path) -> io::Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "bad path"))?;
    // SAFETY: statvfs fills the zeroed struct we give it for a NUL-terminated path.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
}

#[cfg(windows)]
fn platform_free_space(path: &Path) -> io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available = 0u64;
    // SAFETY: `wide` is NUL-terminated and outlives the call; the out pointers are valid.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(available)
}

/// `<Downloads>/Klip`, or `~/Klip`, or a folder under the temp dir when there is no home.
pub fn default_receive_dir() -> PathBuf {
    directories::UserDirs::new()
        .map(|d| {
            d.download_dir()
                .map(|p| p.join("Klip"))
                .unwrap_or_else(|| d.home_dir().join("Klip"))
        })
        .unwrap_or_else(|| std::env::temp_dir().join("Klip"))
}

/// The receive folder: it must be a real folder (made if missing), never a symlink that could lead
/// somewhere the person did not choose.
fn ensure_dir(dir: &Path) -> io::Result<()> {
    match fs::symlink_metadata(dir) {
        Ok(m) if m.file_type().is_symlink() => Err(io::Error::other(
            "the receive folder is a symlink; choose a real folder",
        )),
        Ok(m) if m.is_dir() => Ok(()),
        Ok(_) => Err(io::Error::other("the receive folder is not a folder")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => fs::create_dir_all(dir),
        Err(e) => Err(e),
    }
}

fn partial_name(id: &[u8; 16]) -> String {
    let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
    format!(".klip-{hex}.part")
}

/// A file being received. It is a hidden `.klip-<id>.part` in the receive folder until the whole file
/// has arrived and been checked; only `commit` gives it its real name, and it never replaces a file.
pub struct PartialFile {
    file: tokio::fs::File,
    path: PathBuf,
    dir: PathBuf,
}

impl PartialFile {
    pub fn create(dir: &Path, id: [u8; 16]) -> io::Result<Self> {
        ensure_dir(dir)?;
        let path = dir.join(partial_name(&id));
        let file = private_options().open(&path)?;
        Ok(Self {
            file: tokio::fs::File::from_std(file),
            path,
            dir: dir.to_path_buf(),
        })
    }

    pub fn file(&mut self) -> &mut tokio::fs::File {
        &mut self.file
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Flush it to disk and give it its real (sanitised, unique) name. Returns where it is now.
    pub async fn commit(self, wanted_name: &str) -> io::Result<PathBuf> {
        let Self { file, path, dir } = self;
        let made_permanent = async {
            file.sync_all().await?;
            drop(file);
            let (placeholder, target) = unique_path(&dir, &sanitize_file_name(wanted_name))?;
            drop(placeholder);
            // The placeholder is ours (made by create_new), so renaming over it replaces nothing of the person's.
            match fs::rename(&path, &target) {
                Ok(()) => Ok(target),
                Err(e) => {
                    let _ = fs::remove_file(&target);
                    Err(e)
                }
            }
        }
        .await;
        if made_permanent.is_err() {
            let _ = fs::remove_file(&path);
        }
        made_permanent
    }

    /// Throw the partial file away.
    pub async fn discard(self) {
        let Self { file, path, .. } = self;
        drop(file);
        let _ = fs::remove_file(path);
    }
}

/// Remove `.klip-<32 hex>.part` files older than `older_than` (a transfer that died with the daemon).
/// Only regular files with exactly that name are touched.
pub fn sweep_partials(dir: &Path, older_than: Duration) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let hex = name
            .strip_prefix(".klip-")
            .and_then(|rest| rest.strip_suffix(".part"));
        let looks_ours =
            hex.is_some_and(|h| h.len() == 32 && h.bytes().all(|b| b.is_ascii_hexdigit()));
        let is_file = entry.file_type().is_ok_and(|t| t.is_file());
        if !looks_ours || !is_file {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age > older_than);
        if old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// What this device accepts. Built from the config; Android uses a space factor of 2 because the
/// staged file is copied into shared storage afterwards.
#[derive(Debug, Clone)]
pub struct FilePolicy {
    pub accept: bool,
    pub dir: PathBuf,
    pub max_bytes: u64,
    /// Free space that must be left after the file (a disk is never filled to the brim).
    pub reserve_bytes: u64,
    /// Free space needed per byte of the file (2 on Android, 1 elsewhere).
    pub space_factor: u64,
    pub max_incoming: usize,
    pub max_incoming_per_peer: usize,
}

impl FilePolicy {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            accept: true,
            dir,
            max_bytes: 4 * 1024 * 1024 * 1024,
            reserve_bytes: 512 * 1024 * 1024,
            space_factor: 1,
            max_incoming: 3,
            max_incoming_per_peer: 2,
        }
    }
}

const KEEP_FINISHED: usize = 50;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Resolves when the transfer it belongs to is cancelled.
#[derive(Clone)]
pub struct CancelToken(watch::Receiver<bool>);

impl CancelToken {
    pub fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }

    pub async fn cancelled(&mut self) {
        if self.0.wait_for(|cancelled| *cancelled).await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

struct Entry {
    info: TransferInfo,
    cancel: watch::Sender<bool>,
}

/// Every transfer that is running, and the last 50 that finished (memory only: names are private).
#[derive(Default)]
pub struct TransferRegistry {
    entries: Mutex<Vec<Entry>>,
}

fn is_running(state: TransferState) -> bool {
    matches!(state, TransferState::Waiting | TransferState::Active)
}

impl TransferRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn begin(
        &self,
        peer: &str,
        direction: TransferDirection,
        id: [u8; 16],
        name: &str,
        size: u64,
        state: TransferState,
    ) -> CancelToken {
        let (cancel, token) = watch::channel(false);
        let mut entries = self.entries.lock().unwrap();
        entries.push(Entry {
            info: TransferInfo {
                id,
                peer: peer.to_string(),
                direction,
                name: name.to_string(),
                size,
                done: 0,
                state,
                error: None,
                saved_to: None,
                started_ms: now_ms(),
                finished_ms: if is_running(state) {
                    None
                } else {
                    Some(now_ms())
                },
            },
            cancel,
        });
        Self::trim(&mut entries);
        CancelToken(token)
    }

    fn with(&self, id: [u8; 16], change: impl FnOnce(&mut Entry)) {
        let mut entries = self.entries.lock().unwrap();
        if let Some(entry) = entries.iter_mut().rev().find(|e| e.info.id == id) {
            change(entry);
        }
        Self::trim(&mut entries);
    }

    pub fn set_active(&self, id: [u8; 16]) {
        self.with(id, |e| e.info.state = TransferState::Active);
    }

    pub fn progress(&self, id: [u8; 16], done: u64) {
        self.with(id, |e| e.info.done = done);
    }

    pub fn finish(
        &self,
        id: [u8; 16],
        state: TransferState,
        error: Option<String>,
        saved_to: Option<String>,
    ) {
        self.with(id, |e| {
            e.info.state = state;
            e.info.error = error;
            e.info.saved_to = saved_to;
            e.info.finished_ms = Some(now_ms());
        });
    }

    pub fn list(&self) -> Vec<TransferInfo> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.info.clone())
            .collect()
    }

    /// Ask a running transfer to stop. `false` when there is no running transfer with that id.
    pub fn cancel(&self, id: [u8; 16]) -> bool {
        let entries = self.entries.lock().unwrap();
        match entries
            .iter()
            .rev()
            .find(|e| e.info.id == id && is_running(e.info.state))
        {
            Some(entry) => {
                entry.cancel.send_replace(true);
                true
            }
            None => false,
        }
    }

    /// Stop every running transfer with a device (it was unpaired).
    pub fn cancel_peer(&self, peer: &str) -> usize {
        let entries = self.entries.lock().unwrap();
        let mut stopped = 0;
        for entry in entries
            .iter()
            .filter(|e| e.info.peer == peer && is_running(e.info.state))
        {
            entry.cancel.send_replace(true);
            stopped += 1;
        }
        stopped
    }

    /// Running incoming transfers, from one device or from all.
    pub fn active_incoming(&self, peer: Option<&str>) -> usize {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .filter(|e| {
                e.info.direction == TransferDirection::Receiving
                    && is_running(e.info.state)
                    && peer.is_none_or(|p| e.info.peer == p)
            })
            .count()
    }

    fn trim(entries: &mut Vec<Entry>) {
        let finished = entries.iter().filter(|e| !is_running(e.info.state)).count();
        let mut drop_first = finished.saturating_sub(KEEP_FINISHED);
        entries.retain(|e| {
            if drop_first > 0 && !is_running(e.info.state) {
                drop_first -= 1;
                false
            } else {
                true
            }
        });
    }
}

fn human(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= 1024.0 * MIB {
        format!("{:.1} GiB", b / (1024.0 * MIB))
    } else {
        format!("{:.1} MiB", b / MIB)
    }
}

/// Why a file is not accepted, if it is not: checked before a single byte is read.
fn refusal(
    header: &FileHeader,
    peer: &str,
    policy: &FilePolicy,
    reg: &TransferRegistry,
) -> Option<(RejectCode, String)> {
    if !policy.accept {
        return Some((
            RejectCode::Disabled,
            "this device is not accepting files".into(),
        ));
    }
    if header.size > policy.max_bytes {
        return Some((
            RejectCode::TooLarge,
            format!(
                "the file is {} but this device accepts at most {}",
                human(header.size),
                human(policy.max_bytes)
            ),
        ));
    }
    if reg.active_incoming(None) >= policy.max_incoming
        || reg.active_incoming(Some(peer)) >= policy.max_incoming_per_peer
    {
        return Some((
            RejectCode::Busy,
            "this device is already receiving other files; try again in a moment".into(),
        ));
    }
    match free_space(&policy.dir) {
        Ok(free) => {
            let needed = header
                .size
                .saturating_mul(policy.space_factor)
                .saturating_add(policy.reserve_bytes);
            (free < needed).then(|| {
                (
                    RejectCode::NoSpace,
                    "this device does not have enough free space for the file".to_string(),
                )
            })
        }
        Err(e) => Some((
            RejectCode::Other,
            format!("cannot check the free space: {e}"),
        )),
    }
}

/// Take one incoming file: check the policy, stream it into a partial file, verify it, give it its
/// name, and only then tell the sender "done". Whatever goes wrong, nothing is left on disk.
pub async fn receive_incoming<R, W>(
    incoming: IncomingFile<R, W>,
    peer: &str,
    policy: &FilePolicy,
    reg: &TransferRegistry,
) -> Result<PathBuf, FileError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let header = incoming.header().clone();
    let id = header.id;
    let name = sanitize_file_name(&header.name);
    let refuse = |code: RejectCode, text: String| {
        reg.begin(
            peer,
            TransferDirection::Receiving,
            id,
            &name,
            header.size,
            TransferState::Failed,
        );
        reg.finish(id, TransferState::Failed, Some(text.clone()), None);
        (code, text)
    };
    if let Some((code, text)) = refusal(&header, peer, policy, reg) {
        let (code, text) = refuse(code, text);
        incoming.reject(code, &text).await;
        return Err(FileError::Rejected { code, text });
    }
    let mut part = match PartialFile::create(&policy.dir, id) {
        Ok(part) => part,
        Err(e) => {
            let code = if e.kind() == io::ErrorKind::AlreadyExists {
                RejectCode::Busy
            } else {
                RejectCode::Other
            };
            let (code, text) = refuse(code, format!("this device cannot save the file: {e}"));
            incoming.reject(code, &text).await;
            return Err(FileError::Rejected { code, text });
        }
    };
    let mut cancel = reg.begin(
        peer,
        TransferDirection::Receiving,
        id,
        &name,
        header.size,
        TransferState::Active,
    );
    let mut progress = |n| reg.progress(id, n);
    let outcome = tokio::select! {
        r = incoming.accept(part.file(), &mut progress) => r,
        _ = cancel.cancelled() => Err(FileError::Cancelled),
    };
    match outcome {
        Ok(received) => match part.commit(&name).await {
            Ok(path) => {
                received.ok().await;
                reg.finish(
                    id,
                    TransferState::Done,
                    None,
                    Some(path.display().to_string()),
                );
                Ok(path)
            }
            Err(e) => {
                received.bad("the file could not be saved").await;
                reg.finish(
                    id,
                    TransferState::Failed,
                    Some(format!("could not save the file: {e}")),
                    None,
                );
                Err(FileError::Io(e.to_string()))
            }
        },
        Err(e) => {
            part.discard().await;
            let state = if matches!(e, FileError::Cancelled) {
                TransferState::Cancelled
            } else {
                TransferState::Failed
            };
            reg.finish(id, state, Some(e.to_string()), None);
            Err(e)
        }
    }
}

/// A file opened and measured for sending; it must be a regular file.
pub struct PreparedFile {
    file: tokio::fs::File,
    pub name: String,
    pub size: u64,
}

pub async fn prepare_send(path: &Path) -> io::Result<PreparedFile> {
    let file = tokio::fs::File::open(path).await?;
    let meta = file.metadata().await?;
    if !meta.is_file() {
        return Err(io::Error::other("only regular files can be sent"));
    }
    let raw = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(PreparedFile {
        file,
        name: sanitize_file_name(&raw),
        size: meta.len(),
    })
}

/// Send a prepared file over streams that are already open. A file that changed size since it was
/// measured fails the transfer instead of sending something the receiver would reject anyway.
pub async fn send_prepared<R, W>(
    prepared: PreparedFile,
    id: [u8; 16],
    tx: &mut W,
    rx: &mut R,
    mut cancel: CancelToken,
    reg: &TransferRegistry,
) -> Result<(), FileError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    reg.set_active(id);
    let header = FileHeader {
        version: 1,
        id,
        name: prepared.name.clone(),
        size: prepared.size,
    };
    let mut progress = |n| reg.progress(id, n);
    let result = tokio::select! {
        r = send_file(&header, prepared.file, tx, rx, &mut progress) => r,
        _ = cancel.cancelled() => Err(FileError::Cancelled),
    };
    match &result {
        Ok(()) => reg.finish(id, TransferState::Done, None, None),
        Err(FileError::Cancelled) => {
            reg.finish(id, TransferState::Cancelled, Some("cancelled".into()), None)
        }
        Err(e) => reg.finish(id, TransferState::Failed, Some(e.to_string()), None),
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write as _;
    use std::path::PathBuf;
    use std::time::Duration;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("klip-files-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn hostile_names_stay_inside_the_folder() {
        let table = [
            ("../../etc/passwd", "passwd"),
            ("C:\\Windows\\x.exe", "x.exe"),
            ("/abs/path/file.txt", "file.txt"),
            ("dir/", "dir"),
            ("CON", "_CON"),
            ("con.txt", "_con.txt"),
            ("NUL.", "_NUL"),
            ("Com1.log", "_Com1.log"),
            ("lpt9", "_lpt9"),
            ("..", "file"),
            ("...", "file"),
            (".bashrc", "bashrc"),
            ("a:b*c?.txt", "a_b_c_.txt"),
            ("q\"u<o>t|e.txt", "q_u_o_t_e.txt"),
            ("name\u{202e}gnp.exe", "namegnp.exe"),
            ("a\tb\nc", "abc"),
            ("", "file"),
            ("   ", "file"),
            ("report.pdf ", "report.pdf"),
            (
                "r\u{e9}sum\u{e9} \u{65e5}\u{672c}\u{8a9e}.pdf",
                "r\u{e9}sum\u{e9} \u{65e5}\u{672c}\u{8a9e}.pdf",
            ),
        ];
        for (raw, want) in table {
            assert_eq!(sanitize_file_name(raw), want, "{raw:?}");
        }
        let long = format!("{}.pdf", "x".repeat(5000));
        let cleaned = sanitize_file_name(&long);
        assert!(
            cleaned.len() <= 200 && cleaned.ends_with(".pdf"),
            "{}",
            cleaned.len()
        );
        let wide = format!("{}.txt", "\u{65e5}".repeat(500));
        let cleaned = sanitize_file_name(&wide);
        assert!(cleaned.len() <= 200 && cleaned.ends_with(".txt"));
        assert!(std::str::from_utf8(cleaned.as_bytes()).is_ok());
    }

    #[test]
    fn an_existing_name_gets_a_number_and_is_never_replaced() {
        let dir = temp("unique");
        let mut paths = Vec::new();
        for i in 0..3 {
            let (mut f, p) = unique_path(&dir, "a.txt").unwrap();
            std::io::Write::write_all(&mut f, format!("content {i}").as_bytes()).unwrap();
            paths.push(p);
        }
        let names: Vec<String> = paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a.txt", "a (2).txt", "a (3).txt"]);
        assert_eq!(fs::read_to_string(&paths[0]).unwrap(), "content 0");
        let (_f, p) = unique_path(&dir, "noext").unwrap();
        let (_g, q) = unique_path(&dir, "noext").unwrap();
        assert_eq!(p.file_name().unwrap(), "noext");
        assert_eq!(q.file_name().unwrap(), "noext (2)");
    }

    #[test]
    fn two_threads_creating_the_same_name_never_clash() {
        let dir = temp("race");
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let dir = dir.clone();
                std::thread::spawn(move || unique_path(&dir, "same.bin").unwrap().1)
            })
            .collect();
        let mut got: Vec<PathBuf> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        got.sort();
        got.dedup();
        assert_eq!(got.len(), 16, "every thread got its own file");
        assert!(got.iter().all(|p| p.exists()));
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_private_and_not_executable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("mode");
        let (_f, p) = unique_path(&dir, "x.sh").unwrap();
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[tokio::test]
    async fn a_partial_file_is_removed_on_discard_and_renamed_on_commit() {
        use tokio::io::AsyncWriteExt;
        let dir = temp("partial");
        let mut part = PartialFile::create(&dir, [9; 16]).unwrap();
        let name = part
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(
            name.starts_with(".klip-") && name.ends_with(".part"),
            "{name}"
        );
        part.file().write_all(b"hello").await.unwrap();
        let kept = part.commit("greeting.txt").await.unwrap();
        assert_eq!(fs::read_to_string(&kept).unwrap(), "hello");
        assert_eq!(kept.file_name().unwrap(), "greeting.txt");
        assert!(!dir.join(&name).exists(), "the partial name is gone");

        // A second file with the same wanted name must not replace the first.
        let mut again = PartialFile::create(&dir, [8; 16]).unwrap();
        again.file().write_all(b"second").await.unwrap();
        let second = again.commit("greeting.txt").await.unwrap();
        assert_eq!(second.file_name().unwrap(), "greeting (2).txt");
        assert_eq!(fs::read_to_string(&kept).unwrap(), "hello");

        let mut gone = PartialFile::create(&dir, [7; 16]).unwrap();
        gone.file().write_all(b"x").await.unwrap();
        let path = gone.path().to_path_buf();
        gone.discard().await;
        assert!(!path.exists());
    }

    #[test]
    fn sweep_removes_only_old_klip_partials() {
        let dir = temp("sweep");
        let old = dir.join(format!(".klip-{}.part", "ab".repeat(16)));
        let young = dir.join(format!(".klip-{}.part", "cd".repeat(16)));
        let other = dir.join("keep.part");
        let folder = dir.join(format!(".klip-{}.part", "ef".repeat(16)));
        for p in [&old, &young, &other] {
            fs::write(p, "x").unwrap();
        }
        fs::create_dir(&folder).unwrap();
        let long_ago = std::time::SystemTime::now() - Duration::from_secs(3 * 86_400);
        fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        sweep_partials(&dir, Duration::from_secs(86_400));
        assert!(!old.exists(), "an old partial goes");
        assert!(young.exists() && other.exists() && folder.exists());
    }

    #[test]
    fn free_space_is_positive_for_a_temp_dir_and_for_a_folder_not_made_yet() {
        let dir = temp("space");
        assert!(free_space(&dir).unwrap() > 0);
        assert!(free_space(&dir.join("not").join("made").join("yet")).unwrap() > 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_where_the_folder_should_be_is_refused() {
        let base = temp("symlink");
        let real = base.join("real");
        fs::create_dir(&real).unwrap();
        let link = base.join("recv");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let err = PartialFile::create(&link, [1; 16]).err().expect("refused");
        assert!(err.to_string().to_lowercase().contains("symlink"), "{err}");
        assert!(
            fs::read_dir(&real).unwrap().next().is_none(),
            "nothing was written through it"
        );
    }

    #[test]
    fn the_default_folder_is_a_klip_folder() {
        assert_eq!(default_receive_dir().file_name().unwrap(), "Klip");
    }

    // ------------------------------------------------------------------ receiving and sending

    use klip_net::file::{send_file, FileError, FileHeader, IncomingFile, RejectCode};
    use klip_proto::ipc::{TransferDirection, TransferState};
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll};
    use tokio::io::{duplex, AsyncRead, AsyncWrite, ReadBuf};

    fn policy(dir: &Path) -> FilePolicy {
        FilePolicy {
            accept: true,
            dir: dir.to_path_buf(),
            max_bytes: 10 * 1024 * 1024,
            reserve_bytes: 0,
            space_factor: 1,
            max_incoming: 3,
            max_incoming_per_peer: 2,
        }
    }

    fn data(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i.wrapping_mul(7) % 251) as u8).collect()
    }

    /// A raw sender (so a test can put any name in the header) against `receive_incoming`.
    async fn exchange(
        policy: &FilePolicy,
        reg: &TransferRegistry,
        peer: &str,
        id: u8,
        name: &str,
        bytes: Vec<u8>,
    ) -> (Result<(), FileError>, Result<PathBuf, FileError>) {
        let (s_w, s_r) = duplex(64 * 1024);
        let (r_w, r_r) = duplex(64 * 1024);
        let header = FileHeader {
            version: 1,
            id: [id; 16],
            name: name.into(),
            size: bytes.len() as u64,
        };
        let sender = async move {
            let (mut tx, mut rx) = (s_w, r_r);
            send_file(&header, &bytes[..], &mut tx, &mut rx, &mut |_| {}).await
        };
        let receiver = async {
            let incoming = IncomingFile::open(s_r, r_w).await?;
            receive_incoming(incoming, peer, policy, reg).await
        };
        tokio::join!(sender, receiver)
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn a_file_is_received_into_a_unique_safe_name() {
        let dir = temp("recv");
        let (policy, reg) = (policy(&dir), TransferRegistry::new());
        let (sent, got) = exchange(&policy, &reg, "Vivo", 1, "../../x.sh", data(1000)).await;
        sent.unwrap();
        let path = got.unwrap();
        assert_eq!(path, dir.join("x.sh"));
        assert_eq!(fs::read(&path).unwrap(), data(1000));
        let (_, got) = exchange(&policy, &reg, "Vivo", 2, "x.sh", data(10)).await;
        assert_eq!(got.unwrap(), dir.join("x (2).sh"));
        assert_eq!(
            files_in(&dir),
            ["x (2).sh", "x.sh"],
            "no partial file is left"
        );
        let list = reg.list();
        assert_eq!(list.len(), 2);
        assert!(
            list.iter()
                .all(|t| t.state == TransferState::Done
                    && t.direction == TransferDirection::Receiving)
        );
        assert_eq!(list[0].saved_to.as_deref(), Some(path.to_str().unwrap()));
        assert_eq!(list[0].done, 1000);
    }

    #[allow(clippy::type_complexity)]
    #[tokio::test]
    async fn refusals_come_back_with_their_code_and_leave_nothing_behind() {
        let cases: Vec<(&str, Box<dyn Fn(&mut FilePolicy)>, RejectCode)> = vec![
            (
                "accept off",
                Box::new(|p| p.accept = false),
                RejectCode::Disabled,
            ),
            (
                "too large",
                Box::new(|p| p.max_bytes = 100),
                RejectCode::TooLarge,
            ),
            (
                "no space",
                Box::new(|p| p.reserve_bytes = u64::MAX / 2),
                RejectCode::NoSpace,
            ),
        ];
        for (tag, tweak, want) in cases {
            let dir = temp(&format!("refuse-{}", tag.replace(' ', "")));
            let mut p = policy(&dir);
            tweak(&mut p);
            let reg = TransferRegistry::new();
            let (sent, got) = exchange(&p, &reg, "Vivo", 3, "a.bin", data(1000)).await;
            match sent {
                Err(FileError::Rejected { code, .. }) => assert_eq!(code, want, "{tag}"),
                other => panic!("{tag}: {other:?}"),
            }
            assert!(matches!(got, Err(FileError::Rejected { .. })), "{tag}");
            assert!(files_in(&dir).is_empty(), "{tag}: {:?}", files_in(&dir));
            assert_eq!(reg.list()[0].state, TransferState::Failed, "{tag}");
        }
    }

    #[tokio::test]
    async fn the_limits_on_incoming_transfers_say_busy() {
        let dir = temp("busy");
        let p = policy(&dir);
        let reg = TransferRegistry::new();
        reg.begin(
            "Vivo",
            TransferDirection::Receiving,
            [10; 16],
            "a",
            1,
            TransferState::Active,
        );
        reg.begin(
            "Vivo",
            TransferDirection::Receiving,
            [11; 16],
            "b",
            1,
            TransferState::Active,
        );
        let (sent, _) = exchange(&p, &reg, "Vivo", 4, "c.bin", data(10)).await;
        assert!(
            matches!(
                sent,
                Err(FileError::Rejected {
                    code: RejectCode::Busy,
                    ..
                })
            ),
            "{sent:?}"
        );
        // Another device is fine until three are running in all.
        let (sent, got) = exchange(&p, &reg, "Pad", 5, "d.bin", data(10)).await;
        sent.unwrap();
        got.unwrap();
        reg.begin(
            "Pad",
            TransferDirection::Receiving,
            [12; 16],
            "e",
            1,
            TransferState::Active,
        );
        let (sent, _) = exchange(&p, &reg, "Tab", 6, "f.bin", data(10)).await;
        assert!(
            matches!(
                sent,
                Err(FileError::Rejected {
                    code: RejectCode::Busy,
                    ..
                })
            ),
            "{sent:?}"
        );
    }

    /// A writer that flips the byte at absolute stream position `at`.
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
        ) -> Poll<io::Result<usize>> {
            let start = self.seen;
            let mut bytes = buf.to_vec();
            if self.at >= start && self.at < start + bytes.len() {
                bytes[self.at - start] ^= 0xFF;
            }
            match Pin::new(&mut self.inner).poll_write(cx, &bytes) {
                Poll::Ready(Ok(n)) => {
                    self.seen += n;
                    Poll::Ready(Ok(n))
                }
                other => other,
            }
        }
        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_flush(cx)
        }
        fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }

    #[tokio::test]
    async fn a_hash_mismatch_leaves_nothing_behind() {
        let dir = temp("damaged");
        let (p, reg) = (policy(&dir), TransferRegistry::new());
        let (s_w, s_r) = duplex(64 * 1024);
        let (r_w, r_r) = duplex(64 * 1024);
        let bytes = data(300_000);
        let header = FileHeader {
            version: 1,
            id: [20; 16],
            name: "photo.jpg".into(),
            size: bytes.len() as u64,
        };
        let sender = async move {
            let mut tx = Flip {
                inner: s_w,
                at: 5000,
                seen: 0,
            };
            let mut rx = r_r;
            send_file(&header, &bytes[..], &mut tx, &mut rx, &mut |_| {}).await
        };
        let receiver = async {
            let incoming = IncomingFile::open(s_r, r_w).await?;
            receive_incoming(incoming, "Vivo", &p, &reg).await
        };
        let (sent, got) = tokio::join!(sender, receiver);
        assert!(matches!(got, Err(FileError::HashMismatch)), "{got:?}");
        assert!(matches!(sent, Err(FileError::Peer(_))), "{sent:?}");
        assert!(files_in(&dir).is_empty(), "{:?}", files_in(&dir));
        let t = &reg.list()[0];
        assert_eq!(t.state, TransferState::Failed);
        assert!(t.error.as_deref().unwrap().contains("damaged"));
    }

    /// Yields `left` bytes, then never anything again (a sender that went quiet).
    struct Stall {
        left: usize,
    }

    impl AsyncRead for Stall {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if self.left == 0 {
                return Poll::Pending;
            }
            let n = self.left.min(buf.remaining()).min(10_000);
            buf.put_slice(&vec![1u8; n]);
            self.left -= n;
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn cancel_stops_the_receive_and_removes_the_partial_file() {
        let dir = temp("cancel");
        let (p, reg) = (Arc::new(policy(&dir)), Arc::new(TransferRegistry::new()));
        let (s_w, s_r) = duplex(64 * 1024);
        let (r_w, r_r) = duplex(64 * 1024);
        let header = FileHeader {
            version: 1,
            id: [30; 16],
            name: "big.iso".into(),
            size: 5_000_000,
        };
        let sender = tokio::spawn(async move {
            let (mut tx, mut rx) = (s_w, r_r);
            send_file(
                &header,
                Stall { left: 50_000 },
                &mut tx,
                &mut rx,
                &mut |_| {},
            )
            .await
        });
        let (p2, reg2) = (Arc::clone(&p), Arc::clone(&reg));
        let receiver = tokio::spawn(async move {
            let incoming = IncomingFile::open(s_r, r_w).await?;
            receive_incoming(incoming, "Vivo", &p2, &reg2).await
        });
        for _ in 0..500 {
            if reg.list().first().is_some_and(|t| t.done > 0) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(reg.list()[0].done > 0, "the transfer is running");
        assert!(reg.cancel([30; 16]));
        let got = tokio::time::timeout(Duration::from_secs(5), receiver)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(got, Err(FileError::Cancelled)), "{got:?}");
        assert!(files_in(&dir).is_empty(), "{:?}", files_in(&dir));
        assert_eq!(reg.list()[0].state, TransferState::Cancelled);
        sender.abort();
    }

    #[test]
    fn the_registry_keeps_the_last_50_finished_and_every_active_one() {
        let reg = TransferRegistry::new();
        reg.begin(
            "Vivo",
            TransferDirection::Sending,
            [200; 16],
            "running",
            10,
            TransferState::Active,
        );
        for i in 0..60u8 {
            let id = [i; 16];
            reg.begin(
                "Vivo",
                TransferDirection::Sending,
                id,
                "f",
                10,
                TransferState::Active,
            );
            reg.progress(id, 10);
            reg.finish(id, TransferState::Done, None, None);
        }
        let list = reg.list();
        assert_eq!(
            list.iter()
                .filter(|t| t.state == TransferState::Done)
                .count(),
            50
        );
        assert_eq!(
            list.iter()
                .filter(|t| t.state == TransferState::Active)
                .count(),
            1
        );
        assert!(list.iter().any(|t| t.id == [59; 16]) && !list.iter().any(|t| t.id == [0; 16]));
        assert_eq!(reg.active_incoming(None), 0);
    }

    #[test]
    fn cancelling_a_peer_cancels_only_its_transfers() {
        let reg = TransferRegistry::new();
        let a = reg.begin(
            "Vivo",
            TransferDirection::Sending,
            [1; 16],
            "a",
            1,
            TransferState::Active,
        );
        let b = reg.begin(
            "Pad",
            TransferDirection::Receiving,
            [2; 16],
            "b",
            1,
            TransferState::Active,
        );
        assert_eq!(reg.cancel_peer("Vivo"), 1);
        assert!(a.is_cancelled() && !b.is_cancelled());
        assert!(!reg.cancel([9; 16]), "an unknown id is not an error");
    }

    #[tokio::test]
    async fn only_regular_files_can_be_sent_and_their_names_are_cleaned() {
        let dir = temp("prepare");
        assert!(prepare_send(&dir).await.is_err(), "a folder");
        assert!(prepare_send(&dir.join("missing.txt")).await.is_err());
        #[cfg(unix)]
        assert!(
            prepare_send(Path::new("/dev/null")).await.is_err(),
            "a device"
        );
        let odd = dir.join("we:ird*name.txt");
        fs::write(&odd, "hi").unwrap();
        let prepared = prepare_send(&odd).await.unwrap();
        assert_eq!(
            (prepared.name.as_str(), prepared.size),
            ("we_ird_name.txt", 2)
        );
    }

    async fn send_to_receiver(
        dir: &Path,
        prepared: PreparedFile,
        id: u8,
    ) -> (Result<(), FileError>, Result<PathBuf, FileError>) {
        let (s_w, s_r) = duplex(64 * 1024);
        let (r_w, r_r) = duplex(64 * 1024);
        let (p, send_reg, recv_reg) = (
            policy(dir),
            TransferRegistry::new(),
            TransferRegistry::new(),
        );
        let token = send_reg.begin(
            "Pad",
            TransferDirection::Sending,
            [id; 16],
            &prepared.name,
            prepared.size,
            TransferState::Waiting,
        );
        let sender = async {
            let (mut tx, mut rx) = (s_w, r_r);
            send_prepared(prepared, [id; 16], &mut tx, &mut rx, token, &send_reg).await
        };
        let receiver = async {
            let incoming = IncomingFile::open(s_r, r_w).await?;
            receive_incoming(incoming, "Laptop", &p, &recv_reg).await
        };
        tokio::join!(sender, receiver)
    }

    #[tokio::test]
    async fn a_prepared_file_is_sent_and_a_file_that_changed_size_is_not() {
        let src = temp("src");
        let dst = temp("dst");
        let file = src.join("doc.txt");
        fs::write(&file, data(100_000)).unwrap();
        let (sent, got) = send_to_receiver(&dst, prepare_send(&file).await.unwrap(), 40).await;
        sent.unwrap();
        assert_eq!(fs::read(got.unwrap()).unwrap(), data(100_000));

        // It grows after it was measured: the send fails, the receiver keeps nothing.
        let prepared = prepare_send(&file).await.unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .unwrap()
            .write_all(b"more")
            .unwrap();
        let dst2 = temp("dst2");
        let (sent, got) = send_to_receiver(&dst2, prepared, 41).await;
        assert!(matches!(sent, Err(FileError::TooLong)), "{sent:?}");
        assert!(got.is_err());
        assert!(files_in(&dst2).is_empty());

        // It shrinks.
        let prepared = prepare_send(&file).await.unwrap();
        fs::write(&file, b"tiny").unwrap();
        let dst3 = temp("dst3");
        let (sent, got) = send_to_receiver(&dst3, prepared, 42).await;
        assert!(matches!(sent, Err(FileError::TooShort)), "{sent:?}");
        assert!(got.is_err());
        assert!(files_in(&dst3).is_empty());
    }

    #[tokio::test]
    async fn a_failed_commit_leaves_no_placeholder_and_no_partial() {
        use tokio::io::AsyncWriteExt;
        let dir = temp("commitfail");
        let mut part = PartialFile::create(&dir, [5; 16]).unwrap();
        part.file().write_all(b"x").await.unwrap();
        fs::remove_file(part.path()).unwrap(); // the rename will have nothing to move
        assert!(part.commit("a.txt").await.is_err());
        assert!(files_in(&dir).is_empty(), "{:?}", files_in(&dir));
    }
}
