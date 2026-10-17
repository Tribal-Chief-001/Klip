# File transfer, round 3: the rest of B, C and F

Date: 2026-10-18. Owner request: "bcf poora krdo same protocol" = finish everything still open under B (files on the phone),
C (resume, folders, tray, review issues) and F (small known issues), with the same process as before (spec, plan, tests
first, independent review, verification before claiming, no questions). Builds on `2026-10-17-files-round2-design.md`
(v0.8.1 and v0.9.0). One release: **v0.10.0-alpha**.

## What is still open (the list this round closes)

1. The phone cannot send a folder.
2. "Send a file..." in the tray exists only on Linux (no macOS or Windows picker).
3. The console window that may flash at Windows login (the Run key starts a console programme).
4. A sender-side cancel leaves the receiver a kept partial for 24 hours (it cannot tell a cancel from a network cut).
5. Kept partials have no cap in bytes.
6. A polling (non-X11) Linux session re-reads an over-limit image on every tick.
7. The Linux picker mangles a file name that holds a newline.
8. A version-1-only device refusing a folder, and version 1 / version 2 mixing in general, has no end-to-end test (no
   switch to make a daemon version 1 only).
9. Not exercised on the emulator: `SEND_MULTIPLE` (many files at once), a descriptor that is a pipe (a cloud provider),
   the phone's storage filling up.
10. Android 8-9: a received file lands in the app's own folder, which the notification cannot open.

Out of this round on purpose (cannot be built or checked here): anything on the owner's real phone, real internet transfers
between two machines, how the tray looks in a real panel, a real macOS or Windows desktop, the exported share activity
(inherent to a share target), the 36-megapixel memory peak, phone → laptop auto-send (a different project).

## Decisions (all mine; the owner asked not to be questioned)

**1. Phone sends a folder.** Main screen gets "Send a folder": `ACTION_OPEN_DOCUMENT_TREE`, then a walk of the tree with
`ContentResolver` queries (no new dependency) into a list of `(rel components, document uri, name, size)`, at most 1000
files and 16 levels (same limits as the computers), virtual documents and unreadable ones skipped and counted. The engine
gets `send_folder(top, files, source)` where `source` is a foreign trait `FolderSource { open_fd(index) -> i32 }`: the
engine asks for each file's descriptor only when its turn comes (descriptors are not held for 1000 files), takes ownership
of the fd Kotlin hands over (`detachFd`), and closes it when done. In `klip-core` the folder job is generalized from "paths
from a walk" to "entries with an opener"; the walk-based path keeps `prepare_send` (type check before opening, so a FIFO
cannot block). Result and rows are exactly those of a computer folder send (one row per device, files in turn, one failing
file does not stop the others, device-level refusals stop the job, resume per file).

**2. Tray pickers on macOS and Windows.** Built from pure command builders and output parsers, unit-tested everywhere:
macOS `osascript` with `choose file with multiple selections allowed` (POSIX paths, one per line, an error number -128
means "cancelled" and gives an empty list); Windows `powershell -NoProfile -STA` with `System.Windows.Forms.OpenFileDialog`
(`Multiselect`, paths one per line, UTF-8). The menu entry appears when the programme exists (always on macOS/Windows).
Marked NOT verified on a real desktop in the release notes.

**3. Windows login without a console flash.** The Run value becomes
`wscript.exe //B //nologo "<%LOCALAPPDATA%>\Klip\start-tray.vbs"` (a GUI programme, no console); the script runs
`"<klip.exe>" tray run` hidden (`WshShell.Run cmd, 0, False`). `install` writes the script next to a Klip-owned folder,
`uninstall` removes it and the Run value, ownership of an existing Run value is recognized for both the old
(`"<klip>" tray start`) and the new form (the script path must be Klip's), the exe path is quoted for VBScript
(quotes doubled; `%`, newline, NUL refused as before). CI already installs, queries and uninstalls the entry on Windows.

**4. Sender cancel is told to the receiver.** A deliberate cancel resets the file stream with an application code
(`CANCELLED = 77`, `SendStream::reset`) before the connection closes; a receiver that reads that reset ends the transfer
as `FileError::CancelledByPeer`: it deletes the kept part and the row says "the other device cancelled". The daemon
stopping (cancel_all) is *not* a deliberate cancel and sends nothing special, so a cut stays resumable. Resume tests that
used a sender cancel as the "cut" now cut with a raw stream whose source fails mid-way (no reset). Old receivers see an
ordinary stream reset and keep their behaviour.

**5. Kept partials are also capped in bytes**: 16 GiB in all (oldest removed first), on top of 8 per device / 32 / 24 h.

**6. Oversize image backoff.** After an over-limit image is seen, image reads are skipped for 2 seconds (text is still
read every tick), so a polling session does one expensive read per 2 s instead of ten per second.

**7. Picker separator.** zenity gets `--separator=\x1f` (the ASCII unit separator, which no file name has in practice) and
the parser splits on it when present, else on newlines (kdialog keeps newline-separated output; its newline-in-name limit
is documented).

**8. A switch for version 1 only.** `config.json` gets `file_protocol_v2` (default `true`): `false` makes this device speak
only `klip/file/1` (troubleshooting, and the way the end-to-end tests make an "old" device). Tests: a version 1 receiver
gets a single file whole (no resume, no folder), refuses a folder with "update Klip on it" before any byte moves, and a
new sender still delivers to it; a version 1 sender (via the same switch) delivers to a version 2 receiver.

**9. A debug-only test harness on the phone.** `android/app/src/debug/` (not in the release APK) holds
`DebugShareManyActivity` (fires `SEND_MULTIPLE` with N URIs it creates) and `DebugPipeProvider` (a content provider whose
files are pipes). The emulator run uses them for SEND_MULTIPLE with 40 files (the 32-waiting back-pressure), the pipe
fallback, and a nearly full disk (`fallocate` in `/data/local/tmp`, expect the engine's "not enough free space"
refusal). Results go into `docs/superpowers/emulator-files-checklist.md`.

**10. Android 8-9.** No permission prompt is added (cannot be tested here). The received file stays in the app's own
downloads folder, and the notification says where ("Saved in Klip's own folder: Android 8-9 cannot show it in Downloads"),
and the README says so.

## Calls settled after the builder check

1. **Which resets mean "cancelled".** Only a stream reset with code 77 does (`FileError::CancelledByPeer`: delete the kept
   part, row says "the other device cancelled"). Any other reset (code 0 from a dropped stream, a sender read error, a slow-peer
   abort) is a *cut* (`FileError::Io`: keep the part). A sender-side cancel is `reset(77)`, a short best-effort wait (200 ms) so
   the frame leaves, then the usual close; if the signal is lost the old behaviour applies (kept 24 h). The daemon stopping
   (`cancel_all`, `is_stopping`) never signals a cancel. The cut in the resume tests is a raw stream whose source fails (plain
   close, no reset). Release notes do not promise anything about old receivers (they may delete the part when the reset
   reaches them first, which is harmless).
2. **Windows login entry.** `install` uses the script form only when `wscript.exe` exists in `%SystemRoot%\System32` and the
   VBScript engine is registered (`HKCR\VBScript`); otherwise the old `"<klip>" tray start` form (pure decision function,
   tested). `run_value_is_klips` recognizes both forms so `uninstall` cleans a v0.9 entry too. No automatic migration:
   `klip tray install` again rewrites it (the README says so).

## Constraints

Same as the earlier rounds: one heavy build or test run at a time (7.4 GB machine, `-j2`), no new dependency that is not in
`Cargo.lock`, IPC variants only appended with golden bytes (this round adds none: the phone-side folder API is UniFFI, the
tray reads existing requests), `TransferInfo` and `ConfigInfo` unchanged, release binary budget 9,961,472 bytes (v0.9.0 is
9,705,208: about 256 KB of room), every new behaviour has a test seen failing first, CI runs only on `main` and PRs
(work on a branch, open a PR), disk is tight (the e2e tests now clear old temp folders; check `df` before big builds).

## Not verifiable here (stated in the notes)

The real phone, a real Mac or Windows PC (pickers, the login entry, the tray), real zenity/kdialog dialogs, real
internet, Android 8-9, the SAF folder picker on a real phone (the emulator has a Files app: the walk is exercised there,
the owner's providers may differ).
