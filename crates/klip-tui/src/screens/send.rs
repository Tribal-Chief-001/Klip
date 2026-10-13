//! The Send screen: a multi-line editor whose text goes to every linked device, with a file loader
//! for big payloads and a truthful per-device result.
use crate::api::{ApiError, SendReport};
use crate::app::{AppState, Conn, Effect, ToastKind};
use crate::fmt;
use crate::sanitize::{clean, one_line, truncate, width, wrap_rows};
use crate::textbuf::TextBuf;
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use unicode_segmentation::UnicodeSegmentation;

/// The editor never holds more than this; bigger files are sent as they are ("large payload").
pub const EDITOR_MAX_BYTES: usize = 1024 * 1024;
/// How much of a large payload is shown as a read-only preview.
const PREVIEW_BYTES: usize = 64 * 1024;
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

#[derive(Debug, Clone)]
pub enum SendOutcome {
    Done(SendReport),
    Failed(String),
}

#[derive(Debug)]
pub struct LargePayload {
    pub path: String,
    pub text: String,
}

#[derive(Debug)]
pub struct SendState {
    pub buf: TextBuf,
    pub focus: bool,
    pub sending: bool,
    pub result: Option<SendOutcome>,
    /// The "open a file" path prompt, when open.
    pub prompt: Option<TextBuf>,
    pub large: Option<LargePayload>,
    /// First visible row and first visible column (in grapheme clusters) of the editor.
    pub top: usize,
    pub left: usize,
}

impl Default for SendState {
    fn default() -> Self {
        Self {
            buf: TextBuf::new(EDITOR_MAX_BYTES),
            focus: false,
            sending: false,
            result: None,
            prompt: None,
            large: None,
            top: 0,
            left: 0,
        }
    }
}

/// Rows and columns available inside the editor box, from the terminal size.
pub fn editor_height(app: &AppState) -> usize {
    (app.size.1 as usize).saturating_sub(2 + 5 + 2).max(1)
}
fn editor_width(app: &AppState) -> usize {
    (app.size.0 as usize).saturating_sub(2).max(1)
}

fn ensure_visible(app: &mut AppState) {
    let (h, w) = (editor_height(app), editor_width(app));
    let (row, col) = app.send.buf.cursor();
    if row < app.send.top {
        app.send.top = row;
    } else if row >= app.send.top + h {
        app.send.top = row + 1 - h;
    }
    if col < app.send.left {
        app.send.left = col;
    } else if col >= app.send.left + w.saturating_sub(1) {
        app.send.left = col + 2 - w.max(2);
    }
}

pub fn text_focus(app: &AppState) -> bool {
    app.send.prompt.is_some() || (app.send.focus && app.send.large.is_none())
}

pub fn on_enter(app: &mut AppState) {
    app.send.focus = app.send.large.is_none();
}

pub fn on_leave(app: &mut AppState) {
    app.send.focus = false;
    app.send.prompt = None;
}

pub fn on_daemon_down(app: &mut AppState) {
    app.send.sending = false;
}

/// Called with every send result (also those started from the History screen).
pub fn on_sent(app: &mut AppState, res: &Result<SendReport, ApiError>) {
    app.send.sending = false;
    app.send.result = Some(match res {
        Ok(r) => SendOutcome::Done(r.clone()),
        Err(e) => SendOutcome::Failed(e.plain()),
    });
}

pub fn on_file_loaded(app: &mut AppState, path: String, result: Result<String, String>) {
    match result {
        Err(e) => app.toast(ToastKind::Error, one_line(&e)),
        Ok(text) if text.len() <= EDITOR_MAX_BYTES => {
            app.send.buf = TextBuf::from_text(&text, EDITOR_MAX_BYTES);
            app.send.large = None;
            app.send.focus = true;
            app.send.top = 0;
            app.send.left = 0;
            app.toast(
                ToastKind::Ok,
                format!("Loaded {} ({})", one_line(&path), fmt::bytes(text.len())),
            );
        }
        Ok(text) => {
            let size = text.len();
            app.send.buf.clear();
            app.send.large = Some(LargePayload {
                path: path.clone(),
                text,
            });
            app.send.focus = false;
            app.toast(
                ToastKind::Info,
                format!(
                    "{} ({}) is sent as it is; the editor shows only its start",
                    one_line(&path),
                    fmt::bytes(size)
                ),
            );
        }
    }
}

fn send(app: &mut AppState, fx: &mut Vec<Effect>) {
    if app.send.sending {
        return;
    }
    let text = match &app.send.large {
        Some(l) => l.text.clone(),
        None => app.send.buf.text(),
    };
    if text.trim().is_empty() {
        app.toast(
            ToastKind::Warn,
            "Nothing to send: type or paste some text first",
        );
        return;
    }
    let bytes = text.len();
    let (limit, cap) = (app.clip_limit(), app.request_cap());
    if bytes > limit {
        app.send.result = Some(SendOutcome::Failed(format!(
            "{} is over the limit of {} per clip",
            fmt::bytes(bytes),
            fmt::bytes(limit)
        )));
        return;
    }
    if bytes + 64 > cap {
        app.send.result = Some(SendOutcome::Failed(
            ApiError::TooLarge {
                size: bytes,
                max: cap,
            }
            .plain(),
        ));
        return;
    }
    if !matches!(app.conn, Conn::Up) {
        app.toast(
            ToastKind::Warn,
            "Klip is not running: start it first (Home, s)",
        );
        return;
    }
    app.send.sending = true;
    app.send.result = None;
    fx.push(Effect::Send(text));
}

pub fn paste(app: &mut AppState, text: &str) {
    if let Some(p) = &mut app.send.prompt {
        let one: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
        p.insert_str(&one);
    } else if app.send.focus && app.send.large.is_none() {
        let inserted = app.send.buf.insert_str(text);
        if inserted < text.replace("\r\n", "\n").replace('\r', "\n").len() {
            app.toast(
                ToastKind::Warn,
                "The pasted text was cut at 1 MiB: use Ctrl+O to send a bigger file",
            );
        }
        ensure_visible(app);
    }
}

pub fn key(app: &mut AppState, k: KeyEvent, fx: &mut Vec<Effect>) {
    if app.send.prompt.is_some() {
        return prompt_key(app, k, fx);
    }
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    if (ctrl && k.code == KeyCode::Char('s')) || k.code == KeyCode::F(5) {
        return send(app, fx);
    }
    if ctrl && k.code == KeyCode::Char('o') {
        app.send.prompt = Some(TextBuf::new(4096));
        return;
    }
    if ctrl && k.code == KeyCode::Char('u') {
        app.send.buf.clear();
        app.send.large = None;
        app.send.result = None;
        app.send.top = 0;
        app.send.left = 0;
        return;
    }
    if !(app.send.focus && app.send.large.is_none()) {
        // Not editing: Enter or i starts editing, o opens a file, x clears.
        match k.code {
            KeyCode::Enter | KeyCode::Char('i') if app.send.large.is_none() => {
                app.send.focus = true
            }
            KeyCode::Char('o') => app.send.prompt = Some(TextBuf::new(4096)),
            KeyCode::Char('x') => {
                app.send.buf.clear();
                app.send.large = None;
                app.send.result = None;
            }
            _ => {}
        }
        return;
    }
    let b = &mut app.send.buf;
    match k.code {
        KeyCode::Esc => app.send.focus = false,
        KeyCode::Enter => b.newline(),
        KeyCode::Tab => {
            b.insert_char('\t');
        }
        KeyCode::Backspace => b.backspace(),
        KeyCode::Delete => b.delete(),
        KeyCode::Left => b.left(),
        KeyCode::Right => b.right(),
        KeyCode::Up => b.up(),
        KeyCode::Down => b.down(),
        KeyCode::Home => b.home(),
        KeyCode::End => b.end(),
        KeyCode::Char('a') if ctrl => b.home(),
        KeyCode::Char('e') if ctrl => b.end(),
        KeyCode::Char('w') if ctrl => b.delete_word_back(),
        KeyCode::Char(c) if !ctrl => {
            b.insert_char(c);
        }
        _ => {}
    }
    ensure_visible(app);
}

fn prompt_key(app: &mut AppState, k: KeyEvent, fx: &mut Vec<Effect>) {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let Some(p) = app.send.prompt.as_mut() else {
        return;
    };
    match k.code {
        KeyCode::Esc => app.send.prompt = None,
        KeyCode::Enter => {
            let path = p.text().trim().to_string();
            app.send.prompt = None;
            if path.is_empty() {
                app.toast(ToastKind::Warn, "Type the path of a text file");
            } else {
                fx.push(Effect::LoadFile(path));
            }
        }
        KeyCode::Backspace => p.backspace(),
        KeyCode::Delete => p.delete(),
        KeyCode::Left => p.left(),
        KeyCode::Right => p.right(),
        KeyCode::Home => p.home(),
        KeyCode::End => p.end(),
        KeyCode::Char('u') if ctrl => p.clear(),
        KeyCode::Char(c) if !ctrl => {
            p.insert_char(c);
        }
        _ => {}
    }
}

// -------------------------------------------------------------------------------------- drawing

pub fn view(app: &AppState, theme: &Theme, f: &mut Frame, area: Rect) {
    let [editor, result] =
        Layout::vertical([Constraint::Min(5), Constraint::Length(5)]).areas(area);
    draw_editor(app, theme, f, editor);
    draw_result(app, theme, f, result);
    if let Some(p) = &app.send.prompt {
        draw_prompt(p, theme, f, editor);
    }
}

fn counter(app: &AppState, theme: &Theme) -> (String, ratatui::style::Style) {
    let bytes = match &app.send.large {
        Some(l) => l.text.len(),
        None => app.send.buf.byte_len(),
    };
    let limit = app.clip_limit();
    let style = if bytes > limit {
        theme.st_bad()
    } else if bytes * 10 > limit * 9 {
        theme.st_warn()
    } else {
        theme.st_dim()
    };
    (
        format!(" {} / {} ", fmt::bytes(bytes), fmt::bytes(limit)),
        style,
    )
}

fn draw_editor(app: &AppState, theme: &Theme, f: &mut Frame, area: Rect) {
    let (count, count_style) = counter(app, theme);
    let title = match &app.send.large {
        Some(_) => " Send a large file ",
        None => " Send text to all devices ",
    };
    let block = Block::bordered()
        .title(Span::styled(title, theme.st_title()))
        .title_bottom(Line::from(Span::styled(count, count_style)).right_aligned())
        .border_style(if text_focus(app) {
            theme.st_signal()
        } else {
            theme.st_faint()
        });
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    if let Some(l) = &app.send.large {
        let head = format!(
            "{} · {} · sent as it is, not editable here · Ctrl+U discards it",
            one_line(&l.path),
            fmt::bytes(l.text.len())
        );
        let cut = l
            .text
            .char_indices()
            .nth(PREVIEW_BYTES)
            .map(|(i, _)| i)
            .unwrap_or(l.text.len());
        let mut lines = vec![Line::from(Span::styled(
            truncate(&head, inner.width as usize),
            theme.st_warn(),
        ))];
        lines.extend(
            wrap_rows(&clean(&l.text[..cut]), inner.width as usize)
                .into_iter()
                .take((inner.height as usize).saturating_sub(1))
                .map(|r| Line::from(Span::styled(r, theme.st_dim()))),
        );
        f.render_widget(Paragraph::new(lines), inner);
        return;
    }
    let b = &app.send.buf;
    if b.is_empty() && !app.send.focus {
        crate::screens::centered_text(
            f,
            inner,
            theme.st_dim(),
            "Press Enter to type, or Ctrl+O to load a file",
        );
        return;
    }
    let (h, w) = (inner.height as usize, inner.width as usize);
    let lines: Vec<Line> = b
        .lines()
        .iter()
        .skip(app.send.top)
        .take(h)
        .map(|l| {
            let visible: String = l.graphemes(true).skip(app.send.left).collect();
            Line::from(Span::styled(truncate(&clean(&visible), w), theme.st_text()))
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
    if text_focus(app) && app.send.prompt.is_none() {
        let (row, col) = b.cursor();
        let prefix: String = b.lines()[row]
            .graphemes(true)
            .skip(app.send.left)
            .take(col.saturating_sub(app.send.left))
            .collect();
        let x = (width(&clean(&prefix))).min(w.saturating_sub(1));
        let y = row.saturating_sub(app.send.top).min(h.saturating_sub(1));
        f.set_cursor_position((inner.x + x as u16, inner.y + y as u16));
    }
}

fn draw_result(app: &AppState, theme: &Theme, f: &mut Frame, area: Rect) {
    let block = Block::bordered()
        .title(Span::styled(" Result ", theme.st_title()))
        .border_style(theme.st_faint());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let mut lines: Vec<Line> = Vec::new();
    if app.send.sending {
        let frame = SPINNER[(app.mono_ms / 120) as usize % SPINNER.len()];
        lines.push(Line::from(Span::styled(
            format!("{frame} Sending… (Esc only stops the screen waiting)"),
            theme.st_warn(),
        )));
    } else {
        match &app.send.result {
            None => lines.push(Line::from(Span::styled(
                "Ctrl+S or F5 sends · Ctrl+O loads a file · Ctrl+U clears · Esc leaves the editor",
                theme.st_dim(),
            ))),
            Some(SendOutcome::Failed(m)) => lines.push(Line::from(Span::styled(
                format!("✗ {}", one_line(m)),
                theme.st_bad(),
            ))),
            Some(SendOutcome::Done(r)) if r.delivered.is_empty() && r.failed.is_empty() => lines
                .push(Line::from(Span::styled(
                    "No linked devices: nothing was sent. Pair one first (3).",
                    theme.st_warn(),
                ))),
            Some(SendOutcome::Done(r)) => {
                lines.push(Line::from(Span::styled(
                    format!("Sent {}", fmt::bytes(r.bytes)),
                    theme.st_text(),
                )));
                for n in &r.delivered {
                    lines.push(Line::from(Span::styled(
                        format!("✓ {}", one_line(n)),
                        theme.st_signal(),
                    )));
                }
                for (n, why) in &r.failed {
                    lines.push(Line::from(Span::styled(
                        format!("✗ {}: {}", one_line(n), one_line(why)),
                        theme.st_bad(),
                    )));
                }
            }
        }
    }
    let h = inner.height as usize;
    if lines.len() > h && h > 0 {
        lines.truncate(h - 1);
        lines.push(Line::from(Span::styled("…", theme.st_dim())));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_prompt(p: &TextBuf, theme: &Theme, f: &mut Frame, editor: Rect) {
    let r = Rect {
        x: editor.x + 2,
        y: editor.y + editor.height.saturating_sub(4),
        width: editor.width.saturating_sub(4),
        height: 3,
    };
    if r.width < 10 || r.y < editor.y {
        return;
    }
    f.render_widget(ratatui::widgets::Clear, r);
    let block = Block::bordered()
        .title(Span::styled(
            " Open a text file (Enter loads, Esc cancels) ",
            theme.st_title(),
        ))
        .border_style(theme.st_signal());
    let inner = block.inner(r);
    f.render_widget(block, r);
    let shown = crate::sanitize::tail(&clean(&p.text()), (inner.width as usize).saturating_sub(1));
    f.render_widget(Paragraph::new(shown.clone()).style(theme.st_text()), inner);
    f.set_cursor_position((inner.x + width(&shown) as u16, inner.y));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ApiError, SendReport};
    use crate::app::{update, HelloState, Msg, Reply, Screen};
    use crate::testkit::{sample_hello, sample_status};
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> Msg {
        Msg::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }
    fn ctrl(c: char) -> Msg {
        Msg::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }
    fn typed(a: &mut AppState, s: &str) {
        for c in s.chars() {
            update(
                a,
                if c == '\n' {
                    key(KeyCode::Enter)
                } else {
                    key(KeyCode::Char(c))
                },
            );
        }
    }
    fn app() -> AppState {
        let mut a = AppState::new((100, 30));
        a.conn = Conn::Up;
        a.status = Some(sample_status(vec![]));
        a.hello = HelloState::New(sample_hello());
        update(&mut a, key(KeyCode::Char('5')));
        assert_eq!(a.screen, Screen::Send);
        a
    }

    #[test]
    fn opening_the_screen_focuses_the_editor_and_escape_gives_the_keys_back() {
        let mut a = app();
        assert!(a.text_focus());
        typed(&mut a, "q1p"); // letters and digits are text here, not shortcuts
        assert_eq!(a.send.buf.text(), "q1p");
        assert_eq!(a.screen, Screen::Send);
        update(&mut a, key(KeyCode::Esc));
        assert!(!a.text_focus());
        update(&mut a, key(KeyCode::Char('1')));
        assert_eq!(a.screen, Screen::Home);
    }

    #[test]
    fn ctrl_s_and_f5_send_the_whole_multiline_text_once() {
        let mut a = app();
        typed(&mut a, "line one\n日本語 \u{1F980}\n\tindented");
        let fx = update(&mut a, ctrl('s'));
        assert!(
            matches!(fx.as_slice(), [Effect::Send(t)] if t == "line one\n日本語 \u{1F980}\n\tindented"),
            "{fx:?}"
        );
        assert!(a.send.sending);
        assert!(
            update(&mut a, key(KeyCode::F(5))).is_empty(),
            "a second send while one is running is ignored"
        );
        update(
            &mut a,
            Msg::Reply(Reply::Sent(Ok(SendReport {
                bytes: 30,
                delivered: vec!["Pixel".into()],
                failed: vec![],
            }))),
        );
        assert!(!a.send.sending);
        assert!(matches!(&a.send.result, Some(SendOutcome::Done(r)) if r.delivered == ["Pixel"]));
    }

    #[test]
    fn nothing_to_send_and_too_much_to_send_are_refused_without_calling_the_daemon() {
        let mut a = app();
        assert!(update(&mut a, ctrl('s')).is_empty());
        assert!(a.toasts.iter().any(|t| t.text.contains("Nothing to send")));
        a.send.buf = TextBuf::new(EDITOR_MAX_BYTES);
        if let HelloState::New(h) = &mut a.hello {
            h.config.max_clip_bytes = 10;
        }
        typed(&mut a, "this is longer than ten bytes");
        assert!(update(&mut a, ctrl('s')).is_empty());
        assert!(
            matches!(&a.send.result, Some(SendOutcome::Failed(m)) if m.contains("limit")),
            "{:?}",
            a.send.result
        );
    }

    #[test]
    fn an_old_daemon_has_an_8_mib_request_cap() {
        let mut a = app();
        a.hello = HelloState::Old;
        a.send.large = Some(LargePayload {
            path: "x".into(),
            text: "a".repeat(9 * 1024 * 1024),
        });
        a.send.focus = false;
        assert!(update(&mut a, ctrl('s')).is_empty());
        assert!(
            matches!(&a.send.result, Some(SendOutcome::Failed(m)) if m.contains("at most")),
            "{:?}",
            a.send.result
        );
    }

    #[test]
    fn failures_and_a_dead_daemon_end_the_sending_state() {
        let mut a = app();
        typed(&mut a, "x");
        update(&mut a, ctrl('s'));
        update(
            &mut a,
            Msg::Reply(Reply::Sent(Err(ApiError::Refused(
                "the other device could not write its clipboard".into(),
            )))),
        );
        assert!(!a.send.sending);
        assert!(matches!(&a.send.result, Some(SendOutcome::Failed(m)) if m.contains("clipboard")));
        update(&mut a, ctrl('s'));
        assert!(a.send.sending);
        a.inflight.status = true;
        update(&mut a, Msg::Reply(Reply::Status(Err(ApiError::DaemonDown))));
        assert!(
            !a.send.sending,
            "a dead daemon must not leave the screen waiting"
        );
    }

    #[test]
    fn editing_keys_work() {
        let mut a = app();
        typed(&mut a, "hello world");
        update(&mut a, ctrl('w'));
        assert_eq!(a.send.buf.text(), "hello ");
        update(&mut a, key(KeyCode::Backspace));
        assert_eq!(a.send.buf.text(), "hello");
        update(&mut a, key(KeyCode::Home));
        update(&mut a, key(KeyCode::Delete));
        assert_eq!(a.send.buf.text(), "ello");
        update(&mut a, key(KeyCode::Tab));
        assert!(
            a.send.buf.text().contains('\t'),
            "Tab types a tab inside the editor"
        );
        update(&mut a, ctrl('u'));
        assert!(a.send.buf.is_empty());
    }

    #[test]
    fn the_cursor_row_stays_visible_in_a_long_text() {
        let mut a = app();
        for i in 0..200 {
            typed(&mut a, &format!("line {i}\n"));
        }
        let (row, _) = a.send.buf.cursor();
        let h = editor_height(&a);
        assert!(
            a.send.top <= row && row < a.send.top + h,
            "row {row} not inside {}..{}",
            a.send.top,
            a.send.top + h
        );
        for _ in 0..150 {
            update(&mut a, key(KeyCode::Up));
        }
        let (row, _) = a.send.buf.cursor();
        assert!(a.send.top <= row && row < a.send.top + h);
    }

    #[test]
    fn a_paste_longer_than_the_editor_is_cut_and_says_so() {
        let mut a = app();
        update(&mut a, Msg::Paste("x".repeat(EDITOR_MAX_BYTES + 100)));
        assert_eq!(a.send.buf.byte_len(), EDITOR_MAX_BYTES);
        assert!(
            a.toasts.iter().any(|t| t.text.contains("cut")),
            "{:?}",
            a.toasts
        );
    }

    #[test]
    fn loading_a_file_small_into_the_editor_big_as_a_large_payload() {
        let mut a = app();
        let fx = update(&mut a, ctrl('o'));
        assert!(fx.is_empty() && a.text_focus() && a.send.prompt.is_some());
        typed(&mut a, "~/notes.txt");
        let fx = update(&mut a, key(KeyCode::Enter));
        assert!(
            matches!(fx.as_slice(), [Effect::LoadFile(p)] if p == "~/notes.txt"),
            "{fx:?}"
        );
        assert!(a.send.prompt.is_none());
        update(
            &mut a,
            Msg::Reply(Reply::FileLoaded {
                path: "~/notes.txt".into(),
                result: Ok("small file\n".into()),
            }),
        );
        assert_eq!(a.send.buf.text(), "small file\n");
        assert!(a.send.large.is_none() && a.send.focus);

        let big = "z".repeat(EDITOR_MAX_BYTES + 5);
        update(
            &mut a,
            Msg::Reply(Reply::FileLoaded {
                path: "big.txt".into(),
                result: Ok(big.clone()),
            }),
        );
        assert!(a.send.large.is_some() && a.send.buf.is_empty() && !a.send.focus);
        let fx = update(&mut a, ctrl('s'));
        assert!(
            matches!(fx.as_slice(), [Effect::Send(t)] if t.len() == big.len()),
            "the whole file is sent"
        );
        update(&mut a, ctrl('u'));
        assert!(a.send.large.is_none());
    }

    #[test]
    fn a_file_that_cannot_be_loaded_is_reported() {
        let mut a = app();
        update(
            &mut a,
            Msg::Reply(Reply::FileLoaded {
                path: "nope".into(),
                result: Err("nope: No such file".into()),
            }),
        );
        assert!(a.toasts.iter().any(|t| t.text.contains("No such file")));
    }

    #[test]
    fn escape_closes_the_file_prompt_without_loading() {
        let mut a = app();
        update(&mut a, ctrl('o'));
        typed(&mut a, "abc");
        let fx = update(&mut a, key(KeyCode::Esc));
        assert!(fx.is_empty() && a.send.prompt.is_none());
    }
}
