"""What the tray knows and says, with no GTK and no I/O, so it can be tested anywhere.

The daemon's answers arrive as JSON from `klip api` (see crates/klip-cli/src/bridge.rs).
"""
from __future__ import annotations

import dataclasses
import re
from typing import Any, Optional

# ---------------------------------------------------------------------------------- formatting
# These mirror crates/klip-tui/src/fmt.rs so the terminal app and the tray say the same thing.


def ago(secs: int) -> str:
    if secs <= 1:
        return "just now"
    if secs < 60:
        return f"{secs}s ago"
    if secs < 3600:
        return f"{secs // 60} min ago"
    if secs < 86_400:
        return f"{secs // 3600} h ago"
    return f"{secs // 86_400} d ago"


def size(n: int) -> str:
    if n < 10 * 1024:
        return f"{n:,} B"
    k = 1024.0
    if n < k * k:
        return f"{n / k:.1f} KiB"
    if n < k * k * k:
        return f"{n / (k * k):.1f} MiB"
    return f"{n / (k * k * k):.1f} GiB"


def remaining(secs: int) -> str:
    if secs > 365 * 24 * 3600:
        return "until you resume"
    if secs >= 3600:
        return f"{secs // 3600} h {(secs % 3600) // 60:02d} min"
    return f"{secs // 60}:{secs % 60:02d}"


def plain_error(raw: Optional[str]) -> str:
    """An engine or network message in words a person can act on."""
    if not raw:
        return ""
    r = raw.lower()
    if "unpaired by the other device" in r:
        return "The other device removed this device. Remove it here too, then link again."
    if "paused" in r:
        return "That device has syncing paused."
    if "clipboard" in r and "write" in r:
        return "That device couldn't write to its clipboard."
    if "no addressing information" in r or "cannot reach" in r or "unreachable" in r:
        return "Can't find it right now. It reconnects by itself when it's back."
    if "timed out" in r:
        return "It didn't answer in time."
    return raw[:90]


def variant(resp: Any) -> tuple[Optional[str], Any]:
    """serde's JSON form of a Rust enum: "Unit" or {"Name": payload}."""
    if isinstance(resp, str):
        return resp, None
    if isinstance(resp, dict) and len(resp) == 1:
        (name, payload), = resp.items()
        return name, payload
    return None, None


# --------------------------------------------------------------------------------------- state


@dataclasses.dataclass(frozen=True)
class Peer:
    name: str
    fingerprint: str
    online: bool
    error: str


@dataclasses.dataclass(frozen=True)
class ImageInfo:
    text: str
    size_bytes: int
    width: int
    height: int
    age_secs: int
    kind: str


@dataclasses.dataclass(frozen=True)
class Summary:
    state: str  # stopped | no_devices | waiting | connected | paused
    title: str
    subtitle: str
    tooltip: str
    badge: bool
    peers: tuple
    device_fingerprint: str
    platform: str
    memory: str
    relay_state: str
    relay_mode: str
    paused_secs: int
    last_clip: Optional[str]
    image: Optional[ImageInfo]
    auto_images: bool
    daemon_old: bool


def _int(v: Any, default: int = 0) -> int:
    return v if isinstance(v, int) and not isinstance(v, bool) else default


def _names(names: list) -> str:
    if len(names) == 1:
        return names[0]
    if len(names) == 2:
        return f"{names[0]} and {names[1]}"
    return f"{names[0]} and {len(names) - 1} more"


def _image(images: Any) -> Optional[ImageInfo]:
    waiting = images.get("waiting") if isinstance(images, dict) else None
    if not isinstance(waiting, dict):
        return None
    kind = str(waiting.get("kind") or "png")
    w, h = _int(waiting.get("width")), _int(waiting.get("height"))
    n, age = _int(waiting.get("size_bytes")), _int(waiting.get("age_secs"))
    return ImageInfo(f"{kind.upper()} {w}×{h} · {size(n)} · {ago(age)}", n, w, h, age, kind)


def summarize(status: Any, images: Any, down: bool = False, caps: Optional[list] = None) -> Summary:
    """One consistent picture from the daemon's status and image state (any may be missing)."""
    daemon_old = caps is not None and "share_image" not in caps
    image = _image(images)
    auto = bool(images.get("auto")) if isinstance(images, dict) else False
    badge = image is not None and not auto
    if down or not isinstance(status, dict):
        return Summary("stopped", "Klip is stopped", "Start it to sync your clipboard", "Klip is stopped",
                       False, (), "", "", "", "", "", 0, None, None, auto, daemon_old)

    peers = []
    for p in status.get("peers") or []:
        if not isinstance(p, dict):
            continue
        peers.append(Peer(str(p.get("name") or "Unnamed device"), str(p.get("fingerprint") or ""),
                          bool(p.get("is_online")), plain_error(p.get("last_error"))))
    online = [p for p in peers if p.online]
    paused = bool(status.get("is_paused"))
    left = _int(status.get("paused_remaining_secs"))
    last_sync = status.get("last_sync_time_secs_ago")

    if paused:
        state, title = "paused", "Paused"
        subtitle = remaining(left) if left > 365 * 24 * 3600 else (f"{remaining(left)} left" if left else "")
    elif not peers:
        state, title, subtitle = "no_devices", "No devices linked", "Add your phone or another computer"
    elif online:
        state, title = "connected", f"Connected to {_names([p.name for p in online])}"
        subtitle = "Nothing synced yet" if not isinstance(last_sync, int) else f"Last sync {ago(last_sync)}"
    else:
        state, title = "waiting", f"Waiting for {_names([p.name for p in peers])}"
        subtitle = next((p.error for p in peers if p.error), "Not connected yet. It reconnects by itself.")

    tooltip = title + (f"\n{subtitle}" if subtitle else "")
    if badge:
        tooltip += f"\nImage ready to send ({size(image.size_bytes)})"
    return Summary(state, title, subtitle, tooltip, badge, tuple(peers),
                   str(status.get("fingerprint") or ""), str(status.get("platform") or ""),
                   size(_int(status.get("memory_rss_bytes"))), str(status.get("relay_state") or ""),
                   str(status.get("relay_mode") or ""), left,
                   status.get("last_clip_preview") if isinstance(status.get("last_clip_preview"), str) else None,
                   image, auto, daemon_old)


# ------------------------------------------------------------------------------------- history


@dataclasses.dataclass(frozen=True)
class HistoryRow:
    id_hex: str
    kind: str  # text | image | secret
    preview: str
    origin: str
    age: str
    size_text: str
    can_copy: bool


def history_rows(items: list, now_ms: int) -> list:
    rows = []
    for it in items or []:
        if not isinstance(it, dict):
            continue
        preview = str(it.get("content_preview") or "")
        kind = "secret" if it.get("is_sensitive") else ("image" if preview.startswith("[Image:") else "text")
        age = ago(max(0, now_ms - _int(it.get("timestamp_ms"))) // 1000)
        rows.append(HistoryRow(str(it.get("id_hex") or ""), kind, preview, str(it.get("origin_name") or ""),
                               age, size(_int(it.get("size_bytes"))), _int(it.get("content_len")) > 0))
    return rows


def filter_rows(rows: list, query: str) -> list:
    q = (query or "").strip().lower()
    if not q:
        return list(rows)
    return [r for r in rows if q in r.origin.lower() or (r.kind != "secret" and q in r.preview.lower())]


# --------------------------------------------------------------------------------------- input


def clean_ticket(text: str) -> str:
    """Whatever a copy and paste brings along (spaces, box borders, quotes) is dropped."""
    return "".join(c for c in text if c.isascii() and c.isalnum()).lower()


def looks_like_ticket(text: str) -> bool:
    t = clean_ticket(text)
    return t.startswith("klip1") and 20 <= len(t) <= 2048


PAUSE_CHOICES = [("10 minutes", 600), ("1 hour", 3600), ("8 hours", 28_800), ("Until I resume", 315_360_000)]


def to_int(text: str, lo: int, hi: int) -> int:
    t = (text or "").strip()
    if not re.fullmatch(r"\d+", t):
        raise ValueError("enter a whole number")
    n = int(t)
    if not lo <= n <= hi:
        raise ValueError(f"enter a number from {lo} to {hi}")
    return n


def send_problem(text: str, limit: int) -> Optional[str]:
    if not text.strip():
        return "Type something to send."
    n = len(text.encode("utf-8"))
    if n > limit:
        return f"That is {size(n)}; the limit is {size(limit)}."
    return None


def config_problems(cfg: dict) -> list:
    """The daemon's own rules (DaemonConfig::validate), checked before anything is saved."""
    problems = []
    name = str(cfg.get("device_name") or "").strip()
    if not name:
        problems.append("The device name cannot be empty.")
    elif len(name) > 64:
        problems.append("The device name can have at most 64 characters.")
    if cfg.get("relay") not in ("n0", "lan"):
        problems.append("The relay mode must be n0 or lan.")
    if _int(cfg.get("poll_interval_ms")) < 20:
        problems.append("The polling interval must be at least 20 ms.")
    clip = _int(cfg.get("max_clip_bytes"))
    if clip < 1 or clip > 256 * 1024 * 1024:
        problems.append("The clip size limit must be between 1 byte and 256 MiB.")
    if _int(cfg.get("history_ttl_hours")) > 24 * 365:
        problems.append("Keep clips for at most 8760 hours (one year); 0 keeps them until the limits push them out.")
    if _int(cfg.get("max_history_items")) < 1:
        problems.append("The history must keep at least 1 item.")
    return problems


def describe_sent(payload: Any) -> tuple[bool, str]:
    """(did anybody get it, one sentence) for the daemon's `Sent` answer."""
    if not isinstance(payload, dict):
        return False, "Klip gave an unexpected answer."
    delivered = [str(x) for x in payload.get("delivered") or []]
    failed = [(str(p[0]), str(p[1])) for p in payload.get("failed") or []
              if isinstance(p, (list, tuple)) and len(p) == 2]
    if not delivered and not failed:
        return False, "No devices are linked yet."
    parts = []
    if delivered:
        parts.append("Sent to " + _names(delivered))
    for name, why in failed:
        parts.append(f"{name} did not get it: {plain_error(why)}")
    return bool(delivered), ". ".join(parts)
