"""The logic between the bridge and the window: polling, actions, one consistent picture.

No GTK in here. Time (`schedule`/`cancel`) and the clock are passed in, so everything is testable.
"""
from __future__ import annotations

import base64
import logging
import time
from typing import Any, Callable, NamedTuple, Optional

from . import model
from .bridge import Reply

log = logging.getLogger("klip_tray")

FAST_MS = 3_000  # while the window is open
SLOW_MS = 15_000  # in the background
JOIN_TIMEOUT = 120.0  # joining waits for the other device's person to approve
DEFAULT_SEND_LIMIT = 8 * 1024 * 1024 - 65_536  # what one bridge request can carry


class Outcome(NamedTuple):
    value: Any = None
    error: Optional[str] = None
    down: bool = False

    @property
    def ok(self) -> bool:
        return self.error is None


Callback = Optional[Callable[[Outcome], None]]


class Controller:
    def __init__(self, bridge, schedule, cancel, now_ms: Callable[[], int] = lambda: int(time.time() * 1000)):
        self.bridge = bridge
        self._schedule = schedule
        self._cancel = cancel
        self._now_ms = now_ms
        self.listeners: list = []
        self.summary = model.summarize(None, None, down=True)
        self.status = None
        self.images = None
        self.hello = None
        self.caps: Optional[list] = None
        self.last_error: Optional[str] = None
        self._fast = False
        self._timer = None
        self._polling = False
        self._again = False

    # ------------------------------------------------------------------------------- plumbing

    def _notify(self) -> None:
        for listener in list(self.listeners):
            try:
                listener(self.summary)
            except Exception:  # a broken listener must not stop the others or the polling
                log.exception("a listener failed")

    def _finish(self, cb: Callback, outcome: Outcome) -> None:
        if cb is not None:
            try:
                cb(outcome)
            except Exception:
                log.exception("a callback failed")

    def _ipc(self, request: Any, cb: Callback, expect: Optional[str] = None, parse=None,
             timeout: Optional[float] = None) -> None:
        def handler(reply: Reply) -> None:
            if reply.err is not None:
                self._finish(cb, Outcome(error=reply.err, down=reply.down))
                return
            name, payload = model.variant(reply.ok)
            if name == "Error":
                self._finish(cb, Outcome(error=str(payload)))
            elif expect is not None and name != expect:
                self._finish(cb, Outcome(error=f"Klip gave an unexpected answer ({name})"))
            else:
                try:
                    value = parse(payload) if parse else payload
                except Exception as e:  # a reply of an unexpected shape
                    self._finish(cb, Outcome(error=f"Klip gave an unreadable answer: {e}"))
                    return
                if isinstance(value, Outcome):
                    self._finish(cb, value)
                else:
                    self._finish(cb, Outcome(value=value))

        self.bridge.request({"ipc": request}, handler, timeout)

    def _method(self, name: str, params: dict, cb: Callback, timeout: Optional[float] = None) -> None:
        def handler(reply: Reply) -> None:
            if reply.err is not None:
                self._finish(cb, Outcome(error=reply.err, down=reply.down))
            else:
                self._finish(cb, Outcome(value=reply.ok))

        self.bridge.request({"method": name, "params": params}, handler, timeout)

    def _then_refresh(self, cb: Callback, new_daemon: bool = False) -> Callable[[Outcome], None]:
        def done(outcome: Outcome) -> None:
            if new_daemon:
                self.caps = None  # a started or restarted daemon may be a newer one: ask what it can do
            self._finish(cb, outcome)
            self.refresh()

        return done

    # --------------------------------------------------------------------------------- polling

    def refresh(self) -> None:
        if self._polling:
            self._again = True
            return
        self._polling = True
        self._again = False
        results: dict = {}
        ask_hello = self.caps is None  # decided once: the answers below may change `caps`
        need = 2 + (1 if ask_hello else 0)

        def got(key: str):
            def f(outcome: Outcome) -> None:
                results[key] = outcome
                if len(results) == need:
                    self._finish_poll(results)

            return f

        self._ipc("GetStatus", got("status"), "Status")
        self._ipc("GetImageState", got("images"), "ImageState")
        if ask_hello:
            self._ipc({"Hello": {"client_protocol": 2}}, got("hello"), "Hello")

    def _finish_poll(self, results: dict) -> None:
        try:
            self._apply_poll(results)
        except Exception:  # whatever went wrong, the next poll must still run
            log.exception("could not build the picture from the daemon's answers")
            self._polling = False
            self._schedule_next()

    def _apply_poll(self, results: dict) -> None:
        status: Outcome = results["status"]
        down = not status.ok
        if down:
            self.caps = None
            self.status = None
            self.images = None
            self.hello = None
            self.last_error = status.error
        else:
            self.last_error = None
            self.status = status.value
            images = results["images"]
            self.images = images.value if images.ok else None
            if "hello" in results:
                hello = results["hello"]
                if hello.ok:  # a failed answer is "unknown" (asked again), not "an old daemon"
                    self.hello = hello.value
                    caps = (hello.value or {}).get("capabilities")
                    self.caps = list(caps) if isinstance(caps, list) else []
        summary = model.summarize(self.status, self.images, down=down, caps=self.caps)
        self._polling = False
        changed = summary != self.summary
        self.summary = summary
        if changed:
            self._notify()
        self._schedule_next()
        if self._again:
            self.refresh()

    def _schedule_next(self) -> None:
        if self._timer is not None:
            self._cancel(self._timer)
        self._timer = self._schedule(FAST_MS if self._fast else SLOW_MS, self._fired)

    def _fired(self) -> None:
        self._timer = None
        self.refresh()

    def set_fast(self, fast: bool) -> None:
        if fast == self._fast:
            return
        self._fast = fast
        if self._timer is not None:
            self._schedule_next()
        if fast:
            self.refresh()

    def stop(self) -> None:
        if self._timer is not None:
            self._cancel(self._timer)
            self._timer = None

    # ------------------------------------------------------------------------ images and text

    def send_pending_image(self, cb: Callback = None) -> None:
        self._ipc("SendPendingImage", self._then_refresh(cb), "Sent", parse=_sent_outcome)

    def set_auto_images(self, enabled: bool, cb: Callback = None) -> None:
        self._ipc({"SetAutoImages": {"enabled": bool(enabled)}}, self._then_refresh(cb), "ImageState")

    def thumbnail(self, id_hex: Optional[str], cb: Callback) -> None:
        try:
            ident = None if id_hex is None else _id_list(id_hex)
        except ValueError as e:
            self._finish(cb, Outcome(error=str(e)))
            return
        self._ipc({"GetThumbnail": {"id": ident}}, cb, "Thumbnail", parse=base64.b64decode)

    def send_text(self, text: str, cb: Callback = None) -> None:
        limit = DEFAULT_SEND_LIMIT
        if isinstance(self.hello, dict):
            configured = (self.hello.get("config") or {}).get("max_clip_bytes")
            if isinstance(configured, int) and configured > 0:
                limit = min(configured, DEFAULT_SEND_LIMIT)
        problem = model.send_problem(text, limit)
        if problem:
            self._finish(cb, Outcome(error=problem))
            return
        self._ipc({"SendClip": {"text": text}}, self._then_refresh(cb), "Sent", parse=_sent_outcome)

    # --------------------------------------------------------------------------------- history

    def history(self, cb: Callback) -> None:
        self._ipc({"GetHistory": {"limit": 200}}, cb, "History",
                  parse=lambda items: model.history_rows(items, self._now_ms()))

    def history_text(self, id_hex: str, cb: Callback) -> None:
        self._method("history_text", {"id_hex": id_hex}, lambda o: self._finish(
            cb, Outcome(error=o.error, down=o.down) if not o.ok else Outcome(value=o.value.get("text", ""))))

    def copy_back(self, id_hex: str, cb: Callback = None) -> None:
        try:
            ident = _id_list(id_hex)
        except ValueError as e:
            self._finish(cb, Outcome(error=str(e)))
            return
        self._ipc({"CopyToClipboard": {"id": ident}}, cb, "Copied")

    def clear_history(self, cb: Callback = None) -> None:
        self._ipc("ClearHistory", self._then_refresh(cb), "HistoryCleared")

    # ----------------------------------------------------------------- devices, pause, pairing

    def pause(self, secs: int, cb: Callback = None) -> None:
        self._ipc({"Pause": {"duration_secs": int(secs)}}, self._then_refresh(cb), "Paused")

    def resume(self, cb: Callback = None) -> None:
        self._ipc("Resume", self._then_refresh(cb), "Resumed")

    def unpair(self, selector: str, cb: Callback = None) -> None:
        self._ipc({"Unpair": {"selector": selector}}, self._then_refresh(cb), "Unpaired")

    def unpair_all(self, cb: Callback = None) -> None:
        self._ipc("UnpairAll", self._then_refresh(cb), "UnpairedAll")

    def pair_start(self, cb: Callback, lan: bool = False, ttl: int = 120) -> None:
        self._ipc({"PairStart": {"ttl_secs": int(ttl), "include_lan_addrs": bool(lan)}}, cb, "PairTicket")

    def pair_poll(self, cb: Callback) -> None:
        self._ipc("PairPoll", cb, "PairPoll")

    def pair_decide(self, accept: bool, cb: Callback = None) -> None:
        self._ipc({"PairDecide": {"accept": bool(accept)}}, self._then_refresh(cb), "PairPoll")

    def pair_cancel(self, cb: Callback = None) -> None:
        self._ipc("PairCancel", cb, "PairCancelled")

    def pair_join(self, ticket: str, cb: Callback) -> None:
        if not model.looks_like_ticket(ticket):
            self._finish(cb, Outcome(error="That does not look like a Klip ticket (it starts with klip1)."))
            return
        self._ipc({"PairJoin": {"ticket": model.clean_ticket(ticket)}}, self._then_refresh(cb), "PairSuccess",
                  timeout=JOIN_TIMEOUT)

    def qr(self, text: str, cb: Callback) -> None:
        self._method("qr", {"text": text}, cb)

    # ------------------------------------------------------------------ the daemon and its files

    def stop_daemon(self, cb: Callback = None) -> None:
        self._ipc("StopDaemon", self._then_refresh(cb, True), "DaemonStopping")

    def daemon_start(self, cb: Callback = None) -> None:
        self._method("daemon_start", {}, self._then_refresh(cb, True), timeout=45.0)

    def daemon_restart(self, cb: Callback = None) -> None:
        self._method("daemon_restart", {}, self._then_refresh(cb, True), timeout=60.0)

    def config_get(self, cb: Callback) -> None:
        self._method("config_get", {}, cb)

    def config_set(self, config: dict, cb: Callback = None) -> None:
        problems = model.config_problems(config)
        if problems:
            self._finish(cb, Outcome(error=" ".join(problems)))
            return
        self._method("config_set", {"config": config}, cb)

    def service_state(self, cb: Callback) -> None:
        self._method("service_state", {}, cb)

    def service_set(self, enabled: bool, cb: Callback = None) -> None:
        self._method("service_set", {"enabled": bool(enabled)}, cb)

    def autostart_state(self, cb: Callback) -> None:
        self._method("autostart_get", {}, cb)

    def autostart_set(self, enabled: bool, cb: Callback = None) -> None:
        self._method("autostart_set", {"enabled": bool(enabled)}, cb)

    def log_tail(self, offset: Optional[int], cb: Callback) -> None:
        self._method("log_tail", {"offset": offset}, cb)


def _id_list(id_hex: str) -> list:
    if not isinstance(id_hex, str) or len(id_hex) != 32:
        raise ValueError("that clip has no usable id")
    try:
        return list(bytes.fromhex(id_hex))
    except ValueError:
        raise ValueError("that clip has no usable id") from None


def _sent_outcome(payload: Any) -> Outcome:
    ok, message = model.describe_sent(payload)
    return Outcome(value=message) if ok else Outcome(error=message)
