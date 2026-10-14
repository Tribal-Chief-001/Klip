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


class History(unittest.TestCase):
    def item(self, **kw):
        base = {"id_hex": "ab" * 16, "origin_name": "Vivo V2253", "timestamp_ms": 1_000_000,
                "content_preview": "hello world", "content_len": 11, "size_bytes": 11, "is_sensitive": False}
        base.update(kw)
        return base

    def test_rows_get_a_kind_an_age_and_a_size(self):
        rows = m.history_rows([self.item(), self.item(content_preview="[Image: PNG (186183 B)]",
                                                      content_len=186183, size_bytes=186183, id_hex="cd" * 16),
                               self.item(is_sensitive=True, content_preview="••••", id_hex="ef" * 16)],
                              now_ms=1_000_000 + 125_000)
        self.assertEqual([r.kind for r in rows], ["text", "image", "secret"])
        self.assertEqual(rows[0].age, "2 min ago")
        self.assertEqual(rows[1].size_text, "181.8 KiB")
        self.assertTrue(rows[0].can_copy and rows[1].can_copy)

    def test_a_clip_too_large_to_keep_cannot_be_copied_back(self):
        row = m.history_rows([self.item(content_len=0, size_bytes=5_000_000,
                                        content_preview="[Image: PNG (5000000 B)]")], now_ms=2_000_000)[0]
        self.assertFalse(row.can_copy)
        self.assertEqual(row.kind, "image")

    def test_search_matches_preview_and_device_case_insensitively(self):
        rows = m.history_rows([self.item(content_preview="Invoice 42"), self.item(content_preview="other",
                                                                                 origin_name="Laptop", id_hex="11" * 16)],
                              now_ms=2_000_000)
        self.assertEqual(len(m.filter_rows(rows, "invoice")), 1)
        self.assertEqual(len(m.filter_rows(rows, "LAPTOP")), 1)
        self.assertEqual(len(m.filter_rows(rows, "")), 2)
        self.assertEqual(len(m.filter_rows(rows, "nothing")), 0)

    def test_secrets_are_never_searchable_by_their_hidden_text(self):
        rows = m.history_rows([self.item(is_sensitive=True, content_preview="•••• (Sensitive Token / Key)")],
                              now_ms=2_000_000)
        self.assertEqual(len(m.filter_rows(rows, "sensitive")), 0)


class Tickets(unittest.TestCase):
    def test_a_pasted_ticket_survives_borders_quotes_and_line_breaks(self):
        raw = '"│ klip1ABCDEFGHIJKLMNOPQRSTUV │\n│ 234567abcdefghijkl │"'
        self.assertTrue(m.looks_like_ticket(raw))
        self.assertEqual(m.clean_ticket(raw), "klip1abcdefghijklmnopqrstuv234567abcdefghijkl")

    def test_other_text_is_not_a_ticket(self):
        for bad in ("", "hello", "klip1", "klip1!!!", "k" * 3000, "klip2abcdefghijklmnopqrstuvwxyz"):
            self.assertFalse(m.looks_like_ticket(bad), bad)


class Pause(unittest.TestCase):
    def test_the_pause_choices(self):
        labels = [label for label, _ in m.PAUSE_CHOICES]
        self.assertEqual(labels, ["10 minutes", "1 hour", "8 hours", "Until I resume"])
        self.assertEqual(m.PAUSE_CHOICES[0][1], 600)
        self.assertGreater(m.PAUSE_CHOICES[-1][1], 365 * 24 * 3600)


class Settings(unittest.TestCase):
    def good(self):
        return {"device_name": "Laptop", "relay": "n0", "max_history_items": 50, "history_ttl_hours": 24,
                "poll_interval_ms": 100, "max_clip_bytes": 32 * 1024 * 1024, "lan_discovery": True,
                "auto_send_images": False}

    def test_a_good_configuration_passes(self):
        self.assertEqual(m.config_problems(self.good()), [])

    def test_each_rule_of_the_daemon_is_checked_before_saving(self):
        for key, value, word in (("device_name", "  ", "name"), ("poll_interval_ms", 5, "poll"),
                                 ("max_clip_bytes", 0, "clip"), ("max_clip_bytes", 300 * 1024 * 1024, "clip"),
                                 ("history_ttl_hours", 9000, "hours"), ("max_history_items", 0, "history"),
                                 ("relay", "wan", "relay")):
            cfg = self.good()
            cfg[key] = value
            problems = m.config_problems(cfg)
            self.assertTrue(any(word in p.lower() for p in problems), (key, problems))

    def test_whole_numbers_are_read_from_text(self):
        self.assertEqual(m.to_int(" 42 ", 0, 100), 42)
        for bad in ("", "x", "1.5", "-1", "101"):
            with self.assertRaises(ValueError):
                m.to_int(bad, 0, 100)


class Sizes(unittest.TestCase):
    def test_text_that_is_too_big_is_refused_before_it_is_sent(self):
        self.assertIsNone(m.send_problem("hello", limit=1000))
        self.assertIsNotNone(m.send_problem("", limit=1000))
        self.assertIsNotNone(m.send_problem("   \n", limit=1000))
        self.assertIn("1,000 B", m.send_problem("x" * 2000, limit=1000))
        self.assertIsNone(m.send_problem("é" * 400, limit=1000))
        self.assertIsNotNone(m.send_problem("é" * 600, limit=1000), "limits count bytes, not letters")


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
