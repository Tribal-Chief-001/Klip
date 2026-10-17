//! Receiving and sending files, the parts that touch the disk: safe names, unique paths that never
//! replace anything, free space, and partial files that become permanent only when complete.
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
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
}
