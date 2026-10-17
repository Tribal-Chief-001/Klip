//! `klip tray`: the tray icon and its menu, and the start-at-login entry.
//!
//! What the tray does (poll the daemon, run the menu actions) lives in `controller`; each platform only
//! shows it: a StatusNotifierItem on D-Bus on Linux (`linux`, the `ksni` crate), the menu bar on macOS
//! and the notification area on Windows (`desktop`, `tray-icon` + `tao`). The tray talks to the daemon
//! in this process with the same client the terminal commands use.
pub mod autostart;
pub mod icons;
pub mod model;
pub mod raster;

pub mod controller;
#[cfg(any(target_os = "macos", windows))]
mod desktop;
#[cfg(target_os = "linux")]
mod linux;

use std::fs::{self, File, OpenOptions};
use std::io;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
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
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        #[cfg(not(unix))]
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = dir.join("tray.lock");
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
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

/// Run a child on its own: no console window and no Ctrl-C from ours (Windows), its own process group
/// (Unix).
fn detach(cmd: &mut Command) -> &mut Command {
    #[cfg(unix)]
    {
        cmd.process_group(0)
    }
    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
        cmd.creation_flags(0x0800_0000 | 0x0000_0200)
    }
}

/// The arguments of `cmd` that start `exe` in a console window of its own. The path goes through
/// `cmd.exe`, so anything it would read as syntax is refused.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn windows_terminal_args(exe: &str) -> Result<Vec<String>, String> {
    if exe.is_empty()
        || exe.chars().any(|c| {
            matches!(
                c,
                '&' | '|' | '^' | '<' | '>' | '%' | '"' | '\n' | '\r' | '\0'
            )
        })
    {
        return Err("the path to klip has characters cmd.exe would read as commands".to_string());
    }
    Ok(["/c", "start", "Klip", exe].map(String::from).to_vec())
}

/// Windows: a console window running `klip` (the terminal app).
#[cfg(windows)]
pub fn open_terminal(exe: &Path) -> Result<(), String> {
    let args = windows_terminal_args(&exe.to_string_lossy())?;
    let out = Command::new("cmd")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .creation_flags(0x0800_0000) // the cmd.exe itself stays hidden; `start` makes the console
        .output()
        .map_err(|e| format!("cannot run cmd.exe: {e}"))?;
    match failure_text(
        "cmd",
        out.status.success(),
        &String::from_utf8_lossy(&out.stderr),
    ) {
        Some(text) => Err(text),
        None => Ok(()),
    }
}

/// A child that was started and exited non-zero: the line to show (the programme's own complaint when it gave one).
#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
pub fn failure_text(program: &str, success: bool, stderr: &str) -> Option<String> {
    if success {
        return None;
    }
    let why = stderr.lines().map(str::trim).find(|l| !l.is_empty());
    Some(match why {
        Some(why) => format!("{program}: {why}"),
        None => format!("{program} did not open the terminal"),
    })
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
    let mut command = Command::new("osascript");
    command
        .arg("-e")
        .arg(script)
        .stdin(Stdio::null())
        .stderr(Stdio::piped());
    // Wait for it: a denied "control Terminal" permission is an exit status, and the person should be told.
    let out = detach(&mut command)
        .output()
        .map_err(|e| format!("cannot run osascript: {e}"))?;
    match failure_text(
        "osascript",
        out.status.success(),
        &String::from_utf8_lossy(&out.stderr),
    ) {
        Some(text) => Err(text),
        None => Ok(()),
    }
}

/// Open the terminal app (`klip` with no arguments) in the first terminal program that exists.
#[cfg(target_os = "linux")]
pub fn open_terminal(exe: &Path) -> Result<(), String> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    for (terminal, args) in TERMINALS {
        let Some(found) = find_in_path(terminal, &path) else {
            continue;
        };
        let mut command = Command::new(found);
        command
            .args(args)
            .arg(exe)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let started = detach(&mut command).spawn();
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
    let mut command = Command::new(exe);
    command
        .args(["tray", "run"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = detach(&mut command)
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
    #[cfg(any(target_os = "macos", windows))]
    {
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(controller::run(cmd_rx, out_tx));
        // The menu bar / notification area needs the main thread: this call blocks it for the life of the tray.
        desktop::run(cmd_tx, out_rx)
    }
}

/// The programme that shows a folder in the system's file manager.
pub fn open_command(path: &Path) -> (String, Vec<String>) {
    let program = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    (
        program.to_string(),
        vec![path.to_string_lossy().into_owned()],
    )
}

/// Open `folder` in the file manager (made first when it does not exist yet: nothing has been received).
pub fn open_folder(folder: &Path) -> Result<(), String> {
    fs::create_dir_all(folder).map_err(|e| format!("cannot create {}: {e}", folder.display()))?;
    let (program, args) = open_command(folder);
    let mut command = Command::new(&program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = detach(&mut command)
        .spawn()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    std::thread::spawn(move || {
        // explorer.exe reports 1 even when it worked: only a failure to start is a problem there.
        let _ = child.wait();
    });
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Os {
    Linux,
    MacOs,
    Windows,
}

pub fn current_os() -> Os {
    if cfg!(target_os = "windows") {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::MacOs
    } else {
        Os::Linux
    }
}

/// The ASCII unit separator: zenity puts it between the chosen files, so a name with a newline in it survives.
const PICKER_SEPARATOR: char = '\u{1f}';

const MAC_PICKER: [&str; 5] = [
    "set picked to choose file with prompt \"Send files with Klip\" with multiple selections allowed",
    "set out to \"\"",
    "repeat with f in picked",
    "set out to out & POSIX path of f & linefeed",
    "end repeat",
];

const WINDOWS_PICKER: &str = "Add-Type -AssemblyName System.Windows.Forms; \
$d = New-Object System.Windows.Forms.OpenFileDialog; $d.Multiselect = $true; $d.Title = 'Send files with Klip'; \
if ($d.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { \
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; $d.FileNames | ForEach-Object { $_ } }";

/// The file dialog of a system as a programme and its arguments, when the programme exists (`has`).
/// Linux: `zenity`, else `kdialog`. macOS: `osascript`. Windows: PowerShell with the .NET dialog.
pub fn picker_command_for(os: Os, has: impl Fn(&str) -> bool) -> Option<(String, Vec<String>)> {
    let args = |list: &[&str]| list.iter().map(|a| a.to_string()).collect::<Vec<_>>();
    match os {
        Os::Linux if has("zenity") => Some((
            "zenity".into(),
            args(&[
                "--file-selection",
                "--multiple",
                &format!("--separator={PICKER_SEPARATOR}"),
                "--title=Send files with Klip",
            ]),
        )),
        Os::Linux if has("kdialog") => Some((
            "kdialog".into(),
            args(&[
                "--getopenfilename",
                "--multiple",
                "--separate-output",
                ".",
                "--title",
                "Send files with Klip",
            ]),
        )),
        Os::MacOs if has("osascript") => {
            let mut list = Vec::new();
            for line in MAC_PICKER {
                list.push("-e".to_string());
                list.push(line.to_string());
            }
            list.push("-e".to_string());
            list.push("return out".to_string());
            Some(("osascript".into(), list))
        }
        Os::Windows if has("powershell") => Some((
            "powershell".into(),
            args(&["-NoProfile", "-STA", "-Command", WINDOWS_PICKER]),
        )),
        _ => None,
    }
}

/// The absolute paths a dialog printed. Linux dialogs: separated by the unit separator (zenity) or newlines
/// (kdialog); macOS: one per line; Windows: one per line, drive paths and UNC paths. Anything else is ignored.
pub fn parse_picker_output(os: Os, text: &str) -> Vec<String> {
    let absolute = |p: &str| match os {
        Os::Windows => {
            let b = p.as_bytes();
            (b.len() > 2
                && b[0].is_ascii_alphabetic()
                && b[1] == b':'
                && (b[2] == b'\\' || b[2] == b'/'))
                || p.starts_with("\\\\")
        }
        _ => p.starts_with('/'),
    };
    let parts: Vec<&str> = if os == Os::Linux && text.contains(PICKER_SEPARATOR) {
        text.trim_end_matches('\n')
            .split(PICKER_SEPARATOR)
            .collect()
    } else {
        text.lines().collect()
    };
    parts
        .into_iter()
        .map(|p| p.trim_end_matches('\r'))
        .filter(|p| absolute(p))
        .map(str::to_string)
        .collect()
}

/// How a dialog ended: `Ok(true)` the person cancelled, `Ok(false)` they chose (or may have), `Err` it broke.
pub fn picker_outcome(
    os: Os,
    code: Option<i32>,
    stdout: &str,
    stderr: &str,
) -> Result<bool, String> {
    if code == Some(0) {
        return Ok(stdout.trim().is_empty());
    }
    let why = stderr.lines().map(str::trim).find(|l| !l.is_empty());
    match os {
        // AppleScript reports a closed dialog as error -128 ("User canceled").
        Os::MacOs if stderr.contains("-128") => Ok(true),
        Os::MacOs | Os::Windows => Err(why.unwrap_or("the file dialog failed").to_string()),
        // zenity and kdialog exit 1 with nothing printed when the dialog is closed (GTK may still warn on stderr).
        Os::Linux if code == Some(1) && stdout.trim().is_empty() => Ok(true),
        Os::Linux => Err(why.unwrap_or("the file dialog failed").to_string()),
    }
}

fn on_path(program: &str) -> bool {
    let names: Vec<String> = if cfg!(windows) {
        vec![program.to_string(), format!("{program}.exe")]
    } else {
        vec![program.to_string()]
    };
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| names.iter().any(|n| dir.join(n).is_file()))
    })
}

/// Is there a file dialog to offer "Send a file..." with on this system?
pub fn picker_available() -> bool {
    picker_command_for(current_os(), on_path).is_some()
}

/// Ask the person for files with the system's dialog. An empty list: they cancelled.
pub fn pick_files() -> Result<Vec<String>, String> {
    let os = current_os();
    let (program, args) = picker_command_for(os, on_path).ok_or("no file dialog is available")?;
    let out = Command::new(&program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if picker_outcome(os, out.status.code(), &stdout, &stderr)? {
        return Ok(Vec::new());
    }
    Ok(parse_picker_output(os, &stdout))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_failed_helper_becomes_a_line_for_the_menu_and_a_success_does_not() {
        assert_eq!(failure_text("osascript", true, "ignored"), None);
        assert_eq!(
            failure_text("osascript", false, "\n  execution error: Not authorized to send Apple events to Terminal. (-1743)\n").as_deref(),
            Some("osascript: execution error: Not authorized to send Apple events to Terminal. (-1743)")
        );
        assert_eq!(
            failure_text("cmd", false, "  \n").as_deref(),
            Some("cmd did not open the terminal")
        );
    }

    #[test]
    fn each_system_gets_its_own_file_dialog_and_none_installed_means_no_menu_entry() {
        let only = |name: &'static str| move |p: &str| p == name;
        // Linux: zenity first (with a separator no file name has), else kdialog, else nothing.
        let (program, args) = picker_command_for(Os::Linux, only("zenity")).unwrap();
        assert_eq!(program, "zenity");
        assert!(args.iter().any(|a| a == "--multiple"));
        assert!(args.iter().any(|a| a == "--separator=\u{1f}"), "{args:?}");
        let (program, args) = picker_command_for(Os::Linux, only("kdialog")).unwrap();
        assert_eq!(program, "kdialog");
        assert!(args.iter().any(|a| a == "--getopenfilename"));
        assert!(picker_command_for(Os::Linux, |_| false).is_none());
        assert_eq!(picker_command_for(Os::Linux, |_| true).unwrap().0, "zenity");
        // macOS: osascript with a script that lets the person pick several files and prints POSIX paths.
        let (program, args) = picker_command_for(Os::MacOs, only("osascript")).unwrap();
        assert_eq!(program, "osascript");
        let script = args.join("\n");
        assert!(script.contains("choose file") && script.contains("multiple selections allowed"));
        assert!(script.contains("POSIX path"));
        assert!(picker_command_for(Os::MacOs, |_| false).is_none());
        // Windows: the .NET file dialog through PowerShell (single-threaded apartment, UTF-8 output).
        let (program, args) = picker_command_for(Os::Windows, only("powershell")).unwrap();
        assert_eq!(program, "powershell");
        assert!(args.iter().any(|a| a == "-STA") && args.iter().any(|a| a == "-NoProfile"));
        let script = args.last().unwrap();
        assert!(script.contains("OpenFileDialog") && script.contains("Multiselect = $true"));
        assert!(script.contains("UTF8"));
        assert!(picker_command_for(Os::Windows, |_| false).is_none());
    }

    #[test]
    fn what_each_dialog_prints_becomes_paths_exactly() {
        // zenity: the unit separator, so a newline or a tab inside a name survives.
        let zenity = "/home/me/My Photos/a b.jpg\u{1f}/tmp/new\nline.txt\u{1f}/tmp/\u{00fc}ber.txt";
        assert_eq!(
            parse_picker_output(Os::Linux, zenity),
            vec![
                "/home/me/My Photos/a b.jpg",
                "/tmp/new\nline.txt",
                "/tmp/\u{00fc}ber.txt"
            ]
        );
        // kdialog and macOS print one path per line; anything that is not absolute is ignored.
        let lines = "/a/b c.txt\n\nrelative.txt\n/tmp/with\ttab.txt\n";
        for os in [Os::Linux, Os::MacOs] {
            assert_eq!(
                parse_picker_output(os, lines),
                vec!["/a/b c.txt", "/tmp/with\ttab.txt"]
            );
        }
        // Windows: drive paths and UNC paths, CRLF lines.
        let win =
            "C:\\Users\\me\\My Photos\\a.jpg\r\n\\\\server\\share\\b.txt\r\nrelative\\c.txt\r\n";
        assert_eq!(
            parse_picker_output(Os::Windows, win),
            vec![
                "C:\\Users\\me\\My Photos\\a.jpg",
                "\\\\server\\share\\b.txt"
            ]
        );
        assert!(parse_picker_output(Os::Windows, "").is_empty());
    }

    #[test]
    fn a_cancelled_dialog_is_an_empty_list_and_a_broken_one_is_an_error() {
        // macOS: cancel is exit 1 with error -128 on stderr.
        assert_eq!(
            picker_outcome(
                Os::MacOs,
                Some(1),
                "",
                "execution error: User canceled. (-128)"
            ),
            Ok(true)
        );
        assert!(picker_outcome(
            Os::MacOs,
            Some(1),
            "",
            "execution error: Not authorized (-1743)"
        )
        .is_err());
        assert_eq!(picker_outcome(Os::MacOs, Some(0), "/a\n", ""), Ok(false));
        // Linux dialogs exit 1 with nothing printed when cancelled, even when GTK complains on stderr.
        assert_eq!(picker_outcome(Os::Linux, Some(1), "", ""), Ok(true));
        assert_eq!(
            picker_outcome(
                Os::Linux,
                Some(1),
                "",
                "Gtk-WARNING **: Failed to load module \"appmenu-gtk-module\""
            ),
            Ok(true),
            "a warning on stderr is not an error when the person just closed the dialog"
        );
        assert_eq!(picker_outcome(Os::Linux, Some(0), "/a", ""), Ok(false));
        assert!(picker_outcome(Os::Linux, Some(255), "", "cannot open display").is_err());
        // Windows: a failed PowerShell is an error; success with no output is a cancel.
        assert!(picker_outcome(
            Os::Windows,
            Some(1),
            "",
            "The term 'Add-Type' is not recognized"
        )
        .is_err());
        assert_eq!(picker_outcome(Os::Windows, Some(0), "", ""), Ok(true));
    }

    #[test]
    fn the_folder_is_opened_with_the_programme_each_system_has() {
        let (program, args) = open_command(Path::new("/home/me/Downloads/Klip"));
        let expected = if cfg!(target_os = "windows") {
            "explorer"
        } else if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        assert_eq!(program, expected);
        assert_eq!(args, vec!["/home/me/Downloads/Klip".to_string()]);
    }

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

    #[test]
    fn the_windows_terminal_command_starts_klip_in_its_own_console_and_refuses_cmd_metacharacters()
    {
        assert_eq!(
            windows_terminal_args(r"C:\Program Files\Klip\klip.exe").unwrap(),
            ["/c", "start", "Klip", r"C:\Program Files\Klip\klip.exe"]
        );
        for bad in [
            "a&b.exe", "a|b.exe", "a^b.exe", "a<b.exe", "a>b.exe", "a%b.exe", "a\"b.exe",
            "a\nb.exe", "",
        ] {
            assert!(windows_terminal_args(bad).is_err(), "{bad:?}");
        }
    }
}
