# Klip TUI — design

Status: **draft for owner review** (2026-10-13). Nothing in this document is implemented yet.
Scope chosen by the owner: "B" = a full TUI **plus** small, additive changes to the daemon's control protocol.

## 1. Goal

Today every Klip action is a separate shell command (`klip status`, `klip pair`, `klip unpair X`, …).
The goal is one full-screen terminal app, started with plain `klip`, that does everything the commands do
(and a few things they cannot), with live updates, so nobody has to run commands one by one.

Success means all of the following are demonstrated with command output, not claimed:

1. Every existing `klip <subcommand>` behaves exactly as before (characterization tests recorded *before* any change).
2. Every capability in section 5 works against a **real daemon** (in-process in tests, and the installed one by hand).
3. The terminal is always restored: normal quit, Ctrl+C, SIGTERM, SIGHUP, panic, daemon dying mid-session.
4. A new TUI works against the **currently released daemon (v0.3.6)**; features that need the new protocol are
   shown as unavailable with a clear reason instead of failing.
5. Nothing a clip contains can inject terminal escape sequences or spoof text direction on screen.

"Zero regression, zero errors" is the aim, but the document only promises what the tests in section 10 can show.
Anything they cannot show (other terminal emulators, macOS/Windows runtime) is written down as unverified.

## 2. Non-goals

- Windows. The CLI talks to the daemon over a Unix socket and cannot do so on Windows today; the TUI is compiled
  only on `cfg(unix)`. macOS is compile-checked in CI only, like the rest of the desktop code.
- Renaming a linked device (the protocol has no such call), per-device sync switches, images/HTML in the TUI,
  reading the system clipboard from the TUI (the daemon owns the clipboard), changing the Android app.
- Any change to the network protocol, pairing cryptography, or the Android/UniFFI surface.

## 3. Decisions already made

| Decision | Source |
|---|---|
| Build the TUI; scope B (TUI + additive IPC changes) | owner, 2026-10-13 |
| `klip` without arguments opens the TUI; every subcommand stays | design approved by owner |
| New crate `klip-tui` used by `klip-cli`; one binary | design approved by owner |
| Libraries: `ratatui` + `crossterm` | design approved by owner |
| Honest verification: unverified things are labelled in the README | standing owner rule |

## 4. Architecture

### 4.1 Crates and dependency direction

```
klip-proto  <-  klip-core  <-  klip-daemon
     ^              ^
     |              |
  klip-tui  --------+          (depends on proto + core only)
     ^
     |
  klip-cli  (depends on tui + daemon; supplies the real DaemonApi)
```

`klip-tui` must not depend on `klip-cli` or `klip-daemon` (the latter only as a *dev*-dependency for tests).
The TUI talks to the daemon only through a trait that `klip-cli` implements, which also lets tests use an
in-memory fake.

```rust
#[async_trait::async_trait]            // the `async-trait` crate: needed so the trait is usable as `Arc<dyn DaemonApi>`
pub trait DaemonApi: Send + Sync {
    async fn hello(&self) -> Result<HelloInfo, ApiError>;          // new protocol; Err(Unsupported) on old daemons
    async fn status(&self) -> Result<DaemonStatusInfo, ApiError>;
    async fn history(&self, limit: usize) -> Result<Vec<HistoryItem>, ApiError>;
    async fn send(&self, text: String) -> Result<SendReport, ApiError>;
    async fn pause(&self, secs: u64) -> Result<u64, ApiError>;
    async fn resume(&self) -> Result<(), ApiError>;
    async fn unpair(&self, selector: String) -> Result<String, ApiError>;
    async fn unpair_all(&self) -> Result<Vec<String>, ApiError>;    // new
    async fn copy_to_clipboard(&self, id: [u8; 16]) -> Result<(), ApiError>; // new
    async fn clear_history(&self) -> Result<(), ApiError>;          // new
    async fn pair_start(&self, ttl_secs: u64, lan: bool) -> Result<PairTicket, ApiError>;
    async fn pair_poll(&self) -> Result<PairPollInfo, ApiError>;
    async fn pair_decide(&self, accept: bool) -> Result<PairPollInfo, ApiError>;
    async fn pair_cancel(&self) -> Result<(), ApiError>;
    async fn pair_join(&self, ticket: String) -> Result<(String, String), ApiError>;
    async fn stop_daemon(&self) -> Result<(), ApiError>;
    async fn start_daemon(&self) -> Result<u32, ApiError>;           // spawns `klip start --foreground` detached, as `klip start` does
    async fn install_service(&self) -> Result<String, ApiError>;    // exactly what `klip install-service` does
}
```

`ApiError` distinguishes: `DaemonDown`, `Timeout`, `Unsupported` (old daemon), `Refused(String)` (the daemon answered
`IpcResponse::Error`), `Io(String)`. The TUI never shows a raw Rust error string without context.

The daemon's own files (config, log) are read by the TUI directly through `KlipPaths` (same user, mode 0600/0700),
so the **Log** and **Settings** screens work even when the daemon is not running.

### 4.2 Elm-style core (`klip-tui`)

```
crates/klip-tui/
  src/lib.rs          pub fn run(api: Arc<dyn DaemonApi>, env: TuiEnv) -> Result<(), TuiError>
  src/api.rs          DaemonApi, ApiError, SendReport, HelloInfo
  src/app.rs          AppState, Msg, Effect, update(&mut AppState, Msg) -> Vec<Effect>   (pure, no I/O)
  src/screens/        home.rs devices.rs pair.rs history.rs send.rs log.rs settings.rs   (state + update + view each)
  src/widgets/        tabs.rs statusbar.rs modal.rs toast.rs table.rs qr.rs textbuf.rs spinner.rs
  src/term.rs         TerminalGuard: raw mode, alternate screen, bracketed paste, mouse; restore on Drop/panic/signal
  src/theme.rs        palette, 24-bit / 256 / 16-colour fallback, NO_COLOR, KLIP_THEME
  src/sanitize.rs     display-safe text (control chars, escapes, bidi), width-aware truncation
  src/runtime.rs      event loop, effect executor, poll schedule, redraw throttle
  src/testkit.rs      FakeDaemon (in-memory DaemonApi), screen-to-text helper (feature "testkit")
  tests/              snapshots.rs  keyfuzz.rs  daemon_e2e.rs  pty.rs
```

- `update` is a pure function: given the state and an event it returns the new state and a list of **effects**
  (daemon calls, file reads/writes). It never blocks and never touches the terminal. This is what makes the
  logic testable without a terminal.
- `view(&AppState, &mut Frame)` only draws. It must not panic for any state at any terminal size.
- The runtime owns one `tokio::select!` loop over: crossterm `EventStream` (keys, mouse, paste, resize), a 250 ms
  tick (toasts, spinners, countdowns), and the effect-result channel. Each effect runs in its own task; its reply
  carries a request id and a per-screen generation number, and replies for an older generation are dropped
  (so a slow answer can never overwrite newer data).
- Redraw only when the state changed (dirty flag), at most ~30 times a second. Idle cost is the 250 ms tick only.
- The TUI process is separate from the daemon. Quitting the TUI never stops the daemon (the footer says so).

### 4.3 Terminal safety (`term.rs`)

- Enter: raw mode, alternate screen, bracketed paste, optional mouse capture (`m` toggles it, so native text
  selection can be used to copy a pairing ticket).
- Leave on every path: `Drop`, a panic hook (restores first, then prints the panic message normally),
  SIGTERM/SIGHUP/SIGINT handlers, and the `q`/Ctrl+C keys. A second restore is harmless.
- Not a TTY (stdin or stdout redirected): `klip` prints help and exits with code 2, exactly as today.
- Terminal smaller than 60 × 16: show a centred "terminal too small (need 60×16)" screen; resize recovers.

### 4.4 Data safety (`sanitize.rs`)

Clip text is attacker-influenced (anything copied on any linked device). Before it reaches the screen:

- C0 controls except `\n` and `\t`, DEL and all C1 controls (incl. `ESC`, `CSI`, `OSC`) are replaced by visible
  symbols (`␛`, `␇`, …) so no escape sequence ever reaches the terminal.
- Bidirectional overrides and isolates (U+202A–U+202E, U+2066–U+2069) and zero-width characters are shown as
  `⟦U+202E⟧`-style markers, so text cannot be visually reordered or hidden.
- Tabs expand to 4 columns; `\r` is shown as `␍`; width is computed with `unicode-width`; truncation never
  splits a grapheme or a wide character.
- Items flagged sensitive by the daemon show `•••• secret (N B)` until the person presses `v` on that item;
  the revealed state is per item and cleared when leaving the History screen.
- The TUI never writes clip content to its own logs or to the terminal title.

## 5. Screens

Global layout: tab bar on top, content, one-line status/key-hint bar at the bottom. Brand: ink background is the
terminal's own; signal colour lime `#C8F25A` (dark) / `#4D7A00` (light), warning `#FFB84D` / `#A85F00`,
secondary text dim grey. Meaning is never colour-only (`●` online / `○` offline, words next to colours).

```
 ●─● klip   1 Home  2 Devices  3 Pair  4 History  5 Send  6 Log  7 Settings
╭─ Status ───────────────────────────────────────────────────────╮
│  ● ACTIVE     relay connected · aps1-1.relay.n0.iroh.link      │
│  this device  VCUU-LBYY-ES2H    Linux (X11)    RAM 16.6 MB     │
│  128 clips synced · last 4s ago · "https://github.com/…"       │
╰────────────────────────────────────────────────────────────────╯
╭─ Devices ──────────────────────────────────────────────────────╮
│  ● Vivo V2253   [75A6-H66R-SQDW]   connected                   │
│  ○ Pixel 8      [89JB-4KQ2-X9TM]   offline · unreachable       │
╰────────────────────────────────────────────────────────────────╯
 1-7 screens   p pause   ? help   q quit (Klip keeps running)
```

### 5.1 Global keys

| Key | Action |
|---|---|
| `1`–`7`, `Tab` / `Shift+Tab` | switch screen |
| `?` | help overlay (all keys of the current screen) |
| `p` | pause menu: 15 min · 1 h · 8 h · custom seconds · until resumed (10 years, as on Android); if paused: *Resume now* first |
| `Esc` | close overlay / cancel the current prompt |
| `q`, `Ctrl+C` | quit the TUI (daemon keeps running) |
| `Ctrl+L` | force redraw |
| `m` | toggle mouse capture (when no text field is focused) |

Mouse (when captured): click a tab, click a list row, wheel scrolls lists and previews.

**Text-focus rule.** While a text field is focused (Send editor, Join ticket, History search, Settings fields, pause
"custom seconds"), every printable key goes to the field and the global letter/digit keys above are inactive;
`Esc` leaves the field. `Alt+1`–`Alt+7`, `Ctrl+←/→` and `Ctrl+C` always work, even inside a field.
Opening the Send screen focuses its editor; `Esc` then `1`–`7` (or `Alt+n`) leaves it.

### 5.2 Home

Shows: state (`ACTIVE` / `PAUSED · 12m left` / `STOPPED`), relay mode and live relay state, this device's
fingerprint, platform, daemon RSS, total clips synced, last sync age and preview, linked devices with online dot and
last delivery error in plain words, and `pairing window open · 1:42 left` when relevant.

If the daemon is not reachable: a centred card "Klip is not running" with `S` *start it*, the last 4 log lines,
and automatic retry every second (the screen switches to normal by itself when the daemon appears).
`x` stops the daemon after a `y/N` confirmation.

### 5.3 Devices

List with name, fingerprint, online state, last error. Keys: `↑↓`/`jk` select, `Enter` details pane,
`u` unpair the selected one (`y/N` modal), `U` unpair **all** (modal shows the count; default answer is No).
`unpair_all` reports which devices were removed and which failed; a partial failure is shown, not hidden.
An empty list shows how to pair (`3`).

### 5.4 Pair

Two modes (`Tab` switches): **Show code** and **Join**.

- *Show code:* starts a pairing window; QR (half-block, via the existing `qrcode` crate), the ticket text, this
  device's fingerprint, countdown. Keys: `n` new code, `l` toggle LAN addresses (bigger QR, works offline),
  `t` cycle validity 60/120/300/600 s. If the terminal is too small for the QR, only the ticket is shown with a
  note. Leaving the screen or `Esc` cancels the window (`pair_cancel`).
- *Join:* a text field for the `klip1…` ticket (bracketed paste; whitespace, quotes and card borders are stripped
  by the ticket parser the daemon already uses for `klip pair <ticket>`), `Enter` joins with a progress indicator and the
  daemon's plain-language error on failure.
- **Approval prompt:** whenever `status.pairing_open` is true the runtime polls `pair_poll` every 500 ms, on *any*
  screen. A pending request opens a modal over everything: device name, fingerprint, "compare with the other
  screen", `y` approve / `n` decline (default none; `Esc` = decline). After approval the screen waits for the
  `paired` confirmation (up to 15 s) and says so, like the Android app.

### 5.5 History

Table: time ago, origin device, size, one-line sanitized preview (secret items masked). Right/bottom pane:
full sanitized text with scrolling (first 256 KiB; "… N more bytes" beyond). Non-UTF-8 content shows
`[binary, N bytes]`. Keys: `↑↓ PgUp PgDn g G`, `/` search (case-insensitive substring over the loaded items,
live), `Enter` copy the item to **this** device's clipboard (daemon-side, `copy_to_clipboard`),
`s` send it again to all linked devices (text items), `v` reveal/hide a secret item,
`C` clear the whole history (`y/N`). Fetching: the daemon's history answer carries the full content of every item
(the store holds up to 8 MiB), so it is **not** polled on a short timer. It is fetched when the screen opens, when
`status.total_synced_clips` changes, on `r`, and every 10 s as a safety net, only while the screen is visible.

### 5.6 Send

Multi-line editor (own `TextBuf`, UTF-8 safe, grapheme-aware cursor): typing, `Enter` newline, arrows,
`Home/End`, `Ctrl+A/E`, `Backspace/Delete`, `Ctrl+W` (delete word), `Ctrl+U` (clear), bracketed paste.
`Ctrl+S` or `F5` sends (Ctrl+Enter is not distinguishable in most terminals). Live counter
`1,234 B / 32 MiB limit` turns warning-coloured near the limit and blocks sending above it.
`Ctrl+O` loads a UTF-8 file by path (tilde expanded); a file over 1 MiB opens in *large payload mode*
(read-only preview of the first 64 KiB, sends the whole file; the editor is never asked to hold more than 1 MiB).
Result panel after sending: one line per device, `✓ delivered` or `✗ reason`. `Esc` while waiting stops waiting only;
a delivery already in flight is not recalled (the screen says so).

### 5.7 Log

Tails `daemon.log`: reads the last 256 KiB at open, then follows by byte offset every 500 ms; handles truncation
(offset > file size → restart from 0), a missing file ("no log yet"), and invalid UTF-8 (lossy). Keeps at most 5,000
lines. `f` follow on/off, `/` filter, `G` jump to end. `ERROR`/`WARN` lines are coloured. Control characters are
sanitized like clip text.

### 5.8 Settings

Edits `config.json` through the same `DaemonConfig` type and `validate()` the daemon uses (so the TUI can never write
a file the daemon rejects, and unknown fields are never dropped because every field round-trips).

| Field | Control | Notes |
|---|---|---|
| device name | text | non-empty, shown to peers |
| relay | `n0` / `lan` toggle | one-line explanation of what n0 sees |
| max history items | number | validated by `validate()` |
| max clip size | number with units (KiB/MiB) | 1 B – 256 MiB per `validate()` |
| LAN discovery | on/off | |
| poll interval | read-only | advanced, only used without X11 events |

`s` saves atomically (write temp file with mode 0600, fsync, rename). If the saved file differs from the config the
running daemon reported in `hello` (or, for an old daemon, from what was loaded when the TUI started), a banner
says *restart needed* and `R` restarts the daemon (`stop_daemon` → wait for the socket to disappear → `start_daemon`;
on failure the last log lines are shown). A malformed existing `config.json` is shown as an error with the parser
message and is never overwritten silently.
Below the fields: **Auto-start service** — shows whether the OS unit file exists (systemd user unit / launchd
plist) and offers *Install* which performs exactly what `klip install-service` does today. Also shows the app
version and, from `hello`, the daemon version; a mismatch is flagged.

## 6. Timing and resource budget

| What | Interval | Only when |
|---|---|---|
| `status` | 1 s | daemon reachable (every screen: the tab bar shows the pause state) |
| `pair_poll` | 500 ms | `status.pairing_open` |
| `history` | on open, on change of `total_synced_clips`, `r`, and every 10 s | History screen visible |
| log tail | 500 ms | Log screen visible |
| reconnect | 1 s | daemon down |
| tick | 250 ms | always (toasts, countdown) |

Every daemon call has the same timeouts as the CLI client (default and pair-join values reused). Slow calls show a
spinner after 300 ms. Budget to measure and report (not promise): release binary growth ≤ 1.5 MB, TUI process RSS,
idle CPU of the TUI over 5 minutes. If the binary budget is exceeded, `tui` becomes a default-on cargo feature of
`klip-cli` and the numbers are reported.

## 7. Daemon and protocol changes (additive only)

`postcard` encodes enum variants by index. Therefore:

- New variants are appended at the **end** of `IpcRequest` and `IpcResponse`; no existing variant is reordered,
  renamed or changed. A **golden-bytes test** pins the encoding of every existing variant so any future change that
  shifts an index fails CI.
- New requests: `Hello { client_protocol: u32 }`, `UnpairAll`, `CopyToClipboard { id: [u8; 16] }`, `ClearHistory`.
- New responses: `Hello(HelloInfo { protocol: 2, daemon_version: String, capabilities: Vec<String>, config: ConfigInfo,
  max_request_bytes: usize })` where `ConfigInfo { device_name: String, relay: String, max_history_items: usize,
  max_clip_bytes: usize, lan_discovery: bool }` is what the running daemon loaded, `UnpairedAll { removed: Vec<String>, failed: Vec<(String, String)> }`, `Copied`,
  `HistoryCleared`.
- `capabilities` contains `"unpair_all"`, `"copy_to_clipboard"`, `"clear_history"`. A TUI talking to a daemon that
  answers `Hello` with `IpcResponse::Error` (the released v0.3.6 behaviour for an unknown variant) treats it as
  protocol 1: everything that exists today works; the three new features are greyed out with
  "daemon is older (v0.3.6): restart it after upgrading".
- `UnpairAll`: removes every peer using the same code path as `unpair`, continues after a single failure, reports both lists.
- `CopyToClipboard`: the daemon looks the item up by id and writes it with the same driver path used for received
  clips, including the echo guard, so it is **not** re-broadcast as a new local copy. Unknown id → `Error`.
- `ClearHistory`: clears the local in-memory history only; nothing is sent to other devices.
- **Request size.** `MAX_IPC_REQUEST_BYTES` (8 MiB constant) becomes `config.max_clip_bytes + 64 KiB`, and `hello`
  reports it. The client checks the encoded size **before** writing and prints a clear error; if a write fails
  anyway it tries to read the daemon's answer first. Because the cap now depends on `max_clip_bytes` (up to 256 MiB),
  the daemon must **not** allocate the claimed length up front: it reads into a buffer that grows as bytes arrive
  (and enforces the cap and the I/O timeout while reading), with a test that a client announcing a huge length and
  sending nothing costs no memory. This fixes the observed `klip send -` failure with a 9 MB input
  (`Broken pipe (os error 32)`, README promised 32 MiB) and is needed so the Send screen's limit is truthful.
- **CLI parity:** `klip unpair --all [--yes]` (asks for confirmation unless `--yes`) and `klip history clear`
  (confirmation unless `--yes`). `klip unpair <device>` is unchanged.
- No change to the network protocol, keystore, pairing, or the Android crate. Upgrading the daemon still requires
  restarting it (as with every IPC change); the README already says so.

## 8. Security and privacy

- The control socket stays 0600; the TUI adds no new listener and no network access of its own.
- Clip content appears on screen only after sanitizing (4.4). Secrets are masked until revealed per item.
- `config.json` is written 0600 via temp+rename; `keystore.json` is never read by the TUI.
- The pairing ticket is a one-time secret: it is shown only on the Pair screen, never logged, and the window is
  cancelled when the screen is left.
- The alternate screen keeps clip text out of the terminal's scrollback; terminal multiplexers or screen recorders
  can still capture it (stated in the README).

## 9. Compatibility matrix (to be verified, not assumed)

| Combination | Expectation | Verified by |
|---|---|---|
| new TUI + new daemon | all features | in-process e2e |
| new TUI + released v0.3.6 daemon | all existing features, 3 greyed | real installed daemon, by hand + stub-server test |
| old CLI binary + new daemon | all old commands work (indices unchanged) | golden-bytes test |
| no TTY | help + exit code 2 as before | characterization test |

## 10. Verification plan

Recorded **before** touching anything (first commit of M1): characterization tests for the current CLI — `--help` of
`klip` and each subcommand (golden text), the status/history/pair card renderers with fixed inputs, and the
no-argument/no-TTY exit code.

1. **Unit:** `update()` transition tables per screen; `TextBuf` (insert/delete/cursor across multi-byte, wide and
   combining characters; property tests: never panics, always valid UTF-8, cursor on a grapheme boundary);
   `sanitize` (escape sequences, bidi, zero-width, wide chars, truncation); settings validation parity with
   `DaemonConfig::validate`; number/unit parsing; time formatting.
2. **Key fuzz:** ≥ 100,000 random key/mouse/paste/resize sequences through `update` + `view` on `TestBackend` at random
   sizes (including 1×1 and 500×200): no panic, state invariants hold (selection in range, modal stack consistent).
3. **Snapshots:** every screen × states (empty, populated, daemon down, paused, error, secret item, long names, CJK/
   emoji, too small) × sizes 60×16, 100×30, 200×60, as text golden files reviewed in the diff.
4. **Daemon e2e (in-process):** two real daemons (existing test harness as a dev-dependency) driven through the
   TUI's own effect pipeline: pair via Pair screen (host approves), send and observe on the peer, history copy,
   unpair one, unpair all, pause/resume, settings save + restart, daemon killed and restarted mid-session
   (reconnect), 9 MB and over-limit sends (clear errors), secret masking.
5. **Protocol:** golden bytes for all existing variants; Hello against a stub daemon that answers `Error` for unknown
   variants (old daemon) → degraded mode.
6. **PTY e2e on the real `klip` binary:** spawn in a pseudo-terminal (`portable-pty`), feed keystrokes, read the screen
   with a terminal emulator (`vt100`), assert on text. Cases: start, switch every screen, quit restores the terminal
   (alternate screen left, cooked mode back), SIGTERM, SIGHUP, panic injection (test-only env switch), resize,
   non-TTY fallback, `NO_COLOR`.
7. **Manual on this machine** against the installed v0.3.6 daemon and a freshly built one: screen dumps of every
   screen are attached to the milestone's commit message; idle CPU/RSS of the TUI over 5 minutes; binary size delta.
8. **Regression:** the full existing suite (73 tests today), `cargo clippy --workspace --all-targets -D warnings`,
   `cargo fmt --check`, CI matrix incl. macOS/Windows compile.

**Cannot be verified here and will be written as "unverified" in the README:** appearance and key handling in
specific terminal emulators and multiplexers (kitty, gnome-terminal, alacritty, tmux, screen, SSH sessions),
truecolor/256-colour fallbacks on real displays, macOS runtime, any Windows use.

## 11. Milestones

Each milestone ends with a green workspace test run, clippy/fmt clean, CI green, a verified commit, and README text
limited to what was shown. The pre-release tag `v0.4.0-alpha` is cut after M3 (standing permission), with the APK and
binaries rebuilt by the existing release workflow.

- **M0 (inside M1):** characterization tests; `klip-tui` crate skeleton; `DaemonApi` + `klip-cli` implementation.
- **M1 — foundation:** terminal guard, theme, sanitize, runtime, Elm core, widgets; Home, Devices (single unpair),
  pause menu, help, daemon-down card with start, global keys, mouse; FakeDaemon; unit/key-fuzz/snapshot tests; PTY
  harness with start/quit/SIGTERM/resize tests; works against the released v0.3.6 daemon.
- **M2 — actions and protocol:** Hello/UnpairAll/CopyToClipboard/ClearHistory in `klip-proto` + daemon + tests;
  request-size fix and client pre-flight; Pair (show/join/approval), Send (editor, file, large mode), History
  (search, copy, resend, reveal, clear); `unpair --all`, `history clear`; in-process daemon e2e; degraded-mode test.
- **M3 — operations and polish:** Log, Settings (config editor, restart, service), version mismatch notice,
  theme variants, README/AUDIT docs/BRAND updated, measurements (size, RSS, CPU), full manual run, release.

## 12. Risks and how they are handled

| Risk | Handling |
|---|---|
| Terminal left broken after a crash | guard + panic hook + signal handlers; PTY tests for each path |
| Slow/hung daemon freezes the UI | all I/O in effects with timeouts; UI never awaits inline |
| Stale reply overwrites newer data | generation numbers; replies for old generations dropped |
| Huge clip makes rendering slow | previews capped at 256 KiB; editor capped at 1 MiB; only visible lines are laid out |
| Escape-sequence injection from a clip | sanitize layer (4.4) with tests using real escape payloads |
| `ratatui`/`crossterm` version drift or MSRV | pin exact versions in the workspace manifest; `cargo check` on the pinned MSRV (1.91) at plan time |
| PTY tests flaky | wait on screen content with a deadline, never fixed sleeps; run in CI on Linux |
| Binary growth | measured; feature gate if over budget |
| Protocol mistakes break old clients | append-only variants + golden-bytes test + stub-daemon test |

## 13. Dependencies (planned)

`ratatui` (0.30.x), `crossterm` (0.29.x, `event-stream`), `futures-util`, `async-trait`; already in the workspace: `tokio`, `qrcode`,
`unicode-width`, `serde`, `serde_json`, `owo-colors` (CLI only). Dev-only: `portable-pty`, `vt100`, `proptest` (or the
workspace's `rand` for the fuzz loops). A multi-line editor crate is deliberately **not** used (`tui-textarea` targets
an older `ratatui`); `TextBuf` is small and fully tested instead. Exact versions are fixed at plan time after
`cargo check` on this machine.
