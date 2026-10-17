# The tray icon and the terminal app

> Part of the [Klip guide](guide/README.md). Moved here from the front page, unchanged apart from links.

## The tray app

Klip has a small tray icon with a menu on **Linux** (built for Linux Mint Cinnamon; any panel with StatusNotifier support should work), on the **macOS menu bar** and in the **Windows notification area**. Linux is the one that was seen working on a real desktop; the macOS and Windows trays were built and tested on CI runners only (see [Status](STATUS_AND_LIMITS.md)). Everything else (devices, pairing, history, send, log, settings) is the [terminal app](#the-terminal-app). The tray is part of the `klip` binary (no Python, no GTK, nothing extra to install): on Linux it measured 8.9 MB resident (3.8 MB of its own) and no CPU while idle; the debug builds started on the CI runners measured about 14 MB on Windows and 50 MB on macOS (a release build and a real desktop were not measured).

```bash
klip tray install     # start it every time you log in
klip tray             # start it now (in the background)
```

- **The icon** is monochrome like the other panel icons (on Linux the desktop paints it in the panel's colour, on macOS the system tints a template image, on Windows it is drawn light or dark to match the taskbar theme); the shape is the state: a solid line between the two dots = connected, dots = waiting for a device, two bars = paused, faded = stopped or no devices; a dot in the corner = an image is waiting to be sent. Hover for the details.
- **Left or right click** opens the same menu: the state, send the last copied image, send images automatically, pause or resume, open the terminal app, quit the tray (start Klip, when it is stopped). **Middle click** sends the waiting image.
- **Images are manual**: a copied image is held for 10 minutes (the last one only) and is sent when you use the menu, middle-click the icon, or run `klip share` (bind it to a keyboard shortcut in your desktop's keyboard settings). Prefer the old behaviour? Tick *Send images automatically* in the menu, or run `klip images auto`. Text is always sent automatically.
- **Start at login** (`klip tray install`, undone by `klip tray uninstall`): Linux `~/.config/autostart/klip-tray.desktop`, macOS `~/Library/LaunchAgents/com.klip.tray.plist`, Windows the `KlipTray` value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. Since v0.10 it starts a small script (`%LOCALAPPDATA%\Klip\start-tray.vbs`, run by `wscript.exe`) so no console window flashes at login; where the Windows Script Host is missing it falls back to `klip tray start` (which may flash a console once). If you installed the entry with an older Klip, run `klip tray install` again to get the quiet one. A login entry that Klip did not make is never overwritten or removed (on Windows: only a `KlipTray` value that starts Klip's tray is touched).
- **When something fails** (a refused send, an unreachable daemon) Linux shows one desktop notification; on macOS and Windows the problem is the first line of the menu for 30 seconds.
- Linux talks to the panel over D-Bus (StatusNotifierItem); macOS and Windows use `tao` and `tray-icon`. `klip tray status` says whether it is running. Quitting the tray never stops Klip itself.

## The terminal app

Run `klip` with no arguments on a terminal. Quitting (`q`) leaves Klip running in the background.

```text
●─● klip   1 Home  2 Devices  3 Pair  4 History  5 Send  6 Log  7 Settings                  ● ACTIVE
┌ Status ──────────────────────────────────────────────────────────────────────────────────────────┐
│● ACTIVE   relay connected (https://aps1-1.relay.n0.iroh.link./)                                  │
│this device  4RGS-5DPX-3MZG   Linux (X11)   RAM 15.8 MB                                           │
│128 clips synced · last 4s ago · "https://github.com/…" (38 B)                                    │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
┌ Devices (2) ─────────────────────────────────────────────────────────────────────────────────────┐
│  ● Vivo V2253              [XUXC-EASU-TQP7]  connected                                           │
│  ○ Pixel 8                 [E6Y4-MBFJ-TGTY]  offline · could not reach the device                │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
s start · x stop · 1-7 screens · p pause · h help · q quit
```

*Text snapshot from the test suite with sample data (not a screenshot of a real session).*

| Key | Does |
|---|---|
| `1`-`7` / `Tab` | switch screen (Home, Devices, Pair, History, Send, Log, Settings); `Alt+1-7` also inside a text field |
| `p` | pause or resume syncing |
| `h` | help for the current screen |
| `m` | mouse on/off (off lets you select text with the terminal) |
| `Ctrl+L` | redraw |
| `q` / `Ctrl+C` | quit |
| Devices | `↑↓` select · `u` unpair · `a` unpair all |
| Pair | `n` new code · `l` LAN addresses · `t` validity · `j` join with a ticket · `y`/`n` answer a request |
| History | `↑↓` select · `/` search · `Enter` copy back to this computer · `s` send again · `v` show a secret · `c` clear · `Home`/`End` first/last |
| Send | `Enter` start typing (until then the number keys still switch screens) · `Ctrl+S`/`F5` send · `Ctrl+O` load a file · `Ctrl+U` clear · `Esc` stop typing |
| Log | `f` follow · `/` search · `PgUp`/`PgDn` scroll · `End` newest |
| Settings | `↑↓` select · `Enter` edit/toggle · `s` save · `u` discard · `r` restart Klip |

Keys are lowercase letters only: a capital letter (Shift or Caps Lock) does the same as the small one, outside text fields. Colours: `NO_COLOR=1` switches to bold/reverse, `KLIP_THEME=light` for light terminals; meaning is never colour-only.

The Pair screen shows the QR code beside the ticket. If the window is too small for it, Klip asks the terminal to grow (and gives the old size back when you leave the screen); terminals that do not allow that keep their size and the screen says how big it needs to be.

*Clips shown on screen are sanitized: escape sequences and text-direction tricks appear as visible marks (`␛`, `⟦U+202E⟧`). Secrets are hidden until you press `v`. The pairing ticket is a secret: it is shown only on the Pair screen and the window closes when you leave it. Terminal scrollback and screen recorders can still capture what is on screen.*
