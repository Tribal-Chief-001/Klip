# File transfer between paired devices

Status: written 2026-10-17. The owner asked for "file transfer, fully end to end, production hardened, zero
regression, zero errors, the same process as before, without asking for permissions". They did not describe
the screens, so every decision below that they would normally make is a **Ruling** (also repeated in the
release notes). The wire protocol of clipboard sync does not change.

## Why
Klip moves text and images between your devices. The next thing people copy between a laptop and a phone is a
file: a PDF, a photo taken a minute ago, an APK, a log. Today that needs a cable, a chat app or a cloud drive.
Both devices are already paired, authenticated and online together, so Klip can carry the file itself.

## Goals
1. `klip send-file report.pdf` on a laptop puts the file in the other device's receive folder, intact (checked
   with a BLAKE3 hash), with progress, and the receiver can tell where it landed.
2. An Android share sheet entry "Send as a file with Klip" sends any file (a content URI) to the laptop, and a file
   sent from the laptop arrives in the phone's Downloads/Klip with a notification.
3. Files of gigabytes work with a constant, small amount of memory (streamed, never loaded whole).
4. A paired but misbehaving or compromised device cannot: write outside the receive folder, overwrite an existing
   file, fill the disk, make a file executable, run out the receiver's memory, or stall clipboard sync.
5. Old devices keep syncing clips exactly as before; talking file transfer to one of them fails with a clear
   message ("update Klip on the other device").

## Non-goals (v1, named so nobody expects them)
Resuming an interrupted transfer (it restarts), sending folders (zip them), a Files screen in the terminal app, tray
menu items for files, an accept/decline prompt per file, thumbnails or previews, opening a file automatically,
queueing a file for a device that is offline (same decision as clips: both devices online together), transfers
between more than the paired set, bandwidth limits.

## Anti-goals (a failure even if it "works")
Overwriting or deleting anything the person already has; a received file that can be executed by double click
because Klip set a mode bit; the receive folder filling the system disk until the machine stops; file names
showing up in notifications or logs (they can say what a person is working on).

## Decisions (all Rulings)
- **Ruling: a new ALPN `klip/file/1`, one QUIC connection used only for files, one bidirectional stream per file.**
  The clip protocol (`klip/sync/1`, one clip per stream) is untouched, so clip sync cannot regress and a file cannot
  head-of-line block a clip. An old device does not list the new ALPN, so the handshake refuses it and we say why.
- **Ruling: the transfer protocol (postcard frames, all bounded):** sender -> receiver `FileHeader { version: 1,
  id: [u8;16], name, size: u64, mime: Option<String> }` (frame <= 2 KiB); receiver -> sender `FileReply`
  (`Accept` or `Reject { code, text }`); then exactly `size` raw bytes in 256 KiB chunks; then the 32-byte BLAKE3 of
  those bytes; then receiver -> sender `FileDone` (`Ok` or `Bad { text }`). A hash trailer, not a leading hash:
  the sender reads the file once. The receiver counts bytes: fewer or more than `size`, a hash mismatch, or no
  byte for 30 s ends the transfer and deletes the partial file.
- **Ruling: no resume in v1.** Resuming safely means re-verifying the prefix on both sides; the restart is simple
  and cannot corrupt. The partial file is a hidden `.klip-<id>.part` in the receive folder, removed on failure and on
  the next start if it is older than a day (only files with exactly that pattern).
- **Ruling: receiving is automatic from paired devices, inside hard limits** (clips work the same way): `accept_files`
  (default true), `receive_dir` (default `<Downloads>/Klip`, created on first use), `max_file_bytes` (default 4 GiB),
  a free-space reserve (file size + 512 MiB must fit, else `NoSpace`), at most 3 incoming transfers at once and 2 per
  device (else `Busy`). A refusal is reported to the sender with a plain reason.
- **Ruling: the received name is data, not a path.** Take the last path component (both `/` and `\`), drop control
  characters, `:*?"<>|`, leading dots and trailing dots/spaces, refuse nothing but replace what is unusable, cut to
  200 UTF-8 bytes on a character boundary keeping the extension, avoid Windows device names (`CON`, `NUL`, `COM1`, ...)
  on every OS, empty -> `file`. A name that exists becomes `name (2).ext`; creation uses `create_new` so a race cannot
  overwrite. Mode `0600`: never executable.
- **Ruling: notifications never contain a file name** ("Received 1 file from Vivo (3.2 MiB)"); Linux uses the existing
  `notify-send` path, other desktops show nothing in v1.
- **Ruling: sending from the daemon reads the file itself** (`SendFile { path, to }` over the owner-only control channel);
  only regular files (after opening, checked with fstat), a file that changes size while sending fails the transfer. To
  every online paired device unless `--to NAME`; one file at a time per device, devices in parallel; an offline device is
  reported, not queued.
- **Ruling: control channel gains three appended request variants and matching responses** (`SendFile`,
  `GetTransfers`, `CancelTransfer`), a capability `file_transfer` in `Hello`, golden bytes pinned; protocol version
  stays 2. `TransferInfo` (id, device, direction, name, size, done, state, error, saved_to) for the last 50 transfers,
  kept in memory only.
- **Ruling: CLI** `klip send-file FILE... [--to NAME] [--no-wait]` (waits and prints progress by default; Ctrl-C only
  detaches, `klip transfers --cancel ID` cancels) and `klip transfers [--cancel ID]`. No terminal-app or tray UI in v1.
- **Ruling: Android** receives into `filesDir/incoming` (a path Rust can write), then Kotlin moves the file into
  `Downloads/Klip` with MediaStore (API 29+; older APIs: the app's external Downloads folder) and posts a notification
  that opens it; sending takes an open file descriptor from the share sheet's URI (`detachFd`) so a multi-GB file is
  never copied to the app's cache. The engine API: `send_file_fd`, `transfers`, `cancel_transfer`, and a callback
  `file_received` on the existing bridge. A progress notification while sending, polled from `transfers()`.
- **Ruling: layering.** `klip-net/src/file.rs`: the protocol, generic over `AsyncRead`/`AsyncWrite` (tested with
  in-memory streams); `klip-net` node: the ALPN, accept path and `open_file_stream`; `klip-core/src/files.rs`: policy,
  naming, free space, partial files, the transfer registry; `klip-daemon`: IPC; `klip-cli`: commands; `klip-android` and the
  app: the bridge.
- **Ruling: ship in two steps.** v0.8.0-alpha: everything on desktops (the three OSes compile and run the tests on CI);
  v0.8.1-alpha: Android. Android devices on v0.8.0 or older simply refuse files with the clear message.

## Constraints
Linux binary stays under the 9.5 MiB CI budget (now 9.46 MB; the feature must cost well under 400 KB). One heavy build
or test at a time on the author's machine. No new network service, no new open port, no new dependency unless the
standard library and the crates already in the tree cannot do it (free space uses `libc::statvfs` / `windows-sys`).

## Errors and edge cases
0-byte file; a name that is only dots or only a Windows device name; names with RTL/override characters (control
and format characters removed); two files with the same name arriving at once; the receive folder missing or
read-only or a file; a symlink where the receive folder should be (refused); the sender dies mid-file; the receiver's
disk fills mid-file (checked per chunk through the write error); the peer is unpaired mid-transfer (the stream is
reset); a header that lies about `size` (huge, or smaller than what follows); a 100 MB header frame; random bytes as a
header; cancel from either side; pause (clip sync paused) does not block files (separate decision: it does not);
the file is changed or deleted while being sent.

## Tests
Protocol over in-memory streams: happy path with the hash checked, 0 bytes, exactly one chunk, a bit flipped in
transit, short and long data, reset mid-way, silence timeout, header fuzz (random bytes never panic, oversized frames
refused before allocation). Naming: a table of hostile names. Core: unique names under a race, partial-file cleanup,
limits (size, free space, concurrency), the registry's states. End to end with two real daemons: a 64 MB file in both
directions with equal hashes, refusal when `accept_files` is off, cancel, an old-style peer without the ALPN. CLI:
`send-file`, `transfers`. Golden bytes for the new IPC variants. CI on macOS and Windows runs the same Rust tests.
Android (step two): the emulator against the test peer, both directions, a multi-hundred-MB file's memory.

## Not verifiable here
Real-world speed over the n0 relay (a relayed path is slower than a direct one), a real phone, Android 15/16,
scoped-storage differences on vendor ROMs, Windows/macOS behaviour on a real desktop.
