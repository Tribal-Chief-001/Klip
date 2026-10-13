//! One module per screen; each has its own state, key handling and drawing. This module routes.
pub mod devices;
pub mod history;
pub mod home;
pub mod log;
pub mod pair;
pub mod send;
pub mod settings;

use crate::app::{AppState, Effect, Screen};
use crate::theme::Theme;
use crossterm::event::KeyEvent;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// Which rows of a list of `len` items are shown in `height` rows, keeping `selected` in view.
pub fn window(selected: usize, len: usize, height: usize) -> (usize, usize) {
    if height == 0 || len == 0 {
        return (0, 0);
    }
    let start = selected
        .saturating_sub(height / 2)
        .min(len.saturating_sub(height));
    (start, (start + height).min(len))
}

/// One centred line of text in the middle of `area`.
pub fn centered_text(f: &mut Frame, area: Rect, style: Style, text: &str) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let row = Rect {
        x: area.x,
        y: area.y + area.height / 2,
        width: area.width,
        height: 1,
    };
    f.render_widget(
        Paragraph::new(text.to_string())
            .style(style)
            .alignment(Alignment::Center),
        row,
    );
}

pub fn key(app: &mut AppState, k: KeyEvent, fx: &mut Vec<Effect>) {
    match app.screen {
        Screen::Home => home::key(app, k, fx),
        Screen::Devices => devices::key(app, k, fx),
        Screen::Pair => pair::key(app, k, fx),
        Screen::History => history::key(app, k, fx),
        Screen::Send => send::key(app, k, fx),
        Screen::Log => log::key(app, k),
        Screen::Settings => settings::key(app, k, fx),
    }
}

pub fn paste(app: &mut AppState, text: &str, _fx: &mut Vec<Effect>) {
    match app.screen {
        Screen::Pair => pair::paste(app, text),
        Screen::Send => send::paste(app, text),
        Screen::Settings => settings::paste(app, text),
        _ => {}
    }
}

pub fn scroll(app: &mut AppState, delta: i32) {
    match app.screen {
        Screen::Devices => devices::scroll(app, delta),
        Screen::History => history::scroll(app, delta),
        Screen::Log => log::scroll(app, delta),
        Screen::Settings => settings::scroll(app, delta),
        _ => {}
    }
}

pub fn text_focus(app: &AppState) -> bool {
    match app.screen {
        Screen::Pair => pair::text_focus(app),
        Screen::History => history::text_focus(app),
        Screen::Send => send::text_focus(app),
        Screen::Log => log::text_focus(app),
        Screen::Settings => settings::text_focus(app),
        _ => false,
    }
}

pub fn on_enter(app: &mut AppState, to: Screen, fx: &mut Vec<Effect>) {
    match to {
        Screen::Pair => pair::on_enter(app, fx),
        Screen::History => history::on_enter(app, fx),
        Screen::Send => send::on_enter(app),
        Screen::Log => log::on_enter(app, fx),
        Screen::Settings => settings::on_enter(fx),
        _ => {}
    }
}

pub fn on_leave(app: &mut AppState, from: Screen, fx: &mut Vec<Effect>) {
    match from {
        Screen::Pair => pair::on_leave(app, fx),
        Screen::History => history::on_leave(app),
        Screen::Settings => settings::on_leave(app),
        Screen::Send => send::on_leave(app),
        _ => {}
    }
}

pub fn on_tick(app: &mut AppState, fx: &mut Vec<Effect>) {
    pair::on_tick(app, fx);
    history::on_tick(app, fx);
    log::on_tick(app, fx);
}

pub fn on_daemon_down(app: &mut AppState) {
    pair::on_daemon_down(app);
    send::on_daemon_down(app);
}

pub fn view(app: &AppState, theme: &Theme, f: &mut Frame, area: Rect) {
    match app.screen {
        Screen::Home => home::view(app, theme, f, area),
        Screen::Devices => devices::view(app, theme, f, area),
        Screen::Pair => pair::view(app, theme, f, area),
        Screen::History => history::view(app, theme, f, area),
        Screen::Send => send::view(app, theme, f, area),
        Screen::Log => log::view(app, theme, f, area),
        Screen::Settings => settings::view(app, theme, f, area),
    }
}

/// Screen-specific key hints, each followed by " · ", shown in front of the global ones.
pub fn hints(app: &AppState) -> &'static str {
    match app.screen {
        Screen::Home => "s start · x stop · ",
        Screen::Devices => "↑↓ select · u unpair · a unpair all · ",
        Screen::Pair => {
            if app.pair.mode == pair::PairMode::Show {
                "n new code · l LAN · t validity · j join · "
            } else {
                "Enter link · Esc leave field · c show code · "
            }
        }
        Screen::History => {
            "↑↓ select · / search · Enter copy back · s send again · v reveal · c clear · "
        }
        Screen::Send => "Enter type · Ctrl+S send · Ctrl+O file · Ctrl+U clear · ",
        Screen::Log => "f follow · / search · PgUp PgDn scroll · End latest · ",
        Screen::Settings => "↑↓ select · Enter edit/toggle · s save · u discard · r restart · ",
    }
}

/// Key list for the help overlay: global keys first, then the screen's own.
pub fn help_lines(app: &AppState) -> Vec<(&'static str, &'static str)> {
    let mut v = vec![
        ("1-7 / Tab", "switch screen"),
        ("p", "pause or resume syncing"),
        ("h", "this help"),
        ("m", "mouse on/off (off lets you select text)"),
        ("Alt+1-7", "switch screen, also inside a field"),
        ("Ctrl+L", "redraw"),
        ("q / Ctrl+C", "quit (Klip keeps running)"),
    ];
    match app.screen {
        Screen::Home => {
            v.push(("s", "start Klip when it is not running"));
            v.push(("x", "stop Klip"));
        }
        Screen::Devices => {
            v.push(("↑↓ / j k", "select a device"));
            v.push(("u", "unpair the selected device"));
            v.push(("a", "unpair all devices"));
        }
        Screen::Pair => {
            v.push((
                "n / l / t",
                "new code / toggle LAN addresses / cycle validity",
            ));
            v.push(("c / j", "show a code / join with a ticket"));
            v.push(("Enter", "link with the pasted ticket"));
        }
        Screen::History => {
            v.push(("↑↓ / j k", "select a clip"));
            v.push(("PgUp PgDn", "scroll the clip"));
            v.push(("/", "search (Esc clears)"));
            v.push(("Enter", "copy the clip back to this computer"));
            v.push(("s", "send it to your devices again"));
            v.push(("v", "show or hide a secret"));
            v.push(("c", "clear the history"));
            v.push(("r", "reload"));
        }
        Screen::Settings => {
            v.push(("↑↓ / j k", "select a setting"));
            v.push(("Enter / Space", "edit or toggle the selected setting"));
            v.push(("s", "save to config.json"));
            v.push(("u", "discard unsaved changes"));
            v.push(("r", "restart Klip to apply the saved settings"));
        }
        Screen::Log => {
            v.push(("f", "follow the end of the log, or stop following"));
            v.push(("/", "search (Esc clears)"));
            v.push(("↑↓ PgUp PgDn", "scroll"));
            v.push(("End / Home", "jump to the newest / the oldest line"));
        }
        Screen::Send => {
            v.push(("Ctrl+S / F5", "send the text to all devices"));
            v.push(("Ctrl+O", "load a text file"));
            v.push(("Ctrl+U", "clear"));
            v.push(("Ctrl+W", "delete a word"));
            v.push(("Esc", "leave the editor (then 1-7 switch screens)"));
            v.push(("Enter", "start typing"));
        }
    }
    v
}
