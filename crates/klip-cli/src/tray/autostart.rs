//! The start-at-login entry of the tray: one file (or registry value) per OS, written atomically, and
//! never overwriting or deleting one that Klip did not make.
use super::xdg;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const MARKER: &str = "# Created by `klip tray install`; `klip tray uninstall` removes it.";
const MARKER_TEXT: &str = "Created by `klip tray install`; `klip tray uninstall` removes it.";

/// Did Klip write this entry? (a foreign file of the same name is never touched)
fn ours(text: &str) -> bool {
    text.contains(MARKER_TEXT)
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

/// Write `text` to `path` atomically, unless a file that Klip did not make is there.
fn write_entry(path: &Path, text: &str) -> Result<(), String> {
    match fs::read_to_string(path) {
        Ok(existing) if ours(&existing) => {}
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
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    fs::write(&tmp, text)
        .and_then(|()| fs::rename(&tmp, path))
        .map_err(|e| format!("{}: {e}", path.display()))
}

pub fn install_autostart(path: &Path, exe: &str) -> Result<(), String> {
    write_entry(path, &desktop_entry(exe)?)
}

/// Remove the entry if klip made it. `Ok(false)`: there was nothing to remove.
pub fn uninstall_autostart(path: &Path) -> Result<bool, String> {
    match fs::read_to_string(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("{}: {e}", path.display())),
        Ok(text) if !ours(&text) => Err(format!(
            "{} was not made by klip; not removing it",
            path.display()
        )),
        Ok(_) => fs::remove_file(path)
            .map(|()| true)
            .map_err(|e| format!("{}: {e}", path.display())),
    }
}

pub fn autostart_installed(path: &Path) -> bool {
    fs::read_to_string(path).is_ok_and(|t| ours(&t))
}

fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// The macOS launch agent: runs `klip tray run` at login in the graphical session. No KeepAlive: a
/// tray that was quit stays quit until the next login.
pub fn launch_agent_plist(exe: &str) -> Result<String, String> {
    if exe.is_empty() || exe.chars().any(|c| matches!(c, '\n' | '\r' | '\0')) {
        return Err("the path to klip has characters a launch agent cannot hold".to_string());
    }
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!-- {MARKER_TEXT} -->\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n  <key>Label</key><string>com.klip.tray</string>\n  \
         <key>ProgramArguments</key><array><string>{}</string><string>tray</string><string>run</string></array>\n  \
         <key>RunAtLoad</key><true/>\n  <key>LimitLoadToSessionType</key><string>Aqua</string>\n  \
         <key>ProcessType</key><string>Interactive</string>\n</dict>\n</plist>\n",
        xml_escape(exe)
    ))
}

/// The Windows `Run` value: `"C:\path\klip.exe" tray start` (start returns at once and leaves the tray
/// without a console window). Quotes and `%` are refused: the shell would read them.
pub fn run_key_value(exe: &str) -> Result<String, String> {
    if exe.is_empty()
        || exe
            .chars()
            .any(|c| matches!(c, '"' | '%' | '\n' | '\r' | '\0'))
    {
        return Err(
            "the path to klip has characters the Windows Run key cannot hold safely; put klip in a plainer path"
                .to_string(),
        );
    }
    Ok(format!("\"{exe}\" tray start"))
}

/// Where this OS keeps the login entry, for messages.
#[cfg(target_os = "macos")]
fn launch_agent_path() -> Option<PathBuf> {
    super::home().map(|h| h.join("Library/LaunchAgents/com.klip.tray.plist"))
}

#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

#[cfg(windows)]
fn reg(args: &[&str]) -> Result<std::process::Output, String> {
    std::process::Command::new("reg")
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("cannot run reg.exe: {e}"))
}

/// Register the tray to start at login; returns where (for the message).
pub fn install(exe: &str) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let path = launch_agent_path().ok_or("cannot find your home directory")?;
        write_entry(&path, &launch_agent_plist(exe)?)?;
        return Ok(path.display().to_string());
    }
    #[cfg(windows)]
    {
        let value = run_key_value(exe)?;
        let out = reg(&[
            "add", RUN_KEY, "/v", "KlipTray", "/t", "REG_SZ", "/d", &value, "/f",
        ])?;
        return if out.status.success() {
            Ok(format!("{RUN_KEY}\\KlipTray"))
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        };
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let path = autostart_path().ok_or("cannot find your home directory")?;
        install_autostart(&path, exe)?;
        Ok(path.display().to_string())
    }
}

/// Stop starting the tray at login. `Ok(false)`: there was nothing to remove.
pub fn uninstall() -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        let path = launch_agent_path().ok_or("cannot find your home directory")?;
        return uninstall_autostart(&path);
    }
    #[cfg(windows)]
    {
        if !installed() {
            return Ok(false);
        }
        let out = reg(&["delete", RUN_KEY, "/v", "KlipTray", "/f"])?;
        return if out.status.success() {
            Ok(true)
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        };
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let path = autostart_path().ok_or("cannot find your home directory")?;
        uninstall_autostart(&path)
    }
}

pub fn installed() -> bool {
    #[cfg(target_os = "macos")]
    {
        return launch_agent_path().is_some_and(|p| autostart_installed(&p));
    }
    #[cfg(windows)]
    {
        return reg(&["query", RUN_KEY, "/v", "KlipTray"]).is_ok_and(|o| o.status.success());
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        autostart_path().is_some_and(|p| autostart_installed(&p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tray::clean_exe;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("klip-autostart-{}-{tag}", std::process::id()));
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
    fn the_macos_launch_agent_runs_the_tray_at_login_and_is_marked_as_ours() {
        let plist = launch_agent_plist("/Applications/Klip/klip").unwrap();
        assert!(plist.starts_with("<?xml"), "{plist}");
        for want in [
            "<key>Label</key><string>com.klip.tray</string>",
            "<string>/Applications/Klip/klip</string><string>tray</string><string>run</string>",
            "<key>RunAtLoad</key><true/>",
            "<key>LimitLoadToSessionType</key><string>Aqua</string>",
        ] {
            assert!(plist.contains(want), "{want} in {plist}");
        }
        assert!(
            !plist.contains("KeepAlive"),
            "quitting the tray is final until the next login"
        );
        assert!(ours(&plist));
    }

    #[test]
    fn xml_in_a_path_cannot_break_out_of_the_launch_agent() {
        let plist = launch_agent_plist("/Users/a&b/<x>\"y'/klip").unwrap();
        assert!(
            plist.contains("/Users/a&amp;b/&lt;x&gt;&quot;y&apos;/klip"),
            "{plist}"
        );
        assert!(!plist.contains("<x>"));
        for bad in ["/a\nb/klip", "/a\rb/klip", "/a\0b/klip", ""] {
            assert!(launch_agent_plist(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_windows_run_value_quotes_the_program_and_refuses_what_cmd_would_expand() {
        assert_eq!(
            run_key_value(r"C:\Program Files\Klip\klip.exe").unwrap(),
            r#""C:\Program Files\Klip\klip.exe" tray start"#
        );
        for bad in [r#"C:\a"b\klip.exe"#, r"C:\%PATH%\klip.exe", "C:\\a\nb", ""] {
            assert!(run_key_value(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn only_text_with_the_klip_marker_counts_as_ours() {
        assert!(ours(&desktop_entry("/usr/bin/klip").unwrap()));
        assert!(!ours("[Desktop Entry]\nName=Somebody else\n"));
        assert!(!ours(""));
    }
}
