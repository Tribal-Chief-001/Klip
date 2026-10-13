//! Random keys, mouse events, pastes, resizes, ticks and replies at the pure core and the drawing
//! code: nothing may panic, and the invariants must hold after every step.
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use klip_tui::api::ApiError;
use klip_tui::app::*;
use klip_tui::testkit::*;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

const CHARS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 ?/-_.,:;'\"[]{}\\|~`!@#$%^&*()+=<>日é";
const PASTES: [&str; 6] = [
    "klip1abc",
    "line1\nline2\r\nline3",
    "\x1b[31mred\x1b]0;x\x07",
    "日本語",
    "",
    "1234567890",
];

fn random_key(rng: &mut StdRng) -> KeyEvent {
    let code = match rng.gen_range(0..32) {
        0..=11 => {
            let chars: Vec<char> = CHARS.chars().collect();
            KeyCode::Char(chars[rng.gen_range(0..chars.len())])
        }
        12 => KeyCode::Enter,
        13 => KeyCode::Esc,
        14 => KeyCode::Tab,
        15 => KeyCode::BackTab,
        16 => KeyCode::Up,
        17 => KeyCode::Down,
        18 => KeyCode::Left,
        19 => KeyCode::Right,
        20 => KeyCode::PageUp,
        21 => KeyCode::PageDown,
        22 => KeyCode::Backspace,
        _ => *[KeyCode::Delete, KeyCode::Home, KeyCode::End, KeyCode::F(5)]
            .get(rng.gen_range(0..4))
            .unwrap(),
    };
    let mut mods = KeyModifiers::NONE;
    if rng.gen_bool(0.1) {
        mods |= KeyModifiers::CONTROL;
    }
    if rng.gen_bool(0.1) {
        mods |= KeyModifiers::ALT;
    }
    if rng.gen_bool(0.1) {
        mods |= KeyModifiers::SHIFT;
    }
    KeyEvent {
        code,
        modifiers: mods,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn random_size(rng: &mut StdRng) -> (u16, u16) {
    match rng.gen_range(0..100) {
        0..=14 => (rng.gen_range(0..4), rng.gen_range(0..4)),
        15..=24 => (rng.gen_range(1..40), rng.gen_range(1..20)),
        25 => (rng.gen_range(200..500), rng.gen_range(100..200)),
        _ => (rng.gen_range(50..160), rng.gen_range(10..50)),
    }
}

fn random_reply(rng: &mut StdRng) -> Reply {
    // Later tasks add their own replies to this pool (pairing, history, send, log, settings).
    let peers = (0..rng.gen_range(0..5u8))
        .map(|i| {
            sample_peer(
                &format!("device {i} 日本 \x1b[31m"),
                rng.gen_bool(0.5),
                i + 1,
            )
        })
        .collect();
    match rng.gen_range(0..24) {
        0..=2 => Reply::Status(Ok(sample_status(peers))),
        3 => Reply::Status(Err(ApiError::DaemonDown)),
        4 => Reply::Status(Err(ApiError::Timeout(15))),
        5 => Reply::Hello(Ok(sample_hello())),
        6 => Reply::Hello(Err(ApiError::Unsupported)),
        7 => Reply::Paused(Ok(rng.gen_range(0..400_000_000))),
        8 => Reply::Resumed(Err(ApiError::Refused("no".into()))),
        9 => Reply::Unpaired(Ok("x\x1b".into())),
        10 => Reply::UnpairedAll(Ok((vec!["a".into()], vec![("b".into(), "\x1b[2J".into())]))),
        11 => Reply::DaemonStarted(Err(ApiError::Refused("boom\nmore".into()))),
        12 => Reply::PairStarted {
            session: rng.gen_range(0..6),
            result: Ok(klip_tui::api::PairTicket {
                ticket: "klip1ABC".into(),
                expires_in_secs: 120,
                host_fingerprint: "A-B-C".into(),
            }),
        },
        13 => Reply::PairPolled {
            session: rng.gen_range(0..6),
            result: Ok(klip_tui::api::PairPollInfo {
                window_open: rng.gen_bool(0.5),
                seconds_left: 5,
                pending: rng.gen_bool(0.5).then(|| klip_tui::api::PendingPairInfo {
                    device_name: "dev\x1b[2J".into(),
                    fingerprint: "AAAA".into(),
                }),
                paired: rng.gen_bool(0.2).then(|| "Pixel".to_string()),
            }),
        },
        14 => Reply::PairDecided {
            session: rng.gen_range(0..6),
            result: Err(ApiError::Refused("no pairing request is waiting".into())),
        },
        15 => Reply::PairJoined {
            session: rng.gen_range(0..6),
            result: Err(ApiError::Refused("invalid".into())),
        },
        16 => Reply::PairCancelled(Ok(())),
        17 => Reply::History(Ok((0..rng.gen_range(0..6u8))
            .map(|i| sample_item(i + 1, "clip \x1b[2J 日本\n\u{202e}x", i % 2 == 0))
            .collect())),
        18 => Reply::History(Err(ApiError::DaemonDown)),
        19 => Reply::Sent(Ok(klip_tui::api::SendReport {
            bytes: 3,
            delivered: vec!["a".into()],
            failed: vec![("b".into(), "\x1b".into())],
        })),
        20 => Reply::Sent(Err(ApiError::TooLarge { size: 9, max: 8 })),
        21 => Reply::FileLoaded {
            path: "p\x1b".into(),
            result: Ok("x".repeat(rng.gen_range(0..5000))),
        },
        22 => Reply::FileLoaded {
            path: "q".into(),
            result: Err("no such file\x1b".into()),
        },
        23 => Reply::FileLoaded {
            path: "big".into(),
            result: Ok("y".repeat(klip_tui::screens::send::EDITOR_MAX_BYTES + 1)),
        },
        24 => Reply::Log(Ok(klip_tui::files::LogChunk {
            new_offset: rng.gen_range(0..1000),
            text: "ERROR \x1b[31mboom\x1b]0;x\x07\nplain\n日本語\n".repeat(rng.gen_range(0..300)),
            reset: rng.gen_bool(0.2),
            missing: rng.gen_bool(0.2),
        })),
        25 => Reply::Log(Err("denied \x1b[2J".into())),
        26 => Reply::Log(Ok(klip_tui::files::LogChunk {
            new_offset: 0,
            text: "x".repeat(70_000),
            reset: false,
            missing: false,
        })),
        27 => Reply::Config(Ok(klip_core::DaemonConfig {
            device_name: "n\x1b".into(),
            max_history_items: rng.gen_range(0..2000),
            ..klip_core::DaemonConfig::default()
        })),
        28 => Reply::Config(Err("bad json \x1b[2J".into())),
        29 => Reply::ConfigSaved(Err("denied".into())),
        30 => Reply::Service(klip_tui::files::ServiceState::Installed("/x/\x1b".into())),
        31 => Reply::ServiceInstalled(Ok("done".into())),
        _ => Reply::DaemonRestarted(Err(ApiError::Refused("no".into()))),
    }
}

fn random_msg(rng: &mut StdRng, now: &mut u64, size: (u16, u16)) -> Msg {
    match rng.gen_range(0..100) {
        0..=64 => Msg::Key(random_key(rng)),
        65..=69 => Msg::Mouse(MouseEvent {
            kind: match rng.gen_range(0..4) {
                0 => MouseEventKind::Down(MouseButton::Left),
                1 => MouseEventKind::Up(MouseButton::Left),
                2 => MouseEventKind::ScrollUp,
                _ => MouseEventKind::ScrollDown,
            },
            column: rng.gen_range(0..size.0.saturating_add(10).max(1)),
            row: rng.gen_range(0..size.1.saturating_add(5).max(1)),
            modifiers: KeyModifiers::NONE,
        }),
        70..=74 => Msg::Paste(PASTES[rng.gen_range(0..PASTES.len())].to_string()),
        75..=79 => {
            let (w, h) = random_size(rng);
            Msg::Resize(w, h)
        }
        80..=89 => {
            *now += rng.gen_range(0..1500);
            Msg::Tick {
                mono_ms: *now,
                wall_ms: 1_700_000_000_000 + *now,
            }
        }
        _ => Msg::Reply(random_reply(rng)),
    }
}

fn run(seed: u64, mut app: AppState, messages: usize) {
    let mut rng = StdRng::seed_from_u64(seed);
    let theme = plain_theme();
    let mut terminal = Terminal::new(TestBackend::new(app.size.0, app.size.1)).unwrap();
    let mut now = 0u64;
    for i in 0..messages {
        let msg = random_msg(&mut rng, &mut now, app.size);
        let resized = matches!(msg, Msg::Resize(..));
        let _effects = update(&mut app, msg);
        if resized {
            terminal.backend_mut().resize(app.size.0, app.size.1);
        }
        // Invariants.
        let peers = app.status.as_ref().map(|s| s.peers.len()).unwrap_or(0);
        assert!(
            app.devices.selected <= peers.saturating_sub(1),
            "selection out of range at message {i}"
        );
        assert!(app.toasts.len() <= 3);
        assert!(app.settings.selected < 9);
        assert!(app.log.lines.len() <= klip_tui::screens::log::MAX_LINES);
        let (r, c) = app.send.buf.cursor();
        assert!(r < app.send.buf.lines().len());
        assert!(c <= app.send.buf.lines()[r].chars().count());
        assert!(app.history.selected <= app.history.visible().len().saturating_sub(1));
        // Drawing at every third message (and always after a resize) keeps the run affordable.
        if resized || i % 3 == 0 {
            terminal
                .draw(|f| klip_tui::view::view(&app, &theme, f))
                .unwrap();
        }
        if app.quit {
            app.quit = false; // keep going: quitting is just a flag here
        }
    }
}

fn base(conn: Conn) -> AppState {
    let mut a = AppState::new((100, 30));
    if matches!(conn, Conn::Up) {
        a.status = Some(sample_status(vec![
            sample_peer("Vivo V2253", true, 1),
            sample_peer("Pixel 8", false, 2),
        ]));
    }
    a.conn = conn;
    a
}

#[test]
fn random_input_never_panics_when_the_daemon_is_up() {
    for seed in 0..4 {
        run(seed, base(Conn::Up), 10_000);
    }
}

#[test]
fn random_input_never_panics_when_the_daemon_is_down_or_connecting() {
    run(
        100,
        base(Conn::Down {
            reason: "Klip is not running".into(),
        }),
        8_000,
    );
    run(101, base(Conn::Connecting), 8_000);
}
