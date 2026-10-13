//! The real `klip` binary against an isolated daemon (own config dir, in-memory clipboard).
#![cfg(unix)]
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Isolated {
    dir: PathBuf,
}

impl Isolated {
    fn start(tag: &str, config: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("klip-cli-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), config).unwrap();
        let me = Self { dir };
        let out = me.run(&["start"], None);
        assert!(out.status.success(), "start failed: {}", text(&out));
        me
    }

    fn run(&self, args: &[&str], stdin: Option<&[u8]>) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_klip"));
        cmd.args(args)
            .env("KLIP_CONFIG_DIR", &self.dir)
            .env("KLIP_CLIPBOARD", "memory")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match stdin {
            Some(bytes) => {
                cmd.stdin(Stdio::piped());
                let mut child = cmd.spawn().unwrap();
                let mut sin = child.stdin.take().unwrap();
                let data = bytes.to_vec();
                let writer = std::thread::spawn(move || {
                    let _ = sin.write_all(&data); // the CLI may stop reading early on an error
                });
                let out = child.wait_with_output().unwrap();
                let _ = writer.join();
                out
            }
            None => {
                cmd.stdin(Stdio::null());
                cmd.output().unwrap()
            }
        }
    }
}

impl Drop for Isolated {
    fn drop(&mut self) {
        let _ = self.run(&["stop"], None);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

const LAN_ONLY: &str = r#"{"relay":"lan","lan_discovery":false}"#;

#[test]
fn history_clear_forgets_the_clips() {
    let d = Isolated::start("hc", LAN_ONLY);
    assert!(d
        .run(&["send", "remember-this-text"], None)
        .status
        .success());
    assert!(text(&d.run(&["history"], None)).contains("remember-this-text"));
    let cleared = d.run(&["history", "clear", "--yes"], None);
    assert!(cleared.status.success(), "{}", text(&cleared));
    assert!(
        text(&d.run(&["history"], None)).contains("No clips"),
        "history should be empty"
    );
}

#[test]
fn unpair_all_with_no_devices_says_so() {
    let d = Isolated::start("ua", LAN_ONLY);
    let out = d.run(&["unpair", "--all", "--yes"], None);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("No linked devices"), "{}", text(&out));
}

#[test]
fn unpair_needs_a_device_or_all_and_not_both() {
    let d = Isolated::start("ub", LAN_ONLY);
    assert!(!d.run(&["unpair"], None).status.success());
    assert!(!d.run(&["unpair", "x", "--all"], None).status.success());
}

#[test]
fn a_9_mb_text_through_stdin_gets_a_normal_answer_not_a_broken_pipe() {
    let d = Isolated::start("big", LAN_ONLY);
    let text_9mb = vec![b'a'; 9_000_000];
    let out = d.run(&["send", "-"], Some(&text_9mb));
    let all = text(&out);
    assert!(!all.contains("Broken pipe"), "{all}");
    assert!(out.status.success(), "{all}");
    assert!(all.contains("No linked devices"), "{all}");
}

#[test]
fn text_over_the_daemons_cap_is_refused_before_it_is_written() {
    let d = Isolated::start(
        "cap",
        r#"{"relay":"lan","lan_discovery":false,"max_clip_bytes":1000}"#,
    );
    let out = d.run(&["send", "-"], Some(&vec![b'b'; 100_000]));
    let all = text(&out);
    assert!(!out.status.success(), "{all}");
    assert!(all.contains("at most 66536"), "{all}");
    assert!(!all.contains("Broken pipe"), "{all}");
}
