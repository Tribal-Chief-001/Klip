# The tray in Rust (no Python, no GTK)

Status: written 2026-10-15 on the owner's request ("remove Python, write it in Rust, end to end, no
regressions, as small a footprint as you can"). Decisions the owner would normally make are **Ruling**.

## Problem
The Python/GTK tray costs 27-37 MB of real RAM (the floor of Python + GTK3 alone is 18 MB), needs
python3-gi at runtime, embeds ~40 KB of scripts in the binary, and needs a second process (`klip api`)
to speak to the daemon. The owner wants the same icon and menu with the smallest possible footprint.

## Success criteria
1. `klip tray` (start/run/install/uninstall/status) works as before: same autostart entry, same single
   instance, same menu: state, Start Klip (only when stopped), send the last copied image, send images
   automatically (tick), pause/resume, open the terminal app, quit; left AND right click open it,
   middle click sends the waiting image; the icon shows the state; failures are a desktop notification.
2. No Python, no GTK, no `klip api` bridge. One process, talks to the daemon in-process with the
   existing `DaemonClient`.
3. Resident memory a fraction of the Python tray; idle CPU ~0; no polling faster than every 15 s.
4. Nothing existing breaks: every Rust test that passes today still passes, except the tests of the
   removed bridge/Python files; the daemon protocol is untouched.

## Decisions
- **StatusNotifierItem over D-Bus with the `ksni` crate (Linux only).** Cinnamon's `xapp-sn-watcher`
  (running on the owner's machine) hosts such items next to Electron/Blueman icons. Rejected: speaking
  the XApp-private `org.x.StatusIcon` protocol by hand (more code, no benefit), keeping GTK (the cost).
- **Ruling: the tray lives in the `klip` binary as `klip tray run`.** `klip tray start` spawns it
  detached. The login entry keeps its command (`klip tray run`), so upgrading needs no re-install.
- **Ruling: the `klip api` JSON bridge is removed.** Only the Python tray used it; keeping it would be
  dead code in the binary.
- **Ruling: no `ksni` on macOS/Windows.** The module is Linux-only; elsewhere `klip tray` says so.
- **Menu logic is a pure function** (`model::menu_plan`) tested without D-Bus; D-Bus is only the thin
  adapter. State comes from `GetStatus` + `GetImageState` (a daemon that does not know images simply
  hides the image items); no Hello round trip.
- **Icons**: monochrome symbolic SVGs (fills only) written to `~/.local/share/icons/hicolor/scalable/
  status/klip-<state>[-badge]-symbolic.svg` and referenced by name, so the panel paints them like its
  own icons. Same files as the Python version.
- **Polling** every 15 s, plus when the menu opens (`menu_about_to_show`) and after every action.
- Old Python files in `~/.local/share/klip/tray` are removed when the tray starts.

## Errors and edge cases
- Daemon down: icon "stopped", menu offers Start Klip, the poll recovers by itself.
- No StatusNotifierWatcher: ksni waits for one; after 30 s without a host a notification says so.
- Two trays: the second exits ("already running") through a lock file in the Klip directory.
- A failing action (refused send, unreachable daemon) is one notification; nothing panics.

## Tests
1. Model: states, titles, names, plain errors, menu plan for every state (pure).
2. Icons: valid SVG, fills only, one shape per state, files written once.
3. Login entry/lock/legacy cleanup (existing tests ported).
4. Integration: a private `dbus-daemon`, a fake StatusNotifierWatcher, two real isolated daemons, the
   real `klip tray run`: read the item's properties, click menu entries through dbusmenu, and check
   the daemon's reaction (auto-images switch persisted, held image sent, pause).
5. On the owner's desktop: registered with `xapp-sn-watcher`, memory measured.

## Not verified until built
How the icon and menu look on the real panel; other desktops.
