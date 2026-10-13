//! Test support for this crate and for klip-cli's TUI tests (feature `testkit`).
use crate::api::*;
use async_trait::async_trait;
use klip_proto::hlc::NodeId;
use klip_proto::ipc::{
    max_request_bytes, CAP_CLEAR_HISTORY, CAP_COPY_TO_CLIPBOARD, CAP_UNPAIR_ALL,
    IPC_PROTOCOL_VERSION,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub fn sample_peer(name: &str, online: bool, seed: u8) -> PeerStatusInfo {
    PeerStatusInfo {
        node_id: NodeId::new([seed; 32]),
        name: name.to_string(),
        is_online: online,
        last_error: None,
    }
}

pub fn sample_status(peers: Vec<PeerStatusInfo>) -> DaemonStatusInfo {
    DaemonStatusInfo {
        is_active: true,
        is_paused: false,
        paused_remaining_secs: 0,
        platform: "Linux (X11)".to_string(),
        memory_rss_bytes: 16_600_000,
        node_id: NodeId::new([9; 32]),
        relay_mode: "n0 public relays + discovery".to_string(),
        relay_state: "relay connected (https://aps1-1.relay.n0.iroh.link./)".to_string(),
        pairing_open: false,
        peers,
        total_synced_clips: 128,
        last_sync_time_secs_ago: Some(4),
        last_clip_preview: Some("https://github.com/…".to_string()),
        last_clip_size_bytes: 38,
    }
}

pub fn sample_hello() -> HelloInfo {
    let max_clip = 32 * 1024 * 1024;
    HelloInfo {
        protocol: IPC_PROTOCOL_VERSION,
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities: vec![
            CAP_UNPAIR_ALL.to_string(),
            CAP_COPY_TO_CLIPBOARD.to_string(),
            CAP_CLEAR_HISTORY.to_string(),
        ],
        config: ConfigInfo {
            device_name: "Laptop".to_string(),
            relay: "n0".to_string(),
            max_history_items: 50,
            max_clip_bytes: max_clip,
            lan_discovery: true,
        },
        max_request_bytes: max_request_bytes(max_clip),
    }
}

pub fn sample_item(n: u8, text: &str, sensitive: bool) -> HistoryItem {
    HistoryItem {
        id: [n; 16],
        origin_node: NodeId::new([1; 32]),
        origin_name: "Vivo V2253".to_string(),
        timestamp_ms: 1_700_000_000_000 + u64::from(n) * 1000,
        content_preview: text.chars().take(80).collect(),
        full_content: text.as_bytes().to_vec(),
        size_bytes: text.len(),
        is_sensitive: sensitive,
    }
}

use crate::app::AppState;
use crate::theme::{Theme, TuiEnv};
use klip_core::KlipPaths;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;
use std::path::PathBuf;

/// An environment without colours (so snapshots contain text only) and a private temp directory.
pub fn test_env(tag: &str) -> TuiEnv {
    let dir = std::env::temp_dir().join(format!("klip-tui-test-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    TuiEnv::from_vars(KlipPaths::from_dir(dir), |k| match k {
        "NO_COLOR" => Some("1".to_string()),
        _ => None,
    })
}

pub fn plain_theme() -> Theme {
    Theme::from_env(&test_env("theme"))
}

/// The text of a buffer, one line per row, trailing spaces removed.
pub fn dump(buf: &Buffer) -> String {
    let mut out = String::new();
    for y in 0..buf.area.height {
        let mut row = String::new();
        let mut x = 0;
        while x < buf.area.width {
            let sym = buf[(x, y)].symbol();
            row.push_str(sym);
            // A wide character covers two cells; the second one holds a blank, not a character.
            x += crate::sanitize::width(sym).max(1) as u16;
        }
        out.push_str(row.trim_end());
        out.push('\n');
    }
    out
}

/// Draw `app` on a `cols × rows` test terminal and return the screen as text.
pub fn render(app: &AppState, theme: &Theme, cols: u16, rows: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(cols, rows)).expect("test terminal");
    terminal
        .draw(|f| crate::view::view(app, theme, f))
        .expect("draw");
    dump(terminal.backend().buffer())
}

/// Compare with `tests/snapshots/<name>.txt`; `UPDATE_SNAPSHOTS=1` rewrites the file.
pub fn assert_snapshot(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing snapshot {}; run with UPDATE_SNAPSHOTS=1 and review it",
            path.display()
        )
    });
    assert_eq!(
        actual, want,
        "snapshot {name} changed; if intended, regenerate and review the diff"
    );
}

pub struct FakeState {
    pub up: bool,
    pub status: DaemonStatusInfo,
    /// `None` behaves like a v0.3.6 daemon: no Hello, no copy-back, no clear.
    pub hello: Option<HelloInfo>,
    pub history: Vec<HistoryItem>,
    pub calls: Vec<String>,
    /// Returned (once) by the next call instead of its normal result.
    pub fail_next: Option<ApiError>,
    /// The next call panics (to prove polling survives a dying task).
    pub panic_next: bool,
    pub send_reply: Result<SendReport, ApiError>,
    pub ticket: PairTicket,
    pub poll: PairPollInfo,
    pub join_reply: Result<Paired, ApiError>,
    pub delay_ms: u64,
}

impl FakeState {
    fn new() -> Self {
        Self {
            up: true,
            status: sample_status(vec![
                sample_peer("Vivo V2253", true, 1),
                sample_peer("Pixel 8", false, 2),
            ]),
            hello: Some(sample_hello()),
            history: vec![],
            calls: vec![],
            fail_next: None,
            panic_next: false,
            send_reply: Ok(SendReport {
                bytes: 0,
                delivered: vec!["Vivo V2253".into()],
                failed: vec![],
            }),
            ticket: PairTicket {
                ticket: "klip1TESTTICKETTESTTICKETTESTTICKET".to_string(),
                expires_in_secs: 120,
                host_fingerprint: "AAAA-BBBB-CCCC".to_string(),
            },
            poll: PairPollInfo {
                window_open: true,
                seconds_left: 100,
                pending: None,
                paired: None,
            },
            join_reply: Ok(Paired {
                name: "Pixel 8".into(),
                fingerprint: "DDDD-EEEE-FFFF".into(),
            }),
            delay_ms: 0,
        }
    }
}

pub struct FakeDaemon {
    state: Mutex<FakeState>,
}

impl FakeDaemon {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(FakeState::new()),
        })
    }

    pub fn old_daemon() -> Arc<Self> {
        let d = Self::new();
        d.with(|s| s.hello = None);
        d
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut FakeState) -> R) -> R {
        f(&mut self.state.lock().unwrap())
    }

    pub fn calls(&self) -> Vec<String> {
        self.state.lock().unwrap().calls.clone()
    }

    async fn enter(&self, call: &str) -> Result<(), ApiError> {
        let (delay, panic_now) = {
            let mut s = self.state.lock().unwrap();
            s.calls.push(call.to_string());
            if let Some(e) = s.fail_next.take() {
                return Err(e);
            }
            let panic_now = std::mem::take(&mut s.panic_next);
            if !panic_now && !s.up {
                return Err(ApiError::DaemonDown);
            }
            (s.delay_ms, panic_now)
        };
        // Panic only after the lock is released, so the mutex is not poisoned for later calls.
        if panic_now {
            panic!("fake daemon panic requested by the test");
        }
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
        Ok(())
    }

    fn need_protocol_2(&self) -> Result<(), ApiError> {
        if self.state.lock().unwrap().hello.is_none() {
            Err(ApiError::Unsupported)
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl DaemonApi for FakeDaemon {
    async fn hello(&self) -> Result<HelloInfo, ApiError> {
        self.enter("hello").await?;
        self.state
            .lock()
            .unwrap()
            .hello
            .clone()
            .ok_or(ApiError::Unsupported)
    }

    async fn status(&self) -> Result<DaemonStatusInfo, ApiError> {
        self.enter("status").await?;
        Ok(self.state.lock().unwrap().status.clone())
    }

    async fn history(&self, limit: usize) -> Result<Vec<HistoryItem>, ApiError> {
        self.enter("history").await?;
        Ok(self
            .state
            .lock()
            .unwrap()
            .history
            .iter()
            .take(limit)
            .cloned()
            .collect())
    }

    async fn send(&self, text: String) -> Result<SendReport, ApiError> {
        self.enter(&format!("send {}", text.len())).await?;
        let s = self.state.lock().unwrap();
        let mut reply = s.send_reply.clone();
        if let Ok(r) = &mut reply {
            r.bytes = text.len();
        }
        reply
    }

    async fn pause(&self, secs: u64) -> Result<u64, ApiError> {
        self.enter(&format!("pause {secs}")).await?;
        let mut s = self.state.lock().unwrap();
        s.status.is_paused = true;
        s.status.paused_remaining_secs = secs;
        Ok(secs)
    }

    async fn resume(&self) -> Result<(), ApiError> {
        self.enter("resume").await?;
        let mut s = self.state.lock().unwrap();
        s.status.is_paused = false;
        s.status.paused_remaining_secs = 0;
        Ok(())
    }

    async fn unpair(&self, selector: String) -> Result<String, ApiError> {
        self.enter(&format!("unpair {selector}")).await?;
        let mut s = self.state.lock().unwrap();
        let wanted = selector.replace('-', "").to_uppercase();
        let hit = s.status.peers.iter().position(|p| {
            p.name == selector
                || p.node_id
                    .fingerprint()
                    .replace('-', "")
                    .starts_with(&wanted)
        });
        match hit {
            Some(i) => Ok(s.status.peers.remove(i).name),
            None => Err(ApiError::Refused(format!(
                "no paired device matches '{selector}'"
            ))),
        }
    }

    async fn unpair_all(&self) -> Result<UnpairReport, ApiError> {
        self.enter("unpair_all").await?;
        let mut s = self.state.lock().unwrap();
        let removed = s.status.peers.drain(..).map(|p| p.name).collect();
        Ok((removed, vec![]))
    }

    async fn copy_to_clipboard(&self, id: [u8; 16]) -> Result<(), ApiError> {
        self.enter("copy").await?;
        self.need_protocol_2()?;
        let s = self.state.lock().unwrap();
        if s.history.iter().any(|i| i.id == id) {
            Ok(())
        } else {
            Err(ApiError::Refused(
                "that clip is no longer in the history".to_string(),
            ))
        }
    }

    async fn clear_history(&self) -> Result<(), ApiError> {
        self.enter("clear_history").await?;
        self.need_protocol_2()?;
        self.state.lock().unwrap().history.clear();
        Ok(())
    }

    async fn pair_start(&self, ttl_secs: u64, lan: bool) -> Result<PairTicket, ApiError> {
        self.enter(&format!("pair_start {ttl_secs} {lan}")).await?;
        let mut s = self.state.lock().unwrap();
        s.status.pairing_open = true;
        Ok(s.ticket.clone())
    }

    async fn pair_poll(&self) -> Result<PairPollInfo, ApiError> {
        self.enter("pair_poll").await?;
        Ok(self.state.lock().unwrap().poll.clone())
    }

    async fn pair_decide(&self, accept: bool) -> Result<PairPollInfo, ApiError> {
        self.enter(&format!("pair_decide {accept}")).await?;
        let mut s = self.state.lock().unwrap();
        s.poll.pending = None;
        Ok(s.poll.clone())
    }

    async fn pair_cancel(&self) -> Result<(), ApiError> {
        self.enter("pair_cancel").await?;
        let mut s = self.state.lock().unwrap();
        s.status.pairing_open = false;
        s.poll.window_open = false;
        Ok(())
    }

    async fn pair_join(&self, ticket: String) -> Result<Paired, ApiError> {
        self.enter(&format!("pair_join {ticket}")).await?;
        self.state.lock().unwrap().join_reply.clone()
    }

    async fn stop_daemon(&self) -> Result<(), ApiError> {
        self.enter("stop_daemon").await?;
        self.state.lock().unwrap().up = false;
        Ok(())
    }

    async fn start_daemon(&self) -> Result<u32, ApiError> {
        let mut s = self.state.lock().unwrap();
        s.calls.push("start_daemon".to_string());
        s.up = true;
        Ok(4242)
    }

    async fn install_service(&self) -> Result<String, ApiError> {
        self.enter("install_service").await?;
        Ok("Installed systemd unit: /home/test/.config/systemd/user/klip.service".to_string())
    }
}

use crate::app::Msg;
use crate::runtime::{unix_ms, Runtime};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Instant;
use tokio::sync::mpsc::UnboundedReceiver;

/// The real runtime on a `TestBackend`, with the clock driven by `pump`.
pub struct Harness {
    pub rt: Runtime<TestBackend>,
    rx: UnboundedReceiver<Msg>,
    start: Instant,
}

impl Harness {
    pub fn new(api: Arc<dyn crate::api::DaemonApi>, env: TuiEnv, cols: u16, rows: u16) -> Self {
        let (rt, rx) =
            Runtime::new(api, TestBackend::new(cols, rows), env, (cols, rows)).expect("runtime");
        Self {
            rt,
            rx,
            start: Instant::now(),
        }
    }

    fn now(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    pub fn key(&mut self, code: KeyCode) {
        self.key_mod(code, KeyModifiers::NONE);
    }

    pub fn key_mod(&mut self, code: KeyCode, mods: KeyModifiers) {
        self.rt.dispatch(Msg::Key(KeyEvent::new(code, mods)));
    }

    pub fn text(&mut self, s: &str) {
        for c in s.chars() {
            self.key(KeyCode::Char(c));
        }
    }

    pub fn paste(&mut self, s: &str) {
        self.rt.dispatch(Msg::Paste(s.to_string()));
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.rt.backend_mut().resize(cols, rows);
        self.rt.dispatch(Msg::Resize(cols, rows));
    }

    async fn step(&mut self) {
        let now = self.now();
        self.rt.dispatch(Msg::Tick {
            mono_ms: now,
            wall_ms: unix_ms(),
        });
        while let Ok(m) = self.rx.try_recv() {
            self.rt.dispatch(m);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    /// Let `ms` real milliseconds pass: ticks fire and replies are delivered.
    pub async fn pump(&mut self, ms: u64) {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            self.step().await;
        }
    }

    /// Pump until the screen contains `needle` or `timeout_ms` passed.
    pub async fn wait_for(&mut self, needle: &str, timeout_ms: u64) -> bool {
        let end = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            if self.screen().contains(needle) {
                return true;
            }
            if Instant::now() >= end {
                return false;
            }
            self.step().await;
        }
    }

    /// The current screen as text (draws first if something changed).
    pub fn screen(&mut self) -> String {
        self.rt.draw().expect("draw");
        dump(self.rt.backend().buffer())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_fake_records_calls_and_behaves_like_a_daemon() {
        let d = FakeDaemon::new();
        let st = d.status().await.unwrap();
        assert_eq!(st.peers.len(), 2);
        assert_eq!(d.pause(900).await.unwrap(), 900);
        let removed = d.unpair_all().await.unwrap();
        assert_eq!(removed.0.len(), 2);
        assert!(d.status().await.unwrap().peers.is_empty());
        assert_eq!(d.calls(), ["status", "pause 900", "unpair_all", "status"]);
    }

    #[tokio::test]
    async fn a_stopped_fake_answers_daemon_down_and_fail_next_wins() {
        let d = FakeDaemon::new();
        d.with(|s| s.up = false);
        assert_eq!(d.status().await.unwrap_err(), ApiError::DaemonDown);
        d.with(|s| {
            s.up = true;
            s.fail_next = Some(ApiError::Timeout(15));
        });
        assert_eq!(d.status().await.unwrap_err(), ApiError::Timeout(15));
        assert!(d.status().await.is_ok());
    }

    #[tokio::test]
    async fn an_old_daemon_has_no_hello_copy_or_clear() {
        let d = FakeDaemon::old_daemon();
        assert_eq!(d.hello().await.unwrap_err(), ApiError::Unsupported);
        assert_eq!(
            d.copy_to_clipboard([1; 16]).await.unwrap_err(),
            ApiError::Unsupported
        );
        assert_eq!(d.clear_history().await.unwrap_err(), ApiError::Unsupported);
        assert!(d.status().await.is_ok());
    }

    #[tokio::test]
    async fn history_copy_and_clear_work_on_a_new_daemon() {
        let d = FakeDaemon::new();
        d.with(|s| s.history = vec![sample_item(1, "hello", false)]);
        assert_eq!(d.history(10).await.unwrap().len(), 1);
        d.copy_to_clipboard([1; 16]).await.unwrap();
        assert!(matches!(
            d.copy_to_clipboard([9; 16]).await,
            Err(ApiError::Refused(_))
        ));
        d.clear_history().await.unwrap();
        assert!(d.history(10).await.unwrap().is_empty());
    }

    #[test]
    fn plain_messages_are_human() {
        assert_eq!(ApiError::DaemonDown.plain(), "Klip is not running");
        assert!(ApiError::TooLarge {
            size: 9_000_000,
            max: 8_388_608
        }
        .plain()
        .contains("MiB"));
        assert!(ApiError::Unsupported.plain().contains("older"));
    }
}
