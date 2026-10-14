import os
import tempfile
import unittest
import xml.dom.minidom

from klip_tray import icons


class Icons(unittest.TestCase):
    def test_every_state_is_a_valid_svg_with_fills_only(self):
        for state in icons.STATES:
            for badge in (False, True):
                text = icons.svg(state, badge)
                xml.dom.minidom.parseString(text)  # well formed
                self.assertNotIn("stroke", text, "symbolic icons are recoloured through fills")
                self.assertNotIn("#", text, "no colour of its own: the panel paints it")

    def test_the_states_look_different(self):
        shapes = {icons.svg(s) for s in icons.STATES}
        self.assertEqual(len(shapes), len(icons.STATES))
        self.assertNotEqual(icons.svg("connected"), icons.svg("connected", True), "the badge shows")

    def test_names_are_symbolic_and_unknown_states_fall_back_to_stopped(self):
        self.assertEqual(icons.name_for("connected", False), "klip-connected-symbolic")
        self.assertEqual(icons.name_for("paused", True), "klip-paused-badge-symbolic")
        self.assertEqual(icons.name_for("nonsense", False), "klip-stopped-symbolic")
        self.assertEqual(icons.svg("nonsense"), icons.svg("stopped"))

    def test_install_writes_every_icon_into_the_user_icon_theme_and_is_repeatable(self):
        with tempfile.TemporaryDirectory() as home:
            icons.install(home)
            directory = os.path.join(home, "icons", "hicolor", "scalable", "status")
            names = sorted(os.listdir(directory))
            self.assertEqual(len(names), len(icons.STATES) * 2)
            self.assertTrue(all(n.endswith("-symbolic.svg") for n in names))
            first = os.stat(os.path.join(directory, names[0])).st_mtime_ns
            icons.install(home)
            self.assertEqual(os.stat(os.path.join(directory, names[0])).st_mtime_ns, first, "unchanged = untouched")
            self.assertEqual(sorted(os.listdir(directory)), names, "no temp files left behind")
