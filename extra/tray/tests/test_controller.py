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


HELLO = {"protocol": 2, "daemon_version": "0.6.1", "capabilities": ["share_image"],
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
        c = Controller(self.bridge, self.clock.schedule, self.clock.cancel)
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

    def test_a_failed_hello_is_asked_again_instead_of_calling_the_daemon_old(self):
        c = self.make()
        self.bridge.script["Hello"] = Reply(err="Klip did not answer in time")
        c.refresh()
        self.assertIsNone(c.caps, "an unknown is not 'old'")
        self.assertFalse(c.summary.daemon_old)
        self.bridge.script["Hello"] = Reply(ok={"Hello": HELLO})
        c.refresh()
        self.assertEqual(c.caps, ["share_image"])
        self.assertEqual(self.bridge.names().count("Hello"), 2)

    def test_after_starting_the_daemon_its_abilities_are_asked_again(self):
        c = self.make()
        c.refresh()
        self.bridge.script["daemon_start"] = Reply(ok={"message": "started"})
        before = len(self.bridge.calls)
        c.daemon_start(None)
        asked = [self.bridge.key(x) for x in self.bridge.calls[before:]]
        self.assertIn("Hello", asked, "a started daemon may be a newer one")

    def test_a_poll_never_stays_stuck_when_building_the_picture_fails(self):
        from klip_tray import model
        c = self.make()
        original = model.summarize
        model.summarize = lambda *a, **k: 1 / 0
        try:
            c.refresh()
        finally:
            model.summarize = original
        c.refresh()
        self.assertEqual(c.summary.state, "connected", "the next poll works again")

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

    def test_the_next_poll_is_scheduled_once(self):
        c = self.make()
        c.refresh()
        c.refresh()
        self.assertEqual([t[0] for t in self.clock.due()], [15_000], "one timer, never two")

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
        self.bridge.script["SendPendingImage"] = Reply(err="Klip did not answer in time")
        c.send_pending_image(out.append)
        self.assertEqual(out[1].error, "Klip did not answer in time")

    def test_the_small_actions_send_what_the_daemon_expects(self):
        c = self.make()
        c.set_auto_images(True)
        c.pause(600)
        c.resume()
        calls = {self.bridge.key(p): p["ipc"] for p in self.bridge.calls}
        self.assertEqual(calls["SetAutoImages"], {"SetAutoImages": {"enabled": True}})
        self.assertEqual(calls["Pause"], {"Pause": {"duration_secs": 600}})
        self.assertEqual(calls["Resume"], "Resume")
        self.bridge.script["daemon_start"] = Reply(ok={"message": "started"})
        c.daemon_start()
        self.assertEqual(self.bridge.calls[-4]["method"], "daemon_start", "then the picture is refreshed")


if __name__ == "__main__":
    unittest.main()
