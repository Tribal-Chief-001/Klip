//! One module per screen; each has its own state, key handling and drawing. This module routes to
//! them. Until a screen exists its hook does nothing.
use crate::app::{AppState, Effect, Screen};
use crossterm::event::KeyEvent;

pub fn key(_app: &mut AppState, _k: KeyEvent, _fx: &mut Vec<Effect>) {}
pub fn paste(_app: &mut AppState, _text: &str, _fx: &mut Vec<Effect>) {}
pub fn scroll(_app: &mut AppState, _delta: i32) {}
pub fn text_focus(_app: &AppState) -> bool {
    false
}
pub fn on_enter(_app: &mut AppState, _to: Screen, _fx: &mut Vec<Effect>) {}
pub fn on_leave(_app: &mut AppState, _from: Screen, _fx: &mut Vec<Effect>) {}
pub fn on_tick(_app: &mut AppState, _fx: &mut Vec<Effect>) {}
pub fn on_daemon_down(_app: &mut AppState) {}
