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
