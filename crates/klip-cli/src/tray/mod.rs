//! `klip tray`: the tray icon and its menu (Linux), and the start-at-login entry.
//!
//! The icon is a StatusNotifierItem on D-Bus (the `ksni` crate): the panel draws it and its menu. The
//! tray talks to the daemon in this process with the same client the terminal commands use.
pub mod icons;
pub mod model;

pub mod controller;
mod linux;

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const MARKER: &str = "# Created by `klip tray install`; `klip tray uninstall` removes it.";

/// `current_exe` says "/path/klip (deleted)" after the file was replaced while it runs (an upgrade):
/// the program is at the path without the note.
pub fn clean_exe(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_suffix(" (deleted)") {
        Some(plain) => PathBuf::from(plain),
        None => path.to_path_buf(),
    }
}

pub fn exe_path() -> io::Result<PathBuf> {
    std::env::current_exe().map(|p| clean_exe(&p))
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn xdg(var: &str, fallback: &str) -> Option<PathBuf> {
    match std::env::var_os(var) {
        Some(p) if !p.is_empty() => Some(PathBuf::from(p)),
        _ => home().map(|h| h.join(fallback)),
    }
}

/// The start-at-login entry: `~/.config/autostart/klip-tray.desktop`.
pub fn autostart_path() -> Option<PathBuf> {
    xdg("XDG_CONFIG_HOME", ".config").map(|d| d.join("autostart/klip-tray.desktop"))
}

/// The text of the start-at-login entry for the klip program at `exe`.
pub fn desktop_entry(exe: &str) -> Result<String, String> {
    if exe
        .chars()
        .any(|c| matches!(c, '"' | '`' | '$' | '\\' | '\n' | '\r' | '%'))
    {
        return Err(
            "the path to klip has characters a desktop entry cannot hold safely; \
                    put klip in a plainer path"
                .to_string(),
        );
    }
    let program = if exe
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._+-".contains(c))
    {
        exe.to_string()
    } else {
        format!("\"{exe}\"")
    };
    Ok(format!(
        "{MARKER}\n[Desktop Entry]\nType=Application\nName=Klip tray\nComment=Clipboard sync between your \
         devices: tray icon\nExec={program} tray run\nTerminal=false\nX-GNOME-Autostart-enabled=true\n\
         X-GNOME-Autostart-Delay=3\nNoDisplay=true\n"
    ))
}

pub fn install_autostart(path: &Path, exe: &str) -> Result<(), String> {
    let entry = desktop_entry(exe)?;
    match fs::read_to_string(path) {
        Ok(existing) if existing.starts_with(MARKER) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        _ => {
            return Err(format!(
                "{} already exists and was not made by klip; not touching it",
                path.display()
            ))
        }
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension(format!("desktop.{}.tmp", std::process::id()));
    fs::write(&tmp, entry)
        .and_then(|()| fs::rename(&tmp, path))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Remove the entry if klip made it. `Ok(false)`: there was nothing to remove.
pub fn uninstall_autostart(path: &Path) -> Result<bool, String> {
    match fs::read_to_string(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("{}: {e}", path.display())),
        Ok(text) if !text.starts_with(MARKER) => Err(format!(
            "{} was not made by klip; not removing it",
            path.display()
        )),
        Ok(_) => fs::remove_file(path)
            .map(|()| true)
            .map_err(|e| format!("{}: {e}", path.display())),
    }
}

pub fn autostart_installed(path: &Path) -> bool {
    fs::read_to_string(path).is_ok_and(|t| t.starts_with(MARKER))
}

/// Where the Python tray of v0.6.0 kept its files: `~/.local/share/klip/tray`.
fn legacy_python_dir() -> Option<PathBuf> {
    xdg("XDG_DATA_HOME", ".local/share").map(|d| d.join("klip/tray"))
}

/// Remove what the old Python tray left behind, but only a folder that looks exactly like it
/// (`<version>/klip_tray/` inside); anything else there is not ours to delete.
pub fn remove_legacy_python_files(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut found = 0;
    for e in entries.flatten() {
        if !e.path().join("klip_tray").is_dir() {
            return;
        }
        found += 1;
    }
    if found > 0 {
        let _ = fs::remove_dir_all(dir);
    }
}

/// Keeps a second tray from starting: a lock on `tray.lock` in the Klip directory, held for the life
/// of the process and released by the OS when it exits, even after a crash.
pub struct TrayLock {
    _file: File,
}

impl TrayLock {
    /// `Ok(None)`: another tray holds it.
    pub fn acquire(dir: &Path) -> Result<Option<Self>, String> {
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let path = dir.join("tray.lock");
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create(true).truncate(false);
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = opts
            .open(&path)
            .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(e)) => {
                Err(format!("cannot lock {}: {e}", path.display()))
            }
        }
    }
}

fn klip_dir() -> PathBuf {
    klip_daemon::config::user_paths().dir().to_path_buf()
}

/// Is a tray running for this configuration?
pub fn is_running() -> bool {
    matches!(TrayLock::acquire(&klip_dir()), Ok(None))
}

/// The terminals tried to show the terminal app, with the flag that makes them run a command.
const TERMINALS: [(&str, &[&str]); 7] = [
    ("x-terminal-emulator", &["-e"]),
    ("gnome-terminal", &["--"]),
    ("konsole", &["-e"]),
    ("xfce4-terminal", &["-x"]),
    ("kitty", &[]),
    ("alacritty", &["-e"]),
    ("xterm", &["-e"]),
];

fn find_in_path(program: &str, path: &std::ffi::OsStr) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path)
        .map(|d| d.join(program))
        .find(|p| fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

/// Open the terminal app (`klip` with no arguments) in the first terminal program that exists.
pub fn open_terminal(exe: &Path) -> Result<(), String> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    for (terminal, args) in TERMINALS {
        let Some(found) = find_in_path(terminal, &path) else {
            continue;
        };
        let started = Command::new(found)
            .args(args)
            .arg(exe)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn();
        if let Ok(mut child) = started {
            // Collect its exit status when the terminal is closed (no zombie in a long-lived tray).
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Ok(());
        }
    }
    Err("No terminal program found. Run klip in a terminal.".to_string())
}

/// Start the tray in the background and return its process id.
pub fn start_detached() -> Result<u32, String> {
    if is_running() {
        return Err("The tray is already running.".to_string());
    }
    let exe = exe_path().map_err(|e| e.to_string())?;
    let mut child = Command::new(exe)
        .args(["tray", "run"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("cannot start the tray: {e}"))?;
    // Say "started" only when it holds the lock; a tray that dies at once (no desktop session) is an error.
    for _ in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if is_running() {
            return Ok(child.id());
        }
        if child.try_wait().ok().flatten().is_some() {
            break;
        }
    }
    Err("The tray stopped right after starting. Run `klip tray run` to see why.".to_string())
}

/// Run the tray in this process until it is quit (what the start-at-login entry runs).
pub async fn run() -> Result<(), String> {
    let dir = klip_dir();
    let Some(_lock) = TrayLock::acquire(&dir)? else {
        return Err("The tray is already running.".to_string());
    };
    if let Some(old) = legacy_python_dir() {
        remove_legacy_python_files(&old);
    }
    linux::run().await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("klip-tray-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_start_at_login_entry_runs_the_tray_and_quotes_awkward_paths() {
        let plain = desktop_entry("/home/me/.local/bin/klip").unwrap();
        assert!(plain.starts_with(MARKER));
        assert!(
            plain.contains("Exec=/home/me/.local/bin/klip tray run\n"),
            "{plain}"
        );
        assert!(plain.contains("X-GNOME-Autostart-enabled=true"));
        let spaced = desktop_entry("/home/me/My Tools/klip").unwrap();
        assert!(
            spaced.contains("Exec=\"/home/me/My Tools/klip\" tray run\n"),
            "{spaced}"
        );
        for bad in [
            "/a\"b/klip",
            "/a$b/klip",
            "/a`b/klip",
            "/a\\b/klip",
            "/a\nb/klip",
        ] {
            assert!(desktop_entry(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_replaced_binary_is_not_written_into_the_entry_as_deleted() {
        assert_eq!(
            clean_exe(Path::new("/usr/bin/klip (deleted)")),
            PathBuf::from("/usr/bin/klip")
        );
        assert_eq!(
            clean_exe(Path::new("/usr/bin/klip")),
            PathBuf::from("/usr/bin/klip")
        );
    }

    #[test]
    fn start_at_login_installs_and_removes_only_a_file_klip_made() {
        let dir = temp("autostart");
        let path = dir.join("autostart/klip-tray.desktop");
        assert!(!autostart_installed(&path));
        assert_eq!(uninstall_autostart(&path), Ok(false));
        install_autostart(&path, "/usr/bin/klip").unwrap();
        assert!(autostart_installed(&path));
        install_autostart(&path, "/opt/klip/klip").unwrap(); // reinstalling updates it
        assert!(fs::read_to_string(&path)
            .unwrap()
            .contains("/opt/klip/klip"));
        assert_eq!(uninstall_autostart(&path), Ok(true));
        assert!(!path.exists());

        fs::write(&path, "[Desktop Entry]\nName=Somebody else's\n").unwrap();
        assert!(install_autostart(&path, "/usr/bin/klip").is_err());
        assert!(uninstall_autostart(&path).is_err());
        assert!(
            fs::read_to_string(&path).unwrap().contains("Somebody else"),
            "a foreign file is never touched"
        );
    }

    #[test]
    fn only_a_folder_that_looks_like_the_python_tray_is_removed() {
        let ours = temp("legacy");
        fs::create_dir_all(ours.join("0.6.0/klip_tray")).unwrap();
        fs::write(ours.join("0.6.0/klip_tray/app.py"), "x").unwrap();
        remove_legacy_python_files(&ours);
        assert!(!ours.exists(), "the old Python files are gone");

        let foreign = temp("foreign");
        fs::create_dir_all(foreign.join("0.6.0/klip_tray")).unwrap();
        fs::write(foreign.join("notes.txt"), "mine").unwrap();
        remove_legacy_python_files(&foreign);
        assert!(
            foreign.join("notes.txt").exists(),
            "a folder with anything else is left alone"
        );

        remove_legacy_python_files(&temp("empty"));
        remove_legacy_python_files(Path::new("/nonexistent/klip"));
    }

    #[test]
    fn a_second_tray_is_refused_until_the_first_one_exits() {
        let dir = temp("lock");
        let first = TrayLock::acquire(&dir).unwrap().expect("first");
        assert!(TrayLock::acquire(&dir).unwrap().is_none());
        drop(first);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while TrayLock::acquire(&dir).unwrap().is_none() {
            assert!(std::time::Instant::now() < deadline, "lock not free again");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn a_terminal_is_found_by_name_on_the_path_and_only_if_executable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("path");
        let exe = dir.join("kitty");
        fs::write(&exe, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(dir.join("xterm"), "not executable").unwrap();
        let path = std::env::join_paths([dir.clone()]).unwrap();
        assert_eq!(find_in_path("kitty", &path), Some(exe));
        assert_eq!(find_in_path("xterm", &path), None);
        assert_eq!(find_in_path("konsole", &path), None);
    }
}
