//! Drawing: the frame around every screen — tab bar, status bar, toasts and modals.
use crate::app::{
    is_compact, pause_choices, tab_label, AppState, Conn, Modal, Screen, ToastKind, MIN_COLS,
    MIN_ROWS,
};
use crate::fmt;
use crate::sanitize::{one_line, truncate, width};
use crate::screens;
use crate::theme::Theme;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui::Frame;

/// A `w × h` rectangle in the middle of `area` (clamped to it).
pub fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

pub fn view(app: &AppState, theme: &Theme, f: &mut Frame) {
    let area = f.area();
    if area.width < MIN_COLS || area.height < MIN_ROWS {
        too_small(f, area, theme);
        return;
    }
    let [top, body, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    tab_bar(app, theme, f, top);
    screens::view(app, theme, f, body);
    status_bar(app, theme, f, bottom);
    toasts(app, theme, f, body);
    if let Some(m) = &app.modal {
        modal(app, m, theme, f, area);
    }
}

fn too_small(f: &mut Frame, area: Rect, theme: &Theme) {
    let msg = format!(
        "Terminal too small: Klip needs {MIN_COLS}×{MIN_ROWS}, this is {}×{}",
        area.width, area.height
    );
    f.render_widget(
        Paragraph::new(msg)
            .style(theme.st_warn())
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        centered(area, area.width, 3.min(area.height)),
    );
}

fn chip(app: &AppState, theme: &Theme) -> (String, ratatui::style::Style) {
    match (&app.conn, &app.status) {
        (Conn::Down { .. }, _) => ("○ STOPPED".to_string(), theme.st_bad()),
        (Conn::Connecting, _) => ("… connecting".to_string(), theme.st_dim()),
        (_, Some(s)) if s.is_paused => (
            format!("● PAUSED {}", fmt::remaining(s.paused_remaining_secs)),
            theme.st_warn(),
        ),
        _ => ("● ACTIVE".to_string(), theme.st_signal()),
    }
}

fn tab_bar(app: &AppState, theme: &Theme, f: &mut Frame, area: Rect) {
    let mut spans = vec![
        Span::styled("●─●", theme.st_signal()),
        Span::raw(" "),
        Span::styled("klip", theme.st_text().add_modifier(Modifier::BOLD)),
        Span::raw("  "),
    ];
    let compact = is_compact(area.width);
    for s in Screen::ALL {
        let label = tab_label(s, compact, app.screen);
        spans.push(if s == app.screen {
            Span::styled(label, theme.st_selected())
        } else {
            Span::styled(label, theme.st_dim())
        });
    }
    let (text, style) = chip(app, theme);
    let used: usize = spans.iter().map(|s| width(&s.content)).sum();
    let room = (area.width as usize).saturating_sub(used);
    let chip_w = width(&text);
    if room > chip_w {
        spans.push(Span::raw(" ".repeat(room - chip_w)));
        spans.push(Span::styled(text, style));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn status_bar(app: &AppState, theme: &Theme, f: &mut Frame, area: Rect) {
    let text = if app.text_focus() {
        "Esc leave the field · Alt+1-7 screens · Ctrl+C quit".to_string()
    } else {
        format!(
            "{}1-7 screens · p pause · ? help · q quit",
            screens::hints(app)
        )
    };
    f.render_widget(
        Paragraph::new(truncate(&text, area.width as usize)).style(theme.st_dim()),
        area,
    );
}

fn toasts(app: &AppState, theme: &Theme, f: &mut Frame, body: Rect) {
    if app.toasts.is_empty() || body.height == 0 {
        return;
    }
    let n = app.toasts.len().min(body.height as usize);
    let area = Rect {
        x: body.x,
        y: body.y + body.height - n as u16,
        width: body.width,
        height: n as u16,
    };
    let lines: Vec<Line> = app
        .toasts
        .iter()
        .rev()
        .take(n)
        .rev()
        .map(|t| {
            let (icon, style) = match t.kind {
                ToastKind::Ok => ("✓ ", theme.st_signal()),
                ToastKind::Info => ("· ", theme.st_text()),
                ToastKind::Warn => ("! ", theme.st_warn()),
                ToastKind::Error => ("✗ ", theme.st_bad()),
            };
            Line::from(Span::styled(
                truncate(
                    &format!(" {icon}{} ", one_line(&t.text)),
                    body.width as usize,
                ),
                style,
            ))
            .right_aligned()
        })
        .collect();
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines), area);
}

/// Rows a text takes when wrapped to `inner_w` columns (an estimate good enough to size a box).
fn wrapped_rows(lines: &[String], inner_w: usize) -> usize {
    lines
        .iter()
        .map(|l| width(l).max(1).div_ceil(inner_w.max(1)))
        .sum::<usize>()
        .max(1)
}

fn boxed(
    f: &mut Frame,
    area: Rect,
    title: &str,
    theme: &Theme,
    lines: Vec<Line<'static>>,
    plain: &[String],
    w: u16,
) {
    let w = w.min(area.width.saturating_sub(2)).max(10);
    let inner_w = w.saturating_sub(4) as usize;
    let h = (wrapped_rows(plain, inner_w) as u16 + 2)
        .min(area.height.saturating_sub(2))
        .max(3);
    let r = centered(area, w, h);
    f.render_widget(Clear, r);
    let block = Block::bordered()
        .title(Span::styled(format!(" {title} "), theme.st_title()))
        .border_style(theme.st_signal());
    let inner = block.inner(r);
    f.render_widget(block, r);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn modal(app: &AppState, m: &Modal, theme: &Theme, f: &mut Frame, area: Rect) {
    match m {
        Modal::Help => {
            let rows = screens::help_lines(app);
            let mut plain: Vec<String> = rows.iter().map(|(k, d)| format!("{k:<12}{d}")).collect();
            let mut lines: Vec<Line<'static>> = rows
                .iter()
                .map(|(k, d)| {
                    Line::from(vec![
                        Span::styled(format!("{k:<12}"), theme.st_signal()),
                        Span::styled((*d).to_string(), theme.st_text()),
                    ])
                })
                .collect();
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Esc  close", theme.st_dim())));
            plain.push(String::new());
            plain.push("Esc  close".to_string());
            boxed(f, area, "Keys", theme, lines, &plain, 62);
        }
        Modal::Confirm { title, body, .. } => {
            let body = crate::sanitize::clean(body);
            let mut plain: Vec<String> = body.lines().map(|l| l.to_string()).collect();
            let mut lines: Vec<Line<'static>> = body
                .lines()
                .map(|l| Line::from(Span::styled(l.to_string(), theme.st_text())))
                .collect();
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("y", theme.st_signal()),
                Span::styled("  yes        ", theme.st_text()),
                Span::styled("n", theme.st_signal()),
                Span::styled("  no", theme.st_text()),
            ]));
            plain.push(String::new());
            plain.push("y  yes        n  no".to_string());
            boxed(f, area, &one_line(title), theme, lines, &plain, 62);
        }
        Modal::Notice { title, body } => {
            let body = crate::sanitize::clean(body);
            let mut plain: Vec<String> = body.lines().map(|l| l.to_string()).collect();
            let mut lines: Vec<Line<'static>> = body
                .lines()
                .map(|l| Line::from(Span::styled(l.to_string(), theme.st_text())))
                .collect();
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Enter  close", theme.st_dim())));
            plain.push(String::new());
            plain.push("Enter  close".to_string());
            boxed(f, area, &one_line(title), theme, lines, &plain, 62);
        }
        Modal::PairApproval { name, fingerprint } => {
            let plain = vec![
                "A device is asking to pair with this computer.".to_string(),
                "Compare this fingerprint with the one on that device:".to_string(),
                format!("    {fingerprint}"),
                String::new(),
                "y  approve        n  decline".to_string(),
            ];
            let lines: Vec<Line<'static>> = vec![
                Line::from(Span::styled(plain[0].clone(), theme.st_text())),
                Line::from(Span::styled(plain[1].clone(), theme.st_dim())),
                Line::from(Span::styled(
                    plain[2].clone(),
                    theme.st_signal().add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(vec![
                    Span::styled("y", theme.st_signal()),
                    Span::styled("  approve        ", theme.st_text()),
                    Span::styled("n", theme.st_signal()),
                    Span::styled("  decline", theme.st_text()),
                ]),
            ];
            boxed(
                f,
                area,
                &format!("Link “{name}”?"),
                theme,
                lines,
                &plain,
                62,
            );
        }
        Modal::Pause { selected, custom } => {
            let choices = pause_choices(app.paused());
            let mut lines: Vec<Line<'static>> = Vec::new();
            let mut plain: Vec<String> = Vec::new();
            for (i, (_, label)) in choices.iter().enumerate() {
                let mark = if i == *selected { "▸ " } else { "  " };
                let style = if i == *selected {
                    theme.st_selected()
                } else {
                    theme.st_text()
                };
                lines.push(Line::from(Span::styled(format!("{mark}{label}"), style)));
                plain.push(format!("{mark}{label}"));
            }
            if let Some(buf) = custom {
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled("seconds: ", theme.st_dim()),
                    Span::styled(format!("{}▏", buf.text()), theme.st_text()),
                ]));
                plain.push(String::new());
                plain.push("seconds: ".to_string());
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Enter choose · Esc cancel",
                theme.st_dim(),
            )));
            plain.push(String::new());
            plain.push("Enter choose · Esc cancel".to_string());
            boxed(f, area, "Pause syncing", theme, lines, &plain, 44);
        }
    }
}
