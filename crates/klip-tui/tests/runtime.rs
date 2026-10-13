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

#[tokio::test]
async fn typing_and_sending_text_reaches_the_daemon_and_shows_who_got_it() {
    use crossterm::event::KeyModifiers;
    let fake = FakeDaemon::new();
    let mut h = harness(&fake, "send");
    assert!(h.wait_for("ACTIVE", 3000).await);
    h.text("5");
    h.text("héllo 日本\n");
    h.paste("second line\r\nthird");
    assert!(h.screen().contains("third"));
    h.key_mod(KeyCode::Char('s'), KeyModifiers::CONTROL);
    assert!(h.wait_for("✓ Vivo V2253", 3000).await, "{}", h.screen());
    let sent = fake
        .calls()
        .into_iter()
        .find(|c| c.starts_with("send "))
        .expect("a send call");
    assert_eq!(
        sent,
        format!("send {}", "héllo 日本\nsecond line\nthird".len())
    );
}

#[tokio::test]
async fn a_failed_delivery_is_shown_per_device() {
    use crossterm::event::KeyModifiers;
    let fake = FakeDaemon::new();
    fake.with(|s| {
        s.send_reply = Ok(klip_tui::api::SendReport {
            bytes: 0,
            delivered: vec!["Vivo V2253".into()],
            failed: vec![(
                "Pixel 8".into(),
                "the other device could not write its clipboard".into(),
            )],
        });
    });
    let mut h = harness(&fake, "sendfail");
    assert!(h.wait_for("ACTIVE", 3000).await);
    h.text("5");
    h.text("x");
    h.key_mod(KeyCode::Char('s'), KeyModifiers::CONTROL);
    assert!(
        h.wait_for(
            "✗ Pixel 8: the other device could not write its clipboard",
            3000
        )
        .await,
        "{}",
        h.screen()
    );
    assert!(h.screen().contains("✓ Vivo V2253"));
}

#[tokio::test]
async fn sending_with_no_linked_devices_says_so() {
    use crossterm::event::KeyModifiers;
    let fake = FakeDaemon::new();
    fake.with(|s| {
        s.send_reply = Ok(klip_tui::api::SendReport {
            bytes: 1,
            delivered: vec![],
            failed: vec![],
        })
    });
    let mut h = harness(&fake, "sendnone");
    assert!(h.wait_for("ACTIVE", 3000).await);
    h.text("5");
    h.text("x");
    h.key_mod(KeyCode::Char('s'), KeyModifiers::CONTROL);
    assert!(
        h.wait_for("No linked devices: nothing was sent", 3000)
            .await,
        "{}",
        h.screen()
    );
}

#[tokio::test]
async fn loading_a_real_file_from_disk_works_and_a_missing_one_is_reported() {
    use crossterm::event::KeyModifiers;
    let fake = FakeDaemon::new();
    let mut h = harness(&fake, "sendfile");
    assert!(h.wait_for("ACTIVE", 3000).await);
    let path = std::env::temp_dir().join(format!("klip-tui-send-{}.txt", std::process::id()));
    std::fs::write(&path, "from a file\nsecond\n").unwrap();
    h.text("5");
    h.key_mod(KeyCode::Char('o'), KeyModifiers::CONTROL);
    h.text(path.to_str().unwrap());
    h.key(KeyCode::Enter);
    assert!(h.wait_for("from a file", 3000).await, "{}", h.screen());
    h.key_mod(KeyCode::Char('o'), KeyModifiers::CONTROL);
    h.text("/definitely/not/here.txt");
    h.key(KeyCode::Enter);
    assert!(h.wait_for("No such file", 3000).await, "{}", h.screen());
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn the_log_screen_follows_a_real_file_and_survives_truncation_and_absence() {
    use std::io::Write;
    let env = test_env("logscreen");
    let log = klip_tui::files::log_path(&env.paths);
    let fake = FakeDaemon::new();
    let mut h = Harness::new(fake.clone(), env, 100, 30);
    assert!(h.wait_for("ACTIVE", 3000).await);
    h.text("6");
    assert!(h.wait_for("No log yet", 3000).await, "{}", h.screen());
    std::fs::write(&log, "\x1b[32m✓\x1b[0m device ABCD\nsecond line\n").unwrap();
    assert!(h.wait_for("✓ device ABCD", 3000).await, "{}", h.screen());
    assert!(!h.screen().contains('\x1b'));
    let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
    f.write_all(b"WARN relay slow\nERROR boom \x1b]0;PWNED\x07\n")
        .unwrap();
    assert!(h.wait_for("ERROR boom", 3000).await, "{}", h.screen());
    assert!(
        h.screen().contains("␛]0;PWNED"),
        "hostile bytes are shown as text: {}",
        h.screen()
    );
    std::fs::write(&log, "fresh start\n").unwrap();
    assert!(h.wait_for("fresh start", 3000).await, "{}", h.screen());
    assert!(
        !h.screen().contains("second line"),
        "a truncated log starts over"
    );
    std::fs::remove_file(&log).unwrap();
    assert!(h.wait_for("No log yet", 3000).await, "{}", h.screen());
}

#[tokio::test]
async fn the_log_is_readable_while_the_daemon_is_stopped() {
    let env = test_env("logdown");
    let log = klip_tui::files::log_path(&env.paths);
    std::fs::write(&log, "Klip stopped right after starting (exit status: 1)\n").unwrap();
    let fake = FakeDaemon::new();
    fake.with(|s| s.up = false);
    let mut h = Harness::new(fake.clone(), env, 100, 30);
    assert!(h.wait_for("Klip is not running", 3000).await);
    h.text("6");
    assert!(
        h.wait_for("stopped right after starting", 3000).await,
        "{}",
        h.screen()
    );
}

#[tokio::test]
async fn editing_a_setting_and_saving_writes_a_valid_private_file() {
    use std::os::unix::fs::PermissionsExt;
    let env = test_env("settingsave");
    let cfg_path = env.paths.config_file();
    // The fake daemon runs the device name "Laptop": start from a file that says the same.
    std::fs::write(&cfg_path, r#"{"device_name":"Laptop"}"#).unwrap();
    let fake = FakeDaemon::new();
    let mut h = Harness::new(fake.clone(), env, 100, 30);
    assert!(h.wait_for("ACTIVE", 3000).await);
    h.text("7");
    assert!(
        h.wait_for("Everything is saved and in use", 3000).await,
        "{}",
        h.screen()
    );
    h.key(KeyCode::Enter); // edit the device name
    for _ in 0..30 {
        h.key(KeyCode::Backspace);
    }
    h.text("Desk laptop");
    h.key(KeyCode::Enter);
    assert!(h.wait_for("unsaved changes", 1000).await, "{}", h.screen());
    h.text("s");
    assert!(
        h.wait_for("Saved. Restart Klip", 3000).await,
        "{}",
        h.screen()
    );
    let text = std::fs::read_to_string(&cfg_path).unwrap();
    assert!(text.contains("Desk laptop"), "{text}");
    assert_eq!(
        std::fs::metadata(&cfg_path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let back =
        klip_core::DaemonConfig::load(&klip_core::KlipPaths::from_dir(cfg_path.parent().unwrap()))
            .unwrap();
    assert_eq!(back.device_name, "Desk laptop");
    // The fake daemon reports "Laptop", so the saved name differs from what runs.
    assert!(
        h.wait_for("the running Klip uses other settings", 2000)
            .await,
        "{}",
        h.screen()
    );
}

#[tokio::test]
async fn a_malformed_config_is_shown_and_never_overwritten() {
    let env = test_env("settingbad");
    std::fs::write(env.paths.config_file(), "{ this is not json").unwrap();
    let before = std::fs::read_to_string(env.paths.config_file()).unwrap();
    let path = env.paths.config_file();
    let fake = FakeDaemon::new();
    let mut h = Harness::new(fake.clone(), env, 100, 30);
    assert!(h.wait_for("ACTIVE", 3000).await);
    h.text("7");
    assert!(
        h.wait_for("config.json cannot be read", 3000).await,
        "{}",
        h.screen()
    );
    h.text("s");
    h.pump(300).await;
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        before,
        "the broken file must stay as it was"
    );
    assert!(h.screen().contains("fix it by hand") || h.screen().contains("left untouched"));
}

#[tokio::test]
async fn restarting_stops_waits_and_starts_the_daemon_again() {
    let fake = FakeDaemon::new();
    let mut h = harness(&fake, "restart");
    assert!(h.wait_for("ACTIVE", 3000).await);
    h.text("7");
    for _ in 0..7 {
        h.key(KeyCode::Down); // to the "Apply" row
    }
    h.key(KeyCode::Enter);
    assert!(h.screen().contains("Restart Klip?"), "{}", h.screen());
    h.text("y");
    assert!(
        h.wait_for("Klip restarted (PID 4242)", 8000).await,
        "{}",
        h.screen()
    );
    let calls = fake.calls();
    let stop = calls
        .iter()
        .position(|c| c == "stop_daemon")
        .expect("stopped");
    let start = calls
        .iter()
        .position(|c| c == "start_daemon")
        .expect("started");
    assert!(stop < start, "{calls:?}");
}

#[tokio::test]
async fn auto_start_installs_when_missing() {
    let env = test_env("settingsvc");
    let fake = FakeDaemon::new();
    let mut h = Harness::new(fake.clone(), env, 100, 30);
    assert!(h.wait_for("ACTIVE", 3000).await);
    h.text("7");
    for _ in 0..6 {
        h.key(KeyCode::Down); // to the "Auto-start" row
    }
    assert!(h.wait_for("Auto-start", 1000).await);
    // The harness cannot assume what the developer's home contains: only check the call path when the
    // row says it can install.
    if h.wait_for("not installed", 1500).await {
        h.key(KeyCode::Enter);
        assert!(
            h.wait_for("Auto-start installed", 3000).await,
            "{}",
            h.screen()
        );
        assert!(fake.calls().iter().any(|c| c == "install_service"));
    }
}
