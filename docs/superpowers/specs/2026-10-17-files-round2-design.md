# File transfer, round 2: phone, resume, folders, tray, and the small known issues

Date: 2026-10-17. Owner request: "A B C F" from the open-items list, to the same standard as the earlier phases (spec,
plan, tests first, independent review, verification before claiming, no permission questions). Builds on
`2026-10-17-file-transfer-design.md` (v0.8.0-alpha, computers only).

## Why

v0.8.0-alpha sends files between computers. The owner's phone cannot send or receive a file yet, an interrupted file
starts again from zero, a folder cannot be sent, the tray knows nothing of files, and the review of v0.8.0 and earlier
releases left a list of small known issues. This round closes those. Two releases: **v0.8.1-alpha** (the phone, plus the
phone-side small issues) and **v0.9.0-alpha** (resume, folders, tray, desktop hardening, desktop small issues).

## Goals

1. The Android app receives files (they end in `Downloads/Klip`) and sends files from the Share sheet, with progress, a
   cancel button and honest results. Same protocol, same limits and safety as the computers.
2. An interrupted transfer continues where it stopped when the file is sent again (resume), without trusting the
   partial data: the final BLAKE3 over the whole file still decides.
3. `klip send-file FOLDER` sends the folder's files into one new folder on the other device.
4. The tray shows file activity and offers "Open the Klip folder" (and, on Linux, "Send a file…").
5. Fix the known small issues listed under "Small issues".

## Non-goals and anti-goals

- No offline delivery, no queueing for devices that are away (owner decision, unchanged).
- No resume of a transfer in flight by itself: the person (or script) sends the file again; Klip then continues. No
  automatic retry loop.
- No empty folders, no symlinks, no permissions/mtime restored, no file manager integration, no sync of folders.
- A transfer must never replace an existing file, escape the receive folder, or leave a file under its final name
  that did not match its hash. A failure must not leave debris except a *resumable* partial file (see Resume).
- The clip protocol and `IPC_PROTOCOL_VERSION` (2) do not change. Old devices keep working: v1 file transfer between
  them is unchanged.

## Part 1: the phone (v0.8.1-alpha)

**Engine (`klip-android`).** `KlipEngine.start(.., bridge, accept_files: bool, max_file_bytes: u64)`; the receive folder is
`<data_dir>/incoming` (app-private staging), `file_space_factor` 2 (the file exists twice for a moment: staged and in
Downloads). New methods: `send_file_fd(fd: i32, name: String, size: u64) -> Result<Vec<TransferView>, KlipError>`,
`transfers() -> Vec<TransferView>`, `cancel_transfer(id: Vec<u8>) -> bool`. `ClipboardBridge` gets
`file_received(path, name, size, from)`, called from `spawn_blocking` so a slow copy never blocks the engine's two
workers. **My call (changed after the builder check):** the engine does not reopen `/proc/self/fd/<fd>` (a fresh `open()` is
checked against the app's own rights and fails for descriptors another app's provider handed over). It `dup`s the
descriptor it is given (`BorrowedFd::borrow_raw(fd).try_clone_to_owned()`, no `libc`) and sends from that duplicate;
Kotlin may close its own descriptor as soon as `send_file_fd` returns. A duplicate shares the read position with its
original, so the sends of one file to several devices take turns (a shared lock per file; each send seeks to 0 first).
In `klip-core` this is `SyncCoordinator::send_open_file(file, name, to)` next to `send_file(path, to)`; both share one
internal function. A non-regular descriptor (a pipe from a cloud provider) is refused by the engine; Kotlin then copies
the stream into `cache/outgoing/` (bounded by `max_file_bytes` and free space) and sends that path; leftovers older than
a day are removed at start. A share of many files is fed to the engine with back-pressure (at most 32 waiting at a
time), so 100 photos do not hit the 64-waiting cap. `TransferView { id, peer, outgoing, name, size, done, state, error }`.

**App.** `FileReceiver`: copy the staged file into `MediaStore.Downloads` (`Download/Klip`, API 29+, `IS_PENDING` while
writing, MIME from the extension), compare sizes, delete the staged file only after the verified copy; API 26-28 writes
to `getExternalFilesDir(DIRECTORY_DOWNLOADS)/Klip`. If the copy fails the file stays in the staging folder and the
notification says so. Notification text never carries the file name ("Received 1 file from X (3.0 MiB)", private
visibility on the lock screen). `ShareFileActivity` (`SEND`, `SEND_MULTIPLE`, `*/*`, label "Send as a file with
Klip"): name from `OpenableColumns.DISPLAY_NAME`, size from `SIZE` or `statSize`; the work runs off the main thread.
A `FileTransferNotifier` inside `KlipSyncService` polls `transfers()` once a second while any transfer runs: one
progress notification ("Sending 2 files, 45%") with a Cancel action, a partial wake lock and a Wi-Fi lock held only
while something moves. Settings: switch "Receive files" (default on) and the size limit (default 2 GiB on the phone;
**my call:** lower than the computer's 4 GiB because of storage and the doubled space).

**Phone-side small issues fixed here:** the image Share activity and notification button read the image on the main
thread (open in `onCreate`, read on a worker); mirrored EXIF orientations 2, 4, 5, 7 are flipped.

## Part 2: desktop (v0.9.0-alpha)

**Review issues (v0.8.0).** (a) Unpair cancels by device identity, not by name, and closes that device's file
connections. (b) A cancel that arrives while the receiver is already past the last byte is honoured before the rename
(the file is discarded), and `cancel` only answers true when it can still take effect. (c) On Windows the rename into
place retries a few times (a scanner or indexer may hold the placeholder for a moment). (d) A transfer that moves
fewer than 1 KiB per second for 60 seconds is dropped, so one paired device cannot hold receive slots forever.

**Protocol v2 (`klip/file/2`).** A second ALPN; the sender connects with `connect_with_opts` offering v2 first and v1 as an
additional ALPN, and uses the negotiated one, so old receivers get exactly today's v1 behaviour (no resume, no
folders). v2 header = v1 fields plus `resume_key: [u8; 32]` and an optional `folder: Option<FolderPlace>`
(`batch: [u8;16]`, `top: String`, `rel: Vec<String>`); v2 reply adds `AcceptFrom { offset }`. Frame limits stay (2048
bytes; at most 16 `rel` components of at most 255 chars). Everything else (raw bytes, BLAKE3 trailer, verdict) is v1.

**Resume.** `resume_key = BLAKE3(name, size, mtime_ns, BLAKE3 of the first 64 KiB)` chosen by the sender. The receiver
names its partial `.klip-<hex(BLAKE3(peer_node ‖ key))[..32]>.part` and **keeps** it when a transfer fails because the
network or the daemon stopped (not on a hash failure, a disk error or an explicit cancel). When a header with the
same key arrives and a partial of `n ≤ size` bytes exists and is not in use, the receiver answers `AcceptFrom{n}`;
it hashes the existing bytes locally; the sender hashes bytes `[0, n)` locally and sends from `n`; the trailer is
over the whole file, so a wrong prefix ends in `Bad`, the receiver deletes the partial, and the sender retries once
with a fresh key (from zero). Kept partials older than 24 h are removed at start and whenever a transfer starts (the
v0.8.0 sweep age of 1 h goes back to 24 h; it is now the resume window). Disk space is checked for the missing bytes.
A partial in use answers Busy. **My call:** the progress shown starts at the offset.

**Folders.** `klip send-file DIR` (also `IpcRequest::SendFile` with a directory): the daemon walks it with
`symlink_metadata` (no symlink followed, only regular files, no special files), at most 1000 files and 16 levels deep,
and sends each file as a separate v2 transfer with `FolderPlace`. The receiver picks one new top folder per batch
(`<receive_dir>/<top>`, ` (2)`, ` (3)` if taken, created with `create_dir`, never reused), remembers the batch for
1 hour, creates each `rel` component under it with sanitized names and refuses a path whose existing component is
not a plain directory, then uses the normal placeholder/partial/rename in that folder. A v1-only receiver gets the
refusal "update Klip on it" (a folder cannot be sent flat; **my call**). Empty folders are not sent.

**Tray.** The menu (all three OSes, same controller) gets a status line while transfers run ("Receiving 2 files, 45%")
and the last result, and "Open the Klip folder" (`xdg-open` / `open` / `explorer`, the receive folder). **My call:**
"Send a file…" only on Linux, through `zenity` or `kdialog` when one is installed (hidden otherwise); macOS and
Windows pickers are not built because they cannot be seen working here. No new IPC: the tray gets the transfers from
`GetTransfers` and works out the receive folder the same way the daemon does (`receive_dir` from `config.json`, else the
default Downloads/Klip folder), so `ConfigInfo` does not change.

## Small issues fixed in v0.9.0

Tray refresh dropped while a poll runs (queue one more); the Windows Run value is only replaced or deleted when it is
Klip's own; the exit status of `osascript`/`cmd` is checked and a failure shown; the control pipe is created with
`FILE_FLAG_FIRST_PIPE_INSTANCE` (a squatter makes the daemon fail loudly) and the client sets
`SECURITY_IDENTIFICATION`; the Linux oversize-image guard logs once and remembers the image so a polling session does
not re-read it every tick.

## Left out on purpose (stays on the open list)

The console window that may flash at Windows login (a console-subsystem exe started from the Run key; fixing it
needs a different launch mechanism that cannot be checked here); exported share activities (inherent to a share
target); 36-megapixel memory peak; macOS/Windows pickers; phone→laptop auto-send; asking real hardware.

## Calls settled after the builder check (all mine; the owner asked not to be questioned)

1. **Descriptors:** dup, not `/proc/self/fd` (see Part 1).
2. **Folders and restarts.** A resumable partial is always flat in the receive folder (found by key, moved into the
   folder at commit; the sweep stays non-recursive). The batch memory is in RAM only, keyed by `(peer, batch)`, so one
   device cannot write into another's batch (the batch id is 128 random bits chosen by the sender). A folder sent again
   after an interruption arrives as a *new* folder (`Top (2)`); files that already finished are sent again; the
   unfinished file resumes from its partial. A top folder that stays empty is removed when its batch expires (1 h idle)
   and when the first file of a batch fails.
3. **The phone takes part in v2 and folders (v0.9.0).** It offers both ALPNs. `file_received` gets a fifth argument
   `rel_dir` (`""` or `Top/sub`, relative to the staging folder) and `FileReceiver` puts the file under
   `Download/Klip/<rel_dir>` (MediaStore `RELATIVE_PATH`). The phone *sending* folders (a storage-access tree) is not
   in this round.
4. **A folder is one job, not 1000 rows.** The sending daemon registers **one** registry row per target device for the
   folder (`name` = top folder, `size` = total bytes, `done` = bytes so far, `state` as for a file), walks the folder
   once into a list of paths (at most 1000 files, 16 levels), then sends the files one after another, opening each
   only when its turn comes. The row's cancel stops the job at the current file. A file that fails (refused, damaged,
   unreadable) does not stop the others; the row ends `Failed` with "N of M files failed: <first reason>", or `Done`.
   A cancelled folder leaves what had arrived on the other side (stated in the notes). The receiving side has one row
   per file, as today. `SendFile` on a directory answers `TransfersStarted` with the folder rows, no new struct.
5. **Kept partials are bounded.** Only a v2 transfer with a key keeps its partial, only when at least 1 MiB had
   arrived, at most 8 per device and 32 in all (the oldest is removed to make room), for at most 24 h. v1 transfers
   delete their partial on any failure, as today. Kept partials already occupy disk, so the free-space check sees them;
   on the phone the staging folder is app-private and the Settings screen shows how much is kept there.
6. **Old daemons.** `TransferInfo` and the IPC structs do not change: resume offset and folder progress are carried by
   `done`/`size`/`name`. A new capability string `file_folders` (appended in `Hello`, no golden bytes needed) lets the
   CLI say "restart Klip: the running one is older" before a folder is sent to an old daemon.

## Constraints

Rust edition and MSRV as the workspace; one heavy build or test run at a time (7.4 GB machine, `-j2`,
`--test-threads=2`); Android builds one at a time with `./gradlew --stop` after; binary size budget 9,961,472 bytes
(the release job fails above it; v0.8.0 is 9,597,240 so there are about 364 KB left); no new dependency unless it is
already in the tree; new IPC variants only appended with golden bytes; every new behaviour has a test that was seen
failing first.

## Not verifiable here (will be stated in the release notes)

Anything on the owner's real phone; real internet transfers between two machines; real macOS/Windows desktops
(only CI's runners); how the tray additions look; the Linux file picker with a real `zenity`.
