//! The real `klip` binary in a pseudo-terminal: keystrokes in, the screen read back through a
//! terminal emulator. Covers the parts no in-process test can: raw mode, the alternate screen, signals.
#![cfg(unix)]
mod common;
use common::*;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    parser: Arc<Mutex<vt100::Parser>>,
}

impl Pty {
    fn spawn(args: &[&str], env: &[(String, String)], size: (u16, u16)) -> Pty {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: size.1,
                cols: size.0,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_klip"));
        cmd.args(args);
        cmd.env("TERM", "xterm-256color");
        for (k, v) in env {
            cmd.env(k, v);
        }
        let child = pair.slave.spawn_command(cmd).expect("spawn klip");
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        let writer = pair.master.take_writer().unwrap();
        let parser = Arc::new(Mutex::new(vt100::Parser::new(size.1, size.0, 0)));
        let feed = Arc::clone(&parser);
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                feed.lock().unwrap().process(&buf[..n]);
            }
        });
        Pty {
            master: pair.master,
            writer,
            child,
            parser,
        }
    }

    fn screen(&self) -> String {
        self.parser.lock().unwrap().screen().contents()
    }
    fn alt_screen(&self) -> bool {
        self.parser.lock().unwrap().screen().alternate_screen()
    }
    fn cursor_hidden(&self) -> bool {
        self.parser.lock().unwrap().screen().hide_cursor()
    }
    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).unwrap();
        self.writer.flush().unwrap();
    }
    fn wait_for(&self, needle: &str, secs: u64) -> bool {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            if self.screen().contains(needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        self.screen().contains(needle)
    }
    fn wait_exit(&mut self, secs: u64) -> Option<u32> {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            if let Ok(Some(status)) = self.child.try_wait() {
                std::thread::sleep(Duration::from_millis(100)); // let the reader see the last bytes
                return Some(status.exit_code());
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        None
    }
    fn resize(&mut self, cols: u16, rows: u16) {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        self.parser
            .lock()
            .unwrap()
            .screen_mut()
            .set_size(rows, cols);
    }
    fn signal(&self, sig: i32) {
        let pid = self.child.process_id().expect("pid") as i32;
        // SAFETY: plain kill(2) on the child we spawned.
        unsafe {
            libc::kill(pid, sig);
        }
    }
}

fn running() -> (Isolated, Pty) {
    let d = Isolated::start(&format!("pty{}", std::process::id() % 1000), LAN_ONLY);
    let p = Pty::spawn(&[], &d.env_pairs(), (100, 30));
    (d, p)
}

#[test]
fn home_opens_in_the_alternate_screen_and_q_gives_the_terminal_back() {
    let (_d, mut p) = running();
    assert!(p.wait_for("ACTIVE", 15), "{}", p.screen());
    assert!(p.alt_screen());
    assert!(p.screen().contains("No devices yet"));
    p.send(b"q");
    assert_eq!(p.wait_exit(5), Some(0), "{}", p.screen());
    assert!(!p.alt_screen(), "the alternate screen must be left");
    assert!(!p.cursor_hidden(), "the cursor must be visible again");
}

#[test]
fn number_keys_open_screens_and_the_help_overlay_lists_keys() {
    let (_d, mut p) = running();
    assert!(p.wait_for("ACTIVE", 15));
    p.send(b"2");
    assert!(p.wait_for("Devices (0)", 5), "{}", p.screen());
    p.send(b"3");
    assert!(p.wait_for("Pair a device", 5), "{}", p.screen());
    assert!(
        p.wait_for("Ticket (single use):", 10),
        "the real daemon issues a real ticket: {}",
        p.screen()
    );
    p.send(b"2");
    assert!(p.wait_for("Devices (0)", 5), "{}", p.screen());
    p.send(b"?");
    assert!(p.wait_for("quit (Klip keeps running)", 5), "{}", p.screen());
    p.send(b"q"); // closes the help
    p.send(b"1");
    assert!(p.wait_for("this device", 5), "{}", p.screen());
    p.send(b"q");
    assert_eq!(p.wait_exit(5), Some(0));
}

#[test]
fn sigterm_and_sighup_restore_the_terminal() {
    for (sig, tag) in [(libc::SIGTERM, "term"), (libc::SIGHUP, "hup")] {
        let d = Isolated::start(&format!("sig{tag}"), LAN_ONLY);
        let mut p = Pty::spawn(&[], &d.env_pairs(), (100, 30));
        assert!(p.wait_for("ACTIVE", 15), "{}", p.screen());
        p.signal(sig);
        assert!(
            p.wait_exit(5).is_some(),
            "klip did not exit after signal {sig}"
        );
        assert!(!p.alt_screen(), "alternate screen left after signal {sig}");
        assert!(!p.cursor_hidden());
    }
}

#[test]
fn a_panic_restores_the_terminal_before_the_message_is_printed() {
    if !cfg!(debug_assertions) {
        return; // the injection switch exists in debug builds only
    }
    let d = Isolated::create("panic", LAN_ONLY);
    let mut env = d.env_pairs();
    env.push(("KLIP_TUI_TEST_PANIC".to_string(), "1".to_string()));
    let mut p = Pty::spawn(&[], &env, (100, 30));
    let code = p.wait_exit(10).expect("klip should exit after the panic");
    assert_ne!(code, 0);
    assert!(!p.alt_screen(), "alternate screen must be left");
    assert!(
        p.screen().contains("injected panic"),
        "the panic message must be readable: {}",
        p.screen()
    );
}

#[test]
fn term_dumb_means_help_and_exit_2_like_without_a_terminal() {
    let d = Isolated::create("dumb", LAN_ONLY);
    let mut env = d.env_pairs();
    env.push(("TERM".to_string(), "dumb".to_string()));
    let mut p = Pty::spawn(&[], &env, (100, 30));
    assert_eq!(p.wait_exit(10), Some(2), "{}", p.screen());
    assert!(p.screen().contains("Usage: klip"), "{}", p.screen());
    assert!(!p.alt_screen());
}

#[test]
fn resizing_works_and_a_tiny_window_shows_the_message() {
    let (_d, mut p) = running();
    assert!(p.wait_for("ACTIVE", 15));
    p.resize(70, 20);
    p.send(b"\x0c"); // Ctrl+L: redraw after the emulator's own resize
    assert!(p.wait_for("ACTIVE", 5), "{}", p.screen());
    p.resize(50, 12);
    assert!(p.wait_for("Terminal too small", 5), "{}", p.screen());
    p.resize(100, 30);
    assert!(p.wait_for("ACTIVE", 5), "{}", p.screen());
    p.send(b"q");
    assert_eq!(p.wait_exit(5), Some(0), "{}", p.screen());
}

#[test]
fn with_no_daemon_the_start_card_starts_it() {
    let d = Isolated::create("nodaemon", LAN_ONLY);
    let mut p = Pty::spawn(&[], &d.env_pairs(), (100, 30));
    assert!(p.wait_for("Klip is not running", 10), "{}", p.screen());
    p.send(b"s");
    assert!(p.wait_for("ACTIVE", 40), "{}", p.screen());
    p.send(b"q");
    assert_eq!(p.wait_exit(5), Some(0));
    // `d` is dropped here and runs `klip stop`.
}

#[test]
#[ignore = "manual: needs a running daemon in your normal Klip directory (not isolated)"]
fn against_the_installed_daemon() {
    let mut p = Pty::spawn(&[], &[], (100, 30));
    assert!(
        p.wait_for("ACTIVE", 15) || p.wait_for("PAUSED", 1),
        "{}",
        p.screen()
    );
    println!("---- Home ----\n{}", p.screen());
    p.send(b"2");
    assert!(p.wait_for("Details", 5), "{}", p.screen());
    println!("---- Devices ----\n{}", p.screen());
    p.send(b"q");
    assert_eq!(p.wait_exit(5), Some(0));
}

#[test]
fn hostile_clip_text_cannot_leave_the_alternate_screen_or_change_the_terminal() {
    let d = Isolated::start("hostile", LAN_ONLY);
    // A local send is recorded in the history, so no second device is needed.
    let hostile = "\x1b]0;PWNED\x07\x1b[2J\x1b[?1049l\x1b[31mred\u{202E}evil";
    assert!(d.run(&["send", hostile], None).status.success());
    let mut p = Pty::spawn(&[], &d.env_pairs(), (100, 30));
    assert!(p.wait_for("ACTIVE", 15));
    p.send(b"4");
    assert!(p.wait_for("Clip", 10), "{}", p.screen());
    if p.wait_for("•••• secret", 2) {
        p.send(b"v"); // the secret detector may flag odd text: reveal it, the safety question is the same
    }
    assert!(
        p.wait_for("PWNED", 10),
        "the text must be visible as text: {}",
        p.screen()
    );
    assert!(
        p.alt_screen(),
        "an escape sequence in a clip must not take the terminal out of the alternate screen"
    );
    assert!(p.screen().contains("␛"), "{}", p.screen());
    p.send(b"q");
    assert_eq!(p.wait_exit(5), Some(0));
}
