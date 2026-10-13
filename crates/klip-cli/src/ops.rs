//! Operations built from several control calls, shared by the subcommands and the TUI.
use crate::client::{ClientError, DaemonClient, OLD_DAEMON_REQUEST_CAP};
use klip_daemon::KlipPaths;
use klip_proto::ipc::{IpcRequest, IpcResponse, IPC_PROTOCOL_VERSION};
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// How many bytes one request may carry: asked from the daemon (protocol 2), 8 MiB for older daemons.
pub async fn request_cap(client: &DaemonClient) -> usize {
    match client
        .request(
            &IpcRequest::Hello {
                client_protocol: IPC_PROTOCOL_VERSION,
            },
            OLD_DAEMON_REQUEST_CAP,
        )
        .await
    {
        Ok(IpcResponse::Hello(h)) => h.max_request_bytes,
        _ => OLD_DAEMON_REQUEST_CAP,
    }
}

/// Remove every linked device: one `UnpairAll` call, or, for a daemon that does not know it, one
/// `Unpair` per device. Returns `(removed names, (name, reason) failures)`.
pub async fn unpair_all_compat(
    client: &DaemonClient,
) -> Result<(Vec<String>, Vec<(String, String)>), ClientError> {
    if let Ok(IpcResponse::UnpairedAll { removed, failed }) = client
        .request(&IpcRequest::UnpairAll, OLD_DAEMON_REQUEST_CAP)
        .await
    {
        return Ok((removed, failed));
    }
    let peers = match client
        .request(&IpcRequest::GetStatus, OLD_DAEMON_REQUEST_CAP)
        .await?
    {
        IpcResponse::Status(s) => s.peers,
        IpcResponse::Error(e) => return Err(ClientError::Io(e)),
        other => return Err(ClientError::Io(format!("unexpected reply: {other:?}"))),
    };
    let mut removed = Vec::new();
    let mut failed = Vec::new();
    for peer in peers {
        let selector = peer.node_id.fingerprint();
        match client
            .request(&IpcRequest::Unpair { selector }, OLD_DAEMON_REQUEST_CAP)
            .await?
        {
            IpcResponse::Unpaired { name } => removed.push(name),
            IpcResponse::Error(e) => failed.push((peer.name, e)),
            other => failed.push((peer.name, format!("unexpected reply: {other:?}"))),
        }
    }
    Ok((removed, failed))
}

pub struct Spawned {
    pub child: std::process::Child,
    pub log_path: PathBuf,
}

/// A log file only the owner can read.
pub fn open_private_log(path: &Path) -> std::io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
}

/// Start `klip start --foreground` as a detached background process that logs into the Klip directory.
pub fn spawn_daemon_process(paths: &KlipPaths) -> Result<Spawned, String> {
    let log_path = paths.dir().join("daemon.log");
    let log = open_private_log(&log_path).map_err(|e| e.to_string())?;
    let log_err = log.try_clone().map_err(|e| e.to_string())?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("start")
        .arg("--foreground")
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log_err);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let child = cmd.spawn().map_err(|e| e.to_string())?;
    Ok(Spawned { child, log_path })
}

/// The last `lines` lines of a file, or an empty string when it cannot be read.
pub fn log_tail(path: &Path, lines: usize) -> String {
    std::fs::read_to_string(path)
        .map(|t| {
            let all: Vec<&str> = t.lines().collect();
            all[all.len().saturating_sub(lines)..].join("\n")
        })
        .unwrap_or_default()
}

/// Write the auto-start unit for this OS. Returns the text to show (without the leading tick), or
/// `None` when this OS/environment has nothing to install (for example no `$HOME`).
pub fn install_service() -> Result<Option<String>, String> {
    #[cfg(target_os = "linux")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            let dest = PathBuf::from(home).join(".config/systemd/user/klip.service");
            klip_daemon::generate_systemd_unit_file(&dest).map_err(|e| e.to_string())?;
            return Ok(Some(format!(
                "Installed systemd unit: {}\nNow run: systemctl --user enable --now klip.service",
                dest.display()
            )));
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            let dest = PathBuf::from(home).join("Library/LaunchAgents/com.klip.daemon.plist");
            klip_daemon::generate_launchd_plist_file(&dest).map_err(|e| e.to_string())?;
            return Ok(Some(format!(
                "Installed launchd agent: {}\nNow run: launchctl load {}",
                dest.display(),
                dest.display()
            )));
        }
    }
    #[cfg(target_os = "windows")]
    {
        let dest = klip_daemon::get_klip_dir().join("register-service.ps1");
        klip_daemon::generate_windows_service_script(&dest).map_err(|e| e.to_string())?;
        return Ok(Some(format!(
            "Wrote {}\nRun it in PowerShell: powershell -ExecutionPolicy Bypass -File \"{}\"",
            dest.display(),
            dest.display()
        )));
    }
    #[allow(unreachable_code)]
    Ok(None)
}
