//! The Home screen: state of Klip, this device, the last clip and the linked devices.
use crate::api::DaemonStatusInfo;
use crate::app::{AppState, ConfirmAction, Conn, Effect, Modal, ToastKind};
use crate::fmt;
use crate::sanitize::{one_line, truncate};
use crate::screens::devices::peer_line;
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

pub fn key(app: &mut AppState, k: KeyEvent, fx: &mut Vec<Effect>) {
    match k.code {
        KeyCode::Char('s') if matches!(app.conn, Conn::Down { .. }) => {
            fx.push(Effect::StartDaemon);
            app.toast(ToastKind::Info, "Starting Klip…");
        }
        KeyCode::Char('x') if matches!(app.conn, Conn::Up) => {
            app.modal = Some(Modal::Confirm {
                title: "Stop Klip?".to_string(),
                body: "Syncing stops on this computer until you start Klip again (the s key here, or `klip start`)."
                    .to_string(),
                action: ConfirmAction::StopDaemon,
            });
        }
        _ => {}
    }
}

pub fn view(app: &AppState, theme: &Theme, f: &mut Frame, area: Rect) {
    match (&app.conn, &app.status) {
        (Conn::Down { reason }, _) => down_card(reason, theme, f, area),
        (Conn::Up, Some(st)) => up(st, theme, f, area),
        _ => crate::screens::centered_text(f, area, theme.st_dim(), "Connecting to Klip…"),
    }
}

fn last_clip_line(st: &DaemonStatusInfo, width: usize) -> String {
    match (st.last_sync_time_secs_ago, &st.last_clip_preview) {
        (Some(secs), Some(preview)) => truncate(
            &format!(
                "{} clips synced · last {} · \"{}\" ({})",
                st.total_synced_clips,
                fmt::ago(secs),
                one_line(preview),
                fmt::bytes(st.last_clip_size_bytes)
            ),
            width,
        ),
        _ if st.total_synced_clips > 0 => format!("{} clips synced", st.total_synced_clips),
        _ => "no clips synced yet".to_string(),
    }
}

fn up(st: &DaemonStatusInfo, theme: &Theme, f: &mut Frame, area: Rect) {
    let status_h = if st.pairing_open { 6 } else { 5 };
    let [top, bottom] =
        Layout::vertical([Constraint::Length(status_h), Constraint::Min(3)]).areas(area);

    let block = Block::bordered()
        .title(Span::styled(" Status ", theme.st_title()))
        .border_style(theme.st_faint());
    let inner = block.inner(top);
    f.render_widget(block, top);
    let w = inner.width as usize;
    let state = if st.is_paused {
        Span::styled(
            format!("● PAUSED {}", fmt::remaining(st.paused_remaining_secs)),
            theme.st_warn(),
        )
    } else {
        Span::styled("● ACTIVE", theme.st_signal())
    };
    let mut lines = vec![
        Line::from(vec![
            state,
            Span::styled(
                format!(
                    "   {}",
                    truncate(&one_line(&st.relay_state), w.saturating_sub(24))
                ),
                theme.st_dim(),
            ),
        ]),
        Line::from(vec![
            Span::styled("this device  ", theme.st_dim()),
            Span::styled(st.node_id.fingerprint(), theme.st_text()),
            Span::styled(
                format!(
                    "   {}   RAM {:.1} MB",
                    truncate(&one_line(&st.platform), 28),
                    st.memory_rss_bytes as f64 / 1_048_576.0
                ),
                theme.st_dim(),
            ),
        ]),
        Line::from(Span::styled(last_clip_line(st, w), theme.st_dim())),
    ];
    if st.pairing_open {
        lines.push(Line::from(Span::styled(
            "pairing window open",
            theme.st_warn(),
        )));
    }
    f.render_widget(Paragraph::new(lines), inner);

    let block = Block::bordered()
        .title(Span::styled(
            format!(" Devices ({}) ", st.peers.len()),
            theme.st_title(),
        ))
        .border_style(theme.st_faint());
    let inner = block.inner(bottom);
    f.render_widget(block, bottom);
    if st.peers.is_empty() {
        crate::screens::centered_text(
            f,
            inner,
            theme.st_dim(),
            "No devices yet. Press 3 to pair one.",
        );
    } else {
        let shown = (inner.height as usize).min(st.peers.len());
        let lines: Vec<Line> = st.peers[..shown]
            .iter()
            .map(|p| peer_line(p, inner.width as usize, theme, false))
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }
}

fn down_card(reason: &str, theme: &Theme, f: &mut Frame, area: Rect) {
    let r = crate::view::centered(area, 54, 9);
    let block = Block::bordered()
        .title(Span::styled(" Klip is not running ", theme.st_bad()))
        .border_style(theme.st_faint());
    let inner = block.inner(r);
    f.render_widget(block, r);
    let lines = vec![
        Line::from(Span::styled(
            truncate(&one_line(reason), inner.width as usize),
            theme.st_dim(),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("s", theme.st_signal()),
            Span::raw("  start it"),
        ]),
        Line::from(Span::styled(
            "or run `klip start` in a shell",
            theme.st_dim(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "6  shows the log: why it stopped",
            theme.st_dim(),
        )),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}
