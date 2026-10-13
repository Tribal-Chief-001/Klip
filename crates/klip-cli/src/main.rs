use clap::{Parser, Subcommand};
use klip_cli::client::{ClientError, DaemonClient};
use klip_cli::ops::{request_cap, unpair_all_compat};
use klip_cli::ui::{print_history, print_pair_card, print_pair_request, print_status_card};
use klip_daemon::config::DaemonConfig;
use klip_daemon::engine::DaemonEngine;
use klip_proto::ipc::{IpcRequest, IpcResponse};
use owo_colors::OwoColorize;
use std::io::IsTerminal;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "klip")]
#[command(about = "klip: end-to-end encrypted clipboard sync between your own devices", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the Klip background service
    Start {
        /// Run in the foreground (used by systemd/launchd; shows errors on the terminal)
        #[arg(short, long)]
        foreground: bool,
    },
    /// Stop the background service
    Stop,
    /// Show linked devices, connection state and sync info
    Status,
    /// Link another device. Without an argument: show a QR/ticket for the other device.
    /// With a ticket: join the device that showed it.
    Pair {
        /// Ticket shown by the other device (starts with "klip1")
        ticket: Option<String>,
        /// Put this device's LAN addresses in the ticket (bigger QR, works without internet)
        #[arg(long)]
        lan: bool,
        /// How long the ticket stays valid, in seconds (30-600)
        #[arg(long, default_value = "120")]
        ttl: u64,
    },
    /// Remove a linked device (by name or by the start of its fingerprint), or all of them with --all
    Unpair {
        /// Name or start of the fingerprint of the device to remove
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        device: Option<String>,
        /// Remove every linked device
        #[arg(long)]
        all: bool,
        /// Do not ask for confirmation (with --all)
        #[arg(short, long)]
        yes: bool,
    },
    /// Pause synchronisation
    Pause {
        /// Pause duration in seconds (default: 600s / 10m)
        #[arg(default_value = "600")]
        duration: u64,
    },
    /// Resume synchronisation
    Resume,
    /// Show recently synced clips
    History {
        /// Maximum number of items to show
        #[arg(short, long, default_value = "10")]
        limit: usize,
        #[command(subcommand)]
        action: Option<HistoryAction>,
    },
    /// Send a text to all linked devices right now
    Send {
        /// Text to send, or `-` to read it from standard input (for big or multi-line text:
        /// `cat main.rs | klip send -`)
        text: String,
    },
    /// Install the auto-start service for your OS
    InstallService,
}

#[derive(Subcommand)]
enum HistoryAction {
    /// Forget all clips kept in the history of this device
    Clear {
        /// Do not ask for confirmation
        #[arg(short, long)]
        yes: bool,
    },
}

#[tokio::main]
async fn main() {
    // `klip history | head` must end quietly instead of panicking on a closed pipe.
    #[cfg(unix)]
    // SAFETY: restoring the default disposition of SIGPIPE at startup, before any other thread
    // exists or any signal handler is installed by this program.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("{} {}", "error:".red().bold(), e);
        std::process::exit(1);
    }
}

type AnyResult<T> = Result<T, Box<dyn std::error::Error>>;

async fn rpc(req: IpcRequest) -> AnyResult<IpcResponse> {
    match DaemonClient::send_request(&req).await {
        Ok(IpcResponse::Error(e)) => Err(e.into()),
        Ok(other) => Ok(other),
        Err(e) => Err(e.into()),
    }
}

async fn run(cli: Cli) -> AnyResult<()> {
    match cli.command {
        Commands::Start { foreground } => start(foreground).await?,
        Commands::Stop => match rpc(IpcRequest::StopDaemon).await? {
            IpcResponse::DaemonStopping => println!("{}", "✓ Klip stopped.".green()),
            other => return Err(format!("unexpected reply: {other:?}").into()),
        },
        Commands::Status => match rpc(IpcRequest::GetStatus).await? {
            IpcResponse::Status(info) => print_status_card(&info),
            other => return Err(format!("unexpected reply: {other:?}").into()),
        },
        Commands::Pair {
            ticket: Some(t), ..
        } => {
            println!(
                "{}",
                "Contacting the other device… keep it on the pairing screen.".cyan()
            );
            match rpc(IpcRequest::PairJoin { ticket: t }).await? {
                IpcResponse::PairSuccess {
                    peer_name,
                    peer_fingerprint,
                } => println!(
                    "{} Linked with {} {}",
                    "✓".green().bold(),
                    peer_name.bold(),
                    format!("[{peer_fingerprint}]").dimmed()
                ),
                other => return Err(format!("unexpected reply: {other:?}").into()),
            }
        }
        Commands::Pair {
            ticket: None,
            lan,
            ttl,
        } => pair_host(lan, ttl).await?,
        Commands::Unpair { device, all, yes } => {
            if all {
                unpair_all(yes).await?;
            } else {
                let device = device.expect("clap requires a device unless --all is given");
                match rpc(IpcRequest::Unpair { selector: device }).await? {
                    IpcResponse::Unpaired { name } => {
                        println!("{} Removed {}", "✓".green().bold(), name.bold())
                    }
                    other => return Err(format!("unexpected reply: {other:?}").into()),
                }
            }
        }
        Commands::Pause { duration } => match rpc(IpcRequest::Pause {
            duration_secs: duration,
        })
        .await?
        {
            IpcResponse::Paused { remaining_secs } => println!(
                "{} Sync paused for {} s ({} min).",
                "⏸".yellow().bold(),
                remaining_secs,
                remaining_secs / 60
            ),
            other => return Err(format!("unexpected reply: {other:?}").into()),
        },
        Commands::Resume => match rpc(IpcRequest::Resume).await? {
            IpcResponse::Resumed => println!("{} Sync resumed.", "▶".green().bold()),
            other => return Err(format!("unexpected reply: {other:?}").into()),
        },
        Commands::History { limit, action } => match action {
            Some(HistoryAction::Clear { yes }) => history_clear(yes).await?,
            None => match rpc(IpcRequest::GetHistory { limit }).await? {
                IpcResponse::History(items) => print_history(&items),
                other => return Err(format!("unexpected reply: {other:?}").into()),
            },
        },
        Commands::Send { text } => {
            let text = if text == "-" {
                use std::io::Read;
                if std::io::stdin().is_terminal() {
                    eprintln!("Reading the text from the terminal; finish with Ctrl-D.");
                }
                let mut buf = String::new();
                std::io::stdin()
                    .read_to_string(&mut buf)
                    .map_err(|e| format!("cannot read standard input as text: {e}"))?;
                buf
            } else {
                text
            };
            let client = DaemonClient::for_current_user();
            let cap = request_cap(&client).await;
            let reply = client
                .request(&IpcRequest::SendClip { text }, cap)
                .await
                .map_err(|e| e.to_string())?;
            match reply {
                IpcResponse::Sent {
                    bytes,
                    delivered,
                    failed,
                } => {
                    if delivered.is_empty() && failed.is_empty() {
                        println!(
                            "{} No linked devices. Run `klip pair` first.",
                            "!".yellow().bold()
                        );
                    }
                    for name in &delivered {
                        println!("{} Sent {bytes} B to {}", "✓".green().bold(), name.bold());
                    }
                    for (name, why) in &failed {
                        println!(
                            "{} {} did not receive it: {}",
                            "✗".red().bold(),
                            name.bold(),
                            why
                        );
                    }
                    if delivered.is_empty() && !failed.is_empty() {
                        return Err("could not reach any linked device".into());
                    }
                }
                IpcResponse::Error(e) => return Err(e.into()),
                other => return Err(format!("unexpected reply: {other:?}").into()),
            }
        }
        Commands::InstallService => install_service()?,
    }
    Ok(())
}

async fn start(foreground: bool) -> AnyResult<()> {
    if DaemonClient::send_request(&IpcRequest::GetStatus)
        .await
        .is_ok()
    {
        println!("{}", "Klip is already running.".yellow());
        return Ok(());
    }
    let paths = klip_daemon::config::user_paths();
    // A stale socket from a crash is removed by the daemon itself, but only after it holds the
    // single-instance lock; removing it here could cut off a busy daemon that is still alive.

    if foreground {
        let config = DaemonConfig::load(&paths)?;
        println!("{}", "Starting Klip in the foreground…".cyan());
        let engine = Arc::new(DaemonEngine::init_with(config, paths, None).await?);
        println!(
            "{} device {}  ({})",
            "✓".green().bold(),
            engine.node_id().fingerprint().cyan(),
            engine.sync.relay_mode_label()
        );
        engine.run().await?;
        return Ok(());
    }

    // Background: re-launch ourselves detached; keep a private log so failures are not invisible.
    let log_path = paths.dir().join("daemon.log");
    let log = open_private_log(&log_path)?;
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.arg("start")
        .arg("--foreground")
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn()?;

    for _ in 0..120 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if DaemonClient::send_request(&IpcRequest::GetStatus)
            .await
            .is_ok()
        {
            println!(
                "{} Klip is running in the background (PID {}).",
                "✓".green().bold(),
                child.id()
            );
            return Ok(());
        }
        // The daemon refuses to start for a reason (already running, bad config, no network...):
        // say why right away instead of waiting for the timeout.
        if let Ok(Some(status)) = child.try_wait() {
            let tail = std::fs::read_to_string(&log_path)
                .map(|t| {
                    let lines: Vec<&str> = t.lines().collect();
                    lines[lines.len().saturating_sub(4)..].join("\n")
                })
                .unwrap_or_default();
            return Err(format!(
                "Klip stopped right after starting ({status}). Last log lines:\n{tail}\n(log: {})",
                log_path.display()
            )
            .into());
        }
    }
    Err(format!(
        "Klip did not come up within 30 s. See the log: {}",
        log_path.display()
    )
    .into())
}

fn open_private_log(path: &std::path::Path) -> AnyResult<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    Ok(opts.open(path)?)
}

async fn pair_host(lan: bool, ttl: u64) -> AnyResult<()> {
    let (ticket, expires, fp) = match rpc(IpcRequest::PairStart {
        ttl_secs: ttl,
        include_lan_addrs: lan,
    })
    .await?
    {
        IpcResponse::PairTicket {
            ticket,
            expires_in_secs,
            host_fingerprint,
        } => (ticket, expires_in_secs, host_fingerprint),
        other => return Err(format!("unexpected reply: {other:?}").into()),
    };
    print_pair_card(&ticket, &fp, expires, lan);

    let mut asked = false;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                let _ = rpc(IpcRequest::PairCancel).await;
                println!("\n{}", "Pairing cancelled.".yellow());
                return Ok(());
            }
            _ = tokio::time::sleep(Duration::from_millis(400)) => {}
        }
        let info = match rpc(IpcRequest::PairPoll).await? {
            IpcResponse::PairPoll(i) => i,
            other => return Err(format!("unexpected reply: {other:?}").into()),
        };
        if let Some(name) = info.paired {
            println!("{} Linked with {}", "✓".green().bold(), name.bold());
            return Ok(());
        }
        if let Some(p) = &info.pending {
            if !asked {
                asked = true;
                print_pair_request(p);
                let yes = confirm("Approve this device? [y/N] ").await;
                rpc(IpcRequest::PairDecide { accept: yes }).await?;
                if !yes {
                    println!("{}", "Declined.".yellow());
                    return Ok(());
                }
            }
        } else if !info.window_open {
            if asked {
                continue; // approved; waiting for the "paired" notice
            }
            println!("{}", "Pairing window closed without a device.".yellow());
            return Ok(());
        }
    }
}

async fn confirm(prompt: &str) -> bool {
    use std::io::Write;
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).ok();
        matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
    })
    .await
    .unwrap_or(false)
}

async fn unpair_all(yes: bool) -> AnyResult<()> {
    let peers = match rpc(IpcRequest::GetStatus).await? {
        IpcResponse::Status(s) => s.peers,
        other => return Err(format!("unexpected reply: {other:?}").into()),
    };
    if peers.is_empty() {
        println!("{}", "No linked devices.".yellow());
        return Ok(());
    }
    if !yes {
        let names = peers
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let prompt = format!(
            "Remove all {} linked devices ({names})? [y/N] ",
            peers.len()
        );
        if !confirm(&prompt).await {
            println!("{}", "Cancelled.".yellow());
            return Ok(());
        }
    }
    let (removed, failed) = unpair_all_compat(&DaemonClient::for_current_user())
        .await
        .map_err(|e: ClientError| e.to_string())?;
    for name in &removed {
        println!("{} Removed {}", "✓".green().bold(), name.bold());
    }
    for (name, why) in &failed {
        eprintln!(
            "{} could not remove {}: {why}",
            "error:".red().bold(),
            name.bold()
        );
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("{} device(s) could not be removed", failed.len()).into())
    }
}

async fn history_clear(yes: bool) -> AnyResult<()> {
    if !yes && !confirm("Forget all clips kept in the history of this device? [y/N] ").await {
        println!("{}", "Cancelled.".yellow());
        return Ok(());
    }
    match DaemonClient::send_request(&IpcRequest::ClearHistory).await {
        Ok(IpcResponse::HistoryCleared) => {
            println!("{} History cleared.", "✓".green().bold());
            Ok(())
        }
        Ok(IpcResponse::Error(e)) | Err(e) => Err(format!(
            "could not clear the history ({e}). If the running daemon is older than this klip, restart it: klip stop && klip start"
        )
        .into()),
        Ok(other) => Err(format!("unexpected reply: {other:?}").into()),
    }
}

fn install_service() -> AnyResult<()> {
    #[cfg(target_os = "linux")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            let dest = std::path::PathBuf::from(home).join(".config/systemd/user/klip.service");
            klip_daemon::generate_systemd_unit_file(&dest)?;
            println!(
                "{} Installed systemd unit: {}\nNow run: systemctl --user enable --now klip.service",
                "✓".green(),
                dest.display()
            );
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            let dest =
                std::path::PathBuf::from(home).join("Library/LaunchAgents/com.klip.daemon.plist");
            klip_daemon::generate_launchd_plist_file(&dest)?;
            println!(
                "{} Installed launchd agent: {}\nNow run: launchctl load {}",
                "✓".green(),
                dest.display(),
                dest.display()
            );
        }
    }
    #[cfg(target_os = "windows")]
    {
        let dest = klip_daemon::get_klip_dir().join("register-service.ps1");
        klip_daemon::generate_windows_service_script(&dest)?;
        println!(
            "{} Wrote {}\nRun it in PowerShell: powershell -ExecutionPolicy Bypass -File \"{}\"",
            "✓".green(),
            dest.display(),
            dest.display()
        );
    }
    Ok(())
}
