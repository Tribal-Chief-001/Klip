# Frequently asked questions

## The basics

**What is Klip?** A program (and Android app) that copies what you copy on one of your devices to your other devices, and sends files and folders between them. Like a universal clipboard, for Linux, macOS, Windows and Android.

**Do I need an account?** No. No sign-up, no email, no phone number. Devices are recognised by a key each one makes for itself.

**Does it cost anything?** No. It is free, open source (MIT or Apache-2.0, your choice).

**Is it safe to use?** The contents are end-to-end encrypted. But it is **alpha** software and has **not been independently audited**. Fine for everyday text and files; don't rely on it where a bug would be dangerous. [Details](11-privacy-and-security.md).

**Does it work on iPhone / iPad?** No. Apple does not let apps read the clipboard in the background. Not planned for now.

**Does it work on a Mac with an Intel chip?** The ready-made program is for Apple Silicon (M1 and later). On Intel, build from source ([macOS page](02-macos.md#option-b-build-it-yourself)).

**Does it work on Linux with Wayland?** Only partly (through XWayland). Use an X11 session if nothing syncs. [Linux page](01-linux.md#wayland-important).

**How many devices can I link?** Several. Each pair of devices is linked on its own ([how](05-link-devices.md#more-than-one-phone-or-computer)).

## Syncing

**I copied on the phone but nothing arrived on the computer.** Android only lets the app on screen read the clipboard, so a copy on the phone isn't noticed by itself. Use **Share → Klip**, the Klip tile, or the notification button. [Why](../ANDROID_DETAILS.md).

**I copied on the computer but nothing is on the phone.** Check `klip status`: is the phone green/connected? If not: is the phone on, with internet, and did you finish **Setup** (battery Unrestricted)? See [Troubleshooting](10-troubleshooting.md).

**My other device was off. Will the copy arrive later?** No. Both devices must be online at the same time. This is a deliberate choice: no server of ours keeps your data waiting.

**Why do images not go by themselves?** On purpose, so a private screenshot doesn't reach every device. Send with the tray menu, middle-click, or `klip share`; or switch on automatic images (`klip images auto`).

**Is formatting kept (bold, colours)?** No, plain text only.

**Can it sync more than one clip / a history?** The clipboard is synced one clip at a time. `klip history` shows recent clips on that computer, kept in memory for 24 hours.

**Can I stop it for a moment?** `klip pause` (10 minutes) or `klip pause 3600`; `klip resume` ends it. Also in the tray menu.

**Does it keep working with the screen off?** On a computer, yes (if the computer isn't asleep). On Android, only if you did **Setup** (notifications, Unrestricted battery use, your phone brand's auto-start switch).

## Files

**How big can a file be?** Computers accept up to 4 GiB by default; the phone 2 GiB (changeable). The other device can refuse, and you are told why.

**Where do received files go?** `Downloads/Klip`. [Table](06-daily-use.md#where-files-arrive).

**Will it overwrite a file with the same name?** Never. It adds ` (2)`, ` (3)`.

**The transfer stopped halfway.** Send the same file again; only the missing part travels (for 24 hours).

**Can I send a folder?** Yes, from computers (`klip send-file folder`) and from the phone (**Send a folder**). Up to 1000 files and 16 levels.

**Can I send files without internet?** Yes, on the same Wi-Fi with `"relay": "lan"` on both devices. [Settings](07-settings.md).

**How do I know the file arrived intact?** Every file is checked with a BLAKE3 hash at the end. A damaged file is deleted and you are told.

## Privacy

**Can Klip or anyone else read my clipboard?** Not on the way: it's encrypted between your devices. The public relay servers (run by n0, the iroh project) can see that two device IDs talk, when, and how much, not what.

**Can I avoid third-party servers completely?** Yes: `"relay": "lan"` (same network only).

**Does Klip send analytics?** No analytics, ads or tracking.

**What if I lose my phone?** On any other linked device, remove it (`klip unpair "<phone>"`). It's cut off immediately.

**Does Klip see my passwords?** If you copy one, it syncs like any text. It is hidden in the history view, but still sent. Pause before copying secrets you don't want elsewhere.

## Setup and updating

**Why does my computer/phone warn me about the app?** It isn't signed with a paid certificate (Windows SmartScreen, macOS Gatekeeper, Android Play Protect). Follow the steps on your [platform page](README.md) and only use files from the official Releases page.

**How do I update?** Install the new version over the old one ([Linux](01-linux.md#update), [macOS](02-macos.md#update), [Windows](03-windows.md#update), Android: install the new APK). Links are kept.

**How do I know which version I have?** Android: Settings → Apps → Klip shows the version. On computers there is no version flag yet; compare your file's checksum with the `.sha256` file on the Releases page.

**Do I have to link again after an update?** No. Release APKs are signed with the same key, so installing over the old app keeps your links.

**How do I remove everything?** The last section of your [platform page](README.md).

## Something else

**Where is the log?** `daemon.log` in Klip's settings folder ([where](07-settings.md)). The full-screen app shows it on screen `6`.

**Where do I report a bug or ask for something?** [GitHub issues](https://github.com/Tribal-Chief-001/Klip/issues). Please say your system, what you did, what you expected, and what happened. **Never paste your ticket or `keystore.json`.**

**Can I help?** Yes: try it on a real Mac or Windows PC and tell us what happened. That is the thing that is least tested.

← [Guide start](README.md)
