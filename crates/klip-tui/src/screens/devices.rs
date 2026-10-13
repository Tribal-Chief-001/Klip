//! The Devices screen: the linked devices, unpair one or all.
use crate::api::PeerStatusInfo;
use crate::app::{AppState, ConfirmAction, Conn, Effect, Modal, ToastKind};
use crate::sanitize::{one_line, pad_to, truncate};
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

#[derive(Debug, Default)]
pub struct DevicesState {
    pub selected: usize,
}

fn peers(app: &AppState) -> &[PeerStatusInfo] {
    app.status
        .as_ref()
        .map(|s| s.peers.as_slice())
        .unwrap_or(&[])
}

pub fn scroll(app: &mut AppState, delta: i32) {
    let n = peers(app).len();
    let next =
        (app.devices.selected as i64 + i64::from(delta)).clamp(0, n.saturating_sub(1) as i64);
    app.devices.selected = next as usize;
}

pub fn key(app: &mut AppState, k: KeyEvent, _fx: &mut Vec<Effect>) {
    let n = peers(app).len();
    match k.code {
        KeyCode::Up | KeyCode::Char('k') => scroll(app, -1),
        KeyCode::Down | KeyCode::Char('j') => scroll(app, 1),
        KeyCode::Home => app.devices.selected = 0,
        KeyCode::End => app.devices.selected = n.saturating_sub(1),
        KeyCode::Char('u') => {
            let target = peers(app)
                .get(app.devices.selected)
                .map(|p| (one_line(&p.name), p.node_id.fingerprint()));
            match target {
                None => app.toast(ToastKind::Info, "No linked devices to unpair"),
                Some((name, selector)) => {
                    app.modal = Some(Modal::Confirm {
                        title: format!("Unpair {name}?"),
                        body: "It stops syncing with this computer until you pair it again."
                            .to_string(),
                        action: ConfirmAction::Unpair { selector, name },
                    });
                }
            }
        }
        KeyCode::Char('a') => {
            let names: Vec<String> = peers(app).iter().map(|p| one_line(&p.name)).collect();
            if names.is_empty() {
                app.toast(ToastKind::Info, "No linked devices to unpair");
            } else {
                app.modal = Some(Modal::Confirm {
                    title: "Unpair ALL devices?".to_string(),
                    body: format!(
                        "This removes {} linked device(s): {}.\nThey stop syncing until you pair them again.",
                        names.len(),
                        names.join(", ")
                    ),
                    action: ConfirmAction::UnpairAll { count: names.len() },
                });
            }
        }
        _ => {}
    }
}

/// One row of a device list: marker, online dot, padded name, fingerprint, state.
pub fn peer_line(
    p: &PeerStatusInfo,
    width_cols: usize,
    theme: &Theme,
    selected: bool,
) -> Line<'static> {
    let (dot, dot_style) = if p.is_online {
        ("●", theme.st_signal())
    } else {
        ("○", theme.st_dim())
    };
    let name = pad_to(&one_line(&p.name), 22);
    let fp = format!("[{}]", p.node_id.fingerprint());
    let state = if p.is_online {
        "connected".to_string()
    } else {
        match &p.last_error {
            Some(e) => format!("offline · {}", one_line(e)),
            None => "offline".to_string(),
        }
    };
    let used = 2 + 1 + 1 + 22 + 2 + fp.chars().count() + 2;
    let state = truncate(&state, width_cols.saturating_sub(used));
    let base = if selected {
        theme.st_selected()
    } else {
        theme.st_text()
    };
    let state_style = if selected {
        base
    } else if p.is_online {
        theme.st_signal()
    } else {
        theme.st_dim()
    };
    Line::from(vec![
        Span::styled(if selected { "▸ " } else { "  " }, base),
        Span::styled(dot, if selected { base } else { dot_style }),
        Span::styled(format!(" {name}  {fp}  "), base),
        Span::styled(state, state_style),
    ])
}

pub fn view(app: &AppState, theme: &Theme, f: &mut Frame, area: Rect) {
    let Some(st) = &app.status else {
        let msg = if matches!(app.conn, Conn::Down { .. }) {
            "Klip is not running"
        } else {
            "Connecting to Klip…"
        };
        crate::screens::centered_text(f, area, theme.st_dim(), msg);
        return;
    };
    let [list_area, detail_area] =
        Layout::vertical([Constraint::Min(4), Constraint::Length(6)]).areas(area);

    let block = Block::bordered()
        .title(Span::styled(
            format!(" Devices ({}) ", st.peers.len()),
            theme.st_title(),
        ))
        .border_style(theme.st_faint());
    let inner = block.inner(list_area);
    f.render_widget(block, list_area);
    if st.peers.is_empty() {
        crate::screens::centered_text(
            f,
            inner,
            theme.st_dim(),
            "No devices yet. Press 3 to pair one.",
        );
    } else {
        let selected = app.devices.selected.min(st.peers.len() - 1);
        let (start, end) = crate::screens::window(selected, st.peers.len(), inner.height as usize);
        let lines: Vec<Line> = (start..end)
            .map(|i| peer_line(&st.peers[i], inner.width as usize, theme, i == selected))
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }

    let block = Block::bordered()
        .title(Span::styled(" Details ", theme.st_title()))
        .border_style(theme.st_faint());
    let inner = block.inner(detail_area);
    f.render_widget(block, detail_area);
    if let Some(p) = st
        .peers
        .get(app.devices.selected.min(st.peers.len().saturating_sub(1)))
    {
        let row = |k: &str, v: String| {
            Line::from(vec![
                Span::styled(format!("{k:<13}"), theme.st_dim()),
                Span::styled(
                    truncate(&one_line(&v), (inner.width as usize).saturating_sub(13)),
                    theme.st_text(),
                ),
            ])
        };
        let lines = vec![
            row("Name", p.name.clone()),
            row("Fingerprint", p.node_id.fingerprint()),
            row(
                "State",
                if p.is_online {
                    "connected".to_string()
                } else {
                    "not connected".to_string()
                },
            ),
            row(
                "Last error",
                p.last_error.clone().unwrap_or_else(|| "none".to_string()),
            ),
        ];
        f.render_widget(Paragraph::new(lines), inner);
    }
}
