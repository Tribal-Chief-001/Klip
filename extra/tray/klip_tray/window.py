"""The window: header, pages, navigation, toasts and the confirmation sheet."""
from __future__ import annotations

from typing import Callable, Optional

import gi

gi.require_version("Gtk", "3.0")
from gi.repository import Gdk, GLib, Gtk  # noqa: E402

from . import model, theme  # noqa: E402
from .pages import HomePage, Page  # noqa: E402
from .pages_devices import DevicesPage, PairPage  # noqa: E402
from .pages_history import HistoryPage, SendPage  # noqa: E402
from .pages_more import LogPage, MorePage, SettingsPage  # noqa: E402
from .widgets import Dot, LogoMark, button, classes, hbox, icon_button, label, vbox  # noqa: E402

NAV = [("home", "Home", "go-home-symbolic"), ("devices", "Devices", "computer-symbolic"),
       ("history", "History", "document-open-recent-symbolic"), ("send", "Send", "mail-send-symbolic"),
       ("more", "More", "view-more-symbolic")]
STATE_WORD = {"connected": "Connected", "waiting": "Waiting", "no_devices": "No devices", "paused": "Paused",
              "stopped": "Stopped"}
WIDTH, HEIGHT = 400, 620


class MainView(Gtk.Overlay):
    """Everything inside the window frame. It is also the context the pages talk to (`ui`)."""

    def __init__(self, controller, actions, page_factories=None):
        super().__init__()
        self.controller = controller
        self.actions = actions  # open_terminal(), quit_tray(), hide_window()
        self._toast_timer = None
        self._current = ""
        self.pages: dict = {}
        self._popovers: list = []

        self.logo = LogoMark(30, theme.FAINT, joined=False)
        self.pill_dot = Dot(8)
        self.pill_text = label("", ["small"])
        pill = classes(hbox(self.pill_dot, self.pill_text, spacing=6), ["pill"])
        header = classes(hbox(self.logo, label("klip", ["wordmark"]), spacing=10), ["header"])
        header.pack_end(icon_button("window-close-symbolic", "Close", actions.hide_window), False, False, 0)
        header.pack_end(pill, False, False, 6)

        self.stack = Gtk.Stack()
        self.stack.set_transition_type(Gtk.StackTransitionType.CROSSFADE)
        self.stack.set_transition_duration(110)
        self.stack.set_vexpand(True)

        self.nav_buttons: dict = {}
        nav = classes(hbox(spacing=2), ["nav"])
        nav.set_homogeneous(True)
        for name, text, icon in NAV:
            b = Gtk.Button()
            box = vbox(Gtk.Image.new_from_icon_name(icon, Gtk.IconSize.LARGE_TOOLBAR), label(text, xalign=0.5),
                       spacing=2)
            b.add(box)
            b.set_relief(Gtk.ReliefStyle.NONE)
            b.connect("clicked", lambda _b, n=name: self.navigate(n))
            self.nav_buttons[name] = b
            nav.pack_start(b, True, True, 0)

        frame = classes(vbox(spacing=0), ["klip-frame"])
        frame.pack_start(header, False, False, 0)
        frame.pack_start(self.stack, True, True, 0)  # the pages take all the room between header and tabs
        frame.pack_start(nav, False, False, 0)
        frame.set_margin_start(14)
        frame.set_margin_end(14)
        frame.set_margin_top(10)
        frame.set_margin_bottom(18)
        self.add(frame)

        # Toast and confirmation sheet float over the frame.
        self.toast_label = label("", xalign=0.5, ellipsize=True)
        self.toast_box = classes(vbox(self.toast_label), ["toast"])
        self.toast_rev = Gtk.Revealer()
        self.toast_rev.set_transition_type(Gtk.RevealerTransitionType.SLIDE_UP)
        self.toast_rev.add(self.toast_box)
        self.toast_rev.set_halign(Gtk.Align.CENTER)
        self.toast_rev.set_valign(Gtk.Align.END)
        self.toast_rev.set_margin_bottom(92)
        self.add_overlay(self.toast_rev)

        self.sheet_text = label("", wrap=True)
        self.sheet_yes = button("", ["primary"])
        self.sheet_no = button("Cancel", [], self.hide_confirm)
        self.sheet_box = classes(vbox(self.sheet_text, hbox(self.sheet_no, self.sheet_yes, spacing=8), spacing=12),
                                 ["sheet"])
        self.sheet_rev = Gtk.Revealer()
        self.sheet_rev.set_transition_type(Gtk.RevealerTransitionType.SLIDE_UP)
        self.sheet_rev.add(self.sheet_box)
        self.sheet_rev.set_valign(Gtk.Align.END)
        self.sheet_rev.set_margin_start(14)
        self.sheet_rev.set_margin_end(14)
        self.sheet_rev.set_margin_bottom(18)
        self.add_overlay(self.sheet_rev)
        self._yes_handler = None

        factories = page_factories if page_factories is not None else [HomePage, DevicesPage, PairPage, HistoryPage, SendPage, MorePage, LogPage, SettingsPage]
        for factory in factories:
            self.add_page(factory(self))
        controller.listeners.append(self.on_summary)
        self.show_all()
        self.toast_rev.set_reveal_child(False)
        self.sheet_rev.set_reveal_child(False)
        if "home" in self.pages:
            self.navigate("home")
        self.on_summary(controller.summary)

    # ------------------------------------------------------------------------------ pages

    def add_page(self, page: Page) -> None:
        self.pages[page.name] = page
        self.stack.add_named(page.widget(), page.name)

    def navigate(self, name: str) -> None:
        if name not in self.pages:
            return
        if name == self._current:
            self.pages[name].update(self.controller.summary)  # already there: leaving would reset it
            return
        if self._current and self._current in self.pages:
            self.pages[self._current].on_hide()
        self._current = name
        self.stack.set_visible_child_name(name)
        lit = self.pages[name].parent or name
        for n, b in self.nav_buttons.items():
            ctx = b.get_style_context()
            ctx.add_class("current") if n == lit else ctx.remove_class("current")
        self.pages[name].update(self.controller.summary)
        self.pages[name].on_show()

    @property
    def current(self) -> str:
        return self._current

    def on_summary(self, s: model.Summary) -> None:
        colour = theme.STATE_COLOURS.get(s.state, theme.FAINT)
        self.logo.set_state(colour, joined=s.state in ("connected", "waiting"))
        self.pill_dot.set_colour(colour)
        self.pill_text.set_text(STATE_WORD.get(s.state, ""))
        for page in self.pages.values():
            page.update(s)

    def register_popover(self, pop: Gtk.Popover) -> None:
        self._popovers.append(pop)

    def popover_open(self) -> bool:
        return any(p.get_visible() for p in self._popovers)

    def attention(self, name: str) -> None:
        """Something needs the person now (a pairing request): open the window on that page."""
        self.actions.show_window()
        self.navigate(name)

    def window_shown(self, shown: bool) -> None:
        """The window opened or hid: poll faster while it is open; the page on screen follows."""
        self.controller.set_fast(shown)
        page = self.pages.get(self._current)
        if page:
            page.on_show() if shown else page.on_hide()

    # ------------------------------------------------------------------------------ overlays

    def toast(self, text: str, bad: bool = False) -> None:
        self.toast_label.set_text(text)
        ctx = self.toast_box.get_style_context()
        ctx.add_class("bad") if bad else ctx.remove_class("bad")
        self.toast_rev.set_reveal_child(True)
        if self._toast_timer:
            GLib.source_remove(self._toast_timer)
        self._toast_timer = GLib.timeout_add(3800 if bad else 2400, self._hide_toast)

    def _hide_toast(self) -> bool:
        self.toast_rev.set_reveal_child(False)
        self._toast_timer = None
        return False

    def confirm(self, text: str, action: str, on_yes: Callable[[], None], danger: bool = True) -> None:
        self.sheet_text.set_text(text)
        self.sheet_yes.set_label(action)
        ctx = self.sheet_yes.get_style_context()
        for css in ("primary", "danger"):
            ctx.remove_class(css)
        ctx.add_class("danger" if danger else "primary")
        if self._yes_handler is not None:
            self.sheet_yes.disconnect(self._yes_handler)
        self._yes_handler = self.sheet_yes.connect("clicked", lambda _b: (self.hide_confirm(), on_yes()))
        self.sheet_rev.set_reveal_child(True)

    def hide_confirm(self) -> None:
        self.sheet_rev.set_reveal_child(False)

    def confirming(self) -> bool:
        return self.sheet_rev.get_reveal_child()
