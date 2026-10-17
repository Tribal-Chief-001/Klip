use clap::{Parser, Subcommand, ValueEnum};
use klip_cli::client::{ClientError, DaemonClient};
use klip_cli::ops::{log_tail, request_cap, spawn_daemon_process, unpair_all_compat, Spawned};
use klip_cli::ui::{print_history, print_pair_card, print_pair_request, print_status_card};
use klip_daemon::config::DaemonConfig;
use klip_daemon::engine::DaemonEngine;
use klip_proto::ipc::{IpcRequest, IpcResponse, TransferInfo, TransferState, CAP_FILE_FOLDERS};
use owo_colors::OwoColorize;
use std::io::IsTerminal;
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
    /// Send files or folders to your linked devices (they arrive in the other device's Klip folder)
    SendFile {
        /// The files or folders to send (a folder arrives as one new folder; links are not followed)
        #[arg(required = true)]
        files: Vec<std::path::PathBuf>,
        /// Send only to this device (name or start of the fingerprint); default: every linked device
        #[arg(long)]
        to: Option<String>,
        /// Start the transfer and return at once
        #[arg(long)]
        no_wait: bool,
    },
    /// Show running and recent file transfers, or stop one
    Transfers {
        /// Stop the running transfer whose id starts with this (see the first column)
        #[arg(long, value_name = "ID")]
        cancel: Option<String>,
    },
    /// Install the auto-start service for your OS
    InstallService,
    /// The tray icon (Linux): start it, or make it start when you log in
    Tray {
        #[command(subcommand)]
        action: Option<TrayAction>,
    },
    /// Send the image you copied last to all linked devices (bind it to a keyboard shortcut)
    Share,
    /// Show, or choose, whether copied images are sent at once or wait until you send them
    Images {
        /// `auto`: send every copied image at once. `manual`: it waits for `klip share`
        #[arg(value_enum)]
        mode: Option<ImageMode>,
    },
}

#[derive(Subcommand)]
enum TrayAction {
    /// Start the tray in the background (the default)
    Start,
    /// Run the tray in this terminal (what start-at-login runs)
    Run,
    /// Start the tray every time you log in
    Install,
    /// Stop starting the tray at login
    Uninstall,
    /// Show whether the tray can run here and whether it starts at login
    Status,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ImageMode {
    Auto,
    Manual,
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

#[tokio::main(worker_threads = 2)] // iroh idles at a fraction of the wakeups with 2 workers instead of one per core
async fn main() {
    // `klip history | head` must end quietly instead of panicking on a closed pipe.
    #[cfg(unix)]
    // SAFETY: restoring the default disposition of SIGPIPE at startup, before any other thread
    // exists or any signal handler is installed by this program.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    // A bare `klip` on a terminal opens the full-screen app. Everything else (arguments, pipes,
    // TERM=dumb) goes through clap exactly as before, so scripts and help texts are unchanged.
    {
        if std::env::args_os().len() == 1 && can_open_tui() {
            if let Err(e) = open_tui().await {
                eprintln!("{} {}", "error:".red().bold(), e);
                std::process::exit(1);
            }
            return;
        }
    }
    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("{} {}", "error:".red().bold(), e);
        std::process::exit(1);
    }
}

type AnyResult<T> = Result<T, Box<dyn std::error::Error>>;

fn can_open_tui() -> bool {
    // Windows terminals do not set TERM; Windows Terminal and the modern console handle the escapes.
    #[cfg(windows)]
    let term_ok = true;
    #[cfg(not(windows))]
    let term_ok = std::env::var("TERM")
        .map(|t| !t.is_empty() && t != "dumb")
        .unwrap_or(false);
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal() && term_ok
}

async fn open_tui() -> AnyResult<()> {
    let paths = klip_daemon::config::user_paths();
    let api = std::sync::Arc::new(klip_cli::api::IpcDaemonApi::new(
        DaemonClient::for_current_user(),
        paths.clone(),
    ));
    let env = klip_tui::theme::TuiEnv::from_process(paths);
    klip_tui::runtime::run(api, env).await?;
    Ok(())
}

async fn rpc(req: IpcRequest) -> AnyResult<IpcResponse> {
    match DaemonClient::send_request(&req).await {
        Ok(IpcResponse::Error(e)) => Err(e.into()),
        Ok(other) => Ok(other),
        Err(e) => Err(e.into()),
    }
}

/// Like `rpc`, but an error that is not one of ours (an older daemon does not know the request) gets
/// the usual "restart it" hint.
async fn rpc_hint(req: IpcRequest) -> AnyResult<IpcResponse> {
    match DaemonClient::send_request(&req).await {
        Ok(IpcResponse::Error(e)) => {
            // An older daemon cannot decode the new request and says so; everything else is its own
            // reason and is shown as it is.
            if e.contains("Deserialization") || e.contains("variant") {
                Err(format!(
                    "{e}. If the running daemon is older than this klip, restart it: klip stop && klip start"
                )
                .into())
            } else {
                Err(e.into())
            }
        }
        Ok(other) => Ok(other),
        Err(e) => Err(e.into()),
    }
}

/// The first 8 hex digits of a transfer id: enough to pick one by hand.
fn short_id(id: &[u8; 16]) -> String {
    id[..4].iter().map(|b| format!("{b:02x}")).collect()
}

async fn get_transfers() -> AnyResult<Vec<TransferInfo>> {
    match rpc_hint(IpcRequest::GetTransfers).await? {
        IpcResponse::Transfers(list) => Ok(list),
        other => Err(format!("unexpected reply: {other:?}").into()),
    }
}

fn percent(t: &TransferInfo) -> u64 {
    (t.done * 100).checked_div(t.size).unwrap_or(100)
}

fn is_running(t: &TransferInfo) -> bool {
    matches!(t.state, TransferState::Waiting | TransferState::Active)
}

fn describe(t: &TransferInfo) -> String {
    let arrow = match t.direction {
        klip_proto::ipc::TransferDirection::Sending => "→",
        klip_proto::ipc::TransferDirection::Receiving => "←",
    };
    format!(
        "{arrow} {}  {} ({})",
        t.peer,
        t.name,
        klip_daemon::human_size(t.size)
    )
}

/// Folders are known to daemons from v0.9.0 on; a daemon without the capability needs a restart first.
fn folder_needs_restart(capabilities: Option<&[String]>) -> bool {
    !capabilities.is_some_and(|caps| caps.iter().any(|c| c == CAP_FILE_FOLDERS))
}

async fn send_files(
    files: Vec<std::path::PathBuf>,
    to: Option<String>,
    no_wait: bool,
) -> AnyResult<()> {
    let mut started: Vec<TransferInfo> = Vec::new();
    let mut problems = 0;
    for file in &files {
        // The daemon is another process with its own working directory: it gets absolute paths only.
        let abs = match std::fs::canonicalize(file) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{} {}: {e}", "✗".red().bold(), file.display());
                problems += 1;
                continue;
            }
        };
        if abs.is_dir() {
            // An older daemon would try to open the folder as one file: ask it first.
            let caps =
                match DaemonClient::send_request(&IpcRequest::Hello { client_protocol: 2 }).await {
                    Ok(IpcResponse::Hello(h)) => Some(h.capabilities),
                    _ => None,
                };
            if folder_needs_restart(caps.as_deref()) {
                eprintln!(
                    "{} {}: the running Klip is older and cannot send folders. Restart it: systemctl --user restart klip.service (or klip stop && klip start)",
                    "✗".red().bold(),
                    file.display()
                );
                problems += 1;
                continue;
            }
        }
        let Some(path) = abs.to_str().map(str::to_string) else {
            eprintln!(
                "{} {}: the path is not valid text",
                "✗".red().bold(),
                file.display()
            );
            problems += 1;
            continue;
        };
        match rpc_hint(IpcRequest::SendFile {
            path,
            to: to.clone(),
        })
        .await
        {
            Ok(IpcResponse::TransfersStarted(list)) => started.extend(list),
            Ok(other) => return Err(format!("unexpected reply: {other:?}").into()),
            Err(e) => {
                eprintln!("{} {}: {e}", "✗".red().bold(), file.display());
                problems += 1;
            }
        }
    }
    if no_wait {
        for t in &started {
            if is_running(t) {
                println!(
                    "{} {}  [{}]",
                    "…".cyan().bold(),
                    describe(t),
                    short_id(&t.id)
                );
            } else {
                eprintln!(
                    "{} {}: {}",
                    "✗".red().bold(),
                    describe(t),
                    t.error.as_deref().unwrap_or("failed")
                );
                problems += 1;
            }
        }
    } else {
        problems += wait_for(&started).await?;
    }
    if problems > 0 {
        return Err(format!(
            "{problems} of {} file transfers did not complete",
            started.len().max(files.len())
        )
        .into());
    }
    Ok(())
}

/// Follow the transfers until each has ended; returns how many did not end well.
async fn wait_for(started: &[TransferInfo]) -> AnyResult<usize> {
    let tty = std::io::stderr().is_terminal();
    let begun = std::time::Instant::now();
    let mut first_done: Option<u64> = None;
    let mut reported = std::collections::HashSet::new();
    let mut failed = 0;
    while reported.len() < started.len() {
        let all = get_transfers().await?;
        let mut moved = 0;
        for s in started {
            if reported.contains(&s.id) {
                continue;
            }
            // A device that could not be reached comes back already failed and is not in the list.
            let Some(t) = all
                .iter()
                .find(|t| t.id == s.id)
                .or_else(|| (!is_running(s)).then_some(s))
            else {
                eprintln!(
                    "{} {}: Klip lost track of this transfer",
                    "✗".red().bold(),
                    describe(s)
                );
                reported.insert(s.id);
                failed += 1;
                continue;
            };
            if is_running(t) {
                moved += t.done;
                if tty {
                    let pct = percent(t);
                    let base = *first_done.get_or_insert(t.done);
                    let rate = (moved.saturating_sub(base)) as f64
                        / begun.elapsed().as_secs_f64().max(0.001);
                    eprint!(
                        "\r\x1b[2K{}  {pct}%  {}/s",
                        t.name,
                        klip_daemon::human_size(rate as u64)
                    );
                }
                continue;
            }
            if tty {
                eprint!("\r\x1b[2K");
            }
            reported.insert(t.id);
            match t.state {
                TransferState::Done => println!("{} {}", "✓".green().bold(), describe(t)),
                _ => {
                    failed += 1;
                    let why = t.error.as_deref().unwrap_or("cancelled");
                    eprintln!("{} {}: {why}", "✗".red().bold(), describe(t));
                }
            }
        }
        if reported.len() < started.len() {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    Ok(failed)
}

async fn transfers_command(cancel: Option<String>) -> AnyResult<()> {
    let list = get_transfers().await?;
    if let Some(prefix) = cancel {
        let prefix = prefix.to_lowercase();
        let hits: Vec<&TransferInfo> = list
            .iter()
            .filter(|t| is_running(t) && short_id(&t.id).starts_with(&prefix))
            .collect();
        return match hits.as_slice() {
            [t] => match rpc_hint(IpcRequest::CancelTransfer { id: t.id }).await? {
                IpcResponse::TransferCancelled(true) => {
                    println!("{} Stopped {}", "✓".green().bold(), describe(t));
                    Ok(())
                }
                IpcResponse::TransferCancelled(false) => Err("that transfer has just ended".into()),
                other => Err(format!("unexpected reply: {other:?}").into()),
            },
            [] => Err(format!("no running transfer has an id starting with \"{prefix}\"").into()),
            _ => Err("more than one running transfer matches; type more of the id".into()),
        };
    }
    if list.is_empty() {
        println!("No file transfers yet.");
    }
    for t in &list {
        let state = match t.state {
            TransferState::Waiting => "waiting".to_string(),
            TransferState::Active => format!("{}%", percent(t)),
            TransferState::Done => "done".to_string(),
            TransferState::Cancelled => "stopped".to_string(),
            TransferState::Failed => format!("failed: {}", t.error.as_deref().unwrap_or("unknown")),
        };
        println!("{}  {}  {state}", short_id(&t.id), describe(t));
    }
    Ok(())
}

fn print_sent(bytes: usize, delivered: &[String], failed: &[(String, String)]) -> AnyResult<()> {
    if delivered.is_empty() && failed.is_empty() {
        println!(
            "{} No linked devices. Run `klip pair` first.",
            "!".yellow().bold()
        );
    }
    for name in delivered {
        println!("{} Sent {bytes} B to {}", "✓".green().bold(), name.bold());
    }
    for (name, why) in failed {
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
    Ok(())
}

fn print_image_state(state: &klip_proto::ipc::ImageStateInfo) {
    if state.auto {
        println!("Images are sent automatically as soon as you copy them.");
        println!("To choose each one yourself: klip images manual");
    } else {
        println!("Images wait until you send them: `klip share` sends the last one you copied.");
        println!("To send every copied image at once: klip images auto");
    }
    match &state.waiting {
        Some(w) => println!(
            "{} Waiting: {} {}x{} ({} B), copied {} s ago.",
            "●".green(),
            w.kind.to_uppercase(),
            w.width,
            w.height,
            w.size_bytes,
            w.age_secs
        ),
        None => println!("No image is waiting."),
    }
}

#[cfg(any(unix, windows))]
async fn tray_command(action: TrayAction) -> AnyResult<()> {
    use klip_cli::tray;
    match action {
        TrayAction::Start => {
            let pid = tray::start_detached()?;
            println!("{} Klip tray started (PID {pid}).", "✓".green().bold());
        }
        TrayAction::Run => tray::run().await?,
        TrayAction::Install => {
            let exe = tray::exe_path()?;
            let place = tray::autostart::install(&exe.to_string_lossy())?;
            println!(
                "{} The tray will start when you log in ({place}).",
                "✓".green().bold()
            );
        }
        TrayAction::Uninstall => {
            if tray::autostart::uninstall()? {
                println!(
                    "{} The tray will no longer start at login.",
                    "✓".green().bold()
                );
            } else {
                println!("The tray was not set to start at login.");
            }
        }
        TrayAction::Status => {
            println!(
                "Tray: {}",
                if tray::is_running() {
                    "running"
                } else {
                    "not running (klip tray starts it)"
                }
            );
            println!(
                "Start at login: {}",
                if tray::autostart::installed() {
                    "yes"
                } else {
                    "no"
                }
            );
        }
    }
    Ok(())
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
                } => print_sent(bytes, &delivered, &failed)?,
                IpcResponse::Error(e) => return Err(e.into()),
                other => return Err(format!("unexpected reply: {other:?}").into()),
            }
        }
        Commands::SendFile { files, to, no_wait } => send_files(files, to, no_wait).await?,
        Commands::Transfers { cancel } => transfers_command(cancel).await?,
        Commands::InstallService => install_service()?,
        Commands::Tray { action } => tray_command(action.unwrap_or(TrayAction::Start)).await?,
        Commands::Share => match rpc_hint(IpcRequest::SendPendingImage).await? {
            IpcResponse::Sent {
                bytes,
                delivered,
                failed,
            } => print_sent(bytes, &delivered, &failed)?,
            other => return Err(format!("unexpected reply: {other:?}").into()),
        },
        Commands::Images { mode } => {
            let request = match mode {
                Some(m) => IpcRequest::SetAutoImages {
                    enabled: m == ImageMode::Auto,
                },
                None => IpcRequest::GetImageState,
            };
            match rpc_hint(request).await? {
                IpcResponse::ImageState(state) => print_image_state(&state),
                other => return Err(format!("unexpected reply: {other:?}").into()),
            }
        }
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
        #[cfg(target_os = "linux")]
        {
            engine
                .sync
                .set_undelivered_hook(klip_daemon::notify::hook());
            engine
                .sync
                .set_file_received_hook(klip_daemon::notify::file_hook());
        }
        engine.run().await?;
        return Ok(());
    }

    // Background: re-launch ourselves detached; keep a private log so failures are not invisible.
    let Spawned {
        mut child,
        log_path,
    } = spawn_daemon_process(&paths)?;

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
            let tail = log_tail(&log_path, 4);
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
    if let Some(message) = klip_cli::ops::install_service()? {
        println!("{} {}", "✓".green(), message);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_daemon_without_the_folder_capability_needs_a_restart() {
        let new = vec![
            "unpair_all".to_string(),
            "file_transfer".to_string(),
            "file_folders".to_string(),
        ];
        let old = vec!["unpair_all".to_string(), "file_transfer".to_string()];
        assert!(!folder_needs_restart(Some(&new)));
        assert!(folder_needs_restart(Some(&old)));
        assert!(
            folder_needs_restart(None),
            "no answer to Hello means an old daemon"
        );
    }
}
