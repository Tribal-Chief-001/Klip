//! Receiving and sending files, the parts that touch the disk: safe names, unique paths that never
//! replace anything, free space, and partial files that become permanent only when complete.
use klip_net::file::{send_file, FileError, FileHeader, IncomingFile, RejectCode};
use klip_proto::hlc::NodeId;
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

/// A partial smaller than this is not worth keeping for a resume.
const MIN_KEEP_BYTES: u64 = 1024 * 1024;
const MAX_KEPT_PER_DEVICE: usize = 8;
const MAX_KEPT_TOTAL: usize = 32;

/// The key a sender gives a file so a second send of the same file finds the partial the first one left:
/// BLAKE3 of the name, size, modification time and the first bytes. Never all zeros (that means "no resume").
pub fn resume_key(name: &str, size: u64, mtime_ns: u128, head: &[u8]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(b"klip resume v1");
    h.update(&(name.len() as u64).to_le_bytes());
    h.update(name.as_bytes());
    h.update(&size.to_le_bytes());
    h.update(&mtime_ns.to_le_bytes());
    h.update(blake3::hash(head).as_bytes());
    let key = *h.finalize().as_bytes();
    if key == [0u8; 32] {
        [1u8; 32]
    } else {
        key
    }
}

/// Partials that are open right now (a second transfer for the same one must wait or be told Busy).
static IN_USE: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

fn release(path: &Path) {
    IN_USE.lock().unwrap().retain(|p| p != path);
}

/// The partials kept for a later resume, oldest first: at most 8 per device and 32 in all; going over removes the
/// oldest file.
pub struct KeptIndex {
    items: Mutex<Vec<(PathBuf, Vec<u8>)>>,
}

impl KeptIndex {
    pub const fn new() -> Self {
        Self {
            items: Mutex::new(Vec::new()),
        }
    }

    pub fn register(&self, path: PathBuf, device: Vec<u8>) {
        let mut items = self.items.lock().unwrap();
        items.retain(|(p, _)| p != &path);
        items.push((path, device.clone()));
        while items.iter().filter(|(_, d)| *d == device).count() > MAX_KEPT_PER_DEVICE {
            let i = items.iter().position(|(_, d)| *d == device).unwrap();
            let _ = fs::remove_file(items.remove(i).0);
        }
        while items.len() > MAX_KEPT_TOTAL {
            let _ = fs::remove_file(items.remove(0).0);
        }
    }

    pub fn forget(&self, path: &Path) {
        self.items.lock().unwrap().retain(|(p, _)| p != path);
    }

    /// Delete every partial kept for this device (it was unpaired: nothing of it stays on this one).
    pub fn forget_device(&self, device: &[u8]) {
        self.items.lock().unwrap().retain(|(path, d)| {
            if d == device {
                let _ = fs::remove_file(path);
            }
            d != device
        });
    }
}

/// `KeptIndex::forget_device` on the index the receiver uses.
pub fn forget_kept_partials_of(node: NodeId) {
    KEPT.forget_device(node.fingerprint().as_bytes());
}

impl Default for KeptIndex {
    fn default() -> Self {
        Self::new()
    }
}

static KEPT: KeptIndex = KeptIndex::new();

fn keyed_path(dir: &Path, device: &[u8], key: &[u8; 32]) -> PathBuf {
    let mut h = blake3::Hasher::new();
    h.update(device);
    h.update(key);
    let mut id = [0u8; 16];
    id.copy_from_slice(&h.finalize().as_bytes()[..16]);
    dir.join(partial_name(&id))
}

/// How many bytes of this file a kept partial already holds (0 when there is none).
fn kept_len(dir: &Path, device: &[u8], key: &[u8; 32]) -> u64 {
    fs::metadata(keyed_path(dir, device, key))
        .map(|m| m.len())
        .unwrap_or(0)
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
    /// Opened by resume key: it is in the in-use set and may be kept after a failure.
    keyed: bool,
}

#[cfg(windows)]
const RENAME_RETRY_DELAYS: &[Duration] = &[
    Duration::from_millis(50),
    Duration::from_millis(100),
    Duration::from_millis(200),
    Duration::from_millis(400),
    Duration::from_millis(800),
];
#[cfg(not(windows))]
const RENAME_RETRY_DELAYS: &[Duration] = &[];

/// Run `rename`; when it fails, wait for each delay in turn and run it again. One try plus one per delay.
async fn rename_retrying(
    mut rename: impl FnMut() -> io::Result<()>,
    delays: &[Duration],
) -> io::Result<()> {
    let mut result = rename();
    for delay in delays {
        if result.is_ok() {
            break;
        }
        tokio::time::sleep(*delay).await;
        result = rename();
    }
    result
}

impl PartialFile {
    pub fn create(dir: &Path, id: [u8; 16]) -> io::Result<Self> {
        ensure_dir(dir)?;
        let path = dir.join(partial_name(&id));
        let file = private_options().open(&path)?;
        Ok(Self {
            keyed: false,
            file: tokio::fs::File::from_std(file),
            path,
            dir: dir.to_path_buf(),
        })
    }

    /// Open (or create) the partial for a device's file with this resume key, and say how many bytes it holds.
    /// `AlreadyExists`: another transfer is using it right now.
    pub async fn open_keyed(dir: &Path, device: &[u8], key: &[u8; 32]) -> io::Result<(Self, u64)> {
        ensure_dir(dir)?;
        let path = keyed_path(dir, device, key);
        {
            let mut used = IN_USE.lock().unwrap();
            if used.contains(&path) {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "this file is already being received",
                ));
            }
            used.push(path.clone());
        }
        let opened = (|| {
            let mut options = OpenOptions::new();
            options.read(true).write(true).create(true).truncate(false);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let file = options.open(&path)?;
            let len = file.metadata()?.len();
            Ok::<_, io::Error>((file, len))
        })();
        match opened {
            Ok((file, len)) => {
                KEPT.forget(&path);
                Ok((
                    Self {
                        file: tokio::fs::File::from_std(file),
                        path,
                        dir: dir.to_path_buf(),
                        keyed: true,
                    },
                    len,
                ))
            }
            Err(e) => {
                release(&path);
                Err(e)
            }
        }
    }

    /// Hash the first `len` bytes already in the file and leave the write position right after them.
    pub async fn hash_existing(&mut self, len: u64) -> io::Result<blake3::Hasher> {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        self.file.seek(io::SeekFrom::Start(0)).await?;
        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; 256 * 1024];
        let mut left = len;
        while left > 0 {
            let want = left.min(buf.len() as u64) as usize;
            let n = self.file.read(&mut buf[..want]).await?;
            if n == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            hasher.update(&buf[..n]);
            left -= n as u64;
        }
        self.file.seek(io::SeekFrom::Start(len)).await?;
        Ok(hasher)
    }

    /// Forget what the file holds and start from its beginning.
    pub async fn restart(&mut self) -> io::Result<()> {
        use tokio::io::AsyncSeekExt;
        self.file.set_len(0).await?;
        self.file.seek(io::SeekFrom::Start(0)).await?;
        Ok(())
    }

    pub fn bytes_on_disk(&self) -> u64 {
        fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0)
    }

    /// The transfer broke off: keep what arrived for a later resume (when it is worth it), else delete it.
    pub async fn keep(self, device: Vec<u8>) {
        let Self {
            file, path, keyed, ..
        } = self;
        drop(file);
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if keyed && size >= MIN_KEEP_BYTES {
            KEPT.register(path.clone(), device);
        } else {
            let _ = fs::remove_file(&path);
        }
        if keyed {
            release(&path);
        }
    }

    pub fn file(&mut self) -> &mut tokio::fs::File {
        &mut self.file
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Flush it to disk and give it its real (sanitised, unique) name. Returns where it is now.
    pub async fn commit(self, wanted_name: &str) -> io::Result<PathBuf> {
        self.commit_in(None, wanted_name).await
    }

    /// `commit`, but into another folder than the receive folder (a file of a folder send).
    pub async fn commit_in(self, into: Option<&Path>, wanted_name: &str) -> io::Result<PathBuf> {
        let Self {
            file,
            path,
            dir,
            keyed,
        } = self;
        let dir = into.map_or(dir, Path::to_path_buf);
        let made_permanent = async {
            file.sync_all().await?;
            drop(file);
            let (placeholder, target) = unique_path(&dir, &sanitize_file_name(wanted_name))?;
            drop(placeholder);
            // The placeholder is ours (made by create_new), so renaming over it replaces nothing of the person's.
            // On Windows a virus scanner or the indexer may hold the file for a moment: try again a few times.
            match rename_retrying(|| fs::rename(&path, &target), RENAME_RETRY_DELAYS).await {
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
        if keyed {
            release(&path);
        }
        made_permanent
    }

    /// Throw the partial file away.
    pub async fn discard(self) {
        let Self {
            file, path, keyed, ..
        } = self;
        drop(file);
        let _ = fs::remove_file(&path);
        if keyed {
            release(&path);
        }
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
    /// Which device it is with (names are not unique); `None` until tagged.
    node: Option<NodeId>,
    /// The receiver is making the file permanent: too late to cancel.
    committing: bool,
}

/// Every transfer that is running, and the last 50 that finished (memory only: names are private).
#[derive(Default)]
pub struct TransferRegistry {
    entries: Mutex<Vec<Entry>>,
    on_change: Mutex<Option<std::sync::Arc<dyn Fn() + Send + Sync>>>,
    /// The daemon is shutting down: cancelled transfers are not the person's decision, so partials are kept.
    stopping: std::sync::atomic::AtomicBool,
    /// Folder sends being received: which new top folder each one goes to.
    pub(crate) batches: crate::folders::BatchBook,
}

fn is_running(state: TransferState) -> bool {
    matches!(state, TransferState::Waiting | TransferState::Active)
}

impl TransferRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Called (outside any lock) whenever a transfer begins or ends, never for progress: a host that
    /// wants to show progress polls `list` only while something runs.
    pub fn set_on_change(&self, hook: std::sync::Arc<dyn Fn() + Send + Sync>) {
        *self.on_change.lock().unwrap() = Some(hook);
    }

    fn changed(&self) {
        let hook = self.on_change.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook();
        }
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
            node: None,
            committing: false,
        });
        Self::trim(&mut entries);
        drop(entries);
        self.changed();
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
        self.changed();
    }

    pub fn list(&self) -> Vec<TransferInfo> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.info.clone())
            .collect()
    }

    /// Remember which device a transfer is with, so unpairing one of two devices with the same name stops
    /// only that one's transfers.
    pub fn tag_node(&self, id: [u8; 16], node: NodeId) {
        self.with(id, |e| e.node = Some(node));
    }

    /// The receiver is about to make the file permanent. `false`: it was cancelled first, do not commit.
    /// After a `true` the transfer can no longer be cancelled (`cancel` answers `false`).
    pub fn begin_commit(&self, id: [u8; 16]) -> bool {
        let mut entries = self.entries.lock().unwrap();
        match entries.iter_mut().rev().find(|e| e.info.id == id) {
            Some(e) if !*e.cancel.borrow() => {
                e.committing = true;
                true
            }
            _ => false,
        }
    }

    /// Ask a running transfer to stop. `false` when there is no running transfer with that id, or its file is
    /// already being made permanent.
    pub fn cancel(&self, id: [u8; 16]) -> bool {
        let entries = self.entries.lock().unwrap();
        match entries
            .iter()
            .rev()
            .find(|e| e.info.id == id && is_running(e.info.state) && !e.committing)
        {
            Some(entry) => {
                entry.cancel.send_replace(true);
                true
            }
            None => false,
        }
    }

    /// Stop every running transfer (the daemon is shutting down).
    pub fn cancel_all(&self) {
        self.stopping
            .store(true, std::sync::atomic::Ordering::SeqCst);
        for entry in self.entries.lock().unwrap().iter() {
            if is_running(entry.info.state) {
                entry.cancel.send_replace(true);
            }
        }
    }

    pub fn is_stopping(&self) -> bool {
        self.stopping.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn running(&self) -> usize {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .filter(|e| is_running(e.info.state))
            .count()
    }

    /// `begin` for an incoming transfer, but only if the limits still hold: the check and the entry are
    /// one step, so transfers arriving on different threads cannot all slip under the limit.
    pub fn begin_incoming(
        &self,
        peer: &str,
        id: [u8; 16],
        name: &str,
        size: u64,
        max_total: usize,
        max_per_peer: usize,
    ) -> Option<CancelToken> {
        let (cancel, token) = watch::channel(false);
        let mut entries = self.entries.lock().unwrap();
        let incoming = |e: &&Entry| {
            e.info.direction == TransferDirection::Receiving && is_running(e.info.state)
        };
        if entries.iter().filter(incoming).count() >= max_total
            || entries
                .iter()
                .filter(incoming)
                .filter(|e| e.info.peer == peer)
                .count()
                >= max_per_peer
        {
            return None;
        }
        entries.push(Entry {
            info: TransferInfo {
                id,
                peer: peer.to_string(),
                direction: TransferDirection::Receiving,
                name: name.to_string(),
                size,
                done: 0,
                state: TransferState::Active,
                error: None,
                saved_to: None,
                started_ms: now_ms(),
                finished_ms: None,
            },
            cancel,
            node: None,
            committing: false,
        });
        Self::trim(&mut entries);
        drop(entries);
        self.changed();
        Some(CancelToken(token))
    }

    /// Stop every running transfer with a device (it was unpaired): by identity, and by name for a transfer
    /// that is not tagged with one yet. A file already being made permanent is left alone.
    pub fn cancel_peer(&self, node: NodeId, name: &str) -> usize {
        let entries = self.entries.lock().unwrap();
        let mut stopped = 0;
        for entry in entries.iter().filter(|e| {
            is_running(e.info.state)
                && !e.committing
                && match e.node {
                    Some(n) => n == node,
                    None => e.info.peer == name,
                }
        }) {
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

/// "812 B", "3.5 KiB", "12.0 MiB", "1.5 GiB".
pub fn human_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KIB * KIB * KIB {
        format!("{:.1} GiB", b / (KIB * KIB * KIB))
    } else if b >= KIB * KIB {
        format!("{:.1} MiB", b / (KIB * KIB))
    } else if b >= KIB {
        format!("{:.1} KiB", b / KIB)
    } else {
        format!("{bytes} B")
    }
}

/// Why a file is not accepted, if it is not: checked before a single byte is read.
fn refusal(
    header: &FileHeader,
    have: u64,
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
                human_size(header.size),
                human_size(policy.max_bytes)
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
            // A resumed file only needs room for the bytes still to come.
            let needed = header
                .size
                .saturating_sub(have)
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
    receive_incoming_from(incoming, peer, None, policy, reg).await
}

/// `receive_incoming` for a device whose identity is known (so unpairing it stops this transfer).
pub async fn receive_incoming_from<R, W>(
    incoming: IncomingFile<R, W>,
    peer: &str,
    node: Option<NodeId>,
    policy: &FilePolicy,
    reg: &TransferRegistry,
) -> Result<PathBuf, FileError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let header = incoming.header().clone();
    // Our own id: the sender's is only a claim, and two transfers with one id would share a registry entry.
    let mut id = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut id);
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
    // Version 2 with a resume key: the partial of this device's earlier attempt, if it kept one.
    let key = incoming
        .v2()
        .map(|h| h.resume_key)
        .filter(|k| *k != [0u8; 32]);
    let device: Vec<u8> = node
        .map(|n| n.fingerprint().into_bytes())
        .unwrap_or_else(|| peer.as_bytes().to_vec());
    let have = key.map_or(0, |k| kept_len(&policy.dir, &device, &k));
    if let Some((code, text)) = refusal(&header, have.min(header.size), peer, policy, reg) {
        let (code, text) = refuse(code, text);
        incoming.reject(code, &text).await;
        return Err(FileError::Rejected { code, text });
    }
    let opened = match key {
        Some(k) => PartialFile::open_keyed(&policy.dir, &device, &k).await,
        None => PartialFile::create(&policy.dir, id).map(|part| (part, 0)),
    };
    let (mut part, mut offset) = match opened {
        Ok(opened) => opened,
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
    let Some(mut cancel) = reg.begin_incoming(
        peer,
        id,
        &name,
        header.size,
        policy.max_incoming,
        policy.max_incoming_per_peer,
    ) else {
        part.discard().await;
        let (code, text) = refuse(
            RejectCode::Busy,
            "this device is already receiving other files; try again in a moment".into(),
        );
        incoming.reject(code, &text).await;
        return Err(FileError::Rejected { code, text });
    };
    if let Some(node) = node {
        reg.tag_node(id, node);
    }
    // A file of a folder send goes into that send's new top folder, in the folders it came from.
    let place = incoming.v2().and_then(|h| h.folder.clone());
    let mut target_dir: Option<PathBuf> = None;
    if let Some(place) = &place {
        let made = reg
            .batches
            .dir(&device, place.batch, &place.top, &policy.dir)
            .and_then(|top| crate::folders::place_in_folder(&top, &place.rel));
        match made {
            Ok(dir) => target_dir = Some(dir),
            Err(e) => {
                part.discard().await;
                reg.batches.failed(&device, place.batch);
                let text = format!("this device cannot save the folder: {e}");
                reg.finish(id, TransferState::Failed, Some(text.clone()), None);
                incoming.reject(RejectCode::Other, &text).await;
                return Err(FileError::Rejected {
                    code: RejectCode::Other,
                    text,
                });
            }
        }
    }
    // What the earlier attempt left must still be hashed here; an unusable one is thrown away.
    let mut prefix = blake3::Hasher::new();
    if offset > header.size {
        offset = 0;
        let _ = part.restart().await;
    } else if offset > 0 {
        match part.hash_existing(offset).await {
            Ok(hasher) => prefix = hasher,
            Err(_) => {
                offset = 0;
                let _ = part.restart().await;
            }
        }
    }
    let mut progress = |n| reg.progress(id, n);
    let resumable = incoming.v2().is_some();
    let outcome = tokio::select! {
        r = async {
            if resumable {
                incoming.accept_from(offset, prefix, part.file(), &mut progress).await
            } else {
                incoming.accept(part.file(), &mut progress).await
            }
        } => r,
        _ = cancel.cancelled() => Err(FileError::Cancelled),
    };
    // A cancel that came in after the last byte but before the rename still wins; after this the file is
    // being made permanent and `cancel` says so.
    let outcome = match outcome {
        Ok(received) if !reg.begin_commit(id) => {
            received.bad("the transfer was cancelled").await;
            Err(FileError::Cancelled)
        }
        other => other,
    };
    match outcome {
        Ok(received) => match part.commit_in(target_dir.as_deref(), &name).await {
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
            // A broken connection (or the daemon stopping) leaves a partial the next send can continue; a damaged
            // file, a full disk or the person's own cancel does not.
            let keep = key.is_some()
                && match &e {
                    FileError::Timeout | FileError::TooShort | FileError::Io(_) => true,
                    FileError::Cancelled => reg.is_stopping(),
                    _ => false,
                };
            if keep {
                part.keep(device.clone()).await;
            } else {
                part.discard().await;
            }
            if let Some(place) = &place {
                reg.batches.failed(&device, place.batch);
            }
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
    /// Modification time (nanoseconds since the epoch, 0 when unknown): part of the resume key.
    mtime_ns: u128,
    /// Sends of one open file to several devices share its read position: they take turns on this.
    serial: Option<std::sync::Arc<tokio::sync::Mutex<()>>>,
}

impl PreparedFile {
    /// From a file somebody else opened (an Android descriptor): must be a regular file.
    pub fn from_file(
        file: File,
        name: &str,
        serial: Option<std::sync::Arc<tokio::sync::Mutex<()>>>,
    ) -> io::Result<Self> {
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(io::Error::other("only regular files can be sent"));
        }
        Ok(Self {
            file: tokio::fs::File::from_std(file),
            name: sanitize_file_name(name),
            size: meta.len(),
            mtime_ns: mtime_ns(&meta),
            serial,
        })
    }
}

fn mtime_ns(meta: &fs::Metadata) -> u128 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos())
}

pub async fn prepare_send(path: &Path) -> io::Result<PreparedFile> {
    // Look before opening: opening a FIFO would block until someone writes to it.
    if !tokio::fs::metadata(path).await?.is_file() {
        return Err(io::Error::other("only regular files can be sent"));
    }
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
        mtime_ns: mtime_ns(&meta),
        serial: None,
    })
}

/// One try at sending a prepared file over streams that are already open. Version 2 offers a resume key (or a
/// fresh random one when `fresh_key`), so a receiver that kept a partial of this very file continues from it.
/// Returns the result and the offset the receiver continued from (0: a full send). The registry row is
/// not finished here: the caller may try again.
#[allow(clippy::too_many_arguments)]
pub async fn send_attempt<R, W>(
    prepared: &mut PreparedFile,
    id: [u8; 16],
    version: u8,
    fresh_key: bool,
    folder: Option<&klip_net::file::FolderPlace>,
    base: u64,
    tx: &mut W,
    rx: &mut R,
    cancel: &mut CancelToken,
    reg: &TransferRegistry,
) -> (Result<(), FileError>, u64)
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    reg.set_active(id);
    let mut resumed = 0u64;
    let serial = prepared.serial.clone();
    let (name, size, mtime) = (prepared.name.clone(), prepared.size, prepared.mtime_ns);
    let mut progress = |n| reg.progress(id, base + n);
    let result = tokio::select! {
        r = async {
            // Take our turn on a file shared with other sends, then start from its beginning.
            let _turn = match serial {
                Some(m) => Some(m.lock_owned().await),
                None => None,
            };
            let io = |e: io::Error| FileError::Io(e.to_string());
            prepared.file.seek(io::SeekFrom::Start(0)).await.map_err(io)?;
            if version >= 2 {
                let key = if fresh_key {
                    let mut k = [0u8; 32];
                    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut k);
                    k
                } else {
                    let mut head = Vec::with_capacity(64 * 1024);
                    (&mut prepared.file)
                        .take(64 * 1024)
                        .read_to_end(&mut head)
                        .await
                        .map_err(io)?;
                    prepared.file.seek(io::SeekFrom::Start(0)).await.map_err(io)?;
                    resume_key(&name, size, mtime, &head)
                };
                let v2 = klip_net::file::FileHeaderV2 {
                    version: 2,
                    id,
                    name: name.clone(),
                    size,
                    resume_key: key,
                    folder: folder.cloned(),
                };
                klip_net::file::send_file_v2_offset(&v2, &mut prepared.file, tx, rx, &mut progress, &mut resumed).await
            } else {
                let header = FileHeader { version: 1, id, name: name.clone(), size };
                send_file(&header, &mut prepared.file, tx, rx, &mut progress).await
            }
        } => r,
        _ = cancel.cancelled() => Err(FileError::Cancelled),
    };
    (result, resumed)
}

/// Record how a send ended in the registry.
pub fn finish_send(reg: &TransferRegistry, id: [u8; 16], result: &Result<(), FileError>) {
    match result {
        Ok(()) => reg.finish(id, TransferState::Done, None, None),
        Err(FileError::Cancelled) => {
            reg.finish(id, TransferState::Cancelled, Some("cancelled".into()), None)
        }
        Err(e) => reg.finish(id, TransferState::Failed, Some(e.to_string()), None),
    }
}

/// Send a prepared file once over open streams and record the result (no retry: see `send_attempt`).
pub async fn send_prepared<R, W>(
    mut prepared: PreparedFile,
    id: [u8; 16],
    version: u8,
    tx: &mut W,
    rx: &mut R,
    mut cancel: CancelToken,
    reg: &TransferRegistry,
) -> Result<(), FileError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let (result, _) = send_attempt(
        &mut prepared,
        id,
        version,
        false,
        None,
        0,
        tx,
        rx,
        &mut cancel,
        reg,
    )
    .await;
    finish_send(reg, id, &result);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_written_in_the_unit_a_person_reads() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert_eq!(human_size(12 * 1024 * 1024), "12.0 MiB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024 / 2), "1.5 GiB");
    }
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
        assert!(reg.cancel(reg.list()[0].id));
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
        assert_eq!(reg.cancel_peer(NodeId::new([7; 32]), "Vivo"), 1);
        assert!(a.is_cancelled() && !b.is_cancelled());
        assert!(!reg.cancel([9; 16]), "an unknown id is not an error");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_fifo_is_refused_at_once_instead_of_blocking() {
        let dir = temp("fifo");
        let fifo = dir.join("pipe");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let r = tokio::time::timeout(Duration::from_secs(2), prepare_send(&fifo)).await;
        assert!(
            matches!(r, Ok(Err(_))),
            "{:?}",
            r.map(|r| r.map(|p| p.name))
        );
    }

    #[tokio::test]
    async fn a_peer_reusing_a_transfer_id_gets_its_own_entry() {
        let dir = temp("reuse");
        let p = policy(&dir);
        let reg = TransferRegistry::new();
        for id in [3u8, 11] {
            reg.begin(
                "Vivo",
                TransferDirection::Receiving,
                [id; 16],
                "a",
                1,
                TransferState::Active,
            );
        }
        let (sent, _) = exchange(&p, &reg, "Vivo", 3, "c.bin", data(10)).await;
        assert!(matches!(
            sent,
            Err(FileError::Rejected {
                code: RejectCode::Busy,
                ..
            })
        ));
        let ids: std::collections::HashSet<_> = reg.list().iter().map(|t| t.id).collect();
        assert_eq!(
            ids.len(),
            3,
            "the refusal must not share the running transfer's id"
        );
        assert!(
            reg.cancel([3; 16]),
            "the running one is still running and can be stopped"
        );
    }

    #[test]
    fn the_incoming_limit_is_checked_and_taken_in_one_step() {
        let reg = TransferRegistry::new();
        let go = |peer: &str, n: u8| reg.begin_incoming(peer, [n; 16], "f", 1, 3, 2);
        assert!(go("A", 1).is_some() && go("A", 2).is_some());
        assert!(go("A", 3).is_none(), "two from one device");
        assert!(go("B", 4).is_some());
        assert!(go("C", 5).is_none(), "three in all");
        reg.finish([1; 16], TransferState::Done, None, None);
        assert!(go("C", 6).is_some(), "a finished one frees its place");
    }

    // ---- resume ----------------------------------------------------------------------------------------

    /// A source that fails ("the network dropped") after `left` bytes.
    struct CutAfter {
        data: Vec<u8>,
        pos: usize,
        left: usize,
    }

    impl AsyncRead for CutAfter {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if self.left == 0 && self.pos < self.data.len() {
                return Poll::Ready(Err(io::Error::other("cut")));
            }
            let n = buf
                .remaining()
                .min(self.data.len() - self.pos)
                .min(self.left);
            let start = self.pos;
            buf.put_slice(&self.data[start..start + n]);
            self.pos += n;
            self.left -= n;
            Poll::Ready(Ok(()))
        }
    }

    const MIB: usize = 1024 * 1024;

    /// A version 2 transfer with resume key `key`; the sender fails after `cut_after` bytes when it is given.
    /// Returns the sender's result, the offset the receiver asked to continue from, and the receiver's result.
    async fn exchange_v2(
        policy: &FilePolicy,
        reg: &TransferRegistry,
        peer: &str,
        key: [u8; 32],
        bytes: &[u8],
        cut_after: Option<usize>,
    ) -> (Result<(), FileError>, u64, Result<PathBuf, FileError>) {
        let (s_w, s_r) = duplex(64 * 1024);
        let (r_w, r_r) = duplex(64 * 1024);
        let header = klip_net::file::FileHeaderV2 {
            version: 2,
            id: [1; 16],
            name: "movie.mkv".into(),
            size: bytes.len() as u64,
            resume_key: key,
            folder: None,
        };
        let source = CutAfter {
            data: bytes.to_vec(),
            pos: 0,
            left: cut_after.unwrap_or(usize::MAX),
        };
        let sender = async move {
            let (mut tx, mut rx) = (s_w, r_r);
            let mut offset = 0;
            let r = klip_net::file::send_file_v2_offset(
                &header,
                source,
                &mut tx,
                &mut rx,
                &mut |_| {},
                &mut offset,
            )
            .await;
            (r, offset)
        };
        let receiver = async {
            let incoming = IncomingFile::open_v2(s_r, r_w).await?;
            receive_incoming_from(incoming, peer, None, policy, reg).await
        };
        let ((sent, offset), got) = tokio::join!(sender, receiver);
        (sent, offset, got)
    }

    fn partials_in(dir: &Path) -> Vec<String> {
        files_in(dir)
            .into_iter()
            .filter(|n| n.starts_with(".klip-") && n.ends_with(".part"))
            .collect()
    }

    #[tokio::test]
    async fn a_cut_transfer_keeps_a_partial_and_the_next_one_resumes() {
        let dir = temp("resume1");
        let p = policy(&dir);
        let reg = TransferRegistry::new();
        let data = data(3 * MIB);
        let (sent, _, got) = exchange_v2(&p, &reg, "Pad", [5; 32], &data, Some(2 * MIB)).await;
        assert!(sent.is_err() && got.is_err(), "{sent:?} {got:?}");
        let kept = partials_in(&dir);
        assert_eq!(kept.len(), 1, "{:?}", files_in(&dir));
        assert_eq!(
            fs::metadata(dir.join(&kept[0])).unwrap().len(),
            2 * MIB as u64
        );

        let (sent, offset, got) = exchange_v2(&p, &reg, "Pad", [5; 32], &data, None).await;
        sent.unwrap();
        assert_eq!(
            offset,
            2 * MIB as u64,
            "the receiver asked to continue where it stopped"
        );
        let path = got.unwrap();
        assert_eq!(fs::read(&path).unwrap(), data);
        assert!(partials_in(&dir).is_empty(), "{:?}", files_in(&dir));
    }

    #[tokio::test]
    async fn a_resume_with_a_changed_prefix_ends_bad_and_keeps_nothing() {
        let dir = temp("resume2");
        let p = policy(&dir);
        let reg = TransferRegistry::new();
        let a = data(3 * MIB);
        let (_, _, _) = exchange_v2(&p, &reg, "Pad", [6; 32], &a, Some(2 * MIB)).await;
        assert_eq!(partials_in(&dir).len(), 1);
        let mut b = a.clone();
        b[10] ^= 0xFF; // same key (the sender's idea of "the same file"), different bytes at the start
        let (sent, offset, got) = exchange_v2(&p, &reg, "Pad", [6; 32], &b, None).await;
        assert_eq!(offset, 2 * MIB as u64);
        assert!(matches!(got, Err(FileError::HashMismatch)), "{got:?}");
        assert!(matches!(sent, Err(FileError::Peer(_))), "{sent:?}");
        assert!(
            partials_in(&dir).is_empty(),
            "a damaged partial is deleted: {:?}",
            files_in(&dir)
        );
        assert!(files_in(&dir).is_empty());
    }

    #[tokio::test]
    async fn a_partial_under_one_mib_is_not_kept() {
        let dir = temp("resume3");
        let p = policy(&dir);
        let reg = TransferRegistry::new();
        let data = data(3 * MIB);
        let (_, _, got) = exchange_v2(&p, &reg, "Pad", [7; 32], &data, Some(500 * 1024)).await;
        assert!(got.is_err());
        assert!(files_in(&dir).is_empty(), "{:?}", files_in(&dir));
    }

    #[tokio::test]
    async fn two_devices_with_the_same_file_do_not_share_a_partial() {
        let dir = temp("resume4");
        let p = policy(&dir);
        let reg = TransferRegistry::new();
        let data = data(3 * MIB);
        let _ = exchange_v2(&p, &reg, "Pad", [8; 32], &data, Some(2 * MIB)).await;
        let (sent, offset, got) = exchange_v2(&p, &reg, "Tab", [8; 32], &data, None).await;
        sent.unwrap();
        assert_eq!(offset, 0, "another device starts from zero");
        assert_eq!(fs::read(got.unwrap()).unwrap(), data);
        assert_eq!(partials_in(&dir).len(), 1, "Pad's partial is still there");
    }

    #[tokio::test]
    async fn a_partial_that_is_in_use_answers_busy() {
        let dir = temp("resume5");
        let p = policy(&dir);
        let reg = TransferRegistry::new();
        let (_held, _) = PartialFile::open_keyed(&dir, b"Pad", &[9; 32])
            .await
            .unwrap();
        let (sent, _, got) = exchange_v2(&p, &reg, "Pad", [9; 32], &data(1000), None).await;
        assert!(
            matches!(
                got,
                Err(FileError::Rejected {
                    code: RejectCode::Busy,
                    ..
                })
            ),
            "{got:?}"
        );
        assert!(matches!(sent, Err(FileError::Rejected { .. })), "{sent:?}");
    }

    #[test]
    fn kept_partials_are_bounded_per_device_and_in_total() {
        let dir = temp("resume6");
        let index = KeptIndex::default();
        let mk = |i: usize| {
            let path = dir.join(format!(".klip-{i:032x}.part"));
            fs::write(&path, b"x").unwrap();
            path
        };
        for i in 0..12 {
            index.register(mk(i), b"Pad".to_vec());
        }
        let left = (0..12)
            .filter(|i| dir.join(format!(".klip-{i:032x}.part")).exists())
            .count();
        assert_eq!(left, 8, "at most 8 per device; the oldest go first");
        assert!(!dir.join(format!(".klip-{:032x}.part", 0)).exists());
        assert!(dir.join(format!(".klip-{:032x}.part", 11)).exists());
        for d in 0..5u8 {
            for i in 0..8 {
                index.register(mk(100 + usize::from(d) * 10 + i), vec![d]);
            }
        }
        let total = fs::read_dir(&dir).unwrap().count();
        assert!(total <= 32, "{total} partials kept");
    }

    #[test]
    fn forgetting_a_device_deletes_only_its_kept_partials() {
        let dir = temp("resume7");
        let index = KeptIndex::default();
        let (a, b) = (dir.join(".klip-aaaa"), dir.join(".klip-bbbb"));
        fs::write(&a, b"x").unwrap();
        fs::write(&b, b"x").unwrap();
        index.register(a.clone(), b"Pad".to_vec());
        index.register(b.clone(), b"Tab".to_vec());
        index.forget_device(b"Pad");
        assert!(!a.exists() && b.exists());
    }

    #[test]
    fn the_resume_key_changes_with_name_size_mtime_and_head() {
        let k =
            |name: &str, size: u64, mtime: u128, head: &[u8]| resume_key(name, size, mtime, head);
        let base = k("a.bin", 10, 5, b"head");
        assert_eq!(base, k("a.bin", 10, 5, b"head"));
        for other in [
            k("b.bin", 10, 5, b"head"),
            k("a.bin", 11, 5, b"head"),
            k("a.bin", 10, 6, b"head"),
            k("a.bin", 10, 5, b"hear"),
        ] {
            assert_ne!(base, other);
        }
        assert_ne!(
            base, [0u8; 32],
            "the all-zero key means 'no resume' and is never produced"
        );
    }

    #[test]
    fn the_change_hook_fires_when_a_transfer_begins_or_ends_but_not_on_progress() {
        let reg = TransferRegistry::new();
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = count.clone();
        reg.set_on_change(std::sync::Arc::new(move || {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));
        let n = || count.load(std::sync::atomic::Ordering::SeqCst);
        reg.begin(
            "A",
            TransferDirection::Sending,
            [1; 16],
            "x",
            10,
            TransferState::Waiting,
        );
        assert_eq!(n(), 1);
        reg.set_active([1; 16]);
        reg.progress([1; 16], 5);
        assert_eq!(n(), 1, "progress is not a change");
        reg.finish([1; 16], TransferState::Done, None, None);
        assert_eq!(n(), 2);
        assert!(reg.begin_incoming("B", [2; 16], "y", 1, 3, 2).is_some());
        assert_eq!(n(), 3);
        assert!(reg.begin_incoming("B", [3; 16], "z", 1, 1, 1).is_none());
        assert_eq!(n(), 3, "a refused one is not a change");
    }

    #[test]
    fn two_devices_with_one_name_are_cancelled_independently() {
        let reg = TransferRegistry::new();
        let (a_node, b_node) = (NodeId::new([1; 32]), NodeId::new([2; 32]));
        let a = reg.begin(
            "Pixel",
            TransferDirection::Sending,
            [1; 16],
            "x",
            1,
            TransferState::Active,
        );
        reg.tag_node([1; 16], a_node);
        let b = reg.begin(
            "Pixel",
            TransferDirection::Sending,
            [2; 16],
            "y",
            1,
            TransferState::Active,
        );
        reg.tag_node([2; 16], b_node);
        // An entry not tagged yet falls back to the name.
        let c = reg.begin(
            "Pixel",
            TransferDirection::Receiving,
            [3; 16],
            "z",
            1,
            TransferState::Active,
        );
        assert_eq!(reg.cancel_peer(a_node, "Pixel"), 2);
        assert!(a.is_cancelled() && c.is_cancelled() && !b.is_cancelled());
    }

    #[test]
    fn a_cancel_that_arrives_first_stops_the_commit() {
        let reg = TransferRegistry::new();
        let token = reg.begin(
            "A",
            TransferDirection::Receiving,
            [1; 16],
            "x",
            1,
            TransferState::Active,
        );
        assert!(reg.cancel([1; 16]));
        assert!(token.is_cancelled());
        assert!(
            !reg.begin_commit([1; 16]),
            "cancelled before the rename: do not commit"
        );
    }

    #[test]
    fn a_commit_that_started_cannot_be_cancelled() {
        let reg = TransferRegistry::new();
        let token = reg.begin(
            "A",
            TransferDirection::Receiving,
            [1; 16],
            "x",
            1,
            TransferState::Active,
        );
        assert!(reg.begin_commit([1; 16]));
        assert!(
            !reg.cancel([1; 16]),
            "the file is being made permanent: a cancel would be a lie"
        );
        assert!(!token.is_cancelled());
        assert_eq!(
            reg.cancel_peer(NodeId::new([9; 32]), "A"),
            0,
            "unpair leaves a committing file alone"
        );
    }

    #[tokio::test]
    async fn a_busy_rename_is_retried_a_few_times_then_given_up() {
        let mut calls = 0;
        let delays = [Duration::from_millis(1); 3];
        let r = rename_retrying(
            || {
                calls += 1;
                if calls < 3 {
                    Err(io::Error::other("busy"))
                } else {
                    Ok(())
                }
            },
            &delays,
        )
        .await;
        assert!(r.is_ok() && calls == 3);
        let mut calls = 0;
        let r = rename_retrying(
            || {
                calls += 1;
                Err(io::Error::other("busy"))
            },
            &delays,
        )
        .await;
        assert!(r.is_err());
        assert_eq!(calls, 4, "one try plus one per delay");
        let mut calls = 0;
        let r = rename_retrying(
            || {
                calls += 1;
                Err(io::Error::other("busy"))
            },
            &[],
        )
        .await;
        assert!(r.is_err() && calls == 1, "no delays means one try (Unix)");
    }

    #[test]
    fn cancel_all_stops_every_running_transfer() {
        let reg = TransferRegistry::new();
        let mut a = reg.begin(
            "A",
            TransferDirection::Sending,
            [1; 16],
            "x",
            1,
            TransferState::Waiting,
        );
        let mut b = reg.begin(
            "B",
            TransferDirection::Receiving,
            [2; 16],
            "y",
            1,
            TransferState::Active,
        );
        assert_eq!(reg.running(), 2);
        reg.cancel_all();
        assert!(a.is_cancelled() && b.is_cancelled());
        let _ = (&mut a, &mut b);
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
            send_prepared(prepared, [id; 16], 1, &mut tx, &mut rx, token, &send_reg).await
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
