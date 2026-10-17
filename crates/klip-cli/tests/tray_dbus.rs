//! The real `klip tray run` on a private D-Bus session, against an isolated daemon: read the item's
//! properties, click its menu through dbusmenu, and check what the daemon did.
//!
//! Needs `dbus-daemon` and `gdbus`; without them the tests say so and pass (CI images have them).
#![cfg(target_os = "linux")]
mod common;
use common::*;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn have(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

fn wait_until(what: &str, secs: u64, mut cond: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("timed out waiting for {what}");
}

/// A private session bus, a private home, a fake `notify-send` that records what it was asked to say.
struct Desktop {
    bus: Child,
    address: String,
    home: PathBuf,
    path: std::ffi::OsString,
}

impl Desktop {
    fn new(tag: &str) -> Option<Self> {
        if !have("dbus-daemon") || !have("gdbus") {
            // CI sets this: a runner without the tools must fail, not silently skip.
            assert!(
                std::env::var_os("KLIP_REQUIRE_DBUS_TESTS").is_none(),
                "dbus-daemon and gdbus are required"
            );
            eprintln!("skipped: dbus-daemon and gdbus are needed");
            return None;
        }
        let home =
            std::env::temp_dir().join(format!("klip-tray-dbus-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("bin")).unwrap();
        let notify = home.join("bin/notify-send");
        std::fs::write(
            &notify,
            format!("#!/bin/sh\necho \"$@\" >> {}/notes.txt\n", home.display()),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&notify, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut path = std::ffi::OsString::from(home.join("bin"));
        path.push(":");
        path.push(std::env::var_os("PATH").unwrap_or_default());
        let mut bus = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut address = String::new();
        BufReader::new(bus.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        Some(Self {
            bus,
            address: address.trim().to_string(),
            home,
            path,
        })
    }

    fn notes(&self) -> String {
        std::fs::read_to_string(self.home.join("notes.txt")).unwrap_or_default()
    }

    fn gdbus(&self, args: &[&str]) -> String {
        let out = Command::new("gdbus")
            .args(["call", "--session"])
            .args(args)
            .env("DBUS_SESSION_BUS_ADDRESS", &self.address)
            .output()
            .unwrap();
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    }

    fn tray_command(&self, klip: &Path, dir: &Path) -> Command {
        let mut c = Command::new(klip);
        c.env("DBUS_SESSION_BUS_ADDRESS", &self.address)
            .env("KLIP_CONFIG_DIR", dir)
            .env("KLIP_CLIPBOARD", "memory")
            .env("HOME", &self.home)
            .env("XDG_DATA_HOME", self.home.join("share"))
            .env("PATH", &self.path)
            .env("KLIP_TRAY_HOST_WAIT_SECS", "3")
            .env_remove("KLIP_SYSTEMCTL");
        c
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        let _ = self.bus.kill();
        let _ = self.bus.wait();
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

/// The tray process and the way to talk to its D-Bus item.
struct Tray<'a> {
    desktop: &'a Desktop,
    child: Child,
    name: String,
}

impl<'a> Tray<'a> {
    fn start(desktop: &'a Desktop, dir: &Path) -> Self {
        let child = desktop
            .tray_command(Path::new(env!("CARGO_BIN_EXE_klip")), dir)
            .args(["tray", "run"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let name = format!("org.kde.StatusNotifierItem-{}-1", child.id());
        let t = Self {
            desktop,
            child,
            name,
        };
        wait_until("the tray item on the bus", 20, || {
            t.desktop
                .gdbus(&[
                    "--dest",
                    "org.freedesktop.DBus",
                    "--object-path",
                    "/org/freedesktop/DBus",
                    "--method",
                    "org.freedesktop.DBus.NameHasOwner",
                    &t.name,
                ])
                .contains("true")
        });
        t
    }

    fn prop(&self, name: &str) -> String {
        self.desktop.gdbus(&[
            "--dest",
            &self.name,
            "--object-path",
            "/StatusNotifierItem",
            "--method",
            "org.freedesktop.DBus.Properties.Get",
            "org.kde.StatusNotifierItem",
            name,
        ])
    }

    /// `(id, label, enabled)` of every labelled entry, from dbusmenu's GetLayout.
    fn menu(&self) -> Vec<(u32, String, bool)> {
        let raw = self.desktop.gdbus(&[
            "--dest",
            &self.name,
            "--object-path",
            "/MenuBar",
            "--method",
            "com.canonical.dbusmenu.GetLayout",
            "0",
            "--",
            "-1",
            "[]",
        ]);
        let mut found = Vec::new();
        let mut rest = raw.as_str();
        while let Some(i) = rest.find("(") {
            rest = &rest[i + 1..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            let after = &rest[digits.len()..];
            if digits.is_empty() || !after.starts_with(", {") {
                continue;
            }
            let props = &after[3..after.find('}').unwrap_or(after.len())];
            if let Some(l) = props.find("'label': <'") {
                let label = &props[l + 11..];
                let label = &label[..label.find("'>").unwrap_or(label.len())];
                found.push((
                    digits.parse().unwrap(),
                    label.to_string(),
                    !props.contains("'enabled': <false>"),
                ));
            }
        }
        found
    }

    fn labels(&self) -> Vec<String> {
        self.menu().into_iter().map(|(_, l, _)| l).collect()
    }

    fn click(&self, label: &str) {
        let (id, _, enabled) = self
            .menu()
            .into_iter()
            .find(|(_, l, _)| l == label)
            .unwrap_or_else(|| panic!("no menu entry {label:?} in {:?}", self.labels()));
        assert!(enabled, "{label:?} is disabled");
        let out = self.desktop.gdbus(&[
            "--dest",
            &self.name,
            "--object-path",
            "/MenuBar",
            "--method",
            "com.canonical.dbusmenu.Event",
            &id.to_string(),
            "clicked",
            "<0>",
            "0",
        ]);
        // Quitting ends the process before it answers the click: that is not an error.
        assert!(
            !out.contains("Error") || (label == "Quit the tray" && out.contains("NoReply")),
            "{out}"
        );
    }

    fn icon(&self) -> String {
        self.prop("IconName")
    }
}

impl Drop for Tray<'_> {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn with_no_daemon_the_tray_says_stopped_and_start_klip_starts_it() {
    let Some(desktop) = Desktop::new("stopped") else {
        return;
    };
    let klip_dir = desktop.home.join("klipdir");
    std::fs::create_dir_all(&klip_dir).unwrap();
    std::fs::write(klip_dir.join("config.json"), LAN_ONLY).unwrap();
    let tray = Tray::start(&desktop, &klip_dir);

    assert!(
        tray.icon().contains("klip-stopped-symbolic"),
        "{}",
        tray.icon()
    );
    assert!(
        tray.prop("ItemIsMenu").contains("true"),
        "a left click opens the menu"
    );
    assert!(tray.prop("Title").contains("Klip"));
    assert_eq!(
        tray.labels(),
        [
            "Klip is stopped",
            "Start Klip",
            "Open the Klip folder",
            "Open the Klip terminal app",
            "Quit the tray"
        ]
    );

    struct StopDaemon<'a>(&'a Desktop, PathBuf);
    impl Drop for StopDaemon<'_> {
        fn drop(&mut self) {
            let _ = self
                .0
                .tray_command(Path::new(env!("CARGO_BIN_EXE_klip")), &self.1)
                .arg("stop")
                .output();
        }
    }
    let _stop_when_done = StopDaemon(&desktop, klip_dir.clone());
    tray.click("Start Klip");
    wait_until("the daemon to come up and the tray to notice", 40, || {
        tray.icon().contains("klip-no_devices-symbolic")
    });
    assert!(tray
        .labels()
        .contains(&"Send images automatically".to_string()));
    assert!(tray.labels().contains(&"No devices linked".to_string()));
}

#[test]
fn the_menu_changes_what_the_daemon_does() {
    let Some(desktop) = Desktop::new("menu") else {
        return;
    };
    let d = Isolated::start("traymenu", LAN_ONLY);
    let tray = Tray::start(&desktop, &d.dir);
    wait_until("the tray to see the daemon", 20, || {
        tray.icon().contains("klip-no_devices-symbolic")
    });

    // The images switch: ticking it changes the daemon (and its saved config) and the tick follows.
    assert!(text(&d.run(&["images"], None)).contains("Images wait until you send them"));
    tray.click("Send images automatically");
    wait_until("the daemon to switch to automatic", 10, || {
        text(&d.run(&["images"], None)).contains("sent automatically")
    });
    let saved = std::fs::read_to_string(d.dir.join("config.json")).unwrap();
    assert!(saved.contains("\"auto_send_images\": true"), "{saved}");
    tray.click("Send images automatically");
    wait_until("the daemon to switch back", 10, || {
        text(&d.run(&["images"], None)).contains("wait until you send them")
    });

    // Pause and resume.
    tray.click("10 minutes");
    wait_until("the paused icon", 10, || {
        tray.icon().contains("klip-paused-symbolic")
    });
    assert!(tray.labels().contains(&"Resume syncing".to_string()));
    assert!(!tray.labels().contains(&"Pause syncing".to_string()));
    assert!(text(&d.run(&["status"], None))
        .to_lowercase()
        .contains("paused"));
    tray.click("Resume syncing");
    wait_until("the icon to come back", 10, || {
        tray.icon().contains("klip-no_devices-symbolic")
    });

    // Opening the menu renews the picture at once (the poll alone would take up to 15 s).
    assert!(d.run(&["pause"], None).status.success());
    let out = desktop.gdbus(&[
        "--dest",
        &tray.name,
        "--object-path",
        "/MenuBar",
        "--method",
        "com.canonical.dbusmenu.AboutToShow",
        "0",
    ]);
    assert!(!out.contains("Error"), "{out}");
    wait_until("the menu opening to renew the icon", 6, || {
        tray.icon().contains("klip-paused-symbolic")
    });
    assert!(d.run(&["resume"], None).status.success());
    tray.click("Resume syncing");

    // Nothing is waiting to be sent, so the entry is off.
    assert!(tray
        .menu()
        .iter()
        .any(|(_, l, enabled)| l == "Send the last copied image" && !enabled));

    // Quitting ends the process and frees the name.
    tray.click("Quit the tray");
    let mut tray = tray;
    wait_until("the tray to exit", 10, || {
        tray.child.try_wait().ok().flatten().is_some()
    });
}

#[test]
fn a_failed_action_is_a_notification_and_the_old_python_files_are_cleaned_up() {
    let Some(desktop) = Desktop::new("notify") else {
        return;
    };
    let d = Isolated::start("traynotify", LAN_ONLY);
    // What the Python tray of v0.6.0 left behind.
    let legacy = desktop.home.join("share/klip/tray/0.6.0/klip_tray");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join("app.py"), "old").unwrap();
    let tray = Tray::start(&desktop, &d.dir);
    assert!(
        !desktop.home.join("share/klip/tray").exists(),
        "old files are removed"
    );
    wait_until("the icon files", 10, || {
        desktop
            .home
            .join("share/icons/hicolor/scalable/status/klip-connected-symbolic.svg")
            .exists()
    });

    // The daemon stops under the tray: Pause is then refused, and the person is told.
    assert!(d.run(&["stop"], None).status.success());
    wait_until("the stopped icon", 30, || {
        tray.icon().contains("klip-stopped-symbolic")
    });
    // Only "Start Klip" is offered when stopped, so use the middle click: it sends the waiting image.
    let out = desktop.gdbus(&[
        "--dest",
        &tray.name,
        "--object-path",
        "/StatusNotifierItem",
        "--method",
        "org.kde.StatusNotifierItem.SecondaryActivate",
        "0",
        "0",
    ]);
    assert!(!out.contains("Error"), "{out}");
    wait_until("the notification", 10, || {
        desktop.notes().contains("Klip daemon is not running")
    });
}

#[test]
fn a_second_tray_on_the_same_configuration_is_refused() {
    let Some(desktop) = Desktop::new("second") else {
        return;
    };
    let d = Isolated::create("traytwo", LAN_ONLY);
    let tray = Tray::start(&desktop, &d.dir);
    let second = desktop
        .tray_command(Path::new(env!("CARGO_BIN_EXE_klip")), &d.dir)
        .args(["tray", "run"])
        .output()
        .unwrap();
    assert!(!second.status.success());
    assert!(
        text(&second).contains("already running"),
        "{}",
        text(&second)
    );
    drop(tray);
    let _ = std::fs::remove_dir_all(&d.dir);
}

/// A stand-in for the panel's StatusNotifierWatcher: it records who registers.
struct FakeWatcher {
    items: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl FakeWatcher {
    fn register_status_notifier_item(&self, service: String) {
        self.items.lock().unwrap().push(service);
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.items.lock().unwrap().clone()
    }
}

#[test]
fn with_a_panel_the_tray_registers_itself_and_stays_quiet_and_without_one_it_says_so() {
    let Some(desktop) = Desktop::new("watcher") else {
        return;
    };
    let d = Isolated::create("traywatcher", LAN_ONLY);

    // Without a panel: one notification after the wait (3 s here, 30 s for real).
    let lonely = Tray::start(&desktop, &d.dir);
    wait_until("the no-panel notification", 20, || {
        desktop.notes().contains("no system tray")
    });
    drop(lonely);
    let _ = std::fs::remove_file(desktop.home.join("notes.txt"));
    wait_until("the first tray to release its lock", 10, || {
        TrayLockProbe::free(&d.dir)
    });

    // With a panel: the tray registers its item, and there is nothing to complain about.
    let items = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let watcher_items = items.clone();
    let address = desktop.address.clone();
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
    let server = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let _conn = zbus::connection::Builder::address(address.as_str())
                .unwrap()
                .name("org.kde.StatusNotifierWatcher")
                .unwrap()
                .serve_at(
                    "/StatusNotifierWatcher",
                    FakeWatcher {
                        items: watcher_items,
                    },
                )
                .unwrap()
                .build()
                .await
                .unwrap();
            ready_tx.send(()).unwrap();
            while stop_rx.try_recv().is_err() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });
    });
    ready_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let tray = Tray::start(&desktop, &d.dir);
    wait_until("the tray to register with the watcher", 20, || {
        items
            .lock()
            .unwrap()
            .iter()
            .any(|i| i.contains(&format!("StatusNotifierItem-{}-1", tray.child.id())))
    });
    std::thread::sleep(Duration::from_secs(5)); // longer than the 3 s wait
    assert_eq!(desktop.notes(), "", "a working panel gets no warning");
    drop(tray);
    let _ = stop_tx.send(());
    server.join().unwrap();
}

struct TrayLockProbe;

impl TrayLockProbe {
    /// The lock file can be taken again (the previous tray is gone).
    fn free(dir: &Path) -> bool {
        let Ok(f) = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("tray.lock"))
        else {
            return false;
        };
        f.try_lock().is_ok()
    }
}
