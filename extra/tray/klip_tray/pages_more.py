"""More (links, start at login, about), Log and Settings."""
from __future__ import annotations

import os
from typing import Optional

import gi

gi.require_version("Gtk", "3.0")
from gi.repository import Gtk  # noqa: E402

from . import model  # noqa: E402
from .pages import Page, back_header, switch_row  # noqa: E402
from .widgets import (Ticker, button, card, classes, clipboard_set, hbox, label, scrolled, show,  # noqa: E402
                      vbox)


def menu_row(icon: str, title: str, hint: str, on_click) -> Gtk.Button:
    b = Gtk.Button()
    classes(b, ["menurow"])
    b.set_relief(Gtk.ReliefStyle.NONE)
    img = Gtk.Image.new_from_icon_name(icon, Gtk.IconSize.LARGE_TOOLBAR)
    chevron = Gtk.Image.new_from_icon_name("go-next-symbolic", Gtk.IconSize.BUTTON)
    row = hbox(img, vbox(label(title), label(hint, ["dim", "small"]), spacing=1), spacing=12)
    row.pack_end(chevron, False, False, 0)
    b.add(row)
    b.connect("clicked", lambda _b: on_click())
    return b


class MorePage(Page):
    name = "more"
    title = "More"

    def __init__(self, ui):
        super().__init__(ui)
        self._updating = False
        links = vbox(
            menu_row("preferences-system-symbolic", "Settings", "Name, network, history, limits", lambda: ui.navigate("settings")),
            menu_row("text-x-generic-symbolic", "Log", "What Klip has been doing", lambda: ui.navigate("log")),
            menu_row("utilities-terminal-symbolic", "Open the terminal app", "The full-screen klip", ui.actions.open_terminal),
            spacing=2)
        daemon_row, self.daemon_switch = switch_row("Start Klip when I log in", "Keeps syncing without opening anything.",
                                                    self._daemon_login_changed)
        tray_row, self.tray_switch = switch_row("Show this tray icon when I log in", "Starts this app in your panel.",
                                                self._tray_login_changed)
        self.tray_row = tray_row
        startup = card(label("STARTUP", ["section"]), daemon_row, tray_row, spacing=12)
        self.about = label("", ["dim", "small"], wrap=True, selectable=True)
        about = card(label("ABOUT", ["section"]), self.about, spacing=6)
        quit_btn = button("Quit the tray", ["flat", "danger"], ui.actions.quit_tray)
        quit_btn.set_halign(Gtk.Align.START)
        note = label("Quitting the tray does not stop Klip: it keeps syncing in the background.", ["faint", "small"],
                     wrap=True)
        body = vbox(links, startup, about, quit_btn, note, spacing=12)
        self.root.pack_start(scrolled(body), True, True, 0)
        self.root.show_all()

    def on_show(self) -> None:
        c = self.ui.controller
        c.service_state(self._service_arrived)
        c.autostart_state(self._autostart_arrived)

    def update(self, s: model.Summary) -> None:
        hello = self.ui.controller.hello if isinstance(self.ui.controller.hello, dict) else {}
        tray = os.environ.get("KLIP_VERSION", "")
        parts = [f"klip {tray}" if tray else "klip tray", f"daemon {hello.get('daemon_version', 'not running')}"]
        if hello.get("protocol"):
            parts.append(f"protocol {hello['protocol']}")
        self.about.set_text(" · ".join(parts) + "\nhttps://github.com/Tribal-Chief-001/Klip")

    def _set(self, switch: Gtk.Switch, on: bool) -> None:
        self._updating = True
        switch.set_active(on)
        switch.set_state(on)
        self._updating = False

    def _service_arrived(self, out) -> None:
        if out.ok and isinstance(out.value, dict):
            supported = bool(out.value.get("supported"))
            show(self.daemon_switch.get_parent(), supported)
            self._set(self.daemon_switch, bool(out.value.get("enabled")))

    def _autostart_arrived(self, out) -> None:
        if out.ok and isinstance(out.value, dict):
            show(self.tray_row, True)
            self._set(self.tray_switch, bool(out.value.get("installed")))
        else:
            show(self.tray_row, False)  # an older klip without this: nothing to offer

    def _daemon_login_changed(self, on: bool) -> None:
        if self._updating:
            return

        def done(out):
            if not out.ok:
                self.ui.toast(out.error, bad=True)
                self._set(self.daemon_switch, not on)
            else:
                self.ui.toast("Klip will start when you log in" if on else "Klip will not start at login")

        self.ui.controller.service_set(on, done)

    def _tray_login_changed(self, on: bool) -> None:
        if self._updating:
            return

        def done(out):
            if not out.ok:
                self.ui.toast(out.error, bad=True)
                self._set(self.tray_switch, not on)
            else:
                self.ui.toast("The tray icon will start at login" if on else "The tray icon will not start at login")

        self.ui.controller.autostart_set(on, done)


class LogPage(Page):
    name = "log"
    title = "Log"
    parent = "more"
    MAX_LINES = 4000

    def __init__(self, ui):
        super().__init__(ui)
        self._offset: Optional[int] = None
        self._lines: list = []
        self._inflight = False
        self._ticker = Ticker(self._poll)

        self.follow = Gtk.Switch()
        self.follow.set_active(True)
        self.follow.set_valign(Gtk.Align.CENTER)
        self.search = Gtk.Entry()
        self.search.set_placeholder_text("Search the log")
        self.search.connect("changed", lambda _e: self._redraw())
        top = hbox(label("Follow", ["dim"]), self.follow, spacing=8)
        top.pack_start(self.search, True, True, 0)
        top.pack_end(button("Copy all", ["flat"], self._copy), False, False, 0)
        self.view = Gtk.TextView()
        self.view.set_editable(False)
        self.view.set_cursor_visible(False)
        self.view.set_wrap_mode(Gtk.WrapMode.CHAR)
        self.view.set_left_margin(6)
        self.view.set_right_margin(6)
        self.scroll = scrolled(self.view)
        self.editor = classes(vbox(self.scroll), ["editor", "mono"])
        self.status = label("", ["faint", "small"])
        self.root.pack_start(back_header("Log", lambda: ui.navigate("more")), False, False, 0)
        self.root.pack_start(top, False, False, 0)
        self.root.pack_start(self.editor, True, True, 0)
        self.root.pack_start(self.status, False, False, 0)
        self.root.show_all()

    def on_show(self) -> None:
        self._offset = None
        self._lines = []
        self._poll()
        self._ticker.start(1500)

    def on_hide(self) -> None:
        self._ticker.stop()

    def _poll(self) -> None:
        if self._inflight:
            return
        self._inflight = True

        def got(out):
            self._inflight = False
            if not out.ok:
                self.status.set_text(out.error)
                return
            v = out.value
            if v.get("reset"):
                self._lines = []
            if v.get("missing"):
                self.status.set_text("There is no log yet (Klip writes one when it starts in the background).")
            else:
                self.status.set_text("")
            self._offset = v.get("new_offset")
            text = v.get("text") or ""
            if text:
                self._lines.extend(text.splitlines())
                del self._lines[:-self.MAX_LINES]
                self._redraw()

        self.ui.controller.log_tail(self._offset, got)

    def _redraw(self) -> None:
        q = self.search.get_text().strip().lower()
        lines = [ln for ln in self._lines if q in ln.lower()] if q else self._lines
        self.view.get_buffer().set_text("\n".join(lines))
        if self.follow.get_active():
            end = self.view.get_buffer().get_end_iter()
            self.view.scroll_to_iter(end, 0.0, False, 0.0, 0.0)

    def _copy(self) -> None:
        clipboard_set("\n".join(self._lines))
        self.ui.toast("Log copied")


class SettingsPage(Page):
    name = "settings"
    title = "Settings"
    parent = "more"

    RELAYS = [("n0", "Public relays (works across networks)"), ("lan", "Local network only (no third-party servers)")]

    def __init__(self, ui):
        super().__init__(ui)
        self._loaded: Optional[dict] = None
        self.name_entry = Gtk.Entry()
        self.name_entry.set_max_length(64)
        self.relay = Gtk.ComboBoxText()
        for key, text in self.RELAYS:
            self.relay.append(key, text)
        lan_row, self.lan_switch = switch_row("Find devices on this network",
                                              "Announces this device on the local network (mDNS).", lambda on: None)
        self.history_items = self._spin(1, 1000)
        self.history_hours = self._spin(0, 8760)
        self.clip_mb = self._spin(1, 256)
        self.poll_ms = self._spin(20, 5000)
        self.error = label("", ["bad"], wrap=True)
        self.note = label("", ["warn", "small"], wrap=True)
        self.restart_btn = button("Restart Klip now", [], self._restart)
        self.save_btn = button("Save", ["primary"], self._save)
        self.discard_btn = button("Discard", [], self.on_show)

        def field(title, widget, hint=""):
            box = vbox(label(title), widget, spacing=4)
            if hint:
                box.pack_start(label(hint, ["faint", "small"], wrap=True), False, False, 0)
            return box

        form = vbox(
            card(label("THIS DEVICE", ["section"]), field("Name", self.name_entry,
                                                          "How other devices see this one."), spacing=10),
            card(label("NETWORK", ["section"]), field("How devices find each other", self.relay), lan_row, spacing=10),
            card(label("HISTORY AND LIMITS", ["section"]),
                 field("Keep up to (clips)", self.history_items),
                 field("Forget clips after (hours, 0 = never)", self.history_hours),
                 field("Largest clip (MB)", self.clip_mb, "Android phones can take about 12 MB."), spacing=10),
            card(label("ADVANCED", ["section"]),
                 field("Clipboard check interval (ms)", self.poll_ms,
                       "Only used when the system cannot tell Klip about a change."), spacing=10),
            self.error, self.note, spacing=12)
        actions = hbox(self.discard_btn, self.save_btn, spacing=8)
        actions.set_halign(Gtk.Align.END)
        self.root.pack_start(back_header("Settings", lambda: ui.navigate("more")), False, False, 0)
        self.root.pack_start(scrolled(form), True, True, 0)
        self.root.pack_start(self.restart_btn, False, False, 0)
        self.root.pack_start(actions, False, False, 0)
        self.root.show_all()
        show(self.restart_btn, False)

    @staticmethod
    def _spin(lo: int, hi: int) -> Gtk.SpinButton:
        return Gtk.SpinButton.new_with_range(lo, hi, 1)

    def on_show(self) -> None:
        self.error.set_text("")
        self.ui.controller.config_get(self._loaded_cb)

    def _loaded_cb(self, out) -> None:
        if not out.ok or not isinstance(out.value, dict):
            self.error.set_text(out.error or "Could not read the settings.")
            return
        c = out.value
        self._loaded = dict(c)
        self.name_entry.set_text(str(c.get("device_name", "")))
        self.relay.set_active_id(str(c.get("relay", "n0")))
        self.lan_switch.set_active(bool(c.get("lan_discovery", True)))
        self.lan_switch.set_state(bool(c.get("lan_discovery", True)))
        self.history_items.set_value(int(c.get("max_history_items", 50)))
        self.history_hours.set_value(int(c.get("history_ttl_hours", 24)))
        self.clip_mb.set_value(max(1, int(c.get("max_clip_bytes", 32 * 1024 * 1024)) // (1024 * 1024)))
        self.poll_ms.set_value(int(c.get("poll_interval_ms", 100)))

    def _collect(self) -> dict:
        cfg = dict(self._loaded or {})
        cfg.update({
            "device_name": self.name_entry.get_text().strip(),
            "relay": self.relay.get_active_id() or "n0",
            "lan_discovery": self.lan_switch.get_active(),
            "max_history_items": int(self.history_items.get_value()),
            "history_ttl_hours": int(self.history_hours.get_value()),
            "max_clip_bytes": int(self.clip_mb.get_value()) * 1024 * 1024,
            "poll_interval_ms": int(self.poll_ms.get_value()),
        })
        return cfg

    def _save(self) -> None:
        cfg = self._collect()
        self.error.set_text("")

        def done(out):
            if not out.ok:
                self.error.set_text(out.error)
                return
            self._loaded = cfg
            self.note.set_text("Saved. Klip applies these settings the next time it starts.")
            show(self.restart_btn, True)
            self.ui.toast("Settings saved")

        self.ui.controller.config_set(cfg, done)

    def _restart(self) -> None:
        self.restart_btn.set_sensitive(False)
        self.restart_btn.set_label("Restarting…")

        def done(out):
            self.restart_btn.set_sensitive(True)
            self.restart_btn.set_label("Restart Klip now")
            if out.ok:
                show(self.restart_btn, False)
                self.note.set_text("")
                self.ui.toast("Klip restarted")
            else:
                self.ui.toast(out.error, bad=True)

        self.ui.controller.daemon_restart(done)
