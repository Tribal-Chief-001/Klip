//! The tray on D-Bus: the icon, its menu, and the loop that keeps it true to the daemon.
use super::icons;
use super::model::{menu_plan, summarize, Action, Entry, Summary};
use crate::api::IpcDaemonApi;
use crate::client::{ClientError, DaemonClient, OLD_DAEMON_REQUEST_CAP};
use klip_daemon::config::user_paths;
use klip_proto::ipc::{IpcRequest, IpcResponse};
use klip_tui::api::DaemonApi;
use klip_tui::files::ServiceState;
use ksni::menu::{CheckmarkItem, StandardItem, SubMenu};
use ksni::{MenuItem, OfflineReason, ToolTip, Tray, TrayMethods};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

/// How often the picture is renewed when nothing happens (it is also renewed when the menu opens).
const POLL: Duration = Duration::from_secs(15);
/// With no panel to hold the icon after this long, say so once (`KLIP_TRAY_HOST_WAIT_SECS` for tests).
fn host_wait() -> Duration {
    let secs = std::env::var("KLIP_TRAY_HOST_WAIT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    Duration::from_secs(secs)
}

enum Cmd {
    Refresh,
    Do(Action),
    /// A finished action (so the same one can be started again).
    Done(Action),
    /// A finished poll; `None` = the daemon did not give a usable answer (keep what is shown).
    Fetched(Option<Summary>),
}

struct KlipTray {
    summary: Summary,
    tx: UnboundedSender<Cmd>,
    /// The panel's watcher is not there (set when registering fails or it disappears).
    host_missing: Arc<AtomicBool>,
}

/// `_` marks an access key in a D-Bus menu label; a device name may contain one.
fn label(text: &str) -> String {
    text.replace('_', "__")
}

impl KlipTray {
    fn entry(&self, e: &Entry) -> MenuItem<Self> {
        match e {
            Entry::Separator => MenuItem::Separator,
            Entry::Label(text) => StandardItem {
                label: label(text),
                enabled: false,
                ..Default::default()
            }
            .into(),
            Entry::Item {
                label: text,
                enabled,
                action,
            } => {
                let action = *action;
                StandardItem {
                    label: label(text),
                    enabled: *enabled,
                    activate: Box::new(move |t: &mut Self| {
                        let _ = t.tx.send(Cmd::Do(action));
                    }),
                    ..Default::default()
                }
                .into()
            }
            Entry::Check {
                label: text,
                checked,
                action,
            } => {
                let action = *action;
                CheckmarkItem {
                    label: label(text),
                    checked: *checked,
                    activate: Box::new(move |t: &mut Self| {
                        let _ = t.tx.send(Cmd::Do(action));
                    }),
                    ..Default::default()
                }
                .into()
            }
            Entry::Submenu { label: text, items } => SubMenu {
                label: label(text),
                submenu: items.iter().map(|i| self.entry(i)).collect(),
                ..Default::default()
            }
            .into(),
        }
    }
}

impl Tray for KlipTray {
    /// A left click opens the same menu as a right click.
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        "klip".into()
    }

    fn title(&self) -> String {
        "Klip".into()
    }

    fn icon_name(&self) -> String {
        icons::name_for(self.summary.state, self.summary.badge)
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: self.summary.title.clone(),
            description: self.summary.tooltip_body(),
            icon_name: String::new(),
            icon_pixmap: Vec::new(),
        }
    }

    /// A middle click sends the waiting image.
    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send(Cmd::Do(Action::SendImage));
    }

    /// The picture is renewed as the menu opens.
    fn menu_about_to_show(&mut self) {
        let _ = self.tx.send(Cmd::Refresh);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        menu_plan(&self.summary)
            .iter()
            .map(|e| self.entry(e))
            .collect()
    }

    fn watcher_online(&self) {
        self.host_missing.store(false, Ordering::SeqCst);
    }

    fn watcher_offline(&self, _reason: OfflineReason) -> bool {
        self.host_missing.store(true, Ordering::SeqCst);
        true // keep waiting: the panel may start after us
    }
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
    Some(summarize(Some(&status), images.as_ref()))
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
            super::open_terminal(&exe)
        }
        Action::Quit => Ok(()),
    }
}

async fn stop_signal() {
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

fn spawn_fetch(tx: &UnboundedSender<Cmd>) {
    let tx = tx.clone();
    tokio::spawn(async move {
        let _ = tx.send(Cmd::Fetched(fetch(&DaemonClient::for_current_user()).await));
    });
}

pub async fn run() -> Result<(), String> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .ok_or("cannot find your home directory")?;
    if let Err(e) = icons::install(&data_home) {
        eprintln!("cannot write the tray icons: {e}");
    }
    let (tx, mut rx) = unbounded_channel();
    let host_missing = Arc::new(AtomicBool::new(false));
    let tray = KlipTray {
        summary: Summary::stopped(),
        tx: tx.clone(),
        host_missing: host_missing.clone(),
    };
    // Wait for the panel's watcher instead of failing when it starts after us (at login).
    let handle = tray
        .assume_sni_available(true)
        .spawn()
        .await
        .map_err(|e| format!("cannot show the tray icon: {e}"))?;

    tokio::spawn(async move {
        tokio::time::sleep(host_wait()).await;
        if host_missing.load(Ordering::SeqCst) {
            klip_daemon::notify::say(
                "There is no system tray to show the Klip icon. Use the terminal app: klip",
            );
        }
    });

    let mut shown = Summary::stopped();
    let mut running: HashSet<Action> = HashSet::new();
    let mut fetching = true;
    spawn_fetch(&tx);
    let mut tick = tokio::time::interval(POLL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let stop = stop_signal();
    tokio::pin!(stop);
    loop {
        let mut refresh = false;
        tokio::select! {
            _ = tick.tick() => refresh = true,
            _ = &mut stop => break,
            cmd = rx.recv() => match cmd {
                None | Some(Cmd::Do(Action::Quit)) => break,
                Some(Cmd::Refresh) => refresh = true,
                Some(Cmd::Done(action)) => {
                    running.remove(&action);
                    refresh = true;
                }
                // The same action twice (a double click) runs once.
                Some(Cmd::Do(action)) => {
                    if running.insert(action) {
                        let done = tx.clone();
                        tokio::spawn(async move {
                            let client = DaemonClient::for_current_user();
                            if let Err(text) = perform(&client, action).await {
                                klip_daemon::notify::say(&text);
                            }
                            let _ = done.send(Cmd::Done(action));
                        });
                    }
                }
                Some(Cmd::Fetched(now)) => {
                    fetching = false;
                    if let Some(now) = now.filter(|n| *n != shown) {
                        shown = now.clone();
                        if handle.update(move |t| t.summary = now).await.is_none() {
                            break; // the service is gone
                        }
                    }
                }
            },
        }
        // A poll never waits for another one, and never blocks a click or Quit.
        if refresh && !fetching {
            fetching = true;
            spawn_fetch(&tx);
        }
    }
    let _ = handle.shutdown().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn underscores_in_a_label_are_not_access_keys() {
        assert_eq!(label("Vivo_V2253 and Pad"), "Vivo__V2253 and Pad");
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
