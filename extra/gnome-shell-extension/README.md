# 🐧 Klip Bridge GNOME Shell Extension

Lightweight, 40-line GNOME Shell extension enabling seamless clipboard synchronization under GNOME Mutter Wayland sessions.

## Background
GNOME Mutter intentionally restricts the `ext-data-control-v1` Wayland protocol from unprivileged background daemons. This extension interfaces directly with `Meta.Selection` inside the compositor and streams clipboard updates over a secure user socket (`~/.cache/klip/bridge.sock`).

## Installation
```bash
mkdir -p ~/.local/share/gnome-shell/extensions/klip-bridge@klip.org
cp metadata.json extension.js ~/.local/share/gnome-shell/extensions/klip-bridge@klip.org/
gnome-extensions enable klip-bridge@klip.org
```
Once enabled, the Klip background daemon communicates seamlessly with Mutter clipboard events with zero latency and 0.0% CPU overhead.
