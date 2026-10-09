"""arc engine: the forked arc-driver, in process (macOS only).

Fast accessibility reads of the visible rows, settle on AX notifications, the
freshness guard, menu-bar commands, and minimized/hidden windows. All arc
calls run under one lock: arc's Driver is not documented as thread-safe, and
AX itself serializes per app anyway.
"""

from __future__ import annotations

import logging
import sys
import threading
from typing import Any

from ..element_map import RawNode

log = logging.getLogger("allternit_driver.arc")


_KEYS = {
    "return": "ENTER", "enter": "ENTER", "esc": "ESCAPE", "escape": "ESCAPE", "tab": "TAB",
    "space": "SPACE", "backspace": "BACKSPACE", "delete": "DELETE", "up": "ARROW_UP",
    "down": "ARROW_DOWN", "left": "ARROW_LEFT", "right": "ARROW_RIGHT", "home": "HOME",
    "end": "END", "pageup": "PAGE_UP", "page_up": "PAGE_UP", "pagedown": "PAGE_DOWN",
    "page_down": "PAGE_DOWN", "-": "MINUS", "=": "EQUAL", "[": "LEFT_BRACKET",
    "]": "RIGHT_BRACKET", "\\": "BACKSLASH", ";": "SEMICOLON", "'": "QUOTE", ",": "COMMA",
    ".": "PERIOD", "/": "SLASH", "`": "GRAVE",
}
_MODS = {"cmd": "MOD", "command": "MOD", "super": "MOD", "meta": "MOD", "mod": "MOD", "ctrl": "CTRL",
         "control": "CTRL", "alt": "ALT", "option": "ALT", "opt": "ALT", "shift": "SHIFT"}


def arc_keys(spec: str) -> str:
    """``cmd+shift+s`` / ``Return`` / ``ENTER`` → arc's ``MOD+SHIFT+S`` / ``ENTER``."""
    parts = [p.strip() for p in spec.split("+") if p.strip()]
    if not parts:
        raise ValueError("an empty key")
    *mods, key = parts
    k = _KEYS.get(key.lower(), key.upper())
    return "+".join([*(_MODS.get(m.lower(), m.upper()) for m in mods), k])


class ArcEngine:
    name = "arc"

    def __init__(self, hub: Any = None) -> None:
        self._lock = threading.RLock()
        self._driver: Any = None
        self.error: str | None = None
        # The live map's notification hub (live/macos.py): arc's journal, settle
        # counter and walk caches all run on its one observer per app.
        self.hub = hub
        self._resolved: dict[int, tuple[int, int]] = {}  # pid -> (hub epoch, window id)

    @property
    def available(self) -> bool:
        return self._driver is not None

    def start(self) -> None:
        if sys.platform != "darwin":
            self.error = "arc runs on macOS only"
            return
        try:
            from arc_cua import Driver

            if self.hub is None:
                from ..live.macos import AXHub

                self.hub = AXHub()
            from ..live.macos import app_factory

            self._driver = Driver(app_factory=app_factory(self.hub))
            status = Driver.status()
            # Load the frameworks and backends now (~0.5 s, once), not on the
            # first read a user is waiting for.
            import AppKit  # noqa: F401  # type: ignore
            from arc_cua.backends import macos_app, macos_ax, macos_changes, macos_events, macos_menus  # noqa: F401
            from arc_cua.backends.macos_ocr import _frameworks

            _frameworks()  # Quartz + Vision, loaded by the first window lookup otherwise.
            AppKit.NSWorkspace.sharedWorkspace().frontmostApplication()
            if not status["permissions"]["accessibility"]:
                self.error = "Accessibility permission is not granted"
            log.info("arc engine ready: %s", status)
        except Exception as e:
            self._driver = None
            self.error = f"arc engine unavailable: {e}"
            log.warning(self.error)

    def close(self) -> None:
        with self._lock:
            if self._driver is not None:
                self._driver.close()

    # ---- targets -------------------------------------------------------------

    def resolve(self, pid: int, window_id: int | None) -> int:
        if window_id:
            return int(window_id)
        # The app's current window, as last resolved, while no window-level
        # notification arrived since (no AX call); else ask the app again.
        epoch = self.hub.epoch(pid) if self.hub is not None else None
        hit = self._resolved.get(pid)
        if epoch is not None and hit is not None and hit[0] == epoch:
            return hit[1]
        with self._lock:
            self._driver._app(pid)  # Subscribes the app before its first walk.
            epoch = self.hub.epoch(pid) if self.hub is not None else None
            wid = self._driver.target(pid).window_id
        if epoch is not None:
            self._resolved[pid] = (epoch, wid)
        return wid

    def apps(self) -> list[dict[str, Any]]:
        from arc_cua import Driver

        return Driver.apps()

    # ---- reads -----------------------------------------------------------------

    def read(self, pid: int, window_id: int, full: bool = True) -> tuple[list[RawNode], dict[str, Any], Any]:
        """Walk one window. ``full=False`` patches: the walk cache re-reads only
        the elements the app's notifications named since the last walk (plus a
        child-list check of containers); ``full`` starts the cache over."""
        from arc_cua import WindowTarget

        with self._lock:
            if full:
                cache = getattr(self._driver._app(pid).backend(window_id), "_cache", None)
                if cache is not None:
                    cache.reset(None, None)
            snap = self._driver.observe(WindowTarget(pid, window_id))
        return self.nodes(snap), self.meta(snap), snap

    @staticmethod
    def running(pid: int) -> bool:
        import AppKit  # type: ignore

        app = AppKit.NSRunningApplication.runningApplicationWithProcessIdentifier_(pid)
        return app is not None and not app.isTerminated()

    def forget_window(self, pid: int) -> None:
        self._resolved.pop(pid, None)

    def release(self, pid: int) -> None:
        """Stop working with an app (unwatched, or it quit): its observer, walk
        caches and parked windows go."""
        self._resolved.pop(pid, None)
        with self._lock:
            if self._driver is not None:
                try:
                    self._driver.release(pid)
                except Exception as e:
                    log.debug("release %s: %s", pid, e)

    @staticmethod
    def nodes(snap: Any) -> list[RawNode]:
        out = []
        for e in snap.elements:
            b = e.bounds
            out.append(RawNode(
                key=e.id, role=e.role, name=e.name or "", value=e.value,
                bounds=(b.x, b.y, b.width, b.height) if b is not None else None,
                parent=e.parent_id, enabled=e.enabled, focused=e.focused,
                actions=tuple(str(a).lower() for a in e.actions), native=e.id,
            ))
        return out

    @staticmethod
    def meta(snap: Any) -> dict[str, Any]:
        ctx = snap.context
        return {
            "app": snap.application, "title": snap.window,
            "degraded": None if snap.elements else "empty_tree",
            "hint": ctx.get("hint"),
        }

    def wait(self, snap: Any, timeout_s: float) -> Any:
        with self._lock:
            return self._driver.wait(snap, timeout_s=timeout_s)

    # ---- acting ----------------------------------------------------------------

    def act(self, snap: Any, native: str, op: str, value: Any = None, key: str | None = None) -> Any:
        """One element action, settled. Returns arc's ActResult (status done /
        changed / stale, with a fresh snapshot)."""
        from arc_cua import ActionKind

        with self._lock:
            if op == "focus":
                return self._focus(snap, native)
            if op == "press":
                if native:
                    self._focus(snap, native)
                return self._driver.press(self._driver.target_of(snap), arc_keys(key or "return"), settle=True)
            kind = {
                "click": ActionKind.CLICK, "select": ActionKind.CLICK, "double_click": ActionKind.DOUBLE_CLICK,
                "right_click": ActionKind.RIGHT_CLICK, "set_value": ActionKind.SET_VALUE, "type": ActionKind.TYPE_TEXT,
            }.get(op)
            if kind is None:
                raise ValueError(f"{op} isn't an element action")
            if kind in (ActionKind.SET_VALUE, ActionKind.TYPE_TEXT):
                element = snap.element(native)
                if kind not in element.actions:
                    # Fields that only take typing (web inputs, documents) or only a value write.
                    kind = ActionKind.TYPE_TEXT if kind == ActionKind.SET_VALUE else ActionKind.SET_VALUE
                return self._driver.act(snap, kind, native, value="" if value is None else str(value), settle=True)
            return self._driver.act(snap, kind, native, settle=True)

    def _focus(self, snap: Any, native: str) -> Any:
        import ApplicationServices as AS  # type: ignore
        from arc_cua.driver import ActResult

        app = self._driver._app(snap.context["pid"])
        ref = getattr(app.backend(snap.context["window_id"]), "_refs", {}).get(native)
        if ref is None or AS.AXUIElementSetAttributeValue(ref, "AXFocused", True) != 0:
            raise ValueError(f"{native} can't take focus")
        return ActResult("done", self._driver.observe(self._driver.target_of(snap)))

    def menu(self, pid: int, window_id: int | None, path: list[str]) -> Any:
        from arc_cua import WindowTarget

        with self._lock:
            where = WindowTarget(pid, window_id) if window_id else pid
            return self._driver.run_command(where, " > ".join(path), settle=True)

    def commands(self, pid: int, query: str | None) -> list[dict[str, Any]]:
        with self._lock:
            cmds = self._driver.commands(pid, query=query)
        return [c if isinstance(c, dict) else getattr(c, "__dict__", {"path": str(c)}) for c in cmds]

    def screenshot(self, pid: int, window_id: int) -> tuple[bytes, float]:
        from arc_cua import WindowTarget

        with self._lock:
            shot = self._driver.screenshot(WindowTarget(pid, window_id), max_side=4096)
        return shot.png, shot.scale

    def crop_hashes(self, pid: int, window_id: int, elements: list[Any]) -> None:
        """Attach an 8x8 average hash of each element's pixels (``crop``): a
        small, change-tolerant fingerprint the vision fallback and replay
        compare. One window capture for all elements."""
        from arc_cua.backends.macos_ocr import _capture_window, _frameworks

        quartz = _frameworks()[0]
        with self._lock:
            image = _capture_window(quartz, window_id)
        if image is None or not elements or elements[0].bounds is None:
            return
        ox, oy, ow, _ = elements[0].bounds  # The window element comes first.
        scale = quartz.CGImageGetWidth(image) / max(ow, 1)
        gray = quartz.CGColorSpaceCreateDeviceGray()
        for e in elements:
            if e.bounds is None or e.bounds[2] < 2 or e.bounds[3] < 2:
                continue
            x, y, w, h = e.bounds
            rect = quartz.CGRectMake((x - ox) * scale, (y - oy) * scale, w * scale, h * scale)
            crop = quartz.CGImageCreateWithImageInRect(image, rect)
            if crop is None:
                continue
            ctx = quartz.CGBitmapContextCreate(None, 8, 8, 8, 8, gray, quartz.kCGImageAlphaNone)
            quartz.CGContextDrawImage(ctx, quartz.CGRectMake(0, 0, 8, 8), crop)
            data = quartz.CGBitmapContextGetData(ctx)
            pixels = bytes(data.as_buffer(64)) if hasattr(data, "as_buffer") else bytes(data[:64])
            mean = sum(pixels) / 64
            bits = 0
            for px in pixels:
                bits = (bits << 1) | (px >= mean)
            e.crop = f"{bits:016x}"
