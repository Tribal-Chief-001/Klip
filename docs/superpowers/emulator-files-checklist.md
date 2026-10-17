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
