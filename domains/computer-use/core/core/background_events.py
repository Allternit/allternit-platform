"""
Allternit Computer Use — Background Event Poster

Non-invasive macOS event posting using Quartz CGEvent APIs via pyobjc.
Posts clicks, keys, scroll events directly to target processes without:
  - Stealing window focus
  - Moving the visible mouse cursor
  - Triggering focus-change handlers

Inspired by background-computer-use's NativeBackgroundClickTransport (SkyLight approach).
Without Quartz, posting reports failure (no foreground fallback).

SkyLight / SLEventPostToPid
---------------------------
When a target PID is provided and SkyLight is available, events are posted
directly to that process via SLEventPostToPid instead of the session-level
kCGHIDEventTap / kCGSessionEventTap tap. This enables true per-process
background delivery without moving the visible cursor or stealing focus.

SLEventPostToPid requires the com.apple.private.skylight entitlement on
hardened runtime targets. Works without entitlement on development builds.
"""

from __future__ import annotations

import asyncio
import ctypes
import ctypes.util
import logging
import platform
import time
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Dict, List, Optional, Tuple

logger = logging.getLogger(__name__)

_IS_DARWIN = platform.system() == "Darwin"

# ---------------------------------------------------------------------------
# Optional imports
# ---------------------------------------------------------------------------

_QUARTZ_AVAILABLE = False

if _IS_DARWIN:
    try:
        import Quartz
        import Quartz.CoreGraphics as CG
        _QUARTZ_AVAILABLE = True
    except ImportError:
        pass


# ---------------------------------------------------------------------------
# SkyLight private framework loader
# ---------------------------------------------------------------------------

def _load_skylight() -> Optional[ctypes.CDLL]:
    """
    Attempt to load the SkyLight private framework and resolve SLEventPostToPid.

    SLEventPostToPid(CGEventRef event, pid_t pid) -> void

    Returns the loaded library on success, None if unavailable or load fails.
    SLEventPostToPid requires the com.apple.private.skylight entitlement on
    hardened runtime targets. Works without entitlement on development builds.
    """
    if not _IS_DARWIN:
        return None
    path = "/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight"
    try:
        lib = ctypes.CDLL(path)
        # SLEventPostToPid(CGEventRef event, pid_t pid) -> void
        lib.SLEventPostToPid.restype = None
        lib.SLEventPostToPid.argtypes = [ctypes.c_void_p, ctypes.c_int32]
        return lib
    except Exception:
        return None


_SKYLIGHT: Optional[ctypes.CDLL] = _load_skylight()


# ---------------------------------------------------------------------------
# Post helper — SkyLight or HID tap fallback
# ---------------------------------------------------------------------------

def _post_event(event: Any, pid: Optional[int] = None) -> None:
    """
    Post a CGEvent either to a specific PID (via SLEventPostToPid) or to the
    session HID tap.  Falls back to kCGHIDEventTap when pid is None or when
    SkyLight is unavailable.
    """
    if pid is not None and _SKYLIGHT is not None:
        # Obtain the raw CFTypeRef pointer via objc runtime so ctypes can pass
        # it as c_void_p.  pyobjc CGEventRef objects expose __c_void_p__().
        try:
            raw_ptr = event.__c_void_p__()
            _SKYLIGHT.SLEventPostToPid(raw_ptr, ctypes.c_int32(pid))
            return
        except Exception as exc:
            logger.debug("[EventPoster] SLEventPostToPid failed, falling back: %s", exc)
    # Default: session-level event tap (existing behaviour)
    CG.CGEventPost(CG.kCGSessionEventTap, event)


# ---------------------------------------------------------------------------
# Enums and result types
# ---------------------------------------------------------------------------

class EventPostMode(Enum):
    BACKGROUND_QUARTZ = "background_quartz"
    BACKGROUND = "background"
    FOREGROUND_APPLESCRIPT = "foreground_applescript"


@dataclass
class EventResult:
    success: bool
    mode_used: EventPostMode
    notes: str = ""


# ---------------------------------------------------------------------------
# Key and modifier maps
# ---------------------------------------------------------------------------

# macOS virtual key codes (US QWERTY)
_KEY_MAP: Dict[str, int] = {
    # Letters
    "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7,
    "c": 8, "v": 9, "b": 11, "q": 12, "w": 13, "e": 14, "r": 15,
    "y": 16, "t": 17, "1": 18, "2": 19, "3": 20, "4": 21, "6": 22,
    "5": 23, "=": 24, "9": 25, "7": 26, "-": 27, "8": 28, "0": 29,
    "]": 30, "o": 31, "u": 32, "[": 33, "i": 34, "p": 35,
    "l": 37, "j": 38, "'": 39, "k": 40, ";": 41, "\\": 42,
    ",": 43, "/": 44, "n": 45, "m": 46, ".": 47,
    "`": 50, " ": 49,
    # Special
    "return": 36, "enter": 36,
    "tab": 48,
    "space": 49,
    "delete": 51, "backspace": 51,
    "escape": 53, "esc": 53,
    "command": 55, "cmd": 55,
    "shift": 56,
    "capslock": 57,
    "option": 58, "alt": 58,
    "control": 59, "ctrl": 59,
    "right_shift": 60,
    "right_option": 61,
    "right_control": 62,
    "fn": 63,
    "f17": 64,
    "keypad_decimal": 65,
    "keypad_multiply": 67,
    "keypad_plus": 69,
    "keypad_clear": 71,
    "keypad_divide": 75,
    "keypad_enter": 76,
    "keypad_minus": 78,
    "keypad_equals": 81,
    "keypad_0": 82, "keypad_1": 83, "keypad_2": 84, "keypad_3": 85,
    "keypad_4": 86, "keypad_5": 87, "keypad_6": 88, "keypad_7": 89,
    "keypad_8": 91, "keypad_9": 92,
    "f5": 96, "f6": 97, "f7": 98, "f3": 99, "f8": 100, "f9": 101,
    "f11": 103, "f13": 105, "f16": 106, "f14": 107,
    "f10": 109, "f12": 111, "f15": 113,
    "help": 114, "home": 115, "pageup": 116,
    "forwarddelete": 117,
    "f4": 118, "end": 119, "f2": 120, "pagedown": 121, "f1": 122,
    "left": 123, "right": 124, "down": 125, "up": 126,
    "arrow_left": 123, "arrow_right": 124, "arrow_down": 125, "arrow_up": 126,
}

# CGEventFlags values for modifiers
_MOD_MAP: Dict[str, int] = {
    "cmd": 0x00100000,       # kCGEventFlagMaskCommand
    "command": 0x00100000,
    "shift": 0x00020000,     # kCGEventFlagMaskShift
    "alt": 0x00080000,       # kCGEventFlagMaskAlternate
    "option": 0x00080000,
    "ctrl": 0x00040000,      # kCGEventFlagMaskControl
    "control": 0x00040000,
    "fn": 0x00800000,        # kCGEventFlagMaskSecondaryFn
    "capslock": 0x00010000,  # kCGEventFlagMaskAlphaShift
}


# ---------------------------------------------------------------------------
# BackgroundEventPoster
# ---------------------------------------------------------------------------

class BackgroundEventPoster:
    """
    Posts macOS input events using Quartz CGEvent APIs without focus stealing.

    Reports failure when Quartz is not available (no foreground fallback).

    When a ``pid`` argument is passed to any posting method AND SkyLight is
    available, events are delivered directly to that process via
    ``SLEventPostToPid`` instead of the session-level HID tap.

    SLEventPostToPid requires the com.apple.private.skylight entitlement on
    hardened runtime targets. Works without entitlement on development builds.
    """

    def __init__(self) -> None:
        self._quartz_ok = _QUARTZ_AVAILABLE

    # ── Capabilities ──────────────────────────────────────────────────────────

    @classmethod
    def is_background_posting_available(cls) -> bool:
        """Return True if SLEventPostToPid (SkyLight) is available for targeted PID posting."""
        return _SKYLIGHT is not None

    # ── Available modes ───────────────────────────────────────────────────────

    @property
    def available_modes(self) -> List[EventPostMode]:
        modes: List[EventPostMode] = []
        if _SKYLIGHT is not None:
            modes.append(EventPostMode.BACKGROUND)
        if self._quartz_ok:
            modes.append(EventPostMode.BACKGROUND_QUARTZ)
        modes.append(EventPostMode.FOREGROUND_APPLESCRIPT)
        return modes

    # ── Click events ──────────────────────────────────────────────────────────

    async def post_click(
        self,
        x: float,
        y: float,
        button: str = "left",
        double: bool = False,
        pid: Optional[int] = None,
    ) -> EventResult:
        """Post a mouse click, optionally to a specific PID."""
        if double:
            return await self.post_double_click(x, y, pid=pid)

        if self._quartz_ok:
            try:
                result = self._quartz_click(x, y, button=button, double=False, pid=pid)
                if result:
                    mode = EventPostMode.BACKGROUND if (pid is not None and _SKYLIGHT is not None) else EventPostMode.BACKGROUND_QUARTZ
                    return EventResult(success=True, mode_used=mode)
            except Exception as exc:
                logger.debug("[EventPoster] Quartz click failed: %s", exc)

        return EventResult(
            success=False,
            mode_used=EventPostMode.BACKGROUND_QUARTZ,
            notes="No available click backend succeeded",
        )

    async def post_right_click(
        self,
        x: float,
        y: float,
        pid: Optional[int] = None,
    ) -> EventResult:
        return await self.post_click(x, y, button="right", pid=pid)

    async def post_double_click(
        self,
        x: float,
        y: float,
        pid: Optional[int] = None,
    ) -> EventResult:
        if self._quartz_ok:
            try:
                ok = self._quartz_click(x, y, button="left", double=True, pid=pid)
                if ok:
                    mode = EventPostMode.BACKGROUND if (pid is not None and _SKYLIGHT is not None) else EventPostMode.BACKGROUND_QUARTZ
                    return EventResult(success=True, mode_used=mode)
            except Exception as exc:
                logger.debug("[EventPoster] Quartz double-click failed: %s", exc)

        return EventResult(
            success=False,
            mode_used=EventPostMode.BACKGROUND_QUARTZ,
            notes="No backend available for double-click",
        )

    # ── Scroll ────────────────────────────────────────────────────────────────

    async def post_scroll(
        self,
        x: float,
        y: float,
        dx: float,
        dy: float,
        pid: Optional[int] = None,
    ) -> EventResult:
        """Post a scroll event at (x, y) with delta (dx, dy)."""
        if self._quartz_ok:
            try:
                # Move cursor to target position first
                point = CG.CGPointMake(x, y)
                move_event = CG.CGEventCreateMouseEvent(
                    None, CG.kCGEventMouseMoved, point, CG.kCGMouseButtonLeft
                )
                if move_event:
                    _post_event(move_event, pid)

                # Scroll: unit PIXEL, 2 axes (vertical=axis1, horizontal=axis2)
                scroll_event = CG.CGEventCreateScrollWheelEvent(
                    None,
                    CG.kCGScrollEventUnitPixel,
                    2,
                    int(dy),
                    int(dx),
                )
                if scroll_event:
                    _post_event(scroll_event, pid)
                    mode = EventPostMode.BACKGROUND if (pid is not None and _SKYLIGHT is not None) else EventPostMode.BACKGROUND_QUARTZ
                    return EventResult(success=True, mode_used=mode)
            except Exception as exc:
                logger.debug("[EventPoster] Quartz scroll failed: %s", exc)

        return EventResult(
            success=False,
            mode_used=EventPostMode.BACKGROUND_QUARTZ,
            notes="No backend available for scroll",
        )

    # ── Drag ──────────────────────────────────────────────────────────────────

    async def post_drag(
        self,
        from_x: float,
        from_y: float,
        to_x: float,
        to_y: float,
        duration_ms: int = 500,
        pid: Optional[int] = None,
    ) -> EventResult:
        """Drag from (from_x, from_y) to (to_x, to_y) with optional physics easing."""
        if self._quartz_ok:
            try:
                steps = 10
                delay_s = (duration_ms / 1000.0) / steps

                # Mouse down at start
                start = CG.CGPointMake(from_x, from_y)
                down_event = CG.CGEventCreateMouseEvent(
                    None, CG.kCGEventLeftMouseDown, start, CG.kCGMouseButtonLeft
                )
                if down_event:
                    _post_event(down_event, pid)

                # Interpolate
                for i in range(1, steps + 1):
                    t = i / steps
                    ix = from_x + (to_x - from_x) * t
                    iy = from_y + (to_y - from_y) * t
                    pt = CG.CGPointMake(ix, iy)
                    drag_event = CG.CGEventCreateMouseEvent(
                        None, CG.kCGEventLeftMouseDragged, pt, CG.kCGMouseButtonLeft
                    )
                    if drag_event:
                        _post_event(drag_event, pid)
                    await asyncio.sleep(delay_s)

                # Mouse up at end
                end = CG.CGPointMake(to_x, to_y)
                up_event = CG.CGEventCreateMouseEvent(
                    None, CG.kCGEventLeftMouseUp, end, CG.kCGMouseButtonLeft
                )
                if up_event:
                    _post_event(up_event, pid)

                mode = EventPostMode.BACKGROUND if (pid is not None and _SKYLIGHT is not None) else EventPostMode.BACKGROUND_QUARTZ
                return EventResult(success=True, mode_used=mode)
            except Exception as exc:
                logger.debug("[EventPoster] Quartz drag failed: %s", exc)

        return EventResult(
            success=False,
            mode_used=EventPostMode.BACKGROUND_QUARTZ,
            notes="No backend available for drag",
        )

    # ── Key events ────────────────────────────────────────────────────────────

    async def post_key(
        self,
        keycode: int,
        modifiers: List[str] = [],
        pid: Optional[int] = None,
    ) -> EventResult:
        """Post a key down + key up pair with optional modifier flags."""
        if self._quartz_ok:
            try:
                flags = 0
                for mod in modifiers:
                    flags |= _MOD_MAP.get(mod.lower(), 0)

                for down in (True, False):
                    event = CG.CGEventCreateKeyboardEvent(None, keycode, down)
                    if event and flags:
                        CG.CGEventSetFlags(event, flags)
                    if event:
                        _post_event(event, pid)

                mode = EventPostMode.BACKGROUND if (pid is not None and _SKYLIGHT is not None) else EventPostMode.BACKGROUND_QUARTZ
                return EventResult(success=True, mode_used=mode)
            except Exception as exc:
                logger.debug("[EventPoster] Quartz key failed: %s", exc)

        return EventResult(
            success=False,
            mode_used=EventPostMode.BACKGROUND_QUARTZ,
            notes="No backend available for key event",
        )

    async def post_key_combo(
        self,
        combo: str,
        pid: Optional[int] = None,
    ) -> EventResult:
        """
        Post a key combination like "cmd+s", "cmd+shift+t", "ctrl+a".

        Parses the combo string and dispatches via post_key.
        """
        try:
            keycode, mods = self._parse_combo(combo)
        except ValueError as exc:
            return EventResult(
                success=False,
                mode_used=EventPostMode.BACKGROUND_QUARTZ,
                notes=f"Could not parse combo {combo!r}: {exc}",
            )

        if self._quartz_ok:
            try:
                flags = 0
                for mod in mods:
                    flags |= _MOD_MAP.get(mod.lower(), 0)

                for down in (True, False):
                    event = CG.CGEventCreateKeyboardEvent(None, keycode, down)
                    if event and flags:
                        CG.CGEventSetFlags(event, flags)
                    if event:
                        _post_event(event, pid)

                mode = EventPostMode.BACKGROUND if (pid is not None and _SKYLIGHT is not None) else EventPostMode.BACKGROUND_QUARTZ
                return EventResult(success=True, mode_used=mode)
            except Exception as exc:
                logger.debug("[EventPoster] Quartz key_combo failed: %s", exc)

        return EventResult(
            success=False,
            mode_used=EventPostMode.BACKGROUND_QUARTZ,
            notes=f"No backend available for combo {combo!r}",
        )

    # ── Text typing ───────────────────────────────────────────────────────────

    async def post_type(
        self,
        text: str,
        interval_ms: int = 30,
        pid: Optional[int] = None,
    ) -> EventResult:
        """
        Type text character-by-character.

        Uses CGEventKeyboardSetUnicodeString on macOS for full Unicode support.
        """
        if self._quartz_ok:
            try:
                for char in text:
                    # Create a key event and set Unicode string directly
                    down_event = CG.CGEventCreateKeyboardEvent(None, 0, True)
                    up_event = CG.CGEventCreateKeyboardEvent(None, 0, False)
                    if down_event and up_event:
                        uchar = char
                        CG.CGEventKeyboardSetUnicodeString(down_event, len(uchar), uchar)
                        CG.CGEventKeyboardSetUnicodeString(up_event, len(uchar), uchar)
                        _post_event(down_event, pid)
                        _post_event(up_event, pid)
                    if interval_ms > 0:
                        await asyncio.sleep(interval_ms / 1000.0)
                mode = EventPostMode.BACKGROUND if (pid is not None and _SKYLIGHT is not None) else EventPostMode.BACKGROUND_QUARTZ
                return EventResult(success=True, mode_used=mode)
            except Exception as exc:
                logger.debug("[EventPoster] Quartz type failed: %s", exc)

        return EventResult(
            success=False,
            mode_used=EventPostMode.BACKGROUND_QUARTZ,
            notes="No backend available for typing",
        )

    # ── Cursor movement ───────────────────────────────────────────────────────

    async def move_cursor(self, x: float, y: float) -> EventResult:
        """Move the visible cursor to (x, y)."""
        if self._quartz_ok:
            try:
                pt = CG.CGPointMake(x, y)
                event = CG.CGEventCreateMouseEvent(
                    None, CG.kCGEventMouseMoved, pt, CG.kCGMouseButtonLeft
                )
                if event:
                    CG.CGEventPost(CG.kCGSessionEventTap, event)
                    return EventResult(success=True, mode_used=EventPostMode.BACKGROUND_QUARTZ)
            except Exception as exc:
                logger.debug("[EventPoster] Quartz move_cursor failed: %s", exc)

        return EventResult(
            success=False,
            mode_used=EventPostMode.BACKGROUND_QUARTZ,
            notes="No backend available for cursor move",
        )

    # ── Private helpers ───────────────────────────────────────────────────────

    def _quartz_available(self) -> bool:
        return self._quartz_ok

    def _quartz_click(
        self,
        x: float,
        y: float,
        button: str = "left",
        double: bool = False,
        pid: Optional[int] = None,
    ) -> bool:
        """
        Post mouse down/up via Quartz CGEvent.

        When pid is provided and SkyLight is available, each event is delivered
        directly to that process via SLEventPostToPid.  Otherwise falls back to
        kCGSessionEventTap (session-level, no focus stealing).
        """
        pt = CG.CGPointMake(x, y)

        if button == "right":
            down_type = CG.kCGEventRightMouseDown
            up_type = CG.kCGEventRightMouseUp
            btn = CG.kCGMouseButtonRight
        elif button == "middle":
            down_type = CG.kCGEventOtherMouseDown
            up_type = CG.kCGEventOtherMouseUp
            btn = CG.kCGMouseButtonCenter
        else:
            down_type = CG.kCGEventLeftMouseDown
            up_type = CG.kCGEventLeftMouseUp
            btn = CG.kCGMouseButtonLeft

        down = CG.CGEventCreateMouseEvent(None, down_type, pt, btn)
        up = CG.CGEventCreateMouseEvent(None, up_type, pt, btn)
        if down is None or up is None:
            return False

        if double:
            CG.CGEventSetIntegerValueField(down, CG.kCGMouseEventClickState, 2)
            CG.CGEventSetIntegerValueField(up, CG.kCGMouseEventClickState, 2)

        _post_event(down, pid)
        if double:
            time.sleep(0.03)
        _post_event(up, pid)

        if double:
            # Second click for double-click
            time.sleep(0.03)
            down2 = CG.CGEventCreateMouseEvent(None, down_type, pt, btn)
            up2 = CG.CGEventCreateMouseEvent(None, up_type, pt, btn)
            if down2 and up2:
                CG.CGEventSetIntegerValueField(down2, CG.kCGMouseEventClickState, 2)
                CG.CGEventSetIntegerValueField(up2, CG.kCGMouseEventClickState, 2)
                _post_event(down2, pid)
                _post_event(up2, pid)

        return True

    def _parse_combo(self, combo: str) -> Tuple[int, List[str]]:
        """
        Parse "cmd+s" → (keycode_for_s, ["cmd"]).
        Parse "cmd+shift+tab" → (keycode_for_tab, ["cmd", "shift"]).
        Raises ValueError for unknown key names.
        """
        parts = [p.strip().lower() for p in combo.split("+")]
        modifier_names = set(_MOD_MAP.keys())
        mods: List[str] = []
        keys: List[str] = []

        for part in parts:
            if part in modifier_names:
                mods.append(part)
            else:
                keys.append(part)

        if not keys:
            raise ValueError(f"No non-modifier key in combo: {combo!r}")

        key_name = keys[-1]  # last non-modifier is the primary key
        keycode = _KEY_MAP.get(key_name)
        if keycode is None:
            raise ValueError(f"Unknown key name {key_name!r} in combo {combo!r}")

        return (keycode, mods)


# ---------------------------------------------------------------------------
# Module-level singleton
# ---------------------------------------------------------------------------

_poster: Optional[BackgroundEventPoster] = None


def get_poster() -> BackgroundEventPoster:
    """Return the module-level BackgroundEventPoster singleton."""
    global _poster
    if _poster is None:
        _poster = BackgroundEventPoster()
    return _poster
