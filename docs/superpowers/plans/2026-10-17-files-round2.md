# File transfer round 2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Phone sends and receives files (v0.8.1-alpha); resume, folders, tray status and the known small issues (v0.9.0-alpha).

**Architecture:** Part 1 adds a descriptor-based send path to `klip-core`, an Android engine API in `klip-android`, and Kotlin
(receiver into `Downloads/Klip`, Share-sheet sender, progress notification). Part 2 adds a second ALPN `klip/file/2`
(resume key, folder placement, `AcceptFrom`), negotiated with `connect_with_opts` so v1 devices are untouched; folders
are one registry row per target device that sends its files in turn.

**Tech Stack:** Rust (iroh 1.x QUIC, postcard, BLAKE3, tokio, UniFFI), Kotlin framework views (no AppCompat), Gradle,
JUnit for pure Kotlin logic, Android emulator (x86_64, AVD `klip_test`).

**Spec:** `docs/superpowers/specs/2026-10-17-files-round2-design.md` (settled calls included), on top of
`2026-10-17-file-transfer-design.md`.

## Global Constraints

- One heavy build/test at a time; cargo `-j2`, tests `--test-threads=2` (or 1 for e2e); Android: one Gradle at a time, `./gradlew --stop` after.
- Binary budget 9,961,472 bytes (v0.8.0: 9,597,240); no new dependency that is not already in `Cargo.lock`.
- `IPC_PROTOCOL_VERSION` stays 2; IPC variants only appended with golden bytes; `TransferInfo` and `ConfigInfo` unchanged.
- File protocol v1 (`klip/file/1`) bytes unchanged; v2 is `klip/file/2`; frames ≤ 2048 bytes; v2 `rel` ≤ 16 components of ≤ 255 chars.
- Partials: v2 with key only, ≥ 1 MiB received, ≤ 8 per device, ≤ 32 total, ≤ 24 h; hash failure, disk error, explicit cancel delete.
- Folder: ≤ 1000 files, ≤ 16 levels, `symlink_metadata`, regular files only, one new top folder per batch, never reused.
- Notification text never contains a file name; MediaStore `Download/Klip`; phone default limit 2 GiB; space factor 2.
- Every behaviour change has a test that was seen failing first; kotlin logic that can be pure is JUnit-tested.
- Releases: v0.8.1-alpha after Part 1 (Task 8), v0.9.0-alpha after Part 2 (Task 17); versions bumped in `Cargo.toml`, `android/app/build.gradle.kts` (versionCode 21 then 22), TUI snapshots (`UPDATE_SNAPSHOTS=1 cargo test -p klip-tui --features testkit --test snapshots`).
- CI runs only on `main`/`platforms` pushes and PRs to `main`: work on a branch, open a PR to get macOS/Windows results.

## Review Focus

1. Shared file with a hostile or odd display name (`/`, `..`, emoji, empty, 400 chars) → sanitized, never a path (Tasks 1, 5).
2. A file that grows or shrinks while being sent from a descriptor → the transfer fails, no damaged file arrives (Task 1).
3. A resume whose kept partial was altered or truncated → ends `Bad`, partial deleted, one automatic retry from zero succeeds (Task 11).
4. A folder with a symlink (to a file, to a directory, loop), a FIFO, names `..`, `CON`, 300-char names, 17 levels, 1001 files (Tasks 12, 13).
5. Phone storage full or the app killed in the middle of the copy into Downloads → staged file stays, notification says so, stale staging cleaned at start (Task 4).

---

## PART 1: the phone (v0.8.1-alpha)

### Task 1: send from an open file (klip-core)

**Files:** Modify `crates/klip-core/src/files.rs`, `crates/klip-core/src/transfers.rs`; Test `crates/klip-daemon/tests/files_e2e.rs`.

**Interfaces:**
- Produces `PreparedFile::from_file(file: std::fs::File, name: &str) -> io::Result<PreparedFile>` (fstat: must be regular, else `io::Error::other("only regular files can be sent")`; name sanitized; `size` from fstat).
- Produces `SyncCoordinator::send_open_file(self: &Arc<Self>, file: std::fs::File, name: String, to: Option<String>) -> Vec<Result<TransferInfo, String>>`. Each target gets `file.try_clone()`; the sends of this one file take turns on a shared `tokio::sync::Mutex` and each seeks to 0 first. `send_file(path, to)` and this share one internal `send_to_targets`.
- `send_prepared` seeks the source to 0 (a no-op for a fresh file).

- [ ] **Step 1: tests** in `files_e2e.rs`: `an_open_file_is_sent_and_arrives_after_the_original_is_closed` (24 MiB, equal bytes, original dropped right after the call returns); `one_open_file_sent_to_two_devices_arrives_intact_at_both` (3 daemons); `a_non_regular_open_file_is_refused` (`/dev/null`); `a_file_that_changes_size_while_sent_fails_and_leaves_nothing` (truncate the original after the call, expect `Failed`, receiver folder empty); `a_hostile_display_name_becomes_a_safe_name` (`../../x`, empty name → `file`, 400 chars → ≤ 255).
- [ ] **Step 2:** run `cargo test -j2 -p klip-daemon --test files_e2e -- --test-threads=1 open_file` → FAIL (no such method).
- [ ] **Step 3:** implement as in Interfaces; the registry/cancel/lock code is reused, not copied.
- [ ] **Step 4:** run the new tests and the 14 existing `files_e2e` tests → all pass. Commit `feat(core): send a file from an open descriptor`.

### Task 2: Android engine API (klip-android)

**Files:** Modify `crates/klip-android/src/lib.rs`; Create `crates/klip-android/tests/files.rs`.

**Interfaces:**
- `KlipEngine::start(data_dir: String, device_name: String, use_relays: bool, bridge: Arc<dyn ClipboardBridge>, accept_files: bool, max_file_bytes: u64) -> Result<Arc<Self>, KlipError>`: sets `receive_dir = <data_dir>/incoming`, `accept_files`, `max_file_bytes`, `file_space_factor = 2`, and installs the received hook.
- `ClipboardBridge::file_received(&self, path: String, name: String, size: u64, from: String, rel_dir: String)` (`rel_dir` is `""` until Task 12). Called through `spawn_blocking`.
- `KlipEngine::send_file_fd(&self, fd: i32, name: String, size: u64) -> Result<Vec<TransferView>, KlipError>` (dups the fd with `BorrowedFd::borrow_raw(fd).try_clone_to_owned()`, `// SAFETY:` the caller keeps `fd` open for the call; refuses a size that differs from fstat by more than "unknown (0)"; every per-device refusal comes back as an `Err` joined, partial success as the started list).
- `KlipEngine::transfers(&self) -> Vec<TransferView>`, `KlipEngine::cancel_transfer(&self, id: Vec<u8>) -> bool`, `TransferView { id: Vec<u8>, peer: String, outgoing: bool, name: String, size: u64, done: u64, state: TransferStateView, error: Option<String> }` with enum `TransferStateView { Waiting, Active, Done, Failed, Cancelled }`.

- [ ] **Step 1: tests** (`tests/files.rs`, two in-process engines paired as `engine.rs` does; extend `MockBridge` with `files: Mutex<Vec<(String,String,u64,String,String)>>`): `a_file_sent_from_a_descriptor_reaches_the_other_engine_and_the_bridge_is_told` (bytes equal, `from` is the sender's name, `rel_dir` empty); `receiving_can_be_switched_off_and_says_why`; `the_size_limit_is_enforced`; `transfers_and_cancel_work_and_an_unknown_id_is_false`; `a_pipe_descriptor_is_refused_with_a_clear_message`.
- [ ] **Step 2:** `cargo test -j2 -p klip-android --test files -- --test-threads=1` → FAIL (compile).
- [ ] **Step 3:** implement. Update the existing `engine.rs` MockBridge/start calls for the new signature.
- [ ] **Step 4:** run `cargo test -j2 -p klip-android -- --test-threads=1` → all pass; `cargo clippy -p klip-android --all-targets` clean. Commit.

### Task 3: Kotlin pure helpers and JUnit

**Files:** Modify `android/app/build.gradle.kts` (`testImplementation("junit:junit:4.13.2")`); Create `android/app/src/main/java/com/klip/app/FileText.kt`, `ExifOrient.kt`, `android/app/src/test/java/com/klip/app/FileTextTest.kt`, `ExifOrientTest.kt`.

**Interfaces:**
- `object FileText { fun received(from: String, size: Long): String` ("Received 1 file from X (3.0 MiB)"); `fun progress(list: List<Row>): String` ("Sending 2 files, 45%" / "Receiving 1 file, 10%" / mixed); `fun mimeFor(name: String): String` (`application/octet-stream` fallback, table for common types); `fun safeName(raw: String?): String` (strip path separators and control chars, empty → "file", ≤ 255 chars); `data class Row(val outgoing: Boolean, val size: Long, val done: Long, val running: Boolean)` }`.
- `object ExifOrient { fun ops(orientation: Int): Ops }` with `data class Ops(val rotateDegrees: Float, val flipHorizontal: Boolean, val flipVertical: Boolean)`; mapping for 1-8: 2 flipH; 3 rot180; 4 flipV; 5 rot90 + flipH; 6 rot90; 7 rot270 + flipH; 8 rot270.

- [ ] **Step 1:** write the two test classes (every orientation 1-8 and an unknown value; sizes at 0, 1023, 1536, 3 GiB; names `../a`, `a/b`, `\u0000x`, empty, 400 chars, emoji; progress with zero sizes and mixed directions).
- [ ] **Step 2:** `cd android && ./gradlew --offline :app:testDebugUnitTest` (drop `--offline` if junit is not cached) → FAIL; **Step 3:** implement; **Step 4:** pass; `./gradlew --stop`. Commit.

### Task 4: receiving on the phone

**Files:** Create `FileReceiver.kt`; Modify `ClipboardWriter.kt` (implements `file_received`), `KlipRuntime.kt` (start params, prefs `receive_files`, `max_file_mb`), `MainActivity.kt` (switch + limit row, kept-partials size), `KlipSyncService.kt` (channel "Files", stale staging cleanup at start), regenerated bindings.

**Interfaces:**
- `FileReceiver.onReceived(ctx, stagedPath: String, name: String, size: Long, from: String, relDir: String)`: API 29+ `MediaStore.Downloads` insert (`RELATIVE_PATH = "Download/Klip" + relDir`, `DISPLAY_NAME` made unique by MediaStore, MIME `FileText.mimeFor`, `IS_PENDING=1` while writing), compare copied size, delete the staged file only after the match, then notify `FileText.received`; API 26-28 copy into `getExternalFilesDir(DIRECTORY_DOWNLOADS)/Klip/<relDir>`. On any failure keep the staged file and notify "Received a file but could not save it to Downloads; it is kept inside Klip".
- Notification channel `klip_files` (IMPORTANCE_DEFAULT), `VISIBILITY_PRIVATE`, tap opens the Downloads app (`ACTION_VIEW_DOWNLOADS`).
- Staging cleanup at service start: remove `filesDir/incoming` files that are not `.klip-*.part` and older than 7 days, and `cache/outgoing` older than 1 day.

- [ ] **Step 1:** run `android/build-core.sh x86_64` (regenerates `uniffi/` bindings); implement; **Step 2:** `./gradlew :app:compileDebugKotlin -Pklip.abis=x86_64` compiles; **Step 3:** the behaviour is proven in Task 7 (emulator). Commit (bindings included).

### Task 5: sending from the phone

**Files:** Create `ShareFileActivity.kt`, `FileSender.kt`, `FileTransferNotifier.kt`; Modify `AndroidManifest.xml` (activity for `SEND`/`SEND_MULTIPLE` `*/*`, label "Send as a file with Klip", `exported`, `noHistory`), `KlipSyncService.kt` (notifier hooks), `res/values/strings.xml`.

**Interfaces:**
- `FileSender.send(ctx, uris: List<Uri>)` (worker thread): per uri `openFileDescriptor(uri,"r")`, name `FileText.safeName(OpenableColumns.DISPLAY_NAME)`, size `OpenableColumns.SIZE` else `statSize`; `engine.sendFileFd(pfd.fd, name, size)`; a non-regular descriptor is copied to `cache/outgoing/<random>/<name>` (≤ `max_file_bytes`, free space checked) and sent by `engine.sendFileFd` on that file; at most 32 waiting at once (poll `transfers()`); one toast at the end naming the devices or the first error.
- `FileTransferNotifier` (inside the service): while any transfer is running, once a second `engine.transfers()` → one progress notification (`FileText.progress`, determinate bar, Cancel action = `cancel_transfer` for every running id), a partial wake lock (30 min, refreshed) and a Wi-Fi lock; released when none is running.

- [ ] **Step 1:** implement; **Step 2:** compile; verified in Task 7. Commit.

### Task 6: the two image issues

**Files:** Modify `ShareImageActivity.kt`, `CopyTrampolineActivity.kt` (notification-button read), `ImageSender.kt`.

- [ ] **Step 1:** `ImageSender.decode` uses `ExifOrient.ops` (rotate then `postScale(-1,1)`/`postScale(1,-1)`); Share activity opens the stream in `onCreate` (permission) and reads on a worker thread. **Step 2:** compile, run `FileTextTest`/`ExifOrientTest`; verified on the emulator (a JPEG with orientation 5 is pushed by `adb`, shared, arrives upright at the test peer). Commit.

### Task 7: emulator peer and emulator verification

**Files:** Modify `crates/klip-daemon/examples/emulator_peer.rs` (receive folder `<work>/files/`, `send-file.txt` containing a path triggers `sync.send_file`, writes `files.log` lines `received <name> <size> <blake3>` / `sent <name> <state>`); Create `docs/superpowers/emulator-files-checklist.md`.

- [ ] **Step 1:** extend the example (tests: `cargo build --example emulator_peer`); **Step 2:** build `build-core.sh x86_64` + `gradlew assembleDebug -Pklip.abis=x86_64`, boot AVD `klip_test` headless, install, grant notifications, pair with the peer (ticket typed with `input text` in 40-char chunks). **Step 3 (record each result in the release notes, pass or fail):** peer → phone 5 MB file lands in `Download/Klip` (`adb shell ls`, hash equal via `adb pull`); 300 MB sparse file (`dumpsys meminfo` of the app during the transfer, staged file removed after the copy); phone → peer via `am start -a android.intent.action.SEND -t application/octet-stream --eu android.intent.extra.STREAM content://…` from `/sdcard/Download`; SEND_MULTIPLE of 40 files (back-pressure, no error); cancel from the notification; receiving switched off → the peer is told; an odd name; the phone's storage nearly full (fill with `fallocate` in `/data/local/tmp`) → staged file kept + message. Then rebuild all ABIs (`build-core.sh`). Commit the checklist with the results.

### Task 8: release v0.8.1-alpha

- [ ] Whole-branch read-only Sonnet review of Part 1 → fix pass; `cargo fmt --check`, `cargo clippy --workspace --all-targets`, workspace tests; version bump (Cargo 0.8.1, Android 0.8.1-alpha code 21), snapshots; README (Android files, settings), roadmap; PR to `main` (CI green on three OSes), fast-forward `main`, tag `v0.8.1-alpha`, release notes with the NOT-verified list (real phone, Vivo OEM behaviour, real Downloads app), install the Linux binary and restart the daemon and tray. Ledger line.

---

## PART 2: desktop (v0.9.0-alpha)

### Task 9: review issues of v0.8.0

**Files:** Modify `crates/klip-core/src/files.rs`, `sync.rs`, `transfers.rs`, `crates/klip-net/src/node.rs`; Test `files.rs` (unit), `files_e2e.rs`.

**Interfaces:** `TransferRegistry::begin*` take the peer's `NodeId` as well; `cancel_peer(&self, node: NodeId)` replaces the name version; `TransferRegistry::cancel(id) -> bool` is true only while the transfer can still stop (the receiver marks `committing` right before the rename and `cancel` answers false after); `PartialFile::commit` retries `rename` up to 5 times at 50/100/200/400/800 ms on `PermissionDenied` (Windows) before failing; `receive_incoming` and `send_file` abort with `FileError::Timeout` when fewer than 1024 bytes moved in any 60 s window; unpair closes that device's file connections (`KlipNode::close_file_connections(id)`).

- [ ] **Step 1: tests** `two_devices_with_one_name_cancel_independently` (registry), `a_cancel_after_the_last_byte_discards_the_file` (paused-time test with a hook before commit), `cancel_answers_false_once_the_file_is_committing`, `a_transfer_crawling_at_one_byte_per_second_is_dropped` (paused time), `commit_retries_a_busy_rename` (unix: simulate with a closure injected via a `commit_with(rename: impl FnMut)` seam), `unpair_closes_open_file_connections` (e2e). **Step 2:** FAIL. **Step 3:** implement. **Step 4:** pass; commit.

### Task 10: protocol v2 and negotiation (klip-net)

**Files:** Modify `crates/klip-net/src/file.rs`, `node.rs`, `lib.rs`; Test `crates/klip-net/tests/file_flow.rs`, file.rs unit tests.

**Interfaces:**
- `FILE_ALPN_V2: &[u8] = b"klip/file/2"`. `FileHeaderV2 { version: u8 /*2*/, id: [u8;16], name: String, size: u64, resume_key: [u8;32], folder: Option<FolderPlace> }`, `FolderPlace { batch: [u8;16], top: String, rel: Vec<String> }`, `FileReplyV2 { Accept, AcceptFrom { offset: u64 }, Reject { code: RejectCode, text: String } }`; `parse_header_v2` checks version, name, `top`, ≤ 16 `rel` components each non-empty, ≤ 255 chars, no NUL.
- `IncomingFile` carries `Option<HeaderV2Extras>`; `IncomingFile::accept_from(offset, sink, progress)` and `send_file_v2(&header, source, offset, tx, rx, progress)` where the trailer is the BLAKE3 over the **whole** file (the sender hashes `[0, offset)` locally, the receiver hashes its partial locally).
- `KlipNode::open_file_stream(id) -> Result<FileStreams, NetError>` now connects with `connect_with_opts(.., FILE_ALPN_V2, ConnectOptions::new().with_additional_alpns(vec![FILE_ALPN.to_vec()]))` and `FileStreams::version() -> u8` (1 or 2) from the negotiated ALPN; `NetEvent::File` gains `version: u8`; the node registers both ALPNs when `file_transfer` is on.

- [ ] **Step 1: tests:** v2 header/reply round trips and rejection of 17 components / empty component / NUL / oversize frame; `v2_resume_from_offset_delivers_the_whole_file_and_checks_the_whole_hash`; `a_wrong_prefix_ends_bad`; `a_v1_receiver_is_used_when_the_peer_only_speaks_v1` (node test with `NetConfig` restricted to v1 via a test-only flag `file_v2: false`); all 4 existing `file_flow` tests still pass. **Step 2:** FAIL; **Step 3:** implement; **Step 4:** pass, `cargo clippy`. Commit.

### Task 11: resume (klip-core)

**Files:** Modify `files.rs`, `transfers.rs`, `sync.rs`; Test `files.rs`, `files_e2e.rs`.

**Interfaces:** `resume_key(path_meta: (&str /*name*/, u64 /*size*/, u128 /*mtime_ns*/), first64k: &[u8]) -> [u8; 32]`; `PartialFile::open_keyed(dir, peer: &str, key: &[u8;32]) -> io::Result<(PartialFile, u64 /*existing length*/)>` (name `.klip-<hex(BLAKE3(peer ‖ key))[..32]>.part`; `AlreadyExists`-style `Busy` when in use through an in-process set); `PartialFile::keep(self)` (flush, drop, remember in a bounded index: 8 per peer, 32 total, ≥ 1 MiB, evicts the oldest); `sweep_partials(dir, 24h)` also trims the index; sender: `SyncCoordinator::send_file` computes the key, a rejected-as-Bad resumed attempt is retried once with a random key; `receive_incoming` keeps the partial on `FileError::{Timeout, Io(peer closed), TooShort}` and daemon stop, deletes it on `HashMismatch`, disk errors and cancel; progress starts at the offset; the space check uses `size - offset`. `PARTIAL_MAX_AGE` returns to 24 h.

- [ ] **Step 1: tests** (`files.rs`): `a_cut_transfer_keeps_a_partial_and_the_next_one_resumes`, `a_resume_with_a_changed_prefix_is_retried_from_zero_once`, `kept_partials_are_bounded_per_peer_and_in_total`, `a_partial_under_one_mib_is_not_kept`, `a_cancelled_or_damaged_transfer_keeps_nothing`, `two_devices_with_the_same_file_do_not_share_a_partial`, `a_partial_in_use_answers_busy`, `resume_key_changes_with_name_size_mtime_and_head`; e2e: `cutting_the_connection_halfway_then_sending_again_finishes_with_equal_bytes_and_sends_fewer_bytes` (the sender's `done` after the retry starts at the offset; use `net.shutdown`-free cut via cancelling the QUIC connection in the test peer), `a_v1_peer_gets_a_full_resend`. **Step 2:** FAIL; **Step 3:** implement; **Step 4:** pass; commit.

### Task 12: folders (klip-core)

**Files:** Create `crates/klip-core/src/folders.rs`; Modify `files.rs` (placement under a folder), `transfers.rs`, `sync.rs`; Test `folders.rs` (unit), `files_e2e.rs`.

**Interfaces:** `folders::walk(root: &Path) -> Result<Vec<FolderEntry>, String>` (`FolderEntry { path: PathBuf, rel: Vec<String>, size: u64 }`; `symlink_metadata`; skips symlinks/special files and says how many were skipped; errors on > 1000 files, > 16 levels, or no regular file at all; names sanitized per component); `BatchBook` (receiver; `(peer, batch)` → chosen top folder; `resolve(peer, batch, top, policy_dir) -> io::Result<PathBuf>` creates `<dir>/<unique top>` with `create_dir`; idle 1 h expiry removes it if empty); `place_in_folder(top_dir, rel) -> io::Result<PathBuf>` creating each component with `create_dir`, refusing an existing non-directory or symlink component. Sender: `SyncCoordinator::send_file` on a directory registers one row per target (`name` = top, `size` = total), sends the entries in turn (`done` aggregate), continues after a failed file, final state `Failed("N of M files failed: <first>")` or `Done`, cancel stops at the current file. A v1 peer: whole job fails with `FileUnsupported` text before any byte moves. `file_received` hook gets `rel_dir`.

- [ ] **Step 1: tests:** `walk` ignores symlinks (file, directory, loop) and FIFOs and reports the count, refuses 1001 files and 17 levels, sanitizes `CON`, `a:b`, 300-char names; `place_in_folder` refuses `..`, an existing file where a directory should be, a symlink component; `BatchBook` isolates two peers with the same batch id and removes an empty abandoned folder after expiry (paused time); e2e: `a_folder_arrives_with_its_structure_in_one_new_folder` (nested dirs, 30 files, equal bytes), `sending_the_same_folder_again_makes_a_second_folder`, `one_unreadable_file_fails_only_that_file`, `cancelling_a_folder_stops_it_between_files`, `a_v1_only_device_refuses_a_folder_before_any_byte`, `a_hostile_rel_path_from_a_raw_stream_stays_inside` (raw v2 stream with `rel = ["..","x"]`). **Step 2-4:** FAIL → implement → pass; commit.

### Task 13: IPC and CLI for folders

**Files:** Modify `crates/klip-daemon/src/ipc_server.rs` (directory accepted; capability `CAP_FILE_FOLDERS = "file_folders"` in `klip-proto`), `crates/klip-cli/src/main.rs`; Test `crates/klip-cli/tests/cli_files.rs`, `crates/klip-proto` (constant), goldens.

- [ ] **Step 1: tests:** `send_file_sends_a_folder_into_one_new_folder` (CLI, two daemons, nested files, equal bytes, output says "N files"), `a_folder_to_an_old_daemon_says_restart` (Hello without the capability), `an_empty_folder_is_refused_with_a_clear_message`, help golden updated (`send-file` help mentions folders). **Step 2-4:** FAIL → implement (CLI prints one line per folder row; progress uses `done/size`) → pass; commit.

### Task 14: the phone in v2 and folders

**Files:** Modify `crates/klip-android/src/lib.rs` (nothing new if the core registers both ALPNs; `rel_dir` is filled), `FileReceiver.kt` (`RELATIVE_PATH = Download/Klip/<relDir>`), `FileText.kt` (`relDir` sanitizing helper + test).

- [ ] **Step 1:** Rust test `a_folder_sent_to_an_android_engine_reaches_the_bridge_with_rel_dirs`; Kotlin JUnit for the helper; **Step 2:** implement; **Step 3:** emulator: laptop-side peer sends a nested folder, `adb shell find /sdcard/Download/Klip`; resume on the phone side (cut the peer mid-file, resend). Commit; record results.

### Task 15: tray

**Files:** Modify `crates/klip-cli/src/tray/model.rs`, `controller.rs`, `linux.rs`, `desktop.rs`, `mod.rs`; Test tray unit tests.

**Interfaces:** `Summary` gains `transfers: Option<String>` (from `GetTransfers`: `FileText`-style line, in Rust `transfer_line(&[TransferInfo]) -> Option<String>`, e.g. "Receiving 2 files, 45%", "Sent report.pdf" is NOT used (no names): "Last file: sent" / "failed"); new `Action::OpenFolder` (`xdg-open`/`open`/`explorer` of the receive folder computed like the daemon) and, Linux only, `Action::SendFile` (`zenity --file-selection --multiple --separator=\n`, else `kdialog --getopenfilename`, else the entry is hidden; chosen paths go to `IpcRequest::SendFile`).

- [ ] **Step 1: tests:** `transfer_line_*` (none, one sending, many receiving, failed, done), menu model contains the folder entry on all OSes and the picker entry only when a picker is found (injected `which`), `open_folder_command_is_built_per_os`, picker output with spaces/newlines/unicode paths is parsed exactly. **Step 2-4** FAIL → implement → pass; the macOS/Windows CI jobs compile it. Commit.

### Task 16: small desktop issues

- [ ] **Tests then fixes**, one commit each: (a) `a_refresh_during_a_poll_is_run_after_it` (controller, paused time); (b) Windows `Run` value replaced/deleted only when it starts with Klip's own command (`autostart.rs`, test with an injected registry trait); (c) `osascript`/`cmd` exit status checked and returned as the tray's problem text (injected runner); (d) pipe server created with `FILE_FLAG_FIRST_PIPE_INSTANCE` (a second server with the same name fails: Windows CI test) and the client opens with `SECURITY_IDENTIFICATION`; (e) Linux oversize image logs once per size and is remembered (`klip-platform`, unit test with a fake source). Document what stays open (console flash, per-tick re-read on polling sessions).

### Task 17: finish v0.9.0-alpha

- [ ] Whole-branch read-only Sonnet review of Part 2 → fix pass; fmt, clippy, workspace tests (one at a time), `cargo audit`; PR to `main`, CI green on Linux/macOS/Windows (macOS runs `cli_files`); version 0.9.0 (Android 0.9.0-alpha code 22), snapshots; README (resume, folders, tray, phone), roadmap; fast-forward `main`, tag `v0.9.0-alpha`, release notes with NOT verified (real phone, real internet, tray look, pickers on macOS/Windows), release binary size check, install and restart daemon and tray, update memory `klip-file-transfer.md`, delete `.superpowers/sdd/2026-10-17-file-transfer` and this round's ledger.

## Self-Review

- Spec coverage: Part 1 goals -> Tasks 1-8; resume -> 10, 11; folders -> 12, 13, 14; tray -> 15; review issues a-d -> 9; small issues -> 6, 16; settled calls 1-6 -> 1 (dup), 11/12 (partial flat, batch book, bounds), 14 (phone v2), 12 (one row), 13 (capability).
- Types: `send_open_file`, `PreparedFile::from_file`, `FileHeaderV2`, `FolderPlace`, `FileReplyV2::AcceptFrom`, `BatchBook`, `file_received(.., rel_dir)`, `TransferView` are used with the same names in every task that mentions them.
- Proportion: interfaces and test names only; bodies are the implementer's.
