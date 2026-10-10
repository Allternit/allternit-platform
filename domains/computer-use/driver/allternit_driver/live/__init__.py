"""The live element map: one observer subscription per watched window.

Instead of walking a window's accessibility tree on every ``read_ui``, the
driver subscribes to the OS's accessibility notifications for each window it
reads (AX observers on macOS, AT-SPI events on Linux, UIA events on Windows)
and keeps that window's element map current:

* A notification marks the window dirty and wakes the patch worker, which
  patches the map (macOS: arc's incremental walk re-reads only the elements
  the notifications named) and bumps its version when anything visible changed.
* A read of a clean window answers from the map with no accessibility call at
  all (the "~0 ms re-read"). A read of a dirty window patches first, so a read
  never returns a map older than the last notification.
* A full walk happens only on first sight, on a window-level change (window,
  sheet, menu, focus window), when a burst of structural changes passes
  ``LARGE_CHANGE``, or when the staleness probe (root child count + focused
  element, every ``PROBE_S``) sees a change no notification announced.
* An app that announces nothing (observer failed, or the probe caught it
  twice) is marked degraded: its reads become timed narrow reads (the map is
  reused for at most ``DEGRADED_TTL_S``) and every answer says so.

Lifecycle: windows are watched on demand (first read or act), unwatched after
``TTL_S`` without use, at most ``MAX_WINDOWS`` at once (least recently used
goes first), each map keeps ``element_map.HISTORY`` versions, and an app that
quits drops its watches without touching the sidecar.

Observers implement ``Observer``. Their callbacks only enqueue: all
accessibility I/O happens on the patch worker or the reader's own thread, and
every observer error is contained here, so one misbehaving app degrades its
own windows and never the sidecar.
"""

from __future__ import annotations

import logging
import threading
import time
from dataclasses import dataclass, field
from typing import Any, Callable

log = logging.getLogger("allternit_driver.live")

TTL_S = 300.0  # Unwatch a window this long after its last read or act.
MAX_WINDOWS = 8  # Watched windows at once; the least recently used goes first.
PROBE_S = 2.0  # Staleness probe period per watched window.
EAGER_S = 60.0  # Patch in the background while a caller read the window this recently; lazily (on read) after.
EAGER_MIN_S = 0.1  # At most one background patch per window this often (a busy app's bursts coalesce).
LARGE_CHANGE = 40  # Structural notifications in one burst that make a full walk cheaper than a patch.
DEGRADED_TTL_S = 0.5  # A degraded window's map is reused for at most this long.
DEGRADED_MAX_ELEMENTS = 200  # Narrow reads for degraded windows.
MISSES_TO_DEGRADE = 2  # Unannounced changes caught by the probe before a window counts as degraded.

ELEMENT, STRUCTURE, WINDOW, GONE = "element", "structure", "window", "gone"


class AppGone(LookupError):
    """The watched app quit (or stopped answering): all its windows go."""


class WindowGone(LookupError):
    """One watched window closed."""


class Observer:
    """One OS's accessibility notifications. ``subscribe`` returns None when
    the window is live, or the reason it can only be read on a timer."""

    name = "none"

    def subscribe(self, pid: int, window_id: int, notify: Callable[[str], None]) -> str | None:
        return "no accessibility notifications on this OS"

    def unsubscribe(self, pid: int, window_id: int) -> None:
        pass

    def probe(self, pid: int, window_id: int) -> Any:
        """A cheap signature of the window (root child count, focused element),
        compared between probes; None when it can't be taken. Raises
        ``AppGone`` or ``WindowGone``."""
        return None

    def close(self) -> None:
        pass


@dataclass
class Watch:
    key: str
    pid: int
    window_id: int
    since: float = field(default_factory=time.monotonic)
    used: float = field(default_factory=time.monotonic)
    asked: float = 0.0  # Last read or act by a caller (prewarm doesn't count): background patching follows it.
    dirty: bool = False
    burst: int = 0  # Structural notifications since the last patch.
    full: bool = False  # The next refresh must be a full walk.
    degraded: str | None = None
    notifications: int = 0
    patches: int = 0
    walks: int = 0
    misses: int = 0
    probe_sig: Any = None
    probed: float = 0.0
    refreshed: float = 0.0  # When the map was last brought up to date.


class LiveMaps:
    """Watches, their freshness, and the worker that patches them.

    ``refresh(key, full)`` re-reads one window into its element map (the
    driver holds the reader lock and records the version); ``forget(key)``
    drops a window's map and engine state when it is unwatched."""

    def __init__(self, observer: Observer, refresh: Callable[[str, bool], None], forget: Callable[[str], None]) -> None:
        self.observer = observer
        self._refresh = refresh
        self._forget = forget
        self._lock = threading.Lock()
        self._wake = threading.Event()
        self._watches: dict[str, Watch] = {}
        self._closed = False
        self._worker = threading.Thread(target=self._run, name="live-map", daemon=True)
        self._worker.start()

    # ---- watching --------------------------------------------------------------

    def ensure(self, key: str, pid: int, window_id: int, asked: bool = True) -> Watch:
        """Watch a window (on its first read or act) and mark it used.
        ``asked=False`` is the driver's own prewarm: watched, but not patched in
        the background until a caller reads it."""
        now = time.monotonic()
        with self._lock:
            w = self._watches.get(key)
            if w is not None:
                w.used = now
                if asked:
                    w.asked = now
                return w
            w = self._watches[key] = Watch(key, pid, window_id, full=True, asked=now if asked else 0.0)
            evict = sorted(self._watches.values(), key=lambda x: x.used)[: max(0, len(self._watches) - MAX_WINDOWS)]
        for old in evict:
            self.drop(old.key, "evicted")
        try:
            w.degraded = self.observer.subscribe(pid, window_id, lambda kind, k=key: self.notify(k, kind))
        except Exception as e:  # A bad app degrades its own window, never the caller.
            w.degraded = f"observer failed: {e}"
        if w.degraded:
            log.info("window %s is read on a timer: %s", key, w.degraded)
        return w

    def drop(self, key: str, why: str = "") -> None:
        with self._lock:
            w = self._watches.pop(key, None)
        if w is None:
            return
        log.debug("unwatch %s (%s)", key, why)
        try:
            self.observer.unsubscribe(w.pid, w.window_id)
        except Exception as e:
            log.debug("unsubscribe %s: %s", key, e)
        try:
            self._forget(key)
        except Exception as e:
            log.debug("forget %s: %s", key, e)

    def drop_pid(self, pid: int, why: str) -> None:
        with self._lock:
            keys = [k for k, w in self._watches.items() if w.pid == pid]
        for k in keys:
            self.drop(k, why)

    def get(self, key: str) -> Watch | None:
        with self._lock:
            return self._watches.get(key)

    # ---- notifications -----------------------------------------------------------

    def notify(self, key: str, kind: str) -> None:
        """From an observer: the window changed. Cheap and lock-light: it runs
        on the observer's thread."""
        w = self._watches.get(key)
        if w is None:
            return
        if kind == GONE:
            w.full = True
            w.dirty = True
            self._wake.set()
            return
        w.notifications += 1
        w.misses = 0
        if kind == WINDOW:
            w.full = True
        elif kind == STRUCTURE:
            w.burst += 1
            if w.burst > LARGE_CHANGE:
                w.full = True
        w.dirty = True
        self._wake.set()

    # ---- reads --------------------------------------------------------------------

    def serve(self, key: str) -> str | None:
        """How a read of a watched window is answered: None when the map can be
        served as it is, "patch" or "full" when it must be refreshed first."""
        w = self._watches.get(key)
        if w is None:
            return "full"
        if w.full or not w.refreshed:
            return "full"
        if w.degraded:
            return "full" if time.monotonic() - w.refreshed > DEGRADED_TTL_S else None
        return "patch" if w.dirty else None

    def begin(self, key: str) -> bool:
        """Called by the driver right before it refreshes a window (reader lock
        held): clears the dirty state, so a notification during the read marks
        it dirty again. Returns whether the refresh must be a full walk."""
        w = self._watches.get(key)
        if w is None:
            return True
        full = w.full or not w.refreshed
        w.dirty, w.full, w.burst = False, False, 0
        return full

    def done(self, key: str, full: bool) -> None:
        w = self._watches.get(key)
        if w is None:
            return
        w.refreshed = time.monotonic()
        if full:
            w.walks += 1
        else:
            w.patches += 1
        self._baseline(w)

    def state(self, key: str) -> dict[str, Any]:
        w = self._watches.get(key)
        if w is None:
            return {"state": "off"}
        out: dict[str, Any] = {"state": "degraded" if w.degraded else "live", "observer": self.observer.name}
        if w.degraded:
            out["reason"] = w.degraded
        return out

    def status(self) -> dict[str, Any]:
        with self._lock:
            ws = list(self._watches.values())
        now = time.monotonic()
        return {
            "observer": self.observer.name, "max_windows": MAX_WINDOWS, "ttl_s": TTL_S,
            "windows": [{
                "window": w.key, "state": "degraded" if w.degraded else "live", "reason": w.degraded,
                "notifications": w.notifications, "patches": w.patches, "walks": w.walks,
                "idle_s": round(now - w.used, 1), "dirty": w.dirty,
            } for w in ws],
        }

    def close(self) -> None:
        self._closed = True
        self._wake.set()
        for key in list(self._watches):
            self.drop(key, "closing")
        try:
            self.observer.close()
        except Exception:
            pass

    # ---- worker ---------------------------------------------------------------------

    def _baseline(self, w: Watch) -> None:
        try:
            w.probe_sig = self.observer.probe(w.pid, w.window_id)
        except Exception:
            w.probe_sig = None
        w.probed = time.monotonic()

    def _run(self) -> None:
        timeout = PROBE_S
        while not self._closed:
            self._wake.wait(timeout=timeout)
            self._wake.clear()
            if self._closed:
                return
            now = time.monotonic()
            with self._lock:
                ws = list(self._watches.values())
            timeout = PROBE_S
            for w in ws:
                try:
                    later = self._tend(w, now)
                    if later is not None:
                        timeout = min(timeout, later)
                except WindowGone as e:
                    self.drop(w.key, str(e))
                except LookupError as e:  # AppGone, or the driver's refresh found the target gone.
                    self.drop_pid(w.pid, f"gone: {e}")
                except Exception as e:  # Never let one window stop the worker.
                    log.debug("live map %s: %s", w.key, e)

    def _tend(self, w: Watch, now: float) -> float | None:
        """Look after one window; returns seconds until it wants another look
        sooner than the probe period."""
        if now - w.used > TTL_S:
            self.drop(w.key, "idle")
            return None
        if w.dirty and w.refreshed and not w.degraded and now - w.asked < EAGER_S:
            wait = EAGER_MIN_S - (now - w.refreshed)
            if wait > 0:
                return wait
            self._refresh(w.key, False)  # Patch now, so the next read is a cached one.
            return None
        if w.degraded or not w.refreshed or w.dirty or now - w.probed < PROBE_S:
            return None
        sig = self.observer.probe(w.pid, w.window_id)
        w.probed = now
        if sig is None or w.probe_sig is None or sig == w.probe_sig:
            return None
        # The window changed and no notification said so.
        w.misses += 1
        if w.misses >= MISSES_TO_DEGRADE:
            w.degraded = "changes arrive without accessibility notifications"
            log.info("window %s degraded: %s", w.key, w.degraded)
        w.full = w.dirty = True
        if now - w.asked < EAGER_S:
            self._refresh(w.key, True)
        return None
