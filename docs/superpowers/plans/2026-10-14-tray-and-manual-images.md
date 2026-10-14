# Tray app and manual image sending Implementation Plan

> **For agentic workers:** executed inline by the author (owner gave blanket permission). Steps use checkbox syntax.

**Goal:** images copied on the laptop are held until sent (tray, `klip share`, window), and a full tray app (icon + window) does everything the terminal app does.

**Architecture:** new control-protocol variants appended at the end; the daemon holds the latest captured image; a hidden `klip api` JSON bridge exposes the control socket and the file helpers; a Python/GTK3/XApp tray (embedded in the binary, written to the data dir on demand) is the UI.

**Tech Stack:** Rust (tokio, serde_json, qrcode, image), Python 3.12 + PyGObject (Gtk 3, XApp, cairo, GdkPixbuf), Xephyr for GUI tests.

**Spec:** `docs/superpowers/specs/2026-10-14-tray-and-manual-images-design.md`

## Global Constraints

- No new Rust dependency except enabling what the workspace already has (serde_json, qrcode). `klip` binary must stay under 8,912,896 bytes (CI budget).
- IPC variants are appended at the end; golden-bytes tests for old variants stay unchanged and pass.
- `cargo fmt --check`, `clippy --workspace --all-targets -D warnings` clean; all existing tests stay green.
- One heavy command at a time on this 7.6 GB machine (`-j2`, `nice -n 10`); GUI tests run in a nested X server (`Xephyr :99`), never on the real display; never touch the real daemon or the real phone in tests (`KLIP_CONFIG_DIR`, `KLIP_CLIPBOARD=memory`).
- Python code must pass without GTK for the logic modules; GTK tests are skipped (not failed) when `gi`/XApp are missing, so CI on a plain runner stays green.

## Review Focus

- A held image is never sent without an action, and never lost by an unrelated text copy. (T4)
- `SetAutoImages` persists and a hand-edited `config.json` keeps its other keys' values. (T3, T4)
- The bridge never sends clip contents or secrets except on an explicit `history_text`. (T6)
- A pairing ticket does not stay alive after the person leaves the page. (T9/T10)
- A dead daemon, a dead bridge and malformed replies do not crash the tray. (T8/T10)

### Task 1: Protocol variants
Files: `crates/klip-proto/src/ipc.rs`. Append `IpcRequest::{GetImageState, SendPendingImage, SetAutoImages{enabled}, GetThumbnail{id}}`, `IpcResponse::{ImageState(ImageStateInfo), Thumbnail(Vec<u8>)}`, `WaitingImageInfo`, `ImageStateInfo`, `CAP_SHARE_IMAGE`. Tests: golden bytes of each new variant; old goldens untouched.
- [ ] tests, RED, implement, GREEN, commit

### Task 2: Thumbnails
Files: `crates/klip-diff/src/image_transcode.rs`, `lib.rs`. `thumbnail_png(bytes, max_side) -> Result<Vec<u8>, String>` (validates first, keeps aspect ratio, never upscales). Tests: size/aspect, refuses a bomb header, garbage.

### Task 3: Core state
Files: `crates/klip-core/src/config.rs` (`auto_send_images`, `save`), `sync.rs` (pending image, `image_state`, `hold_image`, `send_pending_image`, `set_auto_images`, `thumbnail`). Tests: default false; save/load round trip keeps other fields; replace; expiry (injectable clock); send removes it.

### Task 4: Daemon
Files: `crates/klip-daemon/src/engine.rs` (route images), `ipc_server.rs` (4 handlers, `Hello` capability). e2e: manual mode holds (peer gets nothing), `SendPendingImage` delivers byte for byte and reports, auto mode sends at once, toggle persists, held image survives a text copy, expiry reply, thumbnail PNG.

### Task 5: CLI commands
Files: `crates/klip-cli/src/main.rs`, `ui.rs`, goldens. `klip share`, `klip images [auto|manual]`. Tests: golden help files updated, `klip share` with nothing held, with a held image against a real daemon.

### Task 6: JSON bridge
Files: new `crates/klip-cli/src/bridge.rs`, `main.rs`. Tests (real binary + real daemon): status JSON has fingerprints; history JSON has no contents; concurrent requests; bad JSON line answered with an error, bridge keeps running; config_get/config_set round trip; EOF exits.

### Task 7: Tray launcher and installer
Files: new `crates/klip-cli/src/tray.rs`; embeds `extra/tray/klip_tray/*.py`. `klip tray [run|install|uninstall|status]`. Tests: files written once per version into a temp data dir, `.desktop` content, uninstall removes only created files, missing python/gi gives the explanatory error.

### Task 8: Python logic (no GTK)
Files: `extra/tray/klip_tray/{model,bridge,texts}.py`, `extra/tray/tests/test_model.py`, `test_bridge.py`. State reducer, formatting, validation, bridge client with a fake child process (restart on exit, timeouts, malformed lines).

### Task 9: Python UI
Files: `extra/tray/klip_tray/{icons,theme,widgets,pages,app,__main__}.py`, `extra/tray/tests/test_ui.py`. Offscreen render of each page to PNG, reviewed by eye; keyboard use; no exceptions with a fake bridge in every state.

### Task 10: Integration
Files: `extra/tray/tests/integration.py`. Two real isolated daemons + `klip api` + the Python controller: pair, text, held image/share, toggle persistence, history, unpair, daemon restart.

### Task 11: Verify and ship
- [ ] Version 0.6.0 / versionCode 16 (Android unchanged but versioned), snapshots, README, docs.
- [ ] fmt, clippy, full tests, Python tests, size budget.
- [ ] Fresh Sonnet review of the whole branch; fix pass.
- [ ] Commit, push, tag `v0.6.0-alpha`, local release binary + APKs, `systemctl --user restart klip.service`, `klip tray install`, start the tray, check it from outside.
- [ ] Release notes (rulings + not verified), memory.
