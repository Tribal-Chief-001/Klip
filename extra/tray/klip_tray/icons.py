"""The tray icons: the Klip mark (two devices joined by a line) in the colour of the state.

Each icon is a rounded dark square, so it reads on dark and light panels alike. A small orange dot in
the corner means an image is waiting to be sent.
"""
from __future__ import annotations

import os
from typing import Optional

INK = "#0B0E10"
LIME = "#C8F25A"
AMBER = "#F2B84B"
GREY = "#7A838A"
BADGE = "#FF8A3D"

STATES = ("connected", "waiting", "no_devices", "paused", "stopped")


def svg(state: str, badge: bool = False) -> str:
    """A 64x64 SVG for a tray state (connected, waiting, no_devices, paused, stopped)."""
    if state not in STATES:
        state = "stopped"
    colour = {"connected": LIME, "waiting": AMBER, "no_devices": GREY, "paused": LIME, "stopped": GREY}[state]
    parts = [f'<rect width="64" height="64" rx="15" fill="{INK}"/>']
    left, right, y = 14, 50, 32
    if state == "connected":
        parts.append(f'<line x1="{left}" y1="{y}" x2="{right}" y2="{y}" stroke="{colour}" '
                     'stroke-width="5" stroke-linecap="round"/>')
    elif state in ("waiting", "no_devices"):
        parts.append(f'<line x1="{left}" y1="{y}" x2="{right}" y2="{y}" stroke="{colour}" stroke-width="4" '
                     'stroke-linecap="round" stroke-dasharray="1 7" opacity="0.9"/>')
    elif state == "paused":
        parts.append(f'<rect x="27" y="23" width="4.5" height="18" rx="2" fill="{colour}"/>'
                     f'<rect x="35" y="23" width="4.5" height="18" rx="2" fill="{colour}"/>')
    for x in (left, right):
        parts.append(f'<circle cx="{x}" cy="{y}" r="8" fill="{colour}"/>')
    if badge:
        parts.append(f'<circle cx="52" cy="12" r="11" fill="{INK}"/><circle cx="52" cy="12" r="8" fill="{BADGE}"/>')
    return ('<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64">'
            + "".join(parts) + "</svg>")


def write_all(directory: str) -> dict:
    """Write every icon into `directory`; returns {(state, badge): path}."""
    os.makedirs(directory, exist_ok=True)
    paths = {}
    for state in STATES:
        for badge in (False, True):
            name = f"klip-{state}{'-badge' if badge else ''}.svg"
            path = os.path.join(directory, name)
            content = svg(state, badge)
            try:
                with open(path) as f:
                    if f.read() == content:
                        paths[(state, badge)] = path
                        continue
            except OSError:
                pass
            with open(path, "w") as f:
                f.write(content)
            paths[(state, badge)] = path
    return paths


def path_for(paths: dict, state: str, badge: bool) -> Optional[str]:
    return paths.get((state if state in STATES else "stopped", bool(badge)))
