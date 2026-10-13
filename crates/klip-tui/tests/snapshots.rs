use klip_tui::app::{AppState, ConfirmAction, Conn, HelloState, Modal, Screen, ToastKind};
use klip_tui::testkit::*;

fn up(cols: u16, rows: u16, peers: Vec<klip_tui::api::PeerStatusInfo>) -> AppState {
    let mut a = AppState::new((cols, rows));
    a.conn = Conn::Up;
    a.hello = HelloState::New(sample_hello());
    a.status = Some(sample_status(peers));
    a
}

fn two_peers() -> Vec<klip_tui::api::PeerStatusInfo> {
    let mut offline = sample_peer("Pixel 8", false, 2);
    offline.last_error = Some("could not reach the device".to_string());
    vec![sample_peer("Vivo V2253", true, 1), offline]
}

fn snap(name: &str, app: &AppState, cols: u16, rows: u16) {
    let text = render(app, &plain_theme(), cols, rows);
    assert_snapshot(&format!("{name}_{cols}x{rows}"), &text);
}

#[test]
fn home_with_devices() {
    snap("home_up", &up(100, 30, two_peers()), 100, 30);
    snap("home_up", &up(60, 16, two_peers()), 60, 16);
}

#[test]
fn home_without_devices() {
    snap("home_empty", &up(100, 30, vec![]), 100, 30);
}

#[test]
fn home_paused_and_pairing_open() {
    let mut a = up(100, 30, two_peers());
    let st = a.status.as_mut().unwrap();
    st.is_paused = true;
    st.paused_remaining_secs = 754;
    st.pairing_open = true;
    snap("home_paused", &a, 100, 30);
}

#[test]
fn home_when_the_daemon_is_down_and_while_connecting() {
    let mut a = AppState::new((100, 30));
    a.conn = Conn::Down {
        reason: "Klip is not running".into(),
    };
    snap("home_down", &a, 100, 30);
    let b = AppState::new((100, 30));
    snap("home_connecting", &b, 100, 30);
}

#[test]
fn long_cjk_and_hostile_device_names_stay_inside_the_box() {
    let peers = vec![
        sample_peer("日本語のとても長いデバイス名前テスト用", true, 1),
        sample_peer("evil\x1b[31m\u{202E}name\x07", false, 2),
    ];
    snap("home_cjk", &up(100, 30, peers), 100, 30);
}

#[test]
fn too_small_terminals_get_a_message_not_a_broken_layout() {
    let a = up(40, 10, two_peers());
    snap("too_small", &a, 40, 10);
}

#[test]
fn devices_screen_with_the_second_device_selected() {
    let mut a = up(100, 30, two_peers());
    a.screen = Screen::Devices;
    a.devices.selected = 1;
    snap("devices", &a, 100, 30);
}

#[test]
fn modals_and_toasts() {
    let mut a = up(100, 30, two_peers());
    a.modal = Some(Modal::Help);
    snap("modal_help", &a, 100, 30);
    a.modal = Some(Modal::Confirm {
        title: "Unpair Pixel 8?".into(),
        body: "It stops syncing with this computer until you pair it again.".into(),
        action: ConfirmAction::Unpair {
            selector: "x".into(),
            name: "Pixel 8".into(),
        },
    });
    snap("modal_confirm", &a, 100, 30);
    a.modal = Some(Modal::Pause {
        selected: 1,
        custom: None,
    });
    snap("modal_pause", &a, 100, 30);
    a.modal = None;
    a.toast(ToastKind::Ok, "Paused: 15:00");
    a.toast(ToastKind::Error, "Could not unpair: boom");
    snap("toasts", &a, 100, 30);
}

#[test]
fn the_tab_bar_fits_at_the_minimum_width() {
    let mut a = up(60, 16, two_peers());
    a.status.as_mut().unwrap().is_paused = true;
    a.status.as_mut().unwrap().paused_remaining_secs = 3900;
    a.screen = Screen::Devices;
    let text = render(&a, &plain_theme(), 60, 16);
    let first = text.lines().next().unwrap();
    assert!(first.contains("PAUSED"), "{first}");
    assert!(first.contains("2 Devices"), "{first}");
}

#[test]
fn pair_screen_with_a_code_and_in_join_mode() {
    use klip_tui::api::PairTicket;
    let mut a = up(100, 60, two_peers());
    a.screen = Screen::Pair;
    a.pair.ticket = Some(PairTicket {
        ticket: format!("klip1{}", "ABCDEFGHJKLMNPQRSTUVWXYZ23456789".repeat(4)),
        expires_in_secs: 120,
        host_fingerprint: "AAAA-BBBB-CCCC".into(),
    });
    a.pair.qr = klip_tui::screens::pair::qr_lines(&a.pair.ticket.as_ref().unwrap().ticket, true);
    a.pair.ours = true;
    snap("pair_show", &a, 100, 60);
    a.size = (100, 30);
    snap("pair_show_small", &a, 100, 30);
    a.size = (60, 16);
    snap("pair_show_min", &a, 60, 16);
    a.size = (100, 30);
    a.pair.mode = klip_tui::screens::pair::PairMode::Join;
    a.pair.join_focus = true;
    a.pair.join.insert_str("klip1PASTEDTICKET");
    snap("pair_join", &a, 100, 30);
    a.pair.joining = true;
    snap("pair_join_waiting", &a, 100, 30);
}

#[test]
fn pairing_approval_and_results() {
    use klip_tui::screens::pair::PairResult;
    let mut a = up(100, 30, two_peers());
    a.screen = Screen::Pair;
    a.modal = Some(Modal::PairApproval {
        name: "Pixel 8".into(),
        fingerprint: "DDDD-EEEE-FFFF".into(),
    });
    snap("pair_approval", &a, 100, 30);
    a.modal = None;
    a.pair.result = Some(PairResult::Linked("Pixel 8".into()));
    snap("pair_linked", &a, 100, 30);
    a.pair.result = Some(PairResult::Expired);
    snap("pair_expired", &a, 100, 30);
}

#[test]
fn history_screen() {
    use klip_tui::testkit::sample_item;
    let items = vec![
        sample_item(
            3,
            "fn main() {\n    println!(\"日本語 hello\");\n}\n",
            false,
        ),
        sample_item(2, "api_key_secret0123456789ABCDEF", true),
        sample_item(1, "evil \x1b[2J\x1b]0;x\x07 \u{202E}text", false),
    ];
    let mut a = up(100, 30, two_peers());
    a.screen = Screen::History;
    a.wall_ms = 1_700_000_000_000 + 3 * 1000 + 42_000;
    a.history.items = items;
    a.history.loaded = true;
    snap("history", &a, 100, 30);
    snap("history", &a, 60, 16);
    a.history.selected = 1;
    snap("history_secret_hidden", &a, 100, 30);
    a.history.revealed.insert([2; 16]);
    snap("history_secret_revealed", &a, 100, 30);
    a.history.revealed.clear();
    a.history.selected = 0;
    a.history.filter.insert_str("hello");
    a.history.filtering = true;
    snap("history_search", &a, 100, 30);
    a.history.filter.clear();
    a.history.filtering = false;
    let mut big = sample_item(9, "x", false);
    big.full_content.clear();
    big.size_bytes = 300_000;
    a.history.items = vec![big];
    snap("history_too_large", &a, 100, 30);
    a.history.items.clear();
    snap("history_empty", &a, 100, 30);
}

#[test]
fn send_screen_states() {
    use klip_tui::api::SendReport;
    use klip_tui::screens::send::{LargePayload, SendOutcome};
    let mut a = up(100, 30, two_peers());
    a.screen = Screen::Send;
    snap("send_empty", &a, 100, 30);
    a.send.focus = true;
    a.send.buf.insert_str(
        "fn main() {\n\tprintln!(\"héllo 日本語 \u{1F980}\");\n}\nevil \x1b[2J\u{202E}text",
    );
    snap("send_text", &a, 100, 30);
    snap("send_text", &a, 60, 16);
    a.send.result = Some(SendOutcome::Done(SendReport {
        bytes: 70,
        delivered: vec!["Vivo V2253".into()],
        failed: vec![(
            "Pixel 8".into(),
            "the other device could not write its clipboard".into(),
        )],
    }));
    snap("send_result", &a, 100, 30);
    a.send.result = Some(SendOutcome::Failed(
        "30.0 MiB is over the limit of 10 B per clip".into(),
    ));
    snap("send_failed", &a, 100, 30);
    a.send.sending = true;
    snap("send_sending", &a, 100, 30);
    a.send.sending = false;
    a.send.prompt = Some(klip_tui::textbuf::TextBuf::from_text(
        "~/notes/todo.txt",
        4096,
    ));
    snap("send_prompt", &a, 100, 30);
    a.send.prompt = None;
    a.send.buf.clear();
    a.send.focus = false;
    a.send.large = Some(LargePayload {
        path: "/tmp/big.log".into(),
        text: "log line\n".repeat(200_000),
    });
    snap("send_large", &a, 100, 30);
}

#[test]
fn log_screen_states() {
    let mut a = up(100, 30, two_peers());
    a.screen = Screen::Log;
    a.log_path = "/home/user/.config/klip/daemon.log".to_string();
    let mut m = up(100, 30, two_peers());
    m.screen = Screen::Log;
    m.log.missing = true;
    snap("log_missing", &m, 100, 30);
    for l in [
        "Starting Klip in the foreground…",
        "✓ device VCUU-LBYY-ES2H  (n0 public relays + discovery)",
        "WARN relay unreachable: dns error",
        "ERROR send to Pixel 8 failed: timed out",
        "hostile \x1b[2J\x1b]0;PWNED\x07 \u{202E}line",
        "a very long line that does not fit in the box and has to be cut with an ellipsis at the right edge of the window, really",
    ] {
        a.log.lines.push_back(l.to_string());
    }
    snap("log", &a, 100, 30);
    snap("log", &a, 60, 16);
    a.log.filter.insert_str("warn");
    a.log.filtering = true;
    snap("log_search", &a, 100, 30);
}

#[test]
fn settings_screen_states() {
    use klip_tui::files::ServiceState;
    // The default device name is this machine's hostname: pin it, or the snapshots differ per computer.
    let cfg = || klip_core::DaemonConfig {
        device_name: "Laptop".to_string(),
        ..klip_core::DaemonConfig::default()
    };
    let mut a = up(100, 30, two_peers());
    a.screen = Screen::Settings;
    a.settings.file = Some(cfg());
    a.settings.draft = cfg();
    a.settings.baseline = Some(cfg());
    a.settings.service = Some(ServiceState::NotInstalled);
    snap("settings", &a, 100, 30);
    snap("settings", &a, 60, 16);
    a.settings.draft.device_name = "Desk laptop".into();
    a.settings.draft.relay = klip_core::RelayMode::Lan;
    a.settings.selected = 4;
    a.settings.editing = Some(klip_tui::textbuf::TextBuf::from_text("999 GiB", 32));
    a.settings.error = Some("max_clip_bytes must be between 1 and 268435456".into());
    snap("settings_editing_error", &a, 100, 30);
    a.settings.editing = None;
    a.settings.error = None;
    a.settings.file = Some(klip_core::DaemonConfig {
        max_history_items: 7,
        ..cfg()
    });
    snap("settings_restart_needed", &a, 100, 30);
    a.settings.load_error = Some("config.json: expected value at line 1 column 1".into());
    snap("settings_broken_file", &a, 100, 30);
}

#[test]
fn every_colour_mode_uses_the_signal_colour_for_the_active_state_and_never_relies_on_colour_alone()
{
    use klip_tui::theme::{Theme, TuiEnv};
    use ratatui::backend::TestBackend;
    use ratatui::style::{Color, Modifier};
    use ratatui::Terminal;
    let a = up(100, 30, two_peers());
    type Case = (&'static [(&'static str, &'static str)], Color, bool);
    let cases: [Case; 5] = [
        (
            &[("COLORTERM", "truecolor")],
            Color::Rgb(200, 242, 90),
            false,
        ),
        (
            &[("COLORTERM", "truecolor"), ("KLIP_THEME", "light")],
            Color::Rgb(77, 122, 0),
            false,
        ),
        (&[("TERM", "xterm-256color")], Color::Indexed(192), false),
        (&[("TERM", "xterm")], Color::Green, false),
        (
            &[("NO_COLOR", "1"), ("COLORTERM", "truecolor")],
            Color::Reset,
            true,
        ),
    ];
    for (vars, want, plain) in cases {
        let env = TuiEnv::from_vars(
            klip_core::KlipPaths::from_dir(std::env::temp_dir().join("klip-tui-theme-snap")),
            |k| {
                vars.iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            },
        );
        let theme = Theme::from_env(&env);
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| klip_tui::view::view(&a, &theme, f)).unwrap();
        let buf = term.backend().buffer();
        // Find the "ACTIVE" word in the status box and look at its first cell.
        let mut found = None;
        for y in 0..buf.area.height {
            let row: String = (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect();
            if let Some(pos) = row.find("● ACTIVE").filter(|_| y > 0) {
                found = Some((row[..pos].chars().count() as u16, y));
                break;
            }
        }
        let (x, y) = found.expect("the ACTIVE marker is on screen");
        let cell = &buf[(x, y)];
        if plain {
            assert_eq!(cell.fg, Color::Reset);
            assert!(
                cell.modifier.contains(Modifier::BOLD),
                "plain mode emphasises with bold"
            );
        } else {
            assert_eq!(cell.fg, want);
        }
        // The state is always also written in words.
        assert!(buf[(x + 2, y)].symbol() == "A");
    }
}

#[test]
fn every_screen_lists_its_keys_in_the_help_and_in_the_hints() {
    use klip_tui::screens::{help_lines, hints};
    for screen in Screen::ALL {
        let mut a = up(100, 30, two_peers());
        a.screen = screen;
        let help = help_lines(&a);
        assert!(
            help.iter().any(|(k, _)| k.contains("1-7")),
            "{screen:?} must list the global keys"
        );
        if screen != Screen::Home {
            assert!(
                help.len() >= 9,
                "{screen:?} must list its own keys: {help:?}"
            );
        }
        if screen != Screen::Pair && screen != Screen::Home {
            assert!(!hints(&a).is_empty(), "{screen:?} needs key hints");
        }
    }
}

#[test]
fn pair_screen_with_a_big_qr_code_side_by_side_and_when_it_does_not_fit() {
    use klip_tui::api::PairTicket;
    use klip_tui::screens::pair;
    let ticket = PairTicket {
        ticket: format!("klip1{}", "abcdefghijklmnopqrstuvwxyz234567".repeat(7)),
        expires_in_secs: 120,
        host_fingerprint: "HW4Z-3H75-X8HS".into(),
    };
    let mut a = up(110, 44, two_peers());
    a.screen = Screen::Pair;
    a.pair.qr = pair::qr_lines(&ticket.ticket, true);
    a.pair.ticket = Some(ticket);
    a.pair.ours = true;
    snap("pair_qr_side_by_side", &a, 110, 44);
    let (c, r) = pair::qr_need(a.pair.qr.as_ref().unwrap());
    a.size = (100, 30);
    snap("pair_qr_no_room", &a, 100, 30);
    assert!(
        c <= 110 && r <= 44,
        "the side-by-side snapshot is at the needed size"
    );
}
