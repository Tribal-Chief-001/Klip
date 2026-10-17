# Everyday use

Klip is set up and your devices are [linked](05-link-devices.md). This page is about what to do day to day.

## Text

### Computer → phone (or computer → computer)

Copy. That's all. A moment later it is on the other device's clipboard; paste there. To be told the result, use `klip send "some text"`: it prints for each device whether it really arrived on its clipboard, or why not.

```bash
klip send "meet at 5"            # send text without copying it first
cat notes.txt | klip send -      # send a file's text (up to the clip limit, 32 MiB)
```

### <a name="phone-to-computer"></a>Phone → computer

Android only lets the app that is *on screen* read the clipboard, so Klip can't notice a copy made in another app. You do one tap. Easiest first:

1. **Share → Klip.** In any app (browser, chat…) press the normal Share button and pick **Klip**. Best for links.
2. **Select text → ⋮ → "Send to Klip"** in the pop-up menu.
3. **The Klip tile:** copy, pull down the top panel, tap **Klip**. (Add it once: Setup → Quick Settings tile.)
4. **The "Send clipboard" button** in Klip's notification, or **Send clipboard now** in the app.

Why, and the advanced optional helper that makes it automatic: [Android details](../ANDROID_DETAILS.md).

### What is sent, what is not

Plain text is sent exactly as it is (tabs, emoji, other scripts, long lines). **Formatting is not** (bold, colours, tables copied from a web page arrive as plain text). Very large text: Android can take about 0.9 MB of text onto its clipboard; bigger works between computers only.

Passwords: text that *looks* like a secret is hidden in the history view, but **it is still synced**. If you copy a password you don't want elsewhere, use **pause** first (below).

## Images

Copied images **wait for you**, on purpose, so a private screenshot doesn't fly to every device.

- **On the computer:** copy the image, then in the tray menu choose *Send the last copied image*, or **middle-click** the tray icon, or run `klip share`. (You can bind `klip share` to a keyboard shortcut in your desktop's settings.) The image is kept for 10 minutes, the last one only.
- Want every image to go at once? Tick *Send images automatically* in the tray menu, or run `klip images auto`. Back: `klip images manual`.
- **On the phone:** Share an image → Klip. JPEG/HEIC are turned into PNG, and location data (EXIF) is removed.
- Limits: images up to 12 MB on the phone (32 MiB on a computer), about 36 megapixels. Too big: the sender is told.

## Files

Files go over their own encrypted stream, are checked end to end (a BLAKE3 checksum), and **never replace an existing file** (a taken name becomes `name (2).ext`). A file that fails halfway leaves nothing behind.

### From the computer

```bash
klip send-file report.pdf                       # to every linked device that is online
klip send-file photo1.jpg photo2.jpg            # several files
klip send-file ~/Pictures/Trip                  # a whole folder: it arrives as one new folder "Trip"
klip send-file movie.mkv --to "Pixel 8"         # only to one device (name or start of fingerprint)
klip send-file big.iso --no-wait                # start, come back at once
klip transfers                                  # running and recent transfers (first column = id)
klip transfers --cancel 3fa9                    # stop one (the start of its id)
```

In the tray menu: **Send a file…** (needs `zenity` or `kdialog` on Linux; built in on macOS and Windows) and **Open the Klip folder**.

### From the phone

- One or more files: **Share → Send as a file with Klip** from Gallery, Files or any app.
- A folder: open Klip → **Send a folder** → pick it.
- A notification shows progress and has **Cancel**.

### Where files arrive

| Device | Folder |
|---|---|
| Linux, macOS, Windows | `Downloads/Klip` in your home folder (change with `receive_dir` in [Settings](07-settings.md)) |
| Android 10+ | `Downloads/Klip` (open the Files app → Downloads) |
| Android 8 and 9 | Klip's own folder; the notification says so (Android 8-9 can't show it in Downloads) |

You get a notice "Received 1 file from <device>", never with the file's name (notifications can be seen on a lock screen).

### Folders

A folder is sent file by file into one new folder on the other device (`Trip`, and `Trip (2)` if you send it again). Links (shortcuts) are not followed, only normal files go, at most **1000 files** and **16 levels deep**, empty folders are not sent. If one file fails, the rest still go and the result says "N of M files failed".

### Cut off halfway? Send it again

If the connection drops or the other Klip restarts, send the **same file again**: the receiver keeps what arrived (parts of at least 1 MiB, for 24 hours) and only the rest travels. The whole file is checked at the end. If you press Cancel on the sending side, the receiver throws the part away at once.

### Limits and refusals

The receiving device can say no, and you are told why: file transfer is switched off there (`accept_files`), the file is bigger than its limit (`max_file_bytes`: 4 GiB on computers, 2 GiB default on the phone), not enough free space (it keeps 512 MiB free), or too many transfers at once. Both devices must be online together.

## Pause, history, status

```bash
klip pause 600       # stop syncing for 600 seconds (10 min is the default)
klip resume          # start again now
klip status          # devices, connection, last sync
klip history         # recent clips (secrets masked; forgotten after 24 h)
klip history clear   # forget them all now
```

Clips are kept in memory only: a restart empties the history.

## The tray icon and the full-screen app

- **Tray icon** (Linux, macOS menu bar, Windows notification area): the shape tells the state (a solid line between two dots = connected, two bars = paused, faded = stopped). Click it for the menu. [All about the tray](../TRAY_AND_TERMINAL_APP.md).
- **Full-screen app:** type `klip` in a terminal. Number keys `1`–`7` switch between Home, Devices, Pair, History, Send, Log and Settings; `h` shows help for the screen; `q` leaves (Klip keeps running). [All the keys](../TRAY_AND_TERMINAL_APP.md#the-terminal-app).

## Good habits

- Keep **Setup** done on the phone (battery Unrestricted), or the laptop will often say the phone is not reachable.
- If you copy something private, **pause** (`klip pause`, or the tray menu) and resume after.
- Not using a device any more? **Remove it** from your other devices ([how](05-link-devices.md#see-check-and-remove-linked-devices)).
