"""The tray controller and the real `klip api` bridge, against real daemons (no GTK).

  KLIP_BIN=target/debug/klip KLIP_PEER_BIN=target/debug/examples/emulator_peer \
      python3 -m unittest tests.integration -v

Daemon A (the test peer, an in-memory clipboard that can "copy" an image) is the one the tray
controller drives through the real `klip api` bridge; daemon B is an ordinary `klip` the test pairs
with. Everything stays on this machine (LAN mode); the real daemon and phone are never involved.
"""
import base64
import json
import os
import shutil
import subprocess
import tempfile
import threading
import time
import unittest

from klip_tray import model
from klip_tray.bridge import Bridge
from klip_tray.controller import Controller

KLIP = os.environ.get("KLIP_BIN")
PEER = os.environ.get("KLIP_PEER_BIN")


def png(width=64, height=48):
    import struct
    import zlib
    rows = b"".join(b"\x00" + b"".join(bytes(((x * 4) % 256, (y * 5) % 256, 120)) for x in range(width))
                    for y in range(height))

    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


def read_json(path):
    with open(path) as f:
        return json.load(f)


def wait_for(cond, secs=20.0, what="a condition"):
    end = time.time() + secs
    while time.time() < end:
        value = cond()
        if value:
            return value
        time.sleep(0.1)
    raise AssertionError(f"timed out waiting for {what}")


def sync(method, *args, **kw):
    """Call a Controller method that takes a callback as its last positional argument and wait for it."""
    box, done = [], threading.Event()
    method(*args, lambda out: (box.append(out), done.set()), **kw)
    if not done.wait(120):
        raise AssertionError("no answer")
    return box[0]


@unittest.skipUnless(KLIP and PEER, "set KLIP_BIN and KLIP_PEER_BIN to run against real daemons")
class RealStack(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.mkdtemp(prefix="klip-it-")
        cls.a_dir = os.path.join(cls.tmp, "a")
        cls.b_dir = os.path.join(cls.tmp, "b")
        cls.work = os.path.join(cls.tmp, "work")
        for d in (cls.a_dir, cls.b_dir, cls.work):
            os.makedirs(d)
        with open(os.path.join(cls.b_dir, "config.json"), "w") as f:
            json.dump({"relay": "lan", "lan_discovery": False, "device_name": "Bee"}, f)
        cls.peer = None
        cls.start_a()
        env_b = dict(os.environ, KLIP_CONFIG_DIR=cls.b_dir, KLIP_CLIPBOARD="memory")
        subprocess.run([KLIP, "start"], env=env_b, check=True, stdout=subprocess.DEVNULL)
        cls.env_b = env_b
        cls.bridge_b = Bridge(["env", f"KLIP_CONFIG_DIR={cls.b_dir}", "KLIP_CLIPBOARD=memory", KLIP, "api"])
        cls.bridge_a = Bridge(["env", f"KLIP_CONFIG_DIR={cls.a_dir}", KLIP, "api"])
        cls.ctl = Controller(cls.bridge_a, lambda ms, fn: 0, lambda h: None)

    @classmethod
    def start_a(cls):
        env = dict(os.environ, KLIP_CONFIG_DIR=cls.a_dir, PEER_MANUAL="1", PEER_RELAY="lan", PEER_NAME="Ay")
        cls.peer_log = os.path.join(cls.tmp, "peer.log")
        with open(cls.peer_log, "w") as log:
            cls.peer = subprocess.Popen([PEER, cls.work], env=env, stdout=log, stderr=subprocess.STDOUT)

        def ready():
            with open(cls.peer_log) as f:
                return "ready" in f.read()

        wait_for(ready, 30, "the test peer to be ready")
        wait_for(lambda: os.path.exists(os.path.join(cls.a_dir, "daemon.sock")), what="A's control socket")

    @classmethod
    def tearDownClass(cls):
        cls.bridge_a.close()
        cls.bridge_b.close()
        if cls.peer and cls.peer.poll() is None:
            cls.peer.kill()
            cls.peer.wait()
        subprocess.run([KLIP, "stop"], env=cls.env_b, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        shutil.rmtree(cls.tmp, ignore_errors=True)

    # --- helpers
    def b(self, ipc):
        r = self.bridge_b.call({"ipc": ipc})
        self.assertIsNone(r.err, r)
        return r.ok

    def a(self, ipc, timeout=None):
        r = self.bridge_a.call({"ipc": ipc}, timeout)
        self.assertIsNone(r.err, r)
        return r.ok

    def refresh(self):
        self.ctl.refresh()
        wait_for(lambda: not self.ctl._polling, 10, "a poll to finish")

    def inject(self, data, name="inject.png"):
        tmp = os.path.join(self.work, name + ".tmp")
        with open(tmp, "wb") as f:
            f.write(data)
        os.replace(tmp, os.path.join(self.work, name))

    # --- the story, in order (unittest runs methods alphabetically, so the names carry the order)

    def test_01_the_tray_sees_the_daemon_pair_and_connect(self):
        self.refresh()
        self.assertEqual(self.ctl.summary.state, "no_devices")
        self.assertTrue(self.ctl.summary.device_fingerprint)
        ticket = self.b({"PairStart": {"ttl_secs": 120, "include_lan_addrs": True}})["PairTicket"]["ticket"]
        qr = self.bridge_b.call({"method": "qr", "params": {"text": ticket}})
        self.assertGreater(qr.ok["size"], 20)

        result = []
        joiner = threading.Thread(target=lambda: result.append(self.a({"PairJoin": {"ticket": ticket}}, 120)))
        joiner.start()
        pending = wait_for(lambda: self.b("PairPoll")["PairPoll"]["pending"], 30, "B to see the request")
        self.assertEqual(pending["device_name"], "Ay")
        self.assertEqual(self.b({"PairDecide": {"accept": True}})["PairPoll"]["pending"], None)
        joiner.join(60)
        self.assertEqual(result[0]["PairSuccess"]["peer_name"], "Bee")
        wait_for(lambda: (self.refresh() or True) and self.ctl.summary.state == "connected", 30, "A to show connected")
        self.assertEqual(self.ctl.summary.title, "Connected to Bee")
        self.assertEqual(self.ctl.summary.peers[0].name, "Bee")
        self.assertRegex(self.ctl.summary.peers[0].fingerprint, r"^[A-Z2-9]{4}-[A-Z2-9]{4}-[A-Z2-9]{4}$")

    def test_02_an_image_is_held_previewed_and_sent_on_purpose(self):
        data = png()
        self.inject(data)
        wait_for(lambda: (self.refresh() or True) and self.ctl.summary.image, 20, "A to hold the image")
        self.assertTrue(self.ctl.summary.badge, "a held image badges the tray icon")
        thumb = self.a({"GetThumbnail": {"id": None}})["Thumbnail"]
        self.assertTrue(base64.b64decode(thumb).startswith(b"\x89PNG"))
        time.sleep(1.0)
        history_b = self.b({"GetHistory": {"limit": 10}})["History"]
        self.assertEqual([h for h in history_b if h["content_preview"].startswith("[Image")], [],
                         "nothing was sent by itself")
        out = sync(self.ctl.send_pending_image)
        self.assertTrue(out.ok, out)
        self.assertEqual(out.value, "Sent to Bee")
        wait_for(lambda: any(h["content_preview"].startswith("[Image: PNG") for h in
                             self.b({"GetHistory": {"limit": 10}})["History"]), 15, "B to receive the image")
        self.refresh()
        self.assertIsNone(self.ctl.summary.image)
        self.assertFalse(self.ctl.summary.badge)
        again = sync(self.ctl.send_pending_image)
        self.assertFalse(again.ok)
        self.assertIn("no image", again.error)

    def test_03_the_automatic_switch_changes_behaviour_and_is_saved(self):
        out = sync(self.ctl.set_auto_images, True)
        self.assertTrue(out.ok, out)
        self.refresh()
        self.assertTrue(self.ctl.summary.auto_images)
        saved = read_json(os.path.join(self.a_dir, "config.json"))
        self.assertIs(saved["auto_send_images"], True)
        before = len(self.b({"GetHistory": {"limit": 50}})["History"])
        self.inject(png(40, 30))
        wait_for(lambda: len(self.b({"GetHistory": {"limit": 50}})["History"]) > before, 15,
                 "the automatic image to arrive")
        self.assertTrue(sync(self.ctl.set_auto_images, False).ok)
        self.assertIs(read_json(os.path.join(self.a_dir, "config.json"))["auto_send_images"], False)

    def test_04_pause_and_the_daemon_going_away_and_coming_back(self):
        self.assertTrue(sync(self.ctl.pause, 120).ok)
        self.refresh()
        self.assertEqual(self.ctl.summary.state, "paused")
        self.assertTrue(sync(self.ctl.resume).ok)
        self.refresh()
        self.assertEqual(self.ctl.summary.state, "connected")

        # The daemon goes away: the tray says "stopped", and recovers by itself when it is back.
        self.peer.kill()
        self.peer.wait()
        wait_for(lambda: (self.refresh() or True) and self.ctl.summary.state == "stopped", 20, "stopped")
        self.assertEqual(self.ctl.summary.title, "Klip is stopped")
        os.remove(os.path.join(self.a_dir, "daemon.sock")) if os.path.exists(os.path.join(self.a_dir, "daemon.sock")) else None
        type(self).start_a()
        wait_for(lambda: (self.refresh() or True) and self.ctl.summary.state in ("connected", "waiting", "no_devices"),
                 30, "the daemon to be seen again")
        self.assertNotEqual(self.ctl.summary.state, "stopped")

    def test_05_unpairing(self):
        wait_for(lambda: (self.refresh() or True) and self.ctl.summary.peers, 30, "the peer after the restart")
        fp = self.ctl.summary.peers[0].fingerprint
        self.a({"Unpair": {"selector": fp}})
        self.refresh()
        self.assertEqual(self.ctl.summary.state, "no_devices")


if __name__ == "__main__":
    unittest.main()
