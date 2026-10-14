# Undelivered-copy alert Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Tell the owner, at the moment it happens, that an automatic copy did not reach a paired device (desktop notification on Linux) and show on the phone how long the laptop has been missing.

**Architecture:** `klip-net` records when each peer was last seen connected and exposes `offline_for`. `klip-core` raises a hook once per outage when an automatic send fails with an unreachable error. The `klip` binary installs a `notify-send` hook; the Android library passes a ready-made label to the Kotlin status text.

**Tech Stack:** Rust 2021 workspace (tokio, iroh), UniFFI, Kotlin framework views, `notify-send`.

**Spec:** `docs/superpowers/specs/2026-10-14-undelivered-alert-design.md`

## Global Constraints

- No new dependency (Cargo or Gradle). No change to the IPC types (`PeerStatusInfo`, `ConfigInfo`, `IpcRequest`, `IpcResponse`).
- No setting, no sound, no outage history, no macOS/Windows notification code.
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings` stay clean.
- Notification text contains the device name and a duration only, never clip content.
- Heavy commands one at a time (7.6 GB RAM machine): use `-j2`, `nice -n 10`, `./gradlew --stop` after Gradle.
- Commits use the repo's ledger style (Why / inventory / verification) and end with the Co-Authored-By trailer.

## Review Focus

- A device unpaired while a send is in flight: no alert, no panic (`UnknownPeer` is not unreachable). Pinned in Task 3.
- Device name containing markup or a leading dash (`<b>x</b> & y`, `-h`): safe single argv item, escaped, after `--`. Pinned in Task 4.
- `notify-send` missing or no display: the daemon carries on. Pinned in Task 4.
- Burst of copies while the device is down: exactly one alert, not one per copy. Pinned in Task 4 (e2e).
- An explicit `klip send` to a down device: reports its own error, raises no alert and does not use up the outage's alert. Pinned in Task 4 (e2e).

---

### Task 1: Commit the amended spec and this plan

**Files:**
- Modify: `docs/superpowers/specs/2026-10-14-undelivered-alert-design.md` (already amended: grace period removed, `offline_label`, hook installed by the CLI)
- Create: `docs/superpowers/plans/2026-10-14-undelivered-alert.md`

- [ ] **Step 1: Commit and push**

```bash
git add docs/superpowers/specs/2026-10-14-undelivered-alert-design.md docs/superpowers/plans/2026-10-14-undelivered-alert.md
git commit -F - <<'EOF'
docs(plan): implementation plan for the undelivered-copy alert; spec drops the grace period

Why / Context
While turning the spec into code the 60 s grace turned out to be wrong: it would hide exactly the
first minute after a phone dies, and a failed copy (after the fresh-connection retry) is a fact.

Detailed inventory
- Spec: no grace; offline_label shared by daemon and Android; hook installed by the klip binary.
- Plan: six tasks (klip-net presence, klip-core hook, daemon notify, Android, ship).

Verification / Impact
Documentation only.

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>
EOF
git push origin main
```

---

### Task 2: klip-net remembers when each peer was last connected

**Files:**
- Modify: `crates/klip-net/src/node.rs` (struct `PeerStatus` ~line 102, struct `Inner` ~line 156, `start` ~line 221, `peer_statuses` ~line 251, `unpair` ~line 295)
- Test: `crates/klip-net/tests/net_flow.rs`

**Interfaces:**
- Produces: `PeerStatus.offline_for: Option<Duration>` (`None` while connected, else time since the peer was last seen connected; a never-seen peer counts as seen at the first `peer_statuses` call).

- [ ] **Step 1: Write the failing test** (append to `net_flow.rs`)

```rust
#[tokio::test]
async fn offline_for_is_none_while_connected_and_counts_up_after_the_peer_goes_away() {
    let host = spawn_node("Laptop", true, None).await;
    let phone = spawn_node("Phone", true, None).await;
    pair(&host, &phone).await;
    // A delivered clip proves the connection is up.
    let res = host.node.send_clip(&clip_from(&host.node, "hi")).await;
    assert!(res.iter().all(|(_, r)| r.is_ok()), "{res:?}");
    let st = host.node.peer_statuses();
    assert!(st[0].connected && st[0].offline_for.is_none(), "{st:?}");

    phone.node.shutdown().await;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while host.node.peer_statuses()[0].connected && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let st = host.node.peer_statuses();
    assert!(!st[0].connected, "peer should be seen as gone: {st:?}");
    assert!(st[0].offline_for.is_some());
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(host.node.peer_statuses()[0].offline_for.unwrap() >= Duration::from_secs(1));
}
```

- [ ] **Step 2: Run it, expect a compile error** (`no field offline_for`)

Run: `nice -n 10 cargo test -p klip-net --test net_flow offline_for -j2`

- [ ] **Step 3: Implement**

In `PeerStatus` add (after `problem`):

```rust
    /// How long since this peer was last seen connected; `None` while it is connected.
    pub offline_for: Option<Duration>,
```

In `Inner` add after `problems`:

```rust
    /// When each peer was last seen connected (refreshed whenever `peer_statuses` runs, which the
    /// 5 s maintainer tick does). Connection state is used, not `PeerDisconnected` events:
    /// `drop_connection` removes a connection without raising one.
    last_seen: Mutex<HashMap<EndpointId, Instant>>,
```

In `start`, in the `Inner { .. }` literal after `problems: ...`:

```rust
            last_seen: Mutex::new(HashMap::new()),
```

Replace the body of `peer_statuses`:

```rust
    pub fn peer_statuses(&self) -> Vec<PeerStatus> {
        let conns = self.inner.conns.lock().unwrap();
        let problems = self.inner.problems.lock().unwrap();
        let mut seen = self.inner.last_seen.lock().unwrap();
        let now = Instant::now();
        self.peers()
            .into_iter()
            .map(|p| {
                let connected = conns
                    .get(&p.id)
                    .map(|c| c.close_reason().is_none())
                    .unwrap_or(false);
                // A peer never seen yet (daemon start, fresh pairing) counts as seen now.
                let last = seen.entry(p.id).or_insert(now);
                if connected {
                    *last = now;
                }
                PeerStatus {
                    connected,
                    offline_for: (!connected).then(|| now.duration_since(*last)),
                    problem: problems.get(&p.id).cloned(),
                    id: p.id,
                    name: p.name,
                }
            })
            .collect()
    }
```

In `unpair`, after the `problems` removal line add:

```rust
        self.inner.last_seen.lock().unwrap().remove(id);
```

- [ ] **Step 4: Run, expect PASS**

Run: `nice -n 10 cargo test -p klip-net -j2 -- --test-threads=2`
Expected: all klip-net tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/klip-net && git commit -m "feat(net): peer_statuses reports how long a peer has been gone" -m "Why: the undelivered-copy alert needs a duration. Adds PeerStatus.offline_for from a last_seen map kept in Inner; test: peer shutdown. Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 3: klip-core raises the hook once per outage

**Files:**
- Modify: `crates/klip-core/src/sync.rs` (imports; new items before `struct PendingPair`; struct `SyncCoordinator` and `new`; `broadcast_local_clip`, `drain_outbox`, `deliver`; `unpair`)
- Modify: `crates/klip-core/src/lib.rs` (exports)
- Test: `crates/klip-core/src/sync.rs` (new `#[cfg(test)] mod tests` at the end)

**Interfaces:**
- Consumes: `PeerStatus.offline_for` (Task 2).
- Produces: `pub type UndeliveredHook = Arc<dyn Fn(&str, Duration) + Send + Sync>`; `SyncCoordinator::set_undelivered_hook(&self, UndeliveredHook)`; `SyncCoordinator::is_sending(&self) -> bool`; `pub fn offline_label(Duration) -> Option<String>`.

- [ ] **Step 1: Write the failing tests** (append to `sync.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_label_is_blank_under_a_minute_then_minutes_then_hours() {
        assert_eq!(offline_label(Duration::from_secs(59)), None);
        assert_eq!(offline_label(Duration::from_secs(60)).as_deref(), Some("1 min"));
        assert_eq!(offline_label(Duration::from_secs(119 * 60)).as_deref(), Some("119 min"));
        assert_eq!(offline_label(Duration::from_secs(120 * 60)).as_deref(), Some("2 h"));
    }

    #[test]
    fn only_could_not_reach_errors_count_as_unreachable() {
        assert!(is_unreachable(&NetError::Connect("x".into())));
        assert!(is_unreachable(&NetError::Stream("x".into())));
        assert!(!is_unreachable(&NetError::PeerRefused("paused".into())));
        assert!(!is_unreachable(&NetError::UnknownPeer));
        assert!(!is_unreachable(&NetError::TooLarge { size: 2, limit: 1 }));
    }

    #[test]
    fn one_alert_per_outage_and_a_success_rearms_it() {
        let node = NodeId::new([9u8; 32]);
        let mut alerted = HashSet::new();
        let fail: Result<(), NetError> = Err(NetError::Connect("down".into()));
        let ok: Result<(), NetError> = Ok(());
        let refused: Result<(), NetError> = Err(NetError::PeerRefused("paused".into()));
        assert!(first_failure_of_outage(&mut alerted, node, &fail));
        assert!(!first_failure_of_outage(&mut alerted, node, &fail));
        assert!(!first_failure_of_outage(&mut alerted, node, &refused));
        assert!(!first_failure_of_outage(&mut alerted, node, &ok));
        assert!(first_failure_of_outage(&mut alerted, node, &fail));
    }
}
```

- [ ] **Step 2: Run, expect compile errors** (`offline_label`, `is_unreachable`, ... not found)

Run: `nice -n 10 cargo test -p klip-core --lib sync::tests -j2`

- [ ] **Step 3: Implement**

Imports at the top of `sync.rs`: change `use klip_net::{ClipOutcome, EndpointId, KlipNode, NetEvent, PairDecision, PeerRecord};` to also import `NetError`, and add `use std::collections::HashSet;` and `use std::time::Duration;` (merge with the existing `std::time::{Instant, SystemTime, UNIX_EPOCH}` import: `use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};`).

Add before `struct PendingPair`:

```rust
/// Called, outside any lock, when an automatic copy could not reach a device: its name and how long
/// it has been gone. Only the name and a duration: never clip content.
pub type UndeliveredHook = Arc<dyn Fn(&str, Duration) + Send + Sync>;

/// "5 min" / "2 h" for how long a device has been gone; `None` under a minute.
pub fn offline_label(gone: Duration) -> Option<String> {
    let mins = gone.as_secs() / 60;
    match mins {
        0 => None,
        1..=119 => Some(format!("{mins} min")),
        _ => Some(format!("{} h", mins / 60)),
    }
}

fn is_unreachable(e: &NetError) -> bool {
    matches!(e, NetError::Connect(_) | NetError::Stream(_))
}

/// One alert per outage: true for the first unreachable failure since the last success (or ever).
fn first_failure_of_outage(
    alerted: &mut HashSet<NodeId>,
    node: NodeId,
    result: &Result<(), NetError>,
) -> bool {
    match result {
        Ok(()) => {
            alerted.remove(&node);
            false
        }
        Err(e) if is_unreachable(e) => alerted.insert(node),
        Err(_) => false,
    }
}
```

In `struct SyncCoordinator` add fields (after `coalesced_clips`):

```rust
    undelivered_hook: Mutex<Option<UndeliveredHook>>,
    /// Devices already alerted about in their current outage.
    alerted: Mutex<HashSet<NodeId>>,
```

In `new`, in the `Self { .. }` literal add:

```rust
            undelivered_hook: Mutex::new(None),
            alerted: Mutex::new(HashSet::new()),
```

Add methods (next to `pause`/`resume` group, e.g. after `pause_remaining_secs`):

```rust
    pub fn set_undelivered_hook(&self, hook: UndeliveredHook) {
        *self.undelivered_hook.lock().unwrap() = Some(hook);
    }

    /// True while a send of an automatic copy is in flight.
    pub fn is_sending(&self) -> bool {
        self.sending.load(Ordering::SeqCst)
    }
```

Change `broadcast_local_clip`'s last line to `Some(self.deliver(&payload, false).await)` and `drain_outbox`'s `self.deliver(&payload).await;` to `self.deliver(&payload, true).await;`.

Replace `deliver` with:

```rust
    /// Send one prepared clip to every peer and record the per-peer outcome. `notify` is true for
    /// automatic copies: an unreachable device then raises the undelivered hook (once per outage).
    async fn deliver(&self, payload: &ClipPayload, notify: bool) -> Vec<DeliveryReport> {
        let results = self.net.send_clip(payload).await;
        let names: HashMap<NodeId, String> = self
            .net
            .peers()
            .into_iter()
            .map(|p| (node_of(&p.id), p.name))
            .collect();
        let gone: HashMap<NodeId, Duration> = if notify {
            self.net
                .peer_statuses()
                .into_iter()
                .filter_map(|s| s.offline_for.map(|d| (node_of(&s.id), d)))
                .collect()
        } else {
            HashMap::new()
        };

        let mut to_alert: Vec<(String, Duration)> = Vec::new();
        let mut errors = self.last_errors.lock().unwrap();
        let mut reports = Vec::new();
        for (id, res) in results {
            let node = node_of(&id);
            let peer_name = names.get(&node).cloned().unwrap_or_else(|| node.short_id());
            {
                let mut alerted = self.alerted.lock().unwrap();
                if notify {
                    if first_failure_of_outage(&mut alerted, node, &res) {
                        let off = gone.get(&node).copied().unwrap_or_default();
                        to_alert.push((peer_name.clone(), off));
                    }
                } else if res.is_ok() {
                    alerted.remove(&node); // an explicit send that worked also ends the outage
                }
            }
            match res {
                Ok(()) => {
                    errors.remove(&node);
                    reports.push(DeliveryReport {
                        peer_name,
                        result: Ok(()),
                    });
                }
                Err(e) => {
                    let msg = e.to_string();
                    errors.insert(node, msg.clone());
                    reports.push(DeliveryReport {
                        peer_name,
                        result: Err(msg),
                    });
                }
            }
        }
        drop(errors);
        // "Last sync" means a peer really accepted something, not that we merely saw a local copy.
        if reports.iter().any(|r| r.result.is_ok()) {
            self.record_sync();
        }
        let hook = self.undelivered_hook.lock().unwrap().clone();
        if let Some(hook) = hook {
            for (name, off) in to_alert {
                hook(&name, off);
            }
        }
        reports
    }
```

In `unpair`, inside the `[one] =>` arm after `self.net.unpair(&one.id);` add:

```rust
                self.alerted.lock().unwrap().remove(&node);
```

In `lib.rs` change the sync export to:

```rust
pub use sync::{node_of, offline_label, DeliveryReport, SyncCoordinator, UndeliveredHook};
```

- [ ] **Step 4: Run, expect PASS**

Run: `nice -n 10 cargo test -p klip-core -j2 -- --test-threads=2`

- [ ] **Step 5: Commit**

```bash
git add crates/klip-core && git commit -m "feat(core): undelivered hook, once per outage, for automatic copies" -m "Why: tell the owner when a copy did not reach a device. Adds UndeliveredHook, offline_label, is_sending; deliver(notify) raises the hook on the first Connect/Stream failure of an outage; a success re-arms it. Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 4: The `klip` binary shows a desktop notification

**Files:**
- Create: `crates/klip-daemon/src/notify.rs`
- Modify: `crates/klip-daemon/src/lib.rs` (add `#[cfg(target_os = "linux")] pub mod notify;`)
- Modify: `crates/klip-cli/src/main.rs` (in `start`, before `engine.run().await?;`)
- Test: `crates/klip-daemon/src/notify.rs` (unit), `crates/klip-daemon/tests/e2e.rs` (end to end)

**Interfaces:**
- Consumes: `klip_core::{offline_label, UndeliveredHook}`, `SyncCoordinator::set_undelivered_hook`, `SyncCoordinator::is_sending` (Task 3).
- Produces: `klip_daemon::notify::hook() -> UndeliveredHook`.

- [ ] **Step 1: Write `notify.rs` with its tests**

```rust
//! Desktop notification when an automatic copy could not reach a device (Linux: `notify-send`).
use klip_core::{offline_label, UndeliveredHook};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

fn escape_markup(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Only the device name and a duration: never clip content (notifications can show on a lock screen).
fn body(name: &str, gone: Duration) -> String {
    let name = escape_markup(name);
    match offline_label(gone) {
        Some(label) => format!("{name} did not get your last copy. Not connected for {label}."),
        None => format!("{name} did not get your last copy. It is not connected right now."),
    }
}

/// `--` comes first so a device name starting with `-` can never be read as an option.
fn args(name: &str, gone: Duration) -> Vec<String> {
    vec![
        "--app-name=Klip".into(),
        "--expire-time=10000".into(),
        "--".into(),
        "Klip".into(),
        body(name, gone),
    ]
}

/// Start the notifier and forget it; a missing program or no display is not an error.
fn run(program: &str, args: &[String]) {
    if let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

pub fn hook() -> UndeliveredHook {
    Arc::new(|name: &str, gone: Duration| run("notify-send", &args(name, gone)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_in_a_device_name_is_escaped_and_a_dash_cannot_become_an_option() {
        let a = args("-h <b>x</b> & y", Duration::from_secs(5));
        let dashdash = a.iter().position(|x| x == "--").unwrap();
        assert_eq!(dashdash, 2, "options first, then --");
        assert!(a[..dashdash].iter().all(|x| !x.contains("-h")));
        assert_eq!(a.len(), 5, "name and text stay one argv item");
        assert_eq!(
            a[4],
            "-h &lt;b&gt;x&lt;/b&gt; &amp; y did not get your last copy. It is not connected right now."
        );
    }

    #[test]
    fn the_text_says_how_long_once_it_is_over_a_minute() {
        assert!(body("Pixel", Duration::from_secs(14 * 60)).ends_with("Not connected for 14 min."));
        assert!(body("Pixel", Duration::from_secs(3 * 3600)).ends_with("Not connected for 3 h."));
    }

    #[test]
    fn a_missing_notifier_is_ignored() {
        run("klip-no-such-program-xyz", &[]);
    }
}
```

- [ ] **Step 2: Wire the module and the hook**

`crates/klip-daemon/src/lib.rs`: add above `pub mod services;`

```rust
#[cfg(target_os = "linux")]
pub mod notify;
```

`crates/klip-cli/src/main.rs`, in `start`, just before `engine.run().await?;`:

```rust
        #[cfg(target_os = "linux")]
        engine.sync.set_undelivered_hook(klip_daemon::notify::hook());
```

- [ ] **Step 3: Run the unit tests, expect PASS**

Run: `nice -n 10 cargo test -p klip-daemon --lib notify -j2`

- [ ] **Step 4: Write the failing e2e test** (append to `crates/klip-daemon/tests/e2e.rs`)

```rust
#[tokio::test]
async fn an_automatic_copy_that_cannot_reach_a_device_alerts_once_per_outage() {
    use std::sync::atomic::Ordering::SeqCst;
    let a = start_daemon("Alpha", fresh_dir("a-alert"), false).await;
    let b = start_daemon("Beta", fresh_dir("b-alert"), false).await;
    assert!(matches!(
        pair(&a, &b, true).await,
        IpcResponse::PairSuccess { .. }
    ));
    let alerts: Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let seen = Arc::clone(&alerts);
    a.engine.sync.set_undelivered_hook(Arc::new(
        move |name: &str, _gone: Duration| seen.lock().unwrap().push(name.to_string()),
    ));
    assert!(wait_until(15, || a.engine.sync.peer_status().iter().all(|p| p.is_online)).await);
    b.stop().await;
    assert!(wait_until(15, || a.engine.sync.peer_status().iter().all(|p| !p.is_online)).await);

    // An explicit send reports its own error and raises no alert.
    let (ok, bad) = sent_report(
        a.ipc(IpcRequest::SendClip {
            text: "explicit".into(),
        })
        .await,
    );
    assert!(ok.is_empty() && bad.len() == 1, "{ok:?} {bad:?}");
    assert!(alerts.lock().unwrap().is_empty());

    // The first automatic copy alerts, with the device name only.
    a.clipboard.user_copy("one");
    assert!(
        wait_until(40, || !alerts.lock().unwrap().is_empty()).await,
        "no alert for a copy that could not be delivered"
    );
    // A burst of further copies in the same outage stays quiet.
    a.clipboard.user_copy("two");
    assert!(wait_until(40, || a.engine.sync.sends_started.load(SeqCst) >= 2).await);
    a.clipboard.user_copy("three");
    assert!(wait_until(60, || a.engine.sync.sends_started.load(SeqCst) >= 3).await);
    assert!(wait_until(60, || !a.engine.sync.is_sending()).await);
    assert_eq!(*alerts.lock().unwrap(), vec!["Beta".to_string()]);
    a.stop().await;
}
```

- [ ] **Step 5: Run it, expect PASS** (Tasks 2-3 provide the behaviour; before them it fails to compile)

Run: `nice -n 10 cargo test -p klip-daemon --test e2e an_automatic_copy -j2`
If a failed dial takes ~10 s the test needs up to ~40 s; that is expected. If `sends_started` never reaches 3 because "two" and "three" were coalesced, lower the third wait to `>= 2` and keep the `is_sending` wait.

- [ ] **Step 6: Commit**

```bash
git add crates/klip-daemon crates/klip-cli && git commit -m "feat(daemon): desktop notification when a copy does not reach a device" -m "Why: silent failures. notify-send via argv (no shell), -- before positionals, markup escaped; installed by the klip binary only so tests never pop notifications; e2e: one alert for a burst, none for an explicit send. Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Android shows how long the laptop has been missing

**Files:**
- Modify: `crates/klip-android/src/lib.rs` (`PeerInfo`, `KlipEngine::status`)
- Modify: `android/app/src/main/java/com/klip/app/StatusText.kt`

**Interfaces:**
- Consumes: `klip_core::{node_of, offline_label}`, `PeerStatus.offline_for` (Tasks 2-3).
- Produces: UniFFI `PeerInfo.offline: Option<String>` (Kotlin `offline: String?`).

- [ ] **Step 1: Rust side**

In `PeerInfo` add after `last_error`:

```rust
    /// How long the device has been gone ("5 min", "2 h"); `None` while connected or under a minute.
    pub offline: Option<String>,
```

In `KlipEngine::status`, after `let s = self.core.sync.status("Android".into(), 0);` add:

```rust
        let offline: std::collections::HashMap<_, _> = self
            .core
            .sync
            .net
            .peer_statuses()
            .into_iter()
            .filter_map(|p| {
                let label = p.offline_for.and_then(klip_core::offline_label)?;
                Some((klip_core::node_of(&p.id), label))
            })
            .collect();
```

and in the `PeerInfo { .. }` literal add `offline: offline.get(&p.node_id).cloned(),` (compute it before `p.node_id` is moved: the literal already uses `p.node_id.fingerprint()` first, so read `offline.get(&p.node_id)` into a local before the struct, e.g. `let off = offline.get(&p.node_id).cloned();` as the first line of the closure body).

- [ ] **Step 2: Kotlin side** (`StatusText.compute`)

Replace the peers line in `lines`:

```kotlin
            lines += st.peers.joinToString("   ") {
                (if (it.online) "● " else "○ ") + it.name + (it.offline?.let { o -> " ($o)" } ?: "")
            }
```

Replace the `online.isEmpty() ->` branch of `compact`:

```kotlin
            online.isEmpty() -> {
                val why = st.peers.firstNotNullOfOrNull { it.lastError }?.let { Fmt.plainPeerError(it) } ?: lastClip
                val away = st.peers.firstNotNullOfOrNull { it.offline } // shortcut: first device only; one laptop is the normal case
                if (away != null) "Offline $away · $why" else why
            }
```

(The duration is also in `lines`, so `Summary.key` changes once a minute and the notification refreshes.)

- [ ] **Step 3: Check Rust, then compile Kotlin**

Run: `nice -n 10 cargo clippy -p klip-android --all-targets -j2 -- -D warnings && nice -n 10 cargo test -p klip-android -j2 -- --test-threads=2`
Then (one heavy job): `export ANDROID_NDK_HOME=$HOME/Android/Sdk/ndk/28.2.13676358 && nice -n 10 android/build-core.sh && cd android && nice -n 10 ./gradlew compileReleaseKotlin --no-daemon; ./gradlew --stop`
Expected: BUILD SUCCESSFUL.

- [ ] **Step 4: Commit**

```bash
git add crates/klip-android android && git commit -m "feat(android): status line says how long the laptop has been gone" -m "Why: a glance at the notification should explain a failed paste. PeerInfo.offline is built in Rust (offline_label), Kotlin only appends it. Not verified on a phone. Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Ship v0.4.3-alpha (standing rule: commit, push, tag, build, install)

**Files:**
- Modify: `Cargo.toml` (workspace version `0.4.3`), `android/app/build.gradle.kts` (`versionCode = 14`, `versionName = "0.4.3-alpha"`), `crates/klip-tui/tests/snapshots/settings_*.txt` (regenerated), `README.md` (one bullet + test count), `docs/AUDIT_AND_ROADMAP.md` (short section)

- [ ] **Step 1: Bump versions and regenerate snapshots**

```bash
sed -i 's/^version = "0.4.2"/version = "0.4.3"/' Cargo.toml
sed -i 's/versionCode = 13/versionCode = 14/; s/versionName = "0.4.2-alpha"/versionName = "0.4.3-alpha"/' android/app/build.gradle.kts
UPDATE_SNAPSHOTS=1 nice -n 10 cargo test -p klip-tui --features testkit --test snapshots -j2
```

- [ ] **Step 2: README and docs**

In `README.md` under "What it does" add the bullet: `- If a copy cannot reach a device, Klip tells you on the laptop (desktop notification, once per outage) and the phone's notification says how long the laptop has been gone.` Update the test count in both README places to the number printed by the full run in Step 3. Add a short "Undelivered-copy alert (v0.4.3-alpha)" paragraph to `docs/AUDIT_AND_ROADMAP.md`, including what is not verified (real notification on this desktop, phone wording, laptop suspend).

- [ ] **Step 3: Full verification**

Run, one at a time: `cargo fmt --all -- --check`, `nice -n 10 cargo clippy --workspace --all-targets -j2 -- -D warnings`, `nice -n 10 cargo test --workspace -j2 -- --test-threads=2` (all green; note the passed count).

- [ ] **Step 4: Commit, push, tag**

```bash
git add -A && git commit -F - <<'EOF'
feat(alert): desktop notification when a copy does not reach a device; v0.4.3-alpha
...ledger message: why, inventory per crate, verification, what is not verified...
Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>
EOF
git push origin main
git tag -a v0.4.3-alpha -m "v0.4.3-alpha: undelivered-copy alert" && git push origin v0.4.3-alpha
```

- [ ] **Step 5: Build locally and install**

```bash
nice -n 10 cargo build --release --locked --bin klip -j2
export ANDROID_NDK_HOME=$HOME/Android/Sdk/ndk/28.2.13676358 && nice -n 10 android/build-core.sh
cd android && rm -rf app/build && KLIP_KEYSTORE_FILE=$HOME/.klip-signing/klip-release.jks KLIP_KEYSTORE_PASSWORD="$(cat $HOME/.klip-signing/keystore.pass)" KLIP_KEY_ALIAS=klip KLIP_KEY_PASSWORD="$(cat $HOME/.klip-signing/keystore.pass)" nice -n 10 ./gradlew assembleRelease --no-daemon; ./gradlew --stop; cd ..
cp android/app/build/outputs/apk/release/app-arm64-v8a-release.apk Klip-Android.apk
cp android/app/build/outputs/apk/release/app-armeabi-v7a-release.apk Klip-Android-armv7.apk
install -Dm755 target/release/klip ~/.local/bin/klip   # stop the service first
systemctl --user stop klip.service && install -Dm755 target/release/klip ~/.local/bin/klip && systemctl --user start klip.service
```

- [ ] **Step 6: Release notes**

Wait for the tag's two workflows (`gh run list`), then `gh release edit v0.4.3-alpha --notes-file <honest notes>` listing what was NOT verified.

---

## Self-review

- Spec coverage: success criteria 1 (Tasks 3-4), 2 (Task 5), 3 (once per outage, explicit sends excluded: Tasks 3-4), 4 (hook signature carries name and duration only). Decisions list: no grace (Task 3 has none), only Connect/Stream (Task 3 tests), hook installed by the binary (Task 4), no IPC change (Tasks 2-5 touch no IPC type).
- Names used consistently: `offline_for`, `offline_label`, `UndeliveredHook`, `set_undelivered_hook`, `is_sending`, `first_failure_of_outage`, `is_unreachable`, `notify::hook`.
- Placeholders: Task 6 Step 4's commit body is "ledger message" to be written from the actual results at that time; every code step has full code.
