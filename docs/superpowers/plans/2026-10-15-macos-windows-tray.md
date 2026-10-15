# macOS and Windows tray + Windows control channel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (native execution, chosen
> because the tasks share interfaces and the only feedback loop for Windows/macOS code is CI).
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** the same tray icon and menu on macOS and Windows as on Linux, a working Windows control
channel, and honest tests on the GitHub runners.

**Architecture:** the tray's logic (poll, fetch, actions, busy guard, problem line) moves into a
platform-neutral `controller` that talks over channels; each OS only implements presentation (ksni on
Linux, tao + tray-icon on macOS/Windows). Windows gets a named-pipe control channel next to the Unix
socket. Pure parts (ids, icons, plist/registry texts) are unit-tested on Linux.

**Tech Stack:** Rust, tokio, ksni (Linux), tray-icon + tao (macOS/Windows), tokio named pipes and
windows-sys (Windows), GitHub Actions runners as the macOS/Windows test bed.

**Spec:** `docs/superpowers/specs/2026-10-15-macos-windows-tray-design.md`

## Global Constraints
- The Linux binary must not grow and its five D-Bus tests (`crates/klip-cli/tests/tray_dbus.rs`) keep passing.
- New dependencies compile only on their OS: `[target.'cfg(target_os = "macos")']`, `[target.'cfg(windows)']`.
- No GTK, Python, webview. `unsafe` only where a Windows API needs it, each with a `// SAFETY:` line.
- Windows pipe: `reject_remote_clients(true)` and an owner-only DACL (`D:P(A;;GA;;;OW)`).
- Login entries are marker-checked, written atomically, and a foreign file is never overwritten.
- Machine budget: one heavy build/test at a time; `cargo ... -j2`; `--test-threads=2`.
- Release notes list what was NOT verified (real menu bar / tray look, real clicks, real desktop clipboard).

## Review Focus
1. Headless runner / no menu bar / no Explorer shell: `klip tray run` must exit with a message, not panic (test: Task 4/7 smoke accepts a clean error exit).
2. A click on a menu id that no longer exists or is garbage: ignored (test: `Action::from_id` rejects it).
3. A path with spaces, quotes or `%` in the LaunchAgent plist / Run key: escaped or refused (tests in Task 3).
4. Two users or two Klip directories on one Windows machine: separate pipe names (test: name differs per directory, stable per directory).
5. A pipe client that arrives while the single instance is busy: retried, not "daemon down" (Task 5 test with parallel clients).
6. Linux regression from the controller extraction (existing D-Bus tests are the gate in Task 1).

## File Structure
- `crates/klip-cli/src/tray/model.rs` (modify): `Action::id/from_id`, `Summary.problem`.
- `crates/klip-cli/src/tray/controller.rs` (create): neutral loop.
- `crates/klip-cli/src/tray/linux.rs` (rename of `app.rs`): ksni presentation only.
- `crates/klip-cli/src/tray/raster.rs` (create): RGBA icons.
- `crates/klip-cli/src/tray/autostart.rs` (create): per-OS login entry (Linux code moves here).
- `crates/klip-cli/src/tray/desktop.rs` (create): tao + tray-icon presentation (macOS/Windows).
- `crates/klip-daemon/src/ipc_pipe.rs` (create, Windows) and edits in `ipc_server.rs`, `engine.rs`, `config.rs` (pipe name).
- `crates/klip-cli/src/client.rs` (modify): Windows `exchange`.
- `.github/workflows/ci.yml` (modify): `platform-tests` job on macOS and Windows.

---

### Task 1: Neutral controller, action ids, problem line (Linux only, behaviour unchanged)

**Files:** modify `tray/model.rs`, `tray/app.rs` -> `tray/linux.rs`, `tray/mod.rs`; create `tray/controller.rs`.

**Interfaces:**
- Produces: `Action::id(&self) -> String` ("start", "send", "auto:1", "auto:0", "pause:600", "resume", "terminal", "quit"), `Action::from_id(&str) -> Option<Action>`.
- Produces: `controller::Cmd { Refresh, Do(Action) }`, `controller::Out { Summary(Summary), Problem(String) }`,
  `controller::run(cmd_rx: UnboundedReceiver<Cmd>, cmd_tx: UnboundedSender<Cmd>, out: UnboundedSender<Out>) -> ()`
  (returns on `Do(Quit)`, SIGTERM/Ctrl-C, or when `out` is closed).
- Produces: `Summary.problem: Option<String>` shown by `menu_plan` as a first disabled entry "⚠ <text>".

- [ ] **Step 1: tests** in `model.rs`: ids round-trip for every `Action`; `from_id("")`, `from_id("pause:x")`, `from_id("auto:2")`, `from_id("rm -rf")` are `None`; `menu_plan` with `problem = Some("boom")` starts with `Label("⚠ boom")`.
- [ ] **Step 2: run** `cargo test -p klip-cli --lib tray::model` -> FAIL (no `id`).
- [ ] **Step 3: implement** `Action::id/from_id` and `Summary.problem` (default `None`; `summarize` leaves it `None`; the controller sets/clears it).
- [ ] **Step 4: move** the loop out of `app.rs` into `controller.rs`: `Cmd`, `fetch`, `perform`, `start_daemon`, `sent_problem`, the busy guard and spawned polls; on an action error the controller sends `Out::Problem(text)` and keeps `problem` in the next `Out::Summary` for 30 s. `linux.rs` keeps only `KlipTray`, the ksni glue, `host_wait`, icons install; it forwards `Out::Summary` to `handle.update` and `Out::Problem` to `klip_daemon::notify::say`.
- [ ] **Step 5: run** the whole `cargo test -p klip-cli` (unit + `tray_dbus` + `tray_cli`) -> all pass; `cargo clippy -p klip-cli --all-targets -D warnings`.
- [ ] **Step 6: commit** "refactor(tray): neutral controller, action ids, problem line".

### Task 2: Run-time icons (`raster.rs`)

**Files:** create `tray/raster.rs`; modify `tray/mod.rs` (`pub mod raster`).

**Interfaces:** `pub struct Rgba { pub w: u32, pub h: u32, pub data: Vec<u8> }`;
`pub fn render(state: State, badge: bool, size: u32, ink: [u8; 3]) -> Rgba` (premultiplied-free RGBA, alpha carries the shape; `ink` is the glyph colour; faded states scale alpha like the SVGs).

- [ ] **Step 1: tests:** size = `w*h*4`; connected has opaque pixels at both dot centres and on the line, waiting has none on the line centre but on the three dots, paused has the two bars, stopped is fainter than connected (max alpha lower), badge adds opaque pixels in the top-right corner and nothing in the bottom-left; every pixel RGB equals `ink` where alpha > 0; rendering is deterministic.
- [ ] **Step 2: implement** with signed-distance coverage and 4x4 supersampling over the 64x64 design grid of `icons.rs` scaled to `size`.
- [ ] **Step 3: look at it:** a `#[test] #[ignore]` writes `target/tray-preview.png` (all states, light and dark ink) using the `image` crate already in the tree for tests only (dev-dependency); view it with the image reader and fix what looks wrong.
- [ ] **Step 4: commit** "feat(tray): run-time RGBA icons for macOS and Windows".

### Task 3: Autostart module with per-OS entries

**Files:** create `tray/autostart.rs` (Linux code moved from `mod.rs`: `autostart_path`, `desktop_entry`, `install_autostart`, `uninstall_autostart`, `autostart_installed`, `MARKER`, tests); modify `tray/mod.rs`, `main.rs` (`tray_command`).

**Interfaces:** `autostart::{install(exe: &str) -> Result<String, String>, uninstall() -> Result<bool, String>, installed() -> bool}` (the `String` is the place it wrote, for the message); pure: `launch_agent_plist(exe: &str) -> Result<String, String>` (macOS), `run_key_value(exe: &str) -> Result<String, String>` (Windows: `"C:\\..\\klip.exe" tray start`).

- [ ] **Step 1: tests (run on Linux):** plist contains `Label` `com.klip.tray`, `ProgramArguments` = exe, `tray`, `run`, `RunAtLoad`, `LimitLoadToSessionType` `Aqua`, a marker comment; paths with `&`, `<`, `>`, `"` are XML-escaped; a path with a newline or NUL is refused. Run key value quotes the path and refuses `"` and `%` and newline. Existing Linux entry tests keep passing after the move.
- [ ] **Step 2: implement** the pure generators; macOS `install` writes `~/Library/LaunchAgents/com.klip.tray.plist` atomically and refuses a foreign file (marker); Windows `install` runs `reg add HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v KlipTray /t REG_SZ /d <value> /f`, `uninstall` `reg delete ... /f`, `installed` `reg query` (exit status).
- [ ] **Step 3: run** `cargo test -p klip-cli`; **commit** "feat(tray): per-OS login entries".

### Task 4: macOS tray (`desktop.rs`, Terminal, CI job)

**Files:** create `tray/desktop.rs`; modify `tray/mod.rs` (`#[cfg(any(target_os="macos", windows))] mod desktop`, `open_terminal` per OS, `run()` per OS), `crates/klip-cli/Cargo.toml` (`tray-icon`, `tao` under `cfg(any(target_os = "macos", windows))`), `lib.rs`/`main.rs` (tray gates become `any(linux, macos, windows)`), `.github/workflows/ci.yml`.

**Interfaces:** `desktop::run(cmd_tx, out_rx) -> Result<(), String>` runs the tao event loop on the calling (main) thread; an `EventLoopProxy<UserEvent>` carries `Out` to it; `muda` menu ids are `Action::id()`; `TrayIconEvent::Click { button: Middle, button_state: Up }` sends `Cmd::Do(Action::SendImage)`.

- [ ] **Step 1: write** the glue after reading the pinned versions' examples (`cargo fetch`; `~/.cargo/registry/src/*/tray-icon-*/examples`): build the icon with `Icon::from_rgba(raster::render(.., 44, [0,0,0]).data, 44, 44)` and `with_icon_as_template(true)`; set activation policy `Accessory`; rebuild the menu from `menu_plan` on every `Out::Summary` (labels escaped as plain text), tooltip = title + body; map menu events through `Action::from_id` (unknown ids ignored).
- [ ] **Step 2: open_terminal (macOS):** refuse an exe path containing `"` or `\`; run `osascript -e 'tell application "Terminal" to do script "<exe>"' -e 'tell application "Terminal" to activate'`; unit-test the generated AppleScript text on Linux.
- [ ] **Step 3: wire** `run()`: lock, then `tokio::spawn(controller::run(..))`, then `desktop::run` on the main thread (blocking); on event-loop exit send `Quit`.
- [ ] **Step 4: CI job** `platform-tests` (matrix macos-latest): `cargo test -p klip-cli --lib` and a smoke step: start `klip tray run` with a temp `KLIP_CONFIG_DIR` in the background, wait 8 s, print its RSS (`ps -o rss`), then kill it; accept "no display" style failures but fail on a panic message (`grep -i panicked`).
- [ ] **Step 5: push the branch, watch CI**, fix until green; **commit** "feat(tray): macOS menu-bar tray".

### Task 5: Windows control channel

**Files:** create `klip-daemon/src/ipc_pipe.rs`; modify `ipc_server.rs` (connection handler generic over `AsyncRead + AsyncWrite + Unpin + Send`), `engine.rs` (start the pipe server on Windows), `config.rs` (`ipc_pipe_name()` from the Klip directory hash), `klip-cli/src/client.rs` (Windows `exchange`), `klip-daemon/Cargo.toml` (`windows-sys` features `Win32_Security`, `Win32_Security_Authorization`, `Win32_Foundation`), `ci.yml` (matrix windows-latest).

**Interfaces:** `get_ipc_pipe_name() -> String` (`\\.\pipe\klip-<16 hex of blake3(klip dir)>`; stable per directory, different between directories); `IpcServer::serve_pipe(name)`.

- [ ] **Step 1: tests (run on Linux for the pure part):** pipe name stable for one dir, different for two dirs, starts with `\\.\pipe\klip-`, 16 hex digits. Windows-only integration test (`#[cfg(windows)]`): start the server in-process on a unique name, send `Hello` and `GetStatus` from two parallel clients, get answers; a second `first_pipe_instance` create fails.
- [ ] **Step 2: implement** server loop: create instance with `ServerOptions::new().first_pipe_instance(true).reject_remote_clients(true)` and an owner-only security descriptor from `ConvertStringSecurityDescriptorToSecurityDescriptorW("D:P(A;;GA;;;OW)")` via `create_with_security_attributes_raw` (free with `LocalFree`); loop `connect().await`, create the next instance first, spawn the handler.
- [ ] **Step 3: client** (`ClientOptions::new().open(name)`; `ERROR_FILE_NOT_FOUND` -> `ClientError::Down`; `ERROR_PIPE_BUSY` -> retry 20 x 50 ms), same framing as Unix.
- [ ] **Step 4: CI:** windows-latest runs `cargo test -p klip-daemon -p klip-cli --lib` plus the new integration test; push, fix until green; **commit**.

### Task 6: Portable CLI and terminal app on Windows

**Files:** `klip-cli/src/lib.rs`, `main.rs`, `Cargo.toml` and `klip-tui` gates: `cfg(unix)` becomes `cfg(any(unix, windows))` where the code is portable; whatever does not compile is cut with a comment `shortcut:` and listed in the README.

- [ ] **Step 1:** flip the gates, `cargo check` on the runner (CI) and read the errors; fix or cut one by one.
- [ ] **Step 2: CI e2e on Windows:** a PowerShell step starts `klip start`, then `klip status`, `klip send hello`, `klip history`, `klip stop` with a temp `KLIP_CONFIG_DIR` and `KLIP_CLIPBOARD=memory`; two daemons pair over loopback with `klip pair` / ticket and a text arrives (reuse the structure of `crates/klip-cli/tests/cli_daemon.rs` where it is portable).
- [ ] **Step 3: commit** "feat(windows): CLI and terminal app can control the daemon".

### Task 7: Windows tray

**Files:** `tray/desktop.rs` (theme-aware icon, `CREATE_NO_WINDOW`), `tray/mod.rs` (`open_terminal`: `cmd /c start "Klip" <exe>`; `start_detached` with `creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)`), `autostart.rs`, `ci.yml`.

- [ ] **Step 1: theme:** read `HKCU\...\Themes\Personalize\SystemUsesLightTheme` (windows-sys `RegGetValueW`, `// SAFETY:`) -> ink white on dark, black on light; unit-test the pure mapping `ink_for(light: bool)`.
- [ ] **Step 2: CI smoke** like macOS (PowerShell: start, sleep 8, print `WorkingSet64`, stop; fail on a panic text).
- [ ] **Step 3: push, fix until green; commit** "feat(tray): Windows notification-area tray".

### Task 8: Docs, version, release

- [ ] **Step 1:** README (platform table, tray section, Windows/macOS limits), roadmap, release notes with a NOT-verified list; version 0.7.0 (Cargo, Android 0.7.0-alpha / 19, TUI snapshots with `--features testkit`).
- [ ] **Step 2:** full verification on Linux (`fmt`, `clippy -D warnings`, `cargo test --workspace`, `cargo audit`), CI green on the branch, **one Sonnet reviewer** on the whole branch diff, fix real findings.
- [ ] **Step 3:** merge `platforms` into `main` (fast-forward), build release binary and APKs, tag `v0.7.0-alpha`, install the Linux binary, check the Linux tray again with the D-Bus tests plus the real panel, publish notes.

## Self-Review
- Spec coverage: criteria 1-5 -> Tasks 4, 5-7, 1 (Linux gate), 4/7 Cargo cfgs, 8 notes. Findings (no Windows IPC / TUI gates) -> Tasks 5-6. Rulings: branch (8), tray-icon+tao (4,7), controller (1), ids (1), icons (2), problem line (1,4,7), autostart (3), terminal (4,7), pipe+DACL (5), console flash (7).
- No placeholders: Task 4 step 1 and Task 6 step 1 are deliberately "read the pinned crate / read the compiler" because the exact API of the pinned versions is fetched at that moment; the interfaces they must produce are fixed above.
- Types: `Action::id/from_id`, `Cmd`, `Out`, `controller::run`, `desktop::run`, `raster::render`, `autostart::{install,uninstall,installed}`, `get_ipc_pipe_name` are used with the same names throughout.
