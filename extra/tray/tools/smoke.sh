#!/bin/sh
# Run the tray against a real isolated daemon in a nested X server and save a screenshot.
#   Xephyr :99 -screen 1100x900 -ac &   then   extra/tray/tools/smoke.sh /path/to/klip OUT.png [page-click-script]
set -e
KB="$1"; OUT="$2"
export KLIP_CONFIG_DIR=/tmp/klip-smoke/cfg KLIP_CLIPBOARD=memory
rm -rf /tmp/klip-smoke; mkdir -p "$KLIP_CONFIG_DIR"
echo '{"relay":"lan","lan_discovery":false}' > "$KLIP_CONFIG_DIR/config.json"
"$KB" start >/dev/null
DISPLAY=:99 XDG_CACHE_HOME=/tmp/klip-smoke/cache dbus-run-session -- sh -c "
  cd \$(dirname \$0)/..; python3 -m klip_tray --show --klip '$KB' > /tmp/klip-smoke/tray.out 2>&1 &
  TP=\$!; sleep 6
  kill -0 \$TP && echo 'tray alive'
  gnome-screenshot -f '$OUT' >/dev/null 2>&1
  kill \$TP; wait \$TP 2>/dev/null" "$0" || true
"$KB" stop >/dev/null || true
