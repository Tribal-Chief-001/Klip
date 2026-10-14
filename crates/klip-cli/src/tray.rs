//! `klip tray`: start the tray app (a Python/GTK program that ships inside this binary), and set it up
//! to start at login.
//!
//! The Python files are embedded; they are written to `$XDG_DATA_HOME/klip/tray/<klip version>/` the
//! first time (and again if one was changed), then run with `python3 -m klip_tray`.
use std::fs;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Every module of the Python package `klip_tray`.
pub const FILES: &[(&str, &str)] = &[
    (
        "__init__.py",
        include_str!("../../../extra/tray/klip_tray/__init__.py"),
    ),
    (
        "__main__.py",
        include_str!("../../../extra/tray/klip_tray/__main__.py"),
    ),
    (
        "app.py",
        include_str!("../../../extra/tray/klip_tray/app.py"),
    ),
    (
        "bridge.py",
        include_str!("../../../extra/tray/klip_tray/bridge.py"),
    ),
    (
        "controller.py",
        include_str!("../../../extra/tray/klip_tray/controller.py"),
    ),
    (
        "icons.py",
        include_str!("../../../extra/tray/klip_tray/icons.py"),
    ),
    (
        "model.py",
        include_str!("../../../extra/tray/klip_tray/model.py"),
    ),
    (
        "pages.py",
        include_str!("../../../extra/tray/klip_tray/pages.py"),
    ),
    (
        "pages_devices.py",
        include_str!("../../../extra/tray/klip_tray/pages_devices.py"),
    ),
    (
        "pages_history.py",
        include_str!("../../../extra/tray/klip_tray/pages_history.py"),
    ),
    (
        "pages_more.py",
        include_str!("../../../extra/tray/klip_tray/pages_more.py"),
    ),
    (
        "theme.py",
        include_str!("../../../extra/tray/klip_tray/theme.py"),
    ),
    (
        "widgets.py",
        include_str!("../../../extra/tray/klip_tray/widgets.py"),
    ),
    (
        "window.py",
        include_str!("../../../extra/tray/klip_tray/window.py"),
    ),
];

const MARKER: &str = "# Created by `klip tray install`; `klip tray uninstall` removes it.";
const CHECK_IMPORTS: &str =
    "import gi; gi.require_version('Gtk','3.0'); gi.require_version('Gdk','3.0'); \
gi.require_version('XApp','1.0'); from gi.repository import Gtk, Gdk, XApp";

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

/// Where the Python files are kept: `~/.local/share/klip/tray`.
pub fn data_root() -> Option<PathBuf> {
    xdg("XDG_DATA_HOME", ".local/share").map(|d| d.join("klip/tray"))
}

/// The start-at-login entry: `~/.config/autostart/klip-tray.desktop`.
pub fn autostart_path() -> Option<PathBuf> {
    xdg("XDG_CONFIG_HOME", ".config").map(|d| d.join("autostart/klip-tray.desktop"))
}

/// Write the embedded package to `<root>/<version>/klip_tray/`, touching only files that differ.
/// Returns the directory to put on `PYTHONPATH`.
pub fn install_files(root: &Path, version: &str) -> io::Result<PathBuf> {
    let base = root.join(version);
    let package = base.join("klip_tray");
    fs::create_dir_all(&package)?;
    for (name, content) in FILES {
        let path = package.join(name);
        if fs::read_to_string(&path).is_ok_and(|c| c == *content) {
            continue;
        }
        // Two first starts at once must not share a half-written file.
        let tmp = package.join(format!(".{name}.{}.tmp", std::process::id()));
        fs::write(&tmp, content)?;
        fs::rename(&tmp, &path)?;
    }
    Ok(base)
}

/// Is Python with GTK 3 and XApp available? The error says what to install.
pub fn python_ready() -> Result<(), String> {
    let out = Command::new("python3")
        .args(["-c", CHECK_IMPORTS])
        .stdin(Stdio::null())
        .output()
        .map_err(|_| {
            "python3 was not found. Install it (and python3-gi) to use the tray.".to_string()
        })?;
    if out.status.success() {
        return Ok(());
    }
    Err("The tray needs Python with GTK 3 and XApp (PyGObject). Linux Mint has them; on Debian or Ubuntu: \
         sudo apt install python3-gi gir1.2-gtk-3.0 gir1.2-xapp-1.0"
        .to_string())
}

fn python_command(show: bool) -> Result<Command, String> {
    python_ready()?;
    let root = data_root().ok_or("cannot find your home directory")?;
    let dir = install_files(&root, env!("CARGO_PKG_VERSION"))
        .map_err(|e| format!("cannot write the tray files: {e}"))?;
    let exe = exe_path().map_err(|e| e.to_string())?;
    let mut cmd = Command::new("python3");
    cmd.args(["-m", "klip_tray"]);
    if show {
        cmd.arg("--show");
    }
    let path = match std::env::var_os("PYTHONPATH") {
        Some(old) if !old.is_empty() => {
            let mut p = dir.into_os_string();
            p.push(":");
            p.push(old);
            p
        }
        _ => dir.into_os_string(),
    };
    cmd.env("PYTHONPATH", path)
        .env("KLIP_BIN", exe)
        .env("KLIP_VERSION", env!("CARGO_PKG_VERSION"))
        .stdin(Stdio::null());
    Ok(cmd)
}

/// Replace this process with the tray (what the start-at-login entry runs).
pub fn run_foreground(show: bool) -> Result<(), String> {
    let mut cmd = python_command(show)?;
    Err(format!("cannot start python3: {}", cmd.exec()))
}

/// Start the tray in the background and return its process id.
pub fn start_detached(show: bool) -> Result<u32, String> {
    let mut cmd = python_command(show)?;
    cmd.stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    cmd.spawn()
        .map(|c| c.id())
        .map_err(|e| format!("cannot start python3: {e}"))
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
    if let Ok(existing) = fs::read_to_string(path) {
        if !existing.starts_with(MARKER) {
            return Err(format!(
                "{} already exists and was not made by klip; not touching it",
                path.display()
            ));
        }
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    fs::write(path, entry).map_err(|e| format!("{}: {e}", path.display()))
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
    fn every_python_module_of_the_package_is_embedded() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extra/tray/klip_tray");
        let mut on_disk: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".py"))
            .collect();
        on_disk.sort();
        let mut embedded: Vec<String> = FILES.iter().map(|(n, _)| n.to_string()).collect();
        embedded.sort();
        assert_eq!(
            embedded, on_disk,
            "a module is missing from (or left in) FILES"
        );
        for (name, content) in FILES {
            assert_eq!(
                fs::read_to_string(dir.join(name)).unwrap(),
                *content,
                "{name} is stale"
            );
        }
    }

    #[test]
    fn files_are_written_per_version_left_alone_when_equal_and_repaired_when_changed() {
        let root = temp("files");
        let base = install_files(&root, "9.9.9").unwrap();
        assert_eq!(base, root.join("9.9.9"));
        let model = base.join("klip_tray/model.py");
        assert!(model.exists());
        let before = fs::metadata(&model).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        install_files(&root, "9.9.9").unwrap();
        assert_eq!(
            fs::metadata(&model).unwrap().modified().unwrap(),
            before,
            "unchanged files are not rewritten"
        );
        fs::write(&model, "tampered").unwrap();
        install_files(&root, "9.9.9").unwrap();
        assert_eq!(
            fs::read_to_string(&model).unwrap(),
            FILES.iter().find(|(n, _)| *n == "model.py").unwrap().1
        );
        let other = install_files(&root, "9.9.10").unwrap();
        assert!(other.join("klip_tray/app.py").exists() && base.join("klip_tray/app.py").exists());
        assert!(fs::read_dir(base.join("klip_tray")).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));
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
    fn the_embedded_logic_modules_import_with_a_plain_python() {
        if Command::new("python3").arg("--version").output().is_err() {
            return;
        }
        let root = temp("import");
        let base = install_files(&root, "0.0.0").unwrap();
        let out = Command::new("python3")
            .args([
                "-c",
                "import klip_tray.model, klip_tray.bridge, klip_tray.controller, klip_tray.icons",
            ])
            .env("PYTHONPATH", &base)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
