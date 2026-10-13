//! The Log screen: the daemon's own log file, followed live. It reads the file directly, so it works
//! while the daemon is stopped, which is exactly when it is needed.
use crate::app::{AppState, Effect, Screen};
use crate::files::LogChunk;
use crate::sanitize::{one_line, strip_ansi, truncate};
use crate::textbuf::TextBuf;
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use std::collections::VecDeque;

/// The log keeps at most this many lines in memory.
pub const MAX_LINES: usize = 5000;
const TAIL_EVERY_MS: u64 = 500;

#[derive(Debug)]
pub struct LogState {
    pub lines: VecDeque<String>,
    /// Where the next read starts; `None` before the first read.
    pub offset: Option<u64>,
    pub follow: bool,
    pub filter: TextBuf,
    pub filtering: bool,
    /// How many lines above the end the view is scrolled.
    pub scroll_back: usize,
    pub missing: bool,
    pub error: Option<String>,
}

impl Default for LogState {
    fn default() -> Self {
        Self {
            lines: VecDeque::new(),
            offset: None,
            follow: true,
            filter: TextBuf::default(),
            filtering: false,
            scroll_back: 0,
            missing: false,
            error: None,
        }
    }
}

impl LogState {
    /// The lines that pass the filter (case-insensitive).
    pub fn shown(&self) -> Vec<&String> {
        let needle = self.filter.text().to_lowercase();
        self.lines
            .iter()
            .filter(|l| needle.is_empty() || l.to_lowercase().contains(&needle))
            .collect()
    }
}

fn tail(app: &mut AppState, fx: &mut Vec<Effect>) {
    if app.inflight.log {
        return;
    }
    app.inflight.log = true;
    app.clock.log_ms = app.mono_ms;
    fx.push(Effect::TailLog(app.log.offset));
}

pub fn on_enter(app: &mut AppState, fx: &mut Vec<Effect>) {
    tail(app, fx);
}

pub fn on_tick(app: &mut AppState, fx: &mut Vec<Effect>) {
    if app.screen == Screen::Log && app.mono_ms.saturating_sub(app.clock.log_ms) >= TAIL_EVERY_MS {
        tail(app, fx);
    }
}

pub fn on_chunk(app: &mut AppState, res: Result<LogChunk, String>) {
    app.inflight.log = false;
    match res {
        Ok(c) => {
            if c.reset {
                app.log.lines.clear();
                app.log.scroll_back = 0;
            }
            app.log.missing = c.missing;
            app.log.error = None;
            for l in c.text.lines() {
                app.log.lines.push_back(strip_ansi(l));
            }
            while app.log.lines.len() > MAX_LINES {
                app.log.lines.pop_front();
            }
            app.log.offset = Some(c.new_offset);
        }
        Err(e) => app.log.error = Some(e),
    }
}

pub fn text_focus(app: &AppState) -> bool {
    app.log.filtering
}

pub fn scroll(app: &mut AppState, delta: i32) {
    // The wheel and the arrows move the view: negative = towards older lines.
    let max = app.log.shown().len().saturating_sub(1);
    if delta < 0 {
        app.log.scroll_back = (app.log.scroll_back + (-delta) as usize).min(max);
        app.log.follow = false;
    } else {
        app.log.scroll_back = app.log.scroll_back.saturating_sub(delta as usize);
    }
}

pub fn key(app: &mut AppState, k: KeyEvent) {
    if app.log.filtering {
        return filter_key(app, k);
    }
    let max = app.log.shown().len().saturating_sub(1);
    match k.code {
        KeyCode::Char('f') => {
            app.log.follow = !app.log.follow;
            if app.log.follow {
                app.log.scroll_back = 0;
            }
        }
        KeyCode::Char('/') => app.log.filtering = true,
        KeyCode::Esc if !app.log.filter.is_empty() => app.log.filter.clear(),
        KeyCode::Up | KeyCode::Char('k') => scroll(app, -1),
        KeyCode::Down | KeyCode::Char('j') => scroll(app, 1),
        KeyCode::PageUp => scroll(app, -10),
        KeyCode::PageDown => scroll(app, 10),
        KeyCode::End => {
            app.log.scroll_back = 0;
            app.log.follow = true;
        }
        KeyCode::Home => {
            app.log.scroll_back = max;
            app.log.follow = false;
        }
        _ => {}
    }
}

fn filter_key(app: &mut AppState, k: KeyEvent) {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let f = &mut app.log.filter;
    match k.code {
        KeyCode::Esc => {
            f.clear();
            app.log.filtering = false;
        }
        KeyCode::Enter => app.log.filtering = false,
        KeyCode::Backspace => f.backspace(),
        KeyCode::Char('u') if ctrl => f.clear(),
        KeyCode::Char(c) if !ctrl => {
            f.insert_char(c);
        }
        _ => {}
    }
    app.log.scroll_back = 0;
}

pub fn view(app: &AppState, theme: &Theme, f: &mut Frame, area: Rect) {
    let title = format!(
        " Log · {} · {} ",
        truncate(
            &one_line(&app.log_path),
            (area.width as usize).saturating_sub(24)
        ),
        if app.log.follow {
            "following"
        } else {
            "paused: f resumes"
        }
    );
    let title = if app.log.filter.is_empty() && !app.log.filtering {
        title
    } else {
        format!("{title}· search “{}” ", one_line(&app.log.filter.text()))
    };
    let block = Block::bordered()
        .title(Span::styled(title, theme.st_title()))
        .border_style(theme.st_faint());
    let inner = block.inner(area);
    f.render_widget(block, area);
    if let Some(e) = &app.log.error {
        crate::screens::centered_text(
            f,
            inner,
            theme.st_bad(),
            &truncate(&one_line(e), inner.width as usize),
        );
        return;
    }
    if app.log.missing && app.log.lines.is_empty() {
        crate::screens::centered_text(
            f,
            inner,
            theme.st_dim(),
            "No log yet: Klip writes it when it is started with `klip start`.",
        );
        return;
    }
    let shown = app.log.shown();
    if shown.is_empty() {
        let msg = if app.log.lines.is_empty() {
            "The log is empty."
        } else {
            "Nothing matches the search."
        };
        crate::screens::centered_text(f, inner, theme.st_dim(), msg);
        return;
    }
    let h = inner.height as usize;
    let end = shown
        .len()
        .saturating_sub(app.log.scroll_back.min(shown.len().saturating_sub(1)));
    let start = end.saturating_sub(h);
    let lines: Vec<Line> = shown[start..end]
        .iter()
        .map(|l| {
            let upper = l.to_uppercase();
            let style = if upper.contains("ERROR") {
                theme.st_bad()
            } else if upper.contains("WARN") {
                theme.st_warn()
            } else {
                theme.st_text()
            };
            Line::from(Span::styled(
                truncate(&one_line(l), inner.width as usize),
                style,
            ))
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{update, Conn, Msg, Reply, Screen};
    use crate::testkit::sample_status;
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> Msg {
        Msg::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }
    fn app() -> AppState {
        let mut a = AppState::new((100, 30));
        a.conn = Conn::Up;
        a.status = Some(sample_status(vec![]));
        update(&mut a, key(KeyCode::Char('6')));
        assert_eq!(a.screen, Screen::Log);
        a
    }
    fn chunk(text: &str, offset: u64) -> Msg {
        Msg::Reply(Reply::Log(Ok(LogChunk {
            new_offset: offset,
            text: text.to_string(),
            reset: false,
            missing: false,
        })))
    }

    #[test]
    fn opening_the_screen_reads_the_log_and_it_is_followed_every_half_second() {
        let mut a = AppState::new((100, 30));
        a.conn = Conn::Up;
        a.status = Some(sample_status(vec![]));
        let fx = update(&mut a, key(KeyCode::Char('6')));
        assert!(matches!(fx.as_slice(), [Effect::TailLog(None)]), "{fx:?}");
        update(&mut a, chunk("one\ntwo\n", 8));
        assert_eq!(a.log.offset, Some(8));
        let t = |a: &mut AppState, ms| {
            update(
                a,
                Msg::Tick {
                    mono_ms: ms,
                    wall_ms: ms,
                },
            )
        };
        a.clock.log_ms = 1_000;
        assert!(t(&mut a, 1_300)
            .iter()
            .all(|e| !matches!(e, Effect::TailLog(_))));
        assert!(t(&mut a, 1_600)
            .iter()
            .any(|e| matches!(e, Effect::TailLog(Some(8)))));
        // Not visible: no reads at all.
        update(&mut a, key(KeyCode::Char('1')));
        a.inflight.log = false;
        assert!(t(&mut a, 9_000)
            .iter()
            .all(|e| !matches!(e, Effect::TailLog(_))));
    }

    #[test]
    fn lines_are_appended_colour_codes_removed_and_the_buffer_is_capped() {
        let mut a = app();
        update(&mut a, chunk("\x1b[32m✓\x1b[0m device ABCD\n", 20));
        assert_eq!(a.log.lines.back().unwrap(), "✓ device ABCD");
        let many: String = (0..MAX_LINES + 100)
            .map(|i| format!("line {i}\n"))
            .collect();
        update(&mut a, chunk(&many, 99));
        assert_eq!(a.log.lines.len(), MAX_LINES);
        assert_eq!(
            a.log.lines.back().unwrap(),
            &format!("line {}", MAX_LINES + 99)
        );
    }

    #[test]
    fn a_truncated_log_starts_over_and_a_missing_one_is_flagged() {
        let mut a = app();
        update(&mut a, chunk("old line\n", 9));
        update(
            &mut a,
            Msg::Reply(Reply::Log(Ok(LogChunk {
                new_offset: 4,
                text: "new\n".into(),
                reset: true,
                missing: false,
            }))),
        );
        assert_eq!(a.log.lines.len(), 1);
        assert_eq!(a.log.lines[0], "new");
        update(
            &mut a,
            Msg::Reply(Reply::Log(Ok(LogChunk {
                new_offset: 0,
                text: String::new(),
                reset: true,
                missing: true,
            }))),
        );
        assert!(a.log.missing && a.log.lines.is_empty());
        update(&mut a, Msg::Reply(Reply::Log(Err("denied".into()))));
        assert_eq!(a.log.error.as_deref(), Some("denied"));
    }

    #[test]
    fn scrolling_back_stops_following_and_g_resumes() {
        let mut a = app();
        update(
            &mut a,
            chunk(&(0..100).map(|i| format!("l{i}\n")).collect::<String>(), 10),
        );
        assert!(a.log.follow);
        update(&mut a, key(KeyCode::PageUp));
        assert!(!a.log.follow && a.log.scroll_back == 10);
        update(&mut a, key(KeyCode::End));
        assert!(a.log.follow && a.log.scroll_back == 0);
        update(&mut a, key(KeyCode::Char('f')));
        assert!(!a.log.follow);
        update(&mut a, key(KeyCode::Char('f')));
        assert!(a.log.follow);
        for _ in 0..500 {
            update(&mut a, key(KeyCode::Up));
        }
        assert!(a.log.scroll_back <= 99, "cannot scroll past the first line");
    }

    #[test]
    fn the_filter_is_case_insensitive_and_escape_clears_it() {
        let mut a = app();
        update(
            &mut a,
            chunk("INFO started\nWARN relay slow\nERROR boom\nINFO done\n", 10),
        );
        update(&mut a, key(KeyCode::Char('/')));
        assert!(a.text_focus());
        for c in "error".chars() {
            update(&mut a, key(KeyCode::Char(c)));
        }
        assert_eq!(a.log.shown().len(), 1);
        update(&mut a, key(KeyCode::Enter));
        assert!(!a.text_focus());
        update(&mut a, key(KeyCode::Esc));
        assert_eq!(a.log.shown().len(), 4);
    }
}
