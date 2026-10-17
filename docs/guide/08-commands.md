# Commands

On Windows use PowerShell; everything is the same. Run `klip --help` or `klip <command> --help` for the built-in help. `klip` with no command opens the full-screen app.

| Command | What it does |
|---|---|
| `klip` | open the full-screen app (needs a real terminal; in a script it prints the help) |
| `klip start [--foreground]` | start the background service (`--foreground` is for service managers) |
| `klip stop` | stop it |
| `klip status` | devices, connection, relay, last sync |
| `klip pair [--lan] [--ttl SECONDS]` | show a QR code + ticket and wait for a device (`--ttl` 30-600, default 120) |
| `klip pair <ticket>` | join the device that showed a ticket (starts with `klip1`) |
| `klip unpair <name\|fingerprint>` | forget one device |
| `klip unpair --all [-y]` | forget every device (asks first unless `-y`) |
| `klip pause [seconds]` | stop syncing for a while (default 600) |
| `klip resume` | start syncing again |
| `klip history [-l N]` | recent clips (secrets masked; forgotten after 24 hours by default) |
| `klip history clear` | forget all clips kept on this computer |
| `klip send "<text>"` | send text now, with the result per device (`klip send -` reads standard input) |
| `klip share` | send the image you copied last (held 10 minutes), with the result per device |
| `klip send-file FILE... [--to NAME] [--no-wait]` | send files or folders; exit code 1 if any did not arrive |
| `klip transfers [--cancel ID]` | running and recent file transfers; stop one |
| `klip images [auto\|manual]` | show or choose whether copied images go at once or wait for `klip share` |
| `klip tray [start\|run\|install\|uninstall\|status]` | the tray icon: start it, or make it start at login (works on Linux, macOS and Windows) |
| `klip install-service` | create the start-at-login service for the background part (see your [platform page](README.md)) |

**Useful combinations**

```bash
klip start && klip status            # start and check
klip send-file ~/Pictures/Trip       # a folder
klip pause 1800                      # half an hour of privacy
klip unpair --all -y                 # reset: forget everybody
```

← [Guide start](README.md)
