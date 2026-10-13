//! The Settings screen: edit `config.json` with the daemon's own validation, restart Klip to apply it,
//! install the auto-start unit. Reads and writes the file directly, so it works with Klip stopped.
use crate::app::{AppState, ConfirmAction, Effect, Modal, ToastKind};
use crate::files::ServiceState;
use crate::fmt;
use crate::sanitize::{clean, one_line, tail, truncate};
use crate::textbuf::TextBuf;
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use klip_core::{DaemonConfig, RelayMode};
use klip_proto::ipc::ConfigInfo;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    DeviceName,
    Relay,
    HistoryItems,
    ClipSize,
    LanDiscovery,
    PollInterval,
    Service,
    Restart,
    Reset,
}

pub const ROW_ORDER: [Row; 9] = [
    Row::DeviceName,
    Row::Relay,
    Row::HistoryItems,
    Row::ClipSize,
    Row::LanDiscovery,
    Row::PollInterval,
    Row::Service,
    Row::Restart,
    Row::Reset,
];

#[derive(Debug, Default)]
pub struct SettingsState {
    /// The last successful read of `config.json` (a missing file reads as the defaults).
    pub file: Option<DaemonConfig>,
    /// Why the file could not be read, when it could not.
    pub load_error: Option<String>,
    pub draft: DaemonConfig,
    /// What a daemon that cannot report its settings (v0.3.6) was started with: the file as first read.
    pub baseline: Option<DaemonConfig>,
    pub selected: usize,
    pub editing: Option<TextBuf>,
    /// Why the value being edited was refused.
    pub error: Option<String>,
    pub restarting: bool,
    pub service: Option<ServiceState>,
}

/// Field-by-field equality (`DaemonConfig` does not implement `PartialEq`).
pub fn same(a: &DaemonConfig, b: &DaemonConfig) -> bool {
    a.device_name == b.device_name
        && a.relay == b.relay
        && a.max_history_items == b.max_history_items
        && a.poll_interval_ms == b.poll_interval_ms
        && a.max_clip_bytes == b.max_clip_bytes
        && a.lan_discovery == b.lan_discovery
}

pub fn info_of(c: &DaemonConfig) -> ConfigInfo {
    ConfigInfo {
        device_name: c.device_name.clone(),
        relay: match c.relay {
            RelayMode::N0 => "n0".to_string(),
            RelayMode::Lan => "lan".to_string(),
        },
        max_history_items: c.max_history_items,
        max_clip_bytes: c.max_clip_bytes,
        lan_discovery: c.lan_discovery,
    }
}

pub fn dirty(app: &AppState) -> bool {
    app.settings
        .file
        .as_ref()
        .map(|f| !same(f, &app.settings.draft))
        .unwrap_or(false)
}

/// True when the saved file differs from what the running daemon uses.
pub fn needs_restart(app: &AppState) -> bool {
    let Some(file) = &app.settings.file else {
        return false;
    };
    let running = match &app.hello {
        crate::app::HelloState::New(h) => Some(h.config.clone()),
        _ => app.settings.baseline.as_ref().map(info_of),
    };
    running.map(|r| r != info_of(file)).unwrap_or(false)
}

/// "32 MiB", "512kb", "1.5 MiB", "1,048,576", "2gib": binary units (1 KiB = 1024 bytes).
pub fn parse_size(s: &str) -> Result<usize, String> {
    let t: String = s
        .trim()
        .chars()
        .filter(|c| *c != '_' && *c != ',')
        .collect();
    if t.is_empty() {
        return Err("type a size, for example 32 MiB".to_string());
    }
    let split = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(t.len());
    let (num, unit) = t.split_at(split);
    let value: f64 = num
        .parse()
        .map_err(|_| format!("'{num}' is not a number"))?;
    let mult: f64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "k" | "kb" | "kib" => 1024.0,
        "m" | "mb" | "mib" => 1024.0 * 1024.0,
        "g" | "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        other => return Err(format!("unknown unit '{other}': use B, KiB, MiB or GiB")),
    };
    let bytes = value * mult;
    if !bytes.is_finite() || bytes < 1.0 || bytes > usize::MAX as f64 {
        return Err("that size is out of range".to_string());
    }
    Ok(bytes as usize)
}

fn row(app: &AppState) -> Row {
    ROW_ORDER[app.settings.selected.min(ROW_ORDER.len() - 1)]
}

pub fn text_focus(app: &AppState) -> bool {
    app.settings.editing.is_some()
}

pub fn on_enter(fx: &mut Vec<Effect>) {
    fx.push(Effect::ReadConfig);
    fx.push(Effect::ReadService);
}

pub fn on_leave(app: &mut AppState) {
    app.settings.editing = None;
    app.settings.error = None;
}

pub fn on_config(app: &mut AppState, res: Result<DaemonConfig, String>) {
    let s = &mut app.settings;
    match res {
        Ok(cfg) => {
            let was_clean = s.file.as_ref().map(|f| same(f, &s.draft)).unwrap_or(true);
            if s.baseline.is_none() {
                s.baseline = Some(cfg.clone());
            }
            s.load_error = None;
            s.file = Some(cfg.clone());
            if was_clean {
                s.draft = cfg;
            }
        }
        Err(e) => {
            s.load_error = Some(e);
            s.file = None;
        }
    }
}

pub fn on_saved(app: &mut AppState, res: Result<(), String>) {
    match res {
        Ok(()) => {
            app.settings.file = Some(app.settings.draft.clone());
            app.settings.load_error = None;
            app.toast(ToastKind::Ok, "Saved. Restart Klip to apply it (R).");
        }
        Err(e) => app.toast(ToastKind::Error, format!("Not saved: {}", one_line(&e))),
    }
}

pub fn on_restarted(app: &mut AppState, res: Result<u32, crate::api::ApiError>) {
    app.settings.restarting = false;
    match res {
        Ok(pid) => app.toast(ToastKind::Ok, format!("Klip restarted (PID {pid})")),
        Err(e) => {
            app.modal = Some(Modal::Notice {
                title: "Klip did not restart".to_string(),
                body: format!("{}\n\nThe Log screen (6) shows why.", e.plain()),
            });
        }
    }
}

fn commit(app: &mut AppState) {
    let Some(buf) = app.settings.editing.as_ref() else {
        return;
    };
    let text = buf.text();
    let mut next = app.settings.draft.clone();
    let result: Result<(), String> = match row(app) {
        Row::DeviceName => {
            let name = text.trim();
            if name.is_empty() {
                Err("the name cannot be empty".to_string())
            } else if name.chars().count() > 64 {
                Err("the name can have at most 64 characters".to_string())
            } else if name.chars().any(|c| c.is_control()) {
                Err("the name cannot contain control characters".to_string())
            } else {
                next.device_name = name.to_string();
                Ok(())
            }
        }
        Row::HistoryItems => match text.trim().parse::<usize>() {
            Ok(n) if (1..=1000).contains(&n) => {
                next.max_history_items = n;
                Ok(())
            }
            _ => Err("enter a number from 1 to 1000".to_string()),
        },
        Row::ClipSize => parse_size(&text).map(|n| next.max_clip_bytes = n),
        _ => Ok(()),
    };
    let result = result.and_then(|_| next.validate());
    match result {
        Ok(()) => {
            app.settings.draft = next;
            app.settings.editing = None;
            app.settings.error = None;
        }
        Err(e) => app.settings.error = Some(e),
    }
}

pub fn paste(app: &mut AppState, text: &str) {
    if let Some(b) = app.settings.editing.as_mut() {
        let one: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
        b.insert_str(&one);
    }
}

pub fn scroll(app: &mut AppState, delta: i32) {
    let next =
        (app.settings.selected as i64 + i64::from(delta)).clamp(0, ROW_ORDER.len() as i64 - 1);
    app.settings.selected = next as usize;
}

pub fn key(app: &mut AppState, k: KeyEvent, fx: &mut Vec<Effect>) {
    if app.settings.editing.is_some() {
        return edit_key(app, k);
    }
    match k.code {
        KeyCode::Up | KeyCode::Char('k') => scroll(app, -1),
        KeyCode::Down | KeyCode::Char('j') => scroll(app, 1),
        KeyCode::Char('s') => {
            if app.settings.load_error.is_some() {
                app.toast(
                    ToastKind::Warn,
                    "config.json has an error: reset it from the last row first",
                );
            } else {
                fx.push(Effect::WriteConfig(app.settings.draft.clone()));
            }
        }
        KeyCode::Char('u') => {
            if let Some(f) = app.settings.file.clone() {
                app.settings.draft = f;
                app.settings.error = None;
            }
        }
        KeyCode::Char('R') => ask_restart(app),
        KeyCode::Enter | KeyCode::Char(' ') => activate(app, fx),
        _ => {}
    }
}

fn ask_restart(app: &mut AppState) {
    app.modal = Some(Modal::Confirm {
        title: "Restart Klip?".to_string(),
        body: "Klip stops and starts again so it reads the saved settings. Syncing pauses for a few seconds.".to_string(),
        action: ConfirmAction::RestartDaemon,
    });
}

fn activate(app: &mut AppState, fx: &mut Vec<Effect>) {
    match row(app) {
        Row::DeviceName => {
            app.settings.editing = Some(TextBuf::from_text(&app.settings.draft.device_name, 256));
            if let Some(b) = app.settings.editing.as_mut() {
                b.end();
            }
        }
        Row::HistoryItems => {
            let mut b = TextBuf::from_text(&app.settings.draft.max_history_items.to_string(), 16);
            b.end();
            app.settings.editing = Some(b);
        }
        Row::ClipSize => {
            let mut b = TextBuf::from_text(
                &fmt::bytes(app.settings.draft.max_clip_bytes).replace(',', ""),
                32,
            );
            b.end();
            app.settings.editing = Some(b);
        }
        Row::Relay => {
            app.settings.draft.relay = match app.settings.draft.relay {
                RelayMode::N0 => RelayMode::Lan,
                RelayMode::Lan => RelayMode::N0,
            };
        }
        Row::LanDiscovery => app.settings.draft.lan_discovery = !app.settings.draft.lan_discovery,
        Row::PollInterval => app.toast(
            ToastKind::Info,
            "Advanced: edit poll_interval_ms in config.json by hand",
        ),
        Row::Service => match &app.settings.service {
            Some(ServiceState::NotInstalled) => fx.push(Effect::InstallService),
            Some(ServiceState::Installed(p)) => {
                let msg = format!("Already installed: {}", p.display());
                app.toast(ToastKind::Info, msg);
            }
            _ => app.toast(
                ToastKind::Info,
                "Auto-start is not available on this system",
            ),
        },
        Row::Restart => ask_restart(app),
        Row::Reset => {
            app.modal = Some(Modal::Confirm {
                title: "Reset all settings?".to_string(),
                body:
                    "config.json is replaced by the defaults. Klip must be restarted to use them."
                        .to_string(),
                action: ConfirmAction::ResetConfig,
            });
        }
    }
    app.settings.error = None;
}

fn edit_key(app: &mut AppState, k: KeyEvent) {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    match k.code {
        KeyCode::Esc => {
            app.settings.editing = None;
            app.settings.error = None;
        }
        KeyCode::Enter => commit(app),
        _ => {
            if let Some(b) = app.settings.editing.as_mut() {
                match k.code {
                    KeyCode::Backspace => b.backspace(),
                    KeyCode::Delete => b.delete(),
                    KeyCode::Left => b.left(),
                    KeyCode::Right => b.right(),
                    KeyCode::Home => b.home(),
                    KeyCode::End => b.end(),
                    KeyCode::Char('u') if ctrl => b.clear(),
                    KeyCode::Char('w') if ctrl => b.delete_word_back(),
                    KeyCode::Char(c) if !ctrl => {
                        b.insert_char(c);
                    }
                    _ => {}
                }
            }
        }
    }
}

// -------------------------------------------------------------------------------------- drawing

fn label(r: Row) -> &'static str {
    match r {
        Row::DeviceName => "Device name",
        Row::Relay => "Network",
        Row::HistoryItems => "History items",
        Row::ClipSize => "Largest clip",
        Row::LanDiscovery => "LAN discovery",
        Row::PollInterval => "Poll interval",
        Row::Service => "Auto-start",
        Row::Restart => "Apply",
        Row::Reset => "Defaults",
    }
}

fn value(app: &AppState, r: Row) -> String {
    let d = &app.settings.draft;
    match r {
        Row::DeviceName => one_line(&d.device_name),
        Row::Relay => match d.relay {
            RelayMode::N0 => {
                "n0 public relays: they see who talks to whom, never the content".to_string()
            }
            RelayMode::Lan => {
                "lan only: no third-party server, devices must be on the same network".to_string()
            }
        },
        Row::HistoryItems => d.max_history_items.to_string(),
        Row::ClipSize => fmt::bytes(d.max_clip_bytes),
        Row::LanDiscovery => if d.lan_discovery { "on" } else { "off" }.to_string(),
        Row::PollInterval => format!("{} ms (advanced; edit config.json)", d.poll_interval_ms),
        Row::Service => match &app.settings.service {
            Some(ServiceState::Installed(p)) => {
                format!("installed ({})", one_line(&p.display().to_string()))
            }
            Some(ServiceState::NotInstalled) => "not installed: Enter installs it".to_string(),
            Some(ServiceState::Unsupported) => "not available on this system".to_string(),
            None => "checking…".to_string(),
        },
        Row::Restart => {
            if app.settings.restarting {
                "restarting Klip…".to_string()
            } else {
                "restart Klip so it reads the saved settings (R)".to_string()
            }
        }
        Row::Reset => "reset every setting to its default".to_string(),
    }
}

pub fn view(app: &AppState, theme: &Theme, f: &mut Frame, area: Rect) {
    let s = &app.settings;
    let block = Block::bordered()
        .title(Span::styled(" Settings ", theme.st_title()))
        .border_style(theme.st_faint());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [banner, rows, about] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(4),
        Constraint::Length(3),
    ])
    .areas(inner);

    let mut notes: Vec<Line> = Vec::new();
    if let Some(e) = &s.load_error {
        notes.push(Line::from(Span::styled(
            format!("✗ config.json cannot be read: {}", one_line(e)),
            theme.st_bad(),
        )));
        notes.push(Line::from(Span::styled(
            "It is left untouched. Reset it from the last row, or fix it by hand.",
            theme.st_dim(),
        )));
    } else {
        if dirty(app) {
            notes.push(Line::from(Span::styled(
                "● unsaved changes: s saves, u discards",
                theme.st_warn(),
            )));
        }
        if needs_restart(app) {
            notes.push(Line::from(Span::styled(
                "● the running Klip uses other settings: R restarts it",
                theme.st_warn(),
            )));
        }
        if notes.is_empty() {
            notes.push(Line::from(Span::styled(
                "Everything is saved and in use.",
                theme.st_dim(),
            )));
        }
    }
    f.render_widget(Paragraph::new(notes).wrap(Wrap { trim: true }), banner);

    let w = rows.width as usize;
    let (start, end) = crate::screens::window(s.selected, ROW_ORDER.len(), rows.height as usize);
    let mut lines: Vec<Line> = Vec::new();
    for (i, r) in ROW_ORDER.iter().enumerate().take(end).skip(start) {
        let sel = i == s.selected;
        let base = if sel {
            theme.st_selected()
        } else {
            theme.st_text()
        };
        let room = w.saturating_sub(18);
        // While typing, the end (where the cursor is) matters; otherwise the start of the value does.
        let text = match (&s.editing, sel) {
            (Some(b), true) => tail(&format!("{}▏", clean(&b.text())), room),
            _ => truncate(&value(app, *r), room),
        };
        lines.push(Line::from(vec![
            Span::styled(if sel { "▸ " } else { "  " }, base),
            Span::styled(
                format!("{:<15}", label(*r)),
                if sel { base } else { theme.st_dim() },
            ),
            Span::styled(text, base),
        ]));
        if sel {
            if let Some(e) = &s.error {
                lines.push(Line::from(Span::styled(
                    format!("    ✗ {}", one_line(e)),
                    theme.st_bad(),
                )));
            }
        }
    }
    f.render_widget(Paragraph::new(lines), rows);

    let daemon = match &app.hello {
        crate::app::HelloState::New(h) => format!("daemon {}", one_line(&h.daemon_version)),
        crate::app::HelloState::Old => {
            "daemon older than this program (it reports no version)".to_string()
        }
        crate::app::HelloState::Unknown => "daemon: not known yet".to_string(),
    };
    let tui = env!("CARGO_PKG_VERSION");
    let mut about_lines = vec![Line::from(Span::styled(
        truncate(&format!("klip {tui} · {daemon}"), about.width as usize),
        theme.st_dim(),
    ))];
    if let crate::app::HelloState::New(h) = &app.hello {
        if h.daemon_version != tui {
            about_lines.push(Line::from(Span::styled(
                "The daemon runs another version than this program: restart it (klip stop, klip start).",
                theme.st_warn(),
            )));
        }
    }
    f.render_widget(Paragraph::new(about_lines).wrap(Wrap { trim: true }), about);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{update, Conn, HelloState, Modal, Msg, Reply, Screen};
    use crate::testkit::{sample_hello, sample_status};
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> Msg {
        Msg::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }
    fn ch(c: char) -> Msg {
        key(KeyCode::Char(c))
    }
    /// The settings `sample_hello` says the daemon runs (the default device name is the hostname).
    fn running_cfg() -> DaemonConfig {
        DaemonConfig {
            device_name: "Laptop".to_string(),
            ..DaemonConfig::default()
        }
    }
    fn app() -> AppState {
        let mut a = AppState::new((100, 30));
        a.conn = Conn::Up;
        a.status = Some(sample_status(vec![]));
        a.hello = HelloState::New(sample_hello());
        update(&mut a, Msg::Reply(Reply::Config(Ok(running_cfg()))));
        update(&mut a, ch('7'));
        assert_eq!(a.screen, Screen::Settings);
        a
    }
    fn select(a: &mut AppState, row: Row) {
        a.settings.selected = ROW_ORDER.iter().position(|r| *r == row).unwrap();
    }

    #[test]
    fn sizes_parse_with_binary_units_and_reject_nonsense() {
        assert_eq!(parse_size("32 MiB").unwrap(), 33_554_432);
        assert_eq!(parse_size("512kb").unwrap(), 524_288);
        assert_eq!(parse_size("1.5 MiB").unwrap(), 1_572_864);
        assert_eq!(parse_size("1,048,576").unwrap(), 1_048_576);
        assert_eq!(parse_size("2gib").unwrap(), 2_147_483_648);
        assert_eq!(parse_size("100").unwrap(), 100);
        assert!(parse_size("").is_err());
        assert!(parse_size("0").is_err());
        assert!(parse_size("10x").unwrap_err().contains("unit"));
        assert!(parse_size("MiB").is_err());
        assert!(parse_size("1e99 GiB").is_err());
    }

    #[test]
    fn entering_the_screen_reads_the_config_and_the_service_state() {
        let mut a = AppState::new((100, 30));
        a.conn = Conn::Up;
        let fx = update(&mut a, ch('7'));
        assert!(
            fx.iter().any(|e| matches!(e, Effect::ReadConfig))
                && fx.iter().any(|e| matches!(e, Effect::ReadService)),
            "{fx:?}"
        );
    }

    #[test]
    fn toggles_flip_and_unsaved_changes_are_flagged_and_saved_on_s() {
        let mut a = app();
        select(&mut a, Row::Relay);
        update(&mut a, key(KeyCode::Enter));
        assert_eq!(a.settings.draft.relay, RelayMode::Lan);
        assert!(dirty(&a));
        select(&mut a, Row::LanDiscovery);
        update(&mut a, ch(' '));
        assert!(!a.settings.draft.lan_discovery);
        let fx = update(&mut a, ch('s'));
        assert!(
            matches!(fx.as_slice(), [Effect::WriteConfig(c)] if c.relay == RelayMode::Lan && !c.lan_discovery),
            "{fx:?}"
        );
        update(&mut a, Msg::Reply(Reply::ConfigSaved(Ok(()))));
        assert!(!dirty(&a));
        assert!(
            a.toasts.iter().any(|t| t.text.contains("Restart")),
            "{:?}",
            a.toasts
        );
    }

    #[test]
    fn u_discards_unsaved_changes() {
        let mut a = app();
        select(&mut a, Row::LanDiscovery);
        update(&mut a, key(KeyCode::Enter));
        assert!(dirty(&a));
        update(&mut a, ch('u'));
        assert!(!dirty(&a));
    }

    #[test]
    fn text_and_number_fields_validate_before_they_apply() {
        let mut a = app();
        select(&mut a, Row::DeviceName);
        update(&mut a, key(KeyCode::Enter));
        assert!(a.text_focus());
        for _ in 0..40 {
            update(&mut a, key(KeyCode::Backspace));
        }
        update(&mut a, key(KeyCode::Enter));
        assert!(a.settings.error.is_some(), "an empty name is refused");
        for c in "My Laptop".chars() {
            update(&mut a, ch(c));
        }
        update(&mut a, key(KeyCode::Enter));
        assert_eq!(a.settings.draft.device_name, "My Laptop");
        assert!(!a.text_focus());

        select(&mut a, Row::ClipSize);
        update(&mut a, key(KeyCode::Enter));
        for _ in 0..20 {
            update(&mut a, key(KeyCode::Backspace));
        }
        for c in "999 GiB".chars() {
            update(&mut a, ch(c));
        }
        update(&mut a, key(KeyCode::Enter));
        assert!(
            a.settings.error.is_some(),
            "above the daemon's 256 MiB maximum"
        );
        assert!(a.text_focus(), "the field stays open so it can be fixed");
        for _ in 0..20 {
            update(&mut a, key(KeyCode::Backspace));
        }
        for c in "8 MiB".chars() {
            update(&mut a, ch(c));
        }
        update(&mut a, key(KeyCode::Enter));
        assert_eq!(a.settings.draft.max_clip_bytes, 8 * 1024 * 1024);

        select(&mut a, Row::HistoryItems);
        update(&mut a, key(KeyCode::Enter));
        for _ in 0..10 {
            update(&mut a, key(KeyCode::Backspace));
        }
        update(&mut a, ch('0'));
        update(&mut a, key(KeyCode::Enter));
        assert!(a.settings.error.is_some(), "zero history items is refused");
    }

    #[test]
    fn escape_cancels_an_edit_and_a_control_character_name_is_refused() {
        let mut a = app();
        select(&mut a, Row::DeviceName);
        update(&mut a, key(KeyCode::Enter));
        update(&mut a, ch('X'));
        update(&mut a, key(KeyCode::Esc));
        assert!(!a.text_focus());
        assert!(!dirty(&a));
        update(&mut a, key(KeyCode::Enter));
        a.settings.editing.as_mut().unwrap().clear();
        a.settings
            .editing
            .as_mut()
            .unwrap()
            .insert_str("bad\x1b[2Jname");
        update(&mut a, key(KeyCode::Enter));
        assert!(a.settings.error.is_some());
    }

    #[test]
    fn a_malformed_config_blocks_saving_until_it_is_reset_explicitly() {
        let mut a = app();
        update(
            &mut a,
            Msg::Reply(Reply::Config(Err(
                "config.json: expected value at line 1".into()
            ))),
        );
        assert!(a.settings.load_error.is_some());
        let fx = update(&mut a, ch('s'));
        assert!(fx.is_empty(), "never overwrite a file we could not read");
        select(&mut a, Row::Reset);
        update(&mut a, key(KeyCode::Enter));
        assert!(matches!(a.modal, Some(Modal::Confirm { .. })));
        let fx = update(&mut a, ch('y'));
        assert!(matches!(fx.as_slice(), [Effect::WriteConfig(_)]), "{fx:?}");
    }

    #[test]
    fn restart_needed_is_judged_against_what_the_daemon_really_runs() {
        let mut a = app();
        assert!(
            !needs_restart(&a),
            "the file equals what the daemon reported"
        );
        select(&mut a, Row::HistoryItems);
        update(&mut a, key(KeyCode::Enter));
        for _ in 0..5 {
            update(&mut a, key(KeyCode::Backspace));
        }
        update(&mut a, ch('9'));
        update(&mut a, key(KeyCode::Enter));
        assert!(!needs_restart(&a), "unsaved changes are not on disk yet");
        update(&mut a, ch('s'));
        update(&mut a, Msg::Reply(Reply::ConfigSaved(Ok(()))));
        assert!(needs_restart(&a));

        // An old daemon says nothing: the file as first read is the baseline.
        let mut b = AppState::new((100, 30));
        b.conn = Conn::Up;
        b.hello = HelloState::Old;
        let mut cfg = DaemonConfig::default();
        update(&mut b, Msg::Reply(Reply::Config(Ok(cfg.clone()))));
        assert!(!needs_restart(&b));
        cfg.max_history_items = 7;
        update(&mut b, Msg::Reply(Reply::Config(Ok(cfg))));
        assert!(needs_restart(&b));
    }

    #[test]
    fn restart_asks_first_and_reports_the_result() {
        let mut a = app();
        select(&mut a, Row::Restart);
        update(&mut a, key(KeyCode::Enter));
        assert!(matches!(a.modal, Some(Modal::Confirm { .. })));
        let fx = update(&mut a, ch('y'));
        assert!(matches!(fx.as_slice(), [Effect::RestartDaemon]), "{fx:?}");
        assert!(a.settings.restarting);
        update(&mut a, Msg::Reply(Reply::DaemonRestarted(Ok(4242))));
        assert!(!a.settings.restarting);
        assert!(a.toasts.iter().any(|t| t.text.contains("4242")));
        update(
            &mut a,
            Msg::Reply(Reply::DaemonRestarted(Err(crate::api::ApiError::Refused(
                "no".into(),
            )))),
        );
        assert!(matches!(a.modal, Some(Modal::Notice { .. })));
    }

    #[test]
    fn the_service_row_installs_only_when_missing() {
        use crate::files::ServiceState;
        let mut a = app();
        select(&mut a, Row::Service);
        update(
            &mut a,
            Msg::Reply(Reply::Service(ServiceState::NotInstalled)),
        );
        let fx = update(&mut a, key(KeyCode::Enter));
        assert!(matches!(fx.as_slice(), [Effect::InstallService]), "{fx:?}");
        update(
            &mut a,
            Msg::Reply(Reply::ServiceInstalled(Ok(
                "Installed systemd unit: /x\nNow run: systemctl --user enable --now klip.service"
                    .into(),
            ))),
        );
        assert!(matches!(&a.modal, Some(Modal::Notice { body, .. }) if body.contains("systemctl")));
        update(
            &mut a,
            Msg::Reply(Reply::Service(ServiceState::Installed("/x".into()))),
        );
        a.modal = None;
        let fx = update(&mut a, key(KeyCode::Enter));
        assert!(fx.is_empty());
    }

    #[test]
    fn a_new_config_read_does_not_throw_away_unsaved_edits() {
        let mut a = app();
        select(&mut a, Row::LanDiscovery);
        update(&mut a, key(KeyCode::Enter));
        update(&mut a, Msg::Reply(Reply::Config(Ok(running_cfg()))));
        assert!(dirty(&a), "the draft survives a re-read");
    }
}
