"""Render the window with fake data to PNG files, to look at it. Not shipped; needs a display.

    DISPLAY=:99 python3 extra/tray/tools/gallery.py OUT_DIR
"""
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "tests"))

import gi

gi.require_version("Gtk", "3.0")
from gi.repository import Gdk, GdkPixbuf, GLib, Gtk  # noqa: E402

from klip_tray import theme  # noqa: E402
from klip_tray.bridge import Reply  # noqa: E402
from klip_tray.controller import Controller  # noqa: E402
from klip_tray.window import MainView, HEIGHT, WIDTH  # noqa: E402
from test_controller import FakeBridge, status, HELLO, NOIMG  # noqa: E402


def peer(name, online, fp="AAAA-BBBB-CCCC", error=None):
    if error is None and fp and not fp[0].isupper():
        fp, error = "AAAA-BBBB-CCCC", fp
    return {"name": name, "is_online": online, "fingerprint": fp, "last_error": error}


class Actions:
    def hide_window(self): pass
    def open_terminal(self): pass
    def quit_tray(self): pass
    def show_window(self): pass


def settle():
    for _ in range(60):
        while Gtk.events_pending():
            Gtk.main_iteration_do(False)


def sample_png(w=160, h=90):
    import io, struct, zlib
    rows = b"".join(b"\x00" + b"".join(bytes((x * 255 // w, y * 255 // h, 140)) for x in range(w)) for y in range(h))
    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


def make_view(script=None, pages=None):
    provider = Gtk.CssProvider()
    provider.load_from_data(theme.CSS.encode())
    Gtk.StyleContext.add_provider_for_screen(Gdk.Screen.get_default(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
    bridge = FakeBridge()
    import base64
    bridge.script["GetThumbnail"] = Reply(ok={"Thumbnail": base64.b64encode(sample_png()).decode()})
    bridge.script.update(script or {})
    ctl = Controller(bridge, lambda ms, fn: 1, lambda h: None, now_ms=lambda: 5_000_000)
    ctl.refresh()
    kwargs = {"page_factories": pages} if pages else {}
    view = MainView(ctl, Actions(), **kwargs)
    return view, ctl, bridge


def snapshot(view, path):
    win = Gtk.OffscreenWindow()
    win.set_size_request(WIDTH + 28, HEIGHT + 28)
    win.set_default_size(WIDTH + 28, HEIGHT + 28)
    win.get_style_context().add_class("klip")
    win.add(view)
    win.show_all()
    settle()
    pix = win.get_pixbuf()
    # composite on a mid grey so the rounded corners and the shadow can be judged
    w, h = pix.get_width(), pix.get_height()
    bg = GdkPixbuf.Pixbuf.new(GdkPixbuf.Colorspace.RGB, False, 8, w, h)
    bg.fill(0x5a6068ff)
    pix.composite(bg, 0, 0, w, h, 0, 0, 1, 1, GdkPixbuf.InterpType.BILINEAR, 255)
    bg.savev(path, "png", [], [])
    win.remove(view)
    win.destroy()


def fake_qr(n=29):
    import random
    rnd = random.Random(4)
    cells = [["1" if rnd.random() < 0.5 else "0" for _ in range(n)] for _ in range(n)]
    for ox, oy in ((0, 0), (n - 7, 0), (0, n - 7)):  # finder-like corners
        for y in range(7):
            for x in range(7):
                edge = x in (0, 6) or y in (0, 6)
                core = 2 <= x <= 4 and 2 <= y <= 4
                cells[oy + y][ox + x] = "1" if edge or core else "0"
    return n, "".join("".join(r) for r in cells)


def run(out, name, script=None, page=None, setup=None, pages=None):
    view, ctl, bridge = make_view(script, pages)
    if page:
        view.navigate(page)
    if setup:
        setup(view, ctl, bridge)
    snapshot(view, os.path.join(out, name + ".png"))
    print("wrote", name)


if __name__ == "__main__":
    out = sys.argv[1]
    os.makedirs(out, exist_ok=True)
    img = {"auto": False, "waiting": {"size_bytes": 186183, "width": 1920, "height": 1080, "age_secs": 130, "kind": "png"}}
    two = {"GetStatus": Reply(ok={"Status": status([peer("Vivo V2253", True, "ABCD-EFGH-JKLM"),
                                                    peer("Pixel 8", False, "NPQR-STUV-WXYZ", "cannot reach peer: timed out")])})}
    n, modules = fake_qr()
    run(out, "home_connected_image", {"GetImageState": Reply(ok={"ImageState": img})})
    run(out, "home_waiting", {"GetStatus": Reply(ok={"Status": status([peer("Vivo V2253", False, error="cannot reach peer: timed out")])})})
    run(out, "home_stopped", {"GetStatus": Reply(err="Klip daemon is not running.", down=True)})
    run(out, "devices_two", two, page="devices")
    run(out, "devices_empty", {"GetStatus": Reply(ok={"Status": status([])})}, page="devices")
    run(out, "pair_choose", two, page="pair")

    def show_code(view, ctl, bridge):
        bridge.script["PairStart"] = Reply(ok={"PairTicket": {"ticket": "klip1" + "abcdefghij" * 14, "expires_in_secs": 102,
                                                              "host_fingerprint": "J73D-FACE-MFHA"}})
        bridge.script["qr"] = Reply(ok={"size": n, "modules": modules})
        bridge.script["PairPoll"] = Reply(ok={"PairPoll": {"window_open": True, "seconds_left": 100, "pending": None, "paired": None}})
        view.pages["pair"]._start_show()
        settle()
    run(out, "pair_show", two, page="pair", setup=show_code)

    def request(view, ctl, bridge):
        show_code(view, ctl, bridge)
        bridge.script["PairPoll"] = Reply(ok={"PairPoll": {"window_open": True, "seconds_left": 90,
                                                           "pending": {"device_name": "Pixel 8", "fingerprint": "NPQR-STUV-WXYZ"}, "paired": None}})
        view.pages["pair"]._poll_once()
        settle()
    run(out, "pair_request", two, page="pair", setup=request)

    def join(view, ctl, bridge):
        view.pages["pair"]._start_join()
        view.pages["pair"].join_text.get_buffer().set_text("klip1" + "abcdefghij" * 8)
        settle()
    run(out, "pair_join", two, page="pair", setup=join)
