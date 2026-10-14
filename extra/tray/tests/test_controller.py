import base64
import unittest

from klip_tray.bridge import Reply
from klip_tray.controller import Controller, Outcome


def peer(name, online, error=None):
    return {"name": name, "is_online": online, "fingerprint": "AAAA-BBBB-CCCC", "last_error": error}


def status(peers=(), **kw):
    base = {"is_active": True, "is_paused": False, "paused_remaining_secs": 0, "platform": "Linux (X11)",
            "memory_rss_bytes": 1000, "fingerprint": "J73D-FACE-MFHA", "relay_mode": "n0",
            "relay_state": "relay connected", "pairing_open": False, "peers": list(peers),
            "total_synced_clips": 0, "last_sync_time_secs_ago": None, "last_clip_preview": None,
            "last_clip_size_bytes": 0}
    base.update(kw)
    return base


HELLO = {"protocol": 2, "daemon_version": "0.6.0", "capabilities": ["share_image"],
         "config": {"device_name": "L", "relay": "n0", "max_history_items": 50, "max_clip_bytes": 1000,
                    "lan_discovery": True}, "max_request_bytes": 66536}
NOIMG = {"auto": False, "waiting": None}


class FakeBridge:
    """Answers from a script keyed by the IPC variant name or the method name; can hold answers back."""

    def __init__(self):
        self.script = {
            "GetStatus": Reply(ok={"Status": status([peer("Vivo", True)])}),
            "GetImageState": Reply(ok={"ImageState": NOIMG}),
            "Hello": Reply(ok={"Hello": HELLO}),
        }
        self.calls = []
        self.held = None  # a list while answers are held back

    @staticmethod
    def key(payload):
        if "ipc" in payload:
            ipc = payload["ipc"]
            return ipc if isinstance(ipc, str) else next(iter(ipc))
        return payload["method"]

    def request(self, payload, callback=None, timeout=None):
        self.calls.append(payload)
        reply = self.script.get(self.key(payload), Reply(ok={"Resumed": None}))
        if callable(reply):
            reply = reply(payload)
        if self.held is not None:
            self.held.append((callback, reply))
        elif callback:
            callback(reply)

    def release(self):
        held, self.held = self.held, None
        for cb, reply in held:
            if cb:
                cb(reply)

    def names(self):
        return [self.key(c) for c in self.calls]


class Clock:
    def __init__(self):
        self.timers = []

    def schedule(self, ms, fn):
        self.timers.append([ms, fn, True])
        return len(self.timers) - 1

    def cancel(self, handle):
        self.timers[handle][2] = False

    def due(self):
        return [t for t in self.timers if t[2]]


class ControllerTests(unittest.TestCase):
    def make(self):
        self.bridge = FakeBridge()
        self.clock = Clock()
        self.seen = []
        c = Controller(self.bridge, self.clock.schedule, self.clock.cancel, now_ms=lambda: 5_000_000)
        c.listeners.append(self.seen.append)
        return c

    def test_a_refresh_builds_the_summary_and_tells_listeners_only_when_it_changes(self):
        c = self.make()
        c.refresh()
        self.assertEqual(c.summary.state, "connected")
        self.assertEqual(len(self.seen), 1)
        c.refresh()
        self.assertEqual(len(self.seen), 1, "an identical picture is not announced again")
        self.bridge.script["GetStatus"] = Reply(ok={"Status": status([peer("Vivo", False)])})
        c.refresh()
        self.assertEqual((c.summary.state, len(self.seen)), ("waiting", 2))

    def test_hello_is_asked_once_and_again_after_the_daemon_was_down(self):
        c = self.make()
        c.refresh()
        c.refresh()
        self.assertEqual(self.bridge.names().count("Hello"), 1)
        self.bridge.script["GetStatus"] = Reply(err="Klip daemon is not running.", down=True)
        c.refresh()
        self.assertEqual(c.summary.state, "stopped")
        self.bridge.script["GetStatus"] = Reply(ok={"Status": status([peer("Vivo", True)])})
        c.refresh()
        self.assertEqual(self.bridge.names().count("Hello"), 2)

    def test_a_daemon_that_does_not_know_images_is_called_old(self):
        c = self.make()
        self.bridge.script["GetImageState"] = Reply(ok={"Error": "unknown variant"})
        self.bridge.script["Hello"] = Reply(ok={"Hello": dict(HELLO, capabilities=["unpair_all"])})
        c.refresh()
        self.assertTrue(c.summary.daemon_old)
        self.assertEqual(c.summary.state, "connected")

    def test_a_poll_in_flight_is_not_doubled(self):
        c = self.make()
        self.bridge.held = []
        c.refresh()
        c.refresh()
        c.refresh()
        self.assertEqual(self.bridge.names().count("GetStatus"), 1)
        self.bridge.release()
        self.assertEqual(self.bridge.names().count("GetStatus"), 2, "one more poll follows, not three")

    def test_the_next_poll_is_scheduled_and_speeds_up_while_the_window_is_open(self):
        c = self.make()
        c.refresh()
        self.assertEqual([t[0] for t in self.clock.due()], [15_000])
        c.set_fast(True)
        self.assertEqual([t[0] for t in self.clock.due()], [3_000])
        c.set_fast(True)
        self.assertEqual(len(self.clock.due()), 1, "no second timer")
        c.set_fast(False)
        self.assertEqual([t[0] for t in self.clock.due()], [15_000])

    def test_a_listener_that_raises_does_not_stop_polling(self):
        c = self.make()
        c.listeners.insert(0, lambda s: 1 / 0)
        c.refresh()
        self.assertEqual(len(self.clock.due()), 1)
        self.assertEqual(len(self.seen), 1, "the other listeners still hear it")

    def test_send_the_waiting_image_reports_and_refreshes(self):
        c = self.make()
        self.bridge.script["SendPendingImage"] = Reply(ok={"Sent": {"bytes": 9, "delivered": ["Vivo"], "failed": []}})
        out = []
        c.send_pending_image(out.append)
        self.assertEqual(out[0].value, "Sent to Vivo")
        self.assertTrue(out[0].ok)
        self.assertIn("GetStatus", self.bridge.names())

    def test_a_refusal_comes_back_as_an_error_not_an_exception(self):
        c = self.make()
        self.bridge.script["SendPendingImage"] = Reply(ok={"Error": "no image is waiting to be sent"})
        out = []
        c.send_pending_image(out.append)
        self.assertEqual(out[0].error, "no image is waiting to be sent")
        self.bridge.script["SendClip"] = Reply(err="Klip did not answer in time")
        c.send_text("hi", out.append)
        self.assertEqual(out[1].error, "Klip did not answer in time")

    def test_sending_text_is_checked_before_it_leaves(self):
        c = self.make()
        c.refresh()
        out = []
        c.send_text("   ", out.append)
        self.assertIn("Type something", out[0].error)
        self.assertNotIn("SendClip", self.bridge.names())
        c.send_text("x" * 70_000, out.append)
        self.assertIn("limit", out[1].error)
        self.assertNotIn("SendClip", self.bridge.names(), "the limit comes from the daemon's Hello")

    def test_the_small_actions_send_what_the_daemon_expects(self):
        c = self.make()
        c.set_auto_images(True)
        c.pause(600)
        c.resume()
        c.unpair("Vivo")
        c.unpair_all()
        c.copy_back("ab" * 16)
        c.clear_history()
        calls = {self.bridge.key(p): p["ipc"] for p in self.bridge.calls}
        self.assertEqual(calls["SetAutoImages"], {"SetAutoImages": {"enabled": True}})
        self.assertEqual(calls["Pause"], {"Pause": {"duration_secs": 600}})
        self.assertEqual(calls["Resume"], "Resume")
        self.assertEqual(calls["Unpair"], {"Unpair": {"selector": "Vivo"}})
        self.assertEqual(calls["UnpairAll"], "UnpairAll")
        self.assertEqual(calls["CopyToClipboard"], {"CopyToClipboard": {"id": [0xAB] * 16}})
        self.assertEqual(calls["ClearHistory"], "ClearHistory")

    def test_history_comes_back_as_rows(self):
        c = self.make()
        item = {"id_hex": "ab" * 16, "origin_name": "Vivo", "timestamp_ms": 4_940_000, "content_preview": "hello",
                "content_len": 5, "size_bytes": 5, "is_sensitive": False}
        self.bridge.script["GetHistory"] = Reply(ok={"History": [item]})
        out = []
        c.history(out.append)
        row = out[0].value[0]
        self.assertEqual((row.kind, row.age, row.can_copy), ("text", "1 min ago", True))

    def test_a_thumbnail_arrives_as_png_bytes(self):
        c = self.make()
        self.bridge.script["GetThumbnail"] = Reply(ok={"Thumbnail": base64.b64encode(b"\x89PNGdata").decode()})
        out = []
        c.thumbnail(None, out.append)
        self.assertEqual(out[0].value, b"\x89PNGdata")
        c.thumbnail("cd" * 16, out.append)
        self.assertEqual(self.bridge.calls[-1]["ipc"], {"GetThumbnail": {"id": [0xCD] * 16}})

    def test_pairing_calls(self):
        c = self.make()
        self.bridge.script["PairStart"] = Reply(ok={"PairTicket": {"ticket": "klip1x", "expires_in_secs": 120,
                                                                   "host_fingerprint": "J73D-FACE-MFHA"}})
        self.bridge.script["qr"] = Reply(ok={"size": 21, "modules": "0" * 441})
        out = []
        c.pair_start(out.append, lan=True)
        self.assertEqual(self.bridge.calls[0]["ipc"], {"PairStart": {"ttl_secs": 120, "include_lan_addrs": True}})
        self.assertEqual(out[0].value["ticket"], "klip1x")
        c.qr("klip1x", out.append)
        self.assertEqual(out[1].value["size"], 21)
        self.bridge.script["PairPoll"] = Reply(ok={"PairPoll": {"window_open": True, "seconds_left": 90, "pending": None, "paired": None}})
        c.pair_poll(out.append)
        self.assertEqual(out[2].value["seconds_left"], 90)
        c.pair_decide(True, out.append)
        c.pair_cancel(out.append)
        c.pair_join("klip1 abcdefghijklmnopqrstuvwxyz", out.append)
        joins = [p["ipc"] for p in self.bridge.calls if self.bridge.key(p) == "PairJoin"]
        self.assertEqual(joins, [{"PairJoin": {"ticket": "klip1abcdefghijklmnopqrstuvwxyz"}}])
        out.clear()
        c.pair_join("hello", out.append)
        self.assertIn("ticket", out[0].error)

    def test_settings_are_checked_before_they_are_saved(self):
        c = self.make()
        out = []
        c.config_set({"device_name": " ", "relay": "n0", "poll_interval_ms": 100, "max_clip_bytes": 1000,
                      "history_ttl_hours": 1, "max_history_items": 5}, out.append)
        self.assertIn("name", out[0].error)
        self.assertNotIn("config_set", self.bridge.names())
        good = {"device_name": "L", "relay": "n0", "poll_interval_ms": 100, "max_clip_bytes": 1000,
                "history_ttl_hours": 1, "max_history_items": 5}
        self.bridge.script["config_set"] = Reply(ok={})
        c.config_set(good, out.append)
        self.assertEqual(self.bridge.calls[-1], {"method": "config_set", "params": {"config": good}})

    def test_the_daemon_helpers_use_the_bridge_methods(self):
        c = self.make()
        for name in ("daemon_start", "daemon_restart", "service_state"):
            self.bridge.script[name] = Reply(ok={"message": name})
        out = []
        c.daemon_start(out.append)
        c.daemon_restart(out.append)
        c.service_state(out.append)
        c.service_set(True, out.append)
        methods = [p for p in self.bridge.calls if "method" in p]
        self.assertEqual([p["method"] for p in methods],
                         ["daemon_start", "daemon_restart", "service_state", "service_set"])
        self.assertEqual(methods[3]["params"], {"enabled": True})


if __name__ == "__main__":
    unittest.main()
