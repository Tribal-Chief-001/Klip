"""The application: tray icon, right-click menu, the window and how it opens."""
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

from . import icons, model, theme  # noqa: E402
from .bridge import Bridge  # noqa: E402
from .controller import Controller  # noqa: E402
from .window import HEIGHT, WIDTH, MainView  # noqa: E402

log = logging.getLogger("klip_tray")
FRAME_MARGIN_X, FRAME_MARGIN_TOP, FRAME_MARGIN_BOTTOM = 14, 10, 18
TERMINALS = [("x-terminal-emulator", ["-e"]), ("gnome-terminal", ["--"]), ("konsole", ["-e"]),
             ("xfce4-terminal", ["-x"]), ("kitty", []), ("alacritty", ["-e"]), ("xterm", ["-e"])]


def config_dir() -> str:
    explicit = os.environ.get("KLIP_CONFIG_DIR")
    if explicit:
        return explicit
    return os.path.join(os.environ.get("XDG_CONFIG_HOME", os.path.expanduser("~/.config")), "klip")


def cache_dir() -> str:
    return os.path.join(os.environ.get("XDG_CACHE_HOME", os.path.expanduser("~/.cache")), "klip", "tray")


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


class KlipWindow(Gtk.Window):
    """A frameless, rounded window that hides itself when you click elsewhere."""

    def __init__(self, view: MainView):
        super().__init__(type=Gtk.WindowType.TOPLEVEL)
        self.view = view
        self.busy = False
        self._shown_at = 0
        self.auto_hidden_at = -10**12  # monotonic µs of the last hide caused by losing the focus
        self._hide_timer = 0
        self.set_title("Klip")
        self.set_decorated(False)
        self.set_resizable(False)
        self.set_skip_taskbar_hint(True)
        self.set_skip_pager_hint(True)
        self.set_keep_above(True)
        self.set_type_hint(Gdk.WindowTypeHint.DIALOG)
        full = (WIDTH + 2 * FRAME_MARGIN_X, HEIGHT + FRAME_MARGIN_TOP + FRAME_MARGIN_BOTTOM)
        self.set_size_request(*full)
        self.set_default_size(*full)
        self.set_app_paintable(True)
        visual = self.get_screen().get_rgba_visual()
        if visual is not None:
            self.set_visual(visual)
        self.get_style_context().add_class("klip")
        self.add(view)
        self.connect("delete-event", lambda *_: self.hide_window() or True)
        self.connect("key-press-event", self._key)
        self.connect("notify::is-active", self._active_changed)

    def _key(self, _w, event) -> bool:
        if event.keyval == Gdk.KEY_Escape:
            if self.view.confirming():
                self.view.hide_confirm()
            else:
                self.hide_window()
            return True
        return False

    def _active_changed(self, *_a) -> None:
        if self.is_active() or not self.get_visible():
            return
        if self._hide_timer:
            GLib.source_remove(self._hide_timer)
        self._hide_timer = GLib.timeout_add(250, self._maybe_hide)

    def _maybe_hide(self) -> bool:
        self._hide_timer = 0
        recently = GLib.get_monotonic_time() - self._shown_at < 500_000
        if self.get_visible() and not self.is_active() and not self.busy and not recently \
                and not self.view.popover_open():
            self.auto_hidden_at = GLib.get_monotonic_time()
            self.hide_window()
        return False

    def show_window(self, x: int = 0, y: int = 0, panel: int = 3) -> None:
        self.position(x, y, panel)
        self._shown_at = GLib.get_monotonic_time()
        self.show_all()
        self.present_with_time(Gtk.get_current_event_time() or 0)
        self.view.window_shown(True)

    def hide_window(self) -> None:
        if self.get_visible():
            self.hide()
            self.view.window_shown(False)

    def position(self, x: int, y: int, panel: int) -> None:
        display = Gdk.Display.get_default()
        monitor = display.get_monitor_at_point(x, y) if (x or y) else display.get_primary_monitor()
        monitor = monitor or display.get_monitor(0)
        area = monitor.get_workarea()
        w, h = WIDTH + 2 * FRAME_MARGIN_X, HEIGHT + FRAME_MARGIN_TOP + FRAME_MARGIN_BOTTOM
        if not (x or y):
            nx, ny = area.x + area.width - w - 8, area.y + area.height - h - 8
        elif panel == Gtk.PositionType.TOP:
            nx, ny = x - w // 2, area.y - FRAME_MARGIN_TOP + 4
        elif panel == Gtk.PositionType.LEFT:
            nx, ny = area.x - FRAME_MARGIN_X + 4, y - h // 2
        elif panel == Gtk.PositionType.RIGHT:
            nx, ny = area.x + area.width - w + FRAME_MARGIN_X - 4, y - h // 2
        else:  # a panel at the bottom (the usual)
            nx, ny = x - w // 2, area.y + area.height - h + FRAME_MARGIN_BOTTOM - 4
        nx = max(area.x, min(nx, area.x + area.width - w))
        ny = max(area.y, min(ny, area.y + area.height - h))
        self.move(nx, ny)


class Actions:
    """What the window can ask the app to do."""

    def __init__(self, app: "TrayApp"):
        self.app = app

    def hide_window(self) -> None:
        self.app.window.hide_window() if self.app.window else None

    def show_window(self) -> None:
        self.app.show_window()

    def open_terminal(self) -> None:
        self.app.open_terminal()

    def quit_tray(self) -> None:
        self.app.quit()

    def set_busy(self, flag: bool) -> None:
        if self.app.window:
            self.app.window.busy = flag


class TrayApp(Gtk.Application):
    def __init__(self, klip_bin: str = "klip", show_on_start: bool = False):
        super().__init__(application_id="org.klip.Tray", flags=Gio.ApplicationFlags.FLAGS_NONE)
        self.klip_bin = klip_bin
        self.show_on_start = show_on_start
        self.window: Optional[KlipWindow] = None
        self.view: Optional[MainView] = None
        self.icon: Optional[XApp.StatusIcon] = None
        self.controller: Optional[Controller] = None
        self.bridge: Optional[Bridge] = None
        self._icon_paths: dict = {}
        self._click = (0, 0, 3)
        self._open_at_press = False
        self._support_checks = 0
        self._first_activate = True
        self._menu_items: dict = {}

    # ------------------------------------------------------------------------------- startup

    def do_startup(self) -> None:
        Gtk.Application.do_startup(self)
        provider = Gtk.CssProvider()
        provider.load_from_data(theme.CSS.encode())
        Gtk.StyleContext.add_provider_for_screen(Gdk.Screen.get_default(), provider,
                                                 Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
        self.bridge = Bridge([self.klip_bin, "api"], post=lambda fn: GLib.idle_add(self._run_posted, fn))
        self.controller = Controller(self.bridge, self._schedule, GLib.source_remove)
        self.controller.listeners.append(self.on_summary)
        self._icon_paths = icons.write_all(cache_dir())
        self._build_icon()
        self.hold()  # stay alive with no window open
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
        # Autostart runs us once with no window; running `klip tray` again asks this process to show it.
        if self._first_activate and not self.show_on_start:
            self._first_activate = False
            return
        self._first_activate = False
        self.show_window()

    def do_shutdown(self) -> None:
        if self.bridge:
            self.bridge.close()
        Gtk.Application.do_shutdown(self)

    # ---------------------------------------------------------------------------------- icon

    def _build_icon(self) -> None:
        self.icon = XApp.StatusIcon()
        self.icon.set_name("klip")
        self.icon.set_icon_name(icons.path_for(self._icon_paths, "stopped", False))
        self.icon.set_tooltip_text("Klip")
        self.icon.connect("button-press-event", self._icon_press)
        self.icon.connect("button-release-event", self._icon_release)
        self.icon.connect("activate", self._icon_activate)
        self.menu = self._build_menu()
        self.icon.set_secondary_menu(self.menu)
        self.icon.set_visible(True)

    def _icon_press(self, _icon, x, y, button, _time, panel) -> None:
        self._click = (int(x), int(y), int(panel))
        w = self.window
        self._open_at_press = bool(w is not None and w.get_visible() and w.is_active())

    def _icon_release(self, _icon, _x, _y, button, _time, _panel) -> None:
        if button == 2:  # middle click: send the waiting image
            self.send_waiting_image()

    def _icon_activate(self, _icon, button, _time) -> None:
        if button != 1:
            return
        self.toggle_window()

    def _check_tray_support(self) -> bool:
        """With no panel to hold the icon, a window is the only way in.

        At login the panel may start a few seconds after us, so this asks again for half a minute
        before it gives up on the tray."""
        if self.icon is None or self.icon.get_state() != XApp.StatusIconState.NO_SUPPORT:
            return False
        self._support_checks += 1
        if self._support_checks < 6:
            GLib.timeout_add(5000, self._check_tray_support)
            return False
        log.warning("no system tray found; opening the window instead")
        self.show_window()
        if self.view:
            self.view.toast("No system tray found: using this window instead.")
        return False

    # ---------------------------------------------------------------------------------- menu

    def _build_menu(self) -> Gtk.Menu:
        menu = Gtk.Menu()
        items = self._menu_items

        def add(key: str, item: Gtk.MenuItem, handler=None, sensitive=True) -> Gtk.MenuItem:
            if handler:
                item.connect("activate", lambda _i: handler())
            item.set_sensitive(sensitive)
            menu.append(item)
            items[key] = item
            return item

        add("title", Gtk.MenuItem(label="Klip"), sensitive=False)
        menu.append(Gtk.SeparatorMenuItem())
        add("image", Gtk.MenuItem(label="Send the last copied image"), self.send_waiting_image, sensitive=False)
        add("open", Gtk.MenuItem(label="Open Klip"), self.show_window)
        pause = add("pause", Gtk.MenuItem(label="Pause syncing"))
        sub = Gtk.Menu()
        for text, secs in model.PAUSE_CHOICES:
            it = Gtk.MenuItem(label=text)
            it.connect("activate", lambda _i, s=secs: self.controller.pause(s))
            sub.append(it)
        pause.set_submenu(sub)
        resume = add("resume", Gtk.MenuItem(label="Resume syncing"), lambda: self.controller.resume())
        resume.set_no_show_all(True)
        auto = Gtk.CheckMenuItem(label="Send images automatically")
        auto.connect("toggled", self._auto_toggled)
        add("auto", auto)
        menu.append(Gtk.SeparatorMenuItem())
        add("pair", Gtk.MenuItem(label="Add a device…"), lambda: self.show_window(page="pair"))
        add("terminal", Gtk.MenuItem(label="Open the terminal app"), self.open_terminal)
        menu.append(Gtk.SeparatorMenuItem())
        add("quit", Gtk.MenuItem(label="Quit the tray"), self.quit)
        menu.show_all()
        resume.hide()
        return menu

    def _auto_toggled(self, item: Gtk.CheckMenuItem) -> None:
        if getattr(self, "_menu_updating", False):
            return
        self.controller.set_auto_images(item.get_active(), self._toast_result("Images are sent automatically"
                                                                              if item.get_active()
                                                                              else "Images wait until you send them"))

    def _toast_result(self, ok_text: str):
        def done(out):
            if self.view:
                self.view.toast(ok_text if out.ok else out.error, bad=not out.ok)

        return done

    # -------------------------------------------------------------------------------- updates

    def on_summary(self, s: model.Summary) -> None:
        if self.icon is not None:
            path = icons.path_for(self._icon_paths, s.state, s.badge)
            if path:
                self.icon.set_icon_name(path)
            self.icon.set_tooltip_text(s.tooltip)
        it = self._menu_items
        if not it:
            return
        it["title"].set_label(s.title)
        it["image"].set_sensitive(bool(s.image) and any(p.online for p in s.peers) and s.state != "paused")
        it["image"].set_label(f"Send the last copied image ({model.size(s.image.size_bytes)})" if s.image
                              else "Send the last copied image")
        paused = s.state == "paused"
        it["pause"].set_visible(not paused)
        it["resume"].set_visible(paused)
        stopped = s.state == "stopped"
        for key in ("pause", "resume", "auto", "pair"):
            it[key].set_sensitive(not stopped)
        self._menu_updating = True
        it["auto"].set_active(s.auto_images)
        self._menu_updating = False

    # --------------------------------------------------------------------------------- actions

    def _ensure_window(self) -> KlipWindow:
        if self.window is None:
            self.view = MainView(self.controller, Actions(self))
            self.window = KlipWindow(self.view)
            self.add_window(self.window)
        return self.window

    def show_window(self, page: Optional[str] = None) -> None:
        window = self._ensure_window()
        x, y, panel = self._click
        if not window.get_visible():
            window.show_window(x, y, panel)
        else:
            window.present_with_time(Gtk.get_current_event_time() or 0)
        if page and self.view:
            self.view.navigate(page)
        self.controller.refresh()

    def toggle_window(self) -> None:
        w = self.window
        since = (GLib.get_monotonic_time() - w.auto_hidden_at) / 1000 if w is not None else 10**9
        action = model.click_action(self._open_at_press, bool(w and w.get_visible()),
                                    bool(w and w.is_active()), since)
        self._open_at_press = False
        if action == "hide":
            w.hide_window()
        elif action == "show":
            self.show_window()

    def send_waiting_image(self) -> None:
        def done(out):
            if self.view:
                self.view.toast(out.value if out.ok else out.error, bad=not out.ok)
            if not out.ok:
                log.info("send image: %s", out.error)
                self._notify_failure(out.error)

        self.controller.send_pending_image(done)

    def _notify_failure(self, text: str) -> None:
        """A middle click has no window to show a toast in: say it once, in words."""
        if self.window is None or not self.window.get_visible():
            try:
                subprocess.Popen(["notify-send", "--app-name=Klip", "Klip", text or "Could not send the image"],
                                 stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            except OSError:
                pass

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
        if self.view:
            self.view.toast("No terminal program found. Run klip in a terminal.", bad=True)
