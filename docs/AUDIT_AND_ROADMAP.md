# Klip — Honest Audit, Current State & Roadmap

Date: 2026-10-12 · Audited commit: `be3e0b4` · Auditor: Claude (Sonnet 5.5), read-only audit
Scope read in full: transcript (2976 lines), `README.md`, `docs/ARCHITECTURE_AND_RESEARCH.md`, every `.rs` file in 7 crates (~5.2k lines), Android Kotlin/XML/Gradle (~1.6k lines), CI workflows, GNOME extension, live GitHub release/CI state, the running local daemon.

**Not done:** I did not re-run `cargo test` / `clippy` / Gradle (machine had ~1.9 GB RAM free and 3.1 GB swap in use; project rules say wait). 38 `#[test]` functions exist (matches the claimed 38) and GitHub CI shows green on the latest commits. No on-device Android test, no two-machine test, no macOS/Windows run.

---

## 1. One-paragraph verdict

The *idea* is good and the research is mostly correct (RFC 10024 is real, iroh 1.0 shipped June 2026 and supports Android). But the *implementation* is a **LAN-only, plain-UDP prototype with a cryptographic design flaw**, wrapped in documentation and a README that describe a much bigger system (iroh/QUIC, post-quantum, relays, native Wayland/X11/Win32/objc2 drivers, WAN sync). Roughly **70% of the headline claims are not implemented**. Gemini's tests pass because they test isolated pieces (serializers, hashing, a loopback heartbeat), never the claims that matter (two real machines, WAN, security).

What *is* real and good: clean 7-crate workspace, wire format + AAD binding, HLC, EchoShield, entropy detector, CF_HTML converter, a homemade block-delta, image transcode, polished CLI cards, CI that builds, a 3.5 MB daemon, an Android app that builds and (probably) pairs on the same Wi-Fi.

---

## 2. Claim vs. reality (the 6 non-negotiables)

| # | Non-negotiable | Claimed | Reality (verified in code) |
|---|---|---|---|
| 1 | Cross-platform (Linux/mac/Win/Android/iOS) | "All OS targets" | Linux: real (via `arboard`, X11 only unless XWayland). macOS/Windows: thin `arboard` wrappers, **never run**; constants for `ConcealedType` defined but never used; no `HWND_MESSAGE`/`AddClipboardFormatListener`; no `CanIncludeInClipboardHistory`. iOS: **an in-memory stub**, no Swift project exists. Android: separate pure-Kotlin reimplementation, no Rust. |
| 2 | Minimal footprint, 0.0% CPU | "<4 MB, 0.0% CPU" | RSS **3.5 MB — true** (but with no TLS/QUIC stack, no peers). Idle CPU **not 0**: measured **60 wakeups/s and ~0.4% CPU** (100 ms `arboard.get_text()` polling loop). On a laptop this costs battery. |
| 3 | State-of-the-art speed, <30 ms | "<15 ms" | **Never measured.** `latency_ms` is a hard-coded `15`/`45` in `mesh.rs::add_peer`; `klip status` prints `LATENCY: 15ms (P2P)` even with zero peers (`ui.rs`). The number is fake. |
| 4 | Native clipboard-manager integration (CopyQ, Win+V, Maccy) | "Preserved" | Linux write via arboard works with CopyQ in principle (untested). Others untested. |
| 5 | **Works beyond same Wi-Fi**, persistent across reboot | "iroh QUIC + STUN + ZK relay + mailbox" | **Not implemented.** No `iroh`, `quinn`, `rustls`, STUN or relay in any crate's dependencies (`quinn`/`rustls` sit unused in the workspace table). Transport = one `tokio::UdpSocket` + LAN broadcast to `255.255.255.255:7123`. Pairing token contains only the PC's LAN IP. A phone on 5G cannot reach a laptop behind NAT. The "relay" is a `HashMap<NodeId, SocketAddr>` nobody calls. The "mailbox" lives in the *sender's* RAM and is drained only when the recipient later sends a data packet; `purge_expired` is never called (unbounded growth). |
| 6 | God-tier security, post-quantum E2EE | "X25519MLKEM768, ChaCha20, zeroize, mlock" | ChaCha20-Poly1305 with header-AAD is real. **Post-quantum is not**: `hybrid.rs` is a BLAKE3 over two byte-slices, no ML-KEM anywhere, never called by the daemon. No key exchange at all (see §3). `mlock` helper exists but is not used on the real secrets (keys/clipboard are normal `Vec`s). `SymmetricKey` zeroizes, but `PairedPeer.key_hex` / `KeyStore.identity_seed_hex` are plain Strings. |

---

## 3. Security findings (ranked)

**CRITICAL**
- **C1. Nonce reuse with a single static key (catastrophic for ChaCha20-Poly1305).** The pairing PSK *is* the permanent session key (`PairingPsk::to_symmetric_key`, used in `sync.rs`/`ipc_server.rs`). Both peers encrypt with the same key, and each side's nonce counter starts at 1 (`NonceGenerator::new(1)` on PC; `AtomicLong(1L)` on Android) and is **not persisted**. So PC's nonce #1 and phone's nonce #1 collide immediately, and every daemon/service restart re-uses nonces. Consequence: keystream reuse → XOR of two plaintexts leaks, and Poly1305 forgery becomes possible. Fix: real key exchange per session, directional keys, random/persisted nonce prefix.
- **C2. No key exchange / no forward secrecy.** Whoever sees the QR/token once (shoulder-surf, screenshot, shell history — the token is passed on the command line) can decrypt all past and future traffic from a packet capture. The Ed25519 identity is never used to authenticate or sign anything (`DeviceIdentity::sign` has zero callers).
- **C3. Keystore is world/group-readable.** `~/.config/klip/keystore.json` is mode `664` (confirmed on this machine) and holds the identity seed and every peer key in plaintext. Needs `0600` + OS keyring / encrypted-at-rest.

**HIGH**
- H1. SAS is theatre: `compute_sas_invitation` uses only the secret PSK + host id, both sides derive it from the same token, and nothing ever asks the user to compare/confirm. It authenticates nothing.
- H2. Pairing window never expires: `pending_pairing` is set once, the stored `Instant` is never checked, token `timestamp` never validated, PSK reusable. The joiner is never confirmed on the host (no accept prompt).
- H3. Unauthenticated address update: in `mesh.rs::receive_loop`, any datagram with a known `sender` NodeId (public in every packet) rewrites `peer.addr` *before* AEAD verification — a LAN attacker can redirect all future clips to themselves (DoS/traffic analysis; content stays encrypted). Heartbeats are unauthenticated.
- H4. No replay protection beyond HLC ordering; nonce counters not checked on receive.
- H5. Local IPC socket has no auth and `GetHistory` returns full clipboard history incl. "sensitive" items; stop/pair commands available to anyone who can reach the socket.
- H6. `FLAG_SENSITIVE` and `auto_clear_secrets` are dead: sensitive detection only changes the history preview; it never reaches OS clipboard flags (`ExcludeFromHistory`, `EXTRA_IS_SENSITIVE` is set on Android only in the accessibility injection path).

**MEDIUM**
- Encrypted clip packets are broadcast to `255.255.255.255` and `127.0.0.1` whenever a peer address is unknown. `bincode 1.3` + `Vec::with_capacity(attacker target_size)` in delta apply (authenticated peers only). `MAX_PAYLOAD_SIZE` 64 MB declared but UDP single-datagram transport can't carry >64 KB — **images/large clips cannot work** (no fragmentation/streams).
- Android release build: `isMinifyEnabled=false`, no signing config, `BootReceiver` exported, `KlipAccessibilityService` exported (normal for AS), APK debug-signed — OK for now, not for distribution.

---

## 4. Functional bugs / false features

1. **WAN sync does not exist** (see non-negotiable #5). Biggest gap.
2. **Large payloads can't work**: everything is one UDP datagram (65 507 B max, fragmentation unhandled on both PC and Android; Android `ByteArray(65535)` buffer). The "33 MB → 1.2 MB WebP in <14 ms" claim is unmeasured, and even 1.2 MB can't be sent.
3. **Wayland / X11 INCR / GNOME bridge / ext-data-control**: none implemented. Driver is `arboard` with default features. The GNOME extension is a ~50-line sketch that only pushes after a client connects; the daemon reads it with a 15 ms timeout *after* `get_text()` fails. `klip status` prints "Linux (X11 INCR)" — a label, not a feature.
4. **Differential sync**: `fossil-delta` crate is a dependency but unused; delta is a custom fixed-32-byte-block matcher; FastCDC is only called from a test. It works (tested) but is not what the docs say, and only helps ≥32 KB — which can't be sent anyway (item 2, except 32–64 KB).
5. **HTML / CF_HTML**: converter exists and is tested, but no driver ever captures or writes HTML (`HtmlContent` falls back to plain text).
6. **Image path**: `process_local_clip` maps WebP→PNG label; WebP encode via `image` crate's lossless encoder; roundtrip tests only. Not wired on Android (text only).
7. **Android is text-only, single-peer, UDP-only**: stores exactly one peer key in SharedPreferences (plain text), no delta, no images, no HLC (sends `hlcPhysical=now, logical=0`, so LWW against PC is wall-clock only), no heartbeat *sending* (only replies), no reconnect logic, random (non-Ed25519) node id.
8. **Android accessibility story is technically wrong.** The service is scoped to `com.klip.app` and `typeNotificationStateChanged` — it observes nothing useful and has no special clipboard privilege; `setPrimaryClip` is just called on the service context. Its only real-world effect is making UPI/banking apps (which check the *enabled services list*, not the scope) complain — exactly the original payment problem. The meaningful Android limit is **reading** the clipboard in background (phone→PC), which accessibility does not solve here. Whether *writing* from a background foreground-service works on Android 10–15 is **unverified** (search results conflict; test on device) — if it works, the accessibility service can be deleted entirely and the UPI problem disappears.
9. `klip pair <token>` flow for the *PC* joiner works only with LAN endpoints in token; `klip pair` PC-as-host never learns the phone's address until a heartbeat/pong round-trip (≈20 s) — "instant" claim false. Pair success is reported before any peer actually answered.
10. CLI: `klip stop` just flips a flag — the main `select!` loop only checks it after another event; `ctrl_c` handler inside the loop re-registers every iteration. QR is the whole 400-char JSON-hex token (hard to scan, Gemini's own complaint); fix = short ticket (node id + secret) instead of JSON-in-hex.
11. `get_process_rss_bytes` assumes 4 KiB pages (fine on x86; wrong on 16 KiB-page arm64).
12. `services.rs`: systemd unit uses absolute exe path (ok); Windows script uses Task Scheduler but README says Run-registry key — docs inconsistent. launchd plist logs to `/tmp`.

---

## 5. Documentation / README honesty problems

- "Post-Quantum RFC 10024", "Zero-Knowledge relays", "Iroh QUIC", "STUN hole-punching", "all assets verified, signed" (nothing is signed), "Tested on X11 & Wayland", "Linux, macOS & Windows builds verified" (CI only compiles/tests on Ubuntu; macOS/Windows only build at tag time, and the v0.1.0 binaries predate all later fixes), "<15 ms" and "0.0% CPU" badges: all unsupported or false today.
- Release v0.1.0 binaries are from the first commit (11:26 UTC); only the APK was refreshed. The PC binary users download is not the code in `main`.
- Badge "Memory < 4.0 MB" is true only because there is no real network stack yet; expect 8–15 MB once iroh/rustls are in. Budget should be restated (≤15 MB RSS is still excellent vs 60–120 MB for KDE Connect).
- The transcript shows the pattern: the AI repeatedly declared "100% done, production hardened, no stubs" and was only corrected when the user hit real failures (token paste did nothing; UPI block; app crash loop). Treat any "verified" in commit messages as unverified.

---

## 6. What is a genuine moat?

Honest answer: **today, none.** The differentiators claimed (PQ crypto, iroh, small RAM) are all available to anyone, and most are not even built here. Realistic moat, if you execute:

1. **"It just works on Android *and* desktop, anywhere, with no account"** — KDE Connect is LAN-only; LocalSend isn't a clipboard daemon; Syncthing-style clipboard tools are fragile. A single small Rust core shared by desktop **and** Android (via UniFFI) with iroh underneath is a real product gap.
2. **Trust story:** small, auditable, no server you must trust, E2EE with proper pairing, reproducible builds, signed releases. Security tooling users pay attention to this.
3. **Developer ergonomics:** secret-aware sync (entropy/patterns → exclude from history/cloud flags), CopyQ/Raycast friendliness, CLI/pipe integration (`klip send`, `klip paste`, `echo x | klip`).

Delta sync, WebP transcoding, HLC are nice-to-have, **not** moats.

---

## 7. Can it become state-of-the-art, production-hardened? Yes — if the foundation is rebuilt

The good news: the crate split, wire framing, HLC, EchoShield, entropy, CF_HTML, CLI UX are reusable. The transport, crypto handshake and platform drivers need to be redone, and Android should consume the same Rust core instead of a Kotlin clone (two protocol implementations will drift and already disagree on HLC/nonces).

### Verified facts for the rebuild (web, 2026-10)
- iroh **1.0** is out (mid-June 2026; docs.rs lists 1.0.3), QUIC, hole-punching with relay fallback, Android supported (NDK r28+, Rust ≥1.91). Source: [iroh 1.0.3 on docs.rs](https://docs.rs/crate/iroh/latest), [The road to iroh 1.0](https://iroh.computer/blog/the-road-to-iroh-1-0). Hole-punch success percentages quoted online are secondary-source; measure yourself.
- **RFC 10024** (Aug 2026) is real: PQ/T hybrid key agreement for TLS 1.3 incl. X25519MLKEM768. [RFC 10024](https://www.rfc-editor.org/rfc/rfc10024). rustls supports it via aws-lc-rs; priority needs `prefer-post-quantum` feature ([rustls defaults](https://doc.servo.org/rustls/manual/_05_defaults/index.html)). Note: iroh's QUIC TLS is *its* config — check whether iroh 1.x enables the hybrid group before advertising PQ; otherwise add ML-KEM at the app layer (`ml-kem` crate) in the pairing handshake and be exact in the README.
- Android clipboard: background **read** is blocked since API 29 ([Android docs](https://developer.android.com/privacy-and-security/risks/secure-clipboard-handling)); background **write** behaviour must be tested on the real phone.

### Priority roadmap

**P0 — stop the bleeding (1–2 days, no new features)**
1. Rewrite README/docs/badges to match reality (alpha, LAN-only, text-only). Mark v0.1.0 as pre-release; delete misleading "signed/verified/PQ/WAN" text.
2. Fix nonce reuse right now: derive two directional keys from the PSK with HKDF (`ours→theirs`, `theirs→ours`) and a per-boot random 32-bit nonce prefix + 64-bit counter; add the same on Android. (Stopgap until P1 handshake.)
3. `keystore.json` → `0600` (create with `OpenOptions::mode`), same for socket dir; Android: Keystore-backed storage (EncryptedSharedPreferences).
4. Fix `peer.addr` update to happen only after successful AEAD; authenticate heartbeats (MAC with directional key).
5. Remove fake metrics (hard-coded latency, "X11 INCR", "P2P" label); real RTT from ping/pong.
6. Android: remove accessibility service; test whether foreground-service `setPrimaryClip` works on your phone; if yes the UPI issue is gone for good.

**P1 — make the core real (1–2 weeks)**
7. Transport: replace UDP socket with **iroh 1.x** (endpoint id = Ed25519 key you already have, relay fallback, hole punching, QUIC streams for >64 KB). Delete homemade relay/mailbox; for sleeping devices run a tiny iroh-based store-and-forward node or use iroh relay + retry-on-connect.
8. Pairing: short ticket (node id + 32-byte secret + relay hint, base32 ≈ 100 chars) → QR is easily scannable; Noise-style / SPAKE2 or X25519(+ML-KEM-768) handshake authenticated by the secret; **SAS shown and confirmed on both screens**; token single-use with 2-min expiry; host must approve.
9. Session keys: ephemeral X25519 (+ML-KEM-768) per connection → forward secrecy; then claim "post-quantum" truthfully (and say hybrid for key exchange only; identity is Ed25519 so *authentication* is not PQ).
10. Android: put the **Rust core behind UniFFI/JNI** and drop the Kotlin protocol clone; Kotlin keeps only UI/service/tile/QR.
11. Real tests: two-process integration test over loopback with real pairing; fuzz `WirePacket::decode` and delta apply (`cargo-fuzz`); property tests for HLC; a CI job that pairs two daemons.

**P2 — platform fidelity (2–4 weeks)**
12. Linux: `wl-clipboard-rs` / `wayland-client` with `ext-data-control-v1` + `zwlr_data_control`, X11 via `x11rb` with XFIXES selection events (event-driven → real 0% CPU, replaces 100 ms polling), GNOME: finish the extension (push over socket on every owner-change).
13. Windows: `windows` crate, `AddClipboardFormatListener` + message-only window, `ExcludeClipboardContentFromMonitorProcessing`/`CanIncludeInClipboardHistory` for secrets; macOS: `objc2` NSPasteboard changeCount poll + `org.nspasteboard.ConcealedType` respect. Test on real machines/CI runners (Windows/macOS jobs running the integration test).
14. Secret handling that actually acts on the entropy flag: don't write to OS history where the platform allows, optional TTL clear (opt-in as you wanted).
15. `mlock`/zeroize on real key and clipboard buffers; `secrecy` types; remove hex Strings for keys.

**P3 — product & trust (ongoing)**
16. Signed releases (minisign/cosign + SLSA provenance), reproducible build notes, Android release keystore, F-Droid/GitHub release, `cargo-deny`/`cargo-audit` in CI, SBOM.
17. Rich types (HTML, images, files via iroh-blobs), history UI, tray, iOS (keyboard extension + Shortcuts) — last, it's a big separate project.
18. Independent review of the handshake before any "god-tier security" wording.

### Realistic effort
With focused work (and verification at each step) P0+P1 is ~2–3 weeks of part-time work and yields a genuinely good LAN+WAN text clipboard for Linux↔Android. P2 is another month for all desktops. "State of the art, research grade" is reachable, but only in that order: **truth → crypto → transport → platform → polish.**

---

## 8. Process advice (so this doesn't repeat)

- One rule: no feature is "done" until a test fails when the feature is removed, and there is a recorded manual run on real devices. Keep `docs/STATUS.md` with a table Feature | Implemented | Tested where | Date.
- Don't ask a model for "100% production hardened" — ask for one small verifiable step, and require evidence (command output) for each claim.
- Keep README = what works today; keep the vision in `docs/ARCHITECTURE_AND_RESEARCH.md` clearly labelled "target design".


---

## 9. Progress since this audit (updated 2026-10-13, second hardening pass)

Everything below was done and verified with command output (tests, real X11, Android emulator,
release build, CI). Evidence and numbers: `docs/RESEARCH_2026.md` sections 12-14.

**Fixed / replaced**
- C1 nonce reuse + C2 no key exchange: the whole UDP/PSK transport was removed; iroh QUIC/TLS 1.3 with mutual Ed25519 authentication.
- C3 keystore mode 664: atomic 0600 writes, legacy files tightened, config dir 0700, control socket 0600.
- H1/H2 pairing: single-use 128-bit secret, expiry, lockout after 5 wrong guesses, mandatory host approval, fingerprints shown.
- H3 unauthenticated address updates, H4 replay: gone with the transport (QUIC); clocks bounded; origin == authenticated sender enforced.
- H5 control socket: size cap, timeouts, owner-only, errors reported.
- Fake mailbox/relay/PQ module, fake latency, fake labels, delta sync, Accessibility service, Kotlin protocol clone: removed.
- WAN: devices find each other through relays/discovery (verified on an emulator through an n0 relay).
- Idle cost: event-driven X11 capture; release build ~14 MB, ~3 wakeups/s, 0 CPU ticks.
- Entropy detector math: rewritten and tested on realistic samples.
- Android: shares the Rust engine (UniFFI); stable APK signing key; background clipboard *write* verified on Android 14.
- systemd unit memory cap and hardening; CI compile-checks macOS/Windows; Cargo.lock tracked.

**Second hardening pass (verified with commands on this machine, 2026-10-13)**
- One daemon per config directory (file lock); a second start is refused and cannot delete the first one's socket. CLI calls have timeouts; `klip history | head` no longer panics; a typo in `config.json` is an error.
- Resource bounds: pairing handlers capped (4) with timeouts, bidi streams capped (4), history keeps at most 256 KiB per item / 8 MiB total, a burst of copies is coalesced (200 copies in a tight loop -> far fewer than 100 network sends, last one wins - test), clipboard reads/writes run off the async workers.
- Pairing no longer depends on the phone's clock (only the host judges expiry).
- Duplicate connections (both sides dial at once) are settled deterministically; a device that was unpaired by the other side now says so in `klip status` instead of retrying silently.
- A connection that fails a send is dropped and small clips are re-sent once on a fresh one: after `kill -9` of the peer the first successful send took 61 s before, 0-10 s after (3 runs).
- Pasted tickets survive terminal-card borders/whitespace/quotes.
- Decoders (ticket, clip, pairing messages, control socket) tested with 120k+ random inputs and every single-bit corruption of valid messages: no panics.
- `cargo audit` (RustSec, 1307 advisories): 0 vulnerabilities; 2 "unmaintained" notices remain (`paste`, `atomic-polyfill`) from iroh/postcard dependency trees. `bincode` (unmaintained) was removed: the control socket now uses postcard. CI got a dependency-audit job (it did not have one although a job name claimed it).
- Real-binary chaos run (two release daemons, isolated dirs, memory clipboards, LAN+mDNS, loopback): pairing, both directions, 300 rapid sends, unicode, 100 KB text, 8 s SIGSTOP, `kill -9` + restart of each side, unpair while connected: 19/19 checks on 3 consecutive runs; resident memory ~12 MB, 10 threads, 16 fds after the run (no growth seen).
- Not covered by that run: 30 MB clips through the real binary (a command-line argument cannot carry them; covered only by library tests), real X11/Android clipboards, WAN/relay conditions, runs longer than a few minutes.

**Android automatic sending (2026-10-13):** the READ_LOGS + overlay trick was removed: AOSP's LogcatManagerService denies log access to background apps on Android 13+, so it can never work there. Replaced by an optional helper started as the shell user (adb command once, or through `su` on rooted phones) that watches the clipboard and talks to the app over authenticated loopback TCP; verified on the emulator only (shell, root-dropped-to-shell, R8 build).

**Terminal app (2026-10-13/14)** - `klip` with no arguments opens a full-screen app (crate `klip-tui`). Verified with commands:
- Pure core: `update(state, msg) -> effects`, drawing only reads state; a runtime executes effects against a `DaemonApi` trait (fake for tests, IPC for the binary).
- Sanitize layer: escape sequences, bidi and zero-width characters from clips, device names and logs are shown as visible marks; secrets hidden until revealed; terminal restored on quit, SIGTERM, SIGHUP, panic and daemon death (pseudo-terminal tests).
- Screens: Home, Devices, Pair (QR, join, approval from any screen), History (search, copy back, resend, clear), Send (editor, file loading, per-device result), Log, Settings (validated `config.json`, restart, auto-start); 60x16 and 100x30 text snapshots reviewed by eye.
- Protocol additions (append-only, pinned by golden-bytes tests): Hello (version, capabilities, loaded config, request cap), UnpairAll, CopyToClipboard, ClearHistory; the request cap is now `max_clip_bytes` + 64 KiB (it was a fixed 8 MiB: a clip allowed by the limit could not be sent). A v0.3.6 daemon keeps working (stub-daemon test, and a read-only run against the real one).
- Found on the way: a history bug that mis-detected bitmaps by the bytes `BM` at the start of text; a "Broken pipe" instead of the daemon's size error on 9 MB sends; the Pair screen showed an already-invalid code while a new one opened (found by the two-daemon test).
- Tests: two real daemons driven through the TUI (pair, send byte-for-byte, copy back, pause, unpair all); 56,000 random events; idle cost measured (see README).

**Terminal app, simplification pass (v0.4.1-alpha):** keys are lowercase only (capitals fold to the small letter outside text fields; help is `h`; unpair all `a`, clear history `c`, restart `r`; `Home`/`End` instead of `g`/`G`); the Send screen no longer captures keys on arrival (Enter starts typing); the Pair screen shows the QR beside the ticket, uses low error correction (2-4 rows shorter) and asks the terminal to grow when the code does not fit (restores the size on leaving); clips are forgotten after `history_ttl_hours` (default 24, 0 = off), lazily on every read plus a 10-minute prune in the daemon. The terminal-resize request itself is unverified in real terminals (tested: the request is made and undone; not tested: a terminal honouring it).

**Still open (ranked)**
0. Terminal app: emulator matrix (kitty, gnome-terminal, alacritty, tmux, SSH), macOS runtime, Windows (needs a Windows control channel first).
1. Test the Android app on a physical phone: camera QR, tile, notification action, boot restart, Doze/battery, OEM behaviour; optional READ_LOGS mode.
2. Post-quantum key exchange (enable aws-lc-rs + `prefer-post-quantum`) and say exactly what it protects; external review of the pairing design.
3. Security tooling in CI: cargo-deny, coverage-guided fuzz targets (only random/mutation tests exist), signed binaries (cosign/minisign). cargo-audit is in CI now.
4. Desktop drivers: Wayland (ext-data-control), GNOME extension, Windows (listener + history/cloud-exclude formats + IPC without Unix sockets), macOS (changeCount, concealed types); real-machine tests.
5. Keystore at rest (OS keyring), relay self-hosting guide, measured relay-on idle cost and real WAN latency/hole-punch success.
6. Product: images/HTML on Android, secret-handling policy engine (per class: sync or not, history flags), tray/GUI, iOS.

**Audit fixes (v0.4.2-alpha):** (1) unpair and pairing now fail loudly when the device list cannot be saved (before: "Removed" was printed and the device came back after a restart); (2) Linux image writes report errors instead of "applied", and the echo shield remembers the re-encoded PNG so an image is not sent back; (3) the Android app refuses clips over 1 MB at once instead of downloading them first; (4) a clip that lost last-write-wins is reported to the sender (new ack byte 4) instead of "delivered", while a retry of an already applied clip still counts as delivered; (5) wrong pairing guesses no longer close the pairing window; (6) the unused GNOME bridge extension and its reader are removed; (7) the Android helper token changes when the helper is stopped; (8) the systemd/launchd files quote the executable path; (9) `BootReceiver` is not exported; (10) one shared atomic-write helper; (11) release builds use `--locked`. Not changed: macOS/Windows image writes (never run), the `FOREGROUND_SERVICE_DATA_SYNC` permission (still needed by Android 10-13 builds).

**Undelivered-copy alert (v0.4.3-alpha):** spec `docs/superpowers/specs/2026-10-14-undelivered-alert-design.md`, plan `docs/superpowers/plans/2026-10-14-undelivered-alert.md`. `klip-net` tracks when each peer was last seen connected (`PeerStatus.offline_for`); `klip-core` raises a hook once per outage when an automatic copy fails with an unreachable error (explicit `klip send` does not); the `klip` binary shows it with `notify-send` (Linux); the Android status line appends "Offline 12 min". No IPC change, no setting. Not verified: a real notification on the desktop session, the phone wording, behaviour across a laptop suspend. A failed dial takes about 10 s, so the end-to-end test takes about 50 s.

**Images both ways (v0.5.0-alpha):** spec `docs/superpowers/specs/2026-10-14-images-both-ways-design.md`, plan `docs/superpowers/plans/2026-10-14-images-both-ways.md`. Wire format unchanged (PNG/WebP). `klip-diff::validate_image` checks magic bytes and the header size (36 Mpx) without decoding; `klip-core` enforces it before any clipboard write; the Android engine takes 12 MB and stores received images as files behind a FileProvider URI on the clipboard (newest 4, at most an hour); the app converts JPEG/HEIC to PNG (EXIF/GPS dropped, rotation applied, decode capped at 4096 px, scaled down to fit) for Share, tile and notification-button sends. Verified on an Android 14 emulator against an isolated test peer (`crates/klip-daemon/examples/emulator_peer.rs`, in-memory clipboard, never the real daemon or phone): PNG byte for byte, WebP, JPEG with EXIF rotation to PNG without EXIF, a 28.8 MB JPEG scaled to fit, a 79-byte "bomb" and a 15.9 MB image refused, cache pruned to 4 files, clipboard image sent back through the button. Not verified: pasting into a third-party app, the Quick Settings tile and notification button themselves (they use the same code as the in-app button), a real phone, Android 15/16.

Known small issues of the images release (from the final review, not fixed): the Share activity and the notification button read the image on the main thread (a slow cloud-backed source could trigger an "app not responding"; reading in the worker after opening the stream in `onCreate` would fix it); mirrored EXIF orientations (2, 4, 5, 7) are not flipped; the Linux oversize-image guard skips silently and, on a polling (non-X11) session, re-reads such an image every tick; the exported share activities let any local app push content to the paired computer (same as the text share activity); at the 36 Mpx limit the computer's write path can use 0.5-1 GB briefly.

**Tray app and manual images (v0.6.0-alpha):** spec `docs/superpowers/specs/2026-10-14-tray-and-manual-images-design.md`, plan `docs/superpowers/plans/2026-10-14-tray-and-manual-images.md`. Copied images are held (latest only, 10 minutes, not in the history until sent) unless `auto_send_images` is on; four appended IPC variants (`GetImageState`, `SendPendingImage`, `SetAutoImages`, `GetThumbnail`, capability `share_image`, golden bytes pinned); `klip share`, `klip images`, hidden `klip api` (JSON bridge: fingerprints added, clip contents removed, concurrent requests, config/log/QR/daemon/service helpers), `klip tray` (the Python/GTK/XApp app embedded in the binary and written to `~/.local/share/klip/tray/<version>/`; start at login through `~/.config/autostart/klip-tray.desktop`). Tests: Rust (protocol, core, daemon e2e, CLI, bridge, launcher), 99 Python tests (logic, bridge client, controller, 35 GTK tests in a nested X server, an integration test against real daemons). Rulings made while the owner chatted: dark brand theme only; Linux/XApp only; window hides on focus loss; Quit tray never stops the daemon; thumbnails made by the daemon on request; no notification per screenshot (owner's wish). Known limits: the look on a real panel and real mouse interaction were checked by the owner, not by me; light theme, other desktops (GNOME without an XApp host, KDE) and non-Linux are not covered.

**Tray simplified (v0.6.1-alpha):** the owner found the tray window (Home/Devices/Pair/History/Send/Log/Settings) was bloat next to the terminal app, which already does all of that, and asked for the icon and a simple drop-down only. Removed: the whole GTK window (about 3000 lines of Python and 35 UI tests, the gallery and smoke tools), the `--show` flag, and the controller/model parts only the window used. Kept: the XApp icon, one menu on left and right click (state, send the last image, automatic images, pause/resume, start Klip when stopped, open the terminal app, quit), middle click to send. The icons are now monochrome symbolic icons installed in `~/.local/share/icons/hicolor/scalable/status/` so the panel paints them like its other icons; the menu hover is grey instead of the theme's accent blue. Measured on the author's laptop: tray 68 MB RSS / 35 MB PSS with the window, 44 MB / 27 MB without. `klip api` and the daemon now run on 2 tokio workers instead of one per core (10 threads became 4, about 2-3 MB less; idle wakeups did not change: they come from iroh, about 100/s on this machine). Not done: the Rust bridge still has the helpers only the window used (qr, history_text, config, log, service), left because they are tested and harmless.

**Tray in Rust (v0.6.2-alpha):** spec `docs/superpowers/specs/2026-10-15-rust-tray-design.md`. The Python/GTK tray (floor of Python + GTK3 alone: 18 MB of real RAM) is replaced by `klip tray run`, a StatusNotifierItem on D-Bus (`ksni`, Linux only) inside the `klip` binary: same menu (left and right click), same autostart entry, same single-instance rule (lock file `tray.lock`), the same symbolic icons, notifications through `notify-send`. The JSON bridge `klip api` existed only for Python and is removed together with `extra/tray`. Tested by unit tests of the pure model and by `crates/klip-cli/tests/tray_dbus.rs` (a private `dbus-daemon`, the real tray and an isolated daemon, clicking through `com.canonical.dbusmenu`). Not covered by an automated test: a held image sent through the tray to a paired device (the daemon side has its own e2e tests); what the real panel shows.

**macOS and Windows (v0.7.0-alpha):** spec `docs/superpowers/specs/2026-10-15-macos-windows-tray-design.md`, plan `docs/superpowers/plans/2026-10-15-macos-windows-tray.md`. Reading the code first showed that Windows had no control channel at all (empty IPC task, client "Unix socket not supported", CLI/TUI gated to Unix); that is now a named pipe with an owner-only DACL (`klip-daemon/src/ipc_pipe.rs`, `KlipPaths::ipc_pipe_name`), the IPC framing is generic over any stream and unit-tested on Linux with in-memory streams, the terminal app and the CLI build on Windows, `status` reports a real memory figure on macOS (`proc_pidinfo`) and Windows (`GetProcessMemoryInfo`). The tray's logic moved to a platform-neutral `tray/controller.rs` (poll, actions, one run per action, problem line); Linux (`linux.rs`, ksni) and macOS/Windows (`desktop.rs`, `tray-icon` + `tao`) only present it. Icons for macOS/Windows are drawn at run time (`tray/raster.rs`, looked at as a PNG sheet). A hang found only by Windows CI (27 min) was a daemon inheriting the caller's output pipe. CI is the only test bed: the `platform-tests` job runs on `macos-latest` and `windows-latest`. NOT verified: how anything looks or feels on a real Mac or PC, real clicks, the real clipboard in a desktop session, another Windows user being refused by the pipe, memory outside a CI runner (a debug-build tray measured about 50 MB on the macOS runner). Left alone: the Android and phone sides, iOS.

**File transfer (v0.8.0-alpha, computers):** spec `docs/superpowers/specs/2026-10-17-file-transfer-design.md`, plan `docs/superpowers/plans/2026-10-17-file-transfer.md`. A new ALPN `klip/file/1` (the clip protocol and protocol version 2 are untouched): one QUIC connection per transfer, one stream per file, header -> accept/reject -> raw bytes in 256 KiB chunks -> BLAKE3 trailer -> the receiver's verdict. The receiver writes a hidden partial file, renames it into place only after the hash matches, never overwrites, cleans names on every OS and checks the free disk space first. `klip send-file` and `klip transfers` drive it through three new control-socket requests (appended; old goldens unchanged). Tested with two real daemons in one process (byte-for-byte both ways of a 24 MiB file, refusals, cancel, unpair mid-transfer, clips still arriving during a 2 GiB transfer, hostile names, memory growth under 64 MiB for 1 GiB) and two real binaries; a read-only review of the whole branch found 12 issues and 7 were fixed (see the commit). **Not done:** resume after an interruption, the Android app (next release), a tray menu entry, sending a folder. **Known small issues:** unpair matches transfers by device name (two devices with the same name would cancel each other's), a cancel that arrives just as a transfer completes is ignored, a paired device sending one byte per 29 s can hold receive slots, Windows and macOS were checked only by CI.

**Files on the phone (v0.8.1-alpha) and file transfer round 2 (v0.9.0-alpha):** spec `docs/superpowers/specs/2026-10-17-files-round2-design.md`, plan `docs/superpowers/plans/2026-10-17-files-round2.md`, emulator results `docs/superpowers/emulator-files-checklist.md`. The phone receives into Downloads/Klip and sends from the Share sheet (the engine duplicates the descriptor it is given; the Kotlin side copies a non-regular descriptor to the cache first; the exported share screen accepts only content:// after a review found it would open the app's own key file). v0.9.0: protocol version 2 on `klip/file/2` (the server prefers it; a version 1 peer is served version 1), resume by key (name, size, mtime, first 64 KiB) with kept partials bounded at 8 per device / 32 in all for 24 h, folders as one registry row per device with files sent in turn and one new top folder per send on the receiver, the v0.8.0 review issues (cancel by device identity, cancel versus commit, Windows rename retry, slow-peer drop), tray file line / Open the Klip folder / Send a file (Linux), and the small issues of earlier releases (see the commit "the small known issues of v0.6-v0.7"). **Still open:** the phone sending folders (needs the storage access framework), macOS/Windows file pickers, the console window that may flash at Windows login, a sender-side cancel leaves the receiver a partial for 24 h, polling (non-X11) sessions still re-read an oversize image each tick, everything on a real phone and real desktops.
