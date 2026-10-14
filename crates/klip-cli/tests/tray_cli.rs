//! `klip tray install|uninstall|status|start`, with HOME and XDG_CONFIG_HOME pointed at a temp
//! directory (the real login entries are never touched).
#![cfg(target_os = "linux")]
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
fn a_running_tray_is_seen_by_status_and_a_second_one_is_refused() {
    let e = Env::new("lock");
    let dir = e.home.join("klipdir");
    std::fs::create_dir_all(&dir).unwrap();
    assert!(
        text(&e.run(&["tray", "status"])).contains("Tray: not running"),
        "{}",
        text(&e.run(&["tray", "status"]))
    );
    // Stand in for a running tray: hold the lock it holds.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("tray.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    assert!(text(&e.run(&["tray", "status"])).contains("Tray: running"));
    for args in [&["tray", "start"][..], &["tray", "run"][..], &["tray"][..]] {
        let out = e.run(args);
        assert!(!out.status.success(), "{args:?}: {}", text(&out));
        assert!(
            text(&out).contains("already running"),
            "{args:?}: {}",
            text(&out)
        );
    }
}
