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
    let name = escape_markup(name);
    match offline_label(gone) {
        Some(label) => format!("{name} did not get your last copy. Not connected for {label}."),
        None => format!("{name} did not get your last copy. It is not connected right now."),
    }
}

/// `--` comes first so a device name starting with `-` can never be read as an option.
fn args(name: &str, gone: Duration) -> Vec<String> {
    vec![
        "--app-name=Klip".into(),
        "--expire-time=10000".into(),
        "--".into(),
        "Klip".into(),
        body(name, gone),
    ]
}

/// Start the notifier and forget it; a missing program or no display is not an error.
fn run(program: &str, args: &[String]) {
    if let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

/// A desktop notification with a short message of our own (the tray's failures), never clip content.
pub fn say(text: &str) {
    run(
        "notify-send",
        &[
            "--app-name=Klip".to_string(),
            "--expire-time=10000".to_string(),
            "--".to_string(),
            "Klip".to_string(),
            escape_markup(text),
        ],
    );
}

/// Never the file's name: it can be private and notifications show on a lock screen.
fn file_body(from: &str, count: usize, size: u64) -> String {
    let files = if count == 1 {
        "1 file".to_string()
    } else {
        format!("{count} files")
    };
    format!(
        "Received {files} from {} ({})",
        escape_markup(from),
        human_size(size)
    )
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
                    run(
                        "notify-send",
                        &[
                            "--app-name=Klip".to_string(),
                            "--expire-time=10000".to_string(),
                            "--".to_string(),
                            "Klip".to_string(),
                            file_body(&from, count, bytes),
                        ],
                    );
                }
            });
        }
    })
}

pub fn hook() -> UndeliveredHook {
    Arc::new(|name: &str, gone: Duration| run("notify-send", &args(name, gone)))
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
        assert_eq!(
            file_body("<Pixel>", 1, 3 * 1024 * 1024),
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
}
