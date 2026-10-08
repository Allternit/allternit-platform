"""Gemini computer_use adapter. Gemini coordinates are 0-999 normalized, so every call
is sent with ``coordinate_space: "normalized_1000"`` and the server scales to the screen."""

from __future__ import annotations

from typing import Any, Dict, List

from .client import AllternitComputers, result_image, result_text

Step = Dict[str, Any]
_MAP = {"control": "ctrl", "meta": "super", "command": "super", "enter": "Return", "escape": "Escape", "backspace": "BackSpace", "tab": "Tab", "delete": "Delete"}


def gemini_keys(keys: str) -> str:
    """``"Control+Shift+T"`` -> ``"ctrl+shift+t"``."""
    parts = [k.strip() for k in keys.split("+")]
    return "+".join(_MAP.get(k.lower(), k.lower() if len(k) == 1 else k) for k in parts)


def _pt(x: Any, y: Any) -> List[int]:
    return [round(float(x)), round(float(y))]


def _key(text: str) -> Step:
    return {"member": "key", "input": {"text": text}}


def _type(text: str) -> Step:
    return {"member": "type", "input": {"text": text}}


def _go(url: str) -> List[Step]:
    return [_key("ctrl+l"), _type(url), _key("Return")]


def gemini_to_calls(name: str, args: Dict[str, Any], search_url: str = "https://www.google.com") -> List[Step]:
    a = args or {}
    d = str(a.get("direction", "down"))
    if name == "open_web_browser":
        return []
    if name == "wait_5_seconds":
        return [{"member": "wait", "input": {"duration": 5}}]
    if name == "go_back":
        return [_key("alt+Left")]
    if name == "go_forward":
        return [_key("alt+Right")]
    if name == "search":
        return _go(search_url)
    if name == "navigate":
        return _go(str(a.get("url", "")))
    if name == "click_at":
        return [{"member": "left_click", "input": {"coordinate": _pt(a["x"], a["y"])}}]
    if name == "hover_at":
        return [{"member": "mouse_move", "input": {"coordinate": _pt(a["x"], a["y"])}}]
    if name == "type_text_at":
        steps: List[Step] = [{"member": "left_click", "input": {"coordinate": _pt(a["x"], a["y"])}}]
        if a.get("clear_before_typing", True):
            steps += [_key("ctrl+a"), _key("BackSpace")]
        steps.append(_type(str(a.get("text", ""))))
        if a.get("press_enter", True):
            steps.append(_key("Return"))
        return steps
    if name == "key_combination":
        return [_key(gemini_keys(str(a.get("keys", ""))))]
    if name == "scroll_document":
        return [{"member": "scroll", "input": {"coordinate": [500, 500], "scroll_direction": d, "scroll_amount": 5}}]
    if name == "scroll_at":
        amount = max(1, round(float(a.get("magnitude", 800)) / 160))
        return [{"member": "scroll", "input": {"coordinate": _pt(a["x"], a["y"]), "scroll_direction": d, "scroll_amount": amount}}]
    if name == "drag_and_drop":
        return [{"member": "left_click_drag", "input": {"start_coordinate": _pt(a["x"], a["y"]), "coordinate": _pt(a["destination_x"], a["destination_y"])}}]
    raise ValueError(f"Unsupported Gemini computer_use function: {name}")


def run_gemini_call(client: AllternitComputers, computer_id: str, name: str, args: Dict[str, Any], search_url: str = "https://www.google.com") -> Dict[str, Any]:
    """Returns ``{"function_response": {name, response}, "inline_data": {mime_type, data}, "error": ...}``."""
    base = {"toolset": "computer", "coordinate_space": "normalized_1000"}
    error = None
    for step in gemini_to_calls(name, args, search_url):
        r = client.toolset(computer_id, {**base, **step})
        if r.get("is_error"):
            error = r
            break
    s = client.toolset(computer_id, {**base, "member": "screenshot", "input": {}})
    img = result_image(s)
    if not img:
        raise RuntimeError("Allternit computer returned no screenshot.")
    response: Dict[str, Any] = {"url": ""}
    if error:
        response["error"] = result_text(error)
    return {"function_response": {"name": name, "response": response}, "inline_data": {"mime_type": img["media_type"], "data": img["data"]}, "error": error}
