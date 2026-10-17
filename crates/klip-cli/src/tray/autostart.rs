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

/// Which login entry to write: the quiet one (a script run by `wscript.exe`, which opens no console window) when the
/// Windows Script Host is there, else the plain `klip tray start` one (which may flash a console window once).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginForm {
    Script,
    Plain,
}

pub fn login_entry_form(has_script_host: bool) -> LoginForm {
    if has_script_host {
        LoginForm::Script
    } else {
        LoginForm::Plain
    }
}

fn safe_for_run_value(text: &str) -> bool {
    !text.is_empty()
        && !text
            .chars()
            .any(|c| matches!(c, '"' | '%' | '\n' | '\r' | '\0'))
}

/// `wscript.exe //B //nologo "<script>"`: batch mode, no banner, no console.
pub fn run_value_script(vbs_path: &str) -> Result<String, String> {
    if !safe_for_run_value(vbs_path) {
        return Err(
            "the path of the login script has characters the Windows Run key cannot hold safely"
                .into(),
        );
    }
    Ok(format!("wscript.exe //B //nologo \"{vbs_path}\""))
}

/// The script: start the tray with its window hidden (`Run cmd, 0, False`). The path is quoted twice for VBScript.
pub fn vbs_text(exe: &str) -> Result<String, String> {
    if !safe_for_run_value(exe) {
        return Err("the path to klip has characters the login script cannot hold safely; put klip in a plainer path".into());
    }
    Ok(format!(
        "CreateObject(\"WScript.Shell\").Run \"\"\"{exe}\"\" tray run\", 0, False\r\n"
    ))
}

/// The script as the Windows Script Host reads it reliably: UTF-16 little endian with a byte order mark (a plain
/// UTF-8 file would be read in the local code page and break a path with an accented letter).
pub fn vbs_bytes(text: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

/// The login script lives in a folder of Klip's own: `...\Klip\start-tray.vbs`.
fn vbs_path_is_klips(path: &str) -> bool {
    let mut parts = path.rsplit(['\\', '/']);
    parts
        .next()
        .is_some_and(|f| f.eq_ignore_ascii_case("start-tray.vbs"))
        && parts.next().is_some_and(|d| d.eq_ignore_ascii_case("Klip"))
}

/// Is this `Run` value Klip's own (`"<path>\\klip.exe" tray start`)? Only then may it be replaced or removed:
/// another programme could have used the name `KlipTray`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn run_value_is_klips(value: &str) -> bool {
    let value = value.trim();
    if let Some(script) = value
        .strip_prefix("wscript.exe //B //nologo \"")
        .and_then(|r| r.strip_suffix('"'))
    {
        return vbs_path_is_klips(script);
    }
    let Some(rest) = value.strip_suffix(" tray start") else {
        return false;
    };
    let Some(path) = rest.strip_prefix('"').and_then(|r| r.strip_suffix('"')) else {
        return false;
    };
    let file = path.rsplit(['\\', '/']).next().unwrap_or("").to_lowercase();
    // `klip`, `klip.exe`, and the release file names (`klip-windows-x86_64.exe`), but not `klipper.exe`.
    let stem = file.strip_suffix(".exe").unwrap_or(&file);
    stem == "klip" || stem.starts_with("klip-")
}

/// The data of a `REG_SZ` value in the output of `reg query <key> /v <name>`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_reg_sz(output: &str, name: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let line = line.trim();
        let rest = line.strip_prefix(name)?.trim_start();
        let data = rest.strip_prefix("REG_SZ")?;
        Some(data.trim().to_string())
    })
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

/// Is the Windows Script Host there and the VBScript engine registered? (It is an optional Windows feature.)
#[cfg(windows)]
fn script_host_available() -> bool {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    Path::new(&root)
        .join("System32")
        .join("wscript.exe")
        .is_file()
        && reg(&["query", r"HKCR\VBScript"]).is_ok_and(|o| o.status.success())
}

#[cfg(windows)]
fn login_script_path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Klip").join("start-tray.vbs"))
}

/// What the `KlipTray` Run value holds now, if it exists.
#[cfg(windows)]
fn current_run_value() -> Option<String> {
    let out = reg(&["query", RUN_KEY, "/v", "KlipTray"]).ok()?;
    if !out.status.success() {
        return None;
    }
    parse_reg_sz(&String::from_utf8_lossy(&out.stdout), "KlipTray")
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
        // The quiet form (a script run by wscript, no console window) when the script host exists, else the plain one.
        // Nothing is written until the existing entry (if any) is known to be Klip's own.
        let (value, script) = match (
            login_entry_form(script_host_available()),
            login_script_path(),
        ) {
            (LoginForm::Script, Some(vbs)) => {
                let text = vbs_text(exe)?;
                (run_value_script(&vbs.to_string_lossy())?, Some((vbs, text)))
            }
            _ => (run_key_value(exe)?, None),
        };
        if let Some(existing) = current_run_value() {
            if !run_value_is_klips(&existing) {
                return Err(format!(
                    "the Run entry KlipTray already belongs to something else ({existing}); not replacing it"
                ));
            }
        }
        if let Some((vbs, text)) = script {
            if let Some(dir) = vbs.parent() {
                fs::create_dir_all(dir)
                    .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            }
            fs::write(&vbs, vbs_bytes(&text))
                .map_err(|e| format!("cannot write {}: {e}", vbs.display()))?;
        }
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
        let Some(existing) = current_run_value() else {
            return Ok(false);
        };
        if !run_value_is_klips(&existing) {
            return Err(format!(
                "the Run entry KlipTray belongs to something else ({existing}); not removing it"
            ));
        }
        let out = reg(&["delete", RUN_KEY, "/v", "KlipTray", "/f"])?;
        if let Some(vbs) = login_script_path() {
            let _ = fs::remove_file(vbs); // the script only ever held Klip's own start line
        }
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

    #[test]
    fn only_a_run_value_that_starts_klips_tray_is_klips() {
        assert!(run_value_is_klips(
            r#""C:\Users\me\bin\klip.exe" tray start"#
        ));
        assert!(run_value_is_klips(r#""/usr/local/bin/klip" tray start"#));
        assert!(run_value_is_klips("  \"C:\\Klip\\KLIP.EXE\" tray start  "));
        assert!(run_value_is_klips(
            r#""C:\Users\me\Downloads\klip-windows-x86_64.exe" tray start"#
        ));
        for foreign in [
            r#""C:\Tools\other.exe" tray start"#,
            r#""C:\Users\me\klip.exe" tray run"#,
            r#"C:\Users\me\klip.exe tray start"#,
            r#""C:\x\klip.exe.bak" tray start"#,
            r#""C:\x\klipper.exe" tray start"#,
            "",
        ] {
            assert!(!run_value_is_klips(foreign), "{foreign}");
        }
    }

    #[test]
    fn the_quiet_login_entry_needs_the_script_host_else_the_plain_one() {
        assert_eq!(login_entry_form(true), LoginForm::Script);
        assert_eq!(login_entry_form(false), LoginForm::Plain);
    }

    #[test]
    fn the_script_run_value_and_the_script_are_built_safely() {
        let vbs = r"C:\Users\me\AppData\Local\Klip\start-tray.vbs";
        assert_eq!(
            run_value_script(vbs).unwrap(),
            r#"wscript.exe //B //nologo "C:\Users\me\AppData\Local\Klip\start-tray.vbs""#
        );
        for bad in [
            "",
            "C:\\a\"b\\start-tray.vbs",
            "C:\\%x%\\start-tray.vbs",
            "C:\\a\nb\\start-tray.vbs",
        ] {
            assert!(run_value_script(bad).is_err(), "{bad:?}");
        }
        let text = vbs_text(r"C:\Program Files\Klip\klip.exe").unwrap();
        assert_eq!(
            text,
            "CreateObject(\"WScript.Shell\").Run \"\"\"C:\\Program Files\\Klip\\klip.exe\"\" tray run\", 0, False\r\n"
        );
        for bad in [
            "",
            "C:\\a\"b\\klip.exe",
            "C:\\100%\\klip.exe",
            "C:\\a\r\nb\\klip.exe",
            "C:\\a\0b",
        ] {
            assert!(vbs_text(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_login_script_is_utf_16_with_a_byte_order_mark_so_any_path_reads_right() {
        let bytes = vbs_bytes("Run \"C:\\Users\\Jos\u{e9}\\klip.exe\"\r\n");
        assert_eq!(
            &bytes[..2],
            &[0xFF, 0xFE],
            "the byte order mark wscript recognises"
        );
        let units: Vec<u16> = bytes[2..]
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(
            String::from_utf16(&units).unwrap(),
            "Run \"C:\\Users\\Jos\u{e9}\\klip.exe\"\r\n"
        );
    }

    #[test]
    fn both_forms_of_the_login_entry_are_recognised_as_klips_and_nothing_else_is() {
        assert!(
            run_value_is_klips(r#""C:\Users\me\klip.exe" tray start"#),
            "the v0.9 form"
        );
        assert!(run_value_is_klips(
            r#"wscript.exe //B //nologo "C:\Users\me\AppData\Local\Klip\start-tray.vbs""#
        ));
        for foreign in [
            r#"wscript.exe //B //nologo "C:\Users\me\evil.vbs""#,
            r#"wscript.exe //B //nologo "C:\Users\me\Other\start-tray.vbs""#,
            r#"wscript.exe "C:\Users\me\AppData\Local\Klip\start-tray.vbs""#,
            r#"cmd /c "C:\Users\me\AppData\Local\Klip\start-tray.vbs""#,
        ] {
            assert!(!run_value_is_klips(foreign), "{foreign}");
        }
    }

    #[test]
    fn the_data_of_a_reg_query_is_read_exactly() {
        let out = "\r\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Run\r\n    KlipTray    REG_SZ    \"C:\\a b\\klip.exe\" tray start\r\n\r\n";
        assert_eq!(
            parse_reg_sz(out, "KlipTray").as_deref(),
            Some("\"C:\\a b\\klip.exe\" tray start")
        );
        assert_eq!(parse_reg_sz(out, "Other"), None);
        assert_eq!(parse_reg_sz("", "KlipTray"), None);
    }

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
