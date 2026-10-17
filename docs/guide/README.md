# Klip guide

Klip copies what you copy on one of your devices onto your other devices, and sends files and folders between them. It is end-to-end encrypted, has no account and no server of ours.

This guide is for people who just want to use it. No programming needed. Pick the page for your device, follow the numbered steps, and you are done in about 10 minutes.

> **Klip is alpha software.** It works, but it is young: read [what is tested and what is not](../STATUS_AND_LIMITS.md) before you depend on it.

## Start here

| I want to… | Read |
|---|---|
| Set up **Linux** | [Linux](01-linux.md) |
| Set up **macOS** | [macOS](02-macos.md) |
| Set up **Windows** | [Windows](03-windows.md) |
| Set up **Android** | [Android](04-android.md) |
| **Link** two devices (do this once, after both are installed) | [Link your devices](05-link-devices.md) |
| Learn how to **use** it every day (text, images, files, folders) | [Everyday use](06-daily-use.md) |
| Change a **setting** | [Settings](07-settings.md) |
| Look up a **command** | [Commands](08-commands.md) |
| Find a quick **answer** | [FAQ](09-faq.md) |
| Fix a **problem** | [Troubleshooting](10-troubleshooting.md) |
| Know what happens to my **data** | [Privacy and security in plain words](11-privacy-and-security.md) |
| Read this in **Hinglish** | [Hinglish quick guide](12-hinglish.md) |
| **Update** or **remove** Klip | the last section of your platform page |

## The three steps, for every device

1. **Install** Klip on each device (a program on a computer, an app on the phone).
2. **Link** them once: one device shows a QR code, the other scans it, and you approve it on the first device.
3. **Use** it: copy on one, paste on the other. Send files with one command or one tap.

## Words used in this guide

| Word | Meaning |
|---|---|
| **Device** | A computer or a phone that runs Klip. |
| **Linked** (or paired) | Two devices that you connected once and that now trust each other. |
| **Daemon** | The small background part of Klip on a computer. It does the work; you do not see it. `klip start` starts it. |
| **Ticket** | The text version of the QR code, starts with `klip1`. Works once, for 2 minutes. |
| **Terminal** | The window where you type commands. On Windows: *PowerShell*. On macOS: *Terminal*. On Linux: your terminal program. |
| **Tray** | The small icon in the top or bottom bar of your screen. |

## What you need to know first

- **Both devices must be on (and online) at the same time.** Klip has no mailbox: if the other device is off, the copy is not delivered later.
- Computers and phones can be on **different networks** (home Wi-Fi and mobile data). Klip finds a way. On the same Wi-Fi it works even without internet when you set `"relay": "lan"` (see [Settings](07-settings.md)).
- **Images are sent when you choose**, on purpose. Text goes by itself.
- A copy on the **phone does not go to the computer by itself** (Android's rule, not ours). You tap a button or use Share. See [Everyday use](06-daily-use.md#phone-to-computer).
