# Undelivered-copy alert and "offline for" status

Status: design approved in chat 2026-10-14; this file awaits written-spec review. No code yet.

## Problem

Klip needs both devices online at the same time (owner decision, no mailbox). The worst real failure
is silent: the owner copies on the laptop, pastes on the phone and gets the old content, because the
phone app was killed (for example by the Vivo battery manager) or the phone has no network. Today
the information exists (`SyncCoordinator::last_errors`, the peer `connected` flag) but is only visible
if the owner opens `klip status`, the terminal app, or the phone notification, and none of them says
for how long a device has been gone.

## Success criteria

1. When an automatic copy cannot reach a paired device, the owner sees one desktop notification
   naming the device and how long it has been disconnected, at the moment of the failed copy.
2. The phone's always-on notification says how long the laptop has been missing ("Waiting for X,
   12 min").
3. No notification noise: at most one alert per device per outage; nothing for devices that are
   merely offline while nobody copies; no alert for explicit sends (they already report errors).
4. Clip content never appears in any notification.

## Decisions (and what was rejected)

- Trigger = a failed delivery of an automatic copy, not a "device offline for N minutes" timer. A
  timer fires every night the laptop is closed and teaches the owner to ignore it.
- Only "could not reach" failures count (`NetError::Connect`, `NetError::Stream`). `PeerRefused`
  (paused, clipboard unavailable, superseded) has its own meaning and its own message today.
- No grace period. A copy that could not be delivered (after the fresh-connection retry) is a fact, not
  a guess, and the owner should re-copy it. A grace would hide exactly the first minute after a phone
  dies. A Wi-Fi hiccup therefore alerts once, with "not connected right now" instead of a duration;
  the once-per-outage rule keeps this quiet.
- One alert per outage: the flag is cleared when a delivery to that device succeeds or when the device
  reconnects (`NetEvent::PeerConnected`), so a device that comes back and dies again alerts again even
  if nobody copied in between (found in the final review).
- No setting, no sound, no phone-side alert, no outage history, no macOS/Windows support, no TUI/CLI
  duration column. Each of these is cheap to add later and none is needed for the problem above.
- No change to the IPC protocol (`PeerStatusInfo`, `ConfigInfo` stay as they are): the alert is raised
  inside the daemon process, and the Android duration travels through the UniFFI `PeerInfo` record,
  which is built together with the app.

## Design

### klip-net (`node.rs`)
- `Inner` gets `last_connected: Mutex<HashMap<EndpointId, Instant>>`, initialised to the node start
  time for every paired peer, set in `register_connection` and refreshed by the 5 s maintainer tick
  for every peer that is connected.
- `PeerStatus` gets `offline_for: Option<Duration>`: `None` while connected, otherwise
  `now - last_connected`. Removing a peer (`unpair`) removes its entry.
- Presence is taken from connection state, not from `PeerDisconnected` events: `drop_connection`
  removes a connection without raising that event, so the events are not reliable.

### klip-core (`sync.rs`)
- New `type UndeliveredHook = Arc<dyn Fn(&str, Duration) + Send + Sync>` and
  `SyncCoordinator::set_undelivered_hook(&self, hook)`; `pub fn is_sending()` (true while a send is in flight,
  for tests) and `pub fn offline_label(Duration) -> Option<String>` ("5 min", "2 h", `None` under a
  minute), shared by the daemon and the Android library.
- `deliver(&payload, notify: bool)`: `drain_outbox` passes `true`, `broadcast_local_clip` passes
  `false`. Per peer result: `Ok` clears the peer's `alerted` flag; `Err(Connect | Stream)` with the
  flag unset sets the flag and calls the hook with the peer name and `offline_for`. The check uses the `NetError` before it is turned into a string.
- `alerted: Mutex<HashSet<NodeId>>`. An unpaired peer is removed from it.

### klip-daemon (new `notify.rs`) and klip-cli
- The `klip` binary installs, before `run()`, a hook that runs `notify-send` (Linux only,
  `cfg(target_os = "linux")`; not installed by the engine, so tests never pop real notifications):
  fixed title `Klip`, body `"<name> did not get your last copy. Not connected for <N> min."` (or "It is not connected right
  now." under a minute),
  arguments passed as argv (no shell, `--` before the positionals), `<`, `>`, `&` escaped in the name, spawned detached and
  never awaited. A missing `notify-send` or no display is ignored.

### klip-android (`lib.rs`) and the app
- `PeerInfo` gets `offline: Option<String>` (the `offline_label`), built from `PeerStatus::offline_for`; `KlipEngine::status()` also calls
  `core.sync.net.peer_statuses()` and matches by node id, so `PeerStatusInfo` does not change.
- `StatusText.compute()`: when no device is online, the device line (and so the notification `key`) and
  the compact line append the coarse duration (the title stays as it is) ("5 min", "2 h"; under one minute nothing). Coarse so the notification text (and its
  `key`) changes at most once a minute while the screen is on; the existing 8 s ticker and the
  `SCREEN_ON` refresh already update it.

## Errors and edge cases

- Fresh daemon start: peers count as "seen" at start, so a first failure says "not connected right now".
- Laptop suspend: `Instant` does not advance during suspend, so durations are understated, never
  overstated; after resume the first failed copy alerts.
- Several devices down: one alert per device, each with its own name.
- Peer unpaired meanwhile: no alert (`UnknownPeer` is not counted).
- Device name from a peer is already control-character-stripped and limited to 64 characters
  (`sanitize_name`); escaping is for notification markup only.

## Tests

1. Core/daemon e2e: two paired daemons, stop one, send two copies: the hook is called exactly once
   with the right name; restart it, one delivery succeeds, stop it again, a copy: called again.
2. Pure unit tests: one alert per outage is re-armed by a success; only `Connect`/`Stream` errors count
   (not `PeerRefused`, `UnknownPeer`, `TooLarge`); `offline_label` boundaries (59 s, 60 s, 119 min,
   120 min).
3. `klip send` (explicit) to a down peer does not call the hook.
4. `notify-send` argument building: name `<b>x</b> & y` is escaped, `--` comes before the positionals,
   a missing `notify-send` is ignored.
5. Android: the text is formatted in Rust (`offline_label`, tested above); the Kotlin side only appends
   it, so it is verified by compiling and reported as not verified on a phone.
6. Existing suites stay green (291 tests); no snapshot changes expected.

## Not verified until built

Real desktop notification on this Cinnamon session, the phone notification wording on the Vivo, and
behaviour across a real laptop suspend.

## Size

About 150 lines of code and 100 lines of tests; no new dependency.
