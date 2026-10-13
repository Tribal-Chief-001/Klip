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
