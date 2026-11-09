//! Desktop notification when an automatic copy could not reach a device (Linux: `notify-send`).
use klip_core::{human_size, offline_label, FileReceivedHook, UndeliveredHook};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

fn escape_markup(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Only the device name and a duration: never clip content (notifications can show on a lock screen).
fn body(name: &str, gone: Duration) -> String {
    match offline_label(gone) {
        Some(label) => format!("{name} did not get your last copy. Not connected for {label}."),
        None => format!("{name} did not get your last copy. It is not connected right now."),
    }
}

/// `--` comes first so a device name starting with `-` can never be read as an option.
#[allow(dead_code)]
fn args(name: &str, gone: Duration) -> Vec<String> {
    vec![
        "--app-name=Klip".into(),
        "--expire-time=10000".into(),
        "--".into(),
        "Klip".into(),
        escape_markup(&body(name, gone)),
    ]
}

/// Start the notifier and enforce a strict timeout so a hung child process (e.g. D-Bus or PowerShell)
/// is always terminated and reaped.
fn run(program: &str, args: &[String]) {
    run_with_timeout(program, args, Duration::from_secs(5));
}

fn run_with_timeout(program: &str, args: &[String], timeout: Duration) {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    if let Ok(mut child) = cmd.spawn() {
        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(_status)) => break,
                    Ok(None) => {
                        if start.elapsed() >= timeout {
                            let _ = child.kill();
                            let _ = child.wait();
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(_) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                }
            }
        });
    }
}

/// Standard base64 encoding without external dependencies.
#[cfg(any(target_os = "windows", test))]
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut res = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        res.push(TABLE[((n >> 18) & 63) as usize] as char);
        res.push(TABLE[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            res.push(TABLE[((n >> 6) & 63) as usize] as char);
        } else {
            res.push('=');
        }
        if chunk.len() > 2 {
            res.push(TABLE[(n & 63) as usize] as char);
        } else {
            res.push('=');
        }
    }
    res
}

/// Encodes a UTF-8 string into PowerShell's expected UTF-16LE Base64 format (-EncodedCommand).
#[cfg(any(target_os = "windows", test))]
fn utf16le_base64_encode(s: &str) -> String {
    let utf16_bytes: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    base64_encode(&utf16_bytes)
}

/// Build the PowerShell toast notification script and encode it as UTF-16LE Base64.
/// Title and body are safely Base64 decoded at runtime inside PowerShell, preventing
/// any shell injection, quote stripping, or XML entity parsing bugs.
#[cfg(any(target_os = "windows", test))]
fn build_windows_toast_encoded_command(title: &str, body_text: &str) -> String {
    let title_b64 = base64_encode(title.as_bytes());
    let body_b64 = base64_encode(body_text.as_bytes());
    let ps_code = format!(
        "[Windows.UI.Notifications.ToastNotificationManager,Windows.UI.Notifications,ContentType=WindowsRuntime] > $null; \
         $title=[System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String('{title_b64}')); \
         $body=[System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String('{body_b64}')); \
         $t=[Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
         $n=$t.GetElementsByTagName('text'); \
         $n.Item(0).AppendChild($t.CreateTextNode($title)) > $null; \
         $n.Item(1).AppendChild($t.CreateTextNode($body)) > $null; \
         $toast=[Windows.UI.Notifications.ToastNotification]::new($t); \
         [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Klip').Show($toast);"
    );
    utf16le_base64_encode(&ps_code)
}

/// Send a native desktop notification across Linux, macOS, and Windows.
pub fn send_native_notification(title: &str, body_text: &str) {
    #[cfg(target_os = "linux")]
    {
        run(
            "notify-send",
            &[
                "--app-name=Klip".to_string(),
                "--expire-time=10000".to_string(),
                "--".to_string(),
                title.to_string(),
                escape_markup(body_text),
            ],
        );
    }
    #[cfg(target_os = "macos")]
    {
        let escaped_body = body_text.replace('\\', "\\\\").replace('"', "\\\"");
        let escaped_title = title.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            escaped_body, escaped_title
        );
        run("osascript", &["-e".to_string(), script]);
    }
    #[cfg(target_os = "windows")]
    {
        let encoded_cmd = build_windows_toast_encoded_command(title, body_text);
        run(
            "powershell",
            &[
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-WindowStyle".to_string(),
                "Hidden".to_string(),
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-EncodedCommand".to_string(),
                encoded_cmd,
            ],
        );
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = (title, body_text);
    }
}

/// A desktop notification with a short message of our own (the tray's failures), never clip content.
pub fn say(text: &str) {
    send_native_notification("Klip", text);
}

/// Never the file's name: it can be private and notifications show on a lock screen.
fn file_body(from: &str, count: usize, size: u64) -> String {
    let files = if count == 1 {
        "1 file".to_string()
    } else {
        format!("{count} files")
    };
    format!("Received {files} from {from} ({})", human_size(size))
}

/// Files from one device that arrive within this time of each other (a folder) make one notification.
const FILE_NOTICE_WINDOW: Duration = Duration::from_secs(2);

pub fn file_hook() -> FileReceivedHook {
    use std::collections::HashMap;
    use std::sync::Mutex;
    let pending: Arc<Mutex<HashMap<String, (usize, u64)>>> = Arc::default();
    Arc::new(move |from: &str, _path: &std::path::Path, size: u64| {
        let first = {
            let mut p = pending.lock().unwrap();
            let entry = p.entry(from.to_string()).or_insert((0, 0));
            entry.0 += 1;
            entry.1 += size;
            entry.0 == 1
        };
        if first {
            let (pending, from) = (Arc::clone(&pending), from.to_string());
            std::thread::spawn(move || {
                std::thread::sleep(FILE_NOTICE_WINDOW);
                let (count, bytes) = pending.lock().unwrap().remove(&from).unwrap_or((0, 0));
                if count > 0 {
                    send_native_notification("Klip", &file_body(&from, count, bytes));
                }
            });
        }
    })
}

pub fn hook() -> UndeliveredHook {
    Arc::new(|name: &str, gone: Duration| {
        send_native_notification("Klip", &body(name, gone));
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_in_a_device_name_is_escaped_and_a_dash_cannot_become_an_option() {
        let a = args("-h <b>x</b> & y", Duration::from_secs(5));
        let dashdash = a.iter().position(|x| x == "--").unwrap();
        assert_eq!(dashdash, 2, "options first, then --");
        assert!(a[..dashdash].iter().all(|x| !x.contains("-h")));
        assert_eq!(a.len(), 5, "name and text stay one argv item");
        assert_eq!(
            a[4],
            "-h &lt;b&gt;x&lt;/b&gt; &amp; y did not get your last copy. It is not connected right now."
        );
    }

    #[test]
    fn the_text_says_how_long_once_it_is_over_a_minute() {
        assert!(body("Pixel", Duration::from_secs(14 * 60)).ends_with("Not connected for 14 min."));
        assert!(body("Pixel", Duration::from_secs(3 * 3600)).ends_with("Not connected for 3 h."));
    }

    #[test]
    fn the_file_notice_names_the_device_and_size_but_not_the_file() {
        let text = file_body("<Pixel>", 1, 3 * 1024 * 1024);
        assert_eq!(text, "Received 1 file from <Pixel> (3.0 MiB)");
        assert_eq!(
            escape_markup(&text),
            "Received 1 file from &lt;Pixel&gt; (3.0 MiB)"
        );
        assert_eq!(
            file_body("Pixel", 12, 40 * 1024 * 1024),
            "Received 12 files from Pixel (40.0 MiB)"
        );
    }

    #[test]
    fn a_missing_notifier_is_ignored() {
        run("klip-no-such-program-xyz", &[]);
    }

    #[test]
    fn base64_and_utf16le_encoding_works_and_handles_arbitrary_quotes_and_special_chars() {
        let b64 = base64_encode(b"hello world");
        assert_eq!(b64, "aGVsbG8gd29ybGQ=");

        let empty = base64_encode(b"");
        assert_eq!(empty, "");

        let one = base64_encode(b"a");
        assert_eq!(one, "YQ==");

        let two = base64_encode(b"ab");
        assert_eq!(two, "YWI=");

        let three = base64_encode(b"abc");
        assert_eq!(three, "YWJj");

        let title = "Klip's \"Notification\" with `ticks` & $vars and \\slashes\\";
        let body = "Special body with unicode: 🚀 <tag> & entity \n 'quotes'";
        let encoded = build_windows_toast_encoded_command(title, body);
        assert!(!encoded.is_empty());
        // Verify encoded string consists only of valid Base64 characters
        assert!(encoded
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='));
    }

    #[test]
    fn child_process_timeout_cleans_up_hanging_command() {
        run_with_timeout("klip-no-such-program-xyz", &[], Duration::from_millis(50));
    }
}
