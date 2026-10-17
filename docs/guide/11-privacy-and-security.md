# Privacy and security, in plain words

## What goes where

```
Your laptop  ◄──── encrypted, direct if possible ────►  Your phone
                          │
              only if a direct path fails
                          ▼
                  a public relay server
           (sees that two devices talk; not what)
```

- **What you copy** (text, images) and **files** go only to the devices you linked, encrypted from your device to theirs. Nobody in the middle can read it.
- There is **no Klip account and no Klip server** that stores anything.
- There are **no analytics, ads or tracking** in the app.
- Clips are kept **in memory** on computers (a restart clears them, and after 24 hours by default). Received files are normal files in your Downloads/Klip.

## What other parties can see

With the default `"relay": "n0"`:

- **n0's public relay and discovery servers** (run by the iroh project) learn that your device IDs exist, their network addresses, when they talk and how much. They can't read contents.
- Your internet provider can see that you connect to those servers or directly to your other device, not what is inside.

If that matters to you: set `"relay": "lan"` ([Settings](07-settings.md)). Then no third-party server is used, but both devices must be on the same network.

## How linking stays safe

- Linking needs a one-time ticket (valid 2 minutes, once) **and** your approval on the other device. The two devices show a short fingerprint like `75A6-H66R-SQDW`; compare them.
- Only linked devices are accepted. A clip is accepted only if the sender is the device that says it copied it.
- If a linked device is lost, remove it from another one: `klip unpair "<name>"`. Until you do, it stays trusted.

## What is stored on your devices

| Where | What |
|---|---|
| `keystore.json` in Klip's settings folder | This device's secret key, in a private file (readable only by you). **It is not password-protected or in the system keyring yet.** Anyone who copies it can act as this device. Never share it or paste it in an issue. |
| `config.json` | Your settings. |
| `daemon.log` | A log of what Klip is doing, for fixing problems. Look at it before you share it. |
| Android | The key is in the app's own private storage (Android's normal app sandbox). Received images are kept as files in the app's cache for a short time. |

Anyone who can run commands **as you** on your computer can read your clipboard history through Klip's local control channel (a private socket on Linux/macOS, a private named pipe on Windows).

## What Klip does not do (yet)

- **Not independently audited.** Don't use it where a bug could be dangerous.
- **No post-quantum encryption** yet (planned). Today it is X25519 + Ed25519 over QUIC/TLS 1.3, which is standard and strong against today's computers.
- **Secret detection is a guess.** It hides likely passwords in the history and asks Android to hide keyboard previews. It does **not** stop them from syncing.
- Notifications about received files never show the file's name (so a lock screen doesn't show it).

## On the phone

Klip asks for: internet; notifications (the status notification); camera (only to scan the pairing QR code); battery exemption (so it isn't frozen); boot-complete (to restart after a reboot). No access to contacts, messages, photos, location or accounts, and **no Accessibility service**. It reads the clipboard only when you tap something (Share, the tile, the button).

## Reporting a security problem

Open a GitHub issue that says only that you have a security problem (no details), and the owner will arrange a private way to receive them.

More technical detail: [Status and limits](../STATUS_AND_LIMITS.md).

← [Guide start](README.md)
