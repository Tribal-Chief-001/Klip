#![cfg(unix)]
//! Full-screen terminal interface for Klip.
//!
//! Layering: `app` is a pure core (state, events, effects, `update`), `view` only draws, `runtime`
//! executes effects against a [`api::DaemonApi`] and owns the terminal. See
//! `docs/superpowers/specs/2026-10-13-klip-tui-design.md`.

pub mod fmt;
pub mod sanitize;
pub mod textbuf;
