//! One module per screen; each has its own state, key handling and drawing. This module routes.
pub mod devices;
pub mod home;
pub mod pair;

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
        _ => {}
    }
}

pub fn paste(app: &mut AppState, text: &str, _fx: &mut Vec<Effect>) {
    if app.screen == Screen::Pair {
        pair::paste(app, text);
    }
}

pub fn scroll(app: &mut AppState, delta: i32) {
    if app.screen == Screen::Devices {
        devices::scroll(app, delta);
    }
}

pub fn text_focus(app: &AppState) -> bool {
    match app.screen {
        Screen::Pair => pair::text_focus(app),
        _ => false,
    }
}

pub fn on_enter(app: &mut AppState, to: Screen, fx: &mut Vec<Effect>) {
    if to == Screen::Pair {
        pair::on_enter(app, fx);
    }
}

pub fn on_leave(app: &mut AppState, from: Screen, fx: &mut Vec<Effect>) {
    if from == Screen::Pair {
        pair::on_leave(app, fx);
    }
}

pub fn on_tick(app: &mut AppState, fx: &mut Vec<Effect>) {
    pair::on_tick(app, fx);
}

pub fn on_daemon_down(app: &mut AppState) {
    pair::on_daemon_down(app);
}

pub fn view(app: &AppState, theme: &Theme, f: &mut Frame, area: Rect) {
    match app.screen {
        Screen::Home => home::view(app, theme, f, area),
        Screen::Devices => devices::view(app, theme, f, area),
        Screen::Pair => pair::view(app, theme, f, area),
        other => centered_text(
            f,
            area,
            theme.st_dim(),
            &format!(
                "{}: this screen is built in a later task of the plan",
                other.title()
            ),
        ),
    }
}

/// Screen-specific key hints, each followed by " · ", shown in front of the global ones.
pub fn hints(app: &AppState) -> &'static str {
    match app.screen {
        Screen::Home => "s start · x stop · ",
        Screen::Devices => "↑↓ select · u unpair · U unpair all · ",
        Screen::Pair => {
            if app.pair.mode == pair::PairMode::Show {
                "n new code · l LAN · t validity · j join · "
            } else {
                "Enter link · Esc leave field · c show code · "
            }
        }
        _ => "",
    }
}

/// Key list for the help overlay: global keys first, then the screen's own.
pub fn help_lines(app: &AppState) -> Vec<(&'static str, &'static str)> {
    let mut v = vec![
        ("1-7 / Tab", "switch screen"),
        ("p", "pause or resume syncing"),
        ("?", "this help"),
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
            v.push(("U", "unpair all devices"));
        }
        Screen::Pair => {
            v.push((
                "n / l / t",
                "new code / toggle LAN addresses / cycle validity",
            ));
            v.push(("c / j", "show a code / join with a ticket"));
            v.push(("Enter", "link with the pasted ticket"));
        }
        _ => {}
    }
    v
}
