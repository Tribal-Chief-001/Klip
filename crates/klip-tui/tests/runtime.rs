use crossterm::event::KeyCode;
use klip_tui::api::ApiError;
use klip_tui::testkit::*;

fn harness(fake: &std::sync::Arc<FakeDaemon>, tag: &str) -> Harness {
    Harness::new(fake.clone(), test_env(tag), 100, 30)
}

#[tokio::test]
async fn boots_and_shows_the_status_from_the_daemon() {
    let fake = FakeDaemon::new();
    let mut h = harness(&fake, "boot");
    assert!(h.wait_for("ACTIVE", 3000).await, "{}", h.screen());
    assert!(h.screen().contains("Vivo V2253"));
    let calls = fake.calls();
    assert!(
        calls.iter().any(|c| c == "status") && calls.iter().any(|c| c == "hello"),
        "{calls:?}"
    );
}

#[tokio::test]
async fn pausing_and_resuming_through_the_menu_reaches_the_daemon() {
    let fake = FakeDaemon::new();
    let mut h = harness(&fake, "pause");
    assert!(h.wait_for("ACTIVE", 3000).await);
    h.text("p");
    assert!(h.screen().contains("Pause syncing"));
    h.key(KeyCode::Enter); // 15 minutes
    assert!(h.wait_for("PAUSED", 3000).await, "{}", h.screen());
    assert!(fake.calls().iter().any(|c| c == "pause 900"));
    h.text("p");
    h.key(KeyCode::Enter); // "Resume now" is the first entry while paused
    assert!(h.wait_for("ACTIVE", 3000).await, "{}", h.screen());
    assert!(fake.calls().iter().any(|c| c == "resume"));
}

#[tokio::test]
async fn a_stopped_daemon_shows_the_start_card_and_recovery_is_automatic() {
    let fake = FakeDaemon::new();
    let mut h = harness(&fake, "down");
    assert!(h.wait_for("ACTIVE", 3000).await);
    fake.with(|s| s.up = false);
    assert!(
        h.wait_for("Klip is not running", 4000).await,
        "{}",
        h.screen()
    );
    assert!(h.screen().contains("STOPPED"));
    fake.with(|s| s.up = true);
    assert!(h.wait_for("ACTIVE", 4000).await, "{}", h.screen());
}

#[tokio::test]
async fn s_on_the_start_card_starts_the_daemon() {
    let fake = FakeDaemon::new();
    fake.with(|s| s.up = false);
    let mut h = harness(&fake, "start");
    assert!(h.wait_for("Klip is not running", 3000).await);
    h.text("s");
    assert!(h.wait_for("ACTIVE", 4000).await, "{}", h.screen());
    assert!(fake.calls().iter().any(|c| c == "start_daemon"));
}

#[tokio::test]
async fn unpair_asks_first_and_unpair_all_removes_the_rest() {
    let fake = FakeDaemon::new();
    let mut h = harness(&fake, "unpair");
    assert!(h.wait_for("ACTIVE", 3000).await);
    h.text("2");
    assert!(h.screen().contains("Devices (2)"), "{}", h.screen());
    h.text("u");
    assert!(h.screen().contains("Unpair Vivo V2253?"), "{}", h.screen());
    h.text("n");
    assert!(
        !fake.calls().iter().any(|c| c.starts_with("unpair")),
        "declining must not call the daemon"
    );
    h.text("u");
    h.text("y");
    assert!(
        h.wait_for("Unpaired Vivo V2253", 3000).await,
        "{}",
        h.screen()
    );
    h.text("U");
    assert!(h.screen().contains("Unpair ALL"), "{}", h.screen());
    h.text("y");
    assert!(
        h.wait_for("Unpaired 1 device", 3000).await,
        "{}",
        h.screen()
    );
    assert!(h.wait_for("No devices yet", 3000).await, "{}", h.screen());
}

#[tokio::test]
async fn resizing_to_a_tiny_terminal_and_back_recovers() {
    let fake = FakeDaemon::new();
    let mut h = harness(&fake, "resize");
    assert!(h.wait_for("ACTIVE", 3000).await);
    h.resize(50, 12);
    assert!(h.screen().contains("Terminal too small"), "{}", h.screen());
    h.resize(1, 1);
    let _ = h.screen(); // must not panic
    h.resize(100, 30);
    assert!(h.screen().contains("ACTIVE"), "{}", h.screen());
}

#[tokio::test]
async fn a_slow_daemon_never_blocks_keys() {
    let fake = FakeDaemon::new();
    fake.with(|s| s.delay_ms = 1500);
    let mut h = harness(&fake, "slow");
    h.text("?");
    assert!(
        h.screen().contains("Keys"),
        "the help overlay must open while the daemon is still answering"
    );
    h.key(KeyCode::Esc);
    h.text("3");
    assert!(h.screen().contains("Pair"), "{}", h.screen());
}

#[tokio::test]
async fn a_panicking_call_does_not_stop_the_polling() {
    let fake = FakeDaemon::new();
    fake.with(|s| s.panic_next = true);
    let mut h = harness(&fake, "panic");
    assert!(h.wait_for("ACTIVE", 5000).await, "{}", h.screen());
}

#[tokio::test]
async fn a_failed_call_is_shown_as_a_toast() {
    let fake = FakeDaemon::new();
    let mut h = harness(&fake, "fail");
    assert!(h.wait_for("ACTIVE", 3000).await);
    fake.with(|s| s.fail_next = Some(ApiError::Refused("no way".into())));
    h.text("p");
    h.key(KeyCode::Enter);
    assert!(
        h.wait_for("Could not pause: no way", 3000).await,
        "{}",
        h.screen()
    );
}

#[tokio::test]
async fn pairing_through_the_screen_shows_the_code_asks_before_linking_and_confirms() {
    use klip_tui::api::PendingPairInfo;
    let fake = FakeDaemon::new();
    let mut h = harness(&fake, "pair");
    assert!(h.wait_for("ACTIVE", 3000).await);
    h.text("3");
    assert!(h.wait_for("klip1TESTTICKET", 3000).await, "{}", h.screen());
    assert!(h.screen().contains("Ticket (single use):"));
    fake.with(|s| {
        s.poll.pending = Some(PendingPairInfo {
            device_name: "Pixel 8".into(),
            fingerprint: "DDDD-EEEE-FFFF".into(),
        });
    });
    assert!(h.wait_for("Link “Pixel 8”?", 3000).await, "{}", h.screen());
    assert!(h.screen().contains("DDDD-EEEE-FFFF"));
    h.text("y");
    h.pump(200).await; // effects run as tasks: let the call happen
    assert!(fake.calls().iter().any(|c| c == "pair_decide true"));
    fake.with(|s| s.poll.paired = Some("Pixel 8".into()));
    assert!(
        h.wait_for("Linked with Pixel 8", 3000).await,
        "{}",
        h.screen()
    );
}

#[tokio::test]
async fn leaving_the_pair_screen_closes_the_window() {
    let fake = FakeDaemon::new();
    let mut h = harness(&fake, "pairleave");
    assert!(h.wait_for("ACTIVE", 3000).await);
    h.text("3");
    assert!(h.wait_for("klip1TESTTICKET", 3000).await);
    h.text("1");
    h.pump(300).await;
    assert!(
        fake.calls().iter().any(|c| c == "pair_cancel"),
        "{:?}",
        fake.calls()
    );
}

#[tokio::test]
async fn joining_with_a_pasted_ticket_reports_success_and_failure() {
    let fake = FakeDaemon::new();
    let mut h = harness(&fake, "join");
    assert!(h.wait_for("ACTIVE", 3000).await);
    h.text("3");
    h.text("j");
    h.paste("klip1JOINJOINJOIN\n");
    h.key(KeyCode::Enter);
    assert!(
        h.wait_for("Linked with Pixel 8", 3000).await,
        "{}",
        h.screen()
    );
    fake.with(|s| s.join_reply = Err(ApiError::Refused("invalid or expired pairing code".into())));
    h.paste("klip1AGAIN");
    h.key(KeyCode::Enter);
    assert!(
        h.wait_for("invalid or expired pairing code", 3000).await,
        "{}",
        h.screen()
    );
}

#[tokio::test]
async fn history_shows_clips_copies_back_and_clears() {
    let fake = FakeDaemon::new();
    fake.with(|s| {
        s.history = vec![
            sample_item(1, "https://example.com/Docs", false),
            sample_item(2, "api_key_SECRET0123456789", true),
        ];
    });
    let mut h = harness(&fake, "hist");
    assert!(h.wait_for("ACTIVE", 3000).await);
    h.text("4");
    assert!(
        h.wait_for("https://example.com/Docs", 3000).await,
        "{}",
        h.screen()
    );
    assert!(h.screen().contains("•••• secret"), "{}", h.screen());
    assert!(
        !h.screen().contains("SECRET0123"),
        "a hidden secret must not appear on screen"
    );
    h.key(KeyCode::Enter);
    h.pump(200).await;
    assert!(fake.calls().iter().any(|c| c == "copy"));
    assert!(
        h.wait_for("Copied to this computer", 2000).await,
        "{}",
        h.screen()
    );
    h.text("j");
    h.text("v");
    assert!(h.wait_for("SECRET0123", 1000).await, "{}", h.screen());
    h.text("C");
    assert!(h.screen().contains("Clear the history?"));
    h.text("y");
    assert!(h.wait_for("History cleared", 3000).await, "{}", h.screen());
    assert!(h.wait_for("No clips yet", 3000).await, "{}", h.screen());
}

#[tokio::test]
async fn on_an_old_daemon_copy_and_clear_explain_themselves() {
    let fake = FakeDaemon::old_daemon();
    fake.with(|s| s.history = vec![sample_item(1, "hello", false)]);
    let mut h = harness(&fake, "histold");
    assert!(h.wait_for("ACTIVE", 3000).await);
    h.text("4");
    assert!(h.wait_for("hello", 3000).await);
    h.key(KeyCode::Enter);
    assert!(h.wait_for("is older", 1000).await, "{}", h.screen());
    assert!(!fake.calls().iter().any(|c| c == "copy"));
    h.text("s");
    assert!(
        h.wait_for("Sent", 3000).await || h.wait_for("Sending", 100).await,
        "{}",
        h.screen()
    );
}

#[tokio::test]
async fn a_huge_hostile_item_renders_quickly_and_safely() {
    let fake = FakeDaemon::new();
    let hostile = format!("\x1b]0;PWNED\x07\x1b[2J\x1b[?1049l{}", "A".repeat(250_000));
    fake.with(|s| s.history = vec![sample_item(1, &hostile, false)]);
    let mut h = harness(&fake, "histbig");
    assert!(h.wait_for("ACTIVE", 3000).await);
    h.text("4");
    let started = std::time::Instant::now();
    assert!(
        h.wait_for("PWNED", 5000).await,
        "the text is shown, not interpreted: {}",
        h.screen()
    );
    let screen = h.screen();
    assert!(!screen.contains('\x1b') && !screen.contains('\x07'));
    assert!(screen.contains('␛'));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "drawing a 250 KB item took {:?}",
        started.elapsed()
    );
}
