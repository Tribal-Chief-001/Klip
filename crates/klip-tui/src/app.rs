//! The pure core of the TUI: state, events, effects and `update`. No I/O, no terminal, no clock
//! (time arrives as `Msg::Tick`). Everything here is unit-tested without a terminal.
use crate::api::{
    ApiError, DaemonStatusInfo, HelloInfo, HistoryItem, PairPollInfo, PairTicket, Paired,
    SendReport, UnpairReport,
};
use crate::fmt;
use crate::screens::devices::DevicesState;
use crate::screens::history::HistoryState;
use crate::screens::pair::PairState;
use crate::screens::send::SendState;
use crate::textbuf::TextBuf;
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

pub const MIN_COLS: u16 = 60;
pub const MIN_ROWS: u16 = 16;
/// "Until I turn it back on": ten years, as on Android.
pub const PAUSE_FOREVER_SECS: u64 = 10 * 365 * 24 * 3600;
/// What a daemon older than the Hello call accepts in one request.
pub const OLD_REQUEST_CAP: usize = 8 * 1024 * 1024;
pub const DEFAULT_CLIP_LIMIT: usize = 32 * 1024 * 1024;
/// Width of the brand mark in front of the tabs ("●─● klip  ").
pub const BRAND_COLS: u16 = 10;
const STATUS_EVERY_MS: u64 = 1000;
const TOAST_MS: u64 = 4000;
const TOAST_ERROR_MS: u64 = 7000;
const MAX_TOASTS: usize = 3;

// ------------------------------------------------------------------------------------------ screens

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Home,
    Devices,
    Pair,
    History,
    Send,
    Log,
    Settings,
}

impl Screen {
    pub const ALL: [Screen; 7] = [
        Screen::Home,
        Screen::Devices,
        Screen::Pair,
        Screen::History,
        Screen::Send,
        Screen::Log,
        Screen::Settings,
    ];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
    pub fn from_index(i: usize) -> Option<Screen> {
        Self::ALL.get(i).copied()
    }
    pub fn title(self) -> &'static str {
        match self {
            Screen::Home => "Home",
            Screen::Devices => "Devices",
            Screen::Pair => "Pair",
            Screen::History => "History",
            Screen::Send => "Send",
            Screen::Log => "Log",
            Screen::Settings => "Settings",
        }
    }
    pub fn next(self) -> Screen {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }
    pub fn prev(self) -> Screen {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

/// Narrow terminals show only the number of the inactive tabs.
pub fn is_compact(width: u16) -> bool {
    width < 90
}

pub fn tab_label(s: Screen, compact: bool, active: Screen) -> String {
    if compact && s != active {
        format!(" {} ", s.index() + 1)
    } else {
        format!(" {} {} ", s.index() + 1, s.title())
    }
}

/// Where each tab label sits in the tab bar: `(screen, start column, end column exclusive)`.
/// Shared by drawing and mouse clicks so they can never disagree.
pub fn tab_layout(width: u16, active: Screen) -> Vec<(Screen, u16, u16)> {
    let compact = is_compact(width);
    let mut x = BRAND_COLS;
    Screen::ALL
        .iter()
        .map(|s| {
            let w = tab_label(*s, compact, active).chars().count() as u16;
            let r = (*s, x, x + w);
            x += w;
            r
        })
        .collect()
}

// ------------------------------------------------------------------------------------------- state

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conn {
    Connecting,
    Up,
    Down { reason: String },
}

#[derive(Debug, Clone)]
pub enum HelloState {
    Unknown,
    /// A daemon that predates the Hello call (v0.3.6): everything that existed then works.
    Old,
    New(HelloInfo),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Ok,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
    pub until_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmAction {
    Unpair { selector: String, name: String },
    UnpairAll { count: usize },
    ClearHistory,
    StopDaemon,
}

#[derive(Debug, Clone)]
pub enum Modal {
    Help,
    Confirm {
        title: String,
        body: String,
        action: ConfirmAction,
    },
    Pause {
        selected: usize,
        custom: Option<TextBuf>,
    },
    Notice {
        title: String,
        body: String,
    },
    /// A device is asking to pair; answered with y/n on any screen.
    PairApproval {
        name: String,
        fingerprint: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseChoice {
    Resume,
    For(u64),
    Custom,
    Forever,
}

pub fn pause_choices(paused: bool) -> Vec<(PauseChoice, &'static str)> {
    let mut v = Vec::new();
    if paused {
        v.push((PauseChoice::Resume, "Resume now"));
    }
    v.push((PauseChoice::For(900), "15 minutes"));
    v.push((PauseChoice::For(3600), "1 hour"));
    v.push((PauseChoice::For(8 * 3600), "8 hours"));
    v.push((PauseChoice::Custom, "Custom (seconds)…"));
    v.push((PauseChoice::Forever, "Until I turn it back on"));
    v
}

/// One request per poll kind may be in flight; screens add their own flags here.
#[derive(Debug, Default)]
pub struct Inflight {
    pub hello: bool,
    pub status: bool,
    pub history: bool,
    pub pair_poll: bool,
    pub log: bool,
}

/// When each poll was last started (monotonic milliseconds).
#[derive(Debug, Default)]
pub struct Clock {
    pub status_ms: u64,
    pub history_ms: u64,
    pub pair_poll_ms: u64,
    pub log_ms: u64,
}

pub struct AppState {
    pub mono_ms: u64,
    pub wall_ms: u64,
    pub ticks: u64,
    pub screen: Screen,
    pub conn: Conn,
    pub status: Option<DaemonStatusInfo>,
    pub hello: HelloState,
    pub size: (u16, u16),
    pub toasts: Vec<Toast>,
    pub modal: Option<Modal>,
    pub mouse_on: bool,
    pub quit: bool,
    pub dirty: bool,
    /// Ctrl+L: the runtime clears the terminal before the next draw.
    pub force_clear: bool,
    pub inflight: Inflight,
    pub clock: Clock,
    pub devices: DevicesState,
    pub pair: PairState,
    pub history: HistoryState,
    pub send: SendState,
    /// Draw QR codes with light modules as blocks (right for light-on-dark terminals).
    pub invert_qr: bool,
}

impl AppState {
    pub fn new(size: (u16, u16)) -> Self {
        Self {
            mono_ms: 0,
            wall_ms: 0,
            ticks: 0,
            screen: Screen::Home,
            conn: Conn::Connecting,
            status: None,
            hello: HelloState::Unknown,
            size,
            toasts: Vec::new(),
            modal: None,
            mouse_on: true,
            quit: false,
            dirty: true,
            force_clear: false,
            inflight: Inflight::default(),
            clock: Clock::default(),
            devices: DevicesState::default(),
            pair: PairState::default(),
            history: HistoryState::default(),
            send: SendState::default(),
            invert_qr: true,
        }
    }

    pub fn paused(&self) -> bool {
        self.status.as_ref().map(|s| s.is_paused).unwrap_or(false)
    }

    pub fn has_capability(&self, cap: &str) -> bool {
        matches!(&self.hello, HelloState::New(h) if h.capabilities.iter().any(|c| c == cap))
    }

    /// The daemon's clip limit (the default when the daemon has not said).
    pub fn clip_limit(&self) -> usize {
        match &self.hello {
            HelloState::New(h) => h.config.max_clip_bytes,
            _ => DEFAULT_CLIP_LIMIT,
        }
    }

    /// The largest request the daemon accepts.
    pub fn request_cap(&self) -> usize {
        match &self.hello {
            HelloState::New(h) => h.max_request_bytes,
            _ => OLD_REQUEST_CAP,
        }
    }

    pub fn toast(&mut self, kind: ToastKind, text: impl Into<String>) {
        let ms = if kind == ToastKind::Error {
            TOAST_ERROR_MS
        } else {
            TOAST_MS
        };
        self.toasts.push(Toast {
            text: text.into(),
            kind,
            until_ms: self.mono_ms + ms,
        });
        while self.toasts.len() > MAX_TOASTS {
            self.toasts.remove(0);
        }
        self.dirty = true;
    }

    /// True while keystrokes go into a text field (then letter and digit shortcuts are inactive).
    pub fn text_focus(&self) -> bool {
        match &self.modal {
            Some(Modal::Pause {
                custom: Some(_), ..
            }) => true,
            Some(_) => false,
            None => crate::screens::text_focus(self),
        }
    }
}

// -------------------------------------------------------------------------------- events & effects

#[derive(Debug)]
pub enum Msg {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Paste(String),
    Resize(u16, u16),
    Tick { mono_ms: u64, wall_ms: u64 },
    Reply(Reply),
}

/// Work for the runtime: a call to the daemon or a file operation. `update` only describes it.
#[derive(Debug, Clone)]
pub enum Effect {
    Hello,
    Status,
    Pause(u64),
    Resume,
    Unpair {
        selector: String,
    },
    UnpairAll,
    StopDaemon,
    StartDaemon,
    PairStart {
        ttl_secs: u64,
        lan: bool,
        session: u64,
        restart: bool,
    },
    PairPoll {
        session: u64,
    },
    PairDecide {
        accept: bool,
        session: u64,
    },
    PairCancel,
    PairJoin {
        ticket: String,
        session: u64,
    },
    History {
        limit: usize,
    },
    CopyItem([u8; 16]),
    ClearHistory,
    Send(String),
    LoadFile(String),
}

/// The result of an effect, delivered back as `Msg::Reply`.
#[derive(Debug)]
pub enum Reply {
    Hello(Result<HelloInfo, ApiError>),
    Status(Result<DaemonStatusInfo, ApiError>),
    Paused(Result<u64, ApiError>),
    Resumed(Result<(), ApiError>),
    Unpaired(Result<String, ApiError>),
    UnpairedAll(Result<UnpairReport, ApiError>),
    DaemonStopped(Result<(), ApiError>),
    DaemonStarted(Result<u32, ApiError>),
    PairStarted {
        session: u64,
        result: Result<PairTicket, ApiError>,
    },
    PairPolled {
        session: u64,
        result: Result<PairPollInfo, ApiError>,
    },
    PairDecided {
        session: u64,
        result: Result<PairPollInfo, ApiError>,
    },
    PairCancelled(Result<(), ApiError>),
    PairJoined {
        session: u64,
        result: Result<Paired, ApiError>,
    },
    History(Result<Vec<HistoryItem>, ApiError>),
    Copied(Result<(), ApiError>),
    HistoryCleared(Result<(), ApiError>),
    Sent(Result<SendReport, ApiError>),
    FileLoaded {
        path: String,
        result: Result<String, String>,
    },
}

/// Effects to run when the app starts.
pub fn boot(app: &mut AppState) -> Vec<Effect> {
    app.inflight.status = true;
    app.inflight.hello = true;
    vec![Effect::Status, Effect::Hello]
}

pub fn update(app: &mut AppState, msg: Msg) -> Vec<Effect> {
    let mut fx = Vec::new();
    match msg {
        Msg::Key(k) => {
            if k.kind != KeyEventKind::Release {
                app.dirty = true;
                on_key(app, k, &mut fx);
            }
        }
        Msg::Paste(text) => {
            app.dirty = true;
            on_paste(app, &text, &mut fx);
        }
        Msg::Mouse(m) => on_mouse(app, m, &mut fx),
        Msg::Resize(w, h) => {
            app.size = (w, h);
            app.dirty = true;
        }
        Msg::Tick { mono_ms, wall_ms } => on_tick(app, mono_ms, wall_ms, &mut fx),
        Msg::Reply(r) => {
            app.dirty = true;
            on_reply(app, r, &mut fx);
        }
    }
    fx
}

// ------------------------------------------------------------------------------------------ input

fn go(app: &mut AppState, to: Screen, fx: &mut Vec<Effect>) {
    if app.screen == to {
        return;
    }
    let from = app.screen;
    if !matches!(app.modal, Some(Modal::PairApproval { .. })) {
        app.modal = None; // a pairing request waiting for an answer stays on top of every screen
    }
    app.screen = to;
    app.dirty = true;
    crate::screens::on_leave(app, from, fx);
    crate::screens::on_enter(app, to, fx);
}

fn on_key(app: &mut AppState, k: KeyEvent, fx: &mut Vec<Effect>) {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    if ctrl && k.code == KeyCode::Char('c') {
        app.quit = true;
        return;
    }
    if alt {
        if let KeyCode::Char(c @ '1'..='7') = k.code {
            if let Some(s) = Screen::from_index(c as usize - '1' as usize) {
                go(app, s, fx);
            }
            return;
        }
    }
    if ctrl {
        match k.code {
            KeyCode::Right => {
                let next = app.screen.next();
                go(app, next, fx);
                return;
            }
            KeyCode::Left => {
                let prev = app.screen.prev();
                go(app, prev, fx);
                return;
            }
            KeyCode::Char('l') => {
                app.force_clear = true;
                return;
            }
            _ => {}
        }
    }
    if app.modal.is_some() {
        on_modal_key(app, k, fx);
        return;
    }
    if crate::screens::text_focus(app) {
        crate::screens::key(app, k, fx);
        return;
    }
    if ctrl || alt {
        crate::screens::key(app, k, fx);
        return;
    }
    match k.code {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Char('?') => app.modal = Some(Modal::Help),
        KeyCode::Char('p') => {
            app.modal = Some(Modal::Pause {
                selected: 0,
                custom: None,
            });
        }
        KeyCode::Char('m') => {
            app.mouse_on = !app.mouse_on;
            let text = if app.mouse_on {
                "Mouse on"
            } else {
                "Mouse off: you can select text with the mouse"
            };
            app.toast(ToastKind::Info, text);
        }
        KeyCode::Char(c @ '1'..='7') => {
            if let Some(s) = Screen::from_index(c as usize - '1' as usize) {
                go(app, s, fx);
            }
        }
        KeyCode::Tab => {
            let next = app.screen.next();
            go(app, next, fx);
        }
        KeyCode::BackTab => {
            let prev = app.screen.prev();
            go(app, prev, fx);
        }
        _ => crate::screens::key(app, k, fx),
    }
}

fn on_paste(app: &mut AppState, text: &str, fx: &mut Vec<Effect>) {
    if let Some(Modal::Pause {
        custom: Some(buf), ..
    }) = &mut app.modal
    {
        let digits: String = text.chars().filter(|c| c.is_ascii_digit()).collect();
        buf.insert_str(&digits);
        return;
    }
    if app.modal.is_none() {
        crate::screens::paste(app, text, fx);
    }
}

fn on_mouse(app: &mut AppState, m: MouseEvent, fx: &mut Vec<Effect>) {
    if !app.mouse_on || app.modal.is_some() {
        return;
    }
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) if m.row == 0 => {
            let hit = tab_layout(app.size.0, app.screen)
                .into_iter()
                .find(|(_, a, b)| m.column >= *a && m.column < *b);
            if let Some((s, _, _)) = hit {
                go(app, s, fx);
            }
        }
        MouseEventKind::ScrollUp => crate::screens::scroll(app, -3),
        MouseEventKind::ScrollDown => crate::screens::scroll(app, 3),
        _ => {}
    }
}

fn on_modal_key(app: &mut AppState, k: KeyEvent, fx: &mut Vec<Effect>) {
    let Some(modal) = app.modal.take() else {
        return;
    };
    let closes = matches!(k.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q'));
    app.modal = match modal {
        Modal::Help => {
            if closes || k.code == KeyCode::Char('?') {
                None
            } else {
                Some(Modal::Help)
            }
        }
        Modal::Notice { .. } => {
            if closes {
                None
            } else {
                Some(modal)
            }
        }
        Modal::Confirm { action, .. }
            if matches!(k.code, KeyCode::Char('y') | KeyCode::Char('Y')) =>
        {
            run_confirmed(action, fx);
            None
        }
        Modal::Confirm { .. } => {
            if closes || matches!(k.code, KeyCode::Char('n') | KeyCode::Char('N')) {
                None
            } else {
                Some(modal)
            }
        }
        Modal::Pause { selected, custom } => pause_key(app, selected, custom, k, fx),
        Modal::PairApproval { .. } => match k.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                crate::screens::pair::decide(app, true, fx);
                None
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                crate::screens::pair::decide(app, false, fx);
                None
            }
            _ => Some(modal),
        },
    };
}

fn run_confirmed(action: ConfirmAction, fx: &mut Vec<Effect>) {
    match action {
        ConfirmAction::Unpair { selector, .. } => fx.push(Effect::Unpair { selector }),
        ConfirmAction::UnpairAll { .. } => fx.push(Effect::UnpairAll),
        ConfirmAction::ClearHistory => fx.push(Effect::ClearHistory),
        ConfirmAction::StopDaemon => fx.push(Effect::StopDaemon),
    }
}

/// Keys inside the pause menu. Returns the modal to keep open, or `None` to close it.
fn pause_key(
    app: &mut AppState,
    mut selected: usize,
    mut custom: Option<TextBuf>,
    k: KeyEvent,
    fx: &mut Vec<Effect>,
) -> Option<Modal> {
    let choices = pause_choices(app.paused());
    if let Some(buf) = custom.as_mut() {
        match k.code {
            KeyCode::Esc => custom = None,
            KeyCode::Backspace => buf.backspace(),
            KeyCode::Char(c) if c.is_ascii_digit() => {
                buf.insert_char(c);
            }
            KeyCode::Enter => {
                let secs: u64 = buf.text().parse().unwrap_or(0);
                if secs == 0 || secs > PAUSE_FOREVER_SECS {
                    app.toast(
                        ToastKind::Error,
                        "Enter a number of seconds from 1 to 315360000",
                    );
                } else {
                    fx.push(Effect::Pause(secs));
                    return None;
                }
            }
            _ => {}
        }
        return Some(Modal::Pause { selected, custom });
    }
    match k.code {
        KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1).min(choices.len() - 1),
        KeyCode::Esc | KeyCode::Char('q') => return None,
        KeyCode::Enter => match choices[selected.min(choices.len() - 1)].0 {
            PauseChoice::Resume => {
                fx.push(Effect::Resume);
                return None;
            }
            PauseChoice::For(secs) => {
                fx.push(Effect::Pause(secs));
                return None;
            }
            PauseChoice::Forever => {
                fx.push(Effect::Pause(PAUSE_FOREVER_SECS));
                return None;
            }
            PauseChoice::Custom => custom = Some(TextBuf::new(12)),
        },
        _ => {}
    }
    Some(Modal::Pause { selected, custom })
}

// ------------------------------------------------------------------------------------------- time

fn on_tick(app: &mut AppState, mono_ms: u64, wall_ms: u64, fx: &mut Vec<Effect>) {
    app.mono_ms = mono_ms;
    app.wall_ms = wall_ms;
    app.ticks += 1;
    let before = app.toasts.len();
    app.toasts.retain(|t| t.until_ms > mono_ms);
    if app.toasts.len() != before || (app.ticks & 3) == 0 || matches!(app.conn, Conn::Connecting) {
        app.dirty = true;
    }
    if !app.inflight.status && mono_ms.saturating_sub(app.clock.status_ms) >= STATUS_EVERY_MS {
        app.inflight.status = true;
        app.clock.status_ms = mono_ms;
        fx.push(Effect::Status);
    }
    crate::screens::on_tick(app, fx);
}

// ----------------------------------------------------------------------------------------- replies

fn on_reply(app: &mut AppState, reply: Reply, fx: &mut Vec<Effect>) {
    match reply {
        Reply::Hello(res) => {
            app.inflight.hello = false;
            match res {
                Ok(h) => app.hello = HelloState::New(h),
                Err(ApiError::Unsupported) | Err(ApiError::Refused(_)) => {
                    app.hello = HelloState::Old
                }
                Err(_) => {} // no answer says nothing about the protocol; asked again after reconnecting
            }
        }
        Reply::Status(res) => {
            app.inflight.status = false;
            match res {
                Ok(s) => on_status(app, s, fx),
                Err(e) => on_down(app, e),
            }
        }
        Reply::Paused(Ok(secs)) => {
            app.toast(ToastKind::Ok, format!("Paused: {}", fmt::remaining(secs)));
            app.clock.status_ms = 0;
        }
        Reply::Paused(Err(e)) => {
            app.toast(ToastKind::Error, format!("Could not pause: {}", e.plain()))
        }
        Reply::Resumed(Ok(())) => {
            app.toast(ToastKind::Ok, "Syncing resumed");
            app.clock.status_ms = 0;
        }
        Reply::Resumed(Err(e)) => {
            app.toast(ToastKind::Error, format!("Could not resume: {}", e.plain()))
        }
        Reply::Unpaired(Ok(name)) => {
            app.toast(
                ToastKind::Ok,
                format!("Unpaired {}", crate::sanitize::one_line(&name)),
            );
            app.clock.status_ms = 0;
        }
        Reply::Unpaired(Err(e)) => {
            app.toast(ToastKind::Error, format!("Could not unpair: {}", e.plain()))
        }
        Reply::UnpairedAll(Ok((removed, failed))) => {
            app.clock.status_ms = 0;
            if failed.is_empty() {
                app.toast(
                    ToastKind::Ok,
                    format!("Unpaired {} device(s)", removed.len()),
                );
            } else {
                let list: Vec<String> = failed
                    .iter()
                    .map(|(n, why)| {
                        format!(
                            "{}: {}",
                            crate::sanitize::one_line(n),
                            crate::sanitize::one_line(why)
                        )
                    })
                    .collect();
                app.modal = Some(Modal::Notice {
                    title: format!(
                        "{} removed, {} could not be removed",
                        removed.len(),
                        failed.len()
                    ),
                    body: list.join("\n"),
                });
            }
        }
        Reply::UnpairedAll(Err(e)) => {
            app.toast(ToastKind::Error, format!("Could not unpair: {}", e.plain()))
        }
        Reply::DaemonStopped(Ok(())) => app.toast(ToastKind::Info, "Klip stopped"),
        Reply::DaemonStopped(Err(e)) => app.toast(
            ToastKind::Error,
            format!("Could not stop Klip: {}", e.plain()),
        ),
        Reply::DaemonStarted(Ok(pid)) => {
            app.toast(ToastKind::Ok, format!("Klip started (PID {pid})"))
        }
        Reply::PairStarted { session, result } => {
            crate::screens::pair::on_started(app, session, result)
        }
        Reply::PairPolled { session, result } => {
            crate::screens::pair::on_polled(app, session, result)
        }
        Reply::PairDecided { session, result } => {
            crate::screens::pair::on_decided(app, session, result)
        }
        Reply::PairCancelled(_) => {}
        Reply::History(res) => crate::screens::history::on_items(app, res),
        Reply::Copied(Ok(())) => app.toast(ToastKind::Ok, "Copied to this computer's clipboard"),
        Reply::Copied(Err(e)) => {
            app.toast(ToastKind::Error, format!("Could not copy: {}", e.plain()))
        }
        Reply::HistoryCleared(Ok(())) => {
            crate::screens::history::on_cleared(app);
            app.toast(ToastKind::Ok, "History cleared");
        }
        Reply::HistoryCleared(Err(e)) => app.toast(
            ToastKind::Error,
            format!("Could not clear the history: {}", e.plain()),
        ),
        Reply::Sent(res) => on_sent(app, res),
        Reply::FileLoaded { path, result } => {
            crate::screens::send::on_file_loaded(app, path, result)
        }
        Reply::PairJoined { session, result } => {
            crate::screens::pair::on_joined(app, session, result)
        }
        Reply::DaemonStarted(Err(e)) => {
            app.modal = Some(Modal::Notice {
                title: "Could not start Klip".to_string(),
                body: format!("{}\n\nThe Log screen (6) shows more.", e.plain()),
            });
        }
    }
}

fn on_sent(app: &mut AppState, res: Result<SendReport, ApiError>) {
    crate::screens::send::on_sent(app, &res);
    match res {
        Ok(r) if r.delivered.is_empty() && r.failed.is_empty() => app.toast(
            ToastKind::Warn,
            "No linked devices: nothing was sent. Pair one first (3).",
        ),
        Ok(r) if r.failed.is_empty() => app.toast(
            ToastKind::Ok,
            format!(
                "Sent {} to {} device(s)",
                fmt::bytes(r.bytes),
                r.delivered.len()
            ),
        ),
        Ok(r) => app.toast(
            ToastKind::Warn,
            format!(
                "Delivered to {}, failed for {}",
                r.delivered.len(),
                r.failed.len()
            ),
        ),
        Err(e) => app.toast(ToastKind::Error, format!("Could not send: {}", e.plain())),
    }
}

fn on_status(app: &mut AppState, s: DaemonStatusInfo, fx: &mut Vec<Effect>) {
    let from_down = matches!(app.conn, Conn::Down { .. });
    app.conn = Conn::Up;
    app.status = Some(s);
    let n = app.status.as_ref().map(|s| s.peers.len()).unwrap_or(0);
    app.devices.selected = app.devices.selected.min(n.saturating_sub(1));
    if from_down {
        // The daemon may have been restarted (even upgraded): ask again what it can do.
        app.hello = HelloState::Unknown;
        if !app.inflight.hello {
            app.inflight.hello = true;
            fx.push(Effect::Hello);
        }
    }
}

fn on_down(app: &mut AppState, e: ApiError) {
    let was_down = matches!(app.conn, Conn::Down { .. });
    app.conn = Conn::Down { reason: e.plain() };
    app.status = None;
    app.devices.selected = 0;
    if was_down {
        return;
    }
    if matches!(
        app.modal,
        Some(Modal::Confirm { .. }) | Some(Modal::Pause { .. }) | Some(Modal::PairApproval { .. })
    ) {
        app.modal = Some(Modal::Notice {
            title: "Klip stopped answering".to_string(),
            body:
                "Your choice was not applied. This screen reconnects by itself when Klip is back."
                    .to_string(),
        });
    }
    crate::screens::on_daemon_down(app);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::sample_status;
    use crossterm::event::{KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

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
        a
    }
    fn tick(a: &mut AppState, t: u64) -> Vec<Effect> {
        update(
            a,
            Msg::Tick {
                mono_ms: t,
                wall_ms: t,
            },
        )
    }

    #[test]
    fn q_and_ctrl_c_quit_even_inside_a_modal() {
        let mut a = app();
        update(&mut a, ch('q'));
        assert!(a.quit);
        let mut a = app();
        update(&mut a, ch('?'));
        assert!(matches!(a.modal, Some(Modal::Help)));
        update(
            &mut a,
            Msg::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        );
        assert!(a.quit);
    }

    #[test]
    fn key_release_events_are_ignored() {
        let mut a = app();
        let mut ev = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        ev.kind = KeyEventKind::Release;
        update(&mut a, Msg::Key(ev));
        assert!(!a.quit);
    }

    #[test]
    fn digits_tab_and_alt_digits_switch_screens() {
        let mut a = app();
        update(&mut a, ch('3'));
        assert_eq!(a.screen, Screen::Pair);
        update(&mut a, key(KeyCode::Tab));
        assert_eq!(a.screen, Screen::History);
        update(&mut a, key(KeyCode::BackTab));
        assert_eq!(a.screen, Screen::Pair);
        update(
            &mut a,
            Msg::Key(KeyEvent::new(KeyCode::Char('7'), KeyModifiers::ALT)),
        );
        assert_eq!(a.screen, Screen::Settings);
        update(
            &mut a,
            Msg::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL)),
        );
        assert_eq!(a.screen, Screen::Home, "Ctrl+Right wraps around");
    }

    #[test]
    fn the_pause_menu_offers_durations_and_resume_only_when_paused() {
        let mut a = app();
        assert_eq!(pause_choices(false).len(), 5);
        assert_eq!(pause_choices(true).len(), 6);
        update(&mut a, ch('p'));
        update(&mut a, key(KeyCode::Down));
        let fx = update(&mut a, key(KeyCode::Enter));
        assert!(matches!(fx.as_slice(), [Effect::Pause(3600)]), "{fx:?}");
        assert!(a.modal.is_none());

        let mut a = app();
        a.status.as_mut().unwrap().is_paused = true;
        update(&mut a, ch('p'));
        let fx = update(&mut a, key(KeyCode::Enter));
        assert!(matches!(fx.as_slice(), [Effect::Resume]), "{fx:?}");
    }

    #[test]
    fn a_custom_pause_takes_digits_only_and_refuses_zero() {
        let mut a = app();
        update(&mut a, ch('p'));
        for _ in 0..3 {
            update(&mut a, key(KeyCode::Down));
        }
        update(&mut a, key(KeyCode::Enter)); // Custom…
        assert!(a.text_focus());
        update(&mut a, ch('0'));
        let fx = update(&mut a, key(KeyCode::Enter));
        assert!(
            fx.is_empty() && a.modal.is_some(),
            "zero seconds is refused"
        );
        assert!(a.toasts.iter().any(|t| t.kind == ToastKind::Error));
        update(&mut a, key(KeyCode::Backspace));
        for c in "9x0".chars() {
            update(&mut a, ch(c));
        }
        let fx = update(&mut a, key(KeyCode::Enter));
        assert!(matches!(fx.as_slice(), [Effect::Pause(90)]), "{fx:?}");
    }

    #[test]
    fn alt_digit_works_inside_a_text_field_and_closes_the_modal() {
        let mut a = app();
        update(&mut a, ch('p'));
        for _ in 0..3 {
            update(&mut a, key(KeyCode::Down));
        }
        update(&mut a, key(KeyCode::Enter));
        assert!(a.text_focus());
        update(
            &mut a,
            Msg::Key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::ALT)),
        );
        assert_eq!(a.screen, Screen::Pair);
        assert!(a.modal.is_none());
    }

    #[test]
    fn status_is_polled_once_a_second_and_never_piled_up() {
        let mut a = app();
        assert!(tick(&mut a, 500).is_empty());
        assert!(matches!(tick(&mut a, 1000).as_slice(), [Effect::Status]));
        assert!(tick(&mut a, 1250).is_empty());
        assert!(
            tick(&mut a, 2500).is_empty(),
            "a request is still in flight"
        );
        update(&mut a, Msg::Reply(Reply::Status(Ok(sample_status(vec![])))));
        assert!(matches!(tick(&mut a, 2600).as_slice(), [Effect::Status]));
        assert!(tick(&mut a, 2700).is_empty());
    }

    #[test]
    fn losing_the_daemon_replaces_a_decision_modal_and_regaining_it_asks_hello_again() {
        let mut a = app();
        a.hello = HelloState::Old;
        a.modal = Some(Modal::Confirm {
            title: "Stop Klip?".into(),
            body: String::new(),
            action: ConfirmAction::StopDaemon,
        });
        a.inflight.status = true;
        update(&mut a, Msg::Reply(Reply::Status(Err(ApiError::DaemonDown))));
        assert!(matches!(a.conn, Conn::Down { .. }));
        assert!(a.status.is_none());
        assert!(
            matches!(a.modal, Some(Modal::Notice { .. })),
            "the decision cannot be applied any more"
        );

        a.inflight.status = true;
        let fx = update(&mut a, Msg::Reply(Reply::Status(Ok(sample_status(vec![])))));
        assert!(matches!(a.conn, Conn::Up));
        assert!(matches!(a.hello, HelloState::Unknown));
        assert!(fx.iter().any(|e| matches!(e, Effect::Hello)), "{fx:?}");
    }

    #[test]
    fn hello_replies_decide_the_protocol_state() {
        let mut a = app();
        update(
            &mut a,
            Msg::Reply(Reply::Hello(Ok(crate::testkit::sample_hello()))),
        );
        assert!(a.has_capability("copy_to_clipboard"));
        assert_eq!(a.clip_limit(), 32 * 1024 * 1024);
        let mut b = app();
        update(&mut b, Msg::Reply(Reply::Hello(Err(ApiError::Unsupported))));
        assert!(matches!(b.hello, HelloState::Old));
        assert!(!b.has_capability("copy_to_clipboard"));
        assert_eq!(b.request_cap(), OLD_REQUEST_CAP);
        let mut c = app();
        update(&mut c, Msg::Reply(Reply::Hello(Err(ApiError::Timeout(15)))));
        assert!(
            matches!(c.hello, HelloState::Unknown),
            "a timeout says nothing about the protocol"
        );
    }

    #[test]
    fn toasts_expire() {
        let mut a = app();
        tick(&mut a, 0);
        a.toast(ToastKind::Info, "hello");
        tick(&mut a, 3_900);
        assert_eq!(a.toasts.len(), 1);
        tick(&mut a, 4_100);
        assert!(a.toasts.is_empty());
    }

    #[test]
    fn tabs_are_clickable_and_compact_on_narrow_terminals() {
        let wide = tab_layout(120, Screen::Home);
        assert_eq!(wide[0], (Screen::Home, BRAND_COLS, BRAND_COLS + 8));
        let narrow = tab_layout(60, Screen::Devices);
        let total_end = narrow.last().unwrap().2;
        assert!(
            total_end <= 60,
            "the compact tab bar must fit in 60 columns, ends at {total_end}"
        );

        let mut a = app();
        let (_, start, _) = wide[3]; // History
        update(
            &mut a,
            Msg::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: start + 1,
                row: 0,
                modifiers: KeyModifiers::NONE,
            }),
        );
        assert_eq!(a.screen, Screen::History);
        a.mouse_on = false;
        update(
            &mut a,
            Msg::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: BRAND_COLS + 1,
                row: 0,
                modifiers: KeyModifiers::NONE,
            }),
        );
        assert_eq!(a.screen, Screen::History, "mouse off: clicks are ignored");
    }

    #[test]
    fn replies_for_actions_become_toasts() {
        let mut a = app();
        update(&mut a, Msg::Reply(Reply::Unpaired(Ok("Pixel".into()))));
        assert!(a
            .toasts
            .iter()
            .any(|t| t.text.contains("Pixel") && t.kind == ToastKind::Ok));
        update(&mut a, Msg::Reply(Reply::Paused(Err(ApiError::DaemonDown))));
        assert!(a.toasts.iter().any(|t| t.kind == ToastKind::Error));
        update(
            &mut a,
            Msg::Reply(Reply::DaemonStarted(Err(ApiError::Refused("boom".into())))),
        );
        assert!(matches!(a.modal, Some(Modal::Notice { .. })));
    }
}
