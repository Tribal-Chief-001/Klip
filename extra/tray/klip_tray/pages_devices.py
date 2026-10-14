"""Devices and pairing."""
from __future__ import annotations

from typing import Optional

import gi

gi.require_version("Gtk", "3.0")
from gi.repository import GLib, Gtk  # noqa: E402

from . import model, theme  # noqa: E402
from .pages import PAD, Page, back_header  # noqa: E402
from .widgets import (Dot, QrView, Ticker, button, card, classes, clipboard_clear_if, clipboard_set, hbox,  # noqa: E402
                      icon_button,
                      label, scrolled, show, vbox)


class DevicesPage(Page):
    name = "devices"
    title = "Devices"

    def __init__(self, ui):
        super().__init__(ui)
        self._key = None
        self._pops: list = []
        add = button("Add device", ["primary"], lambda: ui.navigate("pair"))
        self.heading = hbox(label("Devices", ["title"]), spacing=8)
        self.heading.pack_end(add, False, False, 0)
        self.list = vbox(spacing=8)
        self.empty_title = label("No devices linked", ["title"], xalign=0.5)
        self.empty_hint = label("Link your phone or another computer to share your clipboard. It takes about "
                                "ten seconds.", ["dim"], wrap=True, xalign=0.5)
        self.empty_btn = button("Add a device", ["primary"], lambda: ui.navigate("pair"))
        self.empty_btn.set_halign(Gtk.Align.CENTER)
        self.empty = card(self.empty_title, self.empty_hint, self.empty_btn, spacing=10)
        self.unpair_all_btn = button("Unpair all devices", ["flat", "danger"], self._ask_unpair_all)
        self.unpair_all_btn.set_halign(Gtk.Align.START)
        for w in (self.heading, self.list, self.empty, self.unpair_all_btn):
            self.root.pack_start(w, False, False, 0)
        self.root.show_all()
        show(self.empty, False)
        show(self.unpair_all_btn, False)

    def update(self, s: model.Summary) -> None:
        key = tuple((p.name, p.fingerprint, p.online, p.error) for p in s.peers)
        show(self.empty, not s.peers and s.state != "stopped")
        show(self.heading, bool(s.peers) or s.state == "stopped")
        show(self.unpair_all_btn, len(s.peers) >= 2)
        if key == self._key:
            return
        self._key = key
        for pop in self._pops:
            self.ui.unregister_popover(pop)
        self._pops = []
        for child in self.list.get_children():
            child.destroy()
        for p in s.peers:
            self.list.pack_start(self._row(p), False, False, 0)
        self.list.show_all()

    def _row(self, p: model.Peer) -> Gtk.Widget:
        dot = Dot(10, theme.LIME if p.online else theme.AMBER)
        name = label(p.name, ["title"], ellipsize=True)
        fp = label(p.fingerprint, ["mono", "faint"])
        state = label("Connected" if p.online else (p.error or "Not connected"),
                      ["ok" if p.online else "dim", "small"], wrap=True)
        menu = Gtk.MenuButton()
        menu.add(Gtk.Image.new_from_icon_name("view-more-symbolic", Gtk.IconSize.BUTTON))
        classes(menu, ["flat", "icon"])
        pop = Gtk.Popover()
        classes(pop, ["klip"])
        self.ui.register_popover(pop)
        self._pops.append(pop)
        inner = vbox(button("Unpair this device", ["flat", "danger"],
                            lambda p=p, pop=pop: (pop.popdown(), self._ask_unpair(p))), spacing=2)
        inner.set_margin_start(6)
        inner.set_margin_end(6)
        inner.set_margin_top(6)
        inner.set_margin_bottom(6)
        inner.show_all()
        pop.add(inner)
        menu.set_popover(pop)
        text = vbox(name, fp, state, spacing=2)
        row = hbox(dot, text, spacing=12)
        row.pack_start(Gtk.Box(), True, True, 0)
        row.pack_end(menu, False, False, 0)
        return card(row)

    def _ask_unpair(self, p: model.Peer) -> None:
        self.ui.confirm(f"Unpair {p.name}? It will stop receiving your clipboard until you link it again.",
                        "Unpair", lambda: self._unpair(p))

    def _unpair(self, p: model.Peer) -> None:
        self.ui.controller.unpair(p.fingerprint or p.name, lambda out: self.ui.toast(
            f"Unpaired {p.name}" if out.ok else out.error, bad=not out.ok))

    def _ask_unpair_all(self) -> None:
        self.ui.confirm("Unpair all devices? Nothing will sync until you link them again.", "Unpair all",
                        lambda: self.ui.controller.unpair_all(lambda out: self.ui.toast(
                            "All devices unpaired" if out.ok else out.error, bad=not out.ok)))


class PairPage(Page):
    name = "pair"
    title = "Add a device"
    parent = "devices"

    def __init__(self, ui):
        super().__init__(ui)
        self._mode = "choose"  # choose | show | join
        self._ticket = ""
        self._copied = ""
        self._seconds_left = 0
        self._join_busy = False
        self._poll = Ticker(self._poll_once)
        self._tick = Ticker(self._count_down)
        self._polling_inflight = False

        self.heading = back_header("Add a device", self._back)

        # choose
        self.show_card = card(label("Show a code", ["title"]),
                              label("Another phone or computer scans it or pastes the ticket.", ["dim"], wrap=True),
                              button("Show a code", ["primary"], self._start_show), spacing=8)
        self.join_card = card(label("Enter a code", ["title"]),
                              label("This device joins the one that shows a code (klip pair, or the app).",
                                    ["dim"], wrap=True),
                              button("Enter a code", [], self._start_join), spacing=8)
        self.choose_box = vbox(self.show_card, self.join_card, spacing=12)

        # show
        self.qr = QrView(210)
        self.qr_frame = classes(vbox(self.qr), ["qr"])
        self.qr_frame.set_halign(Gtk.Align.CENTER)
        self.ticket_label = label("", ["mono", "dim"], wrap=True, selectable=True)
        self.ticket_label.set_max_width_chars(34)
        self.countdown = label("", ["dim"], xalign=0.5)
        self.fp_label = label("", ["small", "faint"], xalign=0.5)
        self.copy_btn = button("Copy ticket", [], self._copy_ticket)
        self.cancel_btn = button("Cancel", ["flat"], self._back)
        self.show_box = vbox(self.qr_frame, self.fp_label, self.countdown,
                             scrolled(self.ticket_label, 56), hbox(self.copy_btn, self.cancel_btn, spacing=8),
                             spacing=10)

        # a request waiting for approval (from the code above, or from `klip pair` in a terminal)
        self.req_title = label("", ["title"], wrap=True)
        self.req_fp = label("", ["mono"], wrap=True)
        self.req_note = label("Check that this fingerprint is the one on the other device, then approve.",
                              ["dim"], wrap=True)
        self.approve_btn = button("Approve", ["primary"], lambda: self._decide(True))
        self.decline_btn = button("Decline", ["danger"], lambda: self._decide(False))
        self.request_card = card(self.req_title, self.req_fp, self.req_note,
                                 hbox(self.decline_btn, self.approve_btn, spacing=8), css=["accent"], spacing=8)

        # join
        self.join_text = Gtk.TextView()
        self.join_text.set_wrap_mode(Gtk.WrapMode.CHAR)
        self.join_text.set_size_request(-1, 92)
        self.join_text.set_accepts_tab(False)
        self.join_editor = classes(vbox(self.join_text), ["editor", "mono"])
        self.join_hint = label("Paste the ticket the other device shows (it starts with klip1).", ["dim"], wrap=True)
        self.join_status = label("", ["dim"], wrap=True)
        self.link_btn = button("Link device", ["primary"], self._join)
        self.paste_btn = button("Paste", [], self._paste)
        self.join_box = vbox(self.join_hint, self.join_editor, self.join_status,
                             hbox(self.paste_btn, self.link_btn, spacing=8), spacing=10)

        # A request waiting for approval comes first: it is the one thing that needs the person now.
        self.body = vbox(self.request_card, self.choose_box, self.show_box, self.join_box, spacing=12)
        self.root.pack_start(self.heading, False, False, 0)
        self.root.pack_start(scrolled(self.body), True, True, 0)
        self.root.show_all()
        self._apply_mode()

    # ------------------------------------------------------------------------------ modes

    def _apply_mode(self) -> None:
        show(self.choose_box, self._mode == "choose")
        show(self.show_box, self._mode == "show")
        show(self.join_box, self._mode == "join")
        if self._mode != "show":
            show(self.request_card, False)

    def _set_mode(self, mode: str) -> None:
        self._mode = mode
        self._apply_mode()

    def on_show(self) -> None:
        if self._mode == "choose":
            self.ensure_polling()

    def on_hide(self) -> None:
        self.leave()

    def leave(self) -> None:
        """Hide the secret: a shown code is cancelled when the page or the window closes."""
        if self._join_busy:
            return  # waiting for the other person to approve: the join goes on
        if self._mode == "show":
            self.ui.controller.pair_cancel()
        self._poll.stop()
        self._tick.stop()
        clipboard_clear_if(self._copied)
        self._copied = ""
        self._ticket = ""
        self.ticket_label.set_text("")
        self.qr.set_modules(0, "")
        self.join_text.get_buffer().set_text("")
        self._set_mode("choose")

    def _back(self) -> None:
        self.leave()
        self.ui.navigate("devices")

    # --------------------------------------------------------------------------------- show

    def _start_show(self) -> None:
        self._set_mode("show")
        self.countdown.set_text("Making a code…")
        self.ticket_label.set_text("")
        self.qr.set_modules(0, "")

        def got(out):
            if self._mode != "show":
                return
            if not out.ok:
                self.ui.toast(out.error, bad=True)
                self._set_mode("choose")
                return
            self._ticket = out.value["ticket"]
            self._seconds_left = int(out.value.get("expires_in_secs") or 0)
            self.ticket_label.set_text(self._ticket)
            self.fp_label.set_text(f"This device {out.value.get('host_fingerprint', '')}")
            self._show_countdown()
            self.ui.controller.qr(self._ticket, self._qr_arrived)
            self._poll.start(1200)
            self._tick.start(1000)

        self.ui.controller.pair_start(got)

    def _qr_arrived(self, out) -> None:
        if out.ok and self._mode == "show":
            self.qr.set_modules(int(out.value.get("size", 0)), str(out.value.get("modules", "")))

    def _copy_ticket(self) -> None:
        if self._ticket:
            clipboard_set(self._ticket)
            self._copied = self._ticket
            self.ui.toast("Ticket copied")

    def _count_down(self) -> None:
        if self._mode == "show" and self._seconds_left > 0:
            self._seconds_left -= 1
            self._show_countdown()

    def _show_countdown(self) -> None:
        if self._seconds_left > 0:
            self.countdown.set_text(f"Valid for {model.remaining(self._seconds_left)} · works once")
        else:
            self.countdown.set_text("This code has expired. Go back and make a new one.")

    # --------------------------------------------------------------------------------- polling

    def ensure_polling(self) -> None:
        """Watch for a request waiting for approval, also when `klip pair` was started elsewhere."""
        if not self._poll.running:
            self._poll.start(1500)

    def _poll_once(self) -> None:
        if self._polling_inflight:
            return
        self._polling_inflight = True

        def got(out):
            self._polling_inflight = False
            if not out.ok:
                return
            info = out.value or {}
            pending = info.get("pending")
            if info.get("paired"):
                self.ui.toast(f"Linked with {info['paired']}")
                self.leave()
                self.ui.navigate("devices")
                self.ui.controller.refresh()
                return
            if pending:
                self.req_title.set_text(f"{pending.get('device_name', 'A device')} wants to link")
                self.req_fp.set_text(str(pending.get("fingerprint", "")))
                show(self.request_card, True)
                self.ui.attention("pair")
            else:
                show(self.request_card, False)
            self._seconds_left = int(info.get("seconds_left") or 0) if self._mode == "show" else self._seconds_left
            if not info.get("window_open") and not pending and self._mode != "show":
                self._poll.stop()  # nothing to watch for any more
            if self._mode == "show":
                if not info.get("window_open") and not pending:
                    self._seconds_left = 0
                    self._show_countdown()

        self.ui.controller.pair_poll(got)

    def _decide(self, accept: bool) -> None:
        self.approve_btn.set_sensitive(False)
        self.decline_btn.set_sensitive(False)

        def done(out):
            self.approve_btn.set_sensitive(True)
            self.decline_btn.set_sensitive(True)
            show(self.request_card, False)
            if not out.ok:
                self.ui.toast(out.error, bad=True)
            elif not accept:
                self.ui.toast("Declined")

        self.ui.controller.pair_decide(accept, done)

    def update(self, s: model.Summary) -> None:
        if s.pairing_open and not self._poll.running:
            self.ensure_polling()

    # ---------------------------------------------------------------------------------- join

    def _start_join(self) -> None:
        self.join_text.get_buffer().set_text("")
        self.join_status.set_text("")
        self._set_mode("join")
        self.join_text.grab_focus()

    def _paste(self) -> None:
        display = self.join_text.get_display()
        text = Gtk.Clipboard.get_default(display).wait_for_text()
        if text:
            self.join_text.get_buffer().set_text(text.strip())

    def _join(self) -> None:
        if self._join_busy:
            return
        buf = self.join_text.get_buffer()
        text = buf.get_text(buf.get_start_iter(), buf.get_end_iter(), False)
        if not model.looks_like_ticket(text):
            self.join_status.set_text("That does not look like a Klip ticket (it starts with klip1).")
            self.join_status.get_style_context().add_class("bad")
            return
        self._join_busy = True
        self.ui.actions.set_busy(True)  # losing focus must not hide the window while we wait
        self.link_btn.set_sensitive(False)
        self.join_status.get_style_context().remove_class("bad")
        self.join_status.set_text("Waiting for the other device to approve… (up to 90 seconds)")

        def done(out):
            self._join_busy = False
            self.ui.actions.set_busy(False)
            self.link_btn.set_sensitive(True)
            if out.ok:
                name = (out.value or {}).get("peer_name", "the device")
                self.ui.toast(f"Linked with {name}")
                self.leave()
                self.ui.navigate("devices")
            else:
                self.join_status.get_style_context().add_class("bad")
                self.join_status.set_text(out.error)

        self.ui.controller.pair_join(text, done)
