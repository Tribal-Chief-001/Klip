"""`python3 -m klip_tray [--show] [--klip PATH]` (started by `klip tray`)."""
import argparse
import os
import sys


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(prog="klip tray")
    parser.add_argument("--show", action="store_true", help="open the window at start")
    parser.add_argument("--klip", default=os.environ.get("KLIP_BIN", "klip"), help="the klip program to talk to")
    args = parser.parse_args(argv)
    try:
        from .app import TrayApp, setup_logging
    except (ImportError, ValueError) as e:
        print(f"The Klip tray needs PyGObject with GTK 3 and XApp (Linux Mint: already installed; Debian/Ubuntu: "
              f"python3-gi gir1.2-gtk-3.0 gir1.2-xapp-1.0).\n({e})", file=sys.stderr)
        return 2
    setup_logging()
    from gi.repository import GLib
    GLib.set_prgname("klip-tray")  # the window class the desktop shows (taskbar, alt-tab), not "__main__.py"
    GLib.set_application_name("Klip")
    app = TrayApp(args.klip, args.show)
    return app.run([sys.argv[0]])


if __name__ == "__main__":
    sys.exit(main())
