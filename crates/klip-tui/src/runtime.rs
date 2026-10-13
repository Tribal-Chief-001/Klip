//! Executes effects against a [`DaemonApi`] and owns the terminal. Generic over the ratatui backend
//! so the tests run exactly this code on a `TestBackend`.
use crate::api::{ApiError, DaemonApi};
use crate::app::{boot, update, AppState, Effect, Msg, Reply};
use crate::theme::{Theme, TuiEnv};
use crate::view;
use ratatui::backend::Backend;
use ratatui::Terminal;
use std::fmt::Debug;
use std::io;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

fn io_err<E: Debug>(e: E) -> io::Error {
    io::Error::other(format!("{e:?}"))
}

pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub struct Runtime<B: Backend> {
    pub app: AppState,
    terminal: Terminal<B>,
    api: Arc<dyn DaemonApi>,
    theme: Theme,
    tx: UnboundedSender<Msg>,
}

impl<B: Backend> Runtime<B>
where
    B::Error: Debug,
{
    pub fn new(
        api: Arc<dyn DaemonApi>,
        backend: B,
        env: TuiEnv,
        size: (u16, u16),
    ) -> io::Result<(Self, UnboundedReceiver<Msg>)> {
        let terminal = Terminal::new(backend).map_err(io_err)?;
        let (tx, rx) = unbounded_channel();
        let mut app = AppState::new(size);
        app.mouse_on = env.mouse;
        app.invert_qr = !env.light;
        let first = boot(&mut app);
        let rt = Self {
            app,
            terminal,
            api,
            theme: Theme::from_env(&env),
            tx,
        };
        for fx in first {
            rt.execute(fx);
        }
        Ok((rt, rx))
    }

    /// Apply one event or reply and start the effects it asks for.
    pub fn dispatch(&mut self, msg: Msg) {
        let effects = update(&mut self.app, msg);
        for fx in effects {
            self.execute(fx);
        }
    }

    /// Draw if anything changed since the last draw (and clear first after Ctrl+L).
    pub fn draw(&mut self) -> io::Result<()> {
        if self.app.force_clear {
            // `Terminal::clear` asks the terminal where the cursor is, which a raw-mode loop that is
            // itself reading the keyboard cannot get answered (found with the pseudo-terminal test:
            // "cursor position could not be read"). `resize` clears and resets the buffers without asking.
            let size = self.terminal.size().map_err(io_err)?;
            self.terminal
                .resize(ratatui::layout::Rect::new(0, 0, size.width, size.height))
                .map_err(io_err)?;
            self.app.force_clear = false;
            self.app.dirty = true;
        }
        if !self.app.dirty {
            return Ok(());
        }
        self.app.dirty = false;
        let app = &self.app;
        let theme = &self.theme;
        self.terminal
            .draw(|f| view::view(app, theme, f))
            .map_err(io_err)?;
        Ok(())
    }

    pub fn backend(&self) -> &B {
        self.terminal.backend()
    }

    pub fn backend_mut(&mut self) -> &mut B {
        self.terminal.backend_mut()
    }

    fn execute(&self, fx: Effect) {
        let api = Arc::clone(&self.api);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            // Run in a child task: if the call panics, the poll must still get an answer, or its
            // "in flight" flag would never clear and polling would stop for good.
            let inner = tokio::spawn(run_effect(api, fx.clone()));
            let reply = match inner.await {
                Ok(r) => r,
                Err(e) => failed_reply(&fx, ApiError::Io(format!("internal error: {e}"))),
            };
            let _ = tx.send(Msg::Reply(reply));
        });
    }
}

async fn run_effect(api: Arc<dyn DaemonApi>, fx: Effect) -> Reply {
    match fx {
        Effect::Hello => Reply::Hello(api.hello().await),
        Effect::Status => Reply::Status(api.status().await),
        Effect::Pause(secs) => Reply::Paused(api.pause(secs).await),
        Effect::Resume => Reply::Resumed(api.resume().await),
        Effect::Unpair { selector } => Reply::Unpaired(api.unpair(selector).await),
        Effect::UnpairAll => Reply::UnpairedAll(api.unpair_all().await),
        Effect::StopDaemon => Reply::DaemonStopped(api.stop_daemon().await),
        Effect::StartDaemon => Reply::DaemonStarted(api.start_daemon().await),
        Effect::PairStart {
            ttl_secs,
            lan,
            session,
            restart,
        } => {
            if restart {
                let _ = api.pair_cancel().await; // one task, so the cancel always happens before the start
            }
            Reply::PairStarted {
                session,
                result: api.pair_start(ttl_secs, lan).await,
            }
        }
        Effect::PairPoll { session } => Reply::PairPolled {
            session,
            result: api.pair_poll().await,
        },
        Effect::PairDecide { accept, session } => Reply::PairDecided {
            session,
            result: api.pair_decide(accept).await,
        },
        Effect::PairCancel => Reply::PairCancelled(api.pair_cancel().await),
        Effect::PairJoin { ticket, session } => Reply::PairJoined {
            session,
            result: api.pair_join(ticket).await,
        },
    }
}

/// The reply to hand back when an effect's task died.
fn failed_reply(fx: &Effect, e: ApiError) -> Reply {
    match fx {
        Effect::Hello => Reply::Hello(Err(e)),
        Effect::Status => Reply::Status(Err(e)),
        Effect::Pause(_) => Reply::Paused(Err(e)),
        Effect::Resume => Reply::Resumed(Err(e)),
        Effect::Unpair { .. } => Reply::Unpaired(Err(e)),
        Effect::UnpairAll => Reply::UnpairedAll(Err(e)),
        Effect::StopDaemon => Reply::DaemonStopped(Err(e)),
        Effect::StartDaemon => Reply::DaemonStarted(Err(e)),
        Effect::PairStart { session, .. } => Reply::PairStarted {
            session: *session,
            result: Err(e),
        },
        Effect::PairPoll { session } => Reply::PairPolled {
            session: *session,
            result: Err(e),
        },
        Effect::PairDecide { session, .. } => Reply::PairDecided {
            session: *session,
            result: Err(e),
        },
        Effect::PairCancel => Reply::PairCancelled(Err(e)),
        Effect::PairJoin { session, .. } => Reply::PairJoined {
            session: *session,
            result: Err(e),
        },
    }
}

pub fn to_msg(e: crossterm::event::Event) -> Option<Msg> {
    use crossterm::event::Event;
    match e {
        Event::Key(k) => Some(Msg::Key(k)),
        Event::Mouse(m) => Some(Msg::Mouse(m)),
        Event::Paste(s) => Some(Msg::Paste(s)),
        Event::Resize(w, h) => Some(Msg::Resize(w, h)),
        _ => None,
    }
}

/// The real terminal loop: keys, mouse, paste, resize, a 250 ms tick, effect replies and signals.
pub async fn run(api: Arc<dyn DaemonApi>, env: TuiEnv) -> io::Result<()> {
    use crossterm::event::EventStream;
    use futures_util::StreamExt;
    use ratatui::backend::CrosstermBackend;
    use std::time::{Duration, Instant};
    use tokio::signal::unix::{signal, SignalKind};
    use tokio::time::MissedTickBehavior;

    let mut guard = crate::term::TerminalGuard::enter(env.mouse)?;
    #[cfg(debug_assertions)]
    if std::env::var_os("KLIP_TUI_TEST_PANIC").is_some() {
        panic!("injected panic for the terminal-restore test");
    }
    let size = crossterm::terminal::size().unwrap_or((80, 24));
    let (mut rt, mut rx) = Runtime::new(api, CrosstermBackend::new(io::stdout()), env, size)?;
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut on_term = signal(SignalKind::terminate())?;
    let mut on_hup = signal(SignalKind::hangup())?;
    let mut on_int = signal(SignalKind::interrupt())?;
    let start = Instant::now();
    rt.draw()?;
    loop {
        tokio::select! {
            ev = events.next() => match ev {
                Some(Ok(e)) => {
                    if let Some(m) = to_msg(e) {
                        rt.dispatch(m);
                    }
                }
                Some(Err(_)) | None => break,
            },
            _ = tick.tick() => rt.dispatch(Msg::Tick {
                mono_ms: start.elapsed().as_millis() as u64,
                wall_ms: unix_ms(),
            }),
            Some(m) = rx.recv() => rt.dispatch(m),
            _ = on_term.recv() => break,
            _ = on_hup.recv() => break,
            _ = on_int.recv() => break,
        }
        while let Ok(m) = rx.try_recv() {
            rt.dispatch(m);
        }
        guard.set_mouse(rt.app.mouse_on);
        rt.draw()?;
        if rt.app.quit {
            break;
        }
    }
    guard.restore();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn terminal_events_become_messages() {
        let k = Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert!(matches!(to_msg(k), Some(Msg::Key(_))));
        assert!(matches!(
            to_msg(Event::Resize(10, 5)),
            Some(Msg::Resize(10, 5))
        ));
        assert!(matches!(
            to_msg(Event::Paste("x".into())),
            Some(Msg::Paste(_))
        ));
        assert!(to_msg(Event::FocusGained).is_none());
    }
}
