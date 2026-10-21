"""Windows: UI Automation events, behind the live map's ``Observer``.

UNVERIFIED LIVE: written to the same design as the macOS hub; it waits for a
non-production Windows test machine (no load on the production fleet).

One MTA thread owns the ``IUIAutomation`` client and, per watched window
(``window_id`` is the HWND), a structure-changed handler and a
property-changed handler (name, value, toggle, selection, enabled, expand
state) on the window's element and its subtree, plus one process-wide
focus-changed handler filtered by process id. Handlers only call the live
map's ``notify`` (a flag and an event set) and return; UIA calls them on its
own worker threads. Reads stay on the Cua engine (UIA tree via Cua's
``get_window_state``); the live map only decides when a read is needed.

Uses ``comtypes`` (pure Python, bundled with the driver's Python on Windows).
Without it every window reports degraded and reads fall back to timed narrow
reads.
"""

from __future__ import annotations

import logging
import queue
import threading
from typing import Any, Callable

from . import ELEMENT, STRUCTURE, Observer, WindowGone

log = logging.getLogger("allternit_driver.live.uia")

# UIA property ids: Name, ValueValue, ToggleToggleState, SelectionItemIsSelected,
# IsEnabled, ExpandCollapseExpandCollapseState, HasKeyboardFocus.
_PROPERTIES = (30005, 30045, 30086, 30079, 30010, 30070, 30008)
_TREE_SCOPE_SUBTREE = 7
_STRUCTURE_GONE = 1  # StructureChangeType_ChildRemoved on the window itself: checked by probe.


class UIAObserver(Observer):
    name = "windows-uia"

    def __init__(self) -> None:
        self.error: str | None = None
        self._calls: "queue.Queue[tuple[Callable[[], Any], queue.Queue]]" = queue.Queue()
        self._handlers: dict[tuple[int, int], tuple[Any, Any, Any]] = {}
        self._windows: dict[int, dict[int, Callable[[str], None]]] = {}
        self._lock = threading.Lock()
        self._uia: Any = None
        ready = threading.Event()
        threading.Thread(target=self._run, args=(ready,), name="uia-events", daemon=True).start()
        ready.wait(5)
        if self._uia is None and not self.error:
            self.error = "UI Automation did not start"

    def _run(self, ready: threading.Event) -> None:
        try:
            import comtypes  # type: ignore
            import comtypes.client  # type: ignore

            comtypes.CoInitializeEx(comtypes.COINIT_MULTITHREADED)
            comtypes.client.GetModule("UIAutomationCore.dll")
            from comtypes.gen import UIAutomationClient as U  # type: ignore

            self._U = U
            self._uia = comtypes.client.CreateObject(U.CUIAutomation, interface=U.IUIAutomation)
            self._focus = self._make_focus_handler()
            self._uia.AddFocusChangedEventHandler(None, self._focus)
        except Exception as e:
            self.error = f"UI Automation events unavailable: {e}"
            log.info(self.error)
            ready.set()
            return
        ready.set()
        while True:  # Every UIA call runs on this one MTA thread.
            fn, reply = self._calls.get()
            try:
                reply.put((True, fn()))
            except Exception as e:
                reply.put((False, e))

    def _call(self, fn: Callable[[], Any]) -> Any:
        reply: queue.Queue = queue.Queue()
        self._calls.put((fn, reply))
        ok, value = reply.get(timeout=5)
        if not ok:
            raise value
        return value

    def _fan(self, pid: int, window_id: int | None, kind: str) -> None:
        with self._lock:
            targets = self._windows.get(pid, {})
            notifies = [targets[window_id]] if window_id in targets else list(targets.values())
        for notify in notifies:
            notify(kind)

    def _make_focus_handler(self) -> Any:
        import comtypes  # type: ignore

        U, observer = self._U, self

        class Focus(comtypes.COMObject):
            _com_interfaces_ = [U.IUIAutomationFocusChangedEventHandler]

            def HandleFocusChangedEvent(self, sender: Any) -> int:
                try:
                    observer._fan(int(sender.CurrentProcessId), None, ELEMENT)
                except Exception:
                    pass
                return 0

        return Focus()

    def _make_handlers(self, pid: int, window_id: int) -> tuple[Any, Any]:
        import comtypes  # type: ignore

        U, observer = self._U, self

        class Structure(comtypes.COMObject):
            _com_interfaces_ = [U.IUIAutomationStructureChangedEventHandler]

            def HandleStructureChangedEvent(self, sender: Any, change: int, runtime_id: Any) -> int:
                try:
                    observer._fan(pid, window_id, STRUCTURE)
                except Exception:
                    pass
                return 0

        class Property(comtypes.COMObject):
            _com_interfaces_ = [U.IUIAutomationPropertyChangedEventHandler]

            def HandlePropertyChangedEvent(self, sender: Any, prop: int, value: Any) -> int:
                try:
                    observer._fan(pid, window_id, ELEMENT)
                except Exception:
                    pass
                return 0

        return Structure(), Property()

    def subscribe(self, pid: int, window_id: int, notify: Callable[[str], None]) -> str | None:
        if self.error:
            return self.error
        with self._lock:
            self._windows.setdefault(pid, {})[window_id] = notify
        if (pid, window_id) in self._handlers:
            return None

        def add() -> None:
            element = self._uia.ElementFromHandle(window_id)
            structure, prop = self._make_handlers(pid, window_id)
            self._uia.AddStructureChangedEventHandler(element, _TREE_SCOPE_SUBTREE, None, structure)
            ids = (comtypes_array(_PROPERTIES))
            self._uia.AddPropertyChangedEventHandlerNativeArray(element, _TREE_SCOPE_SUBTREE, None, prop, ids, len(_PROPERTIES))
            self._handlers[(pid, window_id)] = (element, structure, prop)

        try:
            self._call(add)
        except Exception as e:
            return f"UIA subscription failed: {e}"
        return None

    def unsubscribe(self, pid: int, window_id: int) -> None:
        with self._lock:
            self._windows.get(pid, {}).pop(window_id, None)
        handlers = self._handlers.pop((pid, window_id), None)
        if handlers is None or self._uia is None:
            return
        element, structure, prop = handlers

        def remove() -> None:
            self._uia.RemoveStructureChangedEventHandler(element, structure)
            self._uia.RemovePropertyChangedEventHandler(element, prop)

        try:
            self._call(remove)
        except Exception as e:
            log.debug("UIA unsubscribe: %s", e)

    def probe(self, pid: int, window_id: int) -> Any:
        import ctypes

        if not ctypes.windll.user32.IsWindow(window_id):  # type: ignore[attr-defined]
            raise WindowGone(f"window {window_id} closed")
        return None

    def close(self) -> None:
        for pid, wid in list(self._handlers):
            self.unsubscribe(pid, wid)


def comtypes_array(values: tuple[int, ...]) -> Any:
    import ctypes

    return (ctypes.c_int * len(values))(*values)


