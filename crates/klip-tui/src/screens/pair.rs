//! The Pair screen: show a code for another device, or join with a ticket. A request from another
//! device is answered in a modal on any screen (`Modal::PairApproval`).
use crate::api::{ApiError, PairPollInfo, PairTicket, Paired};
use crate::app::{AppState, Conn, Effect, Modal, Screen, ToastKind};
use crate::fmt;
use crate::sanitize::{clean, one_line, tail, truncate, width};
use crate::textbuf::TextBuf;
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;

/// Validity choices for a code, in seconds (the `t` key cycles through them).
pub const TTLS: [u64; 4] = [60, 120, 300, 600];
const POLL_EVERY_MS: u64 = 500;
/// After approving, how long to wait for the other device to finish before saying so.
const APPROVED_WAIT_MS: u64 = 15_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairMode {
    Show,
    Join,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairResult {
    Linked(String),
    Failed(String),
    Expired,
}

#[derive(Debug)]
pub struct PairState {
    pub mode: PairMode,
    /// Bumped whenever a window is started, cancelled or answered; replies for an old value are dropped.
    pub session: u64,
    /// True while a window this TUI opened is open.
    pub ours: bool,
    pub starting: bool,
    pub ticket: Option<PairTicket>,
    pub started_ms: u64,
    pub lan: bool,
    pub ttl_idx: usize,
    pub join: TextBuf,
    pub join_focus: bool,
    pub joining: bool,
    pub result: Option<PairResult>,
    /// After Approve: keep polling until this time for the "paired" notice.
    pub awaiting_paired_until_ms: u64,
    pub qr: Option<Vec<String>>,
}

impl Default for PairState {
    fn default() -> Self {
        Self {
            mode: PairMode::Show,
            session: 0,
            ours: false,
            starting: false,
            ticket: None,
            started_ms: 0,
            lan: false,
            ttl_idx: 1,
            join: TextBuf::new(4096),
            join_focus: false,
            joining: false,
            result: None,
            awaiting_paired_until_ms: 0,
            qr: None,
        }
    }
}

/// The QR code as lines of half-block characters. `invert` draws light modules as blocks, which is
/// right for the usual light-text-on-dark terminal.
pub fn qr_lines(ticket: &str, invert: bool) -> Option<Vec<String>> {
    use qrcode::render::unicode::Dense1x2;
    let code = qrcode::QrCode::new(ticket.as_bytes()).ok()?;
    let (dark, light) = if invert {
        (Dense1x2::Light, Dense1x2::Dark)
    } else {
        (Dense1x2::Dark, Dense1x2::Light)
    };
    let drawn = code
        .render::<Dense1x2>()
        .dark_color(dark)
        .light_color(light)
        .quiet_zone(true)
        .build();
    Some(drawn.lines().map(|l| l.to_string()).collect())
}

pub fn text_focus(app: &AppState) -> bool {
    app.pair.mode == PairMode::Join && app.pair.join_focus
}

// ------------------------------------------------------------------------------------ windows

fn start_window(app: &mut AppState, fx: &mut Vec<Effect>, restart: bool) {
    if !matches!(app.conn, Conn::Up) {
        app.toast(
            ToastKind::Warn,
            "Klip is not running: start it first (Home, s)",
        );
        return;
    }
    app.pair.session += 1;
    app.pair.starting = true;
    app.pair.result = None;
    app.pair.qr = None;
    if restart {
        // The daemon replaces the window, so the code on screen stops working at once.
        app.pair.ticket = None;
    }
    fx.push(Effect::PairStart {
        ttl_secs: TTLS[app.pair.ttl_idx],
        lan: app.pair.lan,
        session: app.pair.session,
        restart,
    });
}

fn cancel_window(app: &mut AppState, fx: &mut Vec<Effect>) {
    if app.pair.ours || app.pair.starting {
        app.pair.session += 1;
        app.pair.ours = false;
        app.pair.starting = false;
        app.pair.ticket = None;
        app.pair.qr = None;
        fx.push(Effect::PairCancel);
    }
}

pub fn on_enter(app: &mut AppState, fx: &mut Vec<Effect>) {
    if app.pair.mode == PairMode::Show
        && !app.pair.ours
        && !app.pair.starting
        && app.pair.result.is_none()
    {
        start_window(app, fx, false);
    }
    if app.pair.mode == PairMode::Join {
        app.pair.join_focus = true;
    }
}

pub fn on_leave(app: &mut AppState, fx: &mut Vec<Effect>) {
    let answering = matches!(app.modal, Some(Modal::PairApproval { .. }))
        || app.pair.awaiting_paired_until_ms > app.mono_ms;
    if !answering {
        cancel_window(app, fx);
    }
    app.pair.join_focus = false;
}

pub fn on_daemon_down(app: &mut AppState) {
    let p = &mut app.pair;
    p.ours = false;
    p.starting = false;
    p.ticket = None;
    p.qr = None;
    p.joining = false;
    p.awaiting_paired_until_ms = 0;
    p.session += 1;
}

pub fn on_tick(app: &mut AppState, fx: &mut Vec<Effect>) {
    if app.pair.awaiting_paired_until_ms > 0 && app.mono_ms >= app.pair.awaiting_paired_until_ms {
        app.pair.awaiting_paired_until_ms = 0;
        app.pair.result = Some(PairResult::Failed(
            "The other device did not finish linking. Check its screen and try again.".to_string(),
        ));
        app.dirty = true;
    }
    let open_elsewhere = app.status.as_ref().map(|s| s.pairing_open).unwrap_or(false);
    let wanted = open_elsewhere || app.pair.ours || app.pair.awaiting_paired_until_ms > 0;
    if wanted
        && !app.inflight.pair_poll
        && app.mono_ms.saturating_sub(app.clock.pair_poll_ms) >= POLL_EVERY_MS
    {
        app.inflight.pair_poll = true;
        app.clock.pair_poll_ms = app.mono_ms;
        fx.push(Effect::PairPoll {
            session: app.pair.session,
        });
    }
    // On this screen in "show" mode there should always be a window, unless the last one ended with a result.
    if app.screen == Screen::Pair
        && app.pair.mode == PairMode::Show
        && matches!(app.conn, Conn::Up)
        && !app.pair.ours
        && !app.pair.starting
        && app.pair.result.is_none()
        && !open_elsewhere
    {
        start_window(app, fx, false);
    }
}

// ------------------------------------------------------------------------------------ answers

/// Answer the request shown in the approval modal.
pub fn decide(app: &mut AppState, accept: bool, fx: &mut Vec<Effect>) {
    app.pair.session += 1;
    fx.push(Effect::PairDecide {
        accept,
        session: app.pair.session,
    });
    if accept {
        app.pair.awaiting_paired_until_ms = app.mono_ms + APPROVED_WAIT_MS;
        app.toast(
            ToastKind::Info,
            "Approved: waiting for the other device to finish",
        );
    } else {
        app.toast(ToastKind::Info, "Declined");
    }
}

pub fn on_started(app: &mut AppState, session: u64, result: Result<PairTicket, ApiError>) {
    if session != app.pair.session {
        return;
    }
    app.pair.starting = false;
    match result {
        Ok(t) => {
            app.pair.qr = qr_lines(&t.ticket, app.invert_qr);
            app.pair.ticket = Some(t);
            app.pair.started_ms = app.mono_ms;
            app.pair.ours = true;
        }
        Err(e) => app.pair.result = Some(PairResult::Failed(e.plain())),
    }
}

pub fn on_polled(app: &mut AppState, session: u64, result: Result<PairPollInfo, ApiError>) {
    app.inflight.pair_poll = false;
    if session != app.pair.session {
        return;
    }
    let Ok(info) = result else { return }; // a dead daemon is handled by the status poll
    if let Some(name) = info.paired {
        let name = one_line(&name);
        app.pair.result = Some(PairResult::Linked(name.clone()));
        app.pair.awaiting_paired_until_ms = 0;
        app.pair.ours = false;
        app.pair.ticket = None;
        app.pair.qr = None;
        if matches!(app.modal, Some(Modal::PairApproval { .. })) {
            app.modal = None;
        }
        app.toast(ToastKind::Ok, format!("Linked with {name}"));
        app.clock.status_ms = 0;
        return;
    }
    if let Some(p) = info.pending {
        if !matches!(app.modal, Some(Modal::PairApproval { .. })) {
            app.modal = Some(Modal::PairApproval {
                name: one_line(&p.device_name),
                fingerprint: one_line(&p.fingerprint),
            });
        }
        return;
    }
    if matches!(app.modal, Some(Modal::PairApproval { .. })) {
        app.modal = Some(Modal::Notice {
            title: "That request is gone".to_string(),
            body: "The other device stopped waiting. Show a new code and let it scan again."
                .to_string(),
        });
    }
    if !info.window_open && app.pair.ticket.is_some() && app.pair.awaiting_paired_until_ms == 0 {
        app.pair.result = Some(PairResult::Expired);
        app.pair.ticket = None;
        app.pair.qr = None;
        app.pair.ours = false;
    }
}

pub fn on_decided(app: &mut AppState, session: u64, result: Result<PairPollInfo, ApiError>) {
    if session != app.pair.session {
        return;
    }
    if let Err(e) = result {
        app.pair.awaiting_paired_until_ms = 0;
        if matches!(&e, ApiError::Refused(m) if m.contains("no pairing request")) {
            app.modal = Some(Modal::Notice {
                title: "That request is gone".to_string(),
                body: "The other device stopped waiting. Show a new code and let it scan again."
                    .to_string(),
            });
        } else {
            app.toast(
                ToastKind::Error,
                format!("Could not answer the request: {}", e.plain()),
            );
        }
    }
}

pub fn on_joined(app: &mut AppState, session: u64, result: Result<Paired, ApiError>) {
    if session != app.pair.session {
        return;
    }
    app.pair.joining = false;
    match result {
        Ok(p) => {
            let name = one_line(&p.name);
            app.pair.join.clear();
            app.pair.result = Some(PairResult::Linked(name.clone()));
            app.toast(ToastKind::Ok, format!("Linked with {name}"));
            app.clock.status_ms = 0;
        }
        Err(e) => app.pair.result = Some(PairResult::Failed(e.plain())),
    }
}

// --------------------------------------------------------------------------------------- keys

fn switch_mode(app: &mut AppState, mode: PairMode, fx: &mut Vec<Effect>) {
    if app.pair.mode == mode {
        return;
    }
    app.pair.result = None;
    match mode {
        PairMode::Show => {
            app.pair.mode = PairMode::Show;
            app.pair.join_focus = false;
            start_window(app, fx, false);
        }
        PairMode::Join => {
            cancel_window(app, fx);
            app.pair.mode = PairMode::Join;
            app.pair.join_focus = true;
        }
    }
}

pub fn key(app: &mut AppState, k: KeyEvent, fx: &mut Vec<Effect>) {
    if text_focus(app) {
        return join_key(app, k, fx);
    }
    match (k.code, app.pair.mode) {
        (KeyCode::Char('c'), _) => switch_mode(app, PairMode::Show, fx),
        (KeyCode::Char('j'), _) => switch_mode(app, PairMode::Join, fx),
        (KeyCode::Char('n'), PairMode::Show) => start_window(app, fx, true),
        (KeyCode::Char('l'), PairMode::Show) => {
            app.pair.lan = !app.pair.lan;
            start_window(app, fx, true);
        }
        (KeyCode::Char('t'), PairMode::Show) => {
            app.pair.ttl_idx = (app.pair.ttl_idx + 1) % TTLS.len();
            start_window(app, fx, true);
        }
        (KeyCode::Enter, PairMode::Join) => app.pair.join_focus = true,
        _ => {}
    }
}

fn join_key(app: &mut AppState, k: KeyEvent, fx: &mut Vec<Effect>) {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let j = &mut app.pair.join;
    match k.code {
        KeyCode::Esc => app.pair.join_focus = false,
        KeyCode::Enter => submit_join(app, fx),
        KeyCode::Backspace => j.backspace(),
        KeyCode::Delete => j.delete(),
        KeyCode::Left => j.left(),
        KeyCode::Right => j.right(),
        KeyCode::Home => j.home(),
        KeyCode::End => j.end(),
        KeyCode::Char('u') if ctrl => j.clear(),
        KeyCode::Char('w') if ctrl => j.delete_word_back(),
        KeyCode::Char(c) if !ctrl && !c.is_whitespace() => {
            j.insert_char(c);
        }
        _ => {}
    }
}

fn submit_join(app: &mut AppState, fx: &mut Vec<Effect>) {
    let raw = app.pair.join.text();
    let ticket = raw
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .to_string();
    if ticket.is_empty() {
        app.toast(ToastKind::Warn, "Paste the ticket first");
    } else if !ticket.starts_with("klip1") {
        app.pair.result = Some(PairResult::Failed(
            "That does not look like a Klip ticket: it starts with klip1.".to_string(),
        ));
    } else if !matches!(app.conn, Conn::Up) {
        app.toast(
            ToastKind::Warn,
            "Klip is not running: start it first (Home, s)",
        );
    } else {
        app.pair.joining = true;
        app.pair.session += 1;
        app.pair.result = None;
        fx.push(Effect::PairJoin {
            ticket,
            session: app.pair.session,
        });
    }
}

pub fn paste(app: &mut AppState, text: &str) {
    if text_focus(app) {
        let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        app.pair.join.insert_str(&compact);
    }
}

// -------------------------------------------------------------------------------------- drawing

fn result_line(r: &PairResult, theme: &Theme) -> Line<'static> {
    match r {
        PairResult::Linked(name) => Line::from(Span::styled(
            format!("✓ Linked with {name}"),
            theme.st_signal(),
        )),
        PairResult::Failed(m) => {
            Line::from(Span::styled(format!("✗ {}", one_line(m)), theme.st_bad()))
        }
        PairResult::Expired => Line::from(Span::styled(
            "The code expired. Press n for a new one.",
            theme.st_warn(),
        )),
    }
}

pub fn view(app: &AppState, theme: &Theme, f: &mut Frame, area: Rect) {
    let block = Block::bordered()
        .title(Span::styled(" Pair a device ", theme.st_title()))
        .border_style(theme.st_faint());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [modes, body] = Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(inner);
    let chip = |label: &str, on: bool| {
        Span::styled(
            label.to_string(),
            if on {
                theme.st_selected()
            } else {
                theme.st_dim()
            },
        )
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            chip(" c  Show a code ", app.pair.mode == PairMode::Show),
            Span::raw("  "),
            chip(" j  Join with a ticket ", app.pair.mode == PairMode::Join),
        ])),
        modes,
    );
    match app.pair.mode {
        PairMode::Show => show(app, theme, f, body),
        PairMode::Join => join(app, theme, f, body),
    }
}

fn show(app: &AppState, theme: &Theme, f: &mut Frame, area: Rect) {
    let p = &app.pair;
    let mut lines: Vec<Line<'static>> = Vec::new();
    if let Some(r) = &p.result {
        lines.push(result_line(r, theme));
        lines.push(Line::from(""));
    }
    let Some(t) = &p.ticket else {
        let msg = if p.starting {
            "Opening a pairing window…"
        } else if matches!(app.conn, Conn::Up) {
            "Press n for a new code."
        } else {
            "Klip is not running."
        };
        lines.push(Line::from(Span::styled(msg, theme.st_dim())));
        f.render_widget(Paragraph::new(lines), area);
        return;
    };
    let w = area.width as usize;
    let ticket = clean(&t.ticket);
    let ticket_rows: Vec<String> = ticket
        .as_bytes()
        .chunks(w.saturating_sub(2).max(1))
        .map(|c| format!("  {}", String::from_utf8_lossy(c)))
        .collect();
    let fixed = lines.len() + 1 + ticket_rows.len() + 1 + 2;
    let qr_fits =
        p.qr.as_ref()
            .map(|q| q.len() + fixed <= area.height as usize && q.iter().all(|l| width(l) <= w));
    match (&p.qr, qr_fits) {
        (Some(q), Some(true)) => {
            for l in q {
                lines.push(Line::from(l.clone()).centered());
            }
        }
        _ => lines.push(Line::from(Span::styled(
            "The QR code does not fit this window: use the ticket below, or make the window bigger.",
            theme.st_dim(),
        ))),
    }
    lines.push(Line::from(Span::styled(
        "Ticket (single use):",
        theme.st_dim(),
    )));
    for r in ticket_rows {
        lines.push(Line::from(Span::styled(r, theme.st_signal())));
    }
    let left = t
        .expires_in_secs
        .saturating_sub(app.mono_ms.saturating_sub(p.started_ms) / 1000);
    lines.push(Line::from(Span::styled(
        truncate(
            &format!(
                "This device {} · expires in {} · LAN addresses {}",
                one_line(&t.host_fingerprint),
                fmt::mmss(left),
                if p.lan { "included" } else { "off" }
            ),
            w,
        ),
        theme.st_text(),
    )));
    lines.push(Line::from(Span::styled(
        format!(
            "n new code · l LAN {} · t validity {} s",
            if p.lan { "off" } else { "on" },
            TTLS[p.ttl_idx]
        ),
        theme.st_dim(),
    )));
    f.render_widget(Paragraph::new(lines), area);
}

fn join(app: &AppState, theme: &Theme, f: &mut Frame, area: Rect) {
    let p = &app.pair;
    let [label, field, msg] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .areas(area);
    f.render_widget(
        Paragraph::new(
            "Run `klip pair` on the other computer (or open “Show my code” in the app), then paste the klip1… ticket here.",
        )
        .style(theme.st_dim())
        .wrap(Wrap { trim: true }),
        label,
    );
    let block = Block::bordered().border_style(if p.join_focus {
        theme.st_signal()
    } else {
        theme.st_faint()
    });
    let inner = block.inner(field);
    f.render_widget(block, field);
    let shown = tail(
        &clean(&p.join.text()),
        (inner.width as usize).saturating_sub(1),
    );
    f.render_widget(Paragraph::new(shown.clone()).style(theme.st_text()), inner);
    if p.join_focus && inner.width > 0 && inner.height > 0 {
        f.set_cursor_position((inner.x + width(&shown) as u16, inner.y));
    }
    let mut lines: Vec<Line<'static>> = Vec::new();
    if p.joining {
        lines.push(Line::from(Span::styled(
            "Waiting for the other device to approve… (Esc only stops waiting here; the other device may still link)",
            theme.st_warn(),
        )));
    }
    if let Some(r) = &p.result {
        lines.push(result_line(r, theme));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), msg);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::PendingPairInfo;
    use crate::app::{update, Msg, Reply};
    use crate::testkit::{sample_hello, sample_status};
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> Msg {
        Msg::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }
    fn ch(c: char) -> Msg {
        key(KeyCode::Char(c))
    }
    fn app() -> AppState {
        let mut a = AppState::new((100, 30));
        a.conn = Conn::Up;
        a.status = Some(sample_status(vec![]));
        a.hello = crate::app::HelloState::New(sample_hello());
        a
    }
    fn ticket() -> PairTicket {
        PairTicket {
            ticket: "klip1ABCDEF".into(),
            expires_in_secs: 120,
            host_fingerprint: "AAAA-BBBB-CCCC".into(),
        }
    }
    fn started(a: &mut AppState) {
        let session = a.pair.session;
        update(
            a,
            Msg::Reply(Reply::PairStarted {
                session,
                result: Ok(ticket()),
            }),
        );
    }
    fn poll(a: &mut AppState, info: PairPollInfo) -> Vec<Effect> {
        a.inflight.pair_poll = true;
        let session = a.pair.session;
        update(
            a,
            Msg::Reply(Reply::PairPolled {
                session,
                result: Ok(info),
            }),
        )
    }
    fn pending() -> PairPollInfo {
        PairPollInfo {
            window_open: true,
            seconds_left: 90,
            pending: Some(PendingPairInfo {
                device_name: "Pixel 8".into(),
                fingerprint: "DDDD-EEEE-FFFF".into(),
            }),
            paired: None,
        }
    }

    #[test]
    fn opening_the_screen_starts_a_window_and_a_stale_reply_is_ignored() {
        let mut a = app();
        let fx = update(&mut a, ch('3'));
        assert!(
            matches!(
                fx.as_slice(),
                [Effect::PairStart {
                    ttl_secs: 120,
                    lan: false,
                    session: 1,
                    restart: false
                }]
            ),
            "{fx:?}"
        );
        assert!(a.pair.starting);
        update(
            &mut a,
            Msg::Reply(Reply::PairStarted {
                session: 0,
                result: Ok(ticket()),
            }),
        );
        assert!(
            a.pair.ticket.is_none(),
            "a reply for an old session must be dropped"
        );
        started(&mut a);
        assert!(a.pair.ours && a.pair.ticket.is_some() && a.pair.qr.is_some());
    }

    #[test]
    fn leaving_cancels_our_window_but_not_while_a_request_waits() {
        let mut a = app();
        update(&mut a, ch('3'));
        started(&mut a);
        let fx = update(&mut a, ch('1'));
        assert!(fx.iter().any(|e| matches!(e, Effect::PairCancel)), "{fx:?}");
        assert!(!a.pair.ours && a.pair.ticket.is_none());

        let mut a = app();
        update(&mut a, ch('3'));
        started(&mut a);
        poll(&mut a, pending());
        assert!(matches!(a.modal, Some(Modal::PairApproval { .. })));
        let fx = update(
            &mut a,
            Msg::Key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::ALT)),
        );
        assert!(
            !fx.iter().any(|e| matches!(e, Effect::PairCancel)),
            "must not decline a waiting request"
        );
        assert!(
            matches!(a.modal, Some(Modal::PairApproval { .. })),
            "the approval stays on top"
        );
    }

    #[test]
    fn a_request_opens_the_approval_and_y_approves_once() {
        let mut a = app();
        update(&mut a, ch('3'));
        started(&mut a);
        poll(&mut a, pending());
        let Some(Modal::PairApproval { name, fingerprint }) = a.modal.clone() else {
            panic!("no approval modal")
        };
        assert_eq!(
            (name.as_str(), fingerprint.as_str()),
            ("Pixel 8", "DDDD-EEEE-FFFF")
        );
        let before = a.pair.session;
        let fx = update(&mut a, ch('y'));
        assert!(
            matches!(fx.as_slice(), [Effect::PairDecide { accept: true, .. }]),
            "{fx:?}"
        );
        assert!(a.modal.is_none());
        assert!(
            a.pair.session > before,
            "the session moves on so older polls cannot reopen the modal"
        );
        assert!(a.pair.awaiting_paired_until_ms > 0);
        // A poll that was already in flight (old session) reports the same request again: ignored.
        update(
            &mut a,
            Msg::Reply(Reply::PairPolled {
                session: before,
                result: Ok(pending()),
            }),
        );
        assert!(a.modal.is_none());
    }

    #[test]
    fn n_and_escape_decline() {
        for code in [KeyCode::Char('n'), KeyCode::Esc] {
            let mut a = app();
            update(&mut a, ch('3'));
            started(&mut a);
            poll(&mut a, pending());
            let fx = update(&mut a, key(code));
            assert!(
                matches!(fx.as_slice(), [Effect::PairDecide { accept: false, .. }]),
                "{fx:?}"
            );
        }
    }

    #[test]
    fn a_window_opened_elsewhere_still_shows_the_request_on_any_screen() {
        let mut a = app();
        a.status.as_mut().unwrap().pairing_open = true; // `klip pair` in another terminal
        let fx = update(
            &mut a,
            Msg::Tick {
                mono_ms: 600,
                wall_ms: 600,
            },
        );
        assert!(
            fx.iter().any(|e| matches!(e, Effect::PairPoll { .. })),
            "{fx:?}"
        );
        poll(&mut a, pending());
        assert!(matches!(a.modal, Some(Modal::PairApproval { .. })));
        assert_eq!(a.screen, Screen::Home);
    }

    #[test]
    fn linking_shows_a_result_and_closes_a_waiting_approval() {
        let mut a = app();
        update(&mut a, ch('3'));
        started(&mut a);
        poll(&mut a, pending());
        poll(
            &mut a,
            PairPollInfo {
                window_open: false,
                seconds_left: 0,
                pending: None,
                paired: Some("Pixel 8".into()),
            },
        );
        assert_eq!(a.pair.result, Some(PairResult::Linked("Pixel 8".into())));
        assert!(a.modal.is_none());
        assert!(a
            .toasts
            .iter()
            .any(|t| t.text.contains("Linked with Pixel 8")));
    }

    #[test]
    fn a_closed_window_without_a_device_means_the_code_expired() {
        let mut a = app();
        update(&mut a, ch('3'));
        started(&mut a);
        poll(
            &mut a,
            PairPollInfo {
                window_open: false,
                seconds_left: 0,
                pending: None,
                paired: None,
            },
        );
        assert_eq!(a.pair.result, Some(PairResult::Expired));
        assert!(a.pair.ticket.is_none());
    }

    #[test]
    fn a_request_that_vanishes_is_reported_when_approving() {
        let mut a = app();
        update(&mut a, ch('3'));
        started(&mut a);
        poll(&mut a, pending());
        update(&mut a, ch('y'));
        let session = a.pair.session;
        update(
            &mut a,
            Msg::Reply(Reply::PairDecided {
                session,
                result: Err(crate::api::ApiError::Refused(
                    "no pairing request is waiting".into(),
                )),
            }),
        );
        assert!(
            matches!(a.modal, Some(Modal::Notice { ref title, .. }) if title.contains("gone")),
            "{:?}",
            a.modal
        );
        assert_eq!(a.pair.awaiting_paired_until_ms, 0);
    }

    #[test]
    fn approved_but_never_finished_times_out() {
        let mut a = app();
        update(&mut a, ch('3'));
        started(&mut a);
        poll(&mut a, pending());
        update(&mut a, ch('y'));
        update(
            &mut a,
            Msg::Tick {
                mono_ms: 20_000,
                wall_ms: 20_000,
            },
        );
        assert!(
            matches!(a.pair.result, Some(PairResult::Failed(_))),
            "{:?}",
            a.pair.result
        );
    }

    #[test]
    fn a_restarted_window_never_shows_the_old_code_which_is_already_invalid() {
        let mut a = app();
        update(&mut a, ch('3'));
        started(&mut a);
        assert!(a.pair.ticket.is_some());
        for c in ['n', 'l', 't'] {
            update(&mut a, ch(c));
            assert!(
                a.pair.ticket.is_none() && a.pair.qr.is_none() && a.pair.starting,
                "after {c} the old code must be hidden until the new one arrives"
            );
            started(&mut a);
            assert!(a.pair.ticket.is_some());
        }
    }

    #[test]
    fn n_l_and_t_restart_the_window_with_new_settings() {
        let mut a = app();
        update(&mut a, ch('3'));
        started(&mut a);
        let fx = update(&mut a, ch('n'));
        assert!(
            matches!(fx.as_slice(), [Effect::PairStart { restart: true, .. }]),
            "{fx:?}"
        );
        started(&mut a);
        let fx = update(&mut a, ch('l'));
        assert!(
            matches!(
                fx.as_slice(),
                [Effect::PairStart {
                    lan: true,
                    restart: true,
                    ..
                }]
            ),
            "{fx:?}"
        );
        started(&mut a);
        let fx = update(&mut a, ch('t'));
        assert!(
            matches!(fx.as_slice(), [Effect::PairStart { ttl_secs: 300, .. }]),
            "{fx:?}"
        );
        for expected in [600, 60, 120] {
            started(&mut a);
            let fx = update(&mut a, ch('t'));
            assert!(
                matches!(fx.as_slice(), [Effect::PairStart { ttl_secs, .. }] if *ttl_secs == expected),
                "{fx:?}"
            );
        }
    }

    #[test]
    fn join_mode_takes_a_pasted_ticket_strips_quotes_and_reports_the_result() {
        let mut a = app();
        update(&mut a, ch('3'));
        started(&mut a);
        let fx = update(&mut a, ch('j'));
        assert!(
            fx.iter().any(|e| matches!(e, Effect::PairCancel)),
            "switching mode closes our window"
        );
        assert!(a.text_focus());
        update(&mut a, Msg::Paste(" \"klip1 AB\nCD\" ".into()));
        assert_eq!(a.pair.join.text(), "\"klip1ABCD\"");
        let fx = update(&mut a, key(KeyCode::Enter));
        assert!(
            matches!(fx.as_slice(), [Effect::PairJoin { ticket, .. }] if ticket == "klip1ABCD"),
            "{fx:?}"
        );
        assert!(a.pair.joining);
        let session = a.pair.session;
        update(
            &mut a,
            Msg::Reply(Reply::PairJoined {
                session,
                result: Ok(Paired {
                    name: "Laptop".into(),
                    fingerprint: "X".into(),
                }),
            }),
        );
        assert_eq!(a.pair.result, Some(PairResult::Linked("Laptop".into())));
        assert!(!a.pair.joining);
    }

    #[test]
    fn join_refuses_text_that_is_not_a_ticket_and_reports_daemon_errors() {
        let mut a = app();
        update(&mut a, ch('3'));
        update(&mut a, ch('j'));
        update(&mut a, Msg::Paste("hello".into()));
        let fx = update(&mut a, key(KeyCode::Enter));
        assert!(fx.is_empty());
        assert!(matches!(&a.pair.result, Some(PairResult::Failed(m)) if m.contains("klip1")));
        update(&mut a, key(KeyCode::Backspace));
        a.pair.join.clear();
        update(&mut a, Msg::Paste("klip1ZZZZ".into()));
        update(&mut a, key(KeyCode::Enter));
        let session = a.pair.session;
        update(
            &mut a,
            Msg::Reply(Reply::PairJoined {
                session,
                result: Err(crate::api::ApiError::Refused(
                    "invalid or expired pairing code".into(),
                )),
            }),
        );
        assert!(matches!(&a.pair.result, Some(PairResult::Failed(m)) if m.contains("expired")));
    }

    #[test]
    fn losing_the_daemon_resets_the_pairing_state_and_replaces_the_approval() {
        let mut a = app();
        update(&mut a, ch('3'));
        started(&mut a);
        poll(&mut a, pending());
        a.inflight.status = true;
        update(
            &mut a,
            Msg::Reply(Reply::Status(Err(crate::api::ApiError::DaemonDown))),
        );
        assert!(matches!(a.modal, Some(Modal::Notice { .. })));
        assert!(
            !a.pair.ours
                && a.pair.ticket.is_none()
                && !a.pair.starting
                && a.pair.awaiting_paired_until_ms == 0
        );
    }

    #[test]
    fn qr_codes_render_for_a_ticket_in_both_polarities() {
        let t = "klip1".to_string() + &"A".repeat(120);
        let inverted = qr_lines(&t, true).unwrap();
        let normal = qr_lines(&t, false).unwrap();
        assert!(inverted.len() > 10 && inverted.len() == normal.len());
        assert_ne!(inverted, normal);
    }
}
