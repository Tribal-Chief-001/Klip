# Klip — Grounded Research Report (2026-10-12)

Purpose: the research the project *should* have started from, done again with sources. Perspective: network architect, security engineer, systems/Rust engineer. Rule: **only what I could verify is stated as fact.** Everything else is labelled.

Evidence grades used throughout:
- ✅ **Primary** — read on the vendor's/standard's own page this session (link given).
- 🟡 **Secondary** — blog, forum, news, third-party summary. Probably right, not authoritative.
- 🔶 **Recalled** — from my own background knowledge, *not* re-checked this session. Verify before relying.
- ❓ **Unknown / conflicting** — needs a real experiment on your hardware.
- 🧮 **Computed** — arithmetic I ran; reproducible.

This report supersedes the *research claims* in `docs/ARCHITECTURE_AND_RESEARCH.md` (written by Gemini). That document's competitor table (RAM figures, "Clipt discontinued", "Barrier crashes >4 MB", "60–120 MB" etc.), "85–90 % hole-punch", "<5 ms migration", "33 MB → 1.2 MB in 14 ms" and "sub-50 ms globally" have **no sources** and several are physically impossible (§3). See `AUDIT_AND_ROADMAP.md` for the code audit.

---

## 0. TL;DR (what the research says)

1. **The goal is achievable, but not literally as written.** Of the 6 non-negotiables: #4 (native clipboard integration) and #5 (works beyond Wi-Fi, mostly) are very achievable with iroh 1.x; #6 (security) is achievable if worded honestly ("hybrid post-quantum key exchange, classical authentication"); #1 is achievable on Linux/Windows/macOS/Android and only *partially* on iOS (Apple forbids background clipboard monitoring); #2 and #3 need re-stating because the numbers were invented.
2. **The hardest real problem is not networking or crypto — it is Android reading the clipboard in the background.** Every serious competitor (KDE Connect, ClipCascade) is stuck on the same wall and uses the same hack (`READ_LOGS` granted over `adb`). Solving that *well* (or honestly designing around it) is where a product can be different.
3. **iroh 1.x (stable since 9 Jul 2026) is the right transport**: dial-by-public-key, QUIC, hole-punching with relay fallback, mutual auth via Ed25519 raw public keys, optional hybrid PQ key exchange. It already provides most of what the old code tried to hand-roll, and what the old design called "Tier 2 app-layer crypto" is redundant for live connections.
4. **Honest physics:** "<50 ms WAN globally" is impossible for far-apart devices (speed of light in fibre: antipodal RTT ≥ ~200 ms 🧮). Realistic target: *same city ≈ 10–40 ms, same continent ≈ 30–100 ms, cross-continent = distance-bound*, plus connection-setup cost on first use.
5. **Several "novel features" are low value** (delta sync for text, WebP transcoding, mlock) and one is **mathematically broken** as implemented (entropy threshold 4.5 can never trigger on hex secrets or on strings shorter than 23 chars 🧮).
6. **Real untapped value** is in: secret-aware handling that cooperates with OS history flags (Windows flags are ✅ documented), a properly designed Android capture path, sleeping-device delivery via push, and trustworthy engineering (signed, fuzzed, reproducible) — not in exotic algorithms.

---

## 1. Problem decomposition and hard limits

A universal clipboard = (A) **capture** local clipboard changes, (B) **inject** remote content, (C) **transport** securely between devices anywhere, (D) **identity/pairing**, (E) **policy** (secrets, size, history), (F) **lifecycle** (background, reboot, battery). Each platform constrains A/B differently; C/D are platform-independent and solved by libraries; F is where mobile OSes bite.

| Platform | Capture (A) | Inject (B) | Verdict |
|---|---|---|---|
| Linux X11 | Event-driven via XFIXES `SelectionNotify`, no polling needed 🟡 ([Chromium/GTK/Wine usage](https://chromium.googlesource.com/chromium/src/+/HEAD/remoting/host/linux/x_server_clipboard.cc)); large data needs ICCCM INCR 🔶 | Own the selection and serve requests (daemon must stay alive) 🔶 | Fully feasible. **Your machine is X11 (Cinnamon)** 🧮(`XDG_SESSION_TYPE=x11`) |
| Linux Wayland (wlroots, KDE) | `ext-data-control-v1` (a port of wlr-data-control) is in wayland-protocols; KWin and wlroots implementations were merge prerequisites 🟡 ([MR 336](https://gitlab.freedesktop.org/wayland/wayland-protocols/-/merge_requests/336)) | same protocol | Feasible; protocol is "privileged" (may be unavailable in sandboxes) 🟡 |
| Linux Wayland (GNOME) | **Mutter exposes neither ext- nor wlr-data-control** per a May 2026 third-party clipboard-manager commit 🟡 ([Strata](https://git.edu4rdshl.dev/edu4rdshl/Strata/commit/8a7091755bce03435c79009882fea2e9f8815a38)); upstream request still open 🟡 ([mutter #524](https://gitlab.gnome.org/GNOME/mutter/-/issues/524)) | via GNOME Shell extension; or the xdg portal Clipboard interface (added in xdg-desktop-portal 1.18, attaches to a RemoteDesktop session, so expect a permission dialog) 🟡 ([Phoronix](https://www.phoronix.com/news/Flatpak-1.18-Released)) | Needs a GNOME-specific extension. Do not claim "Wayland works out of the box". |
| Windows | Message-only window + `AddClipboardFormatListener` 🔶 | `SetClipboardData`; contention needs retry 🔶 | Feasible. History/cloud control formats ✅ ([MS Learn](https://learn.microsoft.com/en-us/windows/win32/dataxchg/clipboard-formats)): `ExcludeClipboardContentFromMonitorProcessing`, `CanIncludeInClipboardHistory` (DWORD 0/1), `CanUploadToCloudClipboard` (DWORD 0/1). |
| macOS | No change notification; poll `NSPasteboard.changeCount` 🔶 | write pasteboard | Feasible. Privacy: a "pasteboard access" alert for programmatic reads was announced; reports say it is **opt-in via a defaults key in macOS 26, not default** 🟡 ([mjtsai](https://mjtsai.com/blog/2025/05/12/pasteboard-privacy-preview-in-macos-15-4)). Design for it (use detect APIs / `accessBehavior`) because it may become default. |
| Android | **Blocked in background since API 29** unless app has focus (or is the default IME, 🟡) ✅ ([Android docs](https://developer.android.com/privacy-and-security/risks/secure-clipboard-handling)); Android 12+ shows a toast on reads ✅; Android 13+ auto-clears 🟡 | Background **write**: docs silent ✅; third-party sources conflict 🟡 → ❓ test on device | The central difficulty — see §5. |
| iOS | No background monitoring; paste from another app triggers the "Allow Paste" prompt unless via `UIPasteControl`/Paste menu 🟡 ([Apple dev forums](https://developer.apple.com/forums/thread/715328)); `detectPatterns` can inspect without prompting but is limited 🟡 | via app/keyboard extension with Full Access 🟡 | Only **user-initiated** sync is possible. Don't promise "silent" on iOS. |

**Physics (latency floor, 🧮):** light in fibre ≈ 200 km/ms. Antipodal one-way ≥ 100 ms → RTT ≥ 200 ms; New York–London ≈ 56 ms RTT floor; Mumbai–Frankfurt ≈ 66 ms floor. No software can beat this. A direct path is as good as it gets; a relay adds a detour. First-time connection adds relay round trips + hole-punch before the direct path (§3).

---

## 2. Competitor landscape (what exists, how they work, where they fail)

Only items I could source. (Not researched this session: Tailscale/Taildrop, Syncthing, Barrier/Deskflow, Pushbullet — so no claims about them.)

| Product | Topology / model | Clipboard behaviour | Known limits (sourced) |
|---|---|---|---|
| **KDE Connect** | LAN-oriented | Has a clipboard plugin | On Android ≥10 automatic background read is blocked → fallback "Send clipboard" in notification / QS tile (Android 14+) or an `adb`-granted `READ_LOGS` hack: app watches its own log for the "clipboard denied" line, then flashes an invisible foreground window to read it ✅/🟡 ([KDE UserBase via search](https://discuss.kde.org/t/kde-connect-clipboard-sync/6422)). |
| **ClipCascade** (GPL-3.0, ~1.9k★) | Server-based (self-host Docker/JAR or community server); optional P2P using STUN but server still needed for signalling/auth, P2P off by default | Text, images, files; Win/mac/Linux/Android; no iOS mentioned | Android background monitoring needs **adb `READ_LOGS` + overlay** permission; Linux Wayland/X11 need workarounds; default message cap 1 MiB; encryption details not specified on its page ✅ ([GitHub](https://github.com/Sathvik-Rao/ClipCascade)). |
| **Apple Universal Clipboard** | Apple-only, same Apple ID, Handoff + Wi-Fi + Bluetooth, ~10 m | single current item, no history | Not available to Windows/Android 🟡 ([Apple help via search](https://help.apple.com/ipod-touch/10/en.lproj/iph220ea8dca.html)). |
| **Microsoft Phone Link** | Microsoft account | Full two-way clipboard only on OEM-integrated phones (Samsung, a few others); a one-way PC→Android "Access PC's clipboard" was in preview (Sep 2025) | Needs special OEM integration per Microsoft 🟡 ([9to5Google](https://9to5google.com/2025/09/03/microsoft-preparing-clipboard-sync-between-windows-and-android-with-gboard-support/)). |
| **Quick Share ↔ AirDrop** | P2P, Pixel 10 (Nov 2025) → Pixel 9, 10a, Galaxy S26 (early 2026) | **Files/photos only; I found no clipboard support** | 🟡 ([Android Authority](https://www.androidauthority.com/google-pixel-9-quick-share-airdrop-support-rollout-3641846)). |
| **LocalSend** | LAN file/text sharing | Not a clipboard daemon | 🟡 (directory listings only). |

**Take-aways:**
- Nobody has a *clean* Android background-read story; the two serious open-source tools both ask users to run `adb`. A product that makes this smooth (guided one-time `adb`/Shizuku-style setup, or an IME path) is a real differentiator.
- The "beyond Wi-Fi" space is thin: ClipCascade needs a server; KDE Connect is LAN. A serverless-by-default, relay-fallback design (iroh) is a genuine gap.
- Apple/Microsoft solve it by owning the OS; open tools can't. Be honest that Klip is a *best-effort* layer, not an OS feature.

---

## 3. Transport: what to build on (iroh 1.x) and what it actually guarantees

### Verified facts
- **iroh 1.0 released 9 July 2026** ("wire format and API are stable") ✅ ([n0 blog](https://iroh.computer/blog/the-road-to-iroh-1-0)). A third-party article says 15 June; trust the primary source. docs.rs lists **1.3.0** as current ✅ ([docs.rs](https://docs.rs/crate/iroh/latest)).
- Dial by **Ed25519 public key** (EndpointId). QUIC; many concurrent streams; hole-punch then relay fallback ✅.
- TLS with **raw public keys (RFC 7250)**, mutual authentication by key ✅. ALPN is visible to on-path observers; Encrypted ClientHello not in 1.0; only Ed25519 keys; no PQ signatures ✅ (same blog).
- Hole punching: both sides first reach a shared relay, exchange addresses, then send simultaneously; relay path drops away once direct path validated; fallback for some corporate firewalls and cellular networks. iroh's own doc says "roughly 9 out of 10 network configurations" allow direct ✅ ([docs](https://docs.iroh.computer/concepts/holepunching)). This is the vendor's number.
- Independent measurement of a *different* stack (libp2p DCUtR): **70 % ± 7.1 %** success over 4.4 M attempts, 85 k networks, 167 countries; QUIC vs TCP statistically indistinguishable ✅ ([arXiv 2510.27500](https://arxiv.org/abs/2510.27500)). → **Plan for 10–30 % of sessions going through a relay**, and test on your own networks. The Gemini doc's "85–90 %" and the secondary "95 % of bytes direct" are not independently supported.
- iroh forked quinn into **noq** for QUIC multipath; depends on rustls 0.23.x ✅ ([docs.rs](https://docs.rs/crate/iroh/latest)).
- **Post-quantum is opt-in**: use `tls-aws-lc-rs` feature + rustls `prefer-post-quantum` → X25519MLKEM768; adds ~1 KB each way (multiple UDP packets for handshake); `ring` backend lacks PQ; **authentication stays Ed25519 → not protected against an active quantum MITM** ✅ ([iroh PQ post](https://iroh.computer/blog/iroh-post-quantum-handshakes)). RFC 10024 (Aug 2026) defines the X25519MLKEM768 hybrid ✅ ([RFC Editor](https://www.rfc-editor.org/rfc/rfc10024)).
- Language bindings: Kotlin via `iroh-ffi` (UniFFI). The **published Maven artifact is not Android-ready (needs source build)** ✅ and one third-party report says its async calls hang on Android 🟡 ([iroh docs](https://docs.iroh.computer/languages/kotlin)). → Budget time for building your own thin UniFFI/JNI wrapper around your Rust core. The same docs warn Android aggressively suspends background processes and kills sockets ✅.

### Implications for Klip's design
1. **Delete** the hand-rolled UDP transport, "relay", "mailbox" and the app-layer ChaCha for live sessions. QUIC/TLS already gives confidentiality + integrity + mutual auth + multiplexing + migration. The old "Tier 2 envelope" only makes sense for *store-and-forward* (§6).
2. Do **not** advertise "zero-knowledge relay" loosely: a relay learns EndpointIDs, IPs, timing and traffic volume (metadata) even though it cannot read payloads. State that.
3. Keepalives: NAT mappings expire; RFC 4787 sets a 2-minute *minimum* for compliant NATs 🔶, real devices and carrier NAT are shorter 🟡. Apple's QUIC client uses 20 s keepalive only while a request is active and none when idle 🟡. On mobile, **idle keepalive vs battery is a trade-off you must decide** — see §5/§6; on desktop it's cheap.
4. First message after idle will pay reconnect cost (relay path first, direct upgrade after). Measure it; don't promise "<15 ms".

### Alternatives considered
libp2p (DCUtR measured 70 %), WebRTC (heavy; not re-researched), WireGuard/Tailscale (needs TUN/root; not re-researched), Nostr/ntfy-style relays (not researched). I have no evidence that any beats iroh for this use; the old doc's reasons for rejecting WebRTC ("40 MB C++") are unsourced.

---

## 4. Cryptography and pairing — what is right, what is hype

**Threat model (write this down first; Klip had none):** (T1) passive network observer/recorder; (T2) active LAN/WAN attacker (spoof, replay, redirect); (T3) malicious/compromised relay; (T4) stolen/lost device; (T5) shoulder-surfed/photographed QR; (T6) local malware (out of scope but must not make it worse); (T7) future quantum adversary recording traffic ("harvest now, decrypt later").

**Recommended primitives**
- **Session security:** rely on iroh QUIC/TLS 1.3 (✅ mutual auth with Ed25519 raw keys). Enable hybrid PQ key exchange (X25519MLKEM768) via aws-lc-rs → addresses T7 for confidentiality only. **Say exactly that.** Authentication is not PQ ✅. Cost: bigger binary (aws-lc-rs heavier than ring ✅), +1 KB per handshake.
- **Pairing:** iroh authenticates by public key, so the QR only needs to carry the host's EndpointId (public) + relay hint + a **one-time high-entropy secret (≥128 bit)** that authorises *this* joiner. Because the QR is an out-of-band, high-entropy channel, a SAS/PAKE is unnecessary; SAS only matters when the channel is unauthenticated (e.g. typing a short code). Controls: single-use, 2–5 min expiry, host-side accept prompt, rate-limit failed attempts, bind secret to EndpointId.
- **If you want a typed short code fallback:** use a balanced PAKE (CPace or SPAKE2); CFRG recommended CPace and OPAQUE; OPAQUE needs a server-held record so fits less 🟡 ([CFRG review](https://mailarchive.ietf.org/arch/msg/cfrg/eo8O6JYPmWY6L9TlcIXStFy5gNQ/)). Rust `spake2` crate's README says it needs a security review before serious use 🟡; I found no CPace crate. → Prefer QR + comparison of a fingerprint, treat typed codes as a later feature needing external review.
- **Store-and-forward payloads (sleeping device):** this is where app-layer encryption is needed. Don't invent a scheme: use HPKE (RFC 9180) 🔶 to the recipient's key; hybrid KEM options: `x-wing` crate exists but the IETF draft says not final and the crate is **unaudited** ✅/🟡 ([docs.rs](https://docs.rs/x-wing), [draft](https://datatracker.ietf.org/doc/html/draft-connolly-cfrg-xwing-kem-04)). Treat as experimental; classical HPKE first.
- **Nonce discipline (the bug in the current code):** with AEAD, never reuse (key, nonce). If any app-layer AEAD remains: per-direction keys (HKDF labels), random 96-bit nonces *or* persisted counters with a random per-boot prefix, replay window on receive. With QUIC this problem disappears for live traffic.
- **Identity key storage:** OS keystores (Linux Secret Service/keyring, macOS Keychain, Windows DPAPI/Credential Manager, Android Keystore) 🔶; at minimum file mode 0600.

**Honest marketing:** "End-to-end encrypted, mutually authenticated by device keys. Hybrid post-quantum key exchange protects recorded traffic. Device authentication is classical (Ed25519)." Do not write "god-tier", "zero-knowledge relay", or "unbreakable".

---

## 5. Platform deep-dive: where the real difficulty lives

### 5.1 Android (hardest)
Facts: background clipboard **read** blocked since API 29 ✅; toast on read from Android 12 ✅; `EXTRA_IS_SENSITIVE` hides preview in keyboard UI (not access control) ✅; foreground service type `dataSync` is capped at **6 h per 24 h** for apps targeting API 35 and `onTimeout()` must stop it or the app crashes ✅ ([Android docs](https://developer.android.com/develop/background-work/services/fg-service-timeout)); Play requires **16 KB page-size** support for native code from 1 Nov 2025, so use NDK r28+ and verify alignment ✅/🟡 ([Android blog](https://android-developers.googleblog.com/2025/07/transition-to-16-kb-page-sizes-android-apps-games-android-studio.html)).
Recalled, **must verify**: 🔶 Android 15 forbids starting `dataSync` (and some other types) foreground services directly from a `BOOT_COMPLETED` receiver — Klip's current design (BootReceiver → dataSync service) may break on Android 15 targets.

Capture options for phone→PC (rank by friction):
1. **User-initiated, zero special permissions:** share-sheet target, `PROCESS_TEXT` action, QS tile (foreground window briefly), notification action. Always works; not "silent".
2. **`READ_LOGS` via one-time `adb` grant** (what KDE Connect and ClipCascade do) ✅. Works on Android 14 per user reports 🟡. Cost: needs a PC + USB debugging once; weaker security posture (app can read logs). Could be wrapped in a guided onboarding.
3. **Own IME (keyboard) strip:** default IME is exempt from the restriction 🟡 (Microsoft security blog / XDA summaries). Big engineering + user must switch keyboards. Gives the best "silent" path for *typing-time* clips.
4. **Shizuku/root:** power users only 🔶.
5. **Accessibility:** does *not* give clipboard-read privilege by itself (my code reading); main observable effect in Klip was banking apps objecting. I did not verify how UPI apps detect it → ❓ hypothesis: they check *enabled* accessibility services irrespective of scope. Recommendation: no Accessibility.

Inject (PC→phone): `setPrimaryClip` from a foreground service — ❓ **run this test first** (E1 below). If background writes work, the whole Accessibility service is unnecessary.

Lifecycle: foreground service with `dataSync` is time-capped (✅ above); alternatives (`connectedDevice`, `specialUse`, or push-wake) not verified here → ❓. Battery-friendly design: no always-on socket; **push-wake** (FCM high-priority wakes from Doze, 4 KB payload cap 🟡 ([Firebase](https://firebase.google.com/docs/cloud-messaging/android/message-priority)); UnifiedPush/ntfy as non-Google alternative, Doze behaviour depends on distributor ❓ 🟡 ([ntfy docs](https://docs.ntfy.sh/subscribe/phone))) → on wake open iroh connection, fetch, close. This **introduces a small always-on component** (push provider or your own relay) — a decision for you (§8).

### 5.2 Linux
- Your own system is X11/Cinnamon → build the X11 path first with XFIXES (event-driven, ~0 wakeups) + INCR. Replace the 100 ms polling that measured 60 wakeups/s.
- Wayland wlroots/KDE: `ext-data-control-v1`, fallback `wlr-data-control`; Rust crate options exist (e.g. `wl-clipboard-rs`, 🔶).
- GNOME-Wayland: extension (keep the bridge idea, but implement owner-change push properly) — the one path that avoids a portal prompt; acceptable to document as "GNOME needs the extension".

### 5.3 Windows
Implement the three documented formats ✅ so secrets can opt out of Win+V history and cloud sync; do *not* set them by default for ordinary clips (preserves your "CopyQ/Win+V keeps everything" rule). Provide per-class policy (secret → exclude; normal → include).

### 5.4 macOS / iOS
macOS: poll `changeCount` at modest interval (measure CPU), honour `org.nspasteboard.ConcealedType`/`TransientType` conventions 🔶, handle pasteboard-privacy alerts gracefully ✅/🟡. iOS: build last — app + share extension + optional keyboard (Full Access) + Shortcuts; push via APNs 🔶; sync is user-initiated.

---

## 6. Sleeping devices and "persistence"

> **DECISION (owner, 2026-10-12): Option 1 only.** Klip syncs only while both devices are online at the same time. **No mailbox server, no push/FCM/UnifiedPush helper, no store-and-forward.** Items 6 in §8, the mailbox box in §11 and the "decide" note in §10 #5 below are therefore **out of scope**; they stay in this document only as the analysis that led to the decision. README must state: "If both devices are never online together, nothing is delivered."

- **Live sessions:** reconnect on resume/network change; QUIC multipath handles path changes (iroh/noq) ✅(stated as the basis for seamless migration; no latency number given).
- **Offline recipient:** a pure P2P system cannot deliver to a powered-off peer. You need *something* always-on: (a) a tiny **sealed mailbox** server (self-hostable, stores only ciphertext + recipient key id + TTL; learns metadata), (b) the user's own always-on device as a rendezvous, or (c) push-wake plus *sender retry* (sender must still be online). I found **no store-and-forward feature in the iroh docs I read** → don't assume iroh relays queue messages. Be explicit with users: "if both devices are never online at the same time and you run no mailbox, delivery isn't possible."
- **Reboot persistence:** `systemd --user`, `launchd`, Task Scheduler, Android boot receiver (+ the Android 15 caveat above). Hardening for the unit: 🔶 standard systemd sandboxing directives (`NoNewPrivileges`, `ProtectSystem`, `PrivateTmp`, `RestrictAddressFamilies`, `MemoryMax`) — not researched this session, but straightforward to apply and test.

---

## 7. The "novel features" — honest evaluation

| Feature | Verdict | Why (with math/evidence) |
|---|---|---|
| **Delta sync (FastCDC/Fossil)** | Low value | Text clips are tiny; latency is RTT-bound. Only helps >32 KB repeated edits. Generic compression (zstd) is simpler 🔶. Keep only if measured benefit; current code uses a custom block-matcher, not Fossil/FastCDC. |
| **WebP lossless transcode** | Situational | Windows `CF_DIB/DIBV5` is raw bitmap, so compression helps there; Linux/macOS usually hold PNG already. The "33.1 → 1.2 MB in 14 ms" figure is unmeasured and content-dependent. Needs a streaming path for MBs (QUIC streams), then measure PNG-optimise vs WebP-lossless on real screenshots. |
| **Shannon-entropy secret detection** | **Broken as specified** | Entropy of a string is ≤ log₂(#distinct symbols) and ≤ log₂(length). Hex alphabet max = 4.0 bits → **hex tokens can never reach 4.5**; strings with < 23 chars can never reach 4.5 🧮. Industry practice: regex for known formats + keyword context + tuned entropy (Checkmarx-style: 4.5 for base64, ~2.7–3 for hex) and, in TruffleHog, live **verification** which cuts false positives 🟡 ([comparison](https://www.jit.io/blog/trufflehog-vs-gitleaks-a-detailed-comparison-of-secret-scanning-tools)). Clipboard context differs (no repo), so expect false positives; the policy must be soft (flag/route), never silent drop. |
| **HLC + LWW** | Fine, needs bounds | A clipboard is a single-register LWW problem; HLC is adequate. Must bound accepted remote physical time (a peer with a far-future clock can otherwise dominate forever) 🔶 (HLC paper recommends max-drift check). Current code lacks it. |
| **CF_HTML ↔ UTF-8** | Nice-to-have | Real format; no driver captures HTML yet. Low priority. |
| **BLAKE3 echo shield** | Needed | Prevents ping-pong; fine. |
| **zeroize / mlock** | Marginal | Good hygiene, doesn't address real threats (T4–T6). Cheap to keep for keys; don't market it. Disable core dumps instead 🔶. |
| **"0 % CPU idle"** | Achievable on Linux/Windows only with event APIs | X11 XFIXES / Wayland data-control events / Windows listener are event-driven; macOS and iOS-less polling is unavoidable (changeCount). Idle QUIC keepalives cost something on mobile. Replace the claim with a measured budget. |

---

## 8. Opportunities that are real (low-hanging vs differentiating)

**Low-hanging (high value, low risk):**
1. iroh transport + QR pairing with host-side approval (replaces ~all the weak parts).
2. Linux X11 XFIXES + INCR driver (your daily machine; truly idle).
3. Policy engine: classes of clip (normal / secret / large / image), per-class rules: sync or not, mark `IS_SENSITIVE` on Android ✅, set Windows history/cloud-exclude formats ✅ for secrets, TTL clear (opt-in).
4. Guided Android onboarding for the `READ_LOGS` path (what competitors leave to wiki pages).
5. CLI ergonomics: `klip send`, `klip paste`, pipe support, `--device X` targeting.

**Differentiating (medium effort):**
6. ~~Mobile push-wake + sealed mailbox for sleeping devices.~~ **Dropped by owner decision (see §6).**
7. Android IME strip for silent typing-time capture/injection.
8. Trust: signed releases, reproducible builds, published threat model, external crypto review before using the word "secure" prominently.

**Not worth it (yet):** custom delta algorithms, PQ signatures (no consensus; iroh says they're waiting ✅), exotic compression, kernel-level anything.

---

## 9. Production-hardening pipeline for a Rust project (checklist)

From sourced list 🟡 ([overview](https://lwn.net/Articles/897437), [RustSec](https://rustsec.org/), [cargo-vet/deny summaries via search]) plus my recall 🔶:
- **Supply chain:** commit `Cargo.lock`; `cargo-deny` (licenses/bans/sources) + `cargo-audit` (RustSec) in CI; `cargo-vet` for new deps; `cargo-auditable` in release binaries; pin GitHub Actions by SHA; Dependabot/renovate.
- **Unsafe & UB:** `cargo-geiger` baseline; `#![forbid(unsafe_code)]` in crates that don't need it; Miri/`cargo-careful` nightly job for unsafe code.
- **Untrusted input:** `cargo-fuzz` targets for packet/ticket/clip parsers and delta/image decoders; size and rate limits everywhere.
- **Concurrency/logic:** `loom` for shared-state code; property tests (proptest) for HLC, policy engine; `cargo-mutants` to check tests actually bite; Kani for small critical invariants (optional).
- **Integration tests that matter:** two real daemons pairing + syncing in CI on Linux; Windows & macOS runners running the same test; an Android instrumented test on an emulator; **16 KB page-size** emulator check ✅.
- **Release trust:** reproducible build notes, SBOM, signatures (minisign/cosign) + provenance (SLSA) 🔶; Android release keystore, R8 enabled.
- **Runtime hardening:** systemd sandbox options; non-root; no network listeners besides the iroh endpoint; IPC socket mode 0600 under `$XDG_RUNTIME_DIR` 🔶; disable core dumps.
- **Docs discipline:** `STATUS.md` listing each feature as Implemented / Tested-where / Date; README only claims what is green there.

---

## 10. Can all six non-negotiables be met? (grounded verdict)

| # | Requirement | Achievable? | What must change |
|---|---|---|---|
| 1 | Linux/mac/Win/Android/iOS | **Mostly.** iOS only user-initiated (Apple restriction ✅/🟡). Wayland-GNOME needs extension. | Reword iOS; ship in order Linux → Android → Win → mac → iOS. |
| 2 | Minimal footprint | **Probably**, but **unmeasured** with iroh + aws-lc-rs (heavier than ring ✅). The old 3.5 MB number is for a build without any network stack. | Prototype and *measure* RSS/binary size/idle wakeups on Linux and Android; then set the budget. |
| 3 | State-of-the-art speed | **Within physics.** LAN ms-level; WAN = RTT + (first-connect cost). | Replace "<15/<50 ms" with percentile targets measured per scenario. |
| 4 | Native clipboard integration (CopyQ, Win+V…) | **Yes** (writing to the standard clipboard integrates automatically). Provide opt-out formats for secrets ✅. | — |
| 5 | Beyond Wi-Fi + persistent | **DECIDED: online-together only, no mailbox.** **Yes for online devices** (iroh: ~9/10 direct per vendor, 70 % in the independent DCUtR study; relay otherwise). **Offline delivery needs a mailbox/push component.** | Decide: self-hostable mailbox/push (recommended) vs "both must be online". |
| 6 | Top-tier security | **Good, not magical.** Hybrid PQ key exchange (opt-in), classical auth ✅. Needs correct pairing, threat model, external review. | Honest wording; remove nonce-reuse design; review before claims. |

---

## 11. Recommended v2 architecture (one page)

```
 clipboard driver (per OS, event-driven)          policy engine (classes, secrets, size)
        │                                                 │
        └──────────── klip-core (Rust, no UI) ────────────┘
                         │  clip model + HLC(bounded) + echo shield
                         │  pairing (QR ticket: endpoint id + relay hint + one-time secret)
                         │  device registry (OS keystore)
                         ▼
              iroh Endpoint (QUIC, Ed25519 raw-key mutual auth,
              optional X25519MLKEM768 via aws-lc-rs)
                 ├─ direct (hole-punched)   ├─ relay (n0 or self-hosted)
                 (no mailbox / push helper — dropped by owner decision, see §6)
 UIs: CLI/TUI (existing cards) · Android app (Kotlin UI over UniFFI/JNI core) · tray later
```
Key point: **one Rust core shared by desktop and Android** so the protocol can't drift; Kotlin only for UI/services/tile/IME.

---

## 12. First experiments (cheap, decisive — do these before writing features)

| # | Experiment | Answers |
|---|---|---|
| E1 | On your phone: foreground service calls `setPrimaryClip` with app in background, screen off; also with Accessibility off | Is Accessibility needed at all? Does UPI complaint vanish? |
| E2 | Minimal iroh endpoint on Linux (+ Android via own JNI/UniFFI build): measure RSS, binary size, idle wakeups/CPU over 10 min, with and without `prefer-post-quantum` | Real footprint budget (#2) |
| E3 | Laptop (home Wi-Fi) ↔ phone (4G/5G) with iroh example: record % direct vs relay, time-to-first-byte cold/warm, over several days/networks | Real #3/#5 numbers |
| E4 | X11 XFIXES watcher prototype: wakeups/s idle vs the current 60/s | Idle CPU claim |
| E5 | Android 15 target: start foreground service from BOOT_COMPLETED; check `dataSync` timeout behaviour; try alternative types | Persistence design |
| E6 | `READ_LOGS` onboarding prototype vs share-sheet-only UX | Phone→PC UX |
| E7 | Entropy/regex detector evaluated on a labelled sample of your real copied secrets and normal text | Policy precision |

---

## 13. Unknowns I could not resolve (don't build on these without checking)
- Android: background `setPrimaryClip` behaviour (E1); how banking RASP detects accessibility; which FGS type is acceptable long-term; Android 15 boot-start restrictions (recalled, unverified).
- Whether iroh relays offer any store-and-forward (I found none).
- iroh memory/binary footprint and cold-connect latency (no published numbers found).
- macOS 26 default status of pasteboard prompt (reports conflict).
- Whether Mutter has since added data-control (only a May 2026 third-party statement).
- iroh Kotlin artifact Android hang (single secondary report).

## Sources
- [The road to iroh 1.0](https://iroh.computer/blog/the-road-to-iroh-1-0) · [iroh docs.rs](https://docs.rs/crate/iroh/latest) · [iroh hole punching](https://docs.iroh.computer/concepts/holepunching) · [iroh PQ handshakes](https://iroh.computer/blog/iroh-post-quantum-handshakes) · [iroh Kotlin docs](https://docs.iroh.computer/languages/kotlin)
- [RFC 10024](https://www.rfc-editor.org/rfc/rfc10024) · [rustls defaults](https://doc.servo.org/rustls/manual/_05_defaults/index.html) · [X-Wing draft](https://datatracker.ietf.org/doc/html/draft-connolly-cfrg-xwing-kem-04) · [x-wing crate](https://docs.rs/x-wing) · [CFRG PAKE review](https://mailarchive.ietf.org/arch/msg/cfrg/eo8O6JYPmWY6L9TlcIXStFy5gNQ/)
- [arXiv 2510.27500 NAT traversal measurement](https://arxiv.org/abs/2510.27500)
- [Android secure clipboard handling](https://developer.android.com/privacy-and-security/risks/secure-clipboard-handling) · [FGS timeout](https://developer.android.com/develop/background-work/services/fg-service-timeout) · [16 KB pages](https://android-developers.googleblog.com/2025/07/transition-to-16-kb-page-sizes-android-apps-games-android-studio.html)
- [KDE Connect clipboard thread](https://discuss.kde.org/t/kde-connect-clipboard-sync/6422) · [ClipCascade](https://github.com/Sathvik-Rao/ClipCascade)
- [MS Learn clipboard formats](https://learn.microsoft.com/en-us/windows/win32/dataxchg/clipboard-formats)
- [ext-data-control MR](https://gitlab.freedesktop.org/wayland/wayland-protocols/-/merge_requests/336) · [mutter #524](https://gitlab.gnome.org/GNOME/mutter/-/issues/524) · [Strata commit](https://git.edu4rdshl.dev/edu4rdshl/Strata/commit/8a7091755bce03435c79009882fea2e9f8815a38) · [xdg-desktop-portal 1.18](https://www.phoronix.com/news/Flatpak-1.18-Released)
- [macOS pasteboard privacy](https://mjtsai.com/blog/2025/05/12/pasteboard-privacy-preview-in-macos-15-4) · [iOS paste prompt](https://developer.apple.com/forums/thread/715328)
- [Phone Link clipboard](https://9to5google.com/2025/09/03/microsoft-preparing-clipboard-sync-between-windows-and-android-with-gboard-support/) · [Quick Share/AirDrop](https://www.androidauthority.com/google-pixel-9-quick-share-airdrop-support-rollout-3641846)
- [TruffleHog vs Gitleaks](https://www.jit.io/blog/trufflehog-vs-gitleaks-a-detailed-comparison-of-secret-scanning-tools) · [FCM priority](https://firebase.google.com/docs/cloud-messaging/android/message-priority) · [ntfy/UnifiedPush](https://docs.ntfy.sh/subscribe/phone)
