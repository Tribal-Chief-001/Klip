<div align="center">

<img src="docs/assets/banner.svg" alt="klip: two devices joined by a line, and the word klip" width="760">

### Copy on one of your devices, paste on another. End-to-end encrypted, no account, no cloud.

[![CI](https://img.shields.io/github/actions/workflow/status/Tribal-Chief-001/Klip/ci.yml?branch=main&style=flat-square&label=CI)](https://github.com/Tribal-Chief-001/Klip/actions/workflows/ci.yml)
[![Android build](https://img.shields.io/github/actions/workflow/status/Tribal-Chief-001/Klip/android.yml?branch=main&style=flat-square&label=Android%20build)](https://github.com/Tribal-Chief-001/Klip/actions/workflows/android.yml)
[![License](https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-blue?style=flat-square)](LICENSE-MIT)
[![Release](https://img.shields.io/github/v/release/Tribal-Chief-001/Klip?include_prereleases&style=flat-square&label=release)](https://github.com/Tribal-Chief-001/Klip/releases)
![Status](https://img.shields.io/badge/status-alpha-orange?style=flat-square)

</div>


> **Alpha software.** The Linux ↔ Android path works and is tested; macOS and Windows are only tested on automatic test machines, never on a real Mac or PC; there has been **no independent security review**. Read [what is tested and what is not](docs/STATUS_AND_LIMITS.md) before you rely on it.

```text
╭── ●─● klip ───────────────────────────────────────────────────────────────╮
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

- **Copy on one device, paste on another.** Text goes by itself; images wait until you send them (so a private screenshot doesn't fly everywhere).
- **Send files and folders** between your devices: `klip send-file photo.jpg`, or Share → Klip on the phone. Checked end to end, never overwrites, picks up where it stopped if cut.
- **Works across networks** (phone on mobile data, laptop at home), and on the same Wi-Fi even without internet.
- **No account, no server of ours.** End-to-end encrypted. You link devices once with a QR code **and** your approval.
- **Honest feedback:** when you send, Klip tells you whether the other device really got it, or why not.
- A **tray icon** on Linux, macOS and Windows, and a **full-screen terminal app** (`klip`).

## Get started in 10 minutes

Pick your device. Each page is a numbered, copy-paste walkthrough with install, start, auto-start, update and uninstall.

| Device | Guide | Ready-made download (latest release) | Tested |
|---|---|---|---|
| 🐧 **Linux** (64-bit, X11) | [Linux guide](docs/guide/01-linux.md) | `klip-linux-x86_64` | ✅ on a real machine |
| 📱 **Android** 8.0+ | [Android guide](docs/guide/04-android.md) | `Klip-Android.apk` (64-bit) · `Klip-Android-armv7.apk` (old 32-bit) | 🟡 emulator + one real phone |
| 🍎 **macOS** (Apple chip) | [macOS guide](docs/guide/02-macos.md) | `klip-macos-arm64` | 🟡 test machine only, never on a real Mac |
| 🪟 **Windows** 10/11 | [Windows guide](docs/guide/03-windows.md) | `klip-windows-x86_64.exe` | 🟡 test machine only, never on a real PC |
| iPhone / iPad | not possible (Apple blocks background clipboard) | | |

Downloads are on the [Releases page](https://github.com/Tribal-Chief-001/Klip/releases) (still marked "pre-release" while Klip is alpha). Each file has a `.sha256` next to it so you can check it.

Then, once:

1. **Link** your devices: run `klip pair` on the computer, scan the QR code with the phone (Klip → **Add a device** → **Scan a code**), approve with `y`. → [Link your devices](docs/guide/05-link-devices.md)
2. **Use** it: copy here, paste there. → [Everyday use](docs/guide/06-daily-use.md)

> **Both devices must be on and online at the same time.** There is no mailbox: a copy made while the other device is off is not delivered later. That is on purpose.

## The guide

| | |
|---|---|
| [**Guide start**](docs/guide/README.md) | the map of all pages and a small word list |
| [Linux](docs/guide/01-linux.md) · [macOS](docs/guide/02-macos.md) · [Windows](docs/guide/03-windows.md) · [Android](docs/guide/04-android.md) | install, start, auto-start, update, remove |
| [Link your devices](docs/guide/05-link-devices.md) | pairing, step by step, and what to do if it fails |
| [Everyday use](docs/guide/06-daily-use.md) | text, images, files, folders, pause, history |
| [Settings](docs/guide/07-settings.md) · [Commands](docs/guide/08-commands.md) | every setting and every command |
| [FAQ](docs/guide/09-faq.md) · [Troubleshooting](docs/guide/10-troubleshooting.md) | quick answers and fixes |
| [Privacy and security](docs/guide/11-privacy-and-security.md) | what goes where, in plain words |
| [Hinglish quick guide](docs/guide/12-hinglish.md) | Hindi-English short version |

**For the curious:** [Status and limits](docs/STATUS_AND_LIMITS.md) (what is tested, what is not, how it works) · [Tray icon and terminal app](docs/TRAY_AND_TERMINAL_APP.md) · [Android details](docs/ANDROID_DETAILS.md) (why the phone can't send by itself, optional helper, banking apps) · [Audit and roadmap](docs/AUDIT_AND_ROADMAP.md) · [Research](docs/RESEARCH_2026.md) · [Original design brief](docs/ARCHITECTURE_AND_RESEARCH.md)

## Quick reference

```bash
klip start                      # start the background service
klip status                     # who is linked, is it connected
klip pair                       # link a new device (QR code)
klip send "text"                # send text now
klip send-file FILE_OR_FOLDER   # send files (--to NAME for one device)
klip pause 600 / klip resume    # stop syncing for a while
klip                            # full-screen app
```

Every command: [Commands](docs/guide/08-commands.md).

## Build and test from source

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
android/build-core.sh arm64-v8a             # Rust engine + Kotlin bindings (needs cargo-ndk, NDK r28+)
cd android && ./gradlew assembleDebug
```

Needs Rust 1.91 or newer. Layout: `klip-proto` (clip/clock/IPC types) · `klip-crypto` (identity, keystore) · `klip-net` (QUIC node, pairing) · `klip-core` (engine) · `klip-platform` (clipboard drivers) · `klip-daemon` + `klip-cli` (desktop) · `klip-tui` (terminal app) · `klip-android` (FFI) · `android/` (app). Tests run in CI on Ubuntu, macOS and Windows.

## Help and feedback

Something unclear or broken? Check [Troubleshooting](docs/guide/10-troubleshooting.md), then open a [GitHub issue](https://github.com/Tribal-Chief-001/Klip/issues). A report from a real Mac or Windows PC is the most valuable thing right now.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
