# File transfer round 3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline) to implement this plan task by task.

**Goal:** Close the open items of B, C and F: sender cancel signal, byte cap on kept partials, oversize-image backoff, picker
separator, macOS/Windows pickers, Windows login without console flash, a version-1-only switch with compatibility tests,
folders sent from the phone, the debug harness and emulator checks, Android 8-9 wording. One release: v0.10.0-alpha.

**Architecture:** Small changes in `klip-net` (reset code 77), `klip-core` (kept index, generalized folder job), `klip-platform`
(Linux backoff), `klip-cli` (tray pickers, login entry), `klip-android` (UniFFI `send_folder` + `FolderSource`), Kotlin (SAF walk,
button, texts) and a debug-only source set for emulator checks.

**Tech Stack:** Rust, iroh/noq QUIC, UniFFI, Kotlin framework views, JUnit, Android 14 emulator.

**Spec:** `docs/superpowers/specs/2026-10-18-files-round3-design.md` (calls settled after the builder check included).

## Global Constraints

- One heavy build/test at a time (`-j2`); check `df -h /` before big builds (e2e tests clear old temp dirs themselves).
- No new IPC variants; `TransferInfo`/`ConfigInfo` unchanged; protocol versions unchanged; reset code 77 = deliberate cancel.
- Binary budget 9,961,472 bytes (v0.9.0: 9,705,208). No new dependency outside `Cargo.lock`.
- Every behaviour has a test seen failing first (mutation check where the change is a guard); Windows/macOS-only code is split
  into pure builders/parsers tested everywhere, and stated NOT verified on a real desktop in the notes.
- Work on a branch; CI runs on PRs; heavy memory test is `#[ignore]`d and run alone.

## Review Focus

1. A cut transfer (connection drop, dropped stream, a sender read error) must still be resumable; only code 77 deletes the part (Task 1).
2. A phone folder with a deep or huge tree, an unreadable document, a virtual document, a name that is hostile, a descriptor that
   cannot be opened, and the user cancelling between files (Tasks 7-9).
3. Windows login entry with a path holding spaces, quotes or `%`, with VBScript missing, and an old v0.9 entry present (Task 5).
4. A version-1-only device on either end: single files work, folders are refused cleanly, nothing hangs (Task 6).
5. Picker output with spaces, unicode, tabs, newlines (zenity separator), cancel (empty output) and an error exit (Tasks 4).

---

### Task 1: sender cancel is told to the receiver

**Files:** `crates/klip-net/src/file.rs`, `node.rs`; `crates/klip-core/src/files.rs`, `transfers.rs`; tests in both and `crates/klip-daemon/tests/files_e2e.rs`.

**Interfaces:** `pub const CANCEL_CODE: u32 = 77;` `FileError::CancelledByPeer` ("the other device cancelled"); `io_err`: text containing
`reset by peer: error 77` => `CancelledByPeer`, other `reset by peer` => `Io` (a cut), `sending stopped by peer` stays
`Peer("the other device stopped the transfer")`; `FileStreams::cancel(self)` async: `tx.reset(77)`, sleep 200 ms, close. Receiver
(`receive_incoming_from`): `CancelledByPeer` => discard the part, row `Cancelled` with that text. Coordinator: after a `Cancelled` attempt
use `streams.cancel()` unless `me.transfers.is_stopping()`.

- [ ] Tests first: net unit `a_reset_with_code_77_means_cancelled_and_other_resets_are_cuts`; core unit `a_peer_cancel_deletes_the_kept_part_and_a_cut_keeps_it`
  (stream error text injected through a source that fails with `io::Error::other("stream reset by peer: error 77")` vs `error 0`);
  e2e `cancelling_on_the_sending_side_tells_the_receiver_and_nothing_is_kept` (sender cancel => no `.part` after 3 s) and
  `a_dropped_sender_stream_is_a_cut_and_the_part_is_kept_and_resumes` (raw `send_file_v2_offset` with a failing source then the normal
  resend resumes; key via `klip_core::files::resume_key`). Convert the two resume e2e tests (`cutting_a_transfer...`, `..._no_longer_matches...`)
  to cut with the raw failing stream instead of `cancel_transfer`; fix `unpairing_the_sender_on_the_receiving_side...` and the CLI cancel
  test expectations to the new rule.
- [ ] Run RED, implement, run GREEN, mutation-check (code 77 treated as a cut => the cancel e2e fails), commit.

### Task 2: kept partials are capped in bytes

**Files:** `crates/klip-core/src/files.rs`. **Interfaces:** `KeptIndex::register(path, device)` also enforces `MAX_KEPT_BYTES = 16 GiB`
(sizes read from the files, oldest removed first until under).

- [ ] Test `kept_partials_are_capped_in_bytes_too` (sparse files via `set_len`, own `KeptIndex`); RED; implement; mutation; commit.

### Task 3: oversize image backoff on Linux

**Files:** `crates/klip-platform/src/linux.rs`. **Interfaces:** `fn image_read_allowed(now: Instant, skip_until: &mut Option<Instant>, oversize_seen: bool) -> bool`
(pure); after an oversize image, `skip_until = now + 2 s`; text is read every tick.

- [ ] Test `an_oversize_image_is_not_read_again_for_two_seconds` (explicit instants); RED; implement and wire; commit.

### Task 4: pickers (separator, macOS, Windows)

**Files:** `crates/klip-cli/src/tray/mod.rs`, `controller.rs`. **Interfaces:** zenity uses `--separator=\x1f`; `parse_picker_output` splits on `\x1f`
when present else newlines; `picker_command_for(os: Os, has: impl Fn(&str)->bool)` with `Os::{Linux, MacOs, Windows}` returning
programme+args; macOS `osascript -e 'POSIX path of (choose file with prompt "Send files with Klip" with multiple selections allowed)'`-style
script producing one POSIX path per line (a user cancel is exit code 1 with `-128` in stderr => empty list, not an error); Windows
`powershell -NoProfile -STA -Command <script>` printing one full path per line (cancel prints nothing); `parse_cancel(os, status, stderr) -> bool`;
`picker_available()` true on macOS/Windows when the programme exists (always), Linux when zenity/kdialog exist.

- [ ] Tests: separator parse with a newline in a name, zenity args contain the separator, the macOS script text quotes a prompt safely and
  asks for multiple selection, the Windows script sets `Multiselect`, cancel detection for macOS (`-128`) and an empty Windows output, availability per OS
  with an injected `has`. RED; implement; the controller's `pick_files` calls the OS builder; commit.

### Task 5: Windows login entry without a console flash

**Files:** `crates/klip-cli/src/tray/autostart.rs`. **Interfaces:** `pub fn login_entry_form(has_script_host: bool) -> Form` (`Form::Script | Form::Plain`);
`pub fn run_value_script(vbs_path: &str) -> Result<String,String>` = `wscript.exe //B //nologo "<vbs>"`; `pub fn vbs_text(exe: &str) -> Result<String,String>`
(`CreateObject("WScript.Shell").Run """<exe>"" tray run", 0, False`, quotes doubled, `%`/newline/NUL refused);
`run_value_is_klips` accepts both forms (the script form must name a `start-tray.vbs`); `install` writes
`%LOCALAPPDATA%\Klip\start-tray.vbs` (Windows only) and `uninstall` removes it; the probe for the script host is Windows-only
(`System32\wscript.exe` exists and `reg query HKCR\VBScript` succeeds).

- [ ] Tests: form decision, run value text, vbs text with spaces/quotes/`%`, ownership of old and new values, foreign values refused; RED; implement;
  README note ("run `klip tray install` again to switch to the quiet login entry"); commit.

### Task 6: `file_protocol_v2` switch and compatibility e2e

**Files:** `crates/klip-core/src/config.rs`, `engine.rs` (NetConfig.file_v2), `crates/klip-daemon/tests/files_e2e.rs`. **Interfaces:** `DaemonConfig.file_protocol_v2: bool`
(default true; documented in README as a troubleshooting switch).

- [ ] Config tests (default, parse, unknown key still an error). E2E: `a_version_one_receiver_gets_a_single_file_whole_and_refuses_a_folder_with_update_klip`,
  `a_version_one_sender_delivers_to_a_version_two_receiver`, `a_cut_to_a_version_one_receiver_restarts_from_zero` (no key => no kept part). RED; implement; commit.

### Task 7: folder job from entries with an opener (core)

**Files:** `crates/klip-core/src/transfers.rs`, `folders.rs`. **Interfaces:** `pub struct ExternalEntry { pub rel: Vec<String>, pub name: String, pub size: u64 }`;
`pub type Opener = Arc<dyn Fn(usize) -> io::Result<std::fs::File> + Send + Sync>`;
`SyncCoordinator::send_folder_entries(self:&Arc<Self>, top: String, entries: Vec<ExternalEntry>, opener: Opener, to: Option<String>) -> Vec<Result<TransferInfo,String>>`
(the same row/limits/stop/retry logic as `send_folder`; names and rel cleaned with `sanitize_file_name`, limits 1000 files / 16 levels, total size = sum; each file opened
only on its turn through `PreparedFile::from_file(file, name, None)` and checked regular); `send_folder` becomes a thin wrapper that builds a path opener.

- [ ] E2E (daemon, direct coordinator call) `a_folder_from_an_opener_arrives_with_its_structure`, `an_opener_that_fails_for_one_file_fails_only_that_file`,
  `entries_over_the_limits_are_refused_before_any_file_is_opened`, `an_opener_is_called_once_per_file_and_never_ahead_of_its_turn` (a counter in the closure). RED; refactor; the existing folder e2e must still pass; commit.

### Task 8: Android engine `send_folder`

**Files:** `crates/klip-android/src/lib.rs`, `tests/folders.rs`. **Interfaces:** UniFFI `FolderFile { rel: Vec<String>, name: String, size: u64 }`; foreign trait
`FolderSource { fn open_fd(&self, index: u32) -> i32 }` (the engine owns the returned fd, -1 = cannot open, called from `spawn_blocking`);
`KlipEngine::send_folder(&self, top: String, files: Vec<FolderFile>, source: Arc<dyn FolderSource>) -> Result<Vec<TransferView>, KlipError>`.

- [ ] Tests (two engines, a mock `FolderSource` returning `into_raw_fd` of temp files): `a_folder_sent_from_fds_arrives_with_structure_and_bridge_notices`, `a_source_that_returns_minus_one_fails_that_file_only`,
  `the_engine_closes_every_fd_it_was_given` (count open fds in `/proc/self/fd` before/after), `too_many_files_is_an_error`. RED; implement; commit (regenerate bindings in Task 9).

### Task 9: phone UI for folders (Kotlin)

**Files:** `FolderPicker.kt` (SAF walk), `FolderSender.kt`, `MainActivity.kt` (button "Send a folder"), `FileText.kt` (+ JUnit). **Interfaces:** `FolderWalk.plan(list: (parent) -> List<Child>)` pure:
bounded depth-first listing of `Child(name, isDir, size, id)` into `Planned(rel, name, size, id)` with `MAX_FILES=1000`, `MAX_DEPTH=16`, `skipped` count, `TooBig` when exceeded;
`FolderSender.send(ctx, treeUri)` (worker thread): walk with `DocumentsContract.buildChildDocumentsUriUsingTree` queries (`COLUMN_DOCUMENT_ID`, `COLUMN_DISPLAY_NAME`,
`COLUMN_MIME_TYPE`, `COLUMN_SIZE`; virtual documents skipped), build `FolderFile`s, a `FolderSource` that does `openFileDescriptor(documentUri,"r").detachFd()`, one toast at the end.

- [ ] JUnit for `FolderWalk` (nesting, limits, skipped, hostile names via `FileText.safeName`), then Kotlin; `build-core.sh` for bindings; compile; commit.

### Task 10: debug harness and emulator run

**Files:** `android/app/src/debug/**` (manifest overlay, `DebugShareManyActivity`, `DebugPipeProvider`, `DebugTreeProvider` (a `DocumentsProvider` with a nested test tree), `DebugSendFolderActivity`),
`docs/superpowers/emulator-files-checklist.md`.

- [ ] Run on the emulator (Android 14) and record each result (pass or fail): SEND_MULTIPLE with 40 files; a pipe descriptor (cache copy path); the debug tree sent as a folder (structure, bytes, one notice);
  resume of a phone-sent file after a sender cancel; a nearly full disk (`fallocate`): the engine's "not enough free space" refusal; storage left clean. Fix what it finds with tests first. Commit.

### Task 11: Android 8-9 wording

**Files:** `FileReceiver.kt`, `FileText.kt` (+ JUnit), README. API < 29 notice text: "Saved in Klip's own folder (Android 8-9 cannot show it in Downloads)"; JUnit for the text.

### Task 12: finish v0.10.0-alpha

- [ ] Whole-branch read-only Sonnet review, one fix pass (tests first), fmt, clippy `-D warnings`, `cargo test --workspace`, memory test alone, `cargo audit` via CI, version 0.10.0 (Android code 23) + snapshots,
  README/roadmap/notes with the NOT-verified list, PR, CI green on three OSes, fast-forward `main`, tag, release notes, install the binary, restart daemon/tray, memory file, delete the ledger.

## Self-Review

- Spec coverage: items 1-10 map to Tasks 7-9 (1), 4 (2), 5 (3), 1 (4), 2 (5), 3 (6), 4 (7), 6 (8), 10 (9), 11 (10); settled calls 1-2 map to Tasks 1 and 5.
- Names used across tasks (`CANCEL_CODE`, `ExternalEntry`, `Opener`, `FolderSource`, `FolderFile`, `FolderWalk`) are defined once.
