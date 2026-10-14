//! Characterization tests: the CLI's help texts and its behaviour without a daemon or a terminal,
//! recorded before the TUI work started. A change here must be deliberate and visible in the diff.
//! Regenerate with `UPDATE_GOLDEN=1 cargo test -p klip-cli --test cli_golden` and review `git diff`.
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn klip(args: &[&str]) -> (String, i32) {
    let dir = std::env::temp_dir().join(format!("klip-golden-{}", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_klip"))
        .args(args)
        .stdin(Stdio::null())
        .env("KLIP_CONFIG_DIR", dir)
        .env_remove("TERM")
        .output()
        .expect("run klip");
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (text, out.status.code().unwrap_or(-1))
}

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

fn check(name: &str, args: &[&str], want_code: i32) {
    let (text, code) = klip(args);
    assert_eq!(code, want_code, "exit code of `klip {}`", args.join(" "));
    let path = golden_path(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing {}; run with UPDATE_GOLDEN=1", path.display()));
    assert_eq!(text, want, "output of `klip {}` changed", args.join(" "));
}

#[test]
fn help_top_level() {
    check("help.txt", &["--help"], 0);
}

#[test]
fn help_of_every_subcommand() {
    for (name, args) in [
        ("help_start.txt", vec!["start", "--help"]),
        ("help_stop.txt", vec!["stop", "--help"]),
        ("help_status.txt", vec!["status", "--help"]),
        ("help_pair.txt", vec!["pair", "--help"]),
        ("help_unpair.txt", vec!["unpair", "--help"]),
        ("help_pause.txt", vec!["pause", "--help"]),
        ("help_resume.txt", vec!["resume", "--help"]),
        ("help_history.txt", vec!["history", "--help"]),
        ("help_send.txt", vec!["send", "--help"]),
        (
            "help_install_service.txt",
            vec!["install-service", "--help"],
        ),
        ("help_share.txt", vec!["share", "--help"]),
        ("help_images.txt", vec!["images", "--help"]),
        ("help_tray.txt", vec!["tray", "--help"]),
    ] {
        check(name, &args, 0);
    }
}

#[test]
fn no_arguments_without_a_terminal_prints_help_and_exits_2() {
    check("no_args.txt", &[], 2);
}

#[test]
fn commands_that_need_the_daemon_say_so_when_it_is_not_running() {
    for (name, args) in [
        ("down_status.txt", vec!["status"]),
        ("down_stop.txt", vec!["stop"]),
        ("down_history.txt", vec!["history"]),
        ("down_send.txt", vec!["send", "hello"]),
        ("down_pause.txt", vec!["pause"]),
        ("down_resume.txt", vec!["resume"]),
        ("down_unpair.txt", vec!["unpair", "someone"]),
        ("down_share.txt", vec!["share"]),
        ("down_images.txt", vec!["images"]),
    ] {
        check(name, &args, 1);
    }
}
