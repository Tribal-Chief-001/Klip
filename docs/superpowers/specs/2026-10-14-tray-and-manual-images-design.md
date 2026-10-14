# Tray app and manual image sending

Status: written 2026-10-14. The owner asked for this in chat ("make the tray icon the most important
thing, full featured, sexy, minimal, simple, fast; auto start; everything the terminal can do; nothing
old may break") and gave blanket permission to run the whole process (brainstorm, spec, plan, build,
test, review, ship) without asking. Decisions that the owner would normally make are marked
**Ruling** and repeated in the release notes.

## Problem

1. Every image copied on the laptop is sent to the phone at once. The owner takes about ten
   screenshots a day and wants to choose which ones go (text keeps syncing automatically).
2. Everything except the automatic text sync needs a terminal (`klip`, `klip pair`, `klip send` ...).
   The owner wants a proper tray icon with a modern window that does all of it, starts at login, and
   is light.

## Success criteria

1. A copied image is held, not sent. One action sends the latest held image: tray (button, middle
   click, menu), command (`klip share`, for a keyboard shortcut), or the window.
2. "Send images automatically" can be switched on or off from the tray, the command line and
   `config.json`; the switch takes effect immediately and survives a restart.
3. The tray icon shows the state at a glance (connected, waiting, paused, stopped) and opens a window
   with every screen the terminal app has: home, devices, pairing (show a QR code, join with a ticket,
   approve a request), history (copy back, send again, reveal a secret, clear), send text or a file,
   log, settings (including restart and auto start).
4. It starts at login, runs with a small footprint (idle: no timers faster than a few seconds, no
   child process per poll), survives the daemon being stopped or restarted, and never crashes on a
   bad answer.
5. Nothing existing breaks: every test that passes today still passes; old and new clients and daemons
   keep working together (the control protocol only gains variants at the end).

## Decisions

- **Ruling: images are manual by default.** New config key `auto_send_images` (default `false`).
  A captured image is held in memory (latest only, 10 minutes) and is not put in the history until it
  is sent. Text and everything else is unchanged.
- **Ruling: no notification per screenshot** (the owner said no). The undelivered-copy alert from
  v0.4.3 stays: it fires only when something that was sent did not arrive.
- **Tray technology: Python 3 + GTK 3 + XApp (`XApp.StatusIcon`)**, which is what Linux Mint
  Cinnamon ships. No new Rust dependency and no change to the size of the `klip` binary beyond the
  embedded scripts. Rejected: a Rust GUI toolkit (egui/iced/gtk-rs: several MB, slow builds on this
  machine, new dependency tree); a Rust StatusNotifierItem crate (menu only, no window).
- **The scripts are embedded in the `klip` binary** (`include_str!`) and written on demand to
  `$XDG_DATA_HOME/klip/tray/<klip version>/klip_tray/`, so one downloaded file is still the whole
  product. `klip tray` runs `python3 -m klip_tray` from there.
- **Ruling: the tray talks to the daemon through a JSON bridge**, `klip api` (a hidden subcommand): one
  JSON object per line on stdin, one per line on stdout, one long-lived child process. It reuses the
  Rust `IpcRequest`/`IpcResponse` types through serde, so Python never re-implements the binary
  protocol. The bridge removes clip contents from history replies, adds `fingerprint` next to every
  `node_id`, and offers the file-based helpers the terminal app already has (config, log, service).
- **Ruling: thumbnails are made by the daemon** (`GetThumbnail`, PNG up to 240 px, only when asked, and
  only for the held image or a stored history image), so the tray never downloads full images.
- **Ruling: dark brand theme only** (ink `#0B0E10`, lime `#C8F25A`, the "two devices joined by a line"
  mark). No light theme in this release.
- **Ruling: closing the window hides it; "Quit tray" quits only the tray.** The daemon is never
  stopped by the tray unless the person presses Stop or Restart.
- **Ruling: Linux with XApp only.** Elsewhere `klip tray` prints a clear message. The daemon, CLI and
  terminal app are unchanged on every platform.
- **Ruling: `klip tray install` registers auto start** (`~/.config/autostart/klip-tray.desktop`). It
  is run on the author's machine when this release is installed, as the owner asked for auto start.

## Design

### Protocol (klip-proto) - variants appended at the end only
- `IpcRequest::GetImageState`, `SendPendingImage`, `SetAutoImages { enabled: bool }`,
  `GetThumbnail { id: Option<[u8; 16]> }` (`None` = the held image).
- `IpcResponse::ImageState(ImageStateInfo { auto: bool, waiting: Option<WaitingImageInfo> })`,
  `Thumbnail(Vec<u8>)`; `SendPendingImage` answers with the existing `Sent {..}`.
- `WaitingImageInfo { size_bytes: usize, width: u32, height: u32, age_secs: u64, kind: String }`.
- Capability `"share_image"` in `Hello`. Protocol version stays 2. The golden-bytes tests gain the
  new variants and keep the old ones.

### klip-core
- `DaemonConfig.auto_send_images: bool` (default false) and `DaemonConfig::save(paths)` (atomic,
  0600; shared with the terminal app's writer).
- `SyncCoordinator`: `auto_images: AtomicBool`, `pending_image: Mutex<Option<Pending>>`,
  `hold_image(content_type, data)`, `image_state()`, `send_pending_image()`, `set_auto_images(bool)`,
  `thumbnail(id)`. A pending image older than 10 minutes counts as gone. A newer image replaces it.
  Sending goes through `broadcast_local_clip`, so the report, history and the undelivered alert behave
  as for any explicit send.

### klip-daemon
- `capture_clipboard`: an image while `auto_images` is false is held (after the same size checks a
  receiver would apply) instead of sent. IPC handlers for the four requests; `SetAutoImages` also saves
  `config.json`.

### klip-cli
- `klip share` (send the held image, prints who got it), `klip images [auto|manual]` (show or set the
  mode), hidden `klip api`, `klip tray [run|install|uninstall|status]`. Help goldens are updated on
  purpose. The terminal app is untouched (its `config.json` writer keeps the new key).

### The bridge (`klip api`)
`{"id":1,"ipc":"GetStatus"}` -> `{"id":1,"ok":{"Status":{...,"peers":[{"fingerprint":"AJG6-...",...}]}}}`.
Other methods: `config_get`, `config_set`, `log_tail`, `daemon_start`, `daemon_restart`,
`daemon_stop`, `service_state`, `service_install`, `autostart_get`, `autostart_set`, `qr` (modules of a
QR code for a ticket), `history_text` (one text item, for "reveal"), `version`. Requests run
concurrently (a pairing join can wait 90 s while status polls continue). EOF on stdin ends it.

### The tray app (`extra/tray/klip_tray`)
- `model.py` (no GTK): state from replies, tray state (stopped / waiting / connected / paused), texts
  ("12 min", plain-language errors), history rows, validation. Fully unit-tested without a display.
- `bridge.py`: starts `klip api`, sends/receives lines on worker threads, delivers results to the GTK
  main loop, restarts the child if it dies, never blocks the UI.
- `icons.py`, `theme.py`: SVG icons for the four states drawn from the brand mark (visible on dark and
  light panels), GTK CSS.
- `pages.py`, `app.py`: `Gtk.Application` (single instance), `XApp.StatusIcon` (left click toggles the
  window, right click menu, middle click sends the held image), the window (422 x 600, rounded, shadow,
  focus-out hides) with pages Home, Devices, Pair, History, Send, Log, Settings, and a toast overlay.
- Polling: status every 3 s while the window is visible, 15 s while hidden; history/log only while
  their page is visible; thumbnails fetched once per row and cached.
- Errors never reach the user as a traceback: every callback is wrapped; failures become a toast and a
  line in `tray.log` (0600, in the Klip directory).

## Errors and edge cases

- Daemon not running at login: the icon shows "stopped"; the menu and Home offer Start; polling
  recovers by itself when it comes up.
- `python3-gi`/XApp missing: `klip tray` explains what to install and exits 1.
- Two trays: the second asks the first to show its window and exits.
- Pairing ticket is a secret: shown only on the pair page; leaving the page or hiding the window cancels
  the pairing (as the terminal app does).
- A request while the daemon is old (no `share_image`): the tray shows "restart Klip to use this".
- Held image bigger than the receiver's limit: send reports the refusal per device.

## Tests

1. Protocol: golden bytes for the new variants; old goldens unchanged.
2. Core/daemon: manual mode holds, auto mode sends, replace, expiry, send reports, toggle persists to
   `config.json`, thumbnail size, history untouched by held images.
3. CLI: `klip share`, `klip images`, help goldens; `klip api` against a real daemon (JSON in and out,
   fingerprint added, contents removed, concurrent requests).
4. Tray installer: files written once per version, autostart `.desktop` content, uninstall removes only
   what it created.
5. Python: unit tests for `model.py`/`bridge.py` (fake child process); GTK tests under a nested X server
   (`Xephyr`) with offscreen rendering of every page to PNG (looked at, not just executed).
6. Integration: two real isolated daemons + the real bridge + the real Python controller: pair, send,
   hold/share an image, toggle, history, unpair.
7. On the author's desktop: the tray starts, registers, opens its window (checked by window title) and
   survives a daemon restart. Clicking the icon is the author's check.

## Not verified until built

How the icon looks on the author's real panel, mouse interaction with the real window, other desktops.
