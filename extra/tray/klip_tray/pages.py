"""The pages of the window. Each page builds its widgets once and is updated from the Summary."""
from __future__ import annotations

from typing import Optional

import gi

gi.require_version("Gtk", "3.0")
from gi.repository import GLib, Gtk  # noqa: E402

from . import model, theme  # noqa: E402
from .widgets import (Dot, QrView, button, card, classes, clipboard_set, hbox, icon_button, label,  # noqa: E402
                      pixbuf_from_png, scrolled, show, vbox)

PAD = 16


class Page:
    name = ""
    title = ""
    parent = ""  # the tab that stays lit while this sub page is open

    def __init__(self, ui):
        self.ui = ui
        self.root: Gtk.Widget = vbox(spacing=12)
        self.root.set_margin_start(PAD)
        self.root.set_margin_end(PAD)
        self.root.set_margin_top(6)
        self.root.set_margin_bottom(PAD)

    def widget(self) -> Gtk.Widget:
        return self.root

    def update(self, summary: model.Summary) -> None:
        pass

    def on_show(self) -> None:
        pass

    def on_hide(self) -> None:
        pass


def back_header(title: str, on_back) -> Gtk.Widget:
    return hbox(icon_button("go-previous-symbolic", "Back", on_back), label(title, ["title"]), spacing=6)


def switch_row(title: str, hint: str, on_change) -> tuple:
    """(row, switch): a title with a dim line under it and a switch on the right."""
    sw = Gtk.Switch()
    sw.set_valign(Gtk.Align.CENTER)
    text = vbox(label(title), label(hint, ["dim", "small"], wrap=True), spacing=2)
    row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=12)
    row.pack_start(text, True, True, 0)
    row.pack_end(sw, False, False, 0)
    sw.connect("state-set", lambda _s, state: on_change(state) or False)
    return row, sw


class HomePage(Page):
    name = "home"
    title = "Home"

    def __init__(self, ui):
        super().__init__(ui)
        c = ui.controller
        self._thumb_key = None
        self._busy = False
        self._updating = False

        # Status.
        self.dot = Dot(14)
        self.status_title = label("", ["title"], ellipsize=True)
        self.status_sub = label("", ["dim"], wrap=True)
        self.start_btn = button("Start Klip", ["primary"], self._start)
        self.status = card(hbox(self.dot, vbox(self.status_title, self.status_sub, spacing=2), spacing=12), spacing=10)
        self.status.pack_start(self.start_btn, False, False, 0)

        # An older daemon that does not know images.
        self.old_label = label("This Klip is older than the app. Restart it to use image sharing.", wrap=True)
        self.old_btn = button("Restart Klip", [], self._restart)
        self.old_box = card(self.old_label, self.old_btn, css=["warnbox"])

        # The image waiting to be sent.
        self.thumb = Gtk.Image()
        self.thumb.set_size_request(92, 68)
        thumb_frame = classes(Gtk.Box(), ["thumb"])
        thumb_frame.pack_start(self.thumb, True, True, 0)
        self.img_title = label("Image ready", ["title"])
        self.img_meta = label("", ["dim", "small"], wrap=True)
        self.send_btn = button("Send", ["primary"], self._send_image)
        self.image_row = hbox(thumb_frame, vbox(self.img_title, self.img_meta, self.send_btn, spacing=4), spacing=12)
        self.image_hint = label("", ["dim"], wrap=True)
        auto_row, self.auto_switch = switch_row("Send images automatically",
                                                "Off: each copied image waits here until you send it.",
                                                self._auto_changed)
        self.image_card = card(self.image_row, self.image_hint, auto_row, spacing=12)
        self.image_card.set_spacing(12)

        # Last clip.
        self.last_title = label("LAST CLIP", ["section"])
        self.last_clip = label("", ["dim"], ellipsize=True)
        self.last_box = vbox(self.last_title, self.last_clip, spacing=4)

        # Quick actions.
        self.pause_btn = Gtk.MenuButton()
        self.pause_btn.set_label("Pause")
        classes(self.pause_btn, [])
        self.pop = Gtk.Popover()
        classes(self.pop, ["klip"])
        self.pop_box = vbox(spacing=2)
        self.pop_box.set_margin_start(6)
        self.pop_box.set_margin_end(6)
        self.pop_box.set_margin_top(6)
        self.pop_box.set_margin_bottom(6)
        self.pop.add(self.pop_box)
        self.pause_btn.set_popover(self.pop)
        self._build_pause_menu(paused=False)
        self.add_btn = button("Add device", [], lambda: ui.navigate("pair"))
        actions = hbox(self.pause_btn, self.add_btn, spacing=8)

        self.footer = label("", ["faint", "small"], wrap=True)
        for w in (self.status, self.old_box, self.image_card, self.last_box, actions, self.footer):
            self.root.pack_start(w, False, False, 0)
        self.root.show_all()
        self._update_visibility(model.summarize(None, None, down=True))

    # ------------------------------------------------------------------------------- actions

    def _start(self) -> None:
        self.start_btn.set_sensitive(False)
        self.start_btn.set_label("Starting…")

        def done(out):
            self.start_btn.set_sensitive(True)
            self.start_btn.set_label("Start Klip")
            if not out.ok:
                self.ui.toast(out.error, bad=True)

        self.ui.controller.daemon_start(done)

    def _restart(self) -> None:
        self.old_btn.set_sensitive(False)
        self.old_btn.set_label("Restarting…")

        def done(out):
            self.old_btn.set_sensitive(True)
            self.old_btn.set_label("Restart Klip")
            self.ui.toast("Klip restarted" if out.ok else out.error, bad=not out.ok)

        self.ui.controller.daemon_restart(done)

    def _send_image(self) -> None:
        if self._busy:
            return
        self._busy = True
        self.send_btn.set_sensitive(False)

        def done(out):
            self._busy = False
            self.ui.toast(out.value if out.ok else out.error, bad=not out.ok)
            self.update(self.ui.controller.summary)

        self.ui.controller.send_pending_image(done)

    def _auto_changed(self, state: bool) -> None:
        if self._updating:
            return

        def done(out):
            if not out.ok:
                self.ui.toast(out.error, bad=True)
            else:
                self.ui.toast("Images are sent automatically" if state else "Images wait until you send them")

        self.ui.controller.set_auto_images(state, done)

    def _build_pause_menu(self, paused: bool) -> None:
        for child in self.pop_box.get_children():
            self.pop_box.remove(child)
        if paused:
            self.pop_box.pack_start(button("Resume now", ["flat"], self._resume), False, False, 0)
        else:
            for text, secs in model.PAUSE_CHOICES:
                self.pop_box.pack_start(button(text, ["flat"], lambda s=secs: self._pause(s)), False, False, 0)
        self.pop_box.show_all()
        self.pause_btn.set_label("Resume" if paused else "Pause")
        self._menu_paused = paused

    def _pause(self, secs: int) -> None:
        self.pop.popdown()
        self.ui.controller.pause(secs, lambda out: None if out.ok else self.ui.toast(out.error, bad=True))

    def _resume(self) -> None:
        self.pop.popdown()
        self.ui.controller.resume(lambda out: None if out.ok else self.ui.toast(out.error, bad=True))

    # --------------------------------------------------------------------------------- update

    def _update_visibility(self, s: model.Summary) -> None:
        stopped = s.state == "stopped"
        show(self.start_btn, stopped)
        show(self.old_box, s.daemon_old and not stopped)
        show(self.image_card, not stopped)
        show(self.last_box, bool(s.last_clip) and not stopped)
        show(self.pause_btn, not stopped)
        show(self.add_btn, not stopped)
        show(self.footer, not stopped)

    def update(self, s: model.Summary) -> None:
        self._update_visibility(s)
        self.dot.set_colour(theme.STATE_COLOURS.get(s.state, theme.FAINT))
        self.status_title.set_text(s.title)
        self.status_sub.set_text(s.subtitle)
        show(self.status_sub, bool(s.subtitle))
        if self._menu_paused != (s.state == "paused"):
            self._build_pause_menu(s.state == "paused")

        self._updating = True
        if self.auto_switch.get_active() != s.auto_images:
            self.auto_switch.set_active(s.auto_images)
            self.auto_switch.set_state(s.auto_images)
        self._updating = False

        has_image = s.image is not None
        show(self.image_row, has_image)
        show(self.image_hint, not has_image)
        self.image_hint.set_text("Images are sent as soon as you copy them." if s.auto_images
                                 else "Copy an image: it waits here until you send it.")
        if has_image:
            online = [p.name for p in s.peers if p.online]
            self.img_meta.set_text(s.image.text)
            target = online[0] if len(online) == 1 else (f"{len(online)} devices" if online else "")
            self.send_btn.set_label(f"Send to {target}" if target else "No device connected")
            self.send_btn.set_sensitive(bool(online) and s.state != "paused" and not self._busy)
            key = (s.image.size_bytes, s.image.width, s.image.height, s.image.kind)
            if key != self._thumb_key:
                self._thumb_key = key
                self.thumb.clear()
                self.ui.controller.thumbnail(None, self._thumb_arrived)
        else:
            self._thumb_key = None
        if s.last_clip:
            self.last_clip.set_text(s.last_clip.replace("\n", " "))
        self.footer.set_text(" · ".join(x for x in (f"This device {s.device_fingerprint}" if s.device_fingerprint else "",
                                                    s.platform, s.memory, s.relay_state) if x))

    def _thumb_arrived(self, out) -> None:
        if not out.ok:
            return
        pix = pixbuf_from_png(out.value, 92)
        if pix is not None:
            self.thumb.set_from_pixbuf(pix)
