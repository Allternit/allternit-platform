"""OpenAI computer-use adapter: run one ``computer_call`` action on an Allternit hosted
computer and return a ``computer_call_output`` item carrying the next screenshot."""

from __future__ import annotations

from typing import Any, Dict, List, Optional

from .client import AllternitComputers, result_image

_KEYS = {
    "CTRL": "ctrl", "CONTROL": "ctrl", "ALT": "alt", "OPTION": "alt", "SHIFT": "shift", "CMD": "super", "META": "super",
    "SUPER": "super", "WIN": "super", "ENTER": "Return", "RETURN": "Return", "ESC": "Escape", "ESCAPE": "Escape",
    "TAB": "Tab", "SPACE": "space", "BACKSPACE": "BackSpace", "DELETE": "Delete", "UP": "Up", "DOWN": "Down",
    "LEFT": "Left", "RIGHT": "Right", "ARROWUP": "Up", "ARROWDOWN": "Down", "ARROWLEFT": "Left", "ARROWRIGHT": "Right",
    "HOME": "Home", "END": "End", "PAGEUP": "Page_Up", "PAGEDOWN": "Page_Down",
}

Step = Dict[str, Any]


def openai_keys(keys: List[str]) -> str:
    """``["CTRL","C"]`` -> ``"ctrl+c"``."""
    return "+".join(_KEYS.get(k.upper(), k.lower() if len(k) == 1 else k) for k in keys)


def _c(x: Any, y: Any) -> List[int]:
    return [round(float(x)), round(float(y))]


def openai_to_calls(a: Dict[str, Any], per_click: int = 100) -> List[Step]:
    t = a.get("type")
    if t == "click":
        b = a.get("button", "left")
        if b == "back":
            return [{"member": "key", "input": {"text": "alt+Left"}}]
        if b == "forward":
            return [{"member": "key", "input": {"text": "alt+Right"}}]
        member = {"right": "right_click", "wheel": "middle_click"}.get(b, "left_click")
        return [{"member": member, "input": {"coordinate": _c(a["x"], a["y"])}}]
    if t == "double_click":
        return [{"member": "double_click", "input": {"coordinate": _c(a["x"], a["y"])}}]
    if t == "drag":
        p = a.get("path") or []
        if len(p) < 2:
            return []
        if len(p) == 2:
            return [{"member": "left_click_drag", "input": {"start_coordinate": _c(p[0]["x"], p[0]["y"]), "coordinate": _c(p[1]["x"], p[1]["y"])}}]
        return (
            [{"member": "mouse_move", "input": {"coordinate": _c(p[0]["x"], p[0]["y"])}}, {"member": "left_mouse_down", "input": {}}]
            + [{"member": "mouse_move", "input": {"coordinate": _c(q["x"], q["y"])}} for q in p[1:]]
            + [{"member": "left_mouse_up", "input": {}}]
        )
    if t == "keypress":
        return [{"member": "key", "input": {"text": openai_keys(a.get("keys") or [])}}]
    if t == "move":
        return [{"member": "mouse_move", "input": {"coordinate": _c(a["x"], a["y"])}}]
    if t == "scroll":
        out: List[Step] = []
        amt = lambda v: max(1, round(abs(v) / per_click))  # noqa: E731
        sx, sy = a.get("scroll_x", 0) or 0, a.get("scroll_y", 0) or 0
        if sy:
            out.append({"member": "scroll", "input": {"coordinate": _c(a["x"], a["y"]), "scroll_direction": "down" if sy > 0 else "up", "scroll_amount": amt(sy)}})
        if sx:
            out.append({"member": "scroll", "input": {"coordinate": _c(a["x"], a["y"]), "scroll_direction": "right" if sx > 0 else "left", "scroll_amount": amt(sx)}})
        return out
    if t == "type":
        return [{"member": "type", "input": {"text": a.get("text", "")}}]
    if t == "wait":
        return [{"member": "wait", "input": {"duration": max(1, round((a.get("ms") or 2000) / 1000))}}]
    if t == "screenshot":
        return []
    raise ValueError(f"Unsupported OpenAI computer action: {t}")


def run_openai_action(
    client: AllternitComputers,
    computer_id: str,
    call_id: str,
    action: Dict[str, Any],
    display: Optional[Dict[str, int]] = None,
    pixels_per_scroll_click: int = 100,
) -> Dict[str, Any]:
    """Returns ``{"output": <computer_call_output item>, "error": <failed result or None>}``."""
    frame = {"model_frame": display, "coordinate_space": "pixels"} if display else {}
    error = None
    for step in openai_to_calls(action, pixels_per_scroll_click):
        r = client.toolset(computer_id, {"toolset": "computer", **step, **frame})
        if r.get("is_error"):
            error = r
            break
    s = client.toolset(computer_id, {"toolset": "computer", "member": "screenshot", "input": {}, **frame})
    img = result_image(s)
    if not img:
        raise RuntimeError("Allternit computer returned no screenshot.")
    return {
        "output": {
            "type": "computer_call_output",
            "call_id": call_id,
            "output": {"type": "computer_screenshot", "image_url": f"data:{img['media_type']};base64,{img['data']}"},
        },
        "error": error,
    }
