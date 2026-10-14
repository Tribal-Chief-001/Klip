"""History and Send."""
from __future__ import annotations

from typing import Optional

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
from gi.repository import Gdk, GLib, Gtk  # noqa: E402

from . import model  # noqa: E402
from .pages import Page  # noqa: E402
from .widgets import (Ticker, button, card, classes, hbox, icon_button, label, pixbuf_from_png,  # noqa: E402
                      scrolled, show, vbox)

KIND_ICON = {"text": "text-x-generic-symbolic", "image": "image-x-generic-symbolic",
             "secret": "dialog-password-symbolic"}
THUMB = 44
MAX_THUMBS_PER_REFRESH = 8


class HistoryPage(Page):
    name = "history"
    title = "History"

    def __init__(self, ui):
        super().__init__(ui)
        self._all: list = []
        self._ids: list = []
        self._views: dict = {}  # id_hex -> (meta label, preview label, reveal button)
        self._thumbs: dict = {}  # id_hex -> pixbuf, or None while loading / not a picture
        self._revealed: dict = {}  # id_hex -> the text shown instead of the hidden preview
        self._loaded_once = False
        self._ticker = Ticker(self.refresh)

        self.search = Gtk.Entry()
        self.search.set_placeholder_text("Search the history")
        self.search.set_hexpand(True)
        self.search.set_icon_from_icon_name(Gtk.EntryIconPosition.PRIMARY, "system-search-symbolic")
        self.search.connect("changed", lambda _e: self._render())
        self.clear_btn = button("Clear", ["flat", "danger"], self._ask_clear)
        top = hbox(self.search, self.clear_btn, spacing=6)
        top.set_child_packing(self.search, True, True, 0, Gtk.PackType.START)

        self.rows = vbox(spacing=6)
        self.empty = label("Nothing yet.\nClips you send or receive show up here, for a day.", ["dim"], wrap=True,
                           xalign=0.5)
        self.empty.set_margin_top(40)
        self.note = label("Kept in memory only: restarting Klip clears it.", ["faint", "small"], xalign=0.5)
        self.root.pack_start(top, False, False, 0)
        body = vbox(self.rows, self.empty, self.note, spacing=10)
        self.root.pack_start(scrolled(body), True, True, 0)
        self.root.show_all()
        show(self.empty, False)

    # ----------------------------------------------------------------------------- loading

    def on_show(self) -> None:
        self.refresh()
        self._ticker.start(3000)

    def on_hide(self) -> None:
        self._ticker.stop()
        self._forget_secrets()

    def _forget_secrets(self) -> None:
        """A revealed secret goes back to hidden whenever the page or the window is left."""
        revealed, self._revealed = self._revealed, {}
        for row in self._all:
            view = self._views.get(row.id_hex)
            if view and row.id_hex in revealed:
                view[1].set_text(self._preview_text(row))

    def refresh(self) -> None:
        self.ui.controller.history(self._arrived)

    def _arrived(self, out) -> None:
        if not out.ok:
            if not self._loaded_once:
                self.empty.set_text(out.error if not out.down else "Klip is not running.")
                show(self.empty, True)
            return
        self._loaded_once = True
        self._all = out.value
        self._render()

    def update(self, s: model.Summary) -> None:
        show(self.clear_btn, bool(self._all))

    # ----------------------------------------------------------------------------- drawing

    def _render(self) -> None:
        rows = model.filter_rows(self._all, self.search.get_text())
        ids = [r.id_hex for r in rows]
        show(self.empty, not rows)
        if not rows:
            self.empty.set_text("Nothing matches." if self._all else
                                "Nothing yet.\nClips you send or receive show up here, for a day.")
        show(self.clear_btn, bool(self._all))
        if ids != self._ids:
            self._ids = ids
            for child in self.rows.get_children():
                self.rows.remove(child)
            self._views = {}
            for r in rows:
                self.rows.pack_start(self._row(r), False, False, 0)
            self.rows.show_all()
            self._fetch_thumbs(rows)
        else:
            for r in rows:  # the same clips: only the ages change
                v = self._views.get(r.id_hex)
                if v:
                    v[0].set_text(self._meta(r))

    @staticmethod
    def _meta(r: model.HistoryRow) -> str:
        return " · ".join(x for x in (r.origin, r.age, r.size_text) if x)

    def _row(self, r: model.HistoryRow) -> Gtk.Widget:
        icon = Gtk.Image.new_from_icon_name(KIND_ICON.get(r.kind, "text-x-generic-symbolic"), Gtk.IconSize.LARGE_TOOLBAR)
        icon.set_size_request(THUMB, THUMB)
        thumb_box = classes(vbox(icon), ["thumb"])
        thumb_box.set_valign(Gtk.Align.START)
        preview = label(self._preview_text(r), wrap=True, ellipsize=True, lines=2)
        meta = label(self._meta(r), ["dim", "small"], ellipsize=True)
        text = vbox(preview, meta, spacing=2)
        left = hbox(thumb_box, spacing=10)
        left.pack_start(text, True, True, 0)  # fill the slot, or the text sits in the middle of it
        ev = Gtk.EventBox()
        ev.add(left)
        ev.set_above_child(False)
        ev.connect("button-release-event", lambda _w, e, r=r: self._copy(r) if e.button == 1 else None)
        ev.connect("realize", lambda w: w.get_window().set_cursor(
            Gdk.Cursor.new_from_name(w.get_display(), "pointer")) if r.can_copy else None)
        ev.set_tooltip_text("Click to copy it back to this computer" if r.can_copy
                            else "Too large to keep in the history")

        buttons = hbox(spacing=2)
        reveal = None
        if r.kind == "secret":
            reveal = icon_button("view-reveal-symbolic", "Show or hide the secret", lambda r=r: self._toggle_reveal(r))
            buttons.pack_start(reveal, False, False, 0)
        if r.kind != "image":
            send = icon_button("mail-send-symbolic", "Send this text again", lambda r=r: self._send_again(r))
            send.set_sensitive(r.can_copy)
            buttons.pack_start(send, False, False, 0)
        cp = icon_button("edit-copy-symbolic", "Copy back to this computer", lambda r=r: self._copy(r))
        cp.set_sensitive(r.can_copy)
        buttons.pack_start(cp, False, False, 0)
        buttons.set_valign(Gtk.Align.CENTER)

        row = hbox(spacing=6)
        row.pack_start(ev, True, True, 0)
        row.pack_end(buttons, False, False, 0)
        self._views[r.id_hex] = (meta, preview, reveal, icon, thumb_box)
        return card(row)

    def _preview_text(self, r: model.HistoryRow) -> str:
        return self._revealed.get(r.id_hex) or r.preview or "(empty)"

    # ----------------------------------------------------------------------------- pictures

    def _fetch_thumbs(self, rows: list) -> None:
        wanted = [r for r in rows if r.kind == "image" and r.can_copy][:MAX_THUMBS_PER_REFRESH]
        for r in wanted:
            if r.id_hex in self._thumbs:
                self._show_thumb(r.id_hex)
                continue
            self._thumbs[r.id_hex] = None
            self.ui.controller.thumbnail(r.id_hex, lambda out, i=r.id_hex: self._thumb_arrived(i, out))

    def _thumb_arrived(self, id_hex: str, out) -> None:
        if out.ok:
            pix = pixbuf_from_png(out.value, THUMB)
            if pix is not None:
                self._thumbs[id_hex] = pix
                self._show_thumb(id_hex)

    def _show_thumb(self, id_hex: str) -> None:
        view = self._views.get(id_hex)
        pix = self._thumbs.get(id_hex)
        if view and pix is not None:
            view[3].set_from_pixbuf(pix)

    # ------------------------------------------------------------------------------ actions

    def _copy(self, r: model.HistoryRow) -> None:
        if not r.can_copy:
            self.ui.toast("That clip was too large to keep. Copy it again where it came from.", bad=True)
            return
        self.ui.controller.copy_back(r.id_hex, lambda out: self.ui.toast(
            "Copied. Paste it on this computer." if out.ok else out.error, bad=not out.ok))

    def _send_again(self, r: model.HistoryRow) -> None:
        def got(out):
            if not out.ok:
                self.ui.toast(out.error, bad=True)
                return
            self.ui.controller.send_text(out.value, lambda o: self.ui.toast(o.value if o.ok else o.error,
                                                                            bad=not o.ok))

        self.ui.controller.history_text(r.id_hex, got)

    def _toggle_reveal(self, r: model.HistoryRow) -> None:
        view = self._views.get(r.id_hex)
        if not view:
            return
        if r.id_hex in self._revealed:
            del self._revealed[r.id_hex]
            view[1].set_text(self._preview_text(r))
            return

        def got(out):
            if not out.ok:
                self.ui.toast(out.error, bad=True)
                return
            self._revealed[r.id_hex] = out.value[:400]
            view[1].set_text(self._revealed[r.id_hex])

        self.ui.controller.history_text(r.id_hex, got)

    def _ask_clear(self) -> None:
        self.ui.confirm("Forget every clip kept in this computer's history? It cannot be undone.", "Clear history",
                        self._clear)

    def _clear(self) -> None:
        def done(out):
            self._revealed.clear()
            self.ui.toast("History cleared" if out.ok else out.error, bad=not out.ok)
            self.refresh()

        self.ui.controller.clear_history(done)


class SendPage(Page):
    name = "send"
    title = "Send"

    def __init__(self, ui):
        super().__init__(ui)
        self._busy = False
        self.text = Gtk.TextView()
        self.text.set_wrap_mode(Gtk.WrapMode.WORD_CHAR)
        self.text.set_accepts_tab(False)
        self.text.set_left_margin(6)
        self.text.set_right_margin(6)
        self.text.set_top_margin(4)
        self.text.set_bottom_margin(4)
        self.editor = classes(vbox(scrolled(self.text, 190)), ["editor"])
        self.placeholder = label("Type or paste text to send…", ["faint"])
        self.counter = label("", ["dim", "small"])
        self.result = label("", ["dim"], wrap=True)
        self.load_btn = button("Load a file…", [], self._load_file)
        self.send_btn = button("Send", ["primary"], self._send)
        controls = hbox(self.counter, spacing=8)
        controls.pack_end(self.send_btn, False, False, 0)
        controls.pack_end(self.load_btn, False, False, 0)
        hint = label("Sends to every linked device now. Plain text only; use Home or the tray for images.",
                     ["faint", "small"], wrap=True)
        for w in (label("Send text", ["title"]), self.editor, controls, self.result, hint):
            self.root.pack_start(w, False, False, 0)
        self.text.get_buffer().connect("changed", lambda _b: self._changed())
        self.text.connect("key-press-event", self._key)
        self.root.show_all()
        self._changed()

    def _limit(self) -> int:
        hello = self.ui.controller.hello
        configured = ((hello or {}).get("config") or {}).get("max_clip_bytes") if isinstance(hello, dict) else None
        return min(configured, 8 * 1024 * 1024 - 65_536) if isinstance(configured, int) and configured > 0 \
            else 8 * 1024 * 1024 - 65_536

    def _contents(self) -> str:
        buf = self.text.get_buffer()
        return buf.get_text(buf.get_start_iter(), buf.get_end_iter(), False)

    def _changed(self) -> None:
        text = self._contents()
        n = len(text.encode("utf-8"))
        over = n > self._limit()
        self.counter.set_text(f"{model.size(n)} of {model.size(self._limit())}" if n else f"Limit {model.size(self._limit())}")
        ctx = self.counter.get_style_context()
        ctx.add_class("bad") if over else ctx.remove_class("bad")
        self.send_btn.set_sensitive(bool(text.strip()) and not over and not self._busy)

    def _key(self, _w, event) -> bool:
        if event.keyval in (Gdk.KEY_Return, Gdk.KEY_KP_Enter) and event.state & Gdk.ModifierType.CONTROL_MASK:
            self._send()
            return True
        return False

    def _send(self) -> None:
        if self._busy:
            return
        text = self._contents()
        problem = model.send_problem(text, self._limit())
        if problem:
            self.result.set_text(problem)
            return
        self._busy = True
        self.send_btn.set_sensitive(False)
        self.result.set_text("Sending…")

        def done(out):
            self._busy = False
            bad = not out.ok
            self.result.set_text(out.error if bad else out.value)
            ctx = self.result.get_style_context()
            ctx.add_class("bad") if bad else ctx.remove_class("bad")
            if not bad:
                self.text.get_buffer().set_text("")
            self._changed()
            self.ui.toast(out.error if bad else out.value, bad=bad)

        self.ui.controller.send_text(text, done)

    def _load_file(self) -> None:
        self.ui.actions.set_busy(True)
        dialog = Gtk.FileChooserNative.new("Load a text file", self.root.get_toplevel(), Gtk.FileChooserAction.OPEN,
                                           "Load", "Cancel")
        response = dialog.run()
        path = dialog.get_filename() if response == Gtk.ResponseType.ACCEPT else None
        dialog.destroy()
        self.ui.actions.set_busy(False)
        if not path:
            return
        try:
            with open(path, "rb") as f:
                raw = f.read(self._limit() + 1)
            if len(raw) > self._limit():
                raise ValueError(f"that file is bigger than the limit ({model.size(self._limit())})")
            content = raw.decode("utf-8")
        except UnicodeDecodeError:
            self.ui.toast("That file is not text (not valid UTF-8).", bad=True)
            return
        except (OSError, ValueError) as e:
            self.ui.toast(str(e), bad=True)
            return
        self.text.get_buffer().set_text(content)
