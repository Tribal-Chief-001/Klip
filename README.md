<div align="center">

# 📎 Klip

### Copy on one of your devices, paste on another. End-to-end encrypted, no account, no cloud.

[![CI](https://img.shields.io/github/actions/workflow/status/Tribal-Chief-001/Klip/ci.yml?branch=main&style=flat-square&label=CI)](https://github.com/Tribal-Chief-001/Klip/actions/workflows/ci.yml)
[![Android build](https://img.shields.io/github/actions/workflow/status/Tribal-Chief-001/Klip/android.yml?branch=main&style=flat-square&label=Android%20build)](https://github.com/Tribal-Chief-001/Klip/actions/workflows/android.yml)
[![License](https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-blue?style=flat-square)](LICENSE-MIT)
![Status](https://img.shields.io/badge/status-alpha-orange?style=flat-square)

</div>

> **Alpha software.** The Linux ↔ Android path works and has been tested (details in [Status](#status--what-is-actually-tested)), but it has had **no independent security review**, has not yet run on a physical phone, and macOS/Windows are only compile-checked. Read [Limits & honesty](#limits--honesty) before relying on it.

```text
╭── 📎 Klip ───────────────────────────────────────────────────────────────╮
│                                                                          │
│ ● STATE    Active                                                        │
│ ● SYSTEM   Linux (X11)                                                   │
│ ● NETWORK  n0 public relays + discovery                                  │
│ ● RELAY    relay connected (https://aps1-1.relay.n0.iroh.link./)         │
│ ● DEVICE   VCUU-LBYY-ES2H                                                │
│                                                                          │
│ Linked devices                                                           │
│   └─ 🟢 Pixel 8  [75A6-H66R-SQDW]  connected                            │
│                                                                          │
│ Last sync  4s ago · "https://github.com/…" (38 B)                        │
╰──────────────────────────────────────────────────────────────────────────╯
```

## What it does

- Copy text on your laptop → it is on your phone's clipboard (and the other way round).
- Works **across networks** (phone on mobile data, laptop at home) — and on the same Wi-Fi without internet.
- **No account, no server of ours.** Devices find each other by cryptographic device ID.
- You decide what is linked: pairing needs a one-time QR/ticket **and** your approval on the other device.
- Honest feedback: when you send something, Klip tells you whether the other device really put it on its clipboard, or why not.

## Status — what is actually tested

| Part | State | Evidence |
|---|---|---|
| Core engine (pairing, sync, conflict resolution) | ✅ working | 70 automated tests (real network endpoints, two full daemons, pairing, unpair, pause, bad input, random-input decoding) run in CI; a two-daemon crash/restart chaos script was run by hand (not in CI) |
| **Linux (X11)** daemon + CLI | ✅ tested on a real machine | pairing, clipboard in both directions through the real X11 clipboard, restart, unpair |
| **Android app** | 🟡 tested on an emulator (Android 14) only | pairing through the n0 relay, sync both ways, background clipboard write, restart/reconnect. **Not yet tried on a physical phone**, not the camera QR scan, not the tile |
| Linux Wayland | 🟡 partial | works through XWayland apps only; GNOME needs more work |
| macOS, Windows | 🔴 compile-checked in CI only | never run; the Windows CLI cannot control the daemon yet |
| iOS | ⚪ not started | Apple does not allow background clipboard access |

## How it works

```mermaid
flowchart LR
    A["💻 Laptop<br/>Klip daemon"] <== "QUIC, end-to-end encrypted<br/>direct when possible" ==> B["📱 Phone<br/>Klip app"]
    A -. "only if a direct path fails" .-> R["relay server<br/>(sees metadata, not content)"]
    B -. " " .-> R
```

- Every device has an Ed25519 key; its public half is the device ID (`klip status` shows a short fingerprint like `VCUU-LBYY-ES2H`).
- Connections use QUIC/TLS 1.3 via [iroh](https://iroh.computer), which authenticates **both** sides by device key and tries to connect directly (hole punching); relays are the fallback.
- Only devices you paired are accepted. A clip is accepted only if the sender is the device that claims to have copied it.
- Conflicts: the newest copy wins (hybrid logical clock); clocks too far in the future are rejected.

## Install

### Linux (from source, ~5 min)

You need Rust 1.91 or newer: <https://rustup.rs>

```bash
git clone https://github.com/Tribal-Chief-001/Klip.git
cd Klip
cargo build --release -p klip-cli
install -Dm755 target/release/klip ~/.local/bin/klip     # make sure ~/.local/bin is in your PATH
klip start
klip status
```

Pre-built binaries and the Android APK are attached to the [Releases](https://github.com/Tribal-Chief-001/Klip/releases) page (pre-releases while the project is alpha).

### Android

1. Download **`Klip-Android.apk`** from the latest release (about 10-11 MB; it is for 64-bit phones, i.e. nearly every phone since 2017). Only if your phone is very old and 32-bit, take `Klip-Android-armv7.apk` instead. Open it and allow “install unknown apps” for your browser/file manager.
2. Open **Klip** and tap **Setup** (top right). It lists every permission and phone setting Klip needs to keep running with the screen off and after a restart, explains what each one does and cannot see, and has the steps for your phone brand (Vivo, Xiaomi, Oppo/Realme/OnePlus, Samsung, Huawei/Honor; others need nothing extra). It marks the ones Android lets it check as done.
3. **Privacy** (top right) says in plain words where clips go and what Klip does and never does.

Needs Android 8.0 or newer. Updating from an earlier alpha keeps your pairings (same signing key).

**Size.** The APK was 39 MB (three CPU types in one file, no code shrinking). It is now one APK per CPU type with the Kotlin code shrunk by R8 and without AppCompat/Material: **10.7 MB for arm64** (measured on a local release build). 95% of that is the Rust engine (QUIC, TLS, relay client), about 10 MB; a 2-3 MB app is not possible with this stack. The Android library is built with `panic = "abort"` (saves 2.2 MB); a crash restarts the background service by itself.

## Link your devices (once)

**On the computer**

```bash
klip pair
```

A QR code and a text ticket appear. **Keep this window open.**

**On the phone**

1. Open Klip → **Scan a QR code to link a device** → point it at the QR.
   (No camera? **Paste a ticket instead**.)
2. Back on the computer a question appears: *“<phone> wants to pair [fingerprint] — Approve?”* → type `y`.

Done. Test it: copy something on one device, paste on the other.

The ticket works **once** and expires after 2 minutes. Pairing a second phone? Run `klip pair` again.
Phone is the one showing the code? In the app tap **Show a QR code for another device**, then on the computer run `klip pair <ticket>`.

## Everyday use

| I want to… | Do this |
|---|---|
| Copy on the computer, paste on the phone | Just copy. It arrives on the phone's clipboard by itself. |
| Copy on the phone, paste on the computer | Share → Klip, or tile / notification button (Android can't notice a copy by itself — see [why](#phone--computer-android-limits)). |
| See what happened | `klip status`, `klip history` |
| Send text without copying | `klip send "text"` |
| Stop syncing for a while | `klip pause 600` … `klip resume` |
| Remove a device | `klip unpair "Pixel 8"` (name or start of its fingerprint) |

### Phone → computer (Android limits)

**Why isn't it automatic?** Android 10+ only lets the app *in front* read the clipboard, and — verified on Android 14 — it does not even tell Klip that something was copied while another app is in front. So Klip cannot notice a copy by itself. (Computer → phone is instant because *writing* the clipboard is allowed.) Ways around it, easiest first, none needs special permissions:

1. **Share → Klip** — in any app (browser, chat…) use the normal Share button and pick **Klip**. Best for links.
2. **Select text → ⋮ → “Send to Klip”** in the text-selection menu.
3. **Quick Settings tile** “Klip” (swipe down, edit tiles, add Klip): copy, then tap the tile.
4. **“Send clipboard”** button in Klip's notification.

Both 1 and 2 were tested to reach the computer without opening Klip.

**Experimental automatic mode (not verified).** KDE Connect watches Android's system log for the “clipboard access denied” line and then briefly shows a transparent window to read the clip. Klip has the same mechanism, off by default. It needs a computer once (`adb shell pm grant com.klip.app android.permission.READ_LOGS`) plus “Display over other apps”; the in-app *Automatic copy from this phone (setup)* screen explains it. **I could not verify it**: on the Android 14 emulator the app was not given other apps' log lines even with the permission granted; people report it working on real phones. Treat it as a try-it-yourself option; undo with `adb shell pm revoke com.klip.app android.permission.READ_LOGS`.

Writing *to* the phone's clipboard from the background works without any special permission (tested on Android 14). If some phone brand blocks it, Klip shows a “tap to copy” notification instead of dropping the clip.

### Banking / UPI apps

Out of the box Klip uses **no Accessibility service** (the earlier prototype did, which is what made UPI apps complain) and none of the special permissions; “Display over other apps” and log access are *off* unless you turn them on yourself. I could not test real UPI apps (no Play Store on my test device), so this is unverified. If a payment app complains: make sure “Display over other apps” and (if you did the adb step) `READ_LOGS` are off for Klip, and stop Klip's notification service while paying if needed. Tell me which app and message it shows and I will build a lean variant without the optional permissions.

## Start automatically

| Platform | Status |
|---|---|
| **Linux** | `klip install-service` + `systemctl --user enable --now klip.service` starts it at every login (systemd user unit). Verified: after `kill -9` systemd restarts the daemon in about 3 s and the phone reconnects. A full reboot of the author's machine has not been exercised. |
| **Android** | A foreground service (type *specialUse*, so Android 15's 6-hour limit and boot restrictions for *dataSync* do not apply) restarts after a reboot and reconnects by itself. Verified on an Android 14 emulator: after killing the app process Android restarted the service within 5 s and the laptop saw the device again; after a full emulator reboot, without opening the app, the service came back and the laptop could deliver a clip to it. After a reboot Android only delivers the boot event after the first unlock. **Not verified:** swiping the app out of recents, Android 15/16, and real phones' battery managers (Xiaomi, Oppo, Vivo, Samsung… may kill background apps: allow background activity / auto-start for Klip in the phone's settings). If you force-stop Klip, open it once to start it again (Android's rule). |
| **macOS** | `klip install-service` writes a launchd agent — never run, untested. |
| **Windows** | `klip install-service` writes a Task Scheduler script — never run; the Windows CLI also cannot control the daemon yet. |

```bash
klip install-service
systemctl --user enable --now klip.service
systemctl --user status klip.service
```

## Settings

`~/.config/klip/config.json` (all keys optional; a typo is an error, never silently ignored):

```json
{
  "device_name": "ThinkPad",
  "relay": "n0",
  "lan_discovery": true,
  "max_clip_bytes": 33554432,
  "max_history_items": 50,
  "poll_interval_ms": 100
}
```

- `"relay": "n0"` (default) uses n0's public relay and discovery servers so devices on different networks can find each other. `"relay": "lan"` uses **no third-party servers at all**, but then both devices must be on the same network.
- `lan_discovery` announces the device ID on your local network (mDNS) so same-Wi-Fi devices connect without internet.
- Environment: `KLIP_CONFIG_DIR` (separate instance), `KLIP_CLIPBOARD=memory` (don't touch the OS clipboard — servers/demos).

## Command reference

| Command | What it does |
|---|---|
| `klip start [--foreground]` | start the background service |
| `klip stop` | stop it |
| `klip status` | devices, connection, relay state, last sync |
| `klip pair [--lan] [--ttl 120]` | show QR + ticket and wait for a device |
| `klip pair <ticket>` | join the device that showed a ticket |
| `klip unpair <name\|fingerprint>` | forget a device |
| `klip pause [seconds]` / `klip resume` | pause or resume syncing |
| `klip history [-l N]` | recent clips (secrets are masked) |
| `klip send "<text>"` | send text now, with per-device result |
| `klip install-service` | create the auto-start unit for your OS |

## Limits & honesty

**Security — what you can and cannot expect**

- Clip contents are end-to-end encrypted between your devices. Relays and network observers see **metadata** (that two device IDs talk, when, and how much), never contents.
- With `"relay": "n0"`, your device ID and its network addresses are published to n0's discovery service so peers can find you. Use `"relay": "lan"` if that matters to you.
- Keys are classical (X25519 + Ed25519). **Post-quantum key exchange is not enabled yet** (planned; the library supports it as an option).
- Not independently audited. Do not rely on it for anything where a bug would be dangerous.
- The “secret” detector (patterns + entropy) is a heuristic: it hides previews in history and asks Android to hide keyboard previews. It does **not** stop secrets from syncing and will miss some.
- The keystore (`~/.config/klip/keystore.json`) holds the device key in a private (0600) file; it is not encrypted with a password or the OS keyring yet.
- Anyone who can run commands as you can read your clipboard history through the local control socket. A stolen or lost device stays trusted until you `klip unpair` it from another one.

**Behaviour**

- **Both devices must be online at the same time.** There is no mailbox or push service: a clip copied while the other device is off is not delivered later. (A deliberate choice: no server of ours.)
- Text only on Android for now. The desktop engine understands images, but the phone app does not yet.
- Clips are limited to 32 MiB by default.
- Idle cost (measured on a release build, one linked device, Linux/X11): about **14 MB** resident memory, ~**3 wakeups per second**, no measurable CPU. Capture is event-driven (the X server tells Klip when the clipboard changes); the remaining wakeups are iroh's connection upkeep. Without X11 events (headless / `KLIP_CLIPBOARD=memory`) Klip falls back to polling and uses more.
- Release binary: 7.8 MB (stripped, size-optimised).
- Debug-signed Android builds change signature between CI runs; installing a newer build over an older one may require uninstalling first (re-pair afterwards). A stable release key is planned.

## Troubleshooting

| Symptom | Try |
|---|---|
| `klip status` says *relay unreachable* | no internet / blocked UDP & HTTPS to n0; use `"relay": "lan"` on the same Wi-Fi |
| Devices linked but **not connected** | wait ~10 s (it retries); check both have internet or the same network; `klip status` shows the last error |
| “could not reach any linked device” on `klip send` | other device is off/asleep/offline — see *both devices online* above |
| “the other device could not write its clipboard” | the phone/desktop clipboard was unavailable; try again |
| Pairing: “invalid or expired pairing code” | tickets work once and expire; run `klip pair` again |
| `klip start` says it did not come up | read `~/.config/klip/daemon.log` |
| Nothing syncs on Wayland | Klip currently uses the X11 clipboard (XWayland); GNOME/Wayland needs an extension that is not finished |
| Banking/UPI app complains about Klip | see [Banking / UPI apps](#banking--upi-apps); also remove the old prototype's Accessibility service if it is still enabled |

## Build & test from source

```bash
cargo test --workspace                      # 70 tests
cargo clippy --workspace --all-targets -- -D warnings
android/build-core.sh arm64-v8a             # Rust engine + Kotlin bindings (needs cargo-ndk, NDK r28+)
cd android && ./gradlew assembleDebug
```

Layout: `klip-proto` (clip/clock/IPC types) · `klip-crypto` (identity, keystore) · `klip-net` (QUIC node, pairing) · `klip-core` (engine) · `klip-platform` (clipboard drivers) · `klip-daemon` + `klip-cli` (desktop) · `klip-android` (FFI) · `android/` (app).

## More documents

- [`docs/AUDIT_AND_ROADMAP.md`](docs/AUDIT_AND_ROADMAP.md) — frank audit of the earlier code and the roadmap
- [`docs/RESEARCH_2026.md`](docs/RESEARCH_2026.md) — sourced research, decisions and measurements
- [`docs/ARCHITECTURE_AND_RESEARCH.md`](docs/ARCHITECTURE_AND_RESEARCH.md) — the original (unverified, aspirational) design brief

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
