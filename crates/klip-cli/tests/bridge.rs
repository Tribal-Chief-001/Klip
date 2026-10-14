//! The hidden `klip api` JSON bridge (what the tray app talks to) against a real isolated daemon.
#![cfg(unix)]
mod common;
use common::*;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Bridge {
    child: Child,
    stdin: Option<ChildStdin>,
    out: BufReader<ChildStdout>,
}

impl Bridge {
    fn start(d: &Isolated) -> Self {
        Self::start_with(d, &[])
    }

    fn start_with(d: &Isolated, extra_env: &[(&str, &str)]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_klip"))
            .arg("api")
            .envs(extra_env.iter().copied())
            .env("KLIP_CONFIG_DIR", &d.dir)
            .env("KLIP_CLIPBOARD", "memory")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start klip api");
        let stdin = child.stdin.take();
        let out = BufReader::new(child.stdout.take().unwrap());
        Self { child, stdin, out }
    }

    fn send_line(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        self.out.read_line(&mut line).unwrap();
        assert!(!line.is_empty(), "the bridge closed its output");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON ({e}): {line}"))
    }

    fn call(&mut self, request: Value) -> Value {
        self.send_line(&request.to_string());
        self.read()
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.stdin = None;
        let _ = self.child.wait();
    }
}

#[test]
fn status_comes_back_as_json_with_fingerprints_added() {
    let d = Isolated::start("br1", LAN_ONLY);
    let mut b = Bridge::start(&d);
    let r = b.call(json!({"id": 7, "ipc": "GetStatus"}));
    assert_eq!(r["id"], 7);
    let status = &r["ok"]["Status"];
    assert_eq!(status["is_active"], true);
    let fp = status["fingerprint"]
        .as_str()
        .expect("fingerprint next to node_id");
    assert_eq!(fp.len(), 14, "{fp}");
    assert!(fp.chars().filter(|c| *c == '-').count() == 2);
}

#[test]
fn history_comes_without_clip_contents_and_the_text_is_fetched_on_purpose() {
    let d = Isolated::start("br2", LAN_ONLY);
    let mut b = Bridge::start(&d);
    let sent = b.call(json!({"id": 1, "ipc": {"SendClip": {"text": "a private sentence"}}}));
    assert!(sent["ok"]["Sent"].is_object(), "{sent}");
    let r = b.call(json!({"id": 2, "ipc": {"GetHistory": {"limit": 5}}}));
    let items = r["ok"]["History"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert!(
        item.get("full_content").is_none(),
        "contents must not travel: {item}"
    );
    assert_eq!(item["content_len"], "a private sentence".len());
    let id_hex = item["id_hex"].as_str().unwrap();
    assert_eq!(id_hex.len(), 32);
    let t = b.call(json!({"id": 3, "method": "history_text", "params": {"id_hex": id_hex}}));
    assert_eq!(t["ok"]["text"], "a private sentence");
    let missing =
        b.call(json!({"id": 4, "method": "history_text", "params": {"id_hex": "00".repeat(16)}}));
    assert!(
        missing["err"].as_str().unwrap().contains("no longer"),
        "{missing}"
    );
}

#[test]
fn many_requests_at_once_are_all_answered_with_their_own_ids() {
    let d = Isolated::start("br3", LAN_ONLY);
    let mut b = Bridge::start(&d);
    for i in 0..30 {
        b.send_line(&json!({"id": i, "ipc": "GetStatus"}).to_string());
    }
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..30 {
        let r = b.read();
        assert!(r["ok"]["Status"].is_object(), "{r}");
        seen.insert(r["id"].as_i64().unwrap());
    }
    assert_eq!(seen.len(), 30);
}

#[test]
fn a_bad_line_gets_an_error_and_the_bridge_keeps_running() {
    let d = Isolated::start("br4", LAN_ONLY);
    let mut b = Bridge::start(&d);
    b.send_line("this is not json");
    let r = b.read();
    assert!(r["err"].as_str().unwrap().contains("not valid JSON"), "{r}");
    let r = b.call(json!({"id": 1, "ipc": {"NoSuchRequest": {}}}));
    assert!(r["err"].as_str().unwrap().contains("bad request"), "{r}");
    let r = b.call(json!({"id": 2, "method": "no_such_method"}));
    assert!(r["err"].as_str().unwrap().contains("unknown method"), "{r}");
    let r = b.call(json!({"id": 3, "ipc": "GetStatus"}));
    assert!(r["ok"]["Status"].is_object(), "still alive: {r}");
}

#[test]
fn a_missing_daemon_is_reported_as_down_not_as_a_crash() {
    let d = Isolated::create("br5", LAN_ONLY);
    let mut b = Bridge::start(&d);
    let r = b.call(json!({"id": 1, "ipc": "GetStatus"}));
    assert_eq!(r["down"], true, "{r}");
    assert!(r["err"].as_str().unwrap().contains("not running"), "{r}");
    let r = b.call(json!({"id": 2, "method": "version"}));
    assert!(r["ok"]["klip"].as_str().unwrap().len() >= 5, "{r}");
}

#[test]
fn config_can_be_read_and_written_and_a_bad_one_is_refused() {
    let d = Isolated::start("br6", LAN_ONLY);
    let mut b = Bridge::start(&d);
    let r = b.call(json!({"id": 1, "method": "config_get"}));
    let mut cfg = r["ok"].clone();
    assert_eq!(cfg["relay"], "lan");
    assert_eq!(cfg["auto_send_images"], false);
    cfg["auto_send_images"] = json!(true);
    cfg["max_history_items"] = json!(9);
    let r = b.call(json!({"id": 2, "method": "config_set", "params": {"config": cfg}}));
    assert!(r["ok"].is_object(), "{r}");
    let file = std::fs::read_to_string(d.dir.join("config.json")).unwrap();
    assert!(
        file.contains("\"auto_send_images\": true") && file.contains("\"max_history_items\": 9"),
        "{file}"
    );

    let before = file;
    let mut bad = cfg.clone();
    bad["max_history_items"] = json!(0);
    let r = b.call(json!({"id": 3, "method": "config_set", "params": {"config": bad}}));
    assert!(
        r["err"].as_str().unwrap().contains("max_history_items"),
        "{r}"
    );
    let mut unknown = cfg.clone();
    unknown["surprise"] = json!(1);
    let r = b.call(json!({"id": 4, "method": "config_set", "params": {"config": unknown}}));
    assert!(r["err"].is_string(), "{r}");
    assert_eq!(
        std::fs::read_to_string(d.dir.join("config.json")).unwrap(),
        before
    );
}

#[test]
fn a_qr_code_comes_back_as_square_modules_and_the_log_can_be_tailed() {
    let d = Isolated::start("br7", LAN_ONLY);
    let mut b = Bridge::start(&d);
    let r = b.call(json!({"id": 1, "method": "qr", "params": {"text": "klip1abcdefghijklmnop"}}));
    let size = r["ok"]["size"].as_u64().unwrap() as usize;
    let modules = r["ok"]["modules"].as_str().unwrap();
    assert!(size >= 21);
    assert_eq!(modules.len(), size * size);
    assert!(modules.chars().all(|c| c == '0' || c == '1'));
    assert!(modules.contains('1') && modules.contains('0'));

    let r = b.call(json!({"id": 2, "method": "log_tail", "params": {"offset": null}}));
    assert!(r["ok"]["new_offset"].is_u64(), "{r}");
    assert!(r["ok"]["text"].is_string() && r["ok"]["reset"].is_boolean());
}

#[test]
fn closing_stdin_ends_the_bridge_quickly_and_cleanly() {
    let d = Isolated::start("br8", LAN_ONLY);
    let mut b = Bridge::start(&d);
    let _ = b.call(json!({"id": 1, "ipc": "GetStatus"}));
    b.stdin = None;
    let t = std::time::Instant::now();
    let status = b.child.wait().unwrap();
    assert!(status.success());
    assert!(t.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn the_daemon_can_be_started_and_restarted_through_the_bridge() {
    let d = Isolated::create("br9", LAN_ONLY);
    let mut b = Bridge::start(&d);
    let r = b.call(json!({"id": 1, "ipc": "GetStatus"}));
    assert_eq!(r["down"], true);
    let r = b.call(json!({"id": 2, "method": "daemon_start"}));
    assert!(
        r["ok"]["message"].as_str().unwrap().contains("started"),
        "{r}"
    );
    let r = b.call(json!({"id": 3, "ipc": "GetStatus"}));
    let first = r["ok"]["Status"]["fingerprint"]
        .as_str()
        .unwrap()
        .to_string();
    let again = b.call(json!({"id": 4, "method": "daemon_start"}));
    assert!(
        again["err"].as_str().unwrap().contains("already running"),
        "{again}"
    );

    let r = b.call(json!({"id": 5, "method": "daemon_restart"}));
    assert!(
        r["ok"]["message"].as_str().unwrap().contains("restarted"),
        "{r}"
    );
    let r = b.call(json!({"id": 6, "ipc": "GetStatus"}));
    assert_eq!(
        r["ok"]["Status"]["fingerprint"].as_str().unwrap(),
        first,
        "same device after a restart"
    );
}

fn fake_systemctl(dir: &std::path::Path) -> (String, std::path::PathBuf) {
    let log = dir.join("systemctl.log");
    let script = dir.join("fake-systemctl");
    std::fs::write(
        &script,
        format!("#!/bin/sh\necho \"$@\" >> '{}'\nexit 0\n", log.display()),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (script.display().to_string(), log)
}

#[test]
fn start_at_login_writes_the_unit_and_enables_or_disables_it_without_touching_the_real_one() {
    let d = Isolated::create("br10", LAN_ONLY);
    let home = d.dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let (fake, log) = fake_systemctl(&d.dir);
    let mut b = Bridge::start_with(
        &d,
        &[("HOME", home.to_str().unwrap()), ("KLIP_SYSTEMCTL", &fake)],
    );
    let r = b.call(json!({"id": 1, "method": "service_state"}));
    assert_eq!(r["ok"]["installed"], false, "{r}");

    let r = b.call(json!({"id": 2, "method": "service_set", "params": {"enabled": true}}));
    assert!(r["ok"].is_object(), "{r}");
    let unit = home.join(".config/systemd/user/klip.service");
    assert!(std::fs::read_to_string(&unit)
        .unwrap()
        .contains("start --foreground"));
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(
        calls.contains("--user daemon-reload") && calls.contains("--user enable klip.service"),
        "{calls}"
    );

    let r = b.call(json!({"id": 3, "method": "service_state"}));
    assert_eq!(r["ok"]["installed"], true, "{r}");
    let r = b.call(json!({"id": 4, "method": "service_set", "params": {"enabled": false}}));
    assert!(r["ok"].is_object(), "{r}");
    assert!(std::fs::read_to_string(&log)
        .unwrap()
        .contains("--user disable klip.service"));
}
