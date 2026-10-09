"""macOS: one AXObserver per app on one run-loop thread, feeding everything.

Upstream arc gives every app three subscribers, each with its own thread
polling a run loop every 20-50 ms: the change journal, the settle counter and
(opt-in) the walk cache's change feed. Here one ``AXHub`` owns a single
AXObserver per app on one run-loop thread that sleeps until the app posts
something, and fans each notification out to:

* arc's change journal (structural changes: the freshness guard) and settle
  counter, through drop-in adapters installed with arc's ``app_factory`` hook;
* each watched window's walk-cache feed (``AXNodeCache``), so the next walk of
  that window re-reads only the elements the notifications named;
* the live map, which marks the window dirty and patches it.

The callback is the only code that runs on the run-loop thread and it makes no
accessibility calls: it counts, appends to a queue and returns, inside a
try/except. Role lookups the journal needs (is a created element a sheet or a
row?) run on the hub's dispatch thread, with a short per-element messaging
timeout, so a hung or crashing app can't stall notifications for the others,
and nothing here goes near arc's SkyLight input path.
"""

from __future__ import annotations

import logging
import threading
import time
from collections import deque
from typing import Any, Callable

from . import ELEMENT, GONE, STRUCTURE, WINDOW, AppGone, Observer, WindowGone

log = logging.getLogger("allternit_driver.live.macos")

AX_TIMEOUT_S = 1.0  # Messaging timeout for a watched app (AX default is 6 s).
ROLE_TIMEOUT_S = 0.25

_WINDOW = {
    "AXFocusedWindowChanged", "AXMainWindowChanged", "AXWindowCreated", "AXWindowMoved", "AXWindowResized",
    "AXWindowMiniaturized", "AXWindowDeminiaturized", "AXMenuOpened", "AXMenuClosed", "AXSheetCreated",
    "AXDrawerCreated", "AXApplicationHidden", "AXApplicationShown",
}
_STRUCTURE = {"AXCreated", "AXUIElementDestroyed", "AXLayoutChanged", "AXRowCountChanged", "AXRowExpanded",
              "AXRowCollapsed", "AXResized", "AXMoved", "AXScrollPositionChanged"}
_ELEMENT = {"AXValueChanged", "AXTitleChanged", "AXSelectedChildrenChanged", "AXSelectedRowsChanged",
            "AXSelectedTextChanged", "AXSelectedCellsChanged", "AXFocusedUIElementChanged", "AXElementBusyChanged"}
NOTIFICATIONS = tuple(sorted(_WINDOW | _STRUCTURE | _ELEMENT))
# arc's journal records these (macos_changes.STRUCTURAL); AXCreated/Destroyed only for containers.
_JOURNALED = {"AXWindowCreated", "AXSheetCreated", "AXDrawerCreated", "AXFocusedWindowChanged", "AXMainWindowChanged",
              "AXMenuOpened", "AXMenuClosed", "AXWindowMiniaturized", "AXWindowDeminiaturized",
              "AXApplicationHidden", "AXApplicationShown", "AXCreated", "AXUIElementDestroyed"}
_CONTAINERS = frozenset({"AXWindow", "AXSheet", "AXDrawer", "AXDialog", "AXPopover", "AXMenu"})
_INVALID = (-25202, -25204)  # kAXErrorInvalidUIElement, kAXErrorCannotComplete (app gone or hung)


def kind_of(name: str) -> str:
    return WINDOW if name in _WINDOW else STRUCTURE if name in _STRUCTURE else ELEMENT


class Journal:
    """Drop-in for arc's ``ChangeJournal``, fed by the hub."""

    def __init__(self) -> None:
        from arc_cua.backends.macos_changes import Change

        self._change = Change
        self._cond = threading.Condition()
        self._sequence = 0
        self._changes: deque[Any] = deque(maxlen=256)
        self.available = False

    @property
    def sequence(self) -> int:
        with self._cond:
            return self._sequence

    def since(self, sequence: int) -> list[Any]:
        with self._cond:
            return [c for c in self._changes if c.sequence > sequence]

    def wait_after(self, sequence: int, timeout_s: float) -> bool:
        with self._cond:
            return self._cond.wait_for(lambda: self._sequence > sequence, timeout=max(0.0, timeout_s))

    def record(self, notification: str, role: str) -> None:
        with self._cond:
            self._sequence += 1
            self._changes.append(self._change(self._sequence, notification, role, time.monotonic()))
            self._cond.notify_all()

    def close(self) -> None:
        pass


class Events:
    """Drop-in for arc's ``AXEventMonitor`` (the settle counter), fed by the hub."""

    def __init__(self, hub: "AXHub", pid: int) -> None:
        self._hub, self._pid = hub, pid
        self._lock = threading.Lock()
        self._count = 0

    @property
    def count(self) -> int:
        with self._lock:
            return self._count

    def bump(self) -> None:
        with self._lock:
            self._count += 1

    def watch(self, pid: int, *, timeout_s: float = 0.2) -> bool:
        return self._hub.live(self._pid)

    def watch_element(self, element: Any, notifications: tuple[str, ...]) -> None:
        self._hub.watch_element(self._pid, element, notifications)

    def close(self) -> None:
        pass


class Feed:
    """The walk cache's change feed (``AXNodeCache(feed)``) for one window."""

    def __init__(self, hub: "AXHub", pid: int) -> None:
        self._hub, self._pid = hub, pid
        self._queue: deque[tuple[Any, str]] = deque(maxlen=4096)
        self.overflowed = False

    @property
    def available(self) -> bool:
        return self._hub.live(self._pid) and not self.overflowed

    def push(self, element: Any, name: str) -> None:
        if len(self._queue) == self._queue.maxlen:
            self.overflowed = True  # Too much to patch: the cache starts over (full walk).
        self._queue.append((element, name))

    def drain(self) -> list[tuple[Any, str]]:
        items = []
        while self._queue:
            items.append(self._queue.popleft())
        if self.overflowed:
            self.overflowed = False
            return [(None, "AXFocusedWindowChanged")]  # A window-level change: AXNodeCache resets.
        return items

    def close(self) -> None:
        self._hub.drop_feed(self._pid, self)


class _App:
    """One subscribed app."""

    def __init__(self, hub: "AXHub", pid: int) -> None:
        self.pid = pid
        self.journal = Journal()
        self.events = Events(hub, pid)
        self.feeds: list[Feed] = []
        self.windows: dict[int, Callable[[str], None]] = {}  # window id -> live map notify
        self.observer: Any = None
        self.source: Any = None
        self.callback: Any = None
        self.error: str | None = None
        self.epoch = 0  # Bumped by every window-level notification (window, sheet, menu, focus window).


class AXHub(Observer):
    name = "macos-ax"

    def __init__(self) -> None:
        self._lock = threading.RLock()
        self._apps: dict[int, _App] = {}
        self._loop: Any = None
        self._ready = threading.Event()
        self._queue: deque[tuple[int, Any, str]] = deque()
        self._pending = threading.Event()
        self._closed = False
        threading.Thread(target=self._run_loop, name="ax-hub-loop", daemon=True).start()
        threading.Thread(target=self._dispatch, name="ax-hub-dispatch", daemon=True).start()
        self._ready.wait(2)

    # ---- arc hooks --------------------------------------------------------------

    def app(self, pid: int) -> _App:
        """Subscribe to an app (idempotent); arc's app factory calls this first."""
        with self._lock:
            app = self._apps.get(pid)
            if app is None:
                app = self._apps[pid] = _App(self, pid)
                self._subscribe(app)
            return app

    def epoch(self, pid: int) -> int | None:
        """The app's window-level change counter; None while it isn't observed.
        Equal values mean no window came, went or took focus in between."""
        app = self._apps.get(pid)
        return app.epoch if app is not None and app.observer is not None else None

    def live(self, pid: int) -> bool:
        app = self._apps.get(pid)
        return app is not None and app.observer is not None

    def feed(self, pid: int) -> Feed:
        app = self.app(pid)
        f = Feed(self, pid)
        with self._lock:
            app.feeds.append(f)
        return f

    def drop_feed(self, pid: int, feed: Feed) -> None:
        with self._lock:
            app = self._apps.get(pid)
            if app is not None and feed in app.feeds:
                app.feeds.remove(feed)

    def watch_element(self, pid: int, element: Any, notifications: tuple[str, ...]) -> None:
        """Also subscribe on one element (a web area posts on itself, not the app)."""
        import ApplicationServices as AS  # type: ignore

        app = self._apps.get(pid)
        if app is None or app.observer is None:
            return
        for name in notifications:
            try:
                AS.AXObserverAddNotification(app.observer, element, name, None)
            except Exception:
                pass

    def release(self, pid: int) -> None:
        """arc released the app: drop the observer and the run-loop source."""
        with self._lock:
            app = self._apps.pop(pid, None)
        if app is not None and app.source is not None and self._loop is not None:
            try:
                from CoreFoundation import CFRunLoopRemoveSource, CFRunLoopWakeUp, kCFRunLoopDefaultMode

                CFRunLoopRemoveSource(self._loop, app.source, kCFRunLoopDefaultMode)
                CFRunLoopWakeUp(self._loop)
            except Exception:
                pass

    # ---- Observer ---------------------------------------------------------------

    def subscribe(self, pid: int, window_id: int, notify: Callable[[str], None]) -> str | None:
        app = self.app(pid)
        with self._lock:
            app.windows[window_id] = notify
        return app.error

    def unsubscribe(self, pid: int, window_id: int) -> None:
        with self._lock:
            app = self._apps.get(pid)
            if app is not None:
                app.windows.pop(window_id, None)

    def probe(self, pid: int, window_id: int) -> Any:
        """(window child count, focused element hash): two AX calls."""
        import ApplicationServices as AS  # type: ignore
        from arc_cua.backends.macos_ax import _core_foundation
        from arc_cua.backends.macos_app import find_ax_window, window_server_info

        app_ref = AS.AXUIElementCreateApplication(pid)
        AS.AXUIElementSetMessagingTimeout(app_ref, AX_TIMEOUT_S)
        window = find_ax_window(app_ref, window_id)
        if window is None:
            err, _ = AS.AXUIElementCopyAttributeValue(app_ref, "AXRole", None)
            if err in _INVALID:
                raise AppGone(f"pid {pid} quit")
            if window_server_info(window_id) is None:
                raise WindowGone(f"window {window_id} closed")
            return None  # A window accessibility doesn't list (the desktop): events only.
        err, count = AS.AXUIElementGetAttributeValueCount(window, "AXChildren", None)
        err2, focused = AS.AXUIElementCopyAttributeValue(app_ref, "AXFocusedUIElement", None)
        if err in _INVALID:
            raise AppGone(f"pid {pid} stopped answering")
        focus = int(_core_foundation().CFHash(focused)) if err2 == 0 and focused is not None else None
        return (count if err == 0 else None, focus)

    def close(self) -> None:
        self._closed = True
        self._pending.set()
        if self._loop is not None:
            from CoreFoundation import CFRunLoopStop

            CFRunLoopStop(self._loop)

    # ---- internals --------------------------------------------------------------------

    def _subscribe(self, app: _App) -> None:
        import ApplicationServices as AS  # type: ignore
        import objc  # type: ignore
        from CoreFoundation import CFRunLoopAddSource, CFRunLoopWakeUp, kCFRunLoopDefaultMode

        pid, queue, pending = app.pid, self._queue, self._pending

        @objc.callbackFor(AS.AXObserverCreate)
        def callback(observer: Any, element: Any, notification: Any, refcon: Any) -> None:
            # Runs on the hub's run loop. No accessibility calls, no locks held long,
            # nothing that can raise out: count, enqueue, return.
            try:
                app.events.bump()
                if str(notification) in _WINDOW:
                    app.epoch += 1
                queue.append((pid, element, str(notification)))
                pending.set()
            except BaseException:
                pass

        try:
            err, observer = AS.AXObserverCreate(pid, callback, None)
            if err != 0 or observer is None:
                app.error = f"AXObserverCreate failed ({err})"
                return
            ref = AS.AXUIElementCreateApplication(pid)
            AS.AXUIElementSetMessagingTimeout(ref, AX_TIMEOUT_S)
            ok = sum(AS.AXObserverAddNotification(observer, ref, name, None) == 0 for name in NOTIFICATIONS)
            if not ok:
                app.error = "the app accepts no accessibility notifications"
                return
            app.observer, app.callback = observer, callback
            app.source = AS.AXObserverGetRunLoopSource(observer)
            app.journal.available = True
            if self._loop is None:
                app.error = "notification run loop not running"
                return
            CFRunLoopAddSource(self._loop, app.source, kCFRunLoopDefaultMode)
            CFRunLoopWakeUp(self._loop)
        except Exception as e:
            app.error = f"observer setup failed: {e}"

    def _run_loop(self) -> None:
        from CoreFoundation import CFRunLoopGetCurrent, CFRunLoopRunInMode, kCFRunLoopDefaultMode

        self._loop = CFRunLoopGetCurrent()
        self._ready.set()
        while not self._closed:
            # Sleeps in the kernel until an app posts (or 60 s pass); returns at
            # once while no app is subscribed, so wait for one first.
            if not any(a.source is not None for a in list(self._apps.values())):
                time.sleep(0.25)
                continue
            try:
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 60.0, False)
            except Exception as e:  # Never let a callback problem end the loop.
                log.debug("run loop: %s", e)

    def _dispatch(self) -> None:
        while not self._closed:
            self._pending.wait()
            self._pending.clear()
            while self._queue:
                pid, element, name = self._queue.popleft()
                try:
                    self._deliver(pid, element, name)
                except Exception as e:
                    log.debug("deliver %s pid=%s: %s", name, pid, e)

    def _deliver(self, pid: int, element: Any, name: str) -> None:
        app = self._apps.get(pid)
        if app is None:
            return
        with self._lock:
            feeds = list(app.feeds)
            windows = list(app.windows.values())
        for f in feeds:
            f.push(element, name)
        if name in _JOURNALED:
            role = ""
            if name in ("AXCreated", "AXUIElementDestroyed"):
                role = _role(element)
                if role not in _CONTAINERS:
                    role = None  # A row or a web node, not a structural change for the journal.
            if role is not None:
                app.journal.record(name, role)
        kind = kind_of(name)
        if name == "AXUIElementDestroyed" and _role(element) == "AXWindow":
            kind = GONE
        for notify in windows:
            notify(kind)


def _role(element: Any) -> str:
    if element is None:
        return ""
    try:
        import ApplicationServices as AS  # type: ignore

        AS.AXUIElementSetMessagingTimeout(element, ROLE_TIMEOUT_S)
        err, value = AS.AXUIElementCopyAttributeValue(element, "AXRole", None)
        return str(value) if err == 0 and value is not None else ""
    except Exception:
        return ""


def app_factory(hub: AXHub) -> Callable[[int], Any]:
    """arc's per-app object, with its journal and settle counter on the hub and
    every window backend's walk cache fed by it."""
    from arc_cua.backends.macos_app import MacOSApp
    from arc_cua.backends.macos_ax import MacOSAXBackend
    from arc_cua.backends.macos_ax_cache import AXNodeCache
    from arc_cua.driver import _App

    class LiveApp(_App):
        def __init__(self, pid: int) -> None:
            sub = hub.app(pid)  # Subscribe first: nothing during the first walk is missed.
            self.journal, self.events = sub.journal, sub.events
            self._pid = pid
            try:
                self.app = MacOSApp(pid)
                self._backend_class = MacOSAXBackend
                self._resolver = MacOSAXBackend(pid, app=self.app)
                self._resolver.open()
            except BaseException:
                hub.release(pid)
                raise
            self._backends: dict[int, Any] = {}
            self._web_areas: set[tuple[int, str]] = set()

        def backend(self, window_id: int) -> Any:
            backend = self._backends.get(window_id)
            if backend is None:
                backend = self._backends[window_id] = MacOSAXBackend(self.app.pid, app=self.app)
                backend._cache = AXNodeCache(hub.feed(self.app.pid))
            return backend

        def close(self) -> None:
            for backend in (*self._backends.values(), self._resolver):
                try:
                    backend.close()
                except Exception:
                    pass
            hub.release(self._pid)

    return LiveApp
