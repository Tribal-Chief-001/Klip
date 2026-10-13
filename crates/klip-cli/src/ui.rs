use klip_proto::ipc::{DaemonStatusInfo, HistoryItem, PendingPairInfo};
use owo_colors::OwoColorize;
use qrcode::render::unicode::Dense1x2;
use qrcode::QrCode;
use unicode_width::UnicodeWidthStr;

const INNER: usize = 74;

/// A rounded box whose rows are padded by *display* width, so emoji and CJK never break the border.
struct Card {
    out: Vec<String>,
}

impl Card {
    fn new(title: &str) -> Self {
        let head = format!("── {title} ");
        let fill = INNER.saturating_sub(head.width());
        Self {
            out: vec![format!("╭{head}{}╮", "─".repeat(fill))],
        }
    }

    fn blank(&mut self) {
        self.row("");
    }

    /// `plain` is used for width math; `styled` is what is printed (may contain ANSI colours).
    fn row_styled(&mut self, plain: &str, styled: &str) {
        let shown = truncate_to_width(plain, INNER - 2);
        let styled = if shown == plain {
            styled.to_string()
        } else {
            shown.clone()
        };
        let pad = (INNER - 2).saturating_sub(shown.width());
        self.out.push(format!("│ {styled}{} │", " ".repeat(pad)));
    }

    fn row(&mut self, text: &str) {
        self.row_styled(text, text);
    }

    fn finish(mut self) {
        self.out.push(format!("╰{}╯", "─".repeat(INNER)));
        for line in &self.out {
            println!("{}", line.dimmed());
        }
    }
}

fn truncate_to_width(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

fn print_card(title: &str, rows: &[(String, String)]) {
    let mut card = Card::new(title);
    for (plain, styled) in rows {
        card.row_styled(plain, styled);
    }
    card.finish();
}

pub fn print_status_card(info: &DaemonStatusInfo) {
    let mut rows: Vec<(String, String)> = Vec::new();
    let mut push = |plain: String, styled: String| rows.push((plain, styled));

    push(String::new(), String::new());
    let (state_plain, state_styled) = if info.is_paused {
        let t = format!("Paused ({}s left)", info.paused_remaining_secs);
        (t.clone(), t.yellow().to_string())
    } else {
        ("Active".to_string(), "Active".green().to_string())
    };
    push(
        format!("● STATE    {state_plain}"),
        format!("● STATE    {state_styled}"),
    );
    push(
        format!("● SYSTEM   {}", info.platform),
        format!("● SYSTEM   {}", info.platform),
    );
    push(
        format!(
            "● MEMORY   {:.1} MB resident",
            info.memory_rss_bytes as f64 / 1_048_576.0
        ),
        format!(
            "● MEMORY   {:.1} MB resident",
            info.memory_rss_bytes as f64 / 1_048_576.0
        ),
    );
    push(
        format!("● NETWORK  {}", info.relay_mode),
        format!("● NETWORK  {}", info.relay_mode),
    );
    push(
        format!("● RELAY    {}", info.relay_state),
        format!("● RELAY    {}", info.relay_state),
    );
    push(
        format!("● DEVICE   {}", info.node_id.fingerprint()),
        format!("● DEVICE   {}", info.node_id.fingerprint().cyan()),
    );
    push(String::new(), String::new());
    push(
        "Linked devices".to_string(),
        "Linked devices".bold().to_string(),
    );
    if info.peers.is_empty() {
        let hint = "  none yet — run `klip pair` to link a device";
        push(hint.to_string(), hint.dimmed().to_string());
    }
    for (i, peer) in info.peers.iter().enumerate() {
        let branch = if i + 1 == info.peers.len() {
            "└─"
        } else {
            "├─"
        };
        let (dot, state) = if peer.is_online {
            ("🟢", "connected")
        } else {
            ("⚪", "not connected")
        };
        let line = format!(
            "  {branch} {dot} {}  [{}]  {state}",
            peer.name,
            peer.node_id.fingerprint()
        );
        push(line.clone(), line);
        if let Some(err) = &peer.last_error {
            let e = format!("  │    last send failed: {err}");
            push(e.clone(), e.red().to_string());
        }
    }
    push(String::new(), String::new());
    let last = match info.last_sync_time_secs_ago {
        Some(secs) => format!(
            "Last sync  {secs}s ago · \"{}\" ({} B)",
            info.last_clip_preview.as_deref().unwrap_or(""),
            info.last_clip_size_bytes
        ),
        None => "Last sync  never".to_string(),
    };
    push(last.clone(), last);
    print_card("●─● klip", &rows);
}

/// Pairing screen for the host: QR + copyable ticket + fingerprint to compare.
pub fn print_pair_card(ticket: &str, host_fingerprint: &str, expires_in_secs: u64, lan: bool) {
    let mut card = Card::new("🔑 Pair a new device");
    card.blank();
    card.row("On the other device: open Klip → Link device → scan this QR,");
    card.row("or paste the ticket below (`klip pair <ticket>` on a computer).");
    card.blank();
    if let Ok(code) = QrCode::new(ticket.as_bytes()) {
        let qr = code
            .render::<Dense1x2>()
            .dark_color(Dense1x2::Light)
            .light_color(Dense1x2::Dark)
            .quiet_zone(true)
            .build();
        for line in qr.lines() {
            // centre the QR inside the card
            let pad = (INNER - 2).saturating_sub(line.width()) / 2;
            card.row(&format!("{}{line}", " ".repeat(pad)));
        }
    }
    card.blank();
    card.row_styled(
        "Ticket (single use):",
        &"Ticket (single use):".bold().to_string(),
    );
    for chunk in wrap_chars(ticket, INNER - 6) {
        card.row_styled(&format!("  {chunk}"), &format!("  {}", chunk.cyan()));
    }
    card.blank();
    let fp = format!("This device: {host_fingerprint}   ·   expires in {expires_in_secs}s");
    card.row(&fp);
    if lan {
        card.row("LAN mode: ticket includes local addresses (works without internet).");
    }
    card.blank();
    card.row("⚡ Waiting for the other device… you will be asked to approve it here.");
    card.finish();
}

pub fn print_pair_request(p: &PendingPairInfo) {
    println!();
    println!(
        "{} {} wants to pair   {}",
        "🔔".bold(),
        p.device_name.bold().cyan(),
        format!("[{}]", p.fingerprint).dimmed()
    );
}

pub fn print_history(items: &[HistoryItem]) {
    if items.is_empty() {
        println!("No clips in the recent history yet.");
        return;
    }
    let mut card = Card::new("Recent clips");
    for (i, item) in items.iter().enumerate() {
        let tag = if item.is_sensitive {
            "🔒 secret"
        } else {
            "📋 clip"
        };
        let head = format!(
            "#{:<2} {tag}  from {}  ({} B)",
            i + 1,
            item.origin_name,
            item.size_bytes
        );
        card.row(&head);
        card.row(&format!(
            "    {}",
            item.content_preview.replace('\n', " ⏎ ")
        ));
    }
    card.finish();
}

fn wrap_chars(s: &str, width: usize) -> Vec<String> {
    s.chars()
        .collect::<Vec<_>>()
        .chunks(width.max(1))
        .map(|c| c.iter().collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_rows_have_equal_display_width_even_with_emoji() {
        let mut card = Card::new("●─● klip");
        card.row("plain");
        card.row("🟢 emoji and 日本語 text");
        card.row(&"x".repeat(500));
        let widths: Vec<usize> = card.out.iter().map(|l| l.width()).collect();
        assert!(widths.iter().all(|w| *w == widths[0]), "{widths:?}");
    }

    #[test]
    fn wrap_never_exceeds_width() {
        for chunk in wrap_chars(&"a".repeat(250), 68) {
            assert!(chunk.len() <= 68);
        }
    }
}
