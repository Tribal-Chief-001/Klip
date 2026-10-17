//! Sending a folder: what the sender walks, and where the receiver puts each file.
//!
//! The sender never follows a symlink and sends regular files only. The receiver makes one new top folder
//! per send, creates the folders in between one component at a time (never through a symlink, never with `..`),
//! and puts every file in there under the usual rules (a name that is taken gets a number).

use crate::files::sanitize_file_name;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// At most this many files in one folder send, and this many folder levels below the top one.
pub const MAX_FILES: usize = 1000;
pub const MAX_DEPTH: usize = klip_net::file::MAX_FOLDER_DEPTH;
/// A send that has been quiet this long is over: its folder is forgotten (and removed when it is still empty).
const BATCH_IDLE: Duration = Duration::from_secs(3600);

/// One file of a folder to send.
#[derive(Debug, Clone)]
pub struct FolderEntry {
    pub path: PathBuf,
    /// The folders between the top one and the file (cleaned names).
    pub rel: Vec<String>,
    pub size: u64,
}

#[derive(Debug)]
pub struct Walked {
    /// The folder's own name, cleaned.
    pub top: String,
    pub entries: Vec<FolderEntry>,
    pub total: u64,
    /// Symlinks and special files that were left out.
    pub skipped: usize,
}

/// List the regular files under `root` without following any link.
pub fn walk(root: &Path) -> Result<Walked, String> {
    let raw = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut walked = Walked {
        top: sanitize_file_name(&raw),
        entries: Vec::new(),
        total: 0,
        skipped: 0,
    };
    visit(root, &mut Vec::new(), &mut walked)?;
    if walked.entries.is_empty() {
        return Err("the folder has no files to send".into());
    }
    Ok(walked)
}

fn visit(dir: &Path, rel: &mut Vec<String>, out: &mut Walked) -> Result<(), String> {
    let mut children: Vec<_> = fs::read_dir(dir)
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
        .flatten()
        .collect();
    children.sort_by_key(|c| c.file_name());
    for child in children {
        let path = child.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            out.skipped += 1;
            continue;
        };
        let kind = meta.file_type();
        if kind.is_symlink() {
            out.skipped += 1;
        } else if kind.is_dir() {
            if rel.len() + 1 > MAX_DEPTH {
                return Err(format!("the folder is deeper than {MAX_DEPTH} levels"));
            }
            rel.push(sanitize_file_name(&child.file_name().to_string_lossy()));
            visit(&path, rel, out)?;
            rel.pop();
        } else if kind.is_file() {
            if out.entries.len() >= MAX_FILES {
                return Err(format!("the folder has more than {MAX_FILES} files"));
            }
            out.total += meta.len();
            out.entries.push(FolderEntry {
                path,
                rel: rel.clone(),
                size: meta.len(),
            });
        } else {
            out.skipped += 1;
        }
    }
    Ok(())
}

/// The folder inside `top` that a file with this `rel` goes to, creating it one component at a time. A name that
/// already exists must be a plain folder: a file or a symlink there is refused, so nothing is ever created
/// through a link.
pub fn place_in_folder(top: &Path, rel: &[String]) -> io::Result<PathBuf> {
    let mut dir = top.to_path_buf();
    for part in rel {
        dir.push(sanitize_file_name(part));
        match fs::symlink_metadata(&dir) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => {
                return Err(io::Error::other(
                    "a file or a link is where the folder should be",
                ))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => fs::create_dir(&dir)?,
            Err(e) => return Err(e),
        }
    }
    Ok(dir)
}

struct Batch {
    dir: PathBuf,
    last_used: Instant,
}

type BatchKey = (Vec<u8>, [u8; 16]);

/// The receiver's memory of folder sends: `(device, batch id)` to the new top folder made for it.
#[derive(Default)]
pub struct BatchBook {
    batches: Mutex<HashMap<BatchKey, Batch>>,
}

impl BatchBook {
    pub fn dir(
        &self,
        device: &[u8],
        batch: [u8; 16],
        top: &str,
        base: &Path,
    ) -> io::Result<PathBuf> {
        self.dir_at(Instant::now(), device, batch, top, base)
    }

    pub fn dir_at(
        &self,
        now: Instant,
        device: &[u8],
        batch: [u8; 16],
        top: &str,
        base: &Path,
    ) -> io::Result<PathBuf> {
        let mut map = self.batches.lock().unwrap();
        map.retain(|_, b| {
            let idle = now.saturating_duration_since(b.last_used) > BATCH_IDLE;
            if idle {
                let _ = fs::remove_dir(&b.dir); // only succeeds while empty
            }
            !idle
        });
        let key = (device.to_vec(), batch);
        if let Some(b) = map.get_mut(&key) {
            b.last_used = now;
            return Ok(b.dir.clone());
        }
        let dir = new_top_folder(base, &sanitize_file_name(top))?;
        map.insert(
            key,
            Batch {
                dir: dir.clone(),
                last_used: now,
            },
        );
        Ok(dir)
    }

    /// A file of this send failed: when its folder is still empty it was never used, take it away.
    pub fn failed(&self, device: &[u8], batch: [u8; 16]) {
        let mut map = self.batches.lock().unwrap();
        if let Some(b) = map.get(&(device.to_vec(), batch)) {
            if fs::remove_dir(&b.dir).is_ok() {
                map.remove(&(device.to_vec(), batch));
            }
        }
    }
}

/// `base/name`, or `base/name (2)`, ... made with `create_dir` so two sends can never share one.
fn new_top_folder(base: &Path, name: &str) -> io::Result<PathBuf> {
    fs::create_dir_all(base)?;
    for n in 1..=10_000u32 {
        let candidate = if n == 1 {
            name.to_string()
        } else {
            format!("{name} ({n})")
        };
        let dir = base.join(candidate);
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "too many folders with that name",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("klip-folders-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn a_folder_is_walked_with_its_structure_sizes_and_a_clean_top_name() {
        let root = temp("walk1").join("Holiday: 2026");
        put(&root.join("a.txt"), b"12345");
        put(&root.join("Day 1/raw/b.cr2"), &[0u8; 100]);
        put(&root.join("Day 1/c.jpg"), &[1u8; 10]);
        let w = walk(&root).unwrap();
        assert_eq!(w.top, "Holiday_ 2026");
        assert_eq!(w.total, 115);
        assert_eq!(w.skipped, 0);
        let mut seen: Vec<(Vec<String>, u64)> =
            w.entries.iter().map(|e| (e.rel.clone(), e.size)).collect();
        seen.sort();
        assert_eq!(
            seen,
            vec![
                (vec![], 5),
                (vec!["Day 1".to_string()], 10),
                (vec!["Day 1".to_string(), "raw".to_string()], 100),
            ]
        );
        assert!(w
            .entries
            .iter()
            .all(|e| e.path.is_file() && e.path.starts_with(&root)));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_loops_and_special_files_are_skipped_and_counted() {
        let root = temp("walk2").join("f");
        put(&root.join("real.txt"), b"x");
        std::os::unix::fs::symlink(root.join("real.txt"), root.join("link.txt")).unwrap();
        std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();
        std::os::unix::fs::symlink("/etc", root.join("etc")).unwrap();
        let fifo = std::ffi::CString::new(root.join("pipe").to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let w = walk(&root).unwrap();
        assert_eq!(w.entries.len(), 1);
        assert_eq!(
            w.skipped, 4,
            "two links to files and folders, a loop, a fifo"
        );
    }

    #[test]
    fn limits_and_empty_folders_are_refused_with_a_reason() {
        let root = temp("walk3");
        assert!(walk(&root).unwrap_err().contains("no files"));
        for i in 0..=MAX_FILES {
            put(&root.join(format!("f{i}")), b"x");
        }
        assert!(walk(&root).unwrap_err().contains("1000"));
        let deep = temp("walk4");
        let mut p = deep.clone();
        for i in 0..=MAX_DEPTH {
            p = p.join(format!("d{i}"));
        }
        put(&p.join("x"), b"x");
        assert!(walk(&deep).unwrap_err().contains("16"));
        let ok = temp("walk5");
        let mut p = ok.clone();
        for i in 0..MAX_DEPTH {
            p = p.join(format!("d{i}"));
        }
        put(&p.join("x"), b"x");
        assert_eq!(
            walk(&ok).unwrap().entries.len(),
            1,
            "exactly 16 levels is allowed"
        );
    }

    #[test]
    fn hostile_and_reserved_names_are_cleaned_per_component() {
        let root = temp("walk6");
        put(&root.join("CON/a:b/x*.txt"), b"x");
        let w = walk(&root).unwrap();
        assert_eq!(
            w.entries[0].rel,
            vec!["_CON".to_string(), "a_b".to_string()]
        );
    }

    #[test]
    fn folders_are_created_one_component_at_a_time_inside_the_top_folder() {
        let top = temp("place1");
        let dir = place_in_folder(&top, &["Day 1".into(), "raw".into()]).unwrap();
        assert_eq!(dir, top.join("Day 1").join("raw"));
        assert!(dir.is_dir());
        assert_eq!(place_in_folder(&top, &[]).unwrap(), top);
        // The same place again is fine (several files go to it).
        assert_eq!(
            place_in_folder(&top, &["Day 1".into(), "raw".into()]).unwrap(),
            dir
        );
        // Dots and slashes cannot climb out.
        let sneaky = place_in_folder(&top, &["..".into(), "x/../../y".into()]).unwrap();
        assert!(sneaky.starts_with(&top), "{sneaky:?}");
    }

    #[test]
    fn a_file_or_a_symlink_where_a_folder_should_be_is_refused() {
        let top = temp("place2");
        put(&top.join("taken"), b"i am a file");
        assert!(place_in_folder(&top, &["taken".into()]).is_err());
        #[cfg(unix)]
        {
            let outside = temp("place2-outside");
            std::os::unix::fs::symlink(&outside, top.join("link")).unwrap();
            assert!(place_in_folder(&top, &["link".into(), "inner".into()]).is_err());
            assert!(
                !outside.join("inner").exists(),
                "nothing was created through the link"
            );
        }
    }

    #[test]
    fn one_new_top_folder_per_send_and_never_a_shared_one_between_devices() {
        let base = temp("batch1");
        let book = BatchBook::default();
        let now = Instant::now();
        let a1 = book.dir_at(now, b"Pad", [1; 16], "Trip", &base).unwrap();
        let a2 = book.dir_at(now, b"Pad", [1; 16], "Trip", &base).unwrap();
        assert_eq!(a1, a2, "every file of one send goes to the same folder");
        assert_eq!(a1, base.join("Trip"));
        let b = book.dir_at(now, b"Tab", [1; 16], "Trip", &base).unwrap();
        assert_eq!(
            b,
            base.join("Trip (2)"),
            "another device with the same batch id gets its own folder"
        );
        let again = book.dir_at(now, b"Pad", [2; 16], "Trip", &base).unwrap();
        assert_eq!(
            again,
            base.join("Trip (3)"),
            "a second send of the same folder is a new folder"
        );
    }

    #[test]
    fn an_empty_abandoned_folder_goes_when_its_send_expires_and_a_used_one_stays() {
        let base = temp("batch2");
        let book = BatchBook::default();
        let t0 = Instant::now();
        let empty = book.dir_at(t0, b"Pad", [1; 16], "Empty", &base).unwrap();
        let used = book.dir_at(t0, b"Pad", [2; 16], "Used", &base).unwrap();
        put(&used.join("f.txt"), b"x");
        // Any later request prunes what has been idle for over an hour.
        book.dir_at(
            t0 + Duration::from_secs(3601),
            b"Pad",
            [3; 16],
            "Other",
            &base,
        )
        .unwrap();
        assert!(!empty.exists(), "an empty folder nobody used is removed");
        assert!(used.exists(), "a folder with files is never removed");
    }

    #[test]
    fn a_failed_first_file_removes_the_still_empty_folder() {
        let base = temp("batch3");
        let book = BatchBook::default();
        let dir = book
            .dir_at(Instant::now(), b"Pad", [1; 16], "T", &base)
            .unwrap();
        book.failed(b"Pad", [1; 16]);
        assert!(!dir.exists());
        // The send's next file simply makes a new folder.
        assert!(book
            .dir_at(Instant::now(), b"Pad", [1; 16], "T", &base)
            .unwrap()
            .exists());
    }
}
