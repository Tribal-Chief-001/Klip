"""The tray icons: the Klip mark (two devices joined by a line) as a monochrome symbolic icon.

Like the other panel icons they have no colour of their own; the desktop paints them in the panel's
foreground colour. The state is the shape: a solid line = connected, dots = waiting, bars = paused,
faded = no device or stopped. A solid dot in the corner = an image is waiting to be sent.
"""
from __future__ import annotations

import os

STATES = ("connected", "waiting", "no_devices", "paused", "stopped")
FADED = {"no_devices": 0.55, "stopped": 0.3}


def svg(state: str, badge: bool = False) -> str:
    """A 64x64 symbolic SVG (fills only: the theme recolours fills, not strokes)."""
    if state not in STATES:
        state = "stopped"
    parts = []
    if state == "connected":
        parts.append('<rect x="14" y="29.5" width="36" height="5" rx="2.5"/>')
    elif state == "waiting":
        parts += [f'<circle cx="{x}" cy="32" r="2.6"/>' for x in (26, 32, 38)]
    elif state == "paused":
        parts += ['<rect x="26.5" y="23" width="4.5" height="18" rx="2"/>',
                  '<rect x="33" y="23" width="4.5" height="18" rx="2"/>']
    parts += [f'<circle cx="{x}" cy="32" r="9"/>' for x in (14, 50)]
    body = "".join(parts)
    if state in FADED:
        body = f'<g opacity="{FADED[state]}">{body}</g>'
    if badge:
        body += '<circle cx="52" cy="12" r="10"/>'
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64">{body}</svg>'


def name_for(state: str, badge: bool) -> str:
    return f"klip-{state if state in STATES else 'stopped'}{'-badge' if badge else ''}-symbolic"


def install(data_home: str) -> None:
    """Put every icon where the panel (another process) finds it: the user's icon theme directory."""
    directory = os.path.join(data_home, "icons", "hicolor", "scalable", "status")
    os.makedirs(directory, exist_ok=True)
    for state in STATES:
        for badge in (False, True):
            path = os.path.join(directory, name_for(state, badge) + ".svg")
            content = svg(state, badge)
            try:
                with open(path) as f:
                    if f.read() == content:
                        continue
            except OSError:
                pass
            tmp = f"{path}.{os.getpid()}.tmp"
            with open(tmp, "w") as f:
                f.write(content)
            os.replace(tmp, path)
