# macOS and Windows: the tray, and the Windows control channel

Status: written 2026-10-15. The owner asked for "the next platforms, with the smallest footprint, end
to end, production hardened, no regressions, all the skill steps, without asking" and said they can
test later but cannot test now. Decisions the owner would normally make are marked **Ruling**.

## Understanding (what was asked, what was assumed)
Asked: a tray icon and menu on macOS and Windows like the Linux one, as light as possible, done
properly. Assumed: the owner has no Windows/macOS machine at hand, so the only test beds are the
GitHub-hosted `macos-latest` and `windows-latest` runners and Linux unit tests; success is "built,
tested as far as CI allows, honestly labelled", never "seen working".

## Findings from reading the code (these change the plan)
- **Windows cannot be controlled at all today.** The daemon's control server is an empty task on
  non-Unix (`engine.rs`), and the client answers "Unix socket not supported" (`client.rs`). So
  `klip status|pair|send|share` and any tray would fail. The terminal app (`klip-tui`) and the CLI
  command layer are `cfg(unix)` too.
- macOS already has the Unix-socket control channel, `klip install-service` (launchd) and a clipboard
  driver (`arboard`), all never run by the owner.
- The tray's logic (state, menu, polling, actions) is not Linux-specific, but it is welded to ksni in
  `tray/app.rs`.

## Success criteria
1. macOS: `klip tray [start|run|install|uninstall|status]` shows a menu-bar icon with the same menu
   (left click, right click; middle click sends the image), starts at login, one instance, no Dock icon.
2. Windows: the same in the notification area, plus a working control channel so the CLI, the terminal
   app and the tray can talk to the daemon.
3. Linux behaviour unchanged (its five D-Bus end-to-end tests keep passing).
4. Footprint stays small: no GTK/Python/webview; the new dependencies compile only on macOS/Windows;
   the Linux binary does not grow.
5. Every claim in the release notes is backed by a test that ran, or is listed as NOT verified.

## Decisions
- **Ruling: work on branch `platforms`, merge to `main` and tag `v0.7.0-alpha` only when CI is green**
  (CI is the only test bed, and `main` is what the owner's devices build from).
- **Ruling: `tray-icon` + `tao` on macOS and Windows only; ksni stays on Linux.** Earlier I proposed
  hand-written Win32/AppKit code. Nothing here can be seen working before release, so the quirks
  (popup-menu focus on Windows, accessory activation policy and template images on macOS, main-thread
  rules) are left to a library thousands of apps already use. Cost: a larger Windows/macOS binary
  (not measured, not budgeted); the memory is measured on the runners and reported, not promised.
- **Shared controller, per-OS presentation.** `tray/controller.rs` (poll, fetch, actions, busy guard,
  problem line) is moved out of `app.rs` and talks through two channels: commands in, `Out::Summary` /
  `Out::Problem` out. Linux/ksni, macOS and Windows each implement only the presentation.
- **Menu ids are strings** (`Action::id()` / `Action::from_id()`), round-trip tested, so every OS maps a
  click back to an action the same way.
- **Icons are drawn at run time** (`tray/raster.rs`, pure RGBA, 4x4 supersampling) from the same shapes
  as the Linux SVGs: macOS gets a black template image (the system tints it), Windows a white or black
  glyph chosen from the system theme. Tested on Linux by pixel checks and by looking at a rendered PNG.
- **Failures:** Linux keeps `notify-send`; macOS and Windows show the problem as the first (disabled)
  line of the menu and in the tooltip for 30 s, so no notification dependency is needed.
- **Autostart:** macOS `~/Library/LaunchAgents/com.klip.tray.plist` (RunAtLoad, written atomically,
  marker-checked like the Linux entry); Windows `HKCU\...\Run` value `KlipTray` through `reg.exe`
  (no FFI). Single instance: the existing `TrayLock` file lock (std locks work on Windows).
- **Open the terminal app:** macOS `osascript` tells Terminal to run `klip` (path validated, no quotes);
  Windows `cmd /c start "Klip" <exe>`.
- **Windows control channel: a named pipe** `\\.\pipe\klip-<hash of the Klip directory>` (so
  `KLIP_CONFIG_DIR` isolation keeps working), same length-prefixed postcard framing as the Unix
  socket, `reject_remote_clients(true)`, and a DACL that grants only the owner (`D:P(A;;GA;;;OW)`),
  because a default pipe could be read by other local users (clip history is on it).
- **Windows terminal app / CLI:** the `cfg(unix)` gates in `klip-cli` and `klip-tui` are relaxed where
  the code is portable (crossterm/ratatui are); what does not compile or run on Windows is cut and
  listed, never hidden.
- **Windows console flash:** `klip tray start` spawns the tray with `CREATE_NO_WINDOW`; the login entry
  runs `klip tray start`, so a console may flash once at login (documented, not hidden).

## Units
`tray/model.rs` (pure) · `tray/raster.rs` (pure) · `tray/controller.rs` (async, no UI) ·
`tray/linux.rs` (ksni, was `app.rs`) · `tray/desktop.rs` (tao + tray-icon, macOS/Windows) ·
`tray/autostart.rs` (per OS) · `tray/mod.rs` (lock, run, start, terminal) ·
`klip-daemon/src/ipc_pipe.rs` and `klip-cli/src/client.rs` Windows paths.

## Phases (each ends green on its own and can ship alone)
0. Refactor on Linux: controller extracted, `Action` ids, raster icons, autostart module. Gate: all
   existing tests including the five D-Bus tests.
1. macOS tray + LaunchAgent + CI job on `macos-latest` (unit tests; a 10 s smoke start of the tray).
2. Windows control channel + enabling the portable CLI/TUI + CI job on `windows-latest` running a real
   daemon end to end (start, status, pair two daemons, send text, history, stop).
3. Windows tray + autostart + CI smoke.
4. Docs, release notes with a NOT-verified list, version 0.7.0, release.

## Errors and edge cases
Daemon down: stopped icon, "Start Klip". No display/shell (headless runner, or Explorer not running):
the tray exits with a clear message instead of a panic. Pipe busy: the client retries briefly.
A second tray or daemon: refused by the file locks. A foreign login entry is never overwritten.

## Not verifiable here (will be stated in the release notes)
How the icons look in the macOS menu bar and the Windows tray (light/dark/HiDPI), real clicks, the
clipboard on a real desktop session of either OS, and memory use outside a CI runner.
