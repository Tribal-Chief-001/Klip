# Android file transfer: what the emulator showed (2026-10-17)

Build: debug APK x86_64 of the `files-round2` branch, Android 14 emulator (API 34, `klip_test`, software GPU, 2 GB), test peer
`crates/klip-daemon/examples/emulator_peer.rs` (own keystore, in-memory clipboard, n0 relay), linked by typing its ticket
into "Paste a ticket". Nothing was tried on the owner's real phone.

| Check | Result |
|---|---|
| Peer sends a 5,000,000-byte file named `holiday photo.bin` | Landed in `/sdcard/Download/Klip/`, SHA-256 equal after `adb pull`; staging folder empty afterwards; one "Received 1 file from EmuPeer (4.8 MiB)" notification (private lock-screen visibility, no file name) |
| Phone shares the same file back (`ACTION_SEND`, MediaStore URI, `ShareFileActivity`) | Arrived at the peer, SHA-256 equal |
| 300 MiB file peer to phone (about 7 minutes over the n0 relay) | Progress notification "Receiving 1 file, 26%..65%" with a bar and a Cancel action; app PSS stayed about 52-60 MB throughout; at the end the staged `.part` was removed after the copy into Downloads, which `cmp`s equal; notification and locks gone |
| Wake lock `klip:transfer` and Wi-Fi lock while moving | First run: **not held** (the `WAKE_LOCK` permission was missing from the manifest; found here, fixed). After the fix both showed in `dumpsys power`/`dumpsys wifi` during the transfer and were released after it |
| Tap Cancel in the notification | Transfer stopped, peer saw "failed: sending stopped by peer", staged partial removed, notification gone, locks released |
| Name `we:ird*na"me ü😀.txt` | Saved as `we_ird_na_me ü😀.txt` |
| "Receive files" switched off in Privacy | The peer was told "this device is not accepting files"; switched on again, files arrive again |
| Privacy screen | Card shows the limit (2.0 GiB), "Kept inside Klip right now: 0 B" |

Not verified: `SEND_MULTIPLE` (adb cannot attach a list of URIs; the code limits to 32 waiting at once), a descriptor that is
a pipe (cloud provider), the phone's storage filling up during the copy, the EXIF mirror operations on a real photo (the
mapping is unit-tested), Android 8-9 (no emulator image), the real phone and its OEM power management.
Seen and not caused by this work: at first start after `pm clear` the watchdog job logs `ForegroundServiceStartNotAllowedException`.

## v0.9.0 run (2026-10-18): folders, resume and the version 2 protocol on the phone

Same emulator and test peer, APK built from the `files-round3` branch (versionName 0.9.0-alpha).

| Check | Result |
|---|---|
| Peer sends a 24-file nested folder `Album` (`Day 1/raw`, `Day 2`) | **First run: 21 of 24 arrived.** Three failed with "the file could not be saved": the app deleted the emptied staging folder after moving each file to Downloads, and the next file of the same send had no folder to be renamed into. Fixed (the app no longer deletes it; the engine recreates a missing folder; unit test). **Second run: all 24 files arrived with the structure under `Download/Klip/Album/...`, three spot-checked files byte-equal, one notification "Received 21 files from EmuPeer" (it counts up during the send), the peer's row ended Done.** |
| Resume on the phone: a 200 MB random file cancelled by the sender at ~48% | The phone kept a 95,662,706-byte partial in its staging folder |
| The same file sent again | The first progress seen was 48% (it continued from the kept part), the row ended Done, `cmp` of the file in Downloads against the original: equal, staging empty of partials |
| Version 2 negotiated with the phone | Yes (folders and resume both need it) |

Not run: SEND_MULTIPLE (adb cannot attach a list), the phone sending a folder (not built), a version-1-only peer, the real phone.

## v0.10.0 run (2026-10-18): the phone sends folders and many files; storage full

Same emulator and test peer; the debug-only harness in `android/app/src/debug/` (not in the release APK) shares files, a
pipe-backed file and a small document tree.

| Check | Result |
|---|---|
| `SEND_MULTIPLE` with 40 files and 2 pipe-backed files (`am start ...DebugShareManyActivity --ei count 40 --ei pipes 2`) | All 42 arrived at the peer, byte for byte (the generator is re-run in Python), no error; the pipe files went through the cache copy |
| The phone sends a document tree as a folder (`DebugSendFolderActivity`: `Trip/` with `a.txt`, `Day 1/b.bin`, `Day 1/raw/c.cr2`, `Day 2/d.jpg` and a virtual "Cloud doc") | **First run crashed the app**: the provider threw `SecurityException` ("not a descendant of root", a bug of the debug provider) and nothing caught it on the worker thread. Fixed: every provider error is now caught and shown as "Klip: cannot read the folder (...)", the app stays up (checked with the broken provider). **Second run**: the folder arrived as `root/` instead of `Trip/`: the display name was read by column position and the provider returns all columns. Fixed (columns by name). **Third run**: `Trip/` with the right structure; three files byte-equal, one (`b.bin`) failed with "the other device stopped answering" because the overloaded emulator stalled for 30 s (the folder went on and finished the others, as designed); **fourth run (`Trip (2)`): all four files byte-equal, the virtual document skipped** |
| Phone storage nearly full (about 250 MB free, `fallocate` in `/data/local/tmp`), the peer sends 190 MB | Refused at once by the phone: "this device does not have enough free space for the file"; nothing staged; space freed again |

Not run: the system's own folder picker UI, a version-1-only peer on the phone, Android 8-9 (no image), the real phone.

After the review fixes (same day): the debug tree was sent again with `d.jpg` served by the provider as a pipe (no size, no
seek): all four files arrived byte-equal, so a pipe-backed file inside a folder goes through the cache copy too.
