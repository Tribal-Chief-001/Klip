//! The platform-neutral tray controller against a real isolated daemon, with no desktop at all:
//! commands go in, summaries and problems come out.
#![cfg(unix)]
mod common;
use common::*;
use klip_cli::tray::controller::{run, Cmd, Out};
use klip_cli::tray::model::{Action, State, Summary};
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

async fn next_summary(
    out: &mut UnboundedReceiver<Out>,
    what: &str,
    mut cond: impl FnMut(&Summary) -> bool,
) -> Summary {
    let end = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        match tokio::time::timeout_at(end, out.recv()).await {
            Ok(Some(Out::Summary(s))) if cond(&s) => return s,
            Ok(Some(_)) => {}
            Ok(None) => panic!("the controller ended while waiting for {what}"),
            Err(_) => panic!("timed out waiting for {what}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_picture_follows_the_daemon_from_stopped_to_running_and_commands_reach_it() {
    // This file's other test never reads the environment, so changing it here cannot race it.
    let nobody = std::env::temp_dir().join(format!("klip-ctl-nobody-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&nobody);
    std::fs::create_dir_all(&nobody).unwrap();
    std::env::set_var("KLIP_CONFIG_DIR", &nobody);
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (out_tx, mut out_rx) = unbounded_channel();
    let controller = tokio::spawn(run(cmd_rx, out_tx));

    // No daemon: the first picture is "stopped".
    next_summary(&mut out_rx, "a stopped picture", |s| {
        s.state == State::Stopped
    })
    .await;

    // A daemon comes up (it may take a moment); the next refresh sees it.
    let d = Isolated::start("controller", LAN_ONLY);
    std::env::set_var("KLIP_CONFIG_DIR", &d.dir);
    cmd_tx.send(Cmd::Refresh).unwrap();
    next_summary(&mut out_rx, "the first picture of a running daemon", |s| {
        s.state == State::NoDevices
    })
    .await;

    // Switch automatic images on, then off.
    cmd_tx.send(Cmd::Do(Action::SetAuto(true))).unwrap();
    let s = next_summary(&mut out_rx, "auto on", |s| s.auto_images).await;
    assert!(s.images_known);
    cmd_tx.send(Cmd::Do(Action::SetAuto(false))).unwrap();
    next_summary(&mut out_rx, "auto off", |s| !s.auto_images).await;

    // Pause and resume.
    cmd_tx.send(Cmd::Do(Action::Pause(600))).unwrap();
    next_summary(&mut out_rx, "paused", |s| s.state == State::Paused).await;
    cmd_tx.send(Cmd::Do(Action::Resume)).unwrap();
    next_summary(&mut out_rx, "resumed", |s| s.state == State::NoDevices).await;

    // Nothing is waiting: the daemon refuses, the controller says so once and keeps it in the menu.
    cmd_tx.send(Cmd::Do(Action::SendImage)).unwrap();
    let mut problem = None;
    let mut shown = false;
    let end = tokio::time::Instant::now() + Duration::from_secs(20);
    while problem.is_none() || !shown {
        match tokio::time::timeout_at(end, out_rx.recv()).await {
            Ok(Some(Out::Problem(text))) => problem = Some(text),
            Ok(Some(Out::Summary(s))) => shown |= s.problem.is_some(),
            _ => panic!("no problem reported: {problem:?} {shown}"),
        }
    }
    assert!(problem.unwrap().to_lowercase().contains("image"));

    // Opening the menu asks for a fresh picture; it must not end or upset anything.
    cmd_tx.send(Cmd::Refresh).unwrap();

    // Quit ends the controller and closes the output.
    cmd_tx.send(Cmd::Do(Action::Quit)).unwrap();
    tokio::time::timeout(Duration::from_secs(10), controller)
        .await
        .expect("the controller ends on Quit")
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_controller_ends_when_nobody_listens_or_nobody_asks() {
    // The output is gone (the platform closed): the controller notices at its next picture or action.
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (out_tx, out_rx) = unbounded_channel();
    drop(out_rx);
    let controller = tokio::spawn(run(cmd_rx, out_tx));
    cmd_tx.send(Cmd::Refresh).unwrap();
    tokio::time::timeout(Duration::from_secs(20), controller)
        .await
        .expect("the controller ends when its output is closed")
        .unwrap();

    // The command side is gone (the platform dropped its sender): it ends at once.
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (out_tx, _out_rx) = unbounded_channel();
    let controller = tokio::spawn(run(cmd_rx, out_tx));
    drop(cmd_tx);
    tokio::time::timeout(Duration::from_secs(10), controller)
        .await
        .expect("the controller ends when nobody can ask it anything")
        .unwrap();
}
