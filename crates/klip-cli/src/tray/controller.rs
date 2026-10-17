//! The tray's brain, with no window system in it: polling, the daemon calls behind every menu action,
//! one run per action at a time, and the problem line. A platform shows what it sends (`Out`) and
//! forwards clicks (`Cmd`); nothing here knows about D-Bus, AppKit or Win32.
use super::model::{summarize, Action, Summary};
use crate::api::IpcDaemonApi;
use crate::client::{ClientError, DaemonClient, OLD_DAEMON_REQUEST_CAP};
use klip_daemon::config::user_paths;
use klip_proto::ipc::{IpcRequest, IpcResponse};
use klip_tui::api::DaemonApi;
use klip_tui::files::ServiceState;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// How often the picture is renewed when nothing happens (a platform also asks when its menu opens).
const POLL: Duration = Duration::from_secs(15);
/// How long a failed action stays as the first line of the menu.
const PROBLEM_FOR: Duration = Duration::from_secs(30);

/// What a platform asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    /// Renew the picture now (the menu is opening).
    Refresh,
    Do(Action),
}

/// What a platform shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Out {
    /// The picture changed (it already carries the problem line).
    Summary(Summary),
    /// An action failed: a platform with notifications says it once.
    Problem(String),
}

enum Event {
    Done(Action, Result<(), String>),
    Fetched(Option<Summary>),
}

/// The daemon's answers as one picture. `Some(stopped)` when nobody listens; `None` when the daemon
/// answered badly or too slowly (a busy daemon is not a stopped one: the caller keeps what is shown).
async fn fetch(client: &DaemonClient) -> Option<Summary> {
    let status = match client
        .request(&IpcRequest::GetStatus, OLD_DAEMON_REQUEST_CAP)
        .await
    {
        Ok(IpcResponse::Status(s)) => s,
        Err(ClientError::Down(_)) => return Some(Summary::stopped()),
        _ => return None,
    };
    let images = match client
        .request(&IpcRequest::GetImageState, OLD_DAEMON_REQUEST_CAP)
        .await
    {
        Ok(IpcResponse::ImageState(i)) => Some(i),
        _ => None, // an older daemon does not know images
    };
    let mut summary = summarize(Some(&status), images.as_ref());
    // Files: the line while they move, and whether this daemon and this system can send one from the menu.
    if let Ok(IpcResponse::Transfers(rows)) = client
        .request(&IpcRequest::GetTransfers, OLD_DAEMON_REQUEST_CAP)
        .await
    {
        summary.files = super::model::transfer_line(&rows);
        summary.can_send_file = super::picker_available();
    }
    Some(summary)
}

fn unit_managed() -> bool {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    cfg!(target_os = "linux")
        && std::env::var_os("KLIP_CONFIG_DIR").is_none()
        && matches!(
            klip_tui::files::service_state(home.as_deref()),
            ServiceState::Installed(_)
        )
}

/// `systemctl --user start klip.service`; `KLIP_SYSTEMCTL` replaces the program (tests).
fn systemctl_start() -> Result<bool, String> {
    let program = std::env::var("KLIP_SYSTEMCTL").unwrap_or_else(|_| "systemctl".to_string());
    std::process::Command::new(program)
        .args(["--user", "start", "klip.service"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .map_err(|e| format!("cannot run systemctl: {e}"))
}

async fn start_daemon() -> Result<(), String> {
    let api = IpcDaemonApi::new(DaemonClient::for_current_user(), user_paths());
    if api.status().await.is_ok() {
        return Ok(());
    }
    if !unit_managed() {
        return api.start_daemon().await.map(|_| ()).map_err(|e| e.plain());
    }
    let ok = tokio::task::spawn_blocking(systemctl_start)
        .await
        .map_err(|e| e.to_string())??;
    if !ok {
        return Err("systemd could not start klip.service".to_string());
    }
    for _ in 0..120 {
        if api.status().await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err("Klip did not come up within 30 s".to_string())
}

/// What a `Sent` answer means for the person: `Some(text)` when something went wrong.
pub fn sent_problem(delivered: &[String], failed: &[(String, String)]) -> Option<String> {
    if delivered.is_empty() && failed.is_empty() {
        return Some("No devices are linked yet.".to_string());
    }
    let parts: Vec<String> = failed
        .iter()
        .map(|(name, why)| format!("{name} did not get it: {}", super::model::plain_error(why)))
        .collect();
    (!parts.is_empty()).then(|| parts.join(". "))
}

/// Where received files land: `receive_dir` from `config.json`, else the default Downloads folder.
fn receive_folder() -> std::path::PathBuf {
    let paths = klip_daemon::config::user_paths();
    klip_daemon::DaemonConfig::load(&paths)
        .ok()
        .and_then(|c| c.receive_dir)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(klip_daemon::default_receive_dir)
}

/// Do one menu action. `Err(text)` is shown as a notification.
async fn perform(client: &DaemonClient, action: Action) -> Result<(), String> {
    let call = |req: IpcRequest| async move {
        client
            .request(&req, OLD_DAEMON_REQUEST_CAP)
            .await
            .map_err(|e| e.to_string())
    };
    match action {
        Action::Start => start_daemon().await,
        Action::SendImage => match call(IpcRequest::SendPendingImage).await? {
            IpcResponse::Sent {
                delivered, failed, ..
            } => sent_problem(&delivered, &failed).map_or(Ok(()), Err),
            IpcResponse::Error(e) => Err(e),
            other => Err(format!("unexpected reply: {other:?}")),
        },
        Action::SetAuto(enabled) => match call(IpcRequest::SetAutoImages { enabled }).await? {
            IpcResponse::ImageState(_) => Ok(()),
            IpcResponse::Error(e) => Err(e),
            other => Err(format!("unexpected reply: {other:?}")),
        },
        Action::Pause(secs) => match call(IpcRequest::Pause {
            duration_secs: secs,
        })
        .await?
        {
            IpcResponse::Paused { .. } => Ok(()),
            IpcResponse::Error(e) => Err(e),
            other => Err(format!("unexpected reply: {other:?}")),
        },
        Action::Resume => match call(IpcRequest::Resume).await? {
            IpcResponse::Resumed => Ok(()),
            IpcResponse::Error(e) => Err(e),
            other => Err(format!("unexpected reply: {other:?}")),
        },
        Action::OpenTerminal => {
            let exe = super::exe_path().map_err(|e| e.to_string())?;
            // osascript/cmd are waited for (their exit status is shown): not on an async worker.
            tokio::task::spawn_blocking(move || super::open_terminal(&exe))
                .await
                .map_err(|e| e.to_string())?
        }
        Action::OpenFolder => super::open_folder(&receive_folder()),
        Action::SendFile => {
            let paths = tokio::task::spawn_blocking(super::pick_files)
                .await
                .map_err(|e| e.to_string())??;
            let mut problems = Vec::new();
            for path in paths {
                match call(IpcRequest::SendFile { path, to: None }).await? {
                    IpcResponse::TransfersStarted(_) => {}
                    IpcResponse::Error(e) => problems.push(e),
                    other => problems.push(format!("unexpected reply: {other:?}")),
                }
            }
            problems.first().map_or(Ok(()), |p| Err(p.clone()))
        }
        Action::Quit => Ok(()),
    }
}

async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = term.recv() => {}
                    _ = tokio::signal::ctrl_c() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn spawn_fetch(events: &UnboundedSender<Event>) {
    let events = events.clone();
    tokio::spawn(async move {
        let _ = events.send(Event::Fetched(
            fetch(&DaemonClient::for_current_user()).await,
        ));
    });
}

/// Run until `Do(Quit)`, a stop signal, the command channel closing, or the output channel closing.
/// At most one poll runs at a time; a refresh asked for meanwhile is remembered and runs right after it.
#[derive(Default)]
struct RefreshGate {
    fetching: bool,
    pending: bool,
}

impl RefreshGate {
    /// A refresh is wanted: `true` when a poll should start now.
    fn request(&mut self) -> bool {
        if self.fetching {
            self.pending = true;
            false
        } else {
            self.fetching = true;
            true
        }
    }

    /// The poll ended: `true` when another one was asked for while it ran.
    fn finished(&mut self) -> bool {
        self.fetching = false;
        if self.pending {
            self.pending = false;
            self.fetching = true;
            true
        } else {
            false
        }
    }
}

pub async fn run(mut cmds: UnboundedReceiver<Cmd>, out: UnboundedSender<Out>) {
    let (events, mut inbox) = unbounded_channel::<Event>();
    let mut base = Summary::stopped();
    let mut have_picture = false;
    let mut problem: Option<(String, Instant)> = None;
    let mut last: Option<Summary> = None;
    let mut running: HashSet<Action> = HashSet::new();
    let mut gate = RefreshGate::default();
    let mut tick = tokio::time::interval(POLL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let stop = stop_signal();
    tokio::pin!(stop);
    loop {
        let mut refresh = false;
        tokio::select! {
            _ = tick.tick() => refresh = true,
            _ = &mut stop => break,
            cmd = cmds.recv() => match cmd {
                None | Some(Cmd::Do(Action::Quit)) => break,
                Some(Cmd::Refresh) => refresh = true,
                // The same action twice (a double click) runs once.
                Some(Cmd::Do(action)) => {
                    if running.insert(action) {
                        let events = events.clone();
                        tokio::spawn(async move {
                            let client = DaemonClient::for_current_user();
                            let _ = events.send(Event::Done(action, perform(&client, action).await));
                        });
                    }
                }
            },
            event = inbox.recv() => match event {
                None => break,
                Some(Event::Done(action, result)) => {
                    running.remove(&action);
                    if let Err(text) = result {
                        problem = Some((text.clone(), Instant::now()));
                        if out.send(Out::Problem(text)).is_err() {
                            break;
                        }
                    }
                    refresh = true;
                }
                Some(Event::Fetched(picture)) => {
                    // A refresh asked for during this poll (after an action, say) runs now, not 15 s later.
                    if gate.finished() {
                        spawn_fetch(&events);
                    }
                    if let Some(picture) = picture {
                        base = picture;
                        have_picture = true;
                    }
                }
            },
        }
        // A poll never waits for another one, and never blocks a click or Quit.
        if refresh && gate.request() {
            spawn_fetch(&events);
        }
        if problem
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() > PROBLEM_FOR)
        {
            problem = None;
        }
        if have_picture {
            let mut view = base.clone();
            view.problem = problem.as_ref().map(|(text, _)| text.clone());
            if last.as_ref() != Some(&view) {
                if out.send(Out::Summary(view.clone())).is_err() {
                    break;
                }
                last = Some(view);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refresh_asked_for_during_a_poll_runs_right_after_it_and_only_once() {
        let mut gate = RefreshGate::default();
        assert!(gate.request(), "idle: start at once");
        assert!(!gate.request(), "a poll is running: remember it");
        assert!(!gate.request(), "asking twice is still one more poll");
        assert!(gate.finished(), "the remembered refresh runs now");
        assert!(!gate.finished(), "and the next end has nothing left");
        assert!(gate.request(), "idle again");
    }

    #[test]
    fn a_send_is_a_problem_only_when_nobody_got_it_or_somebody_did_not() {
        assert_eq!(sent_problem(&["Vivo".into()], &[]), None);
        assert!(sent_problem(&[], &[]).unwrap().contains("No devices"));
        let p = sent_problem(
            &["Vivo".into()],
            &[("Pad".into(), "cannot reach peer: timed out".into())],
        )
        .unwrap();
        assert!(p.starts_with("Pad did not get it") && p.contains("reconnects by itself"));
    }
}
