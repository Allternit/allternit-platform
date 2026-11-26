"""Linux engine: AT-SPI tree reads and actions, XTEST input via xdotool.

The guest-image engine (phase D1b): cloud computers don't carry cua-driver,
so reads walk the AT-SPI tree directly with pyatspi, key/pixel input posts
through xdotool (already the image's pixel fallback), and screenshots come
from scrot (the full screen) cropped to the window with Pillow.

Displays: a guest can run one Xvfb per agent (``allternit_driver.agent_display``,
``:10`` and up) beside the computer's own ``:0``. Every agent's Chrome joins
the one shared D-Bus session, so the one AT-SPI registry lists them all, told
apart by pid; only X is per display. Input (xdotool) and pixels (scrot) for a
window go to the display its process runs on, read from ``/proc/<pid>/environ``;
pid-less calls (a desktop screenshot, a pixel op) take ``display`` from the
call and fall back to the unit's ``DISPLAY``.

``window_id`` is the ordinal of the window among the app's shown top-level
windows (AT-SPI exposes no X window id); reads and actions agree because both
resolve it the same way. Element ``native`` handles are the read's tree path
("0/3/1"), looked up again on a fresh walk at act time.

pyatspi is imported lazily: without it, or without an accessibility bus, the
engine reports unavailable and the driver answers ``engine_unavailable`` (the
live observer degrades windows to timed reads in the same situation).
"""

from __future__ import annotations

import logging
import os
import re
import shutil
import subprocess
import sys
import threading
from typing import Any

from ..element_map import RawNode
from .cua import CuaError

log = logging.getLogger("allternit_driver.atspi")

MAX_ELEMENTS = 400
ABSOLUTE_MAX = 4000
MAX_DEPTH = 48

# AT-SPI role name -> the AX-style role vocabulary the element map, the
# router and the models already share (arc/Cua emit AX*; element_map strips
# the AX prefix for display and lowercases for the router's tables).
_ROLES = {
    "frame": "AXWindow", "dialog": "AXWindow", "window": "AXWindow",
    "push button": "AXButton", "toggle button": "AXButton",
    "radio button": "AXRadioButton", "check box": "AXCheckBox",
    "combo box": "AXComboBox", "text": "AXStaticText", "static text": "AXStaticText",
    "entry": "AXTextField", "password text": "AXTextField",
    "menu item": "AXMenuItem", "menu": "AXMenu", "menu bar": "AXMenuBar",
    "tool bar": "AXToolbar", "tab": "AXTab", "page tab": "AXTab",
    "list": "AXList", "table": "AXTable", "table cell": "AXCell",
    "table row": "AXRow", "column header": "AXCell", "row header": "AXCell",
    "tree": "AXTree", "tree item": "AXRow", "link": "AXLink", "image": "AXImage",
    "scroll pane": "AXScrollArea", "separator": "AXSplitter", "slider": "AXSlider",
    "spin button": "AXIncrementor", "progress bar": "AXProgressIndicator",
    "alert": "AXAlert", "panel": "AXGroup", "application": "AXApplication",
    "status bar": "AXStatusBar", "heading": "AXHeading",
}

# Cua-style key name -> X keysym xdotool understands.
_KEYSYMS = {
    "return": "Return", "escape": "Escape", "page_up": "Page_Up", "page_down": "Page_Down",
    "up": "Up", "down": "Down", "left": "Left", "right": "Right", "home": "Home", "end": "End",
    "delete": "Delete", "backspace": "BackSpace", "space": "space",
}

# Well-known session-bus address files (newest first), written by the guest
# image's desktop run.sh or the headless session-bus unit so a system service
# can reach the desktop's accessibility bus.
_BUS_FILES = ("/run/allternit/session-bus.env",)

_DISPLAY_RE = re.compile(r"^:[0-9]{1,4}(\.[0-9]{1,2})?$")


def check_display(display: Any) -> str | None:
    """A caller's ``display`` (":10"), or None when not given; refuses anything
    that isn't a local X display, so it never reaches a shell or a socket path
    unchecked."""
    if display is None or display == "":
        return None
    d = str(display).strip()
    if not _DISPLAY_RE.match(d):
        raise CuaError("bad_input", f"display must look like :10, not {d[:20]!r}")
    return d


def default_display() -> str:
    return os.environ.get("DISPLAY") or ":0"


def display_of_pid(pid: int) -> str | None:
    """The X display a process was started on (its ``DISPLAY``), or None."""
    try:
        with open(f"/proc/{int(pid)}/environ", "rb") as f:
            for item in f.read().split(b"\0"):
                if item.startswith(b"DISPLAY="):
                    d = item[8:].decode("utf-8", "replace").strip()
                    return d if _DISPLAY_RE.match(d) else None
    except (OSError, ValueError):
        return None
    return None


def _role(name: str) -> str:
    mapped = _ROLES.get(name.lower())
    if mapped is not None:
        return mapped
    return "AX" + "".join(part.capitalize() for part in name.replace("-", " ").split())



def _acc_name(accessible: Any) -> str:
    """An accessible's name. pyatspi (on GObject-introspected Atspi) exposes
    ``.name`` / ``get_name()``; there is no ``getName()``."""
    name = getattr(accessible, "name", None)
    if name is None:
        getter = getattr(accessible, "get_name", None) or getattr(accessible, "getName", None)
        name = getter() if getter else ""
    return name or ""

class ATSPIEngine:
    """The Linux native engine. One instance per sidecar; all pyatspi calls
    run on the calling thread (AT-SPI serializes per app on the bus)."""

    name = "atspi"

    def __init__(self) -> None:
        self.error: str | None = None
        self._desktop: Any = None
        self._lock = threading.Lock()
        self._displays: dict[int, str] = {}  # pid -> X display (a pid's DISPLAY never changes)

    def display_for(self, pid: int | None, display: str | None = None) -> str:
        """The display a call acts on: the caller's ``display``, else the
        target process's own, else the unit's."""
        given = check_display(display)
        if given:
            return given
        if pid:
            d = self._displays.get(int(pid))
            if d is None:
                d = display_of_pid(int(pid))
                if d is not None:
                    if len(self._displays) > 512:
                        self._displays.clear()
                    self._displays[int(pid)] = d
            if d:
                return d
        return default_display()

    # ---- lifecycle ---------------------------------------------------------

    @property
    def available(self) -> bool:
        try:
            self._desktop_ref()
            return True
        except Exception as e:
            self.error = str(e)[:300]
            return False

    @property
    def pixel_available(self) -> bool:
        return shutil.which("xdotool") is not None

    def start(self) -> None:
        try:
            self._desktop_ref()
            self.error = None
        except Exception as e:
            self.error = str(e)[:300]
            log.warning("atspi engine unavailable: %s", e)

    def close(self) -> None:
        self._desktop = None

    def _desktop_ref(self) -> Any:
        """The AT-SPI desktop, connecting on first use and after any failure
        (the desktop session may come up after the driver service)."""
        if self._desktop is not None:
            return self._desktop
        with self._lock:
            if self._desktop is not None:
                return self._desktop
            try:
                import pyatspi  # type: ignore
            except Exception as e:
                raise RuntimeError(f"pyatspi isn't installed: {e}") from e
            address = self._bus_address()
            if address:
                os.environ["DBUS_SESSION_BUS_ADDRESS"] = address
            desktop = pyatspi.Registry.getDesktop(0)
            if desktop is None or desktop.getChildCount() == 0:
                raise RuntimeError("the AT-SPI desktop is empty (no accessibility bus yet)")
            self._desktop = desktop
            return desktop

    @staticmethod
    def _bus_address() -> str | None:
        for path in _BUS_FILES:
            try:
                for line in open(path, encoding="utf-8"):
                    line = line.strip()
                    if line.startswith("DBUS_SESSION_BUS_ADDRESS="):
                        return line.split("=", 1)[1].strip().strip('"')
            except OSError:
                continue
        return os.environ.get("DBUS_SESSION_BUS_ADDRESS") or None

    def _refresh(self, accessible: Any) -> None:
        # pyatspi >= 2.45 can force a cache refresh; older versions serve the
        # cached tree, which the live observer's events keep coherent.
        try:
            refresh = getattr(accessible, "refresh", None)
            if refresh is not None:
                refresh()
        except Exception as e:
            log.debug("atspi refresh: %s", e)

    # ---- apps and windows ----------------------------------------------------

    def _pid_of(self, app: Any) -> int | None:
        get_pid = getattr(app, "get_process_id", None) or getattr(app, "getProcessId", None)
        if get_pid is not None:
            try:
                return int(get_pid())
            except Exception:
                pass
        # Older pyatspi: match the app name against /proc/*/comm.
        try:
            name = (_acc_name(app) or "").strip().lower()
        except Exception:
            return None
        if not name:
            return None
        for pid_s in os.listdir("/proc"):
            if not pid_s.isdigit():
                continue
            try:
                with open(f"/proc/{pid_s}/comm", encoding="utf-8", errors="replace") as f:
                    if f.read().strip().lower() == name:
                        return int(pid_s)
            except OSError:
                continue
        return None

    def _apps(self, pid: int | None = None) -> list[Any]:
        desktop = self._desktop_ref()
        out = []
        for i in range(desktop.getChildCount()):
            try:
                app = desktop.getChildAtIndex(i)
            except Exception:
                continue
            if app is None:
                continue
            if pid is not None and self._pid_of(app) != pid:
                continue
            out.append(app)
        return out

    @staticmethod
    def _is_top(accessible: Any) -> bool:
        try:
            return accessible.getRoleName().lower() in ("frame", "dialog", "window")
        except Exception:
            return False

    # AT-SPI state enum values (ABI-stable; pyatspi's STATE_* names preferred
    # when present).
    _STATE_ENABLED = 8
    _STATE_FOCUSED = 12
    _STATE_SHOWING = 28
    _STATE_VISIBLE = 36

    def _state_const(self, name: str, fallback: int) -> int:
        pyatspi = sys.modules.get("pyatspi")
        try:
            return int(getattr(pyatspi, name, fallback)) if pyatspi is not None else fallback
        except Exception:
            return fallback

    def _showing(self, accessible: Any) -> bool:
        try:
            return bool(accessible.getState().contains(self._state_const("STATE_SHOWING", self._STATE_SHOWING)))
        except Exception:
            return True

    def _top_windows(self, app: Any) -> list[Any]:
        try:
            self._refresh(app)
            children = [app.getChildAtIndex(i) for i in range(app.getChildCount())]
        except Exception as e:
            raise CuaError("window_not_found", f"the app stopped answering its accessibility tree: {e}") from e
        return [c for c in children if c is not None and self._is_top(c)]

    def _window(self, pid: int, window_id: int) -> Any:
        apps = self._apps(pid)
        if not apps:
            raise CuaError("window_not_found", f"no AT-SPI application for pid {pid}")
        tops: list[Any] = []
        for app in apps:
            tops.extend(self._top_windows(app))
        if not tops:
            raise CuaError("window_not_found", f"pid {pid} has no top-level windows")
        if window_id < 0 or window_id >= len(tops):
            raise CuaError("window_not_found", f"pid {pid} has {len(tops)} window(s); window_id {window_id} is out of range")
        return tops[window_id]

    def windows(self, pid: int | None = None) -> list[dict[str, Any]]:
        out: list[dict[str, Any]] = []
        for app in self._apps(pid):
            app_pid = self._pid_of(app)
            name = ""
            try:
                name = _acc_name(app) or ""
            except Exception:
                pass
            for i, win in enumerate(self._top_windows(app)):
                bounds: dict[str, Any] = {}
                try:
                    ext = win.queryComponent().getExtents(0)  # ATSPI_COORD_TYPE_SCREEN
                    bounds = {"x": float(ext.x), "y": float(ext.y), "width": float(ext.width), "height": float(ext.height)}
                except Exception:
                    pass
                title = ""
                try:
                    title = _acc_name(win) or ""
                except Exception:
                    pass
                out.append({
                    "pid": app_pid or 0, "window_id": i, "app_name": name, "title": title,
                    "bounds": bounds, "frame": bounds, "is_on_screen": self._showing(win),
                    "display": self.display_for(app_pid),
                })
        return out

    # ---- reads -----------------------------------------------------------------

    def _value_of(self, accessible: Any, role: str) -> Any:
        if role not in ("AXTextField", "AXTextArea", "AXComboBox"):
            return None
        try:
            text = accessible.queryText()
            n = text.getCharacterCount()
            if n and n < 4000:
                return text.getText(0, n)
        except Exception:
            pass
        return None

    def _walk(self, window: Any, max_elements: int | None) -> list[RawNode]:
        cap = min(max_elements or MAX_ELEMENTS, ABSOLUTE_MAX)
        nodes: list[RawNode] = []
        self._refresh(window)

        def visit(acc: Any, path: str, parent: str | None, depth: int) -> None:
            if len(nodes) >= cap or depth > MAX_DEPTH:
                return
            key = path
            try:
                role_name = acc.getRoleName() or ""
                name = _acc_name(acc) or ""
            except Exception:
                return
            role = _role(role_name)
            bounds = None
            try:
                ext = acc.queryComponent().getExtents(0)
                if ext.width >= 0 and ext.height >= 0:
                    bounds = (float(ext.x), float(ext.y), float(ext.width), float(ext.height))
            except Exception:
                pass
            enabled, focused = True, False
            try:
                states = acc.getState()
                enabled = bool(states.contains(self._state_const("STATE_ENABLED", self._STATE_ENABLED)))
                focused = bool(states.contains(self._state_const("STATE_FOCUSED", self._STATE_FOCUSED)))
            except Exception:
                pass
            actions: tuple[str, ...] = ()
            try:
                action = acc.queryAction()
                actions = tuple(action.getName(i) or "" for i in range(action.getNActions()))
            except Exception:
                pass
            nodes.append(RawNode(
                key=key, role=role, name=name, value=self._value_of(acc, role), bounds=bounds,
                parent=parent, enabled=enabled, focused=focused, actions=actions,
                native={"path": key},
            ))
            try:
                n = acc.getChildCount()
            except Exception:
                return
            for i in range(min(n, 512)):
                try:
                    child = acc.getChildAtIndex(i)
                except Exception:
                    continue
                if child is not None:
                    visit(child, f"{path}/{i}", key, depth + 1)

        visit(window, "0", None, 0)
        return nodes

    def read(self, pid: int, window_id: int, max_elements: int | None = None, query: str | None = None) -> tuple[list[RawNode], dict[str, Any]]:
        window = self._window(pid, window_id)
        title = ""
        app_name = ""
        try:
            title = _acc_name(window) or ""
            apps = self._apps(pid)
            app_name = _acc_name(apps[0]) if apps else ""
        except Exception:
            pass
        nodes = self._walk(window, max_elements)
        meta = {
            "app": app_name,
            "title": title,
            "truncated": len(nodes) >= min(max_elements or MAX_ELEMENTS, ABSOLUTE_MAX),
            "degraded": None if len(nodes) > 1 else "empty_tree",
        }
        return nodes, meta

    def _find(self, pid: int, window_id: int, path: str) -> Any:
        window = self._window(pid, window_id)
        self._refresh(window)
        current = window
        # Paths from _walk: "0" is the window, "0/3/1" walks child 3 then 1.
        parts = path.split("/")
        if parts and parts[0] == "0":
            parts = parts[1:]
        for part in parts:
            if part == "":
                continue
            try:
                current = current.getChildAtIndex(int(part))
            except Exception as e:
                raise CuaError("element_gone", f"the element at {path} is gone: {e}") from e
            if current is None:
                raise CuaError("element_gone", f"the element at {path} is gone")
        return current

    # ---- acting ------------------------------------------------------------------

    @staticmethod
    def _center(acc: Any) -> tuple[float, float]:
        try:
            ext = acc.queryComponent().getExtents(0)
            return (float(ext.x + ext.width / 2), float(ext.y + ext.height / 2))
        except Exception as e:
            raise CuaError("error", f"the element has no position: {e}") from e

    def _invoke(self, acc: Any, *names: str) -> bool:
        wanted = {n.lower() for n in names}
        try:
            action = acc.queryAction()
            for i in range(action.getNActions()):
                if (action.getName(i) or "").lower() in wanted:
                    action.doAction(i)
                    return True
        except Exception:
            pass
        return False

    def _xdo(self, *args: str, display: str | None = None) -> None:
        display = display or default_display()
        try:
            out = subprocess.run(["xdotool", *args], env={**os.environ, "DISPLAY": display},
                                 capture_output=True, text=True, timeout=20)
        except FileNotFoundError as e:
            raise CuaError("engine_unavailable", "xdotool isn't installed on this computer") from e
        except subprocess.TimeoutExpired as e:
            raise CuaError("timeout", "xdotool didn't finish in time") from e
        if out.returncode != 0:
            raise CuaError("error", f"xdotool {' '.join(args[:2])}: {out.stderr.strip()[:200]}")

    @staticmethod
    def _keysym(key: str) -> str:
        n = key.strip().lower().replace("-", "_")
        return _KEYSYMS.get(n, key.strip())

    def _type_text(self, text: str, display: str | None = None) -> None:
        self._xdo("type", "--delay", "12", "--", text, display=display)

    def _press(self, key: str, display: str | None = None) -> None:
        self._xdo("key", "--", self._keysym(key), display=display)

    def act(self, pid: int, window_id: int, native: dict[str, Any], op: str, value: Any = None, key: str | None = None) -> None:
        path = str((native or {}).get("path") or "")
        if not path:
            raise CuaError("unknown_element", "this element has no AT-SPI handle; call read_ui first")
        acc = self._find(pid, window_id, path)
        d = self.display_for(pid)
        if op in ("click", "select"):
            if self._invoke(acc, "click", "activate", "press", "open", "select"):
                return
            try:
                acc.queryComponent().grabFocus()
            except Exception:
                pass
            x, y = self._center(acc)
            self._xdo("mousemove", "--sync", str(round(x)), str(round(y)), display=d)
            self._xdo("click", "1", display=d)
        elif op == "double_click":
            x, y = self._center(acc)
            self._xdo("mousemove", "--sync", str(round(x)), str(round(y)), display=d)
            self._xdo("click", "--repeat", "2", "--delay", "80", "1", display=d)
        elif op == "right_click":
            x, y = self._center(acc)
            self._xdo("mousemove", "--sync", str(round(x)), str(round(y)), display=d)
            self._xdo("click", "3", display=d)
        elif op == "focus":
            try:
                acc.queryComponent().grabFocus()
            except Exception as e:
                raise CuaError("error", f"focus failed: {e}") from e
        elif op == "set_value":
            text = "" if value is None else str(value)
            if self._try_set_value(acc, text):
                return
            try:
                acc.queryComponent().grabFocus()
            except Exception:
                pass
            self._press("ctrl+a", d)
            self._type_text(text, d)
        elif op == "type":
            try:
                acc.queryComponent().grabFocus()
            except Exception:
                pass
            self._type_text("" if value is None else str(value), d)
        elif op == "press":
            if key and "+" in key:
                raise CuaError("unsupported_op", "press takes one key; use a run_batch pixel step ({\"pixel\": {\"tool\": \"hotkey\", \"args\": {\"keys\": [...]}}}) for chords")
            self._press(key or "return", d)
        else:
            raise CuaError("unsupported_op", f"{op} isn't an element action")

    def _try_set_value(self, acc: Any, text: str) -> bool:
        # Text interface direct write (pyatspi >= 2.45), then the Value
        # interface for sliders/spinners; fields that refuse both are typed
        # into (act's caller path), the same fallback the macOS engine uses.
        try:
            text_iface = acc.queryText()
            setter = getattr(text_iface, "setTextContents", None)
            if setter is not None:
                setter(text)
                return True
        except Exception:
            pass
        try:
            value_iface = acc.queryValue()
            num = float(text)
            value_iface.setCurrentValue(num)
            return True
        except Exception:
            pass
        return False

    def menu(self, pid: int, window_id: int, path: list[str]) -> None:
        d = self.display_for(pid)
        for part in path:
            window = self._window(pid, window_id)
            self._refresh(window)
            found: Any = None
            stack = [window]
            while stack and found is None:
                acc = stack.pop()
                try:
                    name = (_acc_name(acc) or "").strip()
                    role = acc.getRoleName().lower()
                except Exception:
                    continue
                if role == "menu item" and name.casefold() == part.casefold():
                    found = acc
                    break
                try:
                    stack.extend(acc.getChildAtIndex(i) for i in range(acc.getChildCount()))
                except Exception:
                    pass
            if found is None:
                raise CuaError("window_not_found", f"no menu item named {part!r} in this window")
            if not self._invoke(found, "click", "activate"):
                x, y = self._center(found)
                self._xdo("mousemove", "--sync", str(round(x)), str(round(y)), display=d)
                self._xdo("click", "1", display=d)
            import time

            time.sleep(0.3)

    # ---- pixels / screenshots ------------------------------------------------------

    def screenshot(self, pid: int | None, window_id: int | None, display: str | None = None) -> bytes:
        """The screen of the window's display (or ``display``, or the unit's),
        cropped to the window when one is given."""
        display = self.display_for(pid, display)
        path = f"/tmp/.allternit-driver-{os.getpid()}-{threading.get_ident()}.png"
        try:
            out = subprocess.run(["scrot", "-z", "-o", path], env={**os.environ, "DISPLAY": display},
                                 capture_output=True, text=True, timeout=30)
        except FileNotFoundError as e:
            raise CuaError("engine_unavailable", "scrot isn't installed on this computer") from e
        if out.returncode != 0:
            raise CuaError("error", f"scrot: {out.stderr.strip()[:200]}")
        try:
            with open(path, "rb") as f:
                png = f.read()
        except OSError as e:
            raise CuaError("error", f"couldn't read the screenshot: {e}") from e
        finally:
            try:
                os.unlink(path)
            except OSError:
                pass
        if pid is None or window_id is None:
            return png
        try:
            win = self._window(pid, int(window_id))
            ext = win.queryComponent().getExtents(0)
            return _crop_png(png, float(ext.x), float(ext.y), float(ext.width), float(ext.height))
        except CuaError:
            raise
        except Exception as e:
            log.warning("window crop failed, returning the full screen: %s", e)
            return png

    def pixel(self, tool: str, args: dict[str, Any]) -> dict[str, Any]:
        coord = args.get("coordinate") or [args.get("x"), args.get("y")]
        x, y = (int(round(float(v))) for v in coord) if isinstance(coord, list) and len(coord) == 2 and coord[0] is not None else (None, None)
        pid = args.get("pid")
        d = self.display_for(int(pid) if pid else None, args.get("display"))
        button = {"left": "1", "middle": "2", "right": "3"}.get(str(args.get("button") or "left"), "1")
        if tool == "get_screen_size":
            out = self._xdout("getdisplaygeometry", display=d)
            parts = out.split()
            if len(parts) >= 2:
                return {"width": float(parts[0]), "height": float(parts[1])}
            raise CuaError("error", f"couldn't parse the display size: {out.strip()[:100]}")
        if tool == "get_cursor_position":
            out = self._xdout("getmouselocation", "--shell", display=d)
            pos = {}
            for line in out.splitlines():
                if "=" in line:
                    k, _, v = line.partition("=")
                    pos[k.strip()] = v.strip()
            return {"x": float(pos.get("X", "0")), "y": float(pos.get("Y", "0"))}
        if tool == "move_cursor":
            self._need_xy(tool, x, y)
            self._xdo("mousemove", "--sync", str(x), str(y), display=d)
            return {}
        if tool == "click" or tool in ("double_click", "right_click"):
            count = 2 if tool == "double_click" else int(args.get("count") or 1)
            if tool == "right_click":
                button = "3"
            mods = [str(m) for m in (args.get("modifier") or [])]
            self._need_xy(tool, x, y)
            down = []
            for m in mods:
                down += ["keydown", _MODS.get(m.lower(), m)]
            if down:
                self._xdo(*down, display=d)
            try:
                self._xdo("mousemove", "--sync", str(x), str(y), display=d)
                self._xdo("click", "--repeat", str(count), "--delay", "80", button, display=d)
            finally:
                if down:
                    self._xdo("keyup", *[_MODS.get(m.lower(), m) for m in reversed(mods)], display=d)
            return {}
        if tool == "drag":
            self._need_xy(tool, x, y)
            from_x = int(round(float(args.get("from_x", x))))
            from_y = int(round(float(args.get("from_y", y))))
            steps = max(2, min(20, (abs(x - from_x) + abs(y - from_y)) // 40 + 2))
            self._xdo("mousemove", "--sync", str(from_x), str(from_y), display=d)
            self._xdo("mousedown", "1", display=d)
            try:
                for i in range(1, steps + 1):
                    self._xdo("mousemove", str(round(from_x + (x - from_x) * i / steps)), str(round(from_y + (y - from_y) * i / steps)), display=d)
            finally:
                self._xdo("mouseup", "1", display=d)
            return {}
        if tool == "scroll":
            direction = str(args.get("scroll_direction") or args.get("direction") or "down")
            btn = {"up": "4", "down": "5", "left": "6", "right": "7"}.get(direction)
            if btn is None:
                raise CuaError("bad_input", "scroll_direction must be up, down, left or right")
            amount = int(args.get("scroll_amount", args.get("amount", 3)) or 3)
            if x is not None and y is not None:
                self._xdo("mousemove", "--sync", str(x), str(y), display=d)
            self._xdo("click", "--repeat", str(max(1, min(amount, 50))), "--delay", "30", btn, display=d)
            return {}
        if tool == "type_text":
            self._type_text(str(args.get("text") or ""), d)
            return {}
        if tool == "press_key":
            self._press(str(args.get("key") or "return"), d)
            return {}
        if tool == "hotkey":
            keys = [str(k) for k in (args.get("keys") or [])]
            if not keys:
                raise CuaError("bad_input", "hotkey needs keys")
            down = []
            for k in keys:
                down += ["keydown", self._keysym(k)]
            self._xdo(*down, display=d)
            up = []
            for k in reversed(keys):
                up += ["keyup", self._keysym(k)]
            self._xdo(*up, display=d)
            return {}
        raise CuaError("bad_input", f"{tool} isn't a pixel op")

    def _xdout(self, *args: str, display: str | None = None) -> str:
        display = display or default_display()
        try:
            out = subprocess.run(["xdotool", *args], env={**os.environ, "DISPLAY": display},
                                 capture_output=True, text=True, timeout=15)
        except FileNotFoundError as e:
            raise CuaError("engine_unavailable", "xdotool isn't installed on this computer") from e
        if out.returncode != 0:
            raise CuaError("error", f"xdotool {' '.join(args)}: {out.stderr.strip()[:200]}")
        return out.stdout

    @staticmethod
    def _need_xy(tool: str, x: int | None, y: int | None) -> None:
        if x is None or y is None:
            raise CuaError("bad_input", f"{tool} needs a coordinate")


_MODS = {"ctrl": "ctrl", "control": "ctrl", "alt": "alt", "option": "alt", "shift": "shift", "cmd": "super", "command": "super", "super": "super", "meta": "super", "win": "super"}


def _crop_png(png: bytes, x: float, y: float, w: float, h: float) -> bytes:
    """Crop a full-screen PNG to a window's bounds with Pillow (installed as a
    runtime dependency on guest images); the full screen when Pillow is out."""
    try:
        from PIL import Image  # type: ignore
        import io

        with Image.open(io.BytesIO(png)) as im:
            box = (max(0, int(x)), max(0, int(y)), min(im.width, int(x + w)), min(im.height, int(y + h)))
            if box[2] <= box[0] or box[3] <= box[1]:
                return png
            cropped = im.crop(box)
            buf = io.BytesIO()
            cropped.save(buf, format="PNG")
            return buf.getvalue()
    except Exception as e:
        log.warning("Pillow crop failed, returning the full screen: %s", e)
        return png
