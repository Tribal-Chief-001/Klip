# Android details: phone to computer, helper, banking apps

> Part of the [Klip guide](guide/README.md).

### Phone → computer (Android limits)

**Why isn't it automatic?** Android 10+ only lets the app *in front* read the clipboard, and — verified on Android 14 — it does not even tell Klip that something was copied while another app is in front. So Klip cannot notice a copy by itself. (Computer → phone is instant because *writing* the clipboard is allowed.) Ways around it, easiest first, none needs special permissions:

1. **Share → Klip** — in any app (browser, chat…) use the normal Share button and pick **Klip**. Best for links.
2. **Select text → ⋮ → “Send to Klip”** in the text-selection menu.
3. **Quick Settings tile** “Klip” (swipe down, edit tiles, add Klip): copy, then tap the tile.
4. **“Send clipboard”** button in Klip's notification.

Both 1 and 2 were tested to reach the computer without opening Klip.

**Automatic sending (optional helper).** Android only lets the app on screen read the clipboard, so by default a copy made in another app is not noticed. The old trick of watching the system log (what KDE Connect used) is closed on **Android 13 and newer**: the system denies log access to background apps (AOSP `LogcatManagerService`), so Klip no longer uses it. What works instead is a small helper process that runs with the Android *shell* identity, which may read the clipboard in the background (the way scrcpy does it):

- **From a computer, once:** *Setup → Automatic copy from this phone* shows one `adb shell …` command (with a secret token) to run with USB debugging on; it starts the helper and returns. It stops when the phone restarts.
- **Rooted phones:** *Start with root* makes Klip start the helper itself (through `su`) whenever its service starts, including after a restart; the helper drops to the shell user.

The helper hands each copy to Klip over a loopback connection that both sides authenticate with the token (SELinux does not let the shell user use an app's Unix socket). Copies that apps mark as sensitive (password managers) are not sent automatically. **Cost:** a separate Java process, about **61 MB PSS** measured on an Android 14 emulator. **Verified (emulator):** with Klip in the background, a copy in another app reached a desktop daemon, started as the shell user and started as root (after dropping to shell); the in-app command, the live status and *Stop* work. **Not verified:** a real phone, Magisk's `su` prompt and the `su` call itself, Android 15/16, OEM restrictions on `app_process`. **Dead ends checked (2026-10-13, Android 14 emulator):** `READ_CLIPBOARD_IN_BACKGROUND` would remove the need for a helper, but it is held through the role `SYSTEM_TEXT_INTELLIGENCE` (`systemOnly`), and `pm grant` refuses it ("managed by role"). The helper's memory is the Android runtime itself: its private pages are mostly the boot images (`boot-framework.art` 13 MB, `boot-framework-adservices.art` 6 MB, `boot.art` 3 MB); `-Xmx`, `-Xusejit:false` and `-Xnorelocate` changed nothing. Only a native (non-Java) helper could be much smaller; that needs raw Binder calls and ClipData parcel parsing that differ between Android versions, which I cannot test on real phones yet.

Writing *to* the phone's clipboard from the background works without any special permission (tested on Android 14). If some phone brand blocks it, Klip shows a “tap to copy” notification instead of dropping the clip.

### Banking / UPI apps

Out of the box Klip uses **no Accessibility service** (the earlier prototype did, which is what made UPI apps complain), **no “display over other apps”** and **no log access**. The optional clipboard helper is a separate process you start yourself. I could not test real UPI apps (no Play Store on my test device), so this is unverified. If a payment app complains: stop the helper (Setup → Automatic copy → Stop) and tell me which app and message it shows; I will build a lean variant.
