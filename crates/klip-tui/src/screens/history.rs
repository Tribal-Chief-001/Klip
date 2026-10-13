//! The History screen: what was synced recently. Browse, search, copy an item back to this
//! computer's clipboard, send it again, reveal a hidden secret, clear.
use crate::api::{ApiError, HistoryItem};
use crate::app::{AppState, ConfirmAction, Conn, Effect, HelloState, Modal, Screen, ToastKind};
use crate::fmt;
use crate::sanitize::{clean, one_line, pad_to, truncate, width, wrap_rows};
use crate::textbuf::TextBuf;
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use std::collections::HashSet;

/// Longest stretch of one item shown in the detail pane.
const DETAIL_MAX_BYTES: usize = 256 * 1024;
const SAFETY_REFETCH_MS: u64 = 10_000;
const DEFAULT_LIMIT: usize = 50;

#[derive(Debug, Default)]
pub struct HistoryState {
    pub items: Vec<HistoryItem>,
    pub loaded: bool,
    /// Index into `visible()`.
    pub selected: usize,
    pub detail_scroll: usize,
    pub filter: TextBuf,
    pub filtering: bool,
    pub revealed: HashSet<[u8; 16]>,
    /// `total_synced_clips` at the last fetch: a change means there is something new.
    pub last_total: u64,
    pub error: Option<String>,
}

/// What an item looks like to a human, or `None` when it is not text.
fn item_text(item: &HistoryItem) -> Option<String> {
    if item.full_content.is_empty() {
        return None;
    }
    let bytes = &item.full_content[..item.full_content.len().min(DETAIL_MAX_BYTES)];
    match std::str::from_utf8(bytes) {
        Ok(s) => Some(s.to_string()),
        Err(e) if e.error_len().is_none() && bytes.len() == DETAIL_MAX_BYTES => {
            // Cut in the middle of a character by the size limit only.
            Some(String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned())
        }
        Err(_) => None,
    }
}

impl HistoryState {
    fn matches(&self, item: &HistoryItem, needle: &str) -> bool {
        if needle.is_empty() {
            return true;
        }
        if item.origin_name.to_lowercase().contains(needle) {
            return true;
        }
        if item.is_sensitive && !self.revealed.contains(&item.id) {
            return false; // a hidden secret must not be discoverable by guessing its content
        }
        match item_text(item) {
            Some(t) => t.to_lowercase().contains(needle),
            None => item.content_preview.to_lowercase().contains(needle),
        }
    }

    /// Indices into `items` that pass the search filter, newest first (the daemon's order).
    pub fn visible(&self) -> Vec<usize> {
        let needle = self.filter.text().to_lowercase();
        (0..self.items.len())
            .filter(|&i| self.matches(&self.items[i], &needle))
            .collect()
    }

    pub fn selected_item(&self) -> Option<&HistoryItem> {
        let vis = self.visible();
        vis.get(self.selected).map(|&i| &self.items[i])
    }

    fn clamp(&mut self) {
        let n = self.visible().len();
        self.selected = self.selected.min(n.saturating_sub(1));
    }
}

fn limit(app: &AppState) -> usize {
    match &app.hello {
        HelloState::New(h) => h.config.max_history_items.max(1),
        _ => DEFAULT_LIMIT,
    }
}

fn fetch(app: &mut AppState, fx: &mut Vec<Effect>) {
    if app.inflight.history {
        return;
    }
    app.inflight.history = true;
    app.clock.history_ms = app.mono_ms;
    app.history.last_total = app
        .status
        .as_ref()
        .map(|s| s.total_synced_clips)
        .unwrap_or(0);
    fx.push(Effect::History { limit: limit(app) });
}

pub fn on_enter(app: &mut AppState, fx: &mut Vec<Effect>) {
    fetch(app, fx);
}

pub fn on_leave(app: &mut AppState) {
    app.history.revealed.clear();
    app.history.filtering = false;
}

pub fn on_tick(app: &mut AppState, fx: &mut Vec<Effect>) {
    if app.screen != Screen::History || app.inflight.history || !matches!(app.conn, Conn::Up) {
        return;
    }
    let total = app
        .status
        .as_ref()
        .map(|s| s.total_synced_clips)
        .unwrap_or(0);
    let stale = app.mono_ms.saturating_sub(app.clock.history_ms) >= SAFETY_REFETCH_MS;
    if !app.history.loaded || total != app.history.last_total || stale {
        fetch(app, fx);
    }
}

pub fn on_items(app: &mut AppState, res: Result<Vec<HistoryItem>, ApiError>) {
    app.inflight.history = false;
    match res {
        Ok(items) => {
            app.history.items = items;
            app.history.loaded = true;
            app.history.error = None;
            app.history.clamp();
        }
        Err(e) => app.history.error = Some(e.plain()),
    }
}

pub fn on_cleared(app: &mut AppState) {
    app.history.items.clear();
    app.history.selected = 0;
    app.history.revealed.clear();
}

pub fn text_focus(app: &AppState) -> bool {
    app.history.filtering
}

pub fn scroll(app: &mut AppState, delta: i32) {
    let n = app.history.visible().len();
    let next =
        (app.history.selected as i64 + i64::from(delta)).clamp(0, n.saturating_sub(1) as i64);
    app.history.selected = next as usize;
    app.history.detail_scroll = 0;
}

pub fn key(app: &mut AppState, k: KeyEvent, fx: &mut Vec<Effect>) {
    if app.history.filtering {
        return filter_key(app, k);
    }
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let n = app.history.visible().len();
    match k.code {
        KeyCode::Up | KeyCode::Char('k') => scroll(app, -1),
        KeyCode::Down | KeyCode::Char('j') => scroll(app, 1),
        KeyCode::Home => {
            app.history.selected = 0;
            app.history.detail_scroll = 0;
        }
        KeyCode::End => {
            app.history.selected = n.saturating_sub(1);
            app.history.detail_scroll = 0;
        }
        KeyCode::PageDown => app.history.detail_scroll += 10,
        KeyCode::PageUp => app.history.detail_scroll = app.history.detail_scroll.saturating_sub(10),
        KeyCode::Char('/') => app.history.filtering = true,
        KeyCode::Esc if !app.history.filter.is_empty() => {
            app.history.filter.clear();
            app.history.selected = 0;
        }
        KeyCode::Char('r') if !ctrl => fetch(app, fx),
        KeyCode::Char('v') => {
            if let Some(item) = app.history.selected_item() {
                let id = item.id;
                if !app.history.revealed.remove(&id) {
                    app.history.revealed.insert(id);
                }
            }
        }
        KeyCode::Enter => copy_back(app, fx),
        KeyCode::Char('s') => resend(app, fx),
        KeyCode::Char('c') => clear_all(app),
        _ => {}
    }
}

fn filter_key(app: &mut AppState, k: KeyEvent) {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let f = &mut app.history.filter;
    match k.code {
        KeyCode::Esc => {
            f.clear();
            app.history.filtering = false;
        }
        KeyCode::Enter => app.history.filtering = false,
        KeyCode::Backspace => f.backspace(),
        KeyCode::Char('u') if ctrl => f.clear(),
        KeyCode::Char(c) if !ctrl => {
            f.insert_char(c);
        }
        _ => {}
    }
    app.history.selected = 0;
    app.history.detail_scroll = 0;
}

fn copy_back(app: &mut AppState, fx: &mut Vec<Effect>) {
    let Some(item) = app.history.selected_item() else {
        return;
    };
    let (id, empty) = (item.id, item.full_content.is_empty());
    if !app.has_capability(klip_proto::ipc::CAP_COPY_TO_CLIPBOARD) {
        app.toast(
            ToastKind::Warn,
            "The running Klip is older: restart it after upgrading to copy items back",
        );
    } else if empty {
        app.toast(
            ToastKind::Warn,
            "That clip was too large to keep in the history",
        );
    } else {
        fx.push(Effect::CopyItem(id));
    }
}

fn resend(app: &mut AppState, fx: &mut Vec<Effect>) {
    let Some(item) = app.history.selected_item() else {
        return;
    };
    if item.full_content.is_empty() {
        app.toast(
            ToastKind::Warn,
            "That clip was too large to keep in the history",
        );
        return;
    }
    match String::from_utf8(item.full_content.clone()) {
        Ok(text) => {
            fx.push(Effect::Send(text));
            app.toast(ToastKind::Info, "Sending again…");
        }
        Err(_) => app.toast(ToastKind::Warn, "Only text can be sent again from here"),
    }
}

fn clear_all(app: &mut AppState) {
    if !app.has_capability(klip_proto::ipc::CAP_CLEAR_HISTORY) {
        app.toast(
            ToastKind::Warn,
            "The running Klip is older: restart it after upgrading to clear the history",
        );
        return;
    }
    app.modal = Some(Modal::Confirm {
        title: "Clear the history?".to_string(),
        body: format!(
            "This forgets the {} clip(s) kept on this computer. Other devices are not affected.",
            app.history.items.len()
        ),
        action: ConfirmAction::ClearHistory,
    });
}

// -------------------------------------------------------------------------------------- drawing

fn row_text(app: &AppState, item: &HistoryItem) -> String {
    if item.is_sensitive && !app.history.revealed.contains(&item.id) {
        return format!("•••• secret ({})", fmt::bytes(item.size_bytes));
    }
    match item_text(item) {
        Some(t) => one_line(&t.chars().take(200).collect::<String>()),
        None if item.full_content.is_empty() => {
            format!("[too large to keep: {}]", fmt::bytes(item.size_bytes))
        }
        None => format!("[binary, {}]", fmt::bytes(item.size_bytes)),
    }
}

fn detail_rows(app: &AppState, item: &HistoryItem, cols: usize) -> Vec<String> {
    if item.is_sensitive && !app.history.revealed.contains(&item.id) {
        return vec![format!(
            "•••• secret ({}): press v to show it",
            fmt::bytes(item.size_bytes)
        )];
    }
    match item_text(item) {
        Some(t) => {
            let mut rows = wrap_rows(&clean(&t), cols);
            if item.full_content.len() > DETAIL_MAX_BYTES {
                rows.push(format!(
                    "… {} more bytes not shown",
                    item.full_content.len() - DETAIL_MAX_BYTES
                ));
            }
            rows
        }
        None if item.full_content.is_empty() => vec![format!(
            "[This clip is {}: too large to keep in the history. Copy it again on the device it came from.]",
            fmt::bytes(item.size_bytes)
        )],
        None => vec![format!("[binary content, {}]", fmt::bytes(item.size_bytes))],
    }
}

pub fn view(app: &AppState, theme: &Theme, f: &mut Frame, area: Rect) {
    let dir = if area.width >= 100 {
        Direction::Horizontal
    } else {
        Direction::Vertical
    };
    let [list_area, detail_area] = Layout::default()
        .direction(dir)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .areas(area);

    let vis = app.history.visible();
    let title = if app.history.filter.is_empty() && !app.history.filtering {
        format!(" History ({}) ", app.history.items.len())
    } else {
        format!(
            " History · search “{}” ({}) ",
            one_line(&app.history.filter.text()),
            vis.len()
        )
    };
    let block = Block::bordered()
        .title(Span::styled(title, theme.st_title()))
        .border_style(theme.st_faint());
    let inner = block.inner(list_area);
    f.render_widget(block, list_area);
    if let Some(err) = &app.history.error {
        crate::screens::centered_text(
            f,
            inner,
            theme.st_bad(),
            &truncate(&one_line(err), inner.width as usize),
        );
    } else if vis.is_empty() {
        let msg = if !app.history.loaded {
            "Loading…"
        } else if app.history.items.is_empty() {
            "No clips yet. Copy something on a linked device."
        } else {
            "Nothing matches the search."
        };
        crate::screens::centered_text(f, inner, theme.st_dim(), msg);
    } else {
        let selected = app.history.selected.min(vis.len() - 1);
        let (start, end) = crate::screens::window(selected, vis.len(), inner.height as usize);
        let w = inner.width as usize;
        let lines: Vec<Line> = (start..end)
            .map(|pos| {
                let item = &app.history.items[vis[pos]];
                let head = format!(
                    "{} {} {} ",
                    pad_to(&fmt::ago_ms(app.wall_ms, item.timestamp_ms), 10),
                    pad_to(&one_line(&item.origin_name), 14),
                    pad_to(&fmt::bytes(item.size_bytes), 9)
                );
                let text = truncate(&row_text(app, item), w.saturating_sub(width(&head) + 2));
                let style = if pos == selected {
                    theme.st_selected()
                } else {
                    theme.st_text()
                };
                Line::from(vec![
                    Span::styled(if pos == selected { "▸ " } else { "  " }, style),
                    Span::styled(
                        head,
                        if pos == selected {
                            style
                        } else {
                            theme.st_dim()
                        },
                    ),
                    Span::styled(text, style),
                ])
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }

    let block = Block::bordered()
        .title(Span::styled(" Clip ", theme.st_title()))
        .border_style(theme.st_faint());
    let inner = block.inner(detail_area);
    f.render_widget(block, detail_area);
    if let Some(item) = app.history.selected_item() {
        let head = format!(
            "{} · {} · {}",
            one_line(&item.origin_name),
            fmt::ago_ms(app.wall_ms, item.timestamp_ms),
            fmt::bytes(item.size_bytes)
        );
        let [h, body] = Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(inner);
        f.render_widget(
            Paragraph::new(truncate(&head, h.width as usize)).style(theme.st_dim()),
            h,
        );
        let rows = detail_rows(app, item, body.width as usize);
        let skip = app.history.detail_scroll.min(rows.len().saturating_sub(1));
        let lines: Vec<Line> = rows
            .into_iter()
            .skip(skip)
            .take(body.height as usize)
            .map(|r| Line::from(Span::styled(r, theme.st_text())))
            .collect();
        f.render_widget(Paragraph::new(lines), body);
    }
    if app.history.filtering {
        let x = list_area.x
            + 2
            + (" History · search “".chars().count() + width(&one_line(&app.history.filter.text())))
                as u16;
        if x < list_area.right() && list_area.height > 0 {
            f.set_cursor_position((x.min(list_area.right().saturating_sub(2)), list_area.y));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{update, HelloState, Msg, Reply, Screen};
    use crate::testkit::{sample_hello, sample_item, sample_status};
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> Msg {
        Msg::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }
    fn ch(c: char) -> Msg {
        key(KeyCode::Char(c))
    }
    fn app_with(items: Vec<HistoryItem>) -> AppState {
        let mut a = AppState::new((100, 30));
        a.conn = Conn::Up;
        a.status = Some(sample_status(vec![]));
        a.hello = HelloState::New(sample_hello());
        a.screen = Screen::History;
        a.history.items = items;
        a.history.loaded = true;
        a
    }
    fn three() -> Vec<HistoryItem> {
        vec![
            sample_item(1, "https://example.com/Docs", false),
            sample_item(2, "api_key_secret0123456789ABCDEF", true),
            sample_item(3, "fn main() {\n    println!(\"hi\");\n}", false),
        ]
    }

    #[test]
    fn entering_the_screen_fetches_the_history_at_once() {
        let mut a = app_with(vec![]);
        a.screen = Screen::Home;
        let fx = update(&mut a, ch('4'));
        assert!(
            matches!(fx.as_slice(), [Effect::History { limit: 50 }]),
            "{fx:?}"
        );
    }

    #[test]
    fn it_refetches_when_the_clip_count_changes_every_ten_seconds_and_on_r_but_not_otherwise() {
        let mut a = app_with(three());
        a.history.last_total = 128; // sample_status has 128 clips synced
        a.clock.history_ms = 1_000;
        let t = |a: &mut AppState, ms| {
            update(
                a,
                Msg::Tick {
                    mono_ms: ms,
                    wall_ms: ms,
                },
            )
        };
        assert!(
            t(&mut a, 2_000)
                .iter()
                .all(|e| !matches!(e, Effect::History { .. })),
            "nothing changed"
        );
        a.status.as_mut().unwrap().total_synced_clips = 129;
        assert!(
            t(&mut a, 2_100)
                .iter()
                .any(|e| matches!(e, Effect::History { .. })),
            "a new clip arrived"
        );
        a.inflight.history = false;
        assert!(t(&mut a, 3_000)
            .iter()
            .all(|e| !matches!(e, Effect::History { .. })));
        assert!(
            t(&mut a, 12_200)
                .iter()
                .any(|e| matches!(e, Effect::History { .. })),
            "10 s safety net"
        );
        a.inflight.history = false;
        let fx = update(&mut a, ch('r'));
        assert!(fx.iter().any(|e| matches!(e, Effect::History { .. })));
    }

    #[test]
    fn it_does_not_poll_when_the_screen_is_not_visible() {
        let mut a = app_with(three());
        a.screen = Screen::Home;
        a.status.as_mut().unwrap().total_synced_clips = 999;
        let fx = update(
            &mut a,
            Msg::Tick {
                mono_ms: 60_000,
                wall_ms: 60_000,
            },
        );
        assert!(fx.iter().all(|e| !matches!(e, Effect::History { .. })));
    }

    #[test]
    fn selection_moves_and_stays_in_range() {
        let mut a = app_with(three());
        update(&mut a, ch('j'));
        update(&mut a, key(KeyCode::Down));
        update(&mut a, key(KeyCode::Down));
        assert_eq!(a.history.selected, 2);
        update(&mut a, key(KeyCode::Home));
        assert_eq!(a.history.selected, 0);
        update(&mut a, key(KeyCode::End));
        assert_eq!(a.history.selected, 2);
    }

    #[test]
    fn search_is_case_insensitive_live_and_escape_clears_it() {
        let mut a = app_with(three());
        update(&mut a, ch('/'));
        assert!(a.text_focus());
        for c in "DOCS".chars() {
            update(&mut a, ch(c));
        }
        assert_eq!(a.history.visible(), vec![0]);
        update(&mut a, key(KeyCode::Enter)); // leave the field, keep the filter
        assert!(!a.text_focus());
        assert_eq!(a.history.visible(), vec![0]);
        update(&mut a, key(KeyCode::Esc)); // clears the filter
        assert_eq!(a.history.visible().len(), 3);
    }

    #[test]
    fn a_hidden_secret_cannot_be_found_by_its_content_until_it_is_revealed() {
        let mut a = app_with(three());
        a.history.filter.insert_str("secret0123");
        assert!(
            a.history.visible().is_empty(),
            "the content of a hidden secret is not searchable"
        );
        a.history.selected = 0;
        a.history.filter.clear();
        update(&mut a, ch('j')); // select the secret
        update(&mut a, ch('v'));
        a.history.filter.insert_str("secret0123");
        assert_eq!(a.history.visible(), vec![1]);
    }

    #[test]
    fn enter_copies_back_only_when_the_daemon_can() {
        let mut a = app_with(three());
        let fx = update(&mut a, key(KeyCode::Enter));
        assert!(
            matches!(fx.as_slice(), [Effect::CopyItem(id)] if *id == [1; 16]),
            "{fx:?}"
        );
        a.hello = HelloState::Old;
        let fx = update(&mut a, key(KeyCode::Enter));
        assert!(fx.is_empty());
        assert!(
            a.toasts.iter().any(|t| t.text.contains("older")),
            "{:?}",
            a.toasts
        );
    }

    #[test]
    fn an_item_whose_content_was_dropped_cannot_be_copied_or_resent() {
        let mut big = sample_item(9, "x", false);
        big.full_content.clear();
        big.size_bytes = 300_000;
        let mut a = app_with(vec![big]);
        assert!(update(&mut a, key(KeyCode::Enter)).is_empty());
        assert!(update(&mut a, ch('s')).is_empty());
        assert!(
            a.toasts
                .iter()
                .filter(|t| t.text.contains("too large"))
                .count()
                >= 1
        );
    }

    #[test]
    fn s_sends_the_text_again_but_not_binary_content() {
        let mut a = app_with(three());
        let fx = update(&mut a, ch('s'));
        assert!(
            matches!(fx.as_slice(), [Effect::Send(t)] if t == "https://example.com/Docs"),
            "{fx:?}"
        );
        let mut png = sample_item(5, "", false);
        png.full_content = vec![0x89, b'P', b'N', b'G', 0xff, 0xfe];
        png.size_bytes = 6;
        let mut a = app_with(vec![png]);
        assert!(update(&mut a, ch('s')).is_empty());
    }

    #[test]
    fn reveal_toggles_per_item_and_is_forgotten_on_leaving() {
        let mut a = app_with(three());
        update(&mut a, ch('j'));
        update(&mut a, ch('v'));
        assert!(a.history.revealed.contains(&[2; 16]));
        update(&mut a, ch('v'));
        assert!(a.history.revealed.is_empty());
        update(&mut a, ch('v'));
        update(&mut a, ch('1'));
        assert!(
            a.history.revealed.is_empty(),
            "leaving the screen hides secrets again"
        );
    }

    #[test]
    fn clearing_asks_first_and_needs_a_capable_daemon() {
        let mut a = app_with(three());
        update(&mut a, ch('c'));
        assert!(matches!(a.modal, Some(Modal::Confirm { .. })));
        let fx = update(&mut a, ch('y'));
        assert!(matches!(fx.as_slice(), [Effect::ClearHistory]), "{fx:?}");
        let mut a = app_with(three());
        a.hello = HelloState::Old;
        update(&mut a, ch('c'));
        assert!(a.modal.is_none());
        assert!(a.toasts.iter().any(|t| t.text.contains("older")));
    }

    #[test]
    fn searching_a_full_history_stays_fast() {
        let big = "lorem ipsum dolor sit amet ".repeat(6_000); // ~160 KB each, like a full 8 MiB store
        let items: Vec<HistoryItem> = (0..50u8).map(|n| sample_item(n, &big, false)).collect();
        let mut a = app_with(items);
        a.history.filter.insert_str("zzz-not-there");
        let started = std::time::Instant::now();
        for _ in 0..5 {
            assert!(a.history.visible().is_empty());
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "5 searches over ~8 MB took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn new_items_keep_the_selection_in_range() {
        let mut a = app_with(three());
        a.history.selected = 2;
        update(
            &mut a,
            Msg::Reply(Reply::History(Ok(vec![sample_item(1, "only", false)]))),
        );
        assert_eq!(a.history.selected, 0);
        update(
            &mut a,
            Msg::Reply(Reply::History(Err(crate::api::ApiError::DaemonDown))),
        );
        assert!(a.history.error.is_some());
    }
}
