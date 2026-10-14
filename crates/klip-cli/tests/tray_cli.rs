//! `klip tray install|uninstall|status` and the bridge's autostart methods, with HOME and
//! XDG_CONFIG_HOME pointed at a temp directory (the real login entries are never touched).
#![cfg(unix)]
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Env {
    home: PathBuf,
}

impl Env {
    fn new(tag: &str) -> Self {
        let home = std::env::temp_dir().join(format!("klip-traycli-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        Self { home }
    }

    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_klip"));
        c.env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("KLIP_CONFIG_DIR", self.home.join("klipdir"))
            .env_remove("PYTHONPATH")
            .stdin(Stdio::null());
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    fn entry(&self) -> PathBuf {
        self.home.join(".config/autostart/klip-tray.desktop")
    }
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn install_uninstall_and_status_manage_only_the_login_entry() {
    let e = Env::new("entry");
    assert!(
        text(&e.run(&["tray", "status"])).contains("Start at login: no"),
        "{}",
        text(&e.run(&["tray", "status"]))
    );

    let out = e.run(&["tray", "install"]);
    assert!(out.status.success(), "{}", text(&out));
    let entry = std::fs::read_to_string(e.entry()).unwrap();
    assert!(
        entry.contains(&format!("Exec={} tray run", env!("CARGO_BIN_EXE_klip"))),
        "{entry}"
    );
    assert!(text(&e.run(&["tray", "status"])).contains("Start at login: yes"));

    let out = e.run(&["tray", "uninstall"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(!e.entry().exists());
    assert!(
        text(&e.run(&["tray", "uninstall"])).contains("was not set"),
        "second time says so"
    );
}

#[test]
fn a_foreign_entry_with_the_same_name_is_refused_and_left_alone() {
    let e = Env::new("foreign");
    std::fs::create_dir_all(e.entry().parent().unwrap()).unwrap();
    std::fs::write(e.entry(), "[Desktop Entry]\nName=Mine\n").unwrap();
    let out = e.run(&["tray", "install"]);
    assert!(!out.status.success());
    assert!(text(&out).contains("not made by klip"), "{}", text(&out));
    let out = e.run(&["tray", "uninstall"]);
    assert!(!out.status.success());
    assert!(std::fs::read_to_string(e.entry())
        .unwrap()
        .contains("Name=Mine"));
}

#[test]
fn the_bridge_can_read_and_set_the_login_entry_for_the_tray() {
    let e = Env::new("bridge");
    let mut child = e
        .command()
        .arg("api")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut call = |req: Value| -> Value {
        writeln!(stdin, "{req}").unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    };
    assert_eq!(
        call(json!({"id": 1, "method": "autostart_get"}))["ok"]["installed"],
        false
    );
    let r = call(json!({"id": 2, "method": "autostart_set", "params": {"enabled": true}}));
    assert_eq!(r["ok"]["installed"], true, "{r}");
    assert!(e.entry().exists());
    let r = call(json!({"id": 3, "method": "autostart_set", "params": {"enabled": false}}));
    assert_eq!(r["ok"]["installed"], false, "{r}");
    assert!(!e.entry().exists());
    let r = call(json!({"id": 4, "method": "autostart_set", "params": {}}));
    assert!(r["err"].as_str().unwrap().contains("enabled"), "{r}");
    drop(stdin);
    let _ = child.wait();
}
