"""Small GTK 3 building blocks. All look comes from theme.CSS; nothing here knows the daemon."""
from __future__ import annotations

import math
from typing import Callable, Iterable, Optional

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
gi.require_version("GdkPixbuf", "2.0")
gi.require_version("Pango", "1.0")
from gi.repository import Gdk, GdkPixbuf, GLib, Gtk, Pango  # noqa: E402

from . import theme  # noqa: E402


def show(widget: Gtk.Widget, flag: bool) -> None:
    """Show or hide a widget for good: `show_all()` on a parent must not bring it back."""
    widget.set_no_show_all(True)
    widget.set_visible(bool(flag))


def classes(widget: Gtk.Widget, css: Iterable[str]) -> Gtk.Widget:
    ctx = widget.get_style_context()
    for name in css:
        ctx.add_class(name)
    return widget


def hbox(*children, spacing: int = 8, css: Iterable[str] = ()) -> Gtk.Box:
    box = classes(Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=spacing), css)
    for child in children:
        box.pack_start(child, False, False, 0)
    return box


def vbox(*children, spacing: int = 8, css: Iterable[str] = ()) -> Gtk.Box:
    box = classes(Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=spacing), css)
    for child in children:
        box.pack_start(child, False, False, 0)
    return box


def label(text: str = "", css: Iterable[str] = (), wrap: bool = False, ellipsize: bool = False,
          selectable: bool = False, xalign: float = 0.0, max_chars: int = 0, lines: int = 0) -> Gtk.Label:
    lab = Gtk.Label(label=text)
    lab.set_xalign(xalign)
    lab.set_line_wrap(wrap)
    if wrap:
        lab.set_line_wrap_mode(Pango.WrapMode.WORD_CHAR)
    if ellipsize:
        lab.set_ellipsize(Pango.EllipsizeMode.END)
    if wrap and not max_chars:
        max_chars = 40  # a wrapped label must not ask for the width of its whole text
    if max_chars:
        lab.set_max_width_chars(max_chars)
    if lines:
        lab.set_lines(lines)
    lab.set_selectable(selectable)
    return classes(lab, css)


def button(text: str = "", css: Iterable[str] = (), on_click: Optional[Callable[[], None]] = None,
           icon: Optional[str] = None, tooltip: Optional[str] = None) -> Gtk.Button:
    btn = Gtk.Button()
    if icon and text:
        btn.set_image(Gtk.Image.new_from_icon_name(icon, Gtk.IconSize.BUTTON))
        btn.set_label(text)
        btn.set_always_show_image(True)
    elif icon:
        btn.set_image(Gtk.Image.new_from_icon_name(icon, Gtk.IconSize.BUTTON))
    else:
        btn.set_label(text)
    btn.set_relief(Gtk.ReliefStyle.NONE) if "flat" in css else None
    classes(btn, css)
    if tooltip:
        btn.set_tooltip_text(tooltip)
    if on_click:
        btn.connect("clicked", lambda _b: on_click())
    btn.set_can_focus(True)
    return btn


def icon_button(icon: str, tooltip: str, on_click: Callable[[], None], css: Iterable[str] = ()) -> Gtk.Button:
    return button("", ["flat", "icon", *css], on_click, icon=icon, tooltip=tooltip)


def card(*children, css: Iterable[str] = (), spacing: int = 8) -> Gtk.Box:
    return vbox(*children, spacing=spacing, css=["card", *css])


def scrolled(child: Gtk.Widget, min_height: int = 0) -> Gtk.ScrolledWindow:
    sw = Gtk.ScrolledWindow()
    sw.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
    sw.set_shadow_type(Gtk.ShadowType.NONE)
    sw.add(child)
    if min_height:
        sw.set_min_content_height(min_height)
    return sw


def rgba(hex_colour: str, alpha: float = 1.0) -> Gdk.RGBA:
    c = Gdk.RGBA()
    c.parse(hex_colour)
    c.alpha = alpha
    return c


class Dot(Gtk.DrawingArea):
    """A round status dot."""

    def __init__(self, diameter: int = 12, colour: str = theme.FAINT):
        super().__init__()
        self._d = diameter
        self._colour = colour
        self.set_size_request(diameter + 2, diameter + 2)
        self.set_valign(Gtk.Align.CENTER)
        self.connect("draw", self._draw)

    def set_colour(self, colour: str) -> None:
        if colour != self._colour:
            self._colour = colour
            self.queue_draw()

    def _draw(self, _w, cr) -> bool:
        c = rgba(self._colour)
        w, h = self.get_allocated_width(), self.get_allocated_height()
        cr.set_source_rgba(c.red, c.green, c.blue, 0.22)
        cr.arc(w / 2, h / 2, w / 2, 0, 2 * math.pi)
        cr.fill()
        cr.set_source_rgba(c.red, c.green, c.blue, 1)
        cr.arc(w / 2, h / 2, self._d / 2 - 1, 0, 2 * math.pi)
        cr.fill()
        return False


class LogoMark(Gtk.DrawingArea):
    """The Klip mark: two devices joined by a line, in the colour of the state."""

    def __init__(self, width: int = 30, colour: str = theme.LIME, joined: bool = True):
        super().__init__()
        self._colour = colour
        self._joined = joined
        self.set_size_request(width, 16)
        self.set_valign(Gtk.Align.CENTER)
        self.connect("draw", self._draw)

    def set_state(self, colour: str, joined: bool) -> None:
        self._colour, self._joined = colour, joined
        self.queue_draw()

    def _draw(self, _w, cr) -> bool:
        c = rgba(self._colour)
        w, h = self.get_allocated_width(), self.get_allocated_height()
        r = 3.6
        cr.set_source_rgba(c.red, c.green, c.blue, 1)
        if self._joined:
            cr.set_line_width(2.4)
            cr.set_line_cap(1)
            cr.move_to(r + 1, h / 2)
            cr.line_to(w - r - 1, h / 2)
            cr.stroke()
        for x in (r + 1, w - r - 1):
            cr.arc(x, h / 2, r, 0, 2 * math.pi)
            cr.fill()
        return False


class QrView(Gtk.DrawingArea):
    """A QR code from the bridge's `modules` string (row by row, '1' = dark)."""

    def __init__(self, pixels: int = 200):
        super().__init__()
        self._size = 0
        self._modules = ""
        self.set_size_request(pixels, pixels)
        self.set_halign(Gtk.Align.CENTER)
        self.connect("draw", self._draw)

    def set_modules(self, size: int, modules: str) -> None:
        ok = size > 0 and len(modules) == size * size
        self._size, self._modules = (size, modules) if ok else (0, "")
        self.queue_draw()

    def _draw(self, _w, cr) -> bool:
        w, h = self.get_allocated_width(), self.get_allocated_height()
        cr.set_source_rgb(1, 1, 1)
        cr.rectangle(0, 0, w, h)
        cr.fill()
        if not self._size:
            return False
        quiet = 2  # modules of white around the code
        total = self._size + 2 * quiet
        cell = min(w, h) / total
        ox = (w - cell * total) / 2
        oy = (h - cell * total) / 2
        cr.set_source_rgb(0, 0, 0)
        for i, ch in enumerate(self._modules):
            if ch == "1":
                x, y = i % self._size, i // self._size
                cr.rectangle(ox + (x + quiet) * cell, oy + (y + quiet) * cell, cell + 0.5, cell + 0.5)
        cr.fill()
        return False


def pixbuf_from_png(data: bytes, max_side: int) -> Optional[GdkPixbuf.Pixbuf]:
    """A PNG's bytes as a picture no bigger than `max_side`; None when they are not a picture."""
    loader = GdkPixbuf.PixbufLoader()
    try:
        loader.write(data)
        loader.close()
    except GLib.Error:
        return None
    pix = loader.get_pixbuf()
    if pix is None:
        return None
    w, h = pix.get_width(), pix.get_height()
    if max(w, h) > max_side:
        scale = max_side / max(w, h)
        pix = pix.scale_simple(max(1, int(w * scale)), max(1, int(h * scale)), GdkPixbuf.InterpType.BILINEAR)
    return pix


def clipboard_set(text: str) -> None:
    display = Gdk.Display.get_default()
    if display is not None:
        Gtk.Clipboard.get_default(display).set_text(text, -1)


def clipboard_clear_if(text: str) -> None:
    """Take a secret back off the clipboard, but only if it is still what we put there."""
    display = Gdk.Display.get_default()
    if display is not None and text:
        cb = Gtk.Clipboard.get_default(display)
        if cb.wait_for_text() == text:
            cb.clear()


class Ticker:
    """A repeating timer that can be started and stopped without ever running twice."""

    def __init__(self, fn: Callable[[], None]):
        self._fn = fn
        self._id = 0

    @property
    def running(self) -> bool:
        return self._id != 0

    def start(self, ms: int) -> None:
        self.stop()
        self._id = GLib.timeout_add(ms, self._tick)

    def stop(self) -> None:
        if self._id:
            GLib.source_remove(self._id)
            self._id = 0

    def _tick(self) -> bool:
        try:
            self._fn()
        except Exception:  # a failing tick must not end the timer silently
            import logging
            logging.getLogger("klip_tray").exception("a timer callback failed")
        return True
