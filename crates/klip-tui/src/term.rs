//! Raw mode, alternate screen, bracketed paste and mouse capture — and, above all, giving the
//! terminal back on every exit path (normal quit, signal, panic).
use crossterm::cursor::Show;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use std::io::{self, Write};

pub struct TerminalGuard {
    active: bool,
    mouse: bool,
}

impl TerminalGuard {
    pub fn enter(mouse: bool) -> io::Result<Self> {
        enable_raw_mode()?;
        let mut out = io::stdout();
        if let Err(e) = execute!(out, EnterAlternateScreen, EnableBracketedPaste) {
            let _ = disable_raw_mode();
            return Err(e);
        }
        if mouse {
            let _ = execute!(out, EnableMouseCapture);
        }
        install_panic_hook();
        Ok(Self {
            active: true,
            mouse,
        })
    }

    pub fn mouse(&self) -> bool {
        self.mouse
    }

    /// Ask the terminal to change its size (xterm window operation `CSI 8 ; rows ; cols t`).
    /// Terminals that do not allow it ignore the request; the app then keeps working at the size
    /// it has.
    pub fn request_size(&self, cols: u16, rows: u16) {
        if !self.active {
            return;
        }
        use std::io::Write;
        let mut out = io::stdout();
        let _ = out.write_all(resize_sequence(cols, rows).as_bytes());
        let _ = out.flush();
    }

    pub fn set_mouse(&mut self, on: bool) {
        if on == self.mouse || !self.active {
            return;
        }
        let mut out = io::stdout();
        let _ = if on {
            execute!(out, EnableMouseCapture)
        } else {
            execute!(out, DisableMouseCapture)
        };
        self.mouse = on;
    }

    /// Give the terminal back. Safe to call more than once.
    pub fn restore(&mut self) {
        if self.active {
            restore_terminal_best_effort();
            self.active = false;
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Leave the alternate screen, show the cursor, stop mouse/paste modes and leave raw mode. Used by
/// the guard, by signal handling and by the panic hook; every step is best effort.
pub fn restore_terminal_best_effort() {
    let mut out = io::stdout();
    let _ = execute!(
        out,
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen,
        Show
    );
    let _ = disable_raw_mode();
    let _ = out.flush();
}

fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // Restore first, so the panic message lands on the normal screen and stays readable.
            restore_terminal_best_effort();
            previous(info);
        }));
    });
}

/// The escape sequence that asks a terminal for `cols` x `rows` characters.
pub fn resize_sequence(cols: u16, rows: u16) -> String {
    format!("\x1b[8;{rows};{cols}t")
}

#[cfg(test)]
mod size_tests {
    use super::*;

    #[test]
    fn the_resize_request_is_rows_then_columns() {
        assert_eq!(resize_sequence(110, 42), "\x1b[8;42;110t");
    }
}
