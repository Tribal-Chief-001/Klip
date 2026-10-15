//! `klip tray`: the tray icon and its menu, and the start-at-login entry.
//!
//! What the tray does (poll the daemon, run the menu actions) lives in `controller`; each platform only
//! shows it: a StatusNotifierItem on D-Bus on Linux (`linux`, the `ksni` crate), the menu bar on macOS
//! (`desktop`, `tray-icon` + `tao`). The tray talks to the daemon in this process with the same client
//! the terminal commands use.
pub mod autostart;
pub mod icons;
pub mod model;
pub mod raster;

pub mod controller;
#[cfg(target_os = "macos")]
mod desktop;
#[cfg(target_os = "linux")]
mod linux;

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

pub(super) fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub(super) fn xdg(var: &str, fallback: &str) -> Option<PathBuf> {
    match std::env::var_os(var) {
        Some(p) if !p.is_empty() => Some(PathBuf::from(p)),
        _ => home().map(|h| h.join(fallback)),
    }
}

/// Where the Python tray of v0.6.0 kept its files: `~/.local/share/klip/tray`.
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
const TERMINALS: [(&str, &[&str]); 7] = [
    ("x-terminal-emulator", &["-e"]),
    ("gnome-terminal", &["--"]),
    ("konsole", &["-e"]),
    ("xfce4-terminal", &["-x"]),
    ("kitty", &[]),
    ("alacritty", &["-e"]),
    ("xterm", &["-e"]),
];

#[cfg(target_os = "linux")]
fn find_in_path(program: &str, path: &std::ffi::OsStr) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path)
        .map(|d| d.join(program))
        .find(|p| fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

/// The AppleScript that opens Terminal and runs `exe`. The path goes through a shell, so it is quoted,
/// and anything that could end the quoting is refused.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn terminal_script(exe: &str) -> Result<String, String> {
    if exe.is_empty()
        || exe
            .chars()
            .any(|c| matches!(c, '"' | '\'' | '\\' | '\n' | '\r' | '\0'))
    {
        return Err(
            "the path to klip has characters that cannot be put in a Terminal command safely"
                .to_string(),
        );
    }
    Ok(format!(
        "tell application \"Terminal\"\n  activate\n  do script \"'{exe}'\"\nend tell"
    ))
}

/// macOS: Terminal runs `klip` (the terminal app).
#[cfg(target_os = "macos")]
pub fn open_terminal(exe: &Path) -> Result<(), String> {
    let script = terminal_script(&exe.to_string_lossy())?;
    let mut child = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("cannot run osascript: {e}"))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Open the terminal app (`klip` with no arguments) in the first terminal program that exists.
#[cfg(target_os = "linux")]
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
    #[cfg(target_os = "linux")]
    {
        if let Some(old) = legacy_python_dir() {
            remove_legacy_python_files(&old);
        }
        linux::run().await
    }
    #[cfg(target_os = "macos")]
    {
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(controller::run(cmd_rx, out_tx));
        // The menu bar needs the main thread: this call blocks it for the life of the tray.
        desktop::run(cmd_tx, out_rx)
    }
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

    #[cfg(target_os = "linux")]
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

    #[test]
    fn the_terminal_script_runs_klip_quoted_and_refuses_what_could_break_out() {
        assert_eq!(
            terminal_script("/Applications/Klip Tools/klip").unwrap(),
            "tell application \"Terminal\"\n  activate\n  do script \"'/Applications/Klip Tools/klip'\"\nend tell"
        );
        for bad in [
            "/a\"b/klip",
            "/a'b/klip",
            "/a\\b/klip",
            "/a\nb/klip",
            "/a\0b/klip",
            "",
        ] {
            assert!(terminal_script(bad).is_err(), "{bad:?}");
        }
    }
}
