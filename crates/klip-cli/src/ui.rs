use klip_proto::ipc::{DaemonStatusInfo, HistoryItem};
use owo_colors::OwoColorize;

pub fn print_status_card(info: &DaemonStatusInfo) {
    let border = "─".repeat(58);
    println!(
        "{} {} {}",
        "╭──".dimmed(),
        "📎 Klip Engine".magenta().bold(),
        format!("{}╮", border).dimmed()
    );
    print_border_line("");

    // Status & Mesh
    let status_str = if info.is_paused {
        format!("Paused ({}s left)", info.paused_remaining_secs)
            .yellow()
            .to_string()
    } else {
        format!("Active ({})", info.platform)
            .green()
            .to_string()
    };

    let mesh_str = if info.peers.iter().any(|p| p.is_online) {
        "Connected".green().to_string()
    } else {
        "Mesh Ready (Listening)".cyan().to_string()
    };

    print_border_line(&format!(
        "  ● STATUS: {:<24} ⚡ MESH: {}",
        status_str, mesh_str
    ));

    // Memory & Latency
    let mem_mb = info.memory_rss_bytes as f64 / (1024.0 * 1024.0);
    let avg_latency = if !info.peers.is_empty() {
        let sum: u32 = info.peers.iter().map(|p| p.latency_ms).sum();
        sum / info.peers.len() as u32
    } else {
        15
    };

    print_border_line(&format!(
        "  ● MEMORY: {:.1} MB RSS            ⏱ LATENCY: {}ms (P2P)",
        mem_mb, avg_latency
    ));
    print_border_line("");

    // Peers
    print_border_line("  Linked Peers:");
    if info.peers.is_empty() {
        print_border_line(&format!(
            "  └─ (No peers paired yet. Run `{}` to link a device)",
            "klip pair".cyan()
        ));
    } else {
        for (idx, peer) in info.peers.iter().enumerate() {
            let is_last = idx == info.peers.len() - 1;
            let branch = if is_last { "└─" } else { "├─" };
            let state_bullet = if peer.is_online {
                "🟢".to_string()
            } else {
                "⚪".to_string()
            };
            let status_label = if peer.is_online { "Online" } else { "Offline" };

            print_border_line(&format!(
                "  {} {:<18} {} {} ({})    {}ms",
                branch, peer.name, state_bullet, status_label, peer.connection_type, peer.latency_ms
            ));
        }
    }

    print_border_line("");

    // Last sync
    if let Some(secs_ago) = info.last_sync_time_secs_ago {
        let preview = info.last_clip_preview.as_deref().unwrap_or("empty");
        print_border_line(&format!(
            "  Last Sync: {}s ago  •  \"{}\" ({} B)",
            secs_ago, preview, info.last_clip_size_bytes
        ));
    } else {
        print_border_line("  Last Sync: None yet");
    }

    println!("{}", format!("╰{}╯", "─".repeat(73)).dimmed());
}

pub fn print_pair_card(token: &str, sas_code: &str) {
    let border = "─".repeat(50);
    println!(
        "{} {} {}",
        "╭──".dimmed(),
        "🔑 Pair New Device".cyan().bold(),
        format!("{}╮", border).dimmed()
    );
    print_border_line("");
    print_border_line("  Scan this QR or copy the pairing token into target device:");
    print_border_line("");

    // Short Auth Code Pill
    print_border_line("  Short Auth Code:  ╭──────────────╮");
    print_border_line(&format!(
        "                    │  {}   │",
        sas_code.green().bold()
    ));
    print_border_line("                    ╰──────────────╯");
    print_border_line("");
    print_border_line(&format!("  Token: {}", token));
    print_border_line("");
    print_border_line("  ⚡ Waiting for peer connection via Iroh P2P mesh...");
    println!("{}", format!("╰{}╯", "─".repeat(71)).dimmed());
}

pub fn print_history(items: &[HistoryItem]) {
    if items.is_empty() {
        println!("No clips in recent history buffer.");
        return;
    }

    println!(
        "{}",
        "╭── Recent Synced Clips ────────────────────────────────────────────────────────╮"
            .magenta()
    );
    for (i, item) in items.iter().enumerate() {
        let tag = if item.is_sensitive {
            "🔒 [SECRET]".yellow().to_string()
        } else {
            "📋 [TEXT]".cyan().to_string()
        };

        println!(
            "│ #{:<2}  {}  From: {:<12}  ({} B)",
            i + 1, tag, item.origin_name, item.size_bytes
        );
        println!("│       {}", item.content_preview);
    }
    println!(
        "{}",
        "╰───────────────────────────────────────────────────────────────────────────────╯"
            .magenta()
    );
}

fn print_border_line(content: &str) {
    let visual_len = strip_ansi_len(content);
    let padding = if visual_len < 73 {
        " ".repeat(73 - visual_len)
    } else {
        "".to_string()
    };
    println!("{} {}{}{}", "│".dimmed(), content, padding, "│".dimmed());
}

fn strip_ansi_len(s: &str) -> usize {
    let mut len = 0;
    let mut in_escape = false;
    for c in s.chars() {
        if c == '\x1b' {
            in_escape = true;
        } else if in_escape {
            if c == 'm' {
                in_escape = false;
            }
        } else {
            len += 1;
        }
    }
    len
}
