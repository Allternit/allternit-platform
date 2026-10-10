"""Windows engine: UI Automation tree reads and actions, input via user32.

The guest-image engine (phase D1b): reads walk the UIA tree directly with
comtypes (no cua-driver on cloud computers), actions invoke the element's
UIA patterns, and pixel/key input posts through user32 (SetCursorPos,
mouse_event, SendInput, keybd_event) — the same primitives the previous
PowerShell control plane used. Screenshots go through System.Drawing in a
PowerShell child, exactly like the image's existing capture path.

``window_id`` is the HWND (what the live UIA observer subscribes by).
Element ``native`` handles are the read's tree path ("0/3/1"), looked up
again on a fresh walk at act time.

Every COM call runs on one MTA thread (the queue pattern from
``live/uia.py``): UIA clients are not safe to share across threads, and
delivering every call on one thread also serializes per window, matching
the driver's one-reader rule.
"""

from __future__ import annotations

import ctypes
import logging
import os
import queue
import subprocess
import threading
import time
from typing import Any, Callable

from ..element_map import RawNode
from .cua import CuaError

log = logging.getLogger("allternit_driver.uia")

MAX_ELEMENTS = 400
ABSOLUTE_MAX = 4000
MAX_DEPTH = 48
QUERY_TIMEOUT = 25.0
SHOT_TIMEOUT = 40.0

# UIA control type id -> AX-style role (the vocabulary the element map, the
# router and the models already share).
_CONTROL_TYPES = {
    50000: "AXButton", 50001: "AXCalendar", 50002: "AXCheckBox", 50003: "AXComboBox",
    50004: "AXTextField", 50005: "AXLink", 50006: "AXImage", 50007: "AXCell",
    50008: "AXList", 50009: "AXMenu", 50010: "AXMenuBar", 50011: "AXMenuItem",
    50012: "AXProgressIndicator", 50013: "AXRadioButton", 50014: "AXScrollBar",
    50015: "AXSlider", 50016: "AXIncrementor", 50017: "AXStatusBar", 50018: "AXTab",
    50019: "AXTabItem", 50020: "AXStaticText", 50021: "AXToolbar", 50022: "AXHelpTag",
    50023: "AXTree", 50024: "AXRow", 50025: "AXGroup", 50026: "AXGroup",
    50027: "AXSlider", 50028: "AXTable", 50029: "AXRow", 50030: "AXDocument",
    50031: "AXMenuButton", 50032: "AXWindow", 50033: "AXGroup", 50034: "AXGroup",
    50035: "AXCell", 50036: "AXTable", 50037: "AXGroup", 50038: "AXSplitter",
}

# UIA property ids used directly (comtypes gen module names vary by build).
_PROP_PROCESS_ID = 30002
_PROP_CONTROL_TYPE = 30003
_PROP_NAME = 30005
_PROP_FOCUSED = 30008
_PROP_ENABLED = 30010
_PROP_VALUE = 30045

# UIA pattern ids.
_PATTERN_INVOKE = 10000
_PATTERN_VALUE = 10002
_PATTERN_SELECTION_ITEM = 10010
_PATTERN_TOGGLE = 10015

# Virtual-key codes for press/hotkey when the name isn't a single character.
_VK = {
    "return": 0x0D, "enter": 0x0D, "escape": 0x1B, "esc": 0x1B, "tab": 0x09,
    "backspace": 0x08, "delete": 0x2E, "space": 0x20,
    "up": 0x26, "down": 0x28, "left": 0x25, "right": 0x27,
    "home": 0x24, "end": 0x23, "page_up": 0x21, "page_down": 0x22,
    "shift": 0x10, "ctrl": 0x11, "control": 0x11, "alt": 0x12,
    "cmd": 0x5B, "win": 0x5B, "super": 0x5B,
}


class _KeyBdInput(ctypes.Structure):
    _fields_ = [("wVk", ctypes.c_ushort), ("wScan", ctypes.c_ushort), ("dwFlags", ctypes.c_ulong),
                ("time", ctypes.c_ulong), ("dwExtraInfo", ctypes.c_size_t)]


class _MouseInput(ctypes.Structure):
    _fields_ = [("dx", ctypes.c_long), ("dy", ctypes.c_long), ("mouseData", ctypes.c_ulong),
                ("dwFlags", ctypes.c_ulong), ("time", ctypes.c_ulong), ("dwExtraInfo", ctypes.c_size_t)]


class _InputUnion(ctypes.Union):
    _fields_ = [("ki", _KeyBdInput), ("mi", _MouseInput)]


class _Input(ctypes.Structure):
    _fields_ = [("type", ctypes.c_ulong), ("union", _InputUnion)]


_INPUT_KEYBOARD = 1
_KEYEVENTF_KEYUP = 0x0002
_KEYEVENTF_UNICODE = 0x0004


def _send_unicode(text: str) -> None:
    """Type text with SendInput(KEYEVENTF_UNICODE): keyboard-layout
    independent, the same primitive the on-screen keyboard uses."""
    events = []
    for ch in text[:10000]:
        code = ord(ch)
        for flags in (0, _KEYEVENTF_KEYUP):
            events.append(_Input(type=_INPUT_KEYBOARD, union=_InputUnion(ki=_KeyBdInput(
                wVk=0, wScan=code, dwFlags=_KEYEVENTF_UNICODE | flags, time=0, dwExtraInfo=0))))
    user32 = ctypes.windll.user32
    for i in range(0, len(events), 200):
        chunk = events[i : i + 200]
        arr = (_Input * len(chunk))(*chunk)
        sent = user32.SendInput(len(chunk), arr, ctypes.sizeof(_Input))
        if sent != len(chunk):
            raise CuaError("error", f"SendInput typed {sent} of {len(chunk)} key events")
        time.sleep(0.01)


def _vk_of(key: str) -> int:
    n = key.strip().lower().replace("-", "_")
    if n in _VK:
        return _VK[n]
    if n.startswith("f") and n[1:].isdigit() and 1 <= int(n[1:]) <= 24:
        return 0x70 + int(n[1:]) - 1
    if len(key.strip()) == 1:
        scanned = ctypes.windll.user32.VkKeyScanW(ctypes.wintypes.WCHAR(key.strip()))
        if scanned != -1 and scanned != 0xFFFF:
            return scanned & 0xFF
    raise CuaError("bad_input", f"{key} isn't a key this computer understands")


def _key_event(vk: int, up: bool) -> None:
    ctypes.windll.user32.keybd_event(vk, 0, _KEYEVENTF_KEYUP if up else 0, 0)


def _click_at(x: int, y: int, button: str = "left", count: int = 1) -> None:
    user32 = ctypes.windll.user32
    if not user32.SetCursorPos(x, y):
        raise CuaError("error", f"SetCursorPos({x}, {y}) failed")
    down_up = {"left": (0x0002, 0x0004), "right": (0x0008, 0x0010), "middle": (0x0020, 0x0040)}.get(button)
    if down_up is None:
        raise CuaError("bad_input", f"unknown mouse button {button}")
    for _ in range(max(1, min(count, 20))):
        user32.mouse_event(down_up[0], 0, 0, 0, 0)
        time.sleep(0.05)
        user32.mouse_event(down_up[1], 0, 0, 0, 0)


def _process_name(pid: int) -> str:
    try:
        kernel32 = ctypes.windll.kernel32
        psapi = ctypes.windll.psapi
        handle = kernel32.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
        if not handle:
            return ""
        try:
            buf = ctypes.create_unicode_buffer(512)
            size = ctypes.c_ulong(512)
            if psapi.GetModuleFileNameExW(handle, None, buf, size):
                return os.path.basename(buf.value)
        finally:
            kernel32.CloseHandle(handle)
    except Exception:
        pass
    return ""


class UIAEngine:
    name = "uia"

    def __init__(self) -> None:
        self.error: str | None = None
        self._calls: "queue.Queue[tuple[Callable[[], Any], queue.Queue]]" = queue.Queue()
        self._uia: Any = None
        self._U: Any = None
        ready = threading.Event()
        threading.Thread(target=self._run, args=(ready,), name="uia-engine", daemon=True).start()
        ready.wait(8)
        if self._uia is None and not self.error:
            self.error = "UI Automation did not start"

    # ---- lifecycle: one MTA thread owns every COM object ----------------------

    def _run(self, ready: threading.Event) -> None:
        try:
            import comtypes  # type: ignore
            import comtypes.client  # type: ignore

            comtypes.CoInitializeEx(comtypes.COINIT_MULTITHREADED)
            comtypes.client.GetModule("UIAutomationCore.dll")
            from comtypes.gen import UIAutomationClient as U  # type: ignore

            self._U = U
            self._uia = comtypes.client.CreateObject(U.CUIAutomation, interface=U.IUIAutomation)
        except Exception as e:
            self.error = f"UI Automation unavailable: {e}"[:300]
            log.info(self.error)
            ready.set()
            return
        ready.set()
        while True:
            fn, reply = self._calls.get()
            try:
                reply.put((True, fn()))
            except Exception as e:
                reply.put((False, e))

    def _call(self, fn: Callable[[], Any], timeout: float = QUERY_TIMEOUT) -> Any:
        if self._uia is None:
            raise CuaError("engine_unavailable", self.error or "UI Automation isn't running")
        reply: queue.Queue = queue.Queue()
        self._calls.put((fn, reply))
        try:
            ok, value = reply.get(timeout=timeout)
        except queue.Empty as e:
            raise CuaError("timeout", "the UI Automation engine didn't answer in time") from e
        if not ok:
            raise value
        return value

    @property
    def available(self) -> bool:
        return self._uia is not None

    @property
    def pixel_available(self) -> bool:
        return True

    def start(self) -> None:
        if self._uia is None and not self.error:
            self.error = "UI Automation did not start"

    def close(self) -> None:
        pass

    # ---- windows ---------------------------------------------------------------

    def windows(self, pid: int | None = None) -> list[dict[str, Any]]:
        user32 = ctypes.windll.user32
        found: list[dict[str, Any]] = []

        @ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)
        def enum(hwnd: int, _: Any) -> bool:
            try:
                win_pid = ctypes.c_ulong(0)
                user32.GetWindowThreadProcessId(hwnd, ctypes.byref(win_pid))
                if pid is not None and int(win_pid.value) != pid:
                    return True
                if not user32.IsWindowVisible(hwnd):
                    return True
                title_buf = ctypes.create_unicode_buffer(512)
                user32.GetWindowTextW(hwnd, title_buf, 512)
                rect = ctypes.wintypes.RECT()
                user32.GetWindowRect(hwnd, ctypes.byref(rect))
                found.append({
                    "pid": int(win_pid.value), "window_id": int(hwnd),
                    "app_name": _process_name(int(win_pid.value)),
                    "title": title_buf.value,
                    "bounds": {"x": float(rect.left), "y": float(rect.top),
                               "width": float(rect.right - rect.left), "height": float(rect.bottom - rect.top)},
                    "frame": {"x": float(rect.left), "y": float(rect.top),
                              "width": float(rect.right - rect.left), "height": float(rect.bottom - rect.top)},
                    "is_on_screen": True,
                })
            except Exception:
                pass
            return True

        user32.EnumWindows(enum, None)
        return found

    # ---- reads (all on the MTA thread) -------------------------------------------

    def read(self, pid: int, window_id: int, max_elements: int | None = None, query: str | None = None) -> tuple[list[RawNode], dict[str, Any]]:
        nodes, title = self._call(lambda: self._read_inner(int(window_id), max_elements))
        meta = {
            "app": _process_name(pid) or "",
            "title": title,
            "truncated": len(nodes) >= min(max_elements or MAX_ELEMENTS, ABSOLUTE_MAX),
            "degraded": None if len(nodes) > 1 else "empty_tree",
        }
        return nodes, meta

    def _read_inner(self, hwnd: int, max_elements: int | None) -> tuple[list[RawNode], str]:
        cap = min(max_elements or MAX_ELEMENTS, ABSOLUTE_MAX)
        automation, walker = self._uia, self._uia.RawViewWalker
        root = automation.ElementFromHandle(hwnd)
        title = str(root.CurrentName or "")
        nodes: list[RawNode] = []

        def visit(el: Any, path: str, parent: str | None, depth: int) -> None:
            if len(nodes) >= cap or depth > MAX_DEPTH:
                return
            key = path
            try:
                ct = int(el.GetCurrentPropertyValue(_PROP_CONTROL_TYPE) or 0)
                name = str(el.GetCurrentPropertyValue(_PROP_NAME) or "")
            except Exception:
                return
            value = None
            try:
                v = el.GetCurrentPropertyValue(_PROP_VALUE)
                if v not in (None, ""):
                    value = str(v)
            except Exception:
                pass
            enabled, focused = True, False
            try:
                enabled = bool(el.GetCurrentPropertyValue(_PROP_ENABLED))
                focused = bool(el.GetCurrentPropertyValue(_PROP_FOCUSED))
            except Exception:
                pass
            actions: list[str] = []
            try:
                if el.GetCurrentPattern(_PATTERN_INVOKE) is not None:
                    actions.append("click")
            except Exception:
                pass
            try:
                if el.GetCurrentPattern(_PATTERN_VALUE) is not None:
                    actions.append("set_value")
            except Exception:
                pass
            try:
                r = el.CurrentBoundingRectangle
                bounds = (float(r.left), float(r.top), float(r.right - r.left), float(r.bottom - r.top))
            except Exception:
                bounds = None
            nodes.append(RawNode(
                key=key, role=_CONTROL_TYPES.get(ct, "AXGroup"), name=name, value=value, bounds=bounds,
                parent=parent, enabled=enabled, focused=focused, actions=tuple(actions),
                native={"path": key},
            ))
            try:
                child = walker.GetFirstChildElement(el)
            except Exception:
                return
            i = 0
            while child is not None and i < 512:
                visit(child, f"{path}/{i}", key, depth + 1)
                i += 1
                try:
                    child = walker.GetNextSiblingElement(child)
                except Exception:
                    break

        visit(root, "0", None, 0)
        return nodes, title

    def _find_inner(self, hwnd: int, path: str) -> Any:
        automation, walker = self._uia, self._uia.RawViewWalker
        root = automation.ElementFromHandle(hwnd)
        parts = path.split("/")
        if parts and parts[0] == "0":
            parts = parts[1:]
        current = root
        for part in parts:
            if part == "":
                continue
            idx = int(part)
            child = walker.GetFirstChildElement(current)
            i = 0
            while child is not None and i < idx:
                child = walker.GetNextSiblingElement(child)
                i += 1
            if child is None:
                raise CuaError("element_gone", f"the element at {path} is gone")
            current = child
        return current

    # ---- acting (COM on the MTA thread; input primitives off-thread) ---------------

    def _invoke_pattern(self, el: Any, pattern_id: int, method: str, *args: Any) -> bool:
        U = self._U

        def work() -> bool:
            try:
                ptr = el.GetCurrentPattern(pattern_id)
                if ptr is None:
                    return False
                iface = ptr.QueryInterface({_PATTERN_INVOKE: U.IUIAutomationInvokePattern,
                                            _PATTERN_VALUE: U.IUIAutomationValuePattern,
                                            _PATTERN_SELECTION_ITEM: U.IUIAutomationSelectionItemPattern,
                                            _PATTERN_TOGGLE: U.IUIAutomationTogglePattern}[pattern_id])
                getattr(iface, method)(*args)
                return True
            except Exception:
                return False

        return self._call(work)

    def act(self, pid: int, window_id: int, native: dict[str, Any], op: str, value: Any = None, key: str | None = None) -> None:
        path = str((native or {}).get("path") or "")
        if not path:
            raise CuaError("unknown_element", "this element has no UI Automation handle; call read_ui first")
        hwnd = int(window_id)
        el = self._call(lambda: self._find_inner(hwnd, path))

        def center() -> tuple[int, int]:
            def work() -> tuple[int, int]:
                r = el.CurrentBoundingRectangle
                return (int((r.left + r.right) / 2), int((r.top + r.bottom) / 2))

            try:
                return self._call(work)
            except Exception as e:
                raise CuaError("error", f"the element has no position: {e}") from e

        if op in ("click", "select"):
            if self._invoke_pattern(el, _PATTERN_INVOKE, "Invoke") or (op == "select" and self._invoke_pattern(el, _PATTERN_SELECTION_ITEM, "Select")):
                return
            try:
                self._call(lambda: el.SetFocus())
            except Exception:
                pass
            x, y = center()
            _click_at(x, y)
        elif op == "double_click":
            x, y = center()
            _click_at(x, y, "left", 2)
        elif op == "right_click":
            x, y = center()
            _click_at(x, y, "right")
        elif op == "focus":
            self._call(lambda: el.SetFocus())
        elif op == "set_value":
            text = "" if value is None else str(value)
            if self._invoke_pattern(el, _PATTERN_VALUE, "SetValue", text):
                return
            try:
                self._call(lambda: el.SetFocus())
            except Exception:
                pass
            _key_event(_VK["ctrl"], False)
            _key_event(_VK["a"], False)
            _key_event(_VK["a"], True)
            _key_event(_VK["ctrl"], True)
            _send_unicode(text)
        elif op == "type":
            try:
                self._call(lambda: el.SetFocus())
            except Exception:
                pass
            _send_unicode("" if value is None else str(value))
        elif op == "press":
            if key and "+" in key:
                raise CuaError("unsupported_op", "press takes one key; use a run_batch pixel step ({\"pixel\": {\"tool\": \"hotkey\", \"args\": {\"keys\": [...]}}}) for chords")
            vk = _vk_of(key or "return")
            _key_event(vk, False)
            _key_event(vk, True)
        else:
            raise CuaError("unsupported_op", f"{op} isn't an element action")

    def menu(self, pid: int, window_id: int, path: list[str]) -> None:
        automation = self._uia
        for part in path:
            def find(name: str = part) -> Any:
                root = automation.ElementFromHandle(int(window_id))
                by_name = automation.CreatePropertyCondition(_PROP_NAME, name)
                by_kind = automation.CreatePropertyCondition(_PROP_CONTROL_TYPE, 50011)  # MenuItem
                condition = automation.CreateAndCondition(by_name, by_kind)
                return automation.FindFirst(root, 4, condition)  # TreeScope_Descendants

            found = self._call(find)
            if found is None:
                raise CuaError("window_not_found", f"no menu item named {part!r} in this window")
            if not self._invoke_pattern(found, _PATTERN_INVOKE, "Invoke"):
                try:
                    self._call(lambda el=found: el.SetFocus())
                except Exception:
                    pass
            time.sleep(0.3)

    # ---- screenshots -----------------------------------------------------------------

    def screenshot(self, pid: int | None, window_id: int | None) -> bytes:
        region = None
        if pid is not None and window_id is not None:
            try:
                def work() -> tuple[float, float, float, float]:
                    el = self._uia.ElementFromHandle(int(window_id))
                    r = el.CurrentBoundingRectangle
                    return (float(r.left), float(r.top), float(r.right - r.left), float(r.bottom - r.top))

                region = self._call(work)
            except Exception as e:
                log.debug("window region for screenshot: %s", e)
        out = os.path.join(os.environ.get("TEMP", r"C:\Windows\Temp"), f"allternit-driver-{os.getpid()}.png")
        if region is None:
            script = _SHOT_PS.format(path=out)
        else:
            script = _SHOT_REGION_PS.format(x=int(region[0]), y=int(region[1]),
                                            w=max(1, int(region[2])), h=max(1, int(region[3])), path=out)
        try:
            proc = subprocess.run(
                ["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", script],
                capture_output=True, text=True, timeout=SHOT_TIMEOUT)
        except FileNotFoundError as e:
            raise CuaError("engine_unavailable", "powershell.exe isn't available on this computer") from e
        except subprocess.TimeoutExpired as e:
            raise CuaError("timeout", "the screenshot didn't finish in time") from e
        if proc.returncode != 0:
            raise CuaError("error", f"screenshot: {proc.stderr.strip()[:200]}")
        try:
            with open(out, "rb") as f:
                return f.read()
        except OSError as e:
            raise CuaError("error", f"couldn't read the screenshot: {e}") from e
        finally:
            try:
                os.unlink(out)
            except OSError:
                pass

    # ---- pixels -----------------------------------------------------------------------

    def pixel(self, tool: str, args: dict[str, Any]) -> dict[str, Any]:
        user32 = ctypes.windll.user32
        coord = args.get("coordinate") or [args.get("x"), args.get("y")]
        x, y = (int(round(float(v))) for v in coord) if isinstance(coord, list) and len(coord) == 2 and coord[0] is not None else (None, None)
        button = str(args.get("button") or "left")
        if tool == "get_screen_size":
            return {"width": float(user32.GetSystemMetrics(0)), "height": float(user32.GetSystemMetrics(1))}
        if tool == "get_cursor_position":
            pt = ctypes.wintypes.POINT()
            if not user32.GetCursorPos(ctypes.byref(pt)):
                raise CuaError("error", "GetCursorPos failed")
            return {"x": float(pt.x), "y": float(pt.y)}
        if tool == "move_cursor":
            self._need_xy(tool, x, y)
            if not user32.SetCursorPos(x, y):
                raise CuaError("error", f"SetCursorPos({x}, {y}) failed")
            return {}
        if tool in ("click", "double_click", "right_click"):
            count = 2 if tool == "double_click" else int(args.get("count") or 1)
            if tool == "right_click":
                button = "right"
            self._need_xy(tool, x, y)
            mods = [str(m).lower() for m in (args.get("modifier") or [])]
            for m in mods:
                _key_event(_vk_of(m), False)
            try:
                _click_at(x, y, button, count)
            finally:
                for m in reversed(mods):
                    _key_event(_vk_of(m), True)
            return {}
        if tool == "drag":
            self._need_xy(tool, x, y)
            from_x = int(round(float(args.get("from_x", x))))
            from_y = int(round(float(args.get("from_y", y))))
            if not user32.SetCursorPos(from_x, from_y):
                raise CuaError("error", "SetCursorPos failed")
            user32.mouse_event(0x0002, 0, 0, 0, 0)  # LEFTDOWN
            try:
                steps = max(2, min(20, (abs(x - from_x) + abs(y - from_y)) // 40 + 2))
                for i in range(1, steps + 1):
                    user32.SetCursorPos(round(from_x + (x - from_x) * i / steps), round(from_y + (y - from_y) * i / steps))
                    time.sleep(0.02)
            finally:
                user32.mouse_event(0x0004, 0, 0, 0, 0)  # LEFTUP
            return {}
        if tool == "scroll":
            direction = str(args.get("scroll_direction") or args.get("direction") or "down")
            delta = {"up": 120, "down": -120}.get(direction)
            if delta is None:
                raise CuaError("bad_input", "on Windows scroll_direction is up or down")
            amount = int(args.get("scroll_amount", args.get("amount", 3)) or 3)
            if x is not None and y is not None:
                user32.SetCursorPos(x, y)
            for _ in range(max(1, min(amount, 50))):
                user32.mouse_event(0x0800, 0, 0, delta, 0)  # WHEEL
                time.sleep(0.03)
            return {}
        if tool == "type_text":
            _send_unicode(str(args.get("text") or ""))
            return {}
        if tool == "press_key":
            vk = _vk_of(str(args.get("key") or "return"))
            _key_event(vk, False)
            _key_event(vk, True)
            return {}
        if tool == "hotkey":
            keys = [str(k) for k in (args.get("keys") or [])]
            if not keys:
                raise CuaError("bad_input", "hotkey needs keys")
            vks = [_vk_of(k) for k in keys]
            for vk in vks:
                _key_event(vk, False)
            for vk in reversed(vks):
                _key_event(vk, True)
            return {}
        raise CuaError("bad_input", f"{tool} isn't a pixel op")

    @staticmethod
    def _need_xy(tool: str, x: int | None, y: int | None) -> None:
        if x is None or y is None:
            raise CuaError("bad_input", f"{tool} needs a coordinate")


_SHOT_PS = r"""
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$bitmap = New-Object System.Drawing.Bitmap $screen.Width, $screen.Height
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($screen.Location, [System.Drawing.Point]::Empty, $screen.Size)
$bitmap.Save('{path}', [System.Drawing.Imaging.ImageFormat]::Png)
$graphics.Dispose()
$bitmap.Dispose()
"""

_SHOT_REGION_PS = r"""
Add-Type -AssemblyName System.Drawing
$bitmap = New-Object System.Drawing.Bitmap {w}, {h}
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen({x}, {y}, 0, 0, $bitmap.Size)
$bitmap.Save('{path}', [System.Drawing.Imaging.ImageFormat]::Png)
$graphics.Dispose()
$bitmap.Dispose()
"""
