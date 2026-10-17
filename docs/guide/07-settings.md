# Settings

You rarely need to change anything. Two ways to do it:

1. **The full-screen app:** run `klip`, press `7` (Settings), move with `↑↓`, `Enter` to change, `s` to save, `r` to restart Klip so it takes effect.
2. **By hand:** edit the file `config.json` in Klip's settings folder, then restart: `klip stop` and `klip start`.

| System | Settings folder |
|---|---|
| Linux | `~/.config/klip/` |
| macOS | `~/Library/Application Support/com.klip.klip/` |
| Windows | `%APPDATA%\klip\klip\config\` |

The file is optional. Every key is optional. **A typo is an error, never silently ignored** (Klip refuses to start and the log, `daemon.log`, says which key). Android has no file: its switches are on the **Privacy** screen.

```json
{
  "device_name": "ThinkPad",
  "relay": "n0",
  "lan_discovery": true,
  "max_clip_bytes": 33554432,
  "max_history_items": 50,
  "history_ttl_hours": 24,
  "poll_interval_ms": 100,
  "auto_send_images": false,
  "accept_files": true,
  "receive_dir": "/home/you/Downloads/Klip",
  "max_file_bytes": 4294967296,
  "file_protocol_v2": true
}
```

| Key | Default | What it does |
|---|---|---|
| `device_name` | the computer's name | The name your other devices show. |
| `relay` | `"n0"` | `"n0"`: use n0's public relay and discovery servers, so devices on different networks find each other. `"lan"`: **no third-party servers at all**, but both devices must be on the same network. |
| `lan_discovery` | `true` | Announce this device on the local network (mDNS) so same-Wi-Fi devices connect without internet. |
| `max_clip_bytes` | 33554432 (32 MiB) | Biggest clip sent or accepted. (Phone: about 12 MB; about 0.9 MB for text.) |
| `max_history_items` | 50 | How many clips `klip history` keeps. |
| `history_ttl_hours` | 24 | Clips older than this are forgotten. `0` = keep until the count or size limits push them out. History lives in memory only, so a restart clears it anyway. |
| `poll_interval_ms` | 100 | How often to look at the clipboard on systems that can't be told about changes. Leave it. |
| `auto_send_images` | `false` | `false`: a copied image waits for you (tray, `klip share`). `true`: every copied image goes at once. Also changed by `klip images auto` / `manual` and the tray. |
| `accept_files` | `true` | Receive files from linked devices. `false`: every file is refused (the sender is told). |
| `receive_dir` | `Downloads/Klip` | Where received files go. Must be a full path, for example `/home/me/Downloads/Klip` or `C:\\Users\\me\\Downloads\\Klip` (in JSON a backslash is written twice). |
| `max_file_bytes` | 4294967296 (4 GiB) | Biggest file this device accepts. |
| `file_protocol_v2` | `true` | Speak the newer file protocol (resume, folders). `false` makes this device behave like one with only the first version (whole files, no folders): only for tracking down a problem. |

## Environment variables (advanced)

| Variable | Use |
|---|---|
| `KLIP_CONFIG_DIR` | Run a second, separate Klip with its own settings and identity. |
| `KLIP_CLIPBOARD=memory` | Don't touch the real clipboard (servers, demos). |
| `NO_COLOR=1`, `KLIP_THEME=light` | Colours of the full-screen app. |

## Android settings

On the phone, **Privacy** has: *Use relay servers* (same as `relay`), *Receive files* with the size limit (default 2 GiB), and *Restart by itself if stopped*. **Setup** is the list of phone switches that keep Klip alive.

← [Guide start](README.md)
