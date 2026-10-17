# Klip on Android

Time needed: about 10 minutes. **Needs Android 8.0 or newer.** Tested on an Android 14 emulator and on one real phone (a Vivo).

## Step 1: install the app

Do this **on the phone**.

1. Open this page in the phone's browser: <https://github.com/Tribal-Chief-001/Klip/releases> and open the newest release.
2. Under **Assets**, tap **`Klip-Android.apk`** (about 11 MB, for 64-bit phones, which is nearly every phone made since 2017). Only if your phone is very old and 32-bit, take `Klip-Android-armv7.apk`.
3. When the download finishes, tap it to open. Android says it can't install apps from this source: tap **Settings** and switch on **Allow from this source** for your browser (or your Files app), go back, then tap **Install**.
4. If *Play Protect* says it has not seen the app before, tap **More details → Install anyway**. Klip is not on the Play Store, so this is normal.

(Optional but good: next to each file there is a `.sha256` file with its checksum, if you want to check the download on a computer.)

Updating later is the same: install the new APK over the old one. Your links are kept.

## Step 2: the Setup screen (this is the important one)

Open **Klip** and tap **Setup** (top right). Android stops background apps to save battery, and some phone makers do it harder than Android does. Klip lists what to switch on, explains each one, and ticks off the ones Android lets it check:

| Item | What to do |
|---|---|
| **Notifications** | Tap *Allow notifications*. Android makes any app that runs all the time show a small notification; Klip uses it for status and a "Send clipboard" button. It cannot read your notifications. |
| **Unrestricted battery use** | Tap it and choose **Unrestricted**. Without it Android may freeze Klip when the screen is off and your laptop will say the phone is not reachable. Costs a little battery (one open network connection). |
| **Auto-start & background (for your phone brand)** | Klip shows the steps for Vivo, Xiaomi, Oppo/Realme/OnePlus, Samsung and Huawei/Honor; other brands need nothing. Android can't check these switches, so after doing them tap **I've done these steps**. |
| **Quick Settings tile** (optional) | One tap on the pull-down panel sends what you just copied. Swipe down twice, tap the pencil, drag **Klip** into the panel. |
| **Send from any app** (optional) | Nothing to set; it tells you that Share → Klip works everywhere. |

When everything is done the top says **All set. Klip can run in the background.**

None of these gives Klip access to your messages, photos, contacts or accounts.

## Step 3: link to your computer

Go to [Link your devices](05-link-devices.md). On the phone the button is **Add a device**.

## Step 4: look at Privacy

Tap **Privacy** (top right). It says in plain words where your clips go. It also has:

- **Use relay servers**: on lets devices on different networks find each other. Off means same network only.
- **Receive files**: on/off and the biggest file size to accept (default 2 GiB).
- **Restart by itself if stopped**: if you remove Klip from Android's active-apps list with its Stop button, Klip starts again within about 15 minutes (only works if battery use is Unrestricted).

## Using it

| I want to… | Do this |
|---|---|
| Get what I copied on the computer onto the phone | Nothing. Just paste on the phone. |
| Send text from the phone to the computer | Select text → **Share → Klip**; or copy, then tap **Send clipboard now** in the app, the **Klip tile**, or the **Send clipboard** button in Klip's notification. (Android lets only the app on screen read the clipboard, so Klip can't notice a copy by itself.) |
| Send a photo or file | In Gallery, Files or any app: **Share → Send as a file with Klip** (one or many). A notification shows progress and has **Cancel**. |
| Send a whole folder | In Klip tap **Send a folder**, pick the folder. Up to 1000 files, 16 levels deep; documents that only live in the cloud are skipped. |
| Find files the computer sent | Files app → **Downloads → Klip**. A notification says a file arrived (never with its name). On Android 8 and 9 the file stays in Klip's own folder instead, and the notification says so. |

All the details: [Everyday use](06-daily-use.md).

## Remove Klip

1. In Klip tap the computer's name → **Remove this device** (optional, but tidy).
2. Android Settings → Apps → Klip → **Uninstall**.
3. On the computer, `klip unpair "<phone name>"` removes the phone from that side.

Received files in `Downloads/Klip` are yours and stay.

Problems? [Troubleshooting](10-troubleshooting.md) · Questions? [FAQ](09-faq.md) · Why can't the phone send automatically? [Android details](../ANDROID_DETAILS.md)
