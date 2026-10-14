import json
import os
import sys
import tempfile
import textwrap
import threading
import time
import unittest

from klip_tray.bridge import Bridge

FAKE = textwrap.dedent('''
    import json, os, sys, threading, time
    out = threading.Lock()

    def say(obj):
        with out:
            print(json.dumps(obj), flush=True)

    def handle(req):
        m = req.get("method")
        if m == "slow":
            time.sleep(float(req.get("params", {}).get("secs", 1)))
        if m == "die":
            os._exit(3)
        if m == "garbage":
            with out:
                print("this is not json", flush=True)
                print("[1, 2, 3]", flush=True)
        if m == "down":
            say({"id": req["id"], "err": "Klip daemon is not running.", "down": True})
            return
        say({"id": req["id"], "ok": {"echo": m, "pid": os.getpid()}})

    for line in sys.stdin:
        try:
            req = json.loads(line)
        except ValueError:
            continue
        threading.Thread(target=handle, args=(req,), daemon=True).start()
''')



class BridgeTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        self.script = os.path.join(self.dir, "fake_klip_api.py")
        with open(self.script, "w") as f:
            f.write(FAKE)
        self.bridge = Bridge([sys.executable, self.script], timeout=5.0)
        self.addCleanup(self.bridge.close)

    def test_a_call_round_trips(self):
        r = self.bridge.call({"method": "hello"})
        self.assertEqual(r.ok["echo"], "hello")
        self.assertIsNone(r.err)
        self.assertFalse(r.down)

    def test_a_slow_request_does_not_block_a_fast_one(self):
        results = []
        done = threading.Event()

        def finish(name):
            def cb(reply):
                results.append((name, reply))
                if len(results) == 2:
                    done.set()
            return cb

        self.bridge.request({"method": "slow", "params": {"secs": 0.8}}, finish("slow"))
        self.bridge.request({"method": "fast"}, finish("fast"))
        self.assertTrue(done.wait(5))
        self.assertEqual([n for n, _ in results], ["fast", "slow"])

    def test_answers_are_handed_to_the_posting_function(self):
        queue = []
        b = Bridge([sys.executable, self.script], post=queue.append, timeout=5.0)
        self.addCleanup(b.close)
        got = []
        b.request({"method": "x"}, got.append)
        deadline = time.time() + 5
        while not queue and time.time() < deadline:
            time.sleep(0.01)
        self.assertEqual(got, [], "nothing runs until the main loop runs it")
        queue.pop(0)()
        self.assertEqual(got[0].ok["echo"], "x")

    def test_a_request_that_gets_no_answer_times_out_and_a_late_answer_is_ignored(self):
        t = time.time()
        r = self.bridge.call({"method": "slow", "params": {"secs": 1.5}}, timeout=0.3)
        self.assertLess(time.time() - t, 1.2)
        self.assertIn("did not answer", r.err)
        time.sleep(1.5)  # the late answer arrives and must not break anything
        self.assertEqual(self.bridge.call({"method": "again"}).ok["echo"], "again")

    def test_when_the_child_dies_waiting_requests_fail_and_the_next_one_restarts_it(self):
        first = self.bridge.call({"method": "pid"}).ok["pid"]
        r = self.bridge.call({"method": "die"})
        self.assertIsNotNone(r.err)
        self.assertFalse(r.down)
        second = self.bridge.call({"method": "pid"}).ok["pid"]
        self.assertNotEqual(first, second)

    def test_lines_that_are_not_answers_are_ignored(self):
        r = self.bridge.call({"method": "garbage"})
        self.assertEqual(r.ok["echo"], "garbage")

    def test_a_missing_program_is_an_error_not_an_exception(self):
        b = Bridge(["/nonexistent/klip", "api"], timeout=1.0)
        self.addCleanup(b.close)
        r = b.call({"method": "x"})
        self.assertIn("cannot start", r.err)

    def test_the_daemon_being_down_is_carried_through(self):
        r = self.bridge.call({"method": "down"})
        self.assertTrue(r.down)
        self.assertIn("not running", r.err)

    def test_close_stops_the_child_and_later_calls_fail_cleanly(self):
        pid = self.bridge.call({"method": "pid"}).ok["pid"]
        self.bridge.close()
        time.sleep(0.3)
        with self.assertRaises(ProcessLookupError):
            os.kill(pid, 0)
        self.assertIsNotNone(self.bridge.call({"method": "x"}).err)

    def test_many_requests_from_many_threads_all_get_their_own_answer(self):
        out = {}

        def worker(i):
            out[i] = self.bridge.call({"method": f"m{i}"}).ok["echo"]

        threads = [threading.Thread(target=worker, args=(i,)) for i in range(40)]
        [t.start() for t in threads]
        [t.join(10) for t in threads]
        self.assertEqual(out, {i: f"m{i}" for i in range(40)})


if __name__ == "__main__":
    unittest.main()
