"""Linux: AT-SPI events over D-Bus, behind the live map's ``Observer``.

UNVERIFIED LIVE: written to the same design as the macOS hub; it waits for a
non-production Linux test machine (no load on the production fleet).

One connection to the accessibility bus (address from ``org.a11y.Bus`` on the
session bus), one ``RegisterEvent`` per event class with the registry, and one
reader thread that blocks on the socket. Each signal names its sender (the
app's bus name), which maps to a pid; the event is delivered to every watched
window of that pid. Reads stay on the Cua engine (AT-SPI tree via Cua's
``get_window_state``); the live map only decides *when* a read is needed, so a
clean window is answered from the map without a call.

Uses ``jeepney`` (pure-Python D-Bus, bundled with the driver's Python). Without
it, or without an accessibility bus, every window reports degraded and reads
fall back to timed narrow reads.
"""

from __future__ import annotations

import logging
import threading
from typing import Any, Callable

from . import ELEMENT, GONE, STRUCTURE, WINDOW, AppGone, Observer

log = logging.getLogger("allternit_driver.live.atspi")

_EVENTS = {  # AT-SPI event class -> live-map kind
    "object:children-changed": STRUCTURE,
    "object:visible-data-changed": STRUCTURE,
    "object:bounds-changed": STRUCTURE,
    "object:property-change:accessible-name": ELEMENT,
    "object:property-change:accessible-value": ELEMENT,
    "object:state-changed": ELEMENT,
    "object:text-changed": ELEMENT,
    "object:value-changed": ELEMENT,
    "object:selection-changed": ELEMENT,
    "object:active-descendant-changed": ELEMENT,
    "focus:": ELEMENT,
    "window:create": WINDOW,
    "window:activate": WINDOW,
    "window:deactivate": WINDOW,
    "window:minimize": WINDOW,
    "window:restore": WINDOW,
    "window:destroy": GONE,
}
_INTERFACES = {"Object", "Window", "Focus"}


def kind_of(interface: str, member: str) -> str:
    """``org.a11y.atspi.Event.Object`` + ``ChildrenChanged`` → a live-map kind."""
    iface = interface.rsplit(".", 1)[-1].lower()
    name = "".join("-" + c.lower() if c.isupper() else c for c in member).lstrip("-")
    for key in (f"{iface}:{name}", f"{iface}:"):
        if key in _EVENTS:
            return _EVENTS[key]
    if iface == "object" and name.startswith("property-change"):
        return ELEMENT
    return STRUCTURE


class ATSPIObserver(Observer):
    name = "linux-atspi"

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._windows: dict[int, dict[int, Callable[[str], None]]] = {}
        self._senders: dict[str, int | None] = {}
        self._conn: Any = None
        self.error: str | None = None
        try:
            self._connect()
            threading.Thread(target=self._read, name="atspi-events", daemon=True).start()
        except Exception as e:
            self.error = f"AT-SPI events unavailable: {e}"
            log.info(self.error)

    def _connect(self) -> None:
        from jeepney import DBusAddress, MatchRule, new_method_call  # type: ignore
        from jeepney.bus_messages import message_bus  # type: ignore
        from jeepney.io.blocking import open_dbus_connection  # type: ignore

        session = open_dbus_connection(bus="SESSION")
        bus = DBusAddress("/org/a11y/bus", bus_name="org.a11y.Bus", interface="org.a11y.Bus")
        address = session.send_and_get_reply(new_method_call(bus, "GetAddress")).body[0]
        session.close()
        self._conn = conn = open_dbus_connection(bus=address)
        for iface in _INTERFACES:
            rule = MatchRule(type="signal", interface=f"org.a11y.atspi.Event.{iface}")
            conn.send_and_get_reply(message_bus.AddMatch(rule))
        registry = DBusAddress("/org/a11y/atspi/registry", bus_name="org.a11y.atspi.Registry",
                               interface="org.a11y.atspi.Registry")
        for event in _EVENTS:
            conn.send_and_get_reply(new_method_call(registry, "RegisterEvent", "s", (event,)))

    def _pid_of(self, sender: str) -> int | None:
        if sender not in self._senders:
            try:
                from jeepney import new_method_call  # type: ignore
                from jeepney.bus_messages import message_bus  # type: ignore

                reply = self._conn.send_and_get_reply(message_bus.GetConnectionUnixProcessID(sender), timeout=0.5)
                self._senders[sender] = int(reply.body[0])
            except Exception:
                self._senders[sender] = None
        return self._senders[sender]

    def _read(self) -> None:
        while True:
            try:
                msg = self._conn.receive()
            except Exception as e:  # Bus gone: every window degrades to timed reads.
                self.error = f"AT-SPI bus closed: {e}"
                return
            try:
                h = msg.header.fields
                iface, member, sender = str(h.get(2, "")), str(h.get(3, "")), str(h.get(6, ""))
                if not iface.startswith("org.a11y.atspi.Event."):
                    continue
                pid = self._pid_of(sender)
                with self._lock:
                    targets = list(self._windows.get(pid, {}).values()) if pid is not None else []
                kind = kind_of(iface, member)
                for notify in targets:
                    notify(kind)
            except Exception as e:
                log.debug("atspi event: %s", e)

    def subscribe(self, pid: int, window_id: int, notify: Callable[[str], None]) -> str | None:
        if self.error:
            return self.error
        with self._lock:
            self._windows.setdefault(pid, {})[window_id] = notify
        return None

    def unsubscribe(self, pid: int, window_id: int) -> None:
        with self._lock:
            self._windows.get(pid, {}).pop(window_id, None)

    def probe(self, pid: int, window_id: int) -> Any:
        import os

        if not os.path.exists(f"/proc/{pid}"):
            raise AppGone(f"pid {pid} quit")
        return None  # Child counts come from Cua's read; events carry the rest.
