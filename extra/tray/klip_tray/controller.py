"""The logic between the bridge and the menu: polling, actions, one consistent picture.

No GTK in here. Time (`schedule`/`cancel`) is passed in, so everything is testable.
"""
from __future__ import annotations

import logging
from typing import Any, Callable, NamedTuple, Optional

from . import model
from .bridge import Reply

log = logging.getLogger("klip_tray")

POLL_MS = 15_000


class Outcome(NamedTuple):
    value: Any = None
    error: Optional[str] = None
    down: bool = False

    @property
    def ok(self) -> bool:
        return self.error is None


Callback = Optional[Callable[[Outcome], None]]


class Controller:
    def __init__(self, bridge, schedule, cancel):
        self.bridge = bridge
        self._schedule = schedule
        self._cancel = cancel
        self.listeners: list = []
        self.summary = model.summarize(None, None, down=True)
        self.status = None
        self.images = None
        self.hello = None
        self.caps: Optional[list] = None
        self.last_error: Optional[str] = None
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
        self._timer = self._schedule(POLL_MS, self._fired)

    def _fired(self) -> None:
        self._timer = None
        self.refresh()

    def stop(self) -> None:
        if self._timer is not None:
            self._cancel(self._timer)
            self._timer = None

    # ---------------------------------------------------------------------------------- actions

    def send_pending_image(self, cb: Callback = None) -> None:
        self._ipc("SendPendingImage", self._then_refresh(cb), "Sent", parse=_sent_outcome)

    def set_auto_images(self, enabled: bool, cb: Callback = None) -> None:
        self._ipc({"SetAutoImages": {"enabled": bool(enabled)}}, self._then_refresh(cb), "ImageState")

    def pause(self, secs: int, cb: Callback = None) -> None:
        self._ipc({"Pause": {"duration_secs": int(secs)}}, self._then_refresh(cb), "Paused")

    def resume(self, cb: Callback = None) -> None:
        self._ipc("Resume", self._then_refresh(cb), "Resumed")

    def daemon_start(self, cb: Callback = None) -> None:
        self._method("daemon_start", {}, self._then_refresh(cb, True), timeout=45.0)


def _sent_outcome(payload: Any) -> Outcome:
    ok, message = model.describe_sent(payload)
    return Outcome(value=message) if ok else Outcome(error=message)
