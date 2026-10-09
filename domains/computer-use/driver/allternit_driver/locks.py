"""One reader and one input owner per window.

* ``reader(window)``: only one tree walk of a window at a time. A caller that
  waited for another reader gets that reader's fresh map instead of walking
  again (see ``Driver.read_ui``), so two clients never double-walk a window.
* ``input(window)``: one input owner per window; window-targeted input from
  different windows runs side by side.
* ``desktop_input()``: screen-wide pixel input (it lands on whatever window is
  under the pointer or focused) excludes every window's input while it runs.
"""

from __future__ import annotations

import threading
from contextlib import contextmanager
from typing import Iterator


class _Gate:
    """Shared/exclusive gate: window input shares it, desktop input owns it."""

    def __init__(self) -> None:
        self._cond = threading.Condition()
        self._shared = 0
        self._exclusive = False
        self._waiting_exclusive = 0

    @contextmanager
    def shared(self) -> Iterator[None]:
        with self._cond:
            while self._exclusive or self._waiting_exclusive:
                self._cond.wait()
            self._shared += 1
        try:
            yield
        finally:
            with self._cond:
                self._shared -= 1
                self._cond.notify_all()

    @contextmanager
    def exclusive(self) -> Iterator[None]:
        with self._cond:
            self._waiting_exclusive += 1
            while self._exclusive or self._shared:
                self._cond.wait()
            self._waiting_exclusive -= 1
            self._exclusive = True
        try:
            yield
        finally:
            with self._cond:
                self._exclusive = False
                self._cond.notify_all()


class WindowLocks:
    def __init__(self) -> None:
        self._guard = threading.Lock()
        self._readers: dict[str, threading.Lock] = {}
        self._inputs: dict[str, threading.Lock] = {}
        self._gate = _Gate()

    def _lock(self, table: dict[str, threading.Lock], key: str) -> threading.Lock:
        with self._guard:
            lock = table.get(key)
            if lock is None:
                lock = table[key] = threading.Lock()
            return lock

    @contextmanager
    def reader(self, window: str) -> Iterator[bool]:
        """Yields True when the caller had to wait for another reader."""
        lock = self._lock(self._readers, window)
        waited = not lock.acquire(blocking=False)
        if waited:
            lock.acquire()
        try:
            yield waited
        finally:
            lock.release()

    @contextmanager
    def input(self, window: str) -> Iterator[None]:
        with self._gate.shared(), self._lock(self._inputs, window):
            yield

    @contextmanager
    def desktop_input(self) -> Iterator[None]:
        with self._gate.exclusive():
            yield
