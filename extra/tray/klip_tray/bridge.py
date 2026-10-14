"""A client for `klip api`: one long-lived child process, JSON lines both ways.

Thread-safe, never blocks the caller (except `call`, meant for worker threads and tests), restarts the
child if it dies, and gives every request an answer: a result, an error, or a timeout.
"""
from __future__ import annotations

import json
import subprocess
import threading
import time
from typing import Any, Callable, NamedTuple, Optional


class Reply(NamedTuple):
    ok: Any = None
    err: Optional[str] = None
    down: bool = False  # the daemon is not running (not the same as "something went wrong")


class Bridge:
    def __init__(self, command, post: Optional[Callable[[Callable], None]] = None, timeout: float = 20.0):
        self._command = list(command)
        self._post = post or (lambda fn: fn())
        self._timeout = timeout
        self._cond = threading.Condition()
        self._proc: Optional[subprocess.Popen] = None
        self._pending: dict = {}  # id -> (callback, deadline, proc)
        self._next_id = 0
        self._closed = False
        self._watchdog: Optional[threading.Thread] = None

    # ------------------------------------------------------------------------------ public

    def request(self, payload: dict, callback: Optional[Callable[[Reply], None]] = None,
                timeout: Optional[float] = None) -> None:
        """Send `payload` (without an id); `callback(Reply)` is run through the `post` function."""
        failure = None
        with self._cond:
            if self._closed:
                failure = "the Klip bridge is closed"
            else:
                failure = self._send_locked(payload, callback, timeout if timeout is not None else self._timeout)
        if failure:
            self._deliver(callback, Reply(err=failure))

    def call(self, payload: dict, timeout: Optional[float] = None) -> Reply:
        """Like `request`, but waits for the answer. Do not use it on the GTK main thread."""
        done = threading.Event()
        box: list = []

        def finish(reply: Reply) -> None:
            box.append(reply)
            done.set()

        saved = self._post
        self._post = lambda fn: fn()  # a blocking call must not wait for the main loop
        try:
            self.request(payload, finish, timeout)
        finally:
            self._post = saved
        done.wait()
        return box[0]

    def close(self) -> None:
        with self._cond:
            self._closed = True
            proc, self._proc = self._proc, None
            pending, self._pending = self._pending, {}
            self._cond.notify_all()
        for callback, _deadline, _proc in pending.values():
            self._deliver(callback, Reply(err="the Klip bridge is closed"))
        if proc is not None:
            try:
                proc.stdin.close()
            except OSError:
                pass
            try:
                proc.wait(timeout=1.0)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()
            self._close_pipes(proc)

    # ----------------------------------------------------------------------------- internals

    @staticmethod
    def _close_pipes(proc: subprocess.Popen) -> None:
        for pipe in (proc.stdin, proc.stdout):
            try:
                if pipe is not None:
                    pipe.close()
            except (OSError, ValueError):
                pass

    def _deliver(self, callback, reply: Reply) -> None:
        if callback is not None:
            self._post(lambda: callback(reply))

    def _start_locked(self) -> Optional[str]:
        if self._proc is not None and self._proc.poll() is None:
            return None
        try:
            proc = subprocess.Popen(self._command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                    stderr=subprocess.DEVNULL, text=True, encoding="utf-8",
                                    errors="replace", bufsize=1)
        except OSError as e:
            return f"cannot start {self._command[0]}: {e}"
        self._proc = proc
        threading.Thread(target=self._read_loop, args=(proc,), name="klip-bridge-read", daemon=True).start()
        if self._watchdog is None:
            self._watchdog = threading.Thread(target=self._watch, name="klip-bridge-watch", daemon=True)
            self._watchdog.start()
        return None

    def _send_locked(self, payload: dict, callback, timeout: float) -> Optional[str]:
        for attempt in (1, 2):
            failure = self._start_locked()
            if failure:
                return failure
            proc = self._proc
            self._next_id += 1
            rid = self._next_id
            line = json.dumps(dict(payload, id=rid), ensure_ascii=False)
            try:
                proc.stdin.write(line + "\n")
                proc.stdin.flush()
            except (OSError, ValueError):
                self._proc = None  # it died under us: start a fresh one and try once more
                try:
                    proc.kill()
                except OSError:
                    pass
                if attempt == 2:
                    return "the Klip bridge stopped unexpectedly"
                continue
            self._pending[rid] = (callback, time.monotonic() + timeout, proc)
            self._cond.notify_all()
            return None
        return None

    def _read_loop(self, proc: subprocess.Popen) -> None:
        try:
            for line in proc.stdout:
                try:
                    obj = json.loads(line)
                except ValueError:
                    continue
                if not isinstance(obj, dict) or "id" not in obj:
                    continue
                with self._cond:
                    entry = self._pending.pop(obj["id"], None)
                if entry is not None:
                    reply = Reply(ok=obj.get("ok"), err=obj.get("err"), down=bool(obj.get("down")))
                    self._deliver(entry[0], reply)
        except (OSError, ValueError):
            pass
        with self._cond:
            if self._proc is proc:
                self._proc = None
            lost = [rid for rid, (_cb, _dl, owner) in self._pending.items() if owner is proc]
            entries = [self._pending.pop(rid) for rid in lost]
        try:
            proc.wait(timeout=0.5)
        except subprocess.TimeoutExpired:
            pass
        self._close_pipes(proc)
        for callback, _deadline, _owner in entries:
            self._deliver(callback, Reply(err="the Klip bridge stopped unexpectedly"))

    def _watch(self) -> None:
        """Answers for requests whose time ran out (the child may be stuck, or the answer lost)."""
        while True:
            with self._cond:
                if self._closed:
                    return
                now = time.monotonic()
                late = [rid for rid, (_cb, deadline, _p) in self._pending.items() if deadline <= now]
                entries = [self._pending.pop(rid) for rid in late]
                if not entries:
                    soonest = min((d for _cb, d, _p in self._pending.values()), default=None)
                    self._cond.wait(None if soonest is None else max(0.01, soonest - now))
            for callback, _deadline, _owner in entries:
                self._deliver(callback, Reply(err="Klip did not answer in time"))
