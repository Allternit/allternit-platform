"""
Allternit Computer Use — Accessibility Tree Desktop Adapter

macOS: Uses NSAccessibility / AXUIElement API via pyobjc
Windows: Uses UI Automation (UIA) via comtypes
Linux: Uses AT-SPI via pyatspi

Inspired by Fazm (mediar-ai/fazm), UFO² (microsoft/UFO), and lahfir/agent-desktop.
Reads native app structure without requiring screenshots.
"""

from __future__ import annotations

import asyncio
import base64
import logging
import platform
import subprocess
import sys
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional, Tuple

logger = logging.getLogger(__name__)

PLATFORM = platform.system().lower()  # "darwin", "windows", "linux"

# ── BackgroundEventPoster import ──────────────────────────────────────────────

_POSTER_AVAILABLE = False
try:
    from ...core.background_events import get_poster as _get_poster
    _POSTER_AVAILABLE = True
except ImportError:
    try:  # gateway launched with core/ on sys.path
        from core.background_events import get_poster as _get_poster
        _POSTER_AVAILABLE = True
    except ImportError:
        logger.debug("background_events not available; desktop input posting disabled")
        _get_poster = None  # type: ignore

# ── Key code tables ───────────────────────────────────────────────────────────

_KEY_CODES: Dict[str, int] = {
    "return": 36, "enter": 76, "tab": 48, "space": 49, "delete": 51,
    "backspace": 51, "escape": 53, "esc": 53, "forwarddelete": 117,
    "home": 115, "end": 119, "pageup": 116, "pagedown": 121,
    "left": 123, "right": 124, "down": 125, "up": 126,
    "f1": 122, "f2": 120, "f3": 99, "f4": 118, "f5": 96, "f6": 97,
    "f7": 98, "f8": 100, "f9": 101, "f10": 109, "f11": 103, "f12": 111,
    "a": 0, "b": 11, "c": 8, "d": 2, "e": 14, "f": 3, "g": 5, "h": 4,
    "i": 34, "j": 38, "k": 40, "l": 37, "m": 46, "n": 45, "o": 31,
    "p": 35, "q": 12, "r": 15, "s": 1, "t": 17, "u": 32, "v": 9,
    "w": 13, "x": 7, "y": 16, "z": 6,
    "0": 29, "1": 18, "2": 19, "3": 20, "4": 21, "5": 23,
    "6": 22, "7": 26, "8": 28, "9": 25,
    "minus": 27, "equal": 24, "leftbracket": 33, "rightbracket": 30,
    "semicolon": 41, "quote": 39, "comma": 43, "period": 47, "slash": 44,
    "backslash": 42, "grave": 50,
}

_MOD_FLAGS: Dict[str, int] = {
    "cmd": 0x100000, "command": 0x100000,
    "ctrl": 0x040000, "control": 0x040000,
    "opt": 0x080000, "option": 0x080000, "alt": 0x080000,
    "shift": 0x020000,
    "fn": 0x800000,
}


# ── Change-count helper (used by _cmd_snapshot_with_refs diff mode) ──────────

def _count_changes(node: Any) -> Dict[str, int]:
    """Walk a diff-annotated AccessibilityNode tree and tally change_type counts."""
    counts: Dict[str, int] = {"added": 0, "removed": 0, "modified": 0, "unchanged": 0}

    def _walk(n: Any) -> None:
        ct = getattr(n, "change_type", None)
        if ct in counts:
            counts[ct] += 1
        elif ct is None:
            counts["unchanged"] += 1
        for child in getattr(n, "children", []):
            _walk(child)

    _walk(node)
    return counts


# ── Data classes ──────────────────────────────────────────────────────────────

@dataclass
class AppElement:
    """Native accessibility element."""
    role: str
    title: str = ""
    value: str = ""
    description: str = ""
    enabled: bool = True
    focused: bool = False
    frame: Optional[Dict[str, float]] = None     # {x, y, width, height}
    ref_id: Optional[str] = None                  # @e1, @e2, …
    is_interactive: bool = False
    children: List["AppElement"] = field(default_factory=list)

    def center(self) -> Optional[Tuple[int, int]]:
        if self.frame:
            return (
                int(self.frame["x"] + self.frame["width"] / 2),
                int(self.frame["y"] + self.frame["height"] / 2),
            )
        return None

    def matches(self, description: str) -> bool:
        desc = description.lower()
        return any(desc in s.lower() for s in [self.title, self.value, self.description, self.role] if s)

    def to_dict(self) -> Dict:
        return {
            "role": self.role, "title": self.title, "value": self.value,
            "description": self.description, "enabled": self.enabled,
            "focused": self.focused, "frame": self.frame,
            "ref_id": self.ref_id, "is_interactive": self.is_interactive,
        }


@dataclass
class AppSnapshot:
    """Accessibility snapshot of a running application."""
    app_name: str
    bundle_id: str = ""
    elements: List[AppElement] = field(default_factory=list)

    def find(self, description: str) -> Optional[AppElement]:
        for el in self.elements:
            if el.matches(description):
                return el
        return None

    def to_text(self, max_elements: int = 80) -> str:
        lines = [f"App: {self.app_name}"]
        for el in self.elements[:max_elements]:
            label = el.title or el.value or el.description or ""
            ref = f"[{el.ref_id}] " if el.ref_id else ""
            lines.append(f"  {ref}[{el.role}] {label}")
        return "\n".join(lines)


# ── Adapter ───────────────────────────────────────────────────────────────────

class AccessibilityAdapter:
    """
    Native accessibility tree adapter for desktop automation.
    Reads app structure without screenshots — privacy-preserving and fast.

    macOS: NSAccessibility via pyobjc (or AppleScript fallback)
    Windows: Windows UIA via comtypes
    Linux: AT-SPI via subprocess

    Full 53-command set ported from lahfir/agent-desktop.
    """

    ADAPTER_ID = "desktop.accessibility"

    # Interactive AX roles (used for ref assignment and click targeting)
    _INTERACTIVE_ROLES = frozenset({
        "AXButton", "AXCheckBox", "AXRadioButton", "AXTextField", "AXTextArea",
        "AXComboBox", "AXPopUpButton", "AXDisclosureTriangle", "AXLink",
        "AXMenuItem", "AXMenuButton", "AXSlider", "AXIncrementor",
        "AXCell", "AXRow", "AXColumn", "AXTabGroup", "AXTab",
        "AXToggleButton", "AXSearchField",
        # lowercase aliases for AT-SPI / generic:
        "button", "checkbox", "radio button", "text", "entry", "combo box",
        "push button", "toggle button", "link", "menu item", "slider",
        "spin button", "list item", "table cell", "tab",
    })

    def __init__(self):
        self._platform = PLATFORM

    # ═════════════════════════════════════════════════════════════════════════
    # Dispatch entry point
    # ═════════════════════════════════════════════════════════════════════════

    async def execute(self, action: str, params: Dict[str, Any]) -> Dict[str, Any]:
        """
        Unified dispatch for all 53 accessibility commands.
        Returns {"success": bool, ...extra}.
        """
        handlers = {
            # Mouse
            "click":                    self._cmd_click,
            "double_click":             self._cmd_double_click,
            "right_click":              self._cmd_right_click,
            "triple_click":             self._cmd_triple_click,
            "hover":                    self._cmd_hover,
            "drag":                     self._cmd_drag,
            "drag_element":             self._cmd_drag_element,
            "mouse_down":               self._cmd_mouse_down,
            "mouse_up":                 self._cmd_mouse_up,
            # Keyboard
            "type_text":                self._cmd_type_text,
            "clear_field":              self._cmd_clear_field,
            "set_value":                self._cmd_set_value,
            "press_key":                self._cmd_press_key,
            "key_combo":                self._cmd_key_combo,
            "key_down":                 self._cmd_key_down,
            "key_up":                   self._cmd_key_up,
            # Scroll
            "scroll":                   self._cmd_scroll,
            "scroll_to_element":        self._cmd_scroll_to_element,
            # AX state
            "toggle":                   self._cmd_toggle,
            "check":                    self._cmd_check,
            "uncheck":                  self._cmd_uncheck,
            "expand":                   self._cmd_expand,
            "collapse":                 self._cmd_collapse,
            "focus":                    self._cmd_focus,
            # Element discovery
            "find_elements":            self._cmd_find_elements,
            "get_element_bounds":       self._cmd_get_element_bounds,
            "get_focused_element":      self._cmd_get_focused_element,
            "read_selected_text":       self._cmd_read_selected_text,
            "snapshot_with_refs":       self._cmd_snapshot_with_refs,
            # App management
            "launch_app":               self._cmd_launch_app,
            "close_app":                self._cmd_close_app,
            # Window management
            "focus_window":             self._cmd_focus_window,
            "list_windows":             self._cmd_list_windows,
            "get_window_frame":         self._cmd_get_window_frame,
            "minimize_window":          self._cmd_minimize_window,
            "maximize_window":          self._cmd_maximize_window,
            "restore_window":           self._cmd_restore_window,
            "resize_window":            self._cmd_resize_window,
            "move_window":              self._cmd_move_window,
            # System
            "get_clipboard":            self._cmd_get_clipboard,
            "set_clipboard":            self._cmd_set_clipboard,
            "take_screenshot":          self._cmd_take_screenshot,
            # Notifications
            "list_notifications":       self._cmd_list_notifications,
            "dismiss_notification":     self._cmd_dismiss_notification,
            "perform_notification_action": self._cmd_perform_notification_action,
            # Legacy aliases
            "read_screen":              self._cmd_read_screen,
            "get_running_apps":         self._cmd_get_running_apps,
            "ax_snapshot":              self._cmd_snapshot_with_refs,
        }
        handler = handlers.get(action)
        if not handler:
            return {"success": False, "error": f"Unknown action: {action}"}
        try:
            return await handler(params)
        except Exception as e:
            logger.error("execute(%s) error: %s", action, e)
            return {"success": False, "error": str(e)}

    # ═════════════════════════════════════════════════════════════════════════
    # Original high-level API (kept for backwards compat)
    # ═════════════════════════════════════════════════════════════════════════

    async def get_running_apps(self) -> List[str]:
        if self._platform == "darwin":
            return await self._macos_running_apps()
        elif self._platform == "windows":
            return await self._windows_running_apps()
        elif self._platform == "linux":
            return await self._linux_running_apps()
        return []

    async def get_app_snapshot(self, app_name: str) -> AppSnapshot:
        if self._platform == "darwin":
            return await self._macos_snapshot(app_name)
        elif self._platform == "windows":
            return await self._windows_snapshot(app_name)
        elif self._platform == "linux":
            return await self._linux_snapshot(app_name)
        return AppSnapshot(app_name=app_name)

    async def find_element(self, description: str, app_name: Optional[str] = None) -> Optional[AppElement]:
        if app_name:
            snapshot = await self.get_app_snapshot(app_name)
            return snapshot.find(description)
        apps = await self.get_running_apps()
        if apps:
            snapshot = await self.get_app_snapshot(apps[0])
            return snapshot.find(description)
        return None

    async def click_element(self, element: AppElement) -> bool:
        center = element.center()
        if not center:
            return False
        return await self._click_at(center[0], center[1])

    async def read_screen(self, app_name: Optional[str] = None) -> str:
        apps = await self.get_running_apps()
        target = app_name or (apps[0] if apps else "")
        if not target:
            return "No running applications found"
        snapshot = await self.get_app_snapshot(target)
        return snapshot.to_text()

    async def capabilities(self) -> Dict:
        return {
            "adapter_id": self.ADAPTER_ID,
            "dom_tree": True,
            "vision_required": False,
            "code_execution": False,
            "file_access": False,
            "multi_tab": False,
            "platform": self._platform,
            "mobile": False,
        }

    async def health_check(self) -> bool:
        try:
            result = subprocess.run(
                ["osascript", "-e", "tell application \"System Events\" to return name of first process"],
                capture_output=True, timeout=3,
            )
            return result.returncode == 0
        except Exception:
            return False

    # ═════════════════════════════════════════════════════════════════════════
    # Mouse command handlers
    # ═════════════════════════════════════════════════════════════════════════

    async def _cmd_click(self, p: Dict) -> Dict:
        x, y = int(p.get("x", 0)), int(p.get("y", 0))
        ok = await self._click_at(x, y)
        return {"success": ok}

    async def _cmd_double_click(self, p: Dict) -> Dict:
        x, y = int(p.get("x", 0)), int(p.get("y", 0))
        ok = await self._double_click_at(x, y)
        return {"success": ok}

    async def _cmd_right_click(self, p: Dict) -> Dict:
        x, y = int(p.get("x", 0)), int(p.get("y", 0))
        ok = await self._right_click_at(x, y)
        return {"success": ok}

    async def _cmd_triple_click(self, p: Dict) -> Dict:
        x, y = int(p.get("x", 0)), int(p.get("y", 0))
        ok = await self._triple_click_at(x, y)
        return {"success": ok}

    async def _cmd_hover(self, p: Dict) -> Dict:
        x, y = int(p.get("x", 0)), int(p.get("y", 0))
        ok = await self._move_mouse(x, y)
        return {"success": ok}

    async def _cmd_drag(self, p: Dict) -> Dict:
        fx, fy = int(p.get("from_x", 0)), int(p.get("from_y", 0))
        tx, ty = int(p.get("to_x", 0)), int(p.get("to_y", 0))
        duration = float(p.get("duration", 0.5))
        ok = await self._drag(fx, fy, tx, ty, duration)
        return {"success": ok}

    async def _cmd_mouse_down(self, p: Dict) -> Dict:
        return {"success": self._mouse_button_event(p, down=True)}

    async def _cmd_mouse_up(self, p: Dict) -> Dict:
        return {"success": self._mouse_button_event(p, down=False)}

    async def _cmd_drag_element(self, p: Dict) -> Dict:
        ref = p.get("ref")
        tx, ty = int(p.get("to_x", 0)), int(p.get("to_y", 0))
        el = await self._resolve_ref(ref, p.get("app_name"))
        if not el:
            return {"success": False, "error": f"Element not found: {ref}"}
        center = el.center()
        if not center:
            return {"success": False, "error": "Element has no frame"}
        ok = await self._drag(center[0], center[1], tx, ty, float(p.get("duration", 0.5)))
        return {"success": ok}

    # ═════════════════════════════════════════════════════════════════════════
    # Keyboard command handlers
    # ═════════════════════════════════════════════════════════════════════════

    async def _cmd_type_text(self, p: Dict) -> Dict:
        text = str(p.get("text", ""))
        ok = await self._type_text(text)
        return {"success": ok}

    async def _cmd_clear_field(self, p: Dict) -> Dict:
        ok = await self._key_combo_str("cmd+a") and await self._type_text("")
        # Select all + delete
        ok = await self._key_combo_str("cmd+a")
        if ok:
            ok = await self._press_key_str("delete")
        return {"success": ok}

    async def _cmd_set_value(self, p: Dict) -> Dict:
        ref = p.get("ref")
        value = str(p.get("value", ""))
        if ref:
            el = await self._resolve_ref(ref, p.get("app_name"))
            if el:
                center = el.center()
                if center:
                    await self._click_at(center[0], center[1])
        # Clear then type
        await self._key_combo_str("cmd+a")
        ok = await self._type_text(value)
        return {"success": ok}

    async def _cmd_press_key(self, p: Dict) -> Dict:
        key = str(p.get("key", ""))
        ok = await self._press_key_str(key)
        return {"success": ok}

    async def _cmd_key_combo(self, p: Dict) -> Dict:
        combo = str(p.get("combo", ""))
        ok = await self._key_combo_str(combo)
        return {"success": ok}

    async def _cmd_key_down(self, p: Dict) -> Dict:
        key = str(p.get("key", ""))
        ok = await self._key_event(key, down=True)
        return {"success": ok}

    async def _cmd_key_up(self, p: Dict) -> Dict:
        key = str(p.get("key", ""))
        ok = await self._key_event(key, down=False)
        return {"success": ok}

    # ═════════════════════════════════════════════════════════════════════════
    # Scroll command handlers
    # ═════════════════════════════════════════════════════════════════════════

    async def _cmd_scroll(self, p: Dict) -> Dict:
        x, y = int(p.get("x", 0)), int(p.get("y", 0))
        dx = float(p.get("delta_x", 0))
        dy = float(p.get("delta_y", 3))
        ok = await self._scroll(x, y, dx, dy)
        return {"success": ok}

    async def _cmd_scroll_to_element(self, p: Dict) -> Dict:
        ref = p.get("ref")
        el = await self._resolve_ref(ref, p.get("app_name"))
        if not el:
            return {"success": False, "error": f"Element not found: {ref}"}
        center = el.center()
        if not center:
            return {"success": False, "error": "Element has no frame"}
        # Scroll up to bring element into view (simple heuristic)
        ok = await self._scroll(center[0], center[1], 0, -3)
        return {"success": ok}

    # ═════════════════════════════════════════════════════════════════════════
    # AX state command handlers
    # ═════════════════════════════════════════════════════════════════════════

    async def _cmd_toggle(self, p: Dict) -> Dict:
        ref = p.get("ref")
        el = await self._resolve_ref(ref, p.get("app_name"))
        if not el:
            return {"success": False, "error": f"Element not found: {ref}"}
        ok = await self._ax_perform_action(el, "AXPress") or await self.click_element(el)
        return {"success": ok}

    async def _cmd_check(self, p: Dict) -> Dict:
        ref = p.get("ref")
        el = await self._resolve_ref(ref, p.get("app_name"))
        if not el:
            return {"success": False, "error": f"Element not found: {ref}"}
        current_val = (el.value or "").lower()
        if current_val in ("1", "true", "checked"):
            return {"success": True, "note": "already checked"}
        ok = await self._ax_perform_action(el, "AXPress") or await self.click_element(el)
        return {"success": ok}

    async def _cmd_uncheck(self, p: Dict) -> Dict:
        ref = p.get("ref")
        el = await self._resolve_ref(ref, p.get("app_name"))
        if not el:
            return {"success": False, "error": f"Element not found: {ref}"}
        current_val = (el.value or "").lower()
        if current_val in ("0", "false", "unchecked", ""):
            return {"success": True, "note": "already unchecked"}
        ok = await self._ax_perform_action(el, "AXPress") or await self.click_element(el)
        return {"success": ok}

    async def _cmd_expand(self, p: Dict) -> Dict:
        ref = p.get("ref")
        el = await self._resolve_ref(ref, p.get("app_name"))
        if not el:
            return {"success": False, "error": f"Element not found: {ref}"}
        ok = await self._ax_perform_action(el, "AXShowMenu") or \
             await self._ax_perform_action(el, "AXPress") or \
             await self.click_element(el)
        return {"success": ok}

    async def _cmd_collapse(self, p: Dict) -> Dict:
        ref = p.get("ref")
        el = await self._resolve_ref(ref, p.get("app_name"))
        if not el:
            return {"success": False, "error": f"Element not found: {ref}"}
        ok = await self._ax_perform_action(el, "AXCancel") or \
             await self._press_key_str("escape") or \
             await self.click_element(el)
        return {"success": ok}

    async def _cmd_focus(self, p: Dict) -> Dict:
        ref = p.get("ref")
        el = await self._resolve_ref(ref, p.get("app_name"))
        if not el:
            return {"success": False, "error": f"Element not found: {ref}"}
        ok = await self.click_element(el)
        return {"success": ok}

    # ═════════════════════════════════════════════════════════════════════════
    # Element discovery command handlers
    # ═════════════════════════════════════════════════════════════════════════

    async def _cmd_find_elements(self, p: Dict) -> Dict:
        query = str(p.get("query", ""))
        app_name = p.get("app_name")
        role_filter = p.get("role")
        apps = await self.get_running_apps()
        target = app_name or (apps[0] if apps else "")
        if not target:
            return {"success": False, "error": "No running app"}
        snap = await self.get_app_snapshot(target)
        results = []
        for el in snap.elements:
            if role_filter and el.role.lower() != str(role_filter).lower():
                continue
            if query and not el.matches(query):
                continue
            results.append(el.to_dict())
        return {"success": True, "elements": results, "count": len(results)}

    async def _cmd_get_element_bounds(self, p: Dict) -> Dict:
        ref = p.get("ref")
        el = await self._resolve_ref(ref, p.get("app_name"))
        if not el:
            return {"success": False, "error": f"Element not found: {ref}"}
        return {"success": True, "frame": el.frame, "center": el.center()}

    async def _cmd_get_focused_element(self, _p: Dict) -> Dict:
        if self._platform == "darwin":
            info = await asyncio.get_event_loop().run_in_executor(None, self._macos_focused_element)
            if info:
                return {"success": True, **info}
        return {"success": False, "error": "Could not determine focused element"}

    async def _cmd_read_selected_text(self, _p: Dict) -> Dict:
        try:
            result = subprocess.run(
                ["osascript", "-e",
                 'tell application "System Events" to keystroke "c" using command down'],
                capture_output=True, timeout=3,
            )
            await asyncio.sleep(0.15)
            text = await self._cmd_get_clipboard({})
            return {"success": True, "text": text.get("text", "")}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def _cmd_snapshot_with_refs(self, p: Dict) -> Dict:
        """Return AX tree with deterministic @e1/@e2 refs and optional skeleton/diff modes.

        Params:
          skeleton  (bool, default True)  – limit tree depth for fast scans
          window_id (int,  optional)      – capture a specific window by CGWindowID
          diff      (bool, default False) – also return a change-annotated diff vs
                                            the previous snapshot for this window_id
        """
        from ...core.accessibility_inspector import AccessibilityInspector
        from ...core.element_refs import get_refmap

        inspector = AccessibilityInspector()
        skeleton = bool(p.get("skeleton", True))
        window_id = p.get("window_id")
        want_diff = bool(p.get("diff", False))

        try:
            if want_diff:
                wid = int(window_id) if window_id is not None else None
                tree, diff_result = await inspector.capture_diff(window_id=wid, skeleton=skeleton)
            elif window_id:
                tree = await inspector.capture_window(int(window_id), skeleton=skeleton)
                diff_result = None
            else:
                tree = await inspector.capture_focused(skeleton=skeleton)
                diff_result = None

            if not tree:
                return {"success": False, "error": "Could not capture AX tree"}

            refmap = get_refmap()
            assigned = refmap.assign_refs(tree)
            await refmap.persist()

            response: Dict[str, Any] = {
                "success": True,
                "tree": tree.to_dict() if hasattr(tree, "to_dict") else tree,
                "ref_map": assigned,
                "surface": getattr(tree, "surface", "window"),
            }

            if want_diff and diff_result is not None:
                # Summarise change counts before serialising
                counts = _count_changes(diff_result)
                response["diff"] = {
                    "tree": diff_result.to_dict() if hasattr(diff_result, "to_dict") else diff_result,
                    "counts": counts,
                }

            return response

        except ImportError:
            # Fallback: return basic snapshot from this adapter
            apps = await self.get_running_apps()
            if not apps:
                return {"success": False, "error": "No running apps"}
            snap = await self.get_app_snapshot(apps[0])
            return {"success": True, "tree": {"role": "App", "children": [e.to_dict() for e in snap.elements[:50]]}}

    # ═════════════════════════════════════════════════════════════════════════
    # App management command handlers
    # ═════════════════════════════════════════════════════════════════════════

    async def _cmd_get_running_apps(self, _p: Dict) -> Dict:
        apps = await self.get_running_apps()
        return {"success": True, "apps": apps}

    async def _cmd_launch_app(self, p: Dict) -> Dict:
        name = str(p.get("name", ""))
        bundle_id = str(p.get("bundle_id", ""))
        if self._platform == "darwin":
            ok = await asyncio.get_event_loop().run_in_executor(
                None, self._macos_launch_app, name, bundle_id
            )
            return {"success": ok}
        return {"success": False, "error": "launch_app not supported on this platform"}

    async def _cmd_close_app(self, p: Dict) -> Dict:
        name = str(p.get("name", ""))
        if self._platform == "darwin":
            try:
                script = f'tell application "{name}" to quit'
                result = subprocess.run(["osascript", "-e", script], capture_output=True, timeout=5)
                return {"success": result.returncode == 0}
            except Exception as e:
                return {"success": False, "error": str(e)}
        return {"success": False, "error": "close_app not supported on this platform"}

    # ═════════════════════════════════════════════════════════════════════════
    # Window management command handlers
    # ═════════════════════════════════════════════════════════════════════════

    async def _cmd_focus_window(self, p: Dict) -> Dict:
        window_id = p.get("window_id")
        app_name = p.get("app_name", "")
        if self._platform == "darwin":
            ok = await asyncio.get_event_loop().run_in_executor(
                None, self._macos_focus_window, window_id, app_name
            )
            return {"success": ok}
        return {"success": False, "error": "focus_window not supported on this platform"}

    async def _cmd_list_windows(self, _p: Dict) -> Dict:
        if self._platform == "darwin":
            windows = await asyncio.get_event_loop().run_in_executor(None, self._macos_list_windows)
            return {"success": True, "windows": windows}
        return {"success": True, "windows": []}

    async def _cmd_get_window_frame(self, p: Dict) -> Dict:
        window_id = int(p.get("window_id", 0))
        if self._platform == "darwin":
            frame = await asyncio.get_event_loop().run_in_executor(
                None, self._macos_get_window_frame, window_id
            )
            return {"success": frame is not None, "frame": frame}
        return {"success": False, "error": "get_window_frame not supported on this platform"}

    async def _cmd_minimize_window(self, p: Dict) -> Dict:
        app_name = str(p.get("app_name", ""))
        if self._platform == "darwin":
            script = f'tell application "System Events" to set miniaturized of window 1 of process "{app_name}" to true'
            try:
                result = subprocess.run(["osascript", "-e", script], capture_output=True, timeout=5)
                return {"success": result.returncode == 0}
            except Exception as e:
                return {"success": False, "error": str(e)}
        return {"success": False, "error": "Not supported"}

    async def _cmd_maximize_window(self, p: Dict) -> Dict:
        app_name = str(p.get("app_name", ""))
        if self._platform == "darwin":
            script = f'''tell application "System Events"
    set win to window 1 of process "{app_name}"
    set {"{"}class of win{"}"} to window
    keystroke "f" using {{control down, command down}}
end tell'''
            try:
                result = subprocess.run(["osascript", "-e", script], capture_output=True, timeout=5)
                return {"success": result.returncode == 0}
            except Exception as e:
                return {"success": False, "error": str(e)}
        return {"success": False, "error": "Not supported"}

    async def _cmd_restore_window(self, p: Dict) -> Dict:
        app_name = str(p.get("app_name", ""))
        if self._platform == "darwin":
            script = f'tell application "System Events" to set miniaturized of window 1 of process "{app_name}" to false'
            try:
                result = subprocess.run(["osascript", "-e", script], capture_output=True, timeout=5)
                return {"success": result.returncode == 0}
            except Exception as e:
                return {"success": False, "error": str(e)}
        return {"success": False, "error": "Not supported"}

    async def _cmd_resize_window(self, p: Dict) -> Dict:
        app_name = str(p.get("app_name", ""))
        width = int(p.get("width", 800))
        height = int(p.get("height", 600))
        if self._platform == "darwin":
            script = f'tell application "System Events" to set size of window 1 of process "{app_name}" to {{{width}, {height}}}'
            try:
                result = subprocess.run(["osascript", "-e", script], capture_output=True, timeout=5)
                return {"success": result.returncode == 0}
            except Exception as e:
                return {"success": False, "error": str(e)}
        return {"success": False, "error": "Not supported"}

    async def _cmd_move_window(self, p: Dict) -> Dict:
        app_name = str(p.get("app_name", ""))
        x = int(p.get("x", 0))
        y = int(p.get("y", 0))
        if self._platform == "darwin":
            script = f'tell application "System Events" to set position of window 1 of process "{app_name}" to {{{x}, {y}}}'
            try:
                result = subprocess.run(["osascript", "-e", script], capture_output=True, timeout=5)
                return {"success": result.returncode == 0}
            except Exception as e:
                return {"success": False, "error": str(e)}
        return {"success": False, "error": "Not supported"}

    # ═════════════════════════════════════════════════════════════════════════
    # Clipboard command handlers
    # ═════════════════════════════════════════════════════════════════════════

    async def _cmd_get_clipboard(self, _p: Dict) -> Dict:
        try:
            if self._platform == "darwin":
                result = subprocess.run(["pbpaste"], capture_output=True, text=True, timeout=3)
                return {"success": True, "text": result.stdout}
            elif self._platform == "linux":
                result = subprocess.run(["xclip", "-selection", "clipboard", "-o"],
                                        capture_output=True, text=True, timeout=3)
                return {"success": True, "text": result.stdout}
            elif self._platform == "windows":
                result = subprocess.run(["powershell", "-command", "Get-Clipboard"],
                                        capture_output=True, text=True, timeout=3)
                return {"success": True, "text": result.stdout.strip()}
        except Exception as e:
            return {"success": False, "error": str(e)}
        return {"success": False, "error": "Not supported"}

    async def _cmd_set_clipboard(self, p: Dict) -> Dict:
        text = str(p.get("text", ""))
        try:
            if self._platform == "darwin":
                proc = subprocess.run(["pbcopy"], input=text.encode(), timeout=3)
                return {"success": proc.returncode == 0}
            elif self._platform == "linux":
                proc = subprocess.run(["xclip", "-selection", "clipboard"],
                                      input=text.encode(), timeout=3)
                return {"success": proc.returncode == 0}
            elif self._platform == "windows":
                proc = subprocess.run(["powershell", "-command", f"Set-Clipboard -Value '{text}'"],
                                      capture_output=True, timeout=3)
                return {"success": proc.returncode == 0}
        except Exception as e:
            return {"success": False, "error": str(e)}
        return {"success": False, "error": "Not supported"}

    # ═════════════════════════════════════════════════════════════════════════
    # Screenshot command handler
    # ═════════════════════════════════════════════════════════════════════════

    async def _cmd_take_screenshot(self, p: Dict) -> Dict:
        b64 = await self._take_screenshot(window_id=p.get("window_id"))
        return {"success": b64 is not None, "image_b64": b64}

    async def _take_screenshot(self, window_id: Optional[int] = None) -> Optional[str]:
        """Capture screenshot, return base64 PNG."""
        try:
            if self._platform == "darwin":
                import tempfile, os
                with tempfile.NamedTemporaryFile(suffix=".png", delete=False) as f:
                    tmp = f.name
                cmd = ["screencapture", "-x", "-t", "png"]
                if window_id:
                    cmd += ["-l", str(window_id)]
                cmd.append(tmp)
                result = subprocess.run(cmd, capture_output=True, timeout=10)
                if result.returncode == 0 and os.path.exists(tmp):
                    with open(tmp, "rb") as f:
                        data = f.read()
                    os.unlink(tmp)
                    return base64.b64encode(data).decode()
        except Exception as e:
            logger.warning("_take_screenshot failed: %s", e)
        return None

    # ═════════════════════════════════════════════════════════════════════════
    # Notification command handlers
    # ═════════════════════════════════════════════════════════════════════════

    async def _cmd_list_notifications(self, _p: Dict) -> Dict:
        try:
            from ...core.accessibility_inspector import AccessibilityInspector
            inspector = AccessibilityInspector()
            notifs = await inspector.list_notifications()
            return {"success": True, "notifications": [n.to_dict() if hasattr(n, "to_dict") else n for n in notifs]}
        except Exception as e:
            return {"success": False, "error": str(e), "notifications": []}

    async def _cmd_dismiss_notification(self, p: Dict) -> Dict:
        notif_id = str(p.get("notification_id", ""))
        try:
            from ...core.accessibility_inspector import AccessibilityInspector
            inspector = AccessibilityInspector()
            ok = await inspector.dismiss_notification(notif_id)
            return {"success": ok}
        except Exception as e:
            return {"success": False, "error": str(e)}

    async def _cmd_perform_notification_action(self, p: Dict) -> Dict:
        notif_id = str(p.get("notification_id", ""))
        action = str(p.get("action", ""))
        try:
            from ...core.accessibility_inspector import AccessibilityInspector
            inspector = AccessibilityInspector()
            ok = await inspector.perform_notification_action(notif_id, action)
            return {"success": ok}
        except Exception as e:
            return {"success": False, "error": str(e)}

    # ═════════════════════════════════════════════════════════════════════════
    # Legacy alias handlers
    # ═════════════════════════════════════════════════════════════════════════

    async def _cmd_read_screen(self, p: Dict) -> Dict:
        text = await self.read_screen(p.get("app_name"))
        return {"success": True, "text": text}

    # ═════════════════════════════════════════════════════════════════════════
    # Low-level macOS input primitives — delegated to BackgroundEventPoster
    # ═════════════════════════════════════════════════════════════════════════

    async def _click_at(self, x: int, y: int) -> bool:
        if _POSTER_AVAILABLE:
            result = await _get_poster().post_click(x, y, button="left")
            return result.success
        return False

    async def _double_click_at(self, x: int, y: int) -> bool:
        if _POSTER_AVAILABLE:
            result = await _get_poster().post_double_click(x, y)
            return result.success
        return False

    async def _right_click_at(self, x: int, y: int) -> bool:
        if _POSTER_AVAILABLE:
            result = await _get_poster().post_right_click(x, y)
            return result.success
        return False

    async def _triple_click_at(self, x: int, y: int) -> bool:
        """Three rapid left clicks — poster handles each click, we sequence them."""
        for _ in range(3):
            ok = await self._click_at(x, y)
            if not ok:
                return False
            await asyncio.sleep(0.05)
        return True

    async def _move_mouse(self, x: int, y: int) -> bool:
        if _POSTER_AVAILABLE:
            result = await _get_poster().move_cursor(x, y)
            return result.success
        return False

    async def _drag(self, fx: int, fy: int, tx: int, ty: int, duration: float = 0.5) -> bool:
        if _POSTER_AVAILABLE:
            # post_drag takes duration_ms (int), convert from seconds
            result = await _get_poster().post_drag(fx, fy, tx, ty, duration_ms=int(duration * 1000))
            return result.success
        return False

    async def _scroll(self, x: int, y: int, dx: float, dy: float) -> bool:
        if _POSTER_AVAILABLE:
            result = await _get_poster().post_scroll(x, y, dx, dy)
            return result.success
        return False

    async def _type_text(self, text: str) -> bool:
        if _POSTER_AVAILABLE:
            result = await _get_poster().post_type(text)
            return result.success
        return False

    async def _press_key_str(self, key: str) -> bool:
        if _POSTER_AVAILABLE:
            key_lower = key.lower().strip()
            code = _KEY_CODES.get(key_lower)
            if code is None:
                logger.warning("Unknown key: %s", key)
                return False
            result = await _get_poster().post_key(code)
            return result.success
        return False

    async def _key_combo_str(self, combo: str) -> bool:
        if _POSTER_AVAILABLE:
            result = await _get_poster().post_key_combo(combo)
            return result.success
        return False

    def cursor_position(self) -> Optional[Tuple[int, int]]:
        """Current OS cursor position in global display points (macOS Quartz)."""
        if self._platform != "darwin":
            return None
        try:
            import Quartz  # type: ignore
            loc = Quartz.CGEventGetLocation(Quartz.CGEventCreate(None))
            return int(loc.x), int(loc.y)
        except Exception:
            return None

    def screen_size(self) -> Optional[Tuple[int, int]]:
        """Main display size in points (macOS Quartz)."""
        if self._platform != "darwin":
            return None
        try:
            import Quartz  # type: ignore
            bounds = Quartz.CGDisplayBounds(Quartz.CGMainDisplayID())
            return int(bounds.size.width), int(bounds.size.height)
        except Exception:
            return None

    def _mouse_button_event(self, p: Dict, down: bool) -> bool:
        """Post an isolated left-button down/up at (x, y) or the current cursor."""
        if self._platform != "darwin":
            return False
        x, y = p.get("x"), p.get("y")
        if x is None or y is None:
            pos = self.cursor_position()
            if pos is None:
                return False
            x, y = pos
        try:
            import Quartz  # type: ignore
            kind = Quartz.kCGEventLeftMouseDown if down else Quartz.kCGEventLeftMouseUp
            ev = Quartz.CGEventCreateMouseEvent(
                None, kind, (float(x), float(y)), Quartz.kCGMouseButtonLeft
            )
            Quartz.CGEventPost(Quartz.kCGHIDEventTap, ev)
            return True
        except Exception:
            return False

    async def _key_event(self, key: str, down: bool) -> bool:
        """Post a single key-down or key-up event.
        BackgroundEventPoster only supports down+up pairs; for isolated key-down/up
        we fall back to a direct Quartz call when available, else no-op."""
        code = _KEY_CODES.get(key.lower().strip())
        if code is None:
            return False
        if self._platform == "darwin":
            try:
                import Quartz  # type: ignore
                src = Quartz.CGEventSourceCreate(Quartz.kCGEventSourceStateHIDSystemState)
                ev = Quartz.CGEventCreateKeyboardEvent(src, code, down)
                Quartz.CGEventPost(Quartz.kCGHIDEventTap, ev)
                return True
            except Exception:
                pass
        return False

    # ═════════════════════════════════════════════════════════════════════════
    # AX action helper
    # ═════════════════════════════════════════════════════════════════════════

    async def _ax_perform_action(self, el: AppElement, action: str) -> bool:
        """Perform an AX action on an element by re-locating it via pyobjc."""
        if self._platform != "darwin":
            return False
        center = el.center()
        if not center:
            return False
        return await asyncio.get_event_loop().run_in_executor(
            None, self._macos_ax_action_at, center[0], center[1], action
        )

    def _macos_ax_action_at(self, x: int, y: int, action: str) -> bool:
        try:
            import ApplicationServices  # type: ignore
            el = ApplicationServices.AXUIElementCreateSystemWide()
            result = ApplicationServices.AXUIElementCopyElementAtPosition(el, float(x), float(y), None)
            if isinstance(result, tuple):
                err, target = result
            else:
                err, target = 0, result
            if err != 0 or target is None:
                return False
            ApplicationServices.AXUIElementPerformAction(target, action)
            return True
        except Exception as e:
            logger.debug("_macos_ax_action_at failed: %s", e)
            return False

    async def _resolve_ref(self, ref: Optional[str], app_name: Optional[str]) -> Optional[AppElement]:
        """Resolve a @eN ref or description string to an AppElement."""
        if not ref:
            return None

        # Try RefMap first if ref starts with @
        if ref.startswith("@"):
            try:
                from ...core.element_refs import get_refmap
                refmap = get_refmap()
                entry = refmap.get(ref)
                if entry:
                    # entry has role, name, center coords
                    frame: Dict[str, float] = {}
                    if "center" in entry:
                        cx, cy = entry["center"]
                        frame = {"x": cx - 5, "y": cy - 5, "width": 10, "height": 10}
                    return AppElement(
                        role=entry.get("role", ""),
                        title=entry.get("name", ""),
                        frame=frame or None,
                        ref_id=ref,
                    )
            except Exception:
                pass

        # Fallback: find by description
        return await self.find_element(ref, app_name)

    # ═════════════════════════════════════════════════════════════════════════
    # macOS platform implementations
    # ═════════════════════════════════════════════════════════════════════════

    async def _macos_running_apps(self) -> List[str]:
        try:
            script = 'tell application "System Events" to get name of every process whose background only is false'
            result = subprocess.run(["osascript", "-e", script], capture_output=True, text=True, timeout=5)
            if result.returncode == 0:
                return [a.strip() for a in result.stdout.strip().split(",")]
        except Exception as e:
            logger.warning("macOS app list failed: %s", e)
        return []

    async def _macos_snapshot(self, app_name: str) -> AppSnapshot:
        elements = []
        try:
            elements = await self._macos_snapshot_pyobjc(app_name)
        except Exception:
            try:
                elements = await self._macos_snapshot_applescript(app_name)
            except Exception as e:
                logger.warning("macOS accessibility snapshot failed: %s", e)
        return AppSnapshot(app_name=app_name, elements=elements)

    async def _macos_snapshot_pyobjc(self, app_name: str) -> List[AppElement]:
        try:
            import AppKit  # type: ignore
            import ApplicationServices  # type: ignore

            running = AppKit.NSWorkspace.sharedWorkspace().runningApplications()
            target_app = next((a for a in running if a.localizedName() == app_name), None)
            if not target_app:
                return []

            pid = target_app.processIdentifier()
            ax_app = ApplicationServices.AXUIElementCreateApplication(pid)
            elements: List[AppElement] = []
            self._traverse_ax(ax_app, elements, depth=0, max_depth=4)
            return elements
        except ImportError:
            raise

    def _ax_attr(self, element: Any, attr: str) -> Any:
        try:
            import ApplicationServices  # type: ignore
            result = ApplicationServices.AXUIElementCopyAttributeValue(element, attr, None)
            if isinstance(result, tuple):
                err, val = result
                return val if err == 0 else None
            return result
        except Exception:
            return None

    def _traverse_ax(self, element: Any, result: List[AppElement], depth: int, max_depth: int) -> None:
        if depth > max_depth:
            return
        try:
            role = self._ax_attr(element, "AXRole") or ""
            title = self._ax_attr(element, "AXTitle") or ""
            value = self._ax_attr(element, "AXValue") or ""
            frame_val = self._ax_attr(element, "AXFrame")
            frame = None
            if frame_val:
                try:
                    frame = {"x": frame_val.origin.x, "y": frame_val.origin.y,
                             "width": frame_val.size.width, "height": frame_val.size.height}
                except Exception:
                    pass
            is_interactive = str(role) in self._INTERACTIVE_ROLES
            el = AppElement(role=str(role), title=str(title), value=str(value),
                            frame=frame, is_interactive=is_interactive)
            if el.role and el.role not in ("", "None"):
                result.append(el)
            children = self._ax_attr(element, "AXChildren") or []
            for child in children:
                self._traverse_ax(child, result, depth + 1, max_depth)
        except Exception:
            pass

    async def _macos_snapshot_applescript(self, app_name: str) -> List[AppElement]:
        script = f'''
tell application "System Events"
    tell process "{app_name}"
        set uiElements to every UI element of window 1
        set elementList to {{}}
        repeat with el in uiElements
            set elementList to elementList & {{role of el & ": " & (name of el as string)}}
        end repeat
        return elementList
    end tell
end tell'''
        result = subprocess.run(["osascript", "-e", script], capture_output=True, text=True, timeout=10)
        if result.returncode == 0:
            elements = []
            for line in result.stdout.strip().split(","):
                line = line.strip()
                if ": " in line:
                    role, name = line.split(": ", 1)
                    elements.append(AppElement(role=role.strip(), title=name.strip()))
            return elements
        return []

    def _macos_focused_element(self) -> Optional[Dict]:
        try:
            import ApplicationServices  # type: ignore
            sys_el = ApplicationServices.AXUIElementCreateSystemWide()
            result = ApplicationServices.AXUIElementCopyAttributeValue(sys_el, "AXFocusedUIElement", None)
            if isinstance(result, tuple):
                err, el = result
            else:
                err, el = 0, result
            if err != 0 or el is None:
                return None
            role = self._ax_attr(el, "AXRole") or ""
            title = self._ax_attr(el, "AXTitle") or ""
            value = self._ax_attr(el, "AXValue") or ""
            frame_val = self._ax_attr(el, "AXFrame")
            frame = None
            if frame_val:
                try:
                    frame = {"x": frame_val.origin.x, "y": frame_val.origin.y,
                             "width": frame_val.size.width, "height": frame_val.size.height}
                except Exception:
                    pass
            return {"role": str(role), "title": str(title), "value": str(value), "frame": frame}
        except Exception as e:
            logger.debug("_macos_focused_element failed: %s", e)
            return None

    def _macos_launch_app(self, name: str, bundle_id: str) -> bool:
        try:
            import AppKit  # type: ignore
            ws = AppKit.NSWorkspace.sharedWorkspace()
            if bundle_id:
                ok = ws.launchApplication_(bundle_id)
                if ok:
                    return True
            if name:
                ok = ws.launchApplication_(name)
                return bool(ok)
        except Exception:
            pass
        try:
            result = subprocess.run(["open", "-a", name or bundle_id], capture_output=True, timeout=10)
            return result.returncode == 0
        except Exception:
            return False

    def _macos_focus_window(self, window_id: Optional[int], app_name: str) -> bool:
        try:
            import AppKit  # type: ignore
            if app_name:
                running = AppKit.NSWorkspace.sharedWorkspace().runningApplications()
                for app in running:
                    if app.localizedName() == app_name:
                        app.activateWithOptions_(AppKit.NSApplicationActivateIgnoringOtherApps)
                        return True
        except Exception as e:
            logger.debug("_macos_focus_window failed: %s", e)
        if app_name:
            try:
                script = f'tell application "{app_name}" to activate'
                result = subprocess.run(["osascript", "-e", script], capture_output=True, timeout=5)
                return result.returncode == 0
            except Exception:
                pass
        return False

    def _macos_list_windows(self) -> List[Dict]:
        try:
            import Quartz  # type: ignore
            windows = Quartz.CGWindowListCopyWindowInfo(
                Quartz.kCGWindowListOptionOnScreenOnly | Quartz.kCGWindowListExcludeDesktopElements,
                Quartz.kCGNullWindowID)
            result = []
            if windows:
                for w in windows:
                    if w.get("kCGWindowLayer", 0) != 0:
                        continue
                    bounds = w.get("kCGWindowBounds", {})
                    result.append({
                        "window_id": w.get("kCGWindowNumber", 0),
                        "title": w.get("kCGWindowName", ""),
                        "app": w.get("kCGWindowOwnerName", ""),
                        "frame": {
                            "x": float(bounds.get("X", 0)),
                            "y": float(bounds.get("Y", 0)),
                            "width": float(bounds.get("Width", 0)),
                            "height": float(bounds.get("Height", 0)),
                        },
                    })
            return result
        except Exception as e:
            logger.debug("_macos_list_windows error: %s", e)
        return []

    def _macos_get_window_frame(self, window_id: int) -> Optional[Dict]:
        try:
            import Quartz  # type: ignore
            windows = Quartz.CGWindowListCopyWindowInfo(
                Quartz.kCGWindowListOptionAll, Quartz.kCGNullWindowID)
            if not windows:
                return None
            for w in windows:
                if w.get("kCGWindowNumber") == window_id:
                    bounds = w.get("kCGWindowBounds", {})
                    return {
                        "x": float(bounds.get("X", 0)),
                        "y": float(bounds.get("Y", 0)),
                        "width": float(bounds.get("Width", 0)),
                        "height": float(bounds.get("Height", 0)),
                    }
        except Exception as e:
            logger.debug("_macos_get_window_frame error: %s", e)
        return None

    # ═════════════════════════════════════════════════════════════════════════
    # Windows platform implementations
    # ═════════════════════════════════════════════════════════════════════════

    async def _windows_running_apps(self) -> List[str]:
        try:
            import comtypes.client  # type: ignore
            import comtypes.gen.UIAutomationClient as uia  # type: ignore
            automation = comtypes.client.CreateObject("{ff48dba4-60ef-4201-aa87-54103eef594e}", interface=uia.IUIAutomation)
            root = automation.GetRootElement()
            condition = automation.CreatePropertyCondition(uia.UIA_IsEnabledPropertyId, True)
            children = root.FindAll(uia.TreeScope_Children, condition)
            return [children.GetElement(i).CurrentName for i in range(children.Length)]
        except Exception:
            return []

    async def _windows_snapshot(self, app_name: str) -> AppSnapshot:
        elements = []
        try:
            import comtypes.client  # type: ignore
            import comtypes.gen.UIAutomationClient as uia  # type: ignore
            automation = comtypes.client.CreateObject("{ff48dba4-60ef-4201-aa87-54103eef594e}", interface=uia.IUIAutomation)
            root = automation.GetRootElement()
            name_condition = automation.CreatePropertyCondition(uia.UIA_NamePropertyId, app_name)
            app_el = root.FindFirst(uia.TreeScope_Children, name_condition)
            if app_el:
                all_condition = automation.CreateTrueCondition()
                children = app_el.FindAll(uia.TreeScope_Subtree, all_condition)
                for i in range(min(children.Length, 200)):
                    child = children.GetElement(i)
                    try:
                        rect = child.CurrentBoundingRectangle
                        frame = {"x": rect.left, "y": rect.top,
                                 "width": rect.right - rect.left, "height": rect.bottom - rect.top}
                        el = AppElement(
                            role=str(child.CurrentControlType),
                            title=str(child.CurrentName or ""),
                            value=str(child.CurrentName or ""),
                            frame=frame,
                        )
                        elements.append(el)
                    except Exception:
                        continue
        except Exception as e:
            logger.warning("Windows accessibility snapshot failed: %s", e)
        return AppSnapshot(app_name=app_name, elements=elements)

    # ═════════════════════════════════════════════════════════════════════════
    # Linux AT-SPI implementations
    # ═════════════════════════════════════════════════════════════════════════

    async def _linux_running_apps(self) -> List[str]:
        try:
            result = subprocess.run(["wmctrl", "-l"], capture_output=True, text=True, timeout=5)
            if result.returncode == 0:
                apps = []
                for line in result.stdout.splitlines():
                    parts = line.split(None, 3)
                    if len(parts) >= 4:
                        apps.append(parts[3].strip())
                return list(dict.fromkeys(apps))
        except FileNotFoundError:
            logger.warning("Linux: wmctrl not found — install with: sudo apt-get install wmctrl")
        except Exception as e:
            logger.warning("Linux app list failed: %s", e)
        return []

    async def _linux_snapshot(self, app_name: str) -> AppSnapshot:
        elements: List[AppElement] = []
        try:
            import pyatspi  # type: ignore
            desktop = pyatspi.Registry.getDesktop(0)
            for app in desktop:
                if app is None:
                    continue
                if app_name.lower() not in (app.name or "").lower():
                    continue
                for win in app:
                    if win is None:
                        continue
                    self._traverse_atspi(win, elements, depth=0, max_depth=4)
                break
        except ImportError:
            logger.warning(
                "Linux: pyatspi not installed — install with: pip install pyatspi. "
                "AT-SPI accessibility requires the at-spi2-core service to be running."
            )
        except Exception as e:
            logger.warning("Linux AT-SPI snapshot failed: %s", e)
        return AppSnapshot(app_name=app_name, elements=elements)

    def _traverse_atspi(self, node: Any, result: List[AppElement], depth: int, max_depth: int) -> None:
        if depth > max_depth or node is None:
            return
        try:
            role = node.getRoleName() or ""
            name = node.name or ""
            try:
                comp = node.queryComponent()
                ext = comp.getExtents(0)
                frame: Optional[Dict[str, float]] = {
                    "x": float(ext.x), "y": float(ext.y),
                    "width": float(ext.width), "height": float(ext.height),
                }
            except Exception:
                frame = None
            el = AppElement(role=role, title=name, frame=frame)
            if role and role not in ("", "unknown"):
                result.append(el)
            for i in range(node.childCount):
                self._traverse_atspi(node.getChildAtIndex(i), result, depth + 1, max_depth)
        except Exception:
            pass
