import os
import tempfile
import unittest

from klip_tray import icons, theme

try:
    import gi
    gi.require_version("Gtk", "3.0")
    gi.require_version("GdkPixbuf", "2.0")
    from gi.repository import GdkPixbuf, Gtk
    HAVE_GTK = True
except (ImportError, ValueError):
    HAVE_GTK = False


class IconTests(unittest.TestCase):
    def test_every_state_and_badge_is_valid_xml_with_the_mark(self):
        import xml.dom.minidom as dom
        for state in icons.STATES:
            for badge in (False, True):
                doc = dom.parseString(icons.svg(state, badge))
                self.assertEqual(len(doc.getElementsByTagName("circle")) >= 2, True)
                self.assertEqual("FF8A3D" in icons.svg(state, badge).upper(), badge)

    def test_unknown_states_fall_back_to_stopped(self):
        self.assertEqual(icons.svg("nonsense"), icons.svg("stopped"))

    def test_write_all_is_idempotent_and_complete(self):
        d = tempfile.mkdtemp()
        first = icons.write_all(d)
        self.assertEqual(len(first), len(icons.STATES) * 2)
        mtimes = {p: os.path.getmtime(p) for p in first.values()}
        second = icons.write_all(d)
        self.assertEqual(first, second)
        self.assertEqual(mtimes, {p: os.path.getmtime(p) for p in second.values()}, "unchanged files are not rewritten")
        self.assertIsNotNone(icons.path_for(first, "connected", True))
        self.assertEqual(icons.path_for(first, "weird", False), first[("stopped", False)])

    @unittest.skipUnless(HAVE_GTK, "needs PyGObject")
    def test_the_icons_load_as_pictures(self):
        d = tempfile.mkdtemp()
        from gi.repository import GLib
        for path in icons.write_all(d).values():
            try:
                pixbuf = GdkPixbuf.Pixbuf.new_from_file_at_size(path, 24, 24)
            except GLib.Error as e:  # a machine without the SVG loader (librsvg)
                self.skipTest(f"cannot load SVG: {e.message}")
            self.assertEqual((pixbuf.get_width(), pixbuf.get_height()), (24, 24))


@unittest.skipUnless(HAVE_GTK, "needs PyGObject")
class ThemeTests(unittest.TestCase):
    def test_the_css_parses_without_a_single_error(self):
        provider = Gtk.CssProvider()
        errors = []
        provider.connect("parsing-error", lambda _p, section, err: errors.append((section.get_start_line(), err.message)))
        provider.load_from_data(theme.CSS.encode())
        self.assertEqual(errors, [])


if __name__ == "__main__":
    unittest.main()
