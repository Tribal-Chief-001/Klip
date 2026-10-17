# File transfer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (native execution: the tasks share
> interfaces, and one heavy build or test runs at a time on this machine). Steps use checkbox (`- [ ]`) syntax.

**Goal:** send a file from one paired device to another (laptop <-> laptop first, then Android), streamed, verified,
bounded, without touching the clip protocol.

**Architecture:** a new ALPN `klip/file/1`, one QUIC connection per transfer, one bidirectional stream carrying a header,
a reply, the raw bytes, a BLAKE3 trailer and a final verdict. The protocol is generic over `AsyncRead`/`AsyncWrite` in
`klip-net`; the receive policy, naming, free space, partial files and the transfer registry live in `klip-core`; the daemon
adds three appended IPC variants; the CLI two commands; Android a bridge, a share target and MediaStore saving.

**Tech Stack:** Rust (tokio, iroh/quinn streams, blake3, postcard), libc/windows-sys for free space, UniFFI, Kotlin.

**Spec:** `docs/superpowers/specs/2026-10-17-file-transfer-design.md`

## Global Constraints
- The clip protocol (`klip/sync/1`) and all existing IPC variants keep their bytes (golden tests stay green); new IPC variants
  are appended at the end, protocol version stays 2, capability string `file_transfer` in `Hello`.
- Linux binary < 9,961,472 bytes (CI budget); the feature should add < 400 KB. No new network port. No new crate unless the tree
  cannot do it (free space: `libc::statvfs` on Unix, `windows-sys` on Windows).
- Received files: never overwrite, never outside the receive folder, mode `0600` on Unix, `create_new`, names sanitised on every OS.
- No file name in a notification or a log line at info level.
- Memory per transfer is constant (256 KiB chunk buffers); limits: 4 GiB default max file, free space = size + 512 MiB (Android:
  2 x size + reserve), 3 incoming at once, 2 per device, 30 s idle timeout.
- One heavy build/test at a time; `cargo ... -j2`; `--test-threads=2`. CI (macOS and Windows jobs) runs the same Rust tests.

## Review Focus
1. A header or data frame that lies (size huge or smaller than the data, a 100 MB frame, random bytes): refused before any big allocation, nothing left on disk (Task 1, 3 tests).
2. Hostile names (`../../x`, `C:\x`, `CON`, trailing dots, RTL override, 5000 characters, only dots): contained in the receive folder (Task 2 table test).
3. Two identical names at once and a file that appears between "check" and "create": no overwrite (Task 2 race test).
4. Disk fills or the free-space figure is wrong mid-way: the write error ends the transfer and the partial file is removed (Task 3 test with a failing sink).
5. Unpair, pause, or shutdown during a transfer: streams are reset, partial files removed, the registry says why (Task 4 test).
6. A new CLI against an old daemon, and a new daemon against a device without the ALPN: a clear message, no hang (Task 5/6 tests).
7. Clipboard sync while a big file is moving: clips still arrive (Task 7 test).

## File Structure
- `crates/klip-net/src/file.rs` (create): `FILE_ALPN`, `FileHeader`, `FileReply`, `FileDone`, framing, `send_file`, `IncomingFile`.
- `crates/klip-net/src/node.rs` (modify): register the ALPN, `open_file_stream`, accept arm, `NetEvent::File`, `NetError::FileUnsupported`.
- `crates/klip-core/src/files.rs` (create): `FilePolicy`, `sanitize_file_name`, `unique_path`, `free_space`, `PartialFile`, `TransferRegistry`, `TransferInfo`.
- `crates/klip-core/src/config.rs`, `sync.rs`, `engine.rs` (modify): config keys, `SyncCoordinator::send_file`, incoming handler, unpair hook.
- `crates/klip-proto/src/ipc.rs` (modify): `SendFile`, `GetTransfers`, `CancelTransfer`, `TransfersStarted`, `Transfers`, `TransferCancelled`, `TransferInfo`.
- `crates/klip-daemon/src/ipc_server.rs`, `notify.rs` (modify). `crates/klip-cli/src/main.rs`, `client.rs` (modify): commands.
- Tests: `crates/klip-net/src/file.rs` (unit), `crates/klip-core/src/files.rs` (unit), `crates/klip-daemon/tests/files_e2e.rs` (two daemons), `crates/klip-cli/tests/cli_files.rs`.
- Android (Task 9+): `crates/klip-android/src/lib.rs`, `android/app/src/main/java/com/klip/app/{FileSender,FileReceiver,ShareFileActivity}.kt`, manifest, strings.

---

### Task 1: The protocol (`klip-net/src/file.rs`)

**Interfaces (produces):**
```rust
pub const FILE_ALPN: &[u8] = b"klip/file/1";
pub const MAX_FILE_FRAME: usize = 2048;
pub const CHUNK: usize = 256 * 1024;
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
pub const DONE_TIMEOUT: Duration = Duration::from_secs(60);
pub const MAX_NAME_CHARS: usize = 255;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileHeader { pub version: u8, pub id: [u8; 16], pub name: String, pub size: u64 }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RejectCode { Disabled, TooLarge, NoSpace, Busy, Paused, Other }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileReply { Accept, Reject { code: RejectCode, text: String } }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileDone { Ok, Bad { text: String } }

#[derive(Debug)]
pub enum FileError { Io(String), Protocol(String), Rejected { code: RejectCode, text: String }, TooLong, TooShort,
                     HashMismatch, Timeout, Peer(String) }

pub async fn send_file<S, W, R>(header: &FileHeader, source: S, tx: &mut W, rx: &mut R,
                                progress: &mut (dyn FnMut(u64) + Send)) -> Result<(), FileError>
    where S: AsyncRead + Unpin, W: AsyncWrite + Unpin, R: AsyncRead + Unpin;

pub struct IncomingFile<R, W> { /* header, rx, tx */ }
impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> IncomingFile<R, W> {
    pub async fn open(rx: R, tx: W) -> Result<Self, FileError>;            // reads and checks the header frame
    pub fn header(&self) -> &FileHeader;
    pub async fn reject(self, code: RejectCode, text: &str);
    pub async fn accept<K: AsyncWrite + Unpin>(self, sink: &mut K, progress: &mut (dyn FnMut(u64) + Send))
        -> Result<Received<W>, FileError>;                                     // Accept, stream, count, hash-check
}
pub struct Received<W> { pub bytes: u64, /* tx */ }
impl<W: AsyncWrite + Unpin> Received<W> { pub async fn ok(self); pub async fn bad(self, text: &str); }
```
Header rules checked by `open`: `version == 1`, name non-empty and <= 255 chars, no NUL; frames are `u32` big-endian length then postcard, length <= `MAX_FILE_FRAME` (checked before reading the body).

- [ ] **Step 1: write the failing tests** (all in a `#[cfg(test)] mod tests` of `file.rs`, using `tokio::io::duplex`): `a_file_goes_through_with_its_hash_checked` (a 700 KB patterned buffer: sender and receiver run as two tasks over two duplex pairs; the sink gets identical bytes; progress ends at the size; `ok()` makes `send_file` return `Ok`); `zero_bytes_and_exactly_one_chunk_work`; `a_flipped_bit_is_caught_by_the_hash` (the middle stream flips one byte: `accept` returns `HashMismatch`, the sender gets `Bad`); `data_shorter_than_the_header_says_is_an_error_not_a_hang` (source returns EOF early -> `TooShort`); `a_source_that_has_more_than_size_bytes_is_refused` (`TooLong`, nothing past `size` is sent); `a_reject_reaches_the_sender_with_its_code` (`FileError::Rejected{code: NoSpace,..}`); `a_silent_peer_times_out` (tokio `time::pause`, 31 s without a byte -> `Timeout`); `a_frame_over_the_cap_is_refused_before_it_is_read` (length prefix 100 MB: `open` returns `Protocol` and never allocates it); `random_bytes_never_panic_as_a_header` (20,000 seeded random buffers through `open`); `a_header_with_a_bad_version_or_name_is_refused`.
- [ ] **Step 2: run** `cargo test -p klip-net --lib file` -> compile error (module missing). Expected: FAIL.
- [ ] **Step 3: implement** `file.rs` (frame helpers `write_frame`/`read_frame` with the size check first; `send_file` reads `source` in `CHUNK`s through `timeout(IDLE_TIMEOUT, ..)`, hashes with `blake3::Hasher`, writes, counts, checks exact length by trying one more read after `size`; trailer; `tx.shutdown()`; reads `FileDone` under `DONE_TIMEOUT`; `accept` writes `Accept`, loops `min(CHUNK, size - got)` reads under `IDLE_TIMEOUT`, writes to the sink, hashes, reads the 32-byte trailer, compares in constant time, and returns `Received`). Register `pub mod file;` in `lib.rs`; add `blake3` to `klip-net` (already in the tree through `klip-proto`).
- [ ] **Step 4: run** the same command. Expected: all pass. Then `cargo clippy -p klip-net --all-targets -D warnings`.
- [ ] **Step 5: commit** "feat(net): the file transfer protocol over any stream".

### Task 2: Names, free space, partial files (`klip-core/src/files.rs`, part 1)

**Interfaces:** `pub fn sanitize_file_name(raw: &str) -> String`; `pub fn unique_path(dir: &Path, name: &str) -> io::Result<(File, PathBuf)>`
(creates with `create_new`, mode `0600`, retries `name (2).ext` ...); `pub fn free_space(dir: &Path) -> io::Result<u64>`;
`pub fn default_receive_dir() -> PathBuf`; `pub struct PartialFile { file, path, final_dir, id }` with `PartialFile::create(dir, id)`, `commit(self, wanted_name) -> io::Result<PathBuf>` (fsync, then `link`+remove or `rename` to a unique name without ever replacing), `discard(self)`; `pub fn sweep_partials(dir: &Path, older_than: Duration)`.

- [ ] **Step 1: tests:** `hostile_names_stay_inside_the_folder` (table: `../../etc/passwd` -> `passwd`, `C:\Windows\x.exe` -> `x.exe`, `CON` -> `_CON`, `con.txt` -> `_con.txt`, `NUL.` -> `_NUL`, `..` -> `file`, `...` -> `file`, `.bashrc` -> `bashrc`, `a:b*c?.txt` -> `a_b_c_.txt`, `name\u{202e}gnp.exe` -> control/format characters removed, a 5000-character name -> <= 200 bytes keeping `.pdf`, `` -> `file`, `report.pdf ` -> `report.pdf`); `an_existing_name_gets_a_number_and_is_never_replaced` (create `a.txt` three times: `a.txt`, `a (2).txt`, `a (3).txt`, contents untouched); `two_threads_creating_the_same_name_never_clash` (16 threads); `the_file_is_private_and_not_executable` (mode `0o600` on Unix); `a_partial_file_is_removed_on_discard_and_renamed_on_commit` and `sweep_removes_only_old_klip_partials` (leaves `keep.part`, `.klip-zz.part` younger than the limit, a directory named like a partial); `free_space_is_positive_for_a_temp_dir_and_an_error_for_a_missing_one`; `a_symlink_where_the_folder_should_be_is_refused`.
- [ ] **Step 2: run** `cargo test -p klip-core --lib files` -> FAIL (module missing).
- [ ] **Step 3: implement** (sanitising order: last component after `/` and `\`; drop control (`char::is_control`) and format characters (`U+200B..U+200F`, `U+202A..U+202E`, `U+2066..U+2069`, `U+FEFF`), replace ``:*?"<>|`` and `\` with `_`; strip leading dots and trailing dots/spaces; reserved device names (case-insensitive, with or without extension) get a leading `_`; empty -> `file`; cut to 200 bytes at a char boundary keeping the extension).
- [ ] **Step 4: run** -> pass; `cargo clippy -p klip-core --all-targets -D warnings`.
- [ ] **Step 5: commit** "feat(core): safe names, unique paths, free space, partial files".

### Task 3: Policy, receiving and sending, the registry (`files.rs`, part 2)

**Interfaces:**
```rust
pub struct FilePolicy { pub accept: bool, pub dir: PathBuf, pub max_bytes: u64, pub reserve_bytes: u64, pub space_factor: u64,
                        pub max_incoming: usize, pub max_incoming_per_peer: usize }
pub enum Direction { Sending, Receiving }
pub enum TransferState { Waiting, Active, Done, Failed, Cancelled }
pub struct TransferInfo { pub id: [u8;16], pub peer: String, pub direction: Direction, pub name: String, pub size: u64,
                          pub done: u64, pub state: TransferState, pub error: Option<String>, pub saved_to: Option<String>,
                          pub started_ms: u64, pub finished_ms: Option<u64> }
pub struct TransferRegistry { /* Mutex<Vec>, keeps the last 50 finished */ }
impl TransferRegistry { pub fn new() -> Self; pub fn add(&self, TransferInfo); pub fn progress(&self, id, done: u64);
    pub fn finish(&self, id, TransferState, Option<String> error, Option<String> saved); pub fn list(&self) -> Vec<TransferInfo>;
    pub fn cancel_token(&self, id) -> Option<CancelToken>; pub fn cancel(&self, id) -> bool; pub fn cancel_peer(&self, peer: &str);
    pub fn incoming_count(&self, peer: Option<&str>) -> usize }
pub async fn receive_incoming<R, W>(incoming: IncomingFile<R, W>, peer: &str, policy: &FilePolicy, reg: &TransferRegistry)
    -> Result<PathBuf, FileError>
pub async fn send_path(path: &Path, header: FileHeader /* name from the path */, tx, rx, peer: &str, reg) -> Result<(), FileError>
```
`receive_incoming` order: refuse when `!accept` (Disabled), `size > max_bytes` (TooLarge), concurrency (Busy), `free_space(dir) < size*space_factor + reserve` (NoSpace), create folder (refuse a symlink), `PartialFile::create`, `accept(sink)`, then `commit` (rename) and only then `received.ok()`; any failure -> `discard` and `received.bad(text)` or the registry says why.

- [ ] **Step 1: tests** (in-memory streams, a temp dir, a `FilePolicy` with small limits): `a_file_is_received_into_a_unique_safe_name`; `accept_off_refuses_with_disabled`; `a_file_over_the_limit_is_refused_before_a_byte_is_read`; `no_space_is_refused` (reserve larger than the free space); `the_fourth_incoming_is_busy_and_a_third_from_one_peer_too`; `a_failing_sink_removes_the_partial_file` (a sink that errors after 100 KB, e.g. a full disk); `a_hash_mismatch_leaves_nothing_behind`; `cancel_resets_the_stream_and_removes_the_partial`; `the_registry_keeps_the_last_50_finished_and_reports_states`; `send_path_refuses_a_directory_and_a_missing_file_and_a_file_that_grows` (fstat after open: regular files only; size changes during the send fail the transfer).
- [ ] **Step 2: run** -> FAIL. **Step 3: implement.** **Step 4: run** -> pass, clippy.
- [ ] **Step 5: commit** "feat(core): receiving and sending files with limits and a transfer registry".

### Task 4: Network node and coordinator

**Interfaces:** `KlipNode::open_file_stream(&self, id: EndpointId) -> Result<(Connection, SendStream, RecvStream), NetError>`
(dials `FILE_ALPN`; on failure while the clip connection to that peer is up -> `NetError::FileUnsupported`);
`NetEvent::File { from: EndpointId, tx: SendStream, rx: RecvStream }` (only for paired peers; at most 8 handlers at once);
`SyncCoordinator::send_file(&self, path: PathBuf, to: Option<String>) -> Vec<Result<TransferInfo, String>>` (per target device,
sequential per device via a per-peer async mutex), `SyncCoordinator::transfers() -> Vec<TransferInfo>`,
`cancel_transfer(id) -> bool`; unpairing a device calls `registry.cancel_peer(name)`; `DaemonConfig` gains `accept_files: bool`
(default true), `receive_dir: Option<String>`, `max_file_bytes: u64` (default 4 GiB, validated 1 MiB..=64 GiB).

- [ ] **Step 1: tests:** `klip-net` integration (two nodes on loopback, like the existing node tests): `a_paired_peer_can_open_a_file_stream_and_a_stranger_cannot`; `a_peer_without_the_alpn_is_reported_as_unsupported` (a node built with an alpn list without `FILE_ALPN`, test-only constructor flag); `klip-core`: `config_defaults_and_validation_for_the_new_keys`, `unpairing_cancels_that_devices_transfers`.
- [ ] **Step 2: run** -> FAIL. **Step 3: implement.** **Step 4: run** the klip-net and klip-core suites -> pass, clippy.
- [ ] **Step 5: commit** "feat(net,core): file streams between paired devices and the coordinator that drives them".

### Task 5: IPC and daemon

**Interfaces (appended after the current last variants):** `IpcRequest::SendFile { path: String, to: Option<String> }`,
`GetTransfers`, `CancelTransfer { id: [u8;16] }`; `IpcResponse::TransfersStarted(Vec<TransferInfo>)`, `Transfers(Vec<TransferInfo>)`,
`TransferCancelled(bool)`; `CAP_FILE_TRANSFER = "file_transfer"` in `Hello`; the daemon refuses a relative `path`.

- [ ] **Step 1: tests:** golden bytes for the six new variants appended to `ipc.rs`'s test (the old goldens unchanged and still pass); daemon e2e: `send_file_with_a_relative_path_is_refused`, `get_transfers_is_empty_at_first`, `hello_lists_the_file_transfer_capability`.
- [ ] **Step 2: run** -> FAIL. **Step 3: implement** handlers in `ipc_server.rs` (stat the file first so a missing file answers at once; start sends as tasks and answer with the `TransferInfo` list); Linux completion notification through `notify::say("Received 1 file from <device> (<size>)")` (no name) via a core hook like `UndeliveredHook`.
- [ ] **Step 4: run** `cargo test -p klip-proto -p klip-daemon`. **Step 5: commit.**

### Task 6: CLI

**Interfaces:** `klip send-file FILE... [--to NAME] [--no-wait]`, `klip transfers [--cancel ID]`; the CLI makes paths absolute
(`std::fs::canonicalize`), checks the `file_transfer` capability (`Hello`) and says "restart Klip: the running one is older" if
missing; progress lines `name  45%  12.3 MiB/s` on a terminal, plain lines when piped; exit code non-zero when any file failed.

- [ ] **Step 1: tests (`crates/klip-cli/tests/cli_files.rs`, two isolated daemons paired through the existing helper pattern of
  `tui_daemon.rs`):** `send_file_delivers_a_file_with_equal_hash`, `send_file_to_a_missing_file_fails_clearly`, `transfers_lists_the_finished_transfer`,
  `send_file_with_accept_off_on_the_other_side_says_why`, `cancel_stops_a_running_transfer` (a 200 MB sparse-ish file), golden help texts for the two commands (`UPDATE_GOLDEN=1` once, reviewed).
- [ ] **Step 2: run** -> FAIL. **Step 3: implement.** **Step 4: run** -> pass. **Step 5: commit.**

### Task 7: End to end, hardening, CI

- [ ] **Step 1: `crates/klip-daemon/tests/files_e2e.rs` (two in-process engines like `e2e.rs`):** a 64 MB file both ways with equal BLAKE3; 0 bytes; a unicode name; two files at once (different peers' folders); a hostile name from a raw stream crafted by the test (the received file lands inside the folder with a safe name); a lying header (size 10 GB, then 5 bytes) leaves no file; unpair during a transfer; clips still sync while a 200 MB file moves (a clip sent mid-transfer arrives within 5 s); memory (`/proc/self/status` VmRSS grows < 40 MB during a 256 MB transfer, Linux only).
- [ ] **Step 2: run, fix, run the whole workspace** (`cargo test --workspace`), clippy, fmt, audit.
- [ ] **Step 3: CI:** the macOS and Windows `platform-tests` jobs run `cargo test -p klip-net -p klip-core --lib` plus `cli_files`; push the branch and make them green.
- [ ] **Step 4: one fresh whole-branch review (Sonnet, read-only)**, one fix pass, ledger.

### Task 8: Docs and the desktop release (v0.8.0-alpha)

- [ ] README (commands, limits, the receive folder, what is not done), roadmap paragraph, version 0.8.0 (Cargo, Android 0.8.0-alpha / 20, TUI snapshots with `--features testkit`), merge to `main`, build, tag, install, release notes with NOT-verified list, memory file.

### Task 9: Android engine API (`klip-android`)

**Interfaces:** `KlipEngine::start(.., incoming_dir: String, accept_files: bool, max_file_bytes: u64)`; `send_file_fd(fd: i32, name: String, size: u64) -> Result<Vec<TransferView>, KlipError>`;
`transfers() -> Vec<TransferView>`; `cancel_transfer(id: Vec<u8>) -> bool`; `ClipboardBridge::file_received(path: String, name: String, size: u64, from: String)`.
`send_file_fd` takes ownership of the descriptor (`File::from_raw_fd`, with `// SAFETY:` and a check that it is a regular file by fstat).

- [ ] Steps: tests with two in-process engines (`crates/klip-android/tests`, a pipe/temp file as the fd), a bridge that records `file_received`; the free-space factor is 2 on Android; implement; run `cargo test -p klip-android`; commit.

### Task 10: Android app (`FileReceiver`, `FileSender`, `ShareFileActivity`)

- [ ] `FileReceiver.onFileReceived`: copy from `filesDir/incoming` into `MediaStore.Downloads` (API 29+, `RELATIVE_PATH = Download/Klip`, MIME from `MimeTypeMap` by extension, `IS_PENDING` while writing), verify the size, delete the staged file only after the verified copy, post a notification (no change when the copy fails: it says the file stays in the app's storage); API < 29: write to `getExternalFilesDir(DIRECTORY_DOWNLOADS)`.
- [ ] `ShareFileActivity` (`SEND`, `SEND_MULTIPLE`, `*/*`, label "Send as a file with Klip"): `openFileDescriptor(uri, "r")`, `detachFd()`, size from `OpenableColumns.SIZE` or `statSize`, name from `DISPLAY_NAME`; calls `send_file_fd` on a worker thread; progress notification polled from `transfers()`; wake lock and Wi-Fi lock while active; toast naming the devices.
- [ ] Settings: switch "Receive files" and the cap, passed to `start`.
- [ ] Emulator (x86_64 debug build, `build-core.sh x86_64`, `gradlew assembleDebug -Pklip.abis=x86_64`, then rebuild the ARM libs): the test peer (`emulator_peer` example, PEER_MANUAL) sends a file to the emulator and receives one from it; a 300 MB file; check the notification, the MediaStore entry and the memory of the app process (`dumpsys meminfo`).
- [ ] Commit; build the release APKs; version 0.8.1-alpha; merge, tag, release notes with what the emulator did and did not show.

## Self-Review
- Spec coverage: goals 1-2 -> Tasks 6, 10; 3 -> Tasks 1, 3, 7 (memory test); 4 -> Tasks 2, 3, 4, 7; 5 -> Tasks 4, 6. Rulings: ALPN/connection per transfer (1, 4), trailer protocol (1), no resume + partial cleanup (2, 3), limits (3), names (2), notifications without names (5), `SendFile` reads the path itself (3, 5), IPC variants + capability (5), CLI (6), Android (9, 10), layering (File Structure), two-step shipping (8, 10).
- No placeholders: the unit-test lists name every case; the Android emulator steps name the commands and what to observe.
- Types: `FileHeader`, `FileReply`, `FileDone`, `RejectCode`, `FileError`, `IncomingFile`, `Received`, `FilePolicy`, `TransferInfo`, `TransferRegistry`, `send_file`, `send_path`, `receive_incoming`, `open_file_stream`, `NetEvent::File`, `SendFile/GetTransfers/CancelTransfer` are used with the same names in every task.
