"""The tray icon and its menu. Everything else (devices, pairing, history, settings) is the terminal app."""
from __future__ import annotations

import logging
import logging.handlers
import os
import shutil
import subprocess
import sys
from typing import Optional

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
gi.require_version("XApp", "1.0")
from gi.repository import Gdk, Gio, GLib, Gtk, XApp  # noqa: E402

from . import icons, model  # noqa: E402
from .bridge import Bridge  # noqa: E402
from .controller import Controller  # noqa: E402

log = logging.getLogger("klip_tray")
TERMINALS = [("x-terminal-emulator", ["-e"]), ("gnome-terminal", ["--"]), ("konsole", ["-e"]),
             ("xfce4-terminal", ["-x"]), ("kitty", []), ("alacritty", ["-e"]), ("xterm", ["-e"])]
# The menu is drawn by this process with the GTK theme; a quiet hover instead of the accent colour,
# like the menus the panel's own applets show.
CSS = b"menuitem:hover { background-image: none; background-color: alpha(@theme_fg_color, 0.1); }"


def config_dir() -> str:
    explicit = os.environ.get("KLIP_CONFIG_DIR")
    if explicit:
        return explicit
    return os.path.join(os.environ.get("XDG_CONFIG_HOME", os.path.expanduser("~/.config")), "klip")


def data_home() -> str:
    return os.environ.get("XDG_DATA_HOME", os.path.expanduser("~/.local/share"))


def setup_logging() -> None:
    root = logging.getLogger("klip_tray")
    root.setLevel(logging.INFO)
    try:
        os.makedirs(config_dir(), mode=0o700, exist_ok=True)
        path = os.path.join(config_dir(), "tray.log")
        handler = logging.handlers.RotatingFileHandler(path, maxBytes=512 * 1024, backupCount=1)
        os.chmod(path, 0o600)
        handler.setFormatter(logging.Formatter("%(asctime)s %(levelname)s %(message)s"))
        root.addHandler(handler)
    except OSError:
        root.addHandler(logging.StreamHandler())
    sys.excepthook = lambda *a: log.error("uncaught exception", exc_info=a)


def notify(text: str) -> None:
    """The menu has no place for a message: say it once, in words."""
    try:
        subprocess.Popen(["notify-send", "--app-name=Klip", "Klip", text], stdin=subprocess.DEVNULL,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    except OSError:
        pass


class TrayApp(Gtk.Application):
    def __init__(self, klip_bin: str = "klip"):
        super().__init__(application_id="org.klip.Tray", flags=Gio.ApplicationFlags.FLAGS_NONE)
        self.klip_bin = klip_bin
        self.icon: Optional[XApp.StatusIcon] = None
        self.controller: Optional[Controller] = None
        self.bridge: Optional[Bridge] = None
        self._support_checks = 0
        self._menu_items: dict = {}
        self._menu_updating = False

    def do_startup(self) -> None:
        Gtk.Application.do_startup(self)
        provider = Gtk.CssProvider()
        provider.load_from_data(CSS)
        Gtk.StyleContext.add_provider_for_screen(Gdk.Screen.get_default(), provider,
                                                 Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
        self.bridge = Bridge([self.klip_bin, "api"], post=lambda fn: GLib.idle_add(self._run_posted, fn))
        self.controller = Controller(self.bridge, self._schedule, GLib.source_remove)
        self.controller.listeners.append(self.on_summary)
        try:
            icons.install(data_home())
        except OSError:
            log.exception("could not write the tray icons")
        self._build_icon()
        self.hold()  # no window: the icon is all there is
        self.controller.refresh()
        GLib.timeout_add(2500, self._check_tray_support)

    @staticmethod
    def _run_posted(fn) -> bool:
        try:
            fn()
        except Exception:
            log.exception("a posted callback failed")
        return False

    @staticmethod
    def _schedule(ms: int, fn) -> int:
        def run() -> bool:
            try:
                fn()
            except Exception:
                log.exception("a scheduled callback failed")
            return False

        return GLib.timeout_add(ms, run)

    def do_activate(self) -> None:
        pass  # a second `klip tray` has nothing to show: the icon is already there

    def do_shutdown(self) -> None:
        if self.bridge:
            self.bridge.close()
        Gtk.Application.do_shutdown(self)

    # ---------------------------------------------------------------------------------- icon

    def _build_icon(self) -> None:
        self.icon = XApp.StatusIcon()
        self.icon.set_name("klip")
        self.icon.set_icon_name(icons.name_for("stopped", False))
        self.icon.set_tooltip_text("Klip")
        self.icon.connect("button-release-event", self._icon_release)
        menu = self._build_menu()
        self.icon.set_primary_menu(menu)  # a left click opens the same menu as a right click
        self.icon.set_secondary_menu(menu)
        self.icon.set_visible(True)

    def _icon_release(self, _icon, _x, _y, button, _time, _panel) -> None:
        if button == 2:  # middle click: send the waiting image
            self.send_waiting_image()

    def _check_tray_support(self) -> bool:
        """At login the panel may start a few seconds after us; only after half a minute is it missing."""
        if self.icon is None or self.icon.get_state() != XApp.StatusIconState.NO_SUPPORT:
            return False
        self._support_checks += 1
        if self._support_checks < 6:
            GLib.timeout_add(5000, self._check_tray_support)
        else:
            log.warning("no system tray found")
            notify("There is no system tray to show the Klip icon. Run klip in a terminal instead.")
        return False

    # ---------------------------------------------------------------------------------- menu

    def _build_menu(self) -> Gtk.Menu:
        menu = Gtk.Menu()
        items = self._menu_items

        def add(key: str, item: Gtk.MenuItem, handler=None) -> Gtk.MenuItem:
            if handler:
                item.connect("activate", lambda _i: handler())
            menu.append(item)
            items[key] = item
            return item

        add("title", Gtk.MenuItem(label="Klip")).set_sensitive(False)
        menu.append(Gtk.SeparatorMenuItem())
        add("start", Gtk.MenuItem(label="Start Klip"), lambda: self.controller.daemon_start(self._report))
        add("image", Gtk.MenuItem(label="Send the last copied image"), self.send_waiting_image)
        auto = Gtk.CheckMenuItem(label="Send images automatically")
        auto.connect("toggled", self._auto_toggled)
        add("auto", auto)
        pause = add("pause", Gtk.MenuItem(label="Pause syncing"))
        sub = Gtk.Menu()
        for text, secs in model.PAUSE_CHOICES:
            it = Gtk.MenuItem(label=text)
            it.connect("activate", lambda _i, s=secs: self.controller.pause(s, self._report))
            sub.append(it)
        pause.set_submenu(sub)
        add("resume", Gtk.MenuItem(label="Resume syncing"), lambda: self.controller.resume(self._report))
        menu.append(Gtk.SeparatorMenuItem())
        add("terminal", Gtk.MenuItem(label="Open the Klip terminal app"), self.open_terminal)
        add("quit", Gtk.MenuItem(label="Quit the tray"), self.quit)
        menu.connect("show", lambda _m: self.controller.refresh())  # the picture is fresh when it opens
        menu.show_all()
        self.on_summary(self.controller.summary, items)
        return menu

    def _auto_toggled(self, item: Gtk.CheckMenuItem) -> None:
        if not self._menu_updating:
            self.controller.set_auto_images(item.get_active(), self._report)

    def _report(self, out) -> None:
        if not out.ok:
            log.info("action failed: %s", out.error)
            notify(out.error)

    def on_summary(self, s: model.Summary, items: Optional[dict] = None) -> None:
        if self.icon is not None:
            self.icon.set_icon_name(icons.name_for(s.state, s.badge))
            self.icon.set_tooltip_text(s.tooltip)
        it = items or self._menu_items
        if not it:
            return
        stopped, paused = s.state == "stopped", s.state == "paused"
        it["title"].set_label(s.title)
        it["image"].set_label(f"Send the last copied image ({model.size(s.image.size_bytes)})" if s.image
                              else "Send the last copied image")
        it["image"].set_sensitive(bool(s.image) and any(p.online for p in s.peers) and not paused and not stopped)
        for key, shown in (("start", stopped), ("image", not stopped), ("auto", not stopped),
                           ("pause", not stopped and not paused), ("resume", paused)):
            it[key].set_visible(shown)
        self._menu_updating = True
        it["auto"].set_active(s.auto_images)
        self._menu_updating = False

    # --------------------------------------------------------------------------------- actions

    def send_waiting_image(self) -> None:
        def done(out):
            if not out.ok:
                log.info("send image: %s", out.error)
                notify(out.error or "Could not send the image")

        self.controller.send_pending_image(done)

    def open_terminal(self) -> None:
        for terminal, args in TERMINALS:
            path = shutil.which(terminal)
            if path:
                try:
                    subprocess.Popen([path, *args, self.klip_bin], stdin=subprocess.DEVNULL,
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
                    return
                except OSError:
                    continue
        notify("No terminal program found. Run klip in a terminal.")
