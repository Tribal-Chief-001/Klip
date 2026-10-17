# Link your devices (once)

Linking ("pairing") tells two devices to trust each other. You do it **once per pair of devices**. It needs:

- Klip installed and running on both ([Linux](01-linux.md), [macOS](02-macos.md), [Windows](03-windows.md), [Android](04-android.md)).
- Both devices online.
- You, next to both of them, for about a minute.

A one-time code **and** your approval are both needed, so nobody else can link to you by guessing.

## Computer + phone (the usual case)

**1. On the computer**, open a terminal (on Windows: PowerShell) and type:

```bash
klip pair
```

A **QR code** and a line of text (the *ticket*, starts with `klip1`) appear. **Keep this window open.** The code works once and expires after 2 minutes (a new one: run the command again).

**2. On the phone**, open Klip → **Add a device** → **Scan a code** → point the camera at the QR code on the computer screen. (First time: allow the camera. No camera or it won't scan? Choose **Paste a ticket** instead, and paste the `klip1…` text.)

**3. Back on the computer** a question appears: *"<phone name> wants to pair [fingerprint]. Approve?"* Check that the fingerprint matches what the phone shows for itself, then type `y` and press Enter.

**4. Done.** Test it: copy a word on the computer and paste it on the phone.

> The text on the computer screen is a *secret* for those two minutes. Don't post a photo of it anywhere.

## Phone shows the code instead

Useful when the computer has no screen to scan, or the camera is on the computer:

1. On the phone: **Add a device → Show my code**. A QR code and a ticket appear.
2. On the computer, type `klip pair klip1…` with the whole ticket (or scan the code with another device).
3. Approve on the phone when it asks.

## Computer + computer

Same thing: on the first computer run `klip pair`. On the second one run `klip pair klip1…` with the ticket text printed by the first (copy it into the second computer by any way you like, for example a chat message to yourself; it only works once and for two minutes). Approve with `y` on the first.

## Pairing in the full-screen app

Run `klip`, press `3` for the **Pair** screen. It shows the QR code and ticket. Keys: `n` new code · `l` use LAN addresses · `t` change how long it is valid · `j` join with a ticket · `y` / `n` approve or refuse a request.

## Same Wi-Fi, no internet

```bash
klip pair --lan
```

puts the computer's local addresses in the ticket (a bigger QR code) so the phone can find it without internet. For everyday use without any internet set `"relay": "lan"` on both devices ([Settings](07-settings.md)).

## More than one phone or computer

Link each new device with `klip pair` again. Every device you link works with every other linked device only if you link them to each other as well: linking phone A and phone B to the same laptop does **not** link A and B together. A good habit: link every device to every other.

## See, check and remove linked devices

| I want to… | Do this |
|---|---|
| See who is linked | `klip status` (a green dot = connected now) |
| Check I linked the right device | the fingerprint (like `75A6-H66R-SQDW`) must be the same on both screens |
| Remove one | computer: `klip unpair "Pixel 8"` (name, or the start of its fingerprint) · phone: tap the device → **Remove this device** |
| Remove all | `klip unpair --all` (asks first; `-y` skips the question) |

**If a device is lost or stolen**, remove it from another of your devices at once. A removed device is cut off immediately and needs a new code to come back.

## If linking does not work

| You see | What it means / do |
|---|---|
| "invalid or expired pairing code" | The code works once and for 2 minutes. Run `klip pair` again. |
| Phone says it can't reach the computer | Keep `klip pair` running; check both have internet (or add `--lan` on the same Wi-Fi); wait 10 seconds and retry. |
| Nothing on the computer after scanning | Scroll up: the question may be above. In the full-screen app press `3`. |
| The camera won't open | Allow the camera for Klip in Android's app settings, or use **Paste a ticket**. |

More in [Troubleshooting](10-troubleshooting.md).
