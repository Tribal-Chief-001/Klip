//! The tray on D-Bus: the icon, its menu, and the loop that keeps it true to the daemon.
use super::controller::{self, Cmd, Out};
use super::icons;
use super::model::{menu_plan, Action, Entry, Summary};
use ksni::menu::{CheckmarkItem, StandardItem, SubMenu};
use ksni::{MenuItem, OfflineReason, ToolTip, Tray, TrayMethods};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

/// With no panel to hold the icon after this long, say so once (`KLIP_TRAY_HOST_WAIT_SECS` for tests).
fn host_wait() -> Duration {
    let secs = std::env::var("KLIP_TRAY_HOST_WAIT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    Duration::from_secs(secs)
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

pub async fn run() -> Result<(), String> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .ok_or("cannot find your home directory")?;
    if let Err(e) = icons::install(&data_home) {
        eprintln!("cannot write the tray icons: {e}");
    }
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (out_tx, mut out_rx) = unbounded_channel();
    let host_missing = Arc::new(AtomicBool::new(false));
    let tray = KlipTray {
        summary: Summary::stopped(),
        tx: cmd_tx,
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

    let controller = tokio::spawn(controller::run(cmd_rx, out_tx));
    while let Some(out) = out_rx.recv().await {
        match out {
            Out::Summary(summary) => {
                if handle.update(move |t| t.summary = summary).await.is_none() {
                    break; // the service is gone
                }
            }
            Out::Problem(text) => klip_daemon::notify::say(&text),
        }
    }
    controller.abort();
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
}
