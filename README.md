# 📎 Klip

> **State-of-the-Art, Cross-Platform, Zero-Knowledge End-to-End Encrypted Universal Clipboard Engine written in Rust.**

---

## 📌 Context & Overview
**Klip** (formerly referred to as LanClip) is an ultra-lightweight, high-performance universal clipboard synchronization daemon designed to work seamlessly across:
- **Linux** (X11 & Wayland)
- **macOS** (13–15+)
- **Windows** (10/11)
- **Android** (10–15+)
- **iOS** (16–18+)

---

## 🔒 User Non-Negotiables
1. **True Cross-Platform**: Linux, macOS, Windows, Android, iOS.
2. **Minimal Resource Footprint**: Idle daemon consumes **< 5–10 MB RAM** and **0.0% CPU**.
3. **State-of-the-Art Speed**: Sub-30ms to 50ms global synchronization latency using binary zero-copy protocols.
4. **Native Clipboard Integration**: Directly writes to OS clipboards so tools like **CopyQ**, **Win+V History**, **Raycast/Maccy**, and Android/iOS clipboards work out-of-the-box.
5. **Beyond Same-WiFi (Global WAN Sync)**: Works anywhere (5G/cellular, different WiFis, corporate firewalls) via QUIC STUN/hole-punching and zero-knowledge encrypted fallback relays. Fully persistent across system reboots and updates.
6. **God-Tier Security**: Post-Quantum hybrid key exchange (`X25519MLKEM768`), ChaCha20-Poly1305 in-place AEAD, zero-knowledge relays, zeroize memory wiping, and `mlock` page pinning.

---

## 📂 Documentation & Specification
- 📘 **[Architecture, Research & Specification](file:///home/lucifer/Documents/Projects/rust/Klip/docs/ARCHITECTURE_AND_RESEARCH.md)**: Master technical specification covering competitor teardown, Iroh QUIC networking, Post-Quantum cryptography, native OS clipboard drivers, novel features, and production hardening.
- 📄 **[`Klip_Full_Chat_Transcript.txt`](file:///home/lucifer/Documents/Projects/rust/Klip/Klip_Full_Chat_Transcript.txt)**: Verbatim brainstorming and ideation transcript.

