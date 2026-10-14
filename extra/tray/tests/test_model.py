import unittest

from klip_tray import model as m


def peer(name, online, fp="AAAA-BBBB-CCCC", error=None):
    return {"name": name, "is_online": online, "fingerprint": fp, "last_error": error}


def status(peers=(), paused=False, paused_left=0, **kw):
    base = {
        "is_active": True, "is_paused": paused, "paused_remaining_secs": paused_left,
        "platform": "Linux (X11)", "memory_rss_bytes": 16 * 1024 * 1024,
        "fingerprint": "J73D-FACE-MFHA", "relay_mode": "n0 public relays + discovery",
        "relay_state": "relay connected (https://x/)", "pairing_open": False,
        "peers": list(peers), "total_synced_clips": 3,
        "last_sync_time_secs_ago": 125, "last_clip_preview": "hello", "last_clip_size_bytes": 5,
    }
    base.update(kw)
    return base


IMG = {"auto": False, "waiting": {"size_bytes": 186183, "width": 1920, "height": 1080,
                                  "age_secs": 130, "kind": "png"}}
NOIMG = {"auto": False, "waiting": None}


class Formatting(unittest.TestCase):
    def test_ages_match_the_terminal_app(self):
        self.assertEqual(m.ago(0), "just now")
        self.assertEqual(m.ago(1), "just now")
        self.assertEqual(m.ago(4), "4s ago")
        self.assertEqual(m.ago(125), "2 min ago")
        self.assertEqual(m.ago(7200), "2 h ago")
        self.assertEqual(m.ago(200_000), "2 d ago")
        self.assertEqual(m.ago(-5), "just now")

    def test_byte_sizes_match_the_terminal_app(self):
        self.assertEqual(m.size(0), "0 B")
        self.assertEqual(m.size(1234), "1,234 B")
        self.assertEqual(m.size(10 * 1024), "10.0 KiB")
        self.assertEqual(m.size(186183), "181.8 KiB")
        self.assertEqual(m.size(5 * 1024 * 1024), "5.0 MiB")

    def test_countdowns(self):
        self.assertEqual(m.remaining(90), "1:30")
        self.assertEqual(m.remaining(3900), "1 h 05 min")
        self.assertEqual(m.remaining(315_360_000), "until you resume")

    def test_plain_errors_say_what_a_person_can_do(self):
        self.assertIn("removed this device", m.plain_error("unpaired by the other device; pair again"))
        self.assertIn("paused", m.plain_error("the other device has sync paused"))
        self.assertIn("clipboard", m.plain_error("the other device could not write its clipboard"))
        self.assertIn("reconnects by itself", m.plain_error("cannot reach peer: timed out"))
        self.assertEqual(m.plain_error("x" * 200), "x" * 90)
        self.assertEqual(m.plain_error(None), "")


class Variants(unittest.TestCase):
    def test_serde_enums_become_name_and_payload(self):
        self.assertEqual(m.variant("Resumed"), ("Resumed", None))
        self.assertEqual(m.variant({"Paused": {"remaining_secs": 5}}), ("Paused", {"remaining_secs": 5}))
        self.assertEqual(m.variant({"Thumbnail": "abc"}), ("Thumbnail", "abc"))
        self.assertEqual(m.variant(None), (None, None))
        self.assertEqual(m.variant({"a": 1, "b": 2}), (None, None), "ambiguous shapes are not guessed")


class Summaries(unittest.TestCase):
    def test_daemon_down_is_stopped(self):
        s = m.summarize(None, None, down=True)
        self.assertEqual(s.state, "stopped")
        self.assertEqual(s.title, "Klip is stopped")
        self.assertFalse(s.badge)

    def test_no_devices(self):
        s = m.summarize(status(), NOIMG)
        self.assertEqual((s.state, s.title), ("no_devices", "No devices linked"))

    def test_connected_names_the_device_and_the_last_sync(self):
        s = m.summarize(status([peer("Vivo V2253", True)]), NOIMG)
        self.assertEqual((s.state, s.title), ("connected", "Connected to Vivo V2253"))
        self.assertEqual(s.subtitle, "Last sync 2 min ago")
        two = m.summarize(status([peer("A", True), peer("B", True)]), NOIMG)
        self.assertEqual(two.title, "Connected to A and B")
        three = m.summarize(status([peer("A", True), peer("B", True), peer("C", True)]), NOIMG)
        self.assertEqual(three.title, "Connected to A and 2 more")
        mixed = m.summarize(status([peer("A", True), peer("B", False)]), NOIMG)
        self.assertEqual(mixed.title, "Connected to A", "only the connected ones are named")

    def test_connected_but_nothing_synced_yet(self):
        s = m.summarize(status([peer("A", True)], last_sync_time_secs_ago=None), NOIMG)
        self.assertEqual(s.subtitle, "Nothing synced yet")

    def test_waiting_explains_in_plain_words(self):
        s = m.summarize(status([peer("Vivo V2253", False, error="cannot reach peer: timed out")]), NOIMG)
        self.assertEqual((s.state, s.title), ("waiting", "Waiting for Vivo V2253"))
        self.assertIn("reconnects by itself", s.subtitle)
        quiet = m.summarize(status([peer("Vivo", False)]), NOIMG)
        self.assertIn("reconnect", quiet.subtitle)

    def test_pause_wins_over_everything_and_shows_what_is_left(self):
        s = m.summarize(status([peer("A", True)], paused=True, paused_left=300), IMG)
        self.assertEqual((s.state, s.title, s.subtitle), ("paused", "Paused", "5:00 left"))
        forever = m.summarize(status([peer("A", True)], paused=True, paused_left=315_000_000), NOIMG)
        self.assertEqual(forever.subtitle, "until you resume")

    def test_a_waiting_image_adds_a_badge_but_only_in_manual_mode(self):
        s = m.summarize(status([peer("A", True)]), IMG)
        self.assertTrue(s.badge)
        self.assertEqual(s.image.text, "PNG 1920×1080 · 181.8 KiB · 2 min ago")
        auto = m.summarize(status([peer("A", True)]), {"auto": True, "waiting": IMG["waiting"]})
        self.assertFalse(auto.badge)
        self.assertTrue(auto.auto_images)
        self.assertIn("Image ready to send", s.tooltip)

    def test_an_old_daemon_without_image_support_is_noticed(self):
        s = m.summarize(status([peer("A", True)]), None, caps=["unpair_all"])
        self.assertTrue(s.daemon_old)
        self.assertFalse(m.summarize(status([peer("A", True)]), NOIMG, caps=["share_image"]).daemon_old)
        self.assertFalse(m.summarize(status([peer("A", True)]), NOIMG, caps=None).daemon_old)

    def test_peers_carry_fingerprints_and_plain_errors(self):
        s = m.summarize(status([peer("A", False, "ZZZZ-1111-2222", "the other device has sync paused")]), NOIMG)
        p = s.peers[0]
        self.assertEqual((p.name, p.fingerprint, p.online), ("A", "ZZZZ-1111-2222", False))
        self.assertIn("paused", p.error)

    def test_an_open_pairing_window_is_noticed(self):
        self.assertTrue(m.summarize(status([], pairing_open=True), NOIMG).pairing_open)
        self.assertFalse(m.summarize(status([]), NOIMG).pairing_open)
        self.assertFalse(m.summarize(None, None, down=True).pairing_open)

    def test_a_malformed_status_never_raises(self):
        for bad in ({}, {"peers": None}, {"peers": [{}]}, {"peers": ["x"]}, []):
            s = m.summarize(bad, {"waiting": "nonsense"})
            self.assertIn(s.state, ("no_devices", "waiting", "stopped"))


class Pause(unittest.TestCase):
    def test_the_pause_choices(self):
        labels = [label for label, _ in m.PAUSE_CHOICES]
        self.assertEqual(labels, ["10 minutes", "1 hour", "8 hours", "Until I resume"])
        self.assertEqual(m.PAUSE_CHOICES[0][1], 600)
        self.assertGreater(m.PAUSE_CHOICES[-1][1], 365 * 24 * 3600)


class SentReports(unittest.TestCase):
    def test_every_outcome_has_words(self):
        ok, msg = m.describe_sent({"bytes": 5, "delivered": ["Vivo"], "failed": []})
        self.assertTrue(ok)
        self.assertEqual(msg, "Sent to Vivo")
        ok, msg = m.describe_sent({"bytes": 5, "delivered": ["A", "B"], "failed": []})
        self.assertEqual(msg, "Sent to A and B")
        ok, msg = m.describe_sent({"bytes": 5, "delivered": [], "failed": [["Vivo", "cannot reach peer: timed out"]]})
        self.assertFalse(ok)
        self.assertIn("Vivo did not get it", msg)
        self.assertIn("reconnects by itself", msg)
        ok, msg = m.describe_sent({"bytes": 5, "delivered": ["A"], "failed": [["B", "the other device has sync paused"]]})
        self.assertTrue(ok)
        self.assertIn("Sent to A", msg)
        self.assertIn("B did not get it", msg)
        ok, msg = m.describe_sent({"bytes": 0, "delivered": [], "failed": []})
        self.assertFalse(ok)
        self.assertIn("No devices", msg)
        ok, msg = m.describe_sent(None)
        self.assertFalse(ok)


if __name__ == "__main__":
    unittest.main()
