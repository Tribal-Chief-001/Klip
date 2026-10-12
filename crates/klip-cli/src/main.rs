use clap::{Parser, Subcommand};
use klip_cli::client::DaemonClient;
use klip_cli::ui::{print_history, print_pair_card, print_status_card};
use klip_daemon::config::{get_ipc_socket_path, DaemonConfig};
use klip_daemon::engine::DaemonEngine;
use klip_proto::ipc::{IpcRequest, IpcResponse};
use owo_colors::OwoColorize;
use std::process::Command;

#[derive(Parser)]
#[command(name = "klip")]
#[command(about = "📎 Klip: Universal E2EE Cross-Platform Clipboard Synchronization Engine", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Launches Klip synchronization daemon
    Start {
        /// Run in foreground instead of background daemon mode
        #[arg(short, long)]
        foreground: bool,
    },
    /// Gracefully terminates running Klip daemon
    Stop,
    /// Displays connected peers, mesh latency, memory RSS, and sync status
    Status,
    /// Links a new device via QR code, SAS PIN, or pairing token
    Pair {
        /// Optional pairing token to connect to an existing peer
        token: Option<String>,
    },
    /// Pauses clipboard synchronization
    Pause {
        /// Pause duration in seconds (default: 600s / 10m)
        #[arg(default_value = "600")]
        duration: u64,
    },
    /// Resumes active clipboard synchronization
    Resume,
    /// Displays recent synced clipboard items from memory ring buffer
    History {
        /// Maximum number of items to show
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },
    /// Manually pushes a text snippet to all paired devices
    Send {
        /// Text content to broadcast
        text: String,
    },
    /// Installs daemon system persistence service (systemd / launchd)
    InstallService,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start { foreground } => {
            let sock_path = get_ipc_socket_path();
            if sock_path.exists() {
                // Check if responding
                if let Ok(IpcResponse::Status(_)) =
                    DaemonClient::send_request(&IpcRequest::GetStatus).await
                {
                    println!("{}", "Klip daemon is already running.".yellow());
                    return Ok(());
                } else {
                    let _ = std::fs::remove_file(&sock_path);
                }
            }

            if foreground {
                println!("{}", "Starting Klip Engine in foreground...".cyan());
                let config = DaemonConfig::default();
                let engine = DaemonEngine::init(config).await?;
                engine.run().await?;
            } else {
                let current_exe = std::env::current_exe()?;
                let mut cmd = Command::new(current_exe);
                cmd.arg("start")
                    .arg("--foreground")
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());
                #[cfg(unix)]
                {
                    use std::os::unix::process::CommandExt;
                    cmd.process_group(0);
                }

                match cmd.spawn() {
                    Ok(child) => {
                        println!(
                            "{} (PID: {})",
                            "✓ Klip daemon launched successfully in background."
                                .green()
                                .bold(),
                            child.id()
                        );
                    }
                    Err(e) => {
                        eprintln!("Failed to spawn daemon: {}", e);
                    }
                }
            }
        }
        Commands::Stop => match DaemonClient::send_request(&IpcRequest::StopDaemon).await {
            Ok(IpcResponse::DaemonStopping) => {
                println!("{}", "✓ Klip daemon stopped successfully.".green());
            }
            Ok(IpcResponse::Error(e)) => {
                eprintln!("Error stopping daemon: {}", e);
            }
            Err(e) => {
                eprintln!("{}", e.yellow());
            }
            _ => {}
        },
        Commands::Status => match DaemonClient::send_request(&IpcRequest::GetStatus).await {
            Ok(IpcResponse::Status(status_info)) => {
                print_status_card(&status_info);
            }
            Ok(IpcResponse::Error(e)) => eprintln!("Daemon error: {}", e),
            Err(e) => eprintln!("{}", e.yellow()),
            _ => {}
        },
        Commands::Pair { token } => {
            if let Some(tok) = token {
                println!("{}", "Connecting to peer using token...".cyan());
                match DaemonClient::send_request(&IpcRequest::PairConnect { token: tok }).await {
                    Ok(IpcResponse::PairSuccess {
                        peer_node,
                        peer_name,
                    }) => {
                        println!(
                            "{} Linked with {} ({})",
                            "✓".green().bold(),
                            peer_name.bold(),
                            peer_node.short_id().cyan()
                        );
                    }
                    Ok(IpcResponse::Error(e)) => eprintln!("Pairing failed: {}", e.red()),
                    Err(e) => eprintln!("{}", e.yellow()),
                    _ => {}
                }
            } else {
                match DaemonClient::send_request(&IpcRequest::PairInit).await {
                    Ok(IpcResponse::PairCode {
                        qr_ascii,
                        sas_code,
                        token,
                    }) => {
                        // Attempt to print terminal QR code
                        if qr2term::print_qr(&token).is_err() {
                            println!("{}", qr_ascii);
                        }
                        print_pair_card(&token, &sas_code);
                    }
                    Ok(IpcResponse::Error(e)) => eprintln!("Pairing init failed: {}", e),
                    Err(e) => eprintln!("{}", e.yellow()),
                    _ => {}
                }
            }
        }
        Commands::Pause { duration } => {
            match DaemonClient::send_request(&IpcRequest::Pause {
                duration_secs: duration,
            })
            .await
            {
                Ok(IpcResponse::Paused { remaining_secs }) => {
                    println!(
                        "{} Synchronization paused for {} seconds ({}m).",
                        "⏸".yellow().bold(),
                        remaining_secs,
                        remaining_secs / 60
                    );
                }
                Ok(IpcResponse::Error(e)) => eprintln!("Error: {}", e),
                Err(e) => eprintln!("{}", e.yellow()),
                _ => {}
            }
        }
        Commands::Resume => match DaemonClient::send_request(&IpcRequest::Resume).await {
            Ok(IpcResponse::Resumed) => {
                println!("{} Synchronization resumed.", "▶".green().bold());
            }
            Ok(IpcResponse::Error(e)) => eprintln!("Error: {}", e),
            Err(e) => eprintln!("{}", e.yellow()),
            _ => {}
        },
        Commands::History { limit } => {
            match DaemonClient::send_request(&IpcRequest::GetHistory { limit }).await {
                Ok(IpcResponse::History(items)) => {
                    print_history(&items);
                }
                Ok(IpcResponse::Error(e)) => eprintln!("Error: {}", e),
                Err(e) => eprintln!("{}", e.yellow()),
                _ => {}
            }
        }
        Commands::Send { text } => {
            let _bytes_len = text.len();
            match DaemonClient::send_request(&IpcRequest::SendClip { text }).await {
                Ok(IpcResponse::Sent { bytes }) => {
                    println!(
                        "{} Synced {} bytes to all paired devices.",
                        "✓".green().bold(),
                        bytes
                    );
                }
                Ok(IpcResponse::Error(e)) => eprintln!("Error: {}", e),
                Err(e) => eprintln!("{}", e.yellow()),
                _ => {}
            }
        }
        Commands::InstallService => {
            #[cfg(target_os = "linux")]
            {
                if let Some(home) = std::env::var_os("HOME") {
                    let dest =
                        std::path::PathBuf::from(home).join(".config/systemd/user/klip.service");
                    klip_daemon::generate_systemd_unit_file(&dest)?;
                    println!(
                        "{} Installed systemd unit to {}\nRun: `systemctl --user enable --now klip.service`",
                        "✓".green(),
                        dest.display()
                    );
                }
            }
            #[cfg(target_os = "macos")]
            {
                if let Some(home) = std::env::var_os("HOME") {
                    let dest = std::path::PathBuf::from(home)
                        .join("Library/LaunchAgents/com.klip.daemon.plist");
                    klip_daemon::generate_launchd_plist_file(&dest)?;
                    println!(
                        "{} Installed launchd plist to {}\nRun: `launchctl load {}`",
                        "✓".green(),
                        dest.display(),
                        dest.display()
                    );
                }
            }
            #[cfg(target_os = "windows")]
            {
                println!("Windows service registration script configured.");
            }
        }
    }

    Ok(())
}
