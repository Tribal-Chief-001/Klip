# Troubleshooting

Find your symptom. Each fix is something you can do without programming. If it doesn't help, the last section says what to send us.

**First, always:** `klip status`. It shows if Klip is running, if it can reach the relay, and each linked device (green = connected, grey = not).

## Klip itself

| Symptom | Try |
|---|---|
| `klip: command not found` / "not recognized" | The folder with `klip` isn't in your PATH. Linux/macOS: `~/.local/bin` ([steps](01-linux.md#step-1-get-the-program)). Windows: close and reopen PowerShell after the PATH step, or run `$env:USERPROFILE\Klip\klip.exe`. |
| `klip start` says it did not come up | Read `daemon.log` in Klip's [settings folder](07-settings.md). Typical causes: a typo in `config.json` (the log names the key), or another Klip already running (`klip status`). |
| `klip` prints help instead of opening the app | It only opens in a real terminal with no arguments (not in a script, not `TERM=dumb`). Use `klip status` etc. in scripts. |
| macOS: "cannot be opened because the developer cannot be verified" | `xattr -d com.apple.quarantine ~/.local/bin/klip`, or System Settings → Privacy & Security → **Open Anyway**. |
| Windows: "Windows protected your PC" | **More info → Run anyway** (the file from the official Releases page only). |
| Windows: `register-service.ps1` says Access is denied | Open PowerShell **as administrator** and run it again; or just run `klip start` after login. |
| Klip stops after I restart the computer | You haven't installed auto-start. See Step 4 on your [platform page](README.md). |
| Nothing syncs on Linux | Are you in a **Wayland** session? Log in with X11 ([why](01-linux.md#wayland-important)). |

## Devices don't connect

| `klip status` shows | Try |
|---|---|
| *relay unreachable* | No internet, or UDP/HTTPS to n0 blocked (company or school network). On the same Wi-Fi set `"relay": "lan"` on **both** devices and restart. |
| Linked device **not connected** | Wait ~10 seconds (it retries). Is the other device on, awake, online, and is Klip running there? On the phone: did you finish **Setup**? Open the Klip app once. |
| Phone keeps disappearing after the screen is off | Phone settings are killing Klip. Do every item in Klip → **Setup**, especially *Unrestricted battery use* and your brand's *auto-start*. Xiaomi, Oppo, Vivo, Realme, Samsung and Huawei are the strict ones. |
| A computer firewall/VPN is in the way | Allow `klip` in the firewall for private networks; try without the VPN. |
| Works at home but not on mobile data (or the other way) | Normal for a few seconds while it finds a path. If it never connects, check the relay line in `klip status` on both sides. |

## Sending and receiving

| Message | Meaning / fix |
|---|---|
| "could not reach any linked device" | The other device is off, asleep or offline. Both must be online together. |
| "the other device could not write its clipboard" | That device's clipboard was busy or refused it. Try again. On Android, text over about 0.9 MB can't be written. |
| Pairing: "invalid or expired pairing code" | The code works once for 2 minutes. Run `klip pair` again. |
| A file was refused | The message tells why: `accept_files` is off there, the file is over its `max_file_bytes`, not enough free space (it keeps 512 MiB free), or too many transfers. Fix that on the receiving device. |
| A file was cut off | Send the same file again; the receiver kept the part that arrived (24 hours). |
| "update Klip on it" (folder refused) | The other device has an old Klip (before 0.9). Update it. |
| I can't find a received file | Computers: `Downloads/Klip`. Android: Files → Downloads → Klip (Android 8-9: Klip's own folder). |
| Copy on the phone isn't on the computer | Expected: use **Share → Klip**, the tile or the notification button. [Why](../ANDROID_DETAILS.md). |
| A picture is not sent | Images wait for you: tray menu, middle-click, or `klip share`; or `klip images auto`. |
| Image refused | Too big (over 12 MB on the phone or 36 megapixels), or not a real PNG/WebP/JPEG. |

## Linux tray

| Symptom | Try |
|---|---|
| No tray icon | `klip tray status`. Your panel needs StatusNotifier support (Cinnamon, KDE, most others; on GNOME install the *AppIndicator* extension). |
| "Send a file…" missing in the menu | Install `zenity` or `kdialog`. |

## Android

| Symptom | Try |
|---|---|
| "App not installed" | An older Klip with a different signing key is there (a test build). Uninstall it first, then install and link again. |
| No notification | Allow notifications for Klip (Setup → Notifications). The notification is what keeps Klip alive. |
| The camera won't scan | Allow camera for Klip in Android settings, or use **Paste a ticket**. |
| Klip was force-stopped | Open it once. Android doesn't let any app restart after "Force stop". |
| Banking/UPI app complains | Klip uses no Accessibility service. Stop the optional clipboard helper (Setup → Automatic copy → Stop) and tell us the app and message. [Details](../ANDROID_DETAILS.md#banking--upi-apps). |

## Start fresh (last resort)

This forgets your links; you'll have to link again.

```bash
klip stop
klip unpair --all -y
klip start
```

To also get a new identity, delete the settings folder ([where](07-settings.md)) while Klip is stopped. On Android: Settings → Apps → Klip → Storage → Clear data.

## What to send us if it still doesn't work

Open an [issue](https://github.com/Tribal-Chief-001/Klip/issues) with: your system and versions (Linux distro and X11/Wayland, macOS version, Windows version, Android version and phone brand), which step failed, the exact message, and the last lines of `daemon.log`. **Never paste a ticket (`klip1…`) or `keystore.json`.**

← [Guide start](README.md)
