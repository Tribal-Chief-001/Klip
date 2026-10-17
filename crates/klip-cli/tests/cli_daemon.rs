//! The real `klip` binary against an isolated daemon (own config dir, in-memory clipboard).
mod common;
use common::*;

#[test]
fn history_clear_forgets_the_clips() {
    let d = Isolated::start("hc", LAN_ONLY);
    assert!(d
        .run(&["send", "remember-this-text"], None)
        .status
        .success());
    assert!(text(&d.run(&["history"], None)).contains("remember-this-text"));
    let cleared = d.run(&["history", "clear", "--yes"], None);
    assert!(cleared.status.success(), "{}", text(&cleared));
    assert!(
        text(&d.run(&["history"], None)).contains("No clips"),
        "history should be empty"
    );
}

#[test]
fn unpair_all_with_no_devices_says_so() {
    let d = Isolated::start("ua", LAN_ONLY);
    let out = d.run(&["unpair", "--all", "--yes"], None);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("No linked devices"), "{}", text(&out));
}

#[test]
fn unpair_needs_a_device_or_all_and_not_both() {
    let d = Isolated::start("ub", LAN_ONLY);
    assert!(!d.run(&["unpair"], None).status.success());
    assert!(!d.run(&["unpair", "x", "--all"], None).status.success());
}

#[test]
fn a_9_mb_text_through_stdin_gets_a_normal_answer_not_a_broken_pipe() {
    let d = Isolated::start("big", LAN_ONLY);
    let text_9mb = vec![b'a'; 9_000_000];
    let out = d.run(&["send", "-"], Some(&text_9mb));
    let all = text(&out);
    assert!(!all.contains("Broken pipe"), "{all}");
    assert!(out.status.success(), "{all}");
    assert!(all.contains("No linked devices"), "{all}");
}

#[test]
fn text_over_the_daemons_cap_is_refused_before_it_is_written() {
    let d = Isolated::start(
        "cap",
        r#"{"relay":"lan","lan_discovery":false,"max_clip_bytes":1000}"#,
    );
    let out = d.run(&["send", "-"], Some(&vec![b'b'; 100_000]));
    let all = text(&out);
    assert!(!out.status.success(), "{all}");
    assert!(all.contains("at most 66536"), "{all}");
    assert!(!all.contains("Broken pipe"), "{all}");
}

#[test]
fn share_with_no_copied_image_explains_what_to_do() {
    let d = Isolated::start("sh1", LAN_ONLY);
    let out = d.run(&["share"], None);
    let all = text(&out);
    assert!(!out.status.success(), "{all}");
    assert!(all.contains("no image is waiting"), "{all}");
}

#[test]
fn the_image_mode_can_be_shown_changed_and_is_kept_in_config_json() {
    let d = Isolated::start("im1", LAN_ONLY);
    let shown = text(&d.run(&["images"], None));
    assert!(shown.contains("wait until you send"), "{shown}");
    assert!(shown.contains("klip share"), "{shown}");

    let out = d.run(&["images", "auto"], None);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("sent automatically"), "{}", text(&out));
    let saved = std::fs::read_to_string(d.dir.join("config.json")).unwrap();
    assert!(saved.contains("\"auto_send_images\": true"), "{saved}");
    assert!(text(&d.run(&["images"], None)).contains("sent automatically"));

    let out = d.run(&["images", "manual"], None);
    assert!(out.status.success());
    assert!(text(&out).contains("wait until you send"), "{}", text(&out));
    assert!(std::fs::read_to_string(d.dir.join("config.json"))
        .unwrap()
        .contains("\"auto_send_images\": false"));

    let bad = d.run(&["images", "sometimes"], None);
    assert!(!bad.status.success());
    assert!(text(&bad).contains("possible values"), "{}", text(&bad));
}

#[test]
fn many_clients_at_once_all_get_an_answer() {
    // On Windows one pipe instance serves one client at a time: the others must wait their turn
    // (a busy pipe is retried), not be told the daemon is down.
    let d = Isolated::start("parallel", LAN_ONLY);
    let env = d.env_pairs();
    let handles: Vec<_> = (0..16)
        .map(|_| {
            let env = env.clone();
            std::thread::spawn(move || {
                std::process::Command::new(env!("CARGO_BIN_EXE_klip"))
                    .arg("status")
                    .envs(env)
                    .stdin(std::process::Stdio::null())
                    .output()
                    .unwrap()
            })
        })
        .collect();
    for h in handles {
        let out = h.join().unwrap();
        assert!(out.status.success(), "{}", text(&out));
        assert!(text(&out).contains("STATE"), "{}", text(&out));
    }
}
