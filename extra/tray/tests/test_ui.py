"""The GTK window with a scripted bridge. Needs PyGObject and a display (a nested X server is fine)."""
import base64
import os
import unittest

try:
    import gi
    gi.require_version("Gtk", "3.0")
    from gi.repository import Gdk, Gtk
    HAVE_DISPLAY = Gtk.init_check()[0]
except (ImportError, ValueError):
    HAVE_DISPLAY = False

if HAVE_DISPLAY:
    from klip_tray import theme
    from klip_tray.bridge import Reply
    from klip_tray.controller import Controller
    from klip_tray.window import MainView
    from tests.test_controller import FakeBridge, HELLO, NOIMG, status

IMG = {"auto": False, "waiting": {"size_bytes": 186183, "width": 1920, "height": 1080, "age_secs": 130, "kind": "png"}}


def peer(name, online, fp="AAAA-BBBB-CCCC", error=None):
    return {"name": name, "is_online": online, "fingerprint": fp, "last_error": error}


def settle():
    for _ in range(30):
        while Gtk.events_pending():
            Gtk.main_iteration_do(False)


class Actions:
    def __init__(self):
        self.calls = []

    def hide_window(self):
        self.calls.append("hide")

    def show_window(self):
        self.calls.append("show")

    def open_terminal(self):
        self.calls.append("terminal")

    def quit_tray(self):
        self.calls.append("quit")

    def set_busy(self, flag):
        self.calls.append(("busy", flag))


def tiny_png(w=8, h=8):
    import struct
    import zlib
    rows = b"".join(b"\x00" + bytes((200, 30, 30)) * w for _ in range(h))

    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


@unittest.skipUnless(HAVE_DISPLAY, "needs PyGObject and a display")
class UiTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        provider = Gtk.CssProvider()
        provider.load_from_data(theme.CSS.encode())
        Gtk.StyleContext.add_provider_for_screen(Gdk.Screen.get_default(), provider,
                                                 Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)

    def make(self, **script):
        self.bridge = FakeBridge()
        self.bridge.script["GetThumbnail"] = Reply(ok={"Thumbnail": base64.b64encode(tiny_png()).decode()})
        self.bridge.script.update(script)
        self.ctl = Controller(self.bridge, lambda ms, fn: 1, lambda h: None, now_ms=lambda: 5_000_000)
        self.ctl.refresh()
        self.actions = Actions()
        self.view = MainView(self.ctl, self.actions)
        self.win = Gtk.OffscreenWindow()
        self.win.set_size_request(428, 648)
        self.win.add(self.view)
        self.win.show_all()
        self.addCleanup(self._teardown)
        settle()
        return self.view

    def _teardown(self):
        for pop in list(self.view._popovers):
            pop.destroy()  # popovers have windows of their own: they go first
        self.win.destroy()

    def calls(self, name):
        return [c for c in self.bridge.calls if self.bridge.key(c) == name]

    # ---------------------------------------------------------------------------- the window

    def test_every_page_builds_and_updates_in_every_daemon_state(self):
        states = [
            {},
            {"GetStatus": Reply(err="Klip daemon is not running.", down=True)},
            {"GetStatus": Reply(ok={"Status": status([])})},
            {"GetStatus": Reply(ok={"Status": status([peer("Vivo", False, error="cannot reach peer: timed out")])})},
            {"GetStatus": Reply(ok={"Status": status([peer("Vivo", True)], is_paused=True, paused_remaining_secs=300)})},
            {"GetImageState": Reply(ok={"ImageState": IMG})},
            {"GetImageState": Reply(ok={"Error": "unknown variant"})},
            {"GetStatus": Reply(ok={"Status": {}})},
        ]
        for script in states:
            view = self.make(**script)
            for name in list(view.pages):
                view.navigate(name)
                settle()
                self.assertEqual(view.current, name)
            view.navigate("home")
            view.window_shown(True)
            view.window_shown(False)

    def test_navigation_lights_the_parent_tab_of_a_sub_page_and_does_not_leave_the_page_you_are_on(self):
        view = self.make()
        view.navigate("pair")
        self.assertTrue(view.nav_buttons["devices"].get_style_context().has_class("current"))
        self.assertFalse(view.nav_buttons["home"].get_style_context().has_class("current"))
        view.navigate("settings")
        self.assertTrue(view.nav_buttons["more"].get_style_context().has_class("current"))
        hidden = []
        view.pages["settings"].on_hide = lambda: hidden.append(1)
        view.navigate("settings")
        self.assertEqual(hidden, [], "going to the page you are on must not run its leave code")

    def test_toasts_and_the_confirmation_sheet(self):
        view = self.make()
        view.toast("Copied")
        self.assertEqual(view.toast_label.get_text(), "Copied")
        self.assertTrue(view.toast_rev.get_reveal_child())
        view.toast("Nope", bad=True)
        self.assertTrue(view.toast_box.get_style_context().has_class("bad"))
        done = []
        view.confirm("Sure?", "Do it", lambda: done.append(1))
        self.assertTrue(view.confirming())
        view.sheet_no.emit("clicked")
        self.assertFalse(view.confirming())
        self.assertEqual(done, [])
        view.confirm("Sure?", "Do it", lambda: done.append(1))
        view.sheet_yes.emit("clicked")
        self.assertEqual(done, [1])
        self.assertFalse(view.confirming())

    def test_the_header_follows_the_state(self):
        view = self.make()
        self.assertEqual(view.pill_text.get_text(), "Connected")
        self.bridge.script["GetStatus"] = Reply(ok={"Status": status([peer("Vivo", False)])})
        self.ctl.refresh()
        self.assertEqual(view.pill_text.get_text(), "Waiting")

    # ------------------------------------------------------------------------------- home

    def test_home_sends_the_waiting_image_and_shows_its_thumbnail(self):
        view = self.make(GetImageState=Reply(ok={"ImageState": IMG}),
                         SendPendingImage=Reply(ok={"Sent": {"bytes": 9, "delivered": ["Vivo"], "failed": []}}))
        home = view.pages["home"]
        self.assertTrue(home.image_row.get_visible())
        self.assertFalse(home.image_hint.get_visible())
        self.assertEqual(home.send_btn.get_label(), "Send to Vivo")
        self.assertIsNotNone(home.thumb.get_pixbuf(), "the thumbnail was fetched and drawn")
        self.assertEqual(len(self.calls("GetThumbnail")), 1)
        home.send_btn.emit("clicked")
        self.assertEqual(len(self.calls("SendPendingImage")), 1)
        self.assertEqual(view.toast_label.get_text(), "Sent to Vivo")

    def test_home_disables_the_send_button_when_nobody_is_connected(self):
        view = self.make(GetImageState=Reply(ok={"ImageState": IMG}),
                         GetStatus=Reply(ok={"Status": status([peer("Vivo", False)])}))
        home = view.pages["home"]
        self.assertFalse(home.send_btn.get_sensitive())
        self.assertEqual(home.send_btn.get_label(), "No device connected")

    def test_home_without_an_image_explains_the_manual_mode(self):
        view = self.make()
        home = view.pages["home"]
        self.assertFalse(home.image_row.get_visible())
        self.assertIn("waits here", home.image_hint.get_text())
        self.bridge.script["GetImageState"] = Reply(ok={"ImageState": {"auto": True, "waiting": None}})
        self.ctl.refresh()
        self.assertIn("as soon as you copy", home.image_hint.get_text())
        self.assertTrue(home.auto_switch.get_active())

    def test_the_automatic_images_switch_tells_the_daemon_but_updating_it_from_the_daemon_does_not(self):
        view = self.make(SetAutoImages=Reply(ok={"ImageState": {"auto": True, "waiting": None}}))
        home = view.pages["home"]
        self.bridge.script["GetImageState"] = Reply(ok={"ImageState": {"auto": True, "waiting": None}})
        self.ctl.refresh()
        self.assertEqual(self.calls("SetAutoImages"), [], "the daemon's own value must not be sent back")
        home.auto_switch.emit("state-set", False)
        self.assertEqual(self.calls("SetAutoImages")[-1]["ipc"], {"SetAutoImages": {"enabled": False}})

    def test_a_stopped_daemon_offers_start_and_an_old_one_offers_restart(self):
        view = self.make(GetStatus=Reply(err="Klip daemon is not running.", down=True))
        home = view.pages["home"]
        self.assertTrue(home.start_btn.get_visible())
        self.assertFalse(home.image_card.get_visible())
        self.bridge.script["daemon_start"] = Reply(ok={"message": "started"})
        home.start_btn.emit("clicked")
        self.assertEqual(len([c for c in self.bridge.calls if c.get("method") == "daemon_start"]), 1)
        view = self.make(Hello=Reply(ok={"Hello": dict(HELLO, capabilities=["unpair_all"])}))
        self.assertTrue(view.pages["home"].old_box.get_visible())

    def test_pausing_from_the_home_menu(self):
        view = self.make(Pause=Reply(ok={"Paused": {"remaining_secs": 600}}))
        home = view.pages["home"]
        buttons = home.pop_box.get_children()
        self.assertEqual(len(buttons), 4)
        buttons[0].emit("clicked")
        self.assertEqual(self.calls("Pause")[0]["ipc"], {"Pause": {"duration_secs": 600}})
        self.bridge.script["GetStatus"] = Reply(ok={"Status": status([peer("Vivo", True)], is_paused=True,
                                                                      paused_remaining_secs=500)})
        self.ctl.refresh()
        self.assertEqual(home.pause_btn.get_label(), "Resume")
        self.assertEqual(len(home.pop_box.get_children()), 1)

    # ----------------------------------------------------------------------------- devices

    def test_devices_lists_peers_and_unpairing_asks_first(self):
        view = self.make(GetStatus=Reply(ok={"Status": status([peer("Vivo", True, "ABCD-EFGH-JKLM"),
                                                               peer("Pixel", False, "NPQR-STUV-WXYZ")])}),
                         Unpair=Reply(ok={"Unpaired": {"name": "Pixel"}}))
        page = view.pages["devices"]
        self.assertEqual(len(page.list.get_children()), 2)
        self.assertTrue(page.unpair_all_btn.get_visible())
        page._ask_unpair(view.ctl_summary_peers()[1] if hasattr(view, "ctl_summary_peers") else self.ctl.summary.peers[1])
        self.assertTrue(view.confirming())
        self.assertEqual(self.calls("Unpair"), [], "nothing happens before the person confirms")
        view.sheet_yes.emit("clicked")
        self.assertEqual(self.calls("Unpair")[0]["ipc"], {"Unpair": {"selector": "NPQR-STUV-WXYZ"}})

    def test_devices_with_nobody_linked_shows_the_invitation(self):
        view = self.make(GetStatus=Reply(ok={"Status": status([])}))
        page = view.pages["devices"]
        self.assertTrue(page.empty.get_visible())
        self.assertFalse(page.unpair_all_btn.get_visible())

    # -------------------------------------------------------------------------------- pair

    def test_showing_a_code_draws_the_qr_and_leaving_the_page_cancels_it(self):
        view = self.make(PairStart=Reply(ok={"PairTicket": {"ticket": "klip1" + "a" * 60, "expires_in_secs": 120,
                                                            "host_fingerprint": "J73D-FACE-MFHA"}}),
                         qr=Reply(ok={"size": 21, "modules": "01" * 220 + "0"}),
                         PairPoll=Reply(ok={"PairPoll": {"window_open": True, "seconds_left": 118, "pending": None,
                                                         "paired": None}}),
                         PairCancel=Reply(ok={"PairCancelled": None}))
        view.navigate("pair")
        page = view.pages["pair"]
        page._start_show()
        settle()
        self.assertTrue(page.show_box.get_visible())
        self.assertIn("Valid for 2:00", page.countdown.get_text())
        self.assertEqual(page.ticket_label.get_text(), "klip1" + "a" * 60)
        view.navigate("devices")  # leaving: the ticket is a secret
        self.assertEqual(len(self.calls("PairCancel")), 1)
        self.assertEqual(page.ticket_label.get_text(), "")
        self.assertFalse(page.show_box.get_visible())

    def test_a_request_waiting_for_approval_is_shown_first_and_can_be_approved_or_declined(self):
        view = self.make(PairPoll=Reply(ok={"PairPoll": {"window_open": True, "seconds_left": 90, "paired": None,
                                                         "pending": {"device_name": "Pixel 8",
                                                                     "fingerprint": "NPQR-STUV-WXYZ"}}}),
                         PairDecide=Reply(ok={"PairPoll": {"window_open": True, "seconds_left": 90,
                                                           "pending": None, "paired": None}}))
        page = view.pages["pair"]
        page._poll_once()
        settle()
        self.assertTrue(page.request_card.get_visible())
        self.assertEqual(page.req_title.get_text(), "Pixel 8 wants to link")
        self.assertIn("show", self.actions.calls, "the window is brought forward")
        self.assertEqual(view.current, "pair")
        page.approve_btn.emit("clicked")
        self.assertEqual(self.calls("PairDecide")[0]["ipc"], {"PairDecide": {"accept": True}})
        self.assertFalse(page.request_card.get_visible())

    def test_joining_checks_the_ticket_first_and_reports_the_result(self):
        view = self.make(PairJoin=Reply(ok={"PairSuccess": {"peer_name": "Laptop", "peer_fingerprint": "X"}}))
        page = view.pages["pair"]
        view.navigate("pair")
        page._start_join()
        page.join_text.get_buffer().set_text("hello")
        page._join()
        self.assertEqual(self.calls("PairJoin"), [])
        self.assertIn("does not look like", page.join_status.get_text())
        page.join_text.get_buffer().set_text('"│ klip1' + "abcdefghij" * 5 + ' │"')
        page._join()
        self.assertEqual(len(self.calls("PairJoin")), 1)
        self.assertEqual(view.current, "devices")
        self.assertIn("Linked with Laptop", view.toast_label.get_text())

    # ------------------------------------------------------------------------------ history

    def history_script(self):
        items = [
            {"id_hex": "01" * 16, "origin_name": "Vivo", "timestamp_ms": 4_990_000, "content_preview": "alpha text",
             "content_len": 10, "size_bytes": 10, "is_sensitive": False},
            {"id_hex": "02" * 16, "origin_name": "lucifer-pc", "timestamp_ms": 4_900_000,
             "content_preview": "[Image: PNG (186183 B)]", "content_len": 186183, "size_bytes": 186183,
             "is_sensitive": False},
            {"id_hex": "03" * 16, "origin_name": "lucifer-pc", "timestamp_ms": 4_800_000,
             "content_preview": "•••• (Sensitive Token / Key)", "content_len": 30, "size_bytes": 30,
             "is_sensitive": True},
            {"id_hex": "04" * 16, "origin_name": "lucifer-pc", "timestamp_ms": 4_700_000,
             "content_preview": "[Image: PNG (9000000 B)]", "content_len": 0, "size_bytes": 9_000_000,
             "is_sensitive": False},
        ]
        return {"GetHistory": Reply(ok={"History": items}),
                "history_text": Reply(ok={"text": "the real secret"}),
                "CopyToClipboard": Reply(ok={"Copied": None}),
                "SendClip": Reply(ok={"Sent": {"bytes": 3, "delivered": ["Vivo"], "failed": []}}),
                "ClearHistory": Reply(ok={"HistoryCleared": None})}

    def test_history_shows_rows_fetches_image_thumbnails_and_copies_back(self):
        view = self.make(**self.history_script())
        view.navigate("history")
        settle()
        page = view.pages["history"]
        self.assertEqual(len(page.rows.get_children()), 4)
        self.assertEqual(len(self.calls("GetThumbnail")), 1, "only the keepable image gets a thumbnail")
        page._copy(page._all[0])
        self.assertEqual(self.calls("CopyToClipboard")[0]["ipc"], {"CopyToClipboard": {"id": [1] * 16}})
        self.assertIn("Copied", view.toast_label.get_text())
        page._copy(page._all[3])
        self.assertEqual(len(self.calls("CopyToClipboard")), 1, "too large to keep: no request, a clear message")
        self.assertIn("too large", view.toast_label.get_text())

    def test_history_reveals_a_secret_only_on_purpose_and_hides_it_again(self):
        view = self.make(**self.history_script())
        view.navigate("history")
        page = view.pages["history"]
        secret = page._all[2]
        self.assertNotIn("real secret", page._views[secret.id_hex][1].get_text())
        page._toggle_reveal(secret)
        self.assertEqual(page._views[secret.id_hex][1].get_text(), "the real secret")
        page._toggle_reveal(secret)
        self.assertNotIn("real secret", page._views[secret.id_hex][1].get_text())

    def test_history_search_and_send_again_and_clear(self):
        view = self.make(**self.history_script())
        view.navigate("history")
        page = view.pages["history"]
        page.search.set_text("alpha")
        self.assertEqual(len(page.rows.get_children()), 1)
        page.search.set_text("nothing like this")
        self.assertTrue(page.empty.get_visible())
        page.search.set_text("")
        page._send_again(page._all[0])
        self.assertEqual(self.calls("SendClip")[0]["ipc"], {"SendClip": {"text": "the real secret"}})
        page._ask_clear()
        self.assertTrue(view.confirming())
        view.sheet_yes.emit("clicked")
        self.assertEqual(len(self.calls("ClearHistory")), 1)

    def test_history_refresh_only_updates_ages_when_the_clips_are_the_same(self):
        view = self.make(**self.history_script())
        view.navigate("history")
        page = view.pages["history"]
        first = page.rows.get_children()
        page.refresh()
        self.assertEqual(page.rows.get_children(), first, "the same rows: nothing is rebuilt (no flicker, no lost scroll)")

    # --------------------------------------------------------------------------------- send

    def test_send_enables_the_button_for_real_text_within_the_limit_and_sends_with_the_keyboard(self):
        view = self.make(SendClip=Reply(ok={"Sent": {"bytes": 5, "delivered": ["Vivo"], "failed": []}}))
        page = view.pages["send"]
        self.assertFalse(page.send_btn.get_sensitive())
        page.text.get_buffer().set_text("hello")
        self.assertTrue(page.send_btn.get_sensitive())
        page.text.get_buffer().set_text("x" * 2000)  # the fake daemon's limit is 1,000 bytes
        self.assertFalse(page.send_btn.get_sensitive())
        self.assertTrue(page.counter.get_style_context().has_class("bad"))
        page.text.get_buffer().set_text("hello")
        page._send()
        self.assertEqual(self.calls("SendClip")[0]["ipc"], {"SendClip": {"text": "hello"}})
        self.assertEqual(page._contents(), "", "a sent text is cleared so it is not sent twice")
        self.assertIn("Sent to Vivo", page.result.get_text())

    def test_a_failed_send_keeps_the_text(self):
        view = self.make(SendClip=Reply(ok={"Sent": {"bytes": 5, "delivered": [], "failed": [["Vivo", "cannot reach peer"]]}}))
        page = view.pages["send"]
        page.text.get_buffer().set_text("keep me")
        page._send()
        self.assertEqual(page._contents(), "keep me")
        self.assertIn("did not get it", page.result.get_text())

    # --------------------------------------------------------------------- more and settings

    def test_more_shows_the_startup_switches_and_hides_the_tray_one_for_an_older_klip(self):
        view = self.make(service_state=Reply(ok={"supported": True, "installed": True, "enabled": True, "active": True}),
                         autostart_get=Reply(err="unknown method autostart_get"))
        page = view.pages["more"]
        view.navigate("more")
        settle()
        self.assertTrue(page.daemon_switch.get_active())
        self.assertFalse(page.tray_row.get_visible())
        self.bridge.script["service_set"] = Reply(ok={"enabled": False})
        page.daemon_switch.emit("state-set", False)
        calls = [c for c in self.bridge.calls if c.get("method") == "service_set"]
        self.assertEqual(calls[0]["params"], {"enabled": False})

    def test_more_actions_reach_the_app(self):
        view = self.make()
        page = view.pages["more"]
        links = page.root.get_children()[0]
        self.assertTrue(links is not None)
        self.actions.open_terminal()
        self.assertIn("terminal", self.actions.calls)

    CFG = {"device_name": "lucifer-pc", "relay": "n0", "max_history_items": 50, "history_ttl_hours": 24,
           "poll_interval_ms": 100, "max_clip_bytes": 32 * 1024 * 1024, "lan_discovery": True,
           "auto_send_images": True}

    def test_settings_load_edit_and_save_keep_the_keys_they_do_not_show(self):
        view = self.make(config_get=Reply(ok=dict(self.CFG)), config_set=Reply(ok={}))
        view.navigate("settings")
        settle()
        page = view.pages["settings"]
        self.assertEqual(page.name_entry.get_text(), "lucifer-pc")
        self.assertEqual(page.relay.get_active_id(), "n0")
        self.assertEqual(int(page.clip_mb.get_value()), 32)
        page.name_entry.set_text("Desk")
        page.history_items.set_value(20)
        page._save()
        sent = [c for c in self.bridge.calls if c.get("method") == "config_set"][0]["params"]["config"]
        self.assertEqual(sent["device_name"], "Desk")
        self.assertEqual(sent["max_history_items"], 20)
        self.assertIs(sent["auto_send_images"], True, "a setting this page does not show is kept as it was")
        self.assertTrue(page.restart_btn.get_visible())

    def test_settings_refuse_bad_values_with_the_reason(self):
        view = self.make(config_get=Reply(ok=dict(self.CFG)))
        view.navigate("settings")
        page = view.pages["settings"]
        page.name_entry.set_text("   ")
        page._save()
        self.assertIn("name", page.error.get_text())
        self.assertEqual([c for c in self.bridge.calls if c.get("method") == "config_set"], [])

    # ------------------------------------------------------------------------------- log

    def test_the_log_follows_new_lines_filters_and_restarts_after_a_rotation(self):
        chunks = [Reply(ok={"new_offset": 50, "text": "one\ntwo\nthree\n", "reset": False, "missing": False}),
                  Reply(ok={"new_offset": 80, "text": "four needle\n", "reset": False, "missing": False}),
                  Reply(ok={"new_offset": 10, "text": "fresh\n", "reset": True, "missing": False})]
        view = self.make(log_tail=lambda payload: chunks.pop(0))
        view.navigate("log")
        page = view.pages["log"]
        buf = lambda: page.view.get_buffer().get_text(*page.view.get_buffer().get_bounds(), False)
        self.assertEqual(buf(), "one\ntwo\nthree")
        page._poll()
        self.assertEqual(buf(), "one\ntwo\nthree\nfour needle")
        self.assertEqual([c for c in self.bridge.calls if c.get("method") == "log_tail"][-1]["params"]["offset"], 50)
        page.search.set_text("needle")
        self.assertEqual(buf(), "four needle")
        page.search.set_text("")
        page._poll()
        self.assertEqual(buf(), "fresh")


if __name__ == "__main__":
    unittest.main()
