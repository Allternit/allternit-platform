"""Gemini computer_use adapter. Gemini coordinates are 0-999 normalized, so every call
is sent with ``coordinate_space: "normalized_1000"`` and the server scales to the screen."""

from __future__ import annotations

from typing import Any, Callable, Dict, List, Optional

from .client import AllternitComputers, result_image, result_text
from .v2 import computer_v2_tool, run_computer_v2_member

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


def gemini_computer_v2_declaration() -> Dict[str, Any]:
    """The ``computer_v2`` function declaration for Gemini models: the ten structured members
    next to the ``computer_use`` declaration. Route matching ``functionCall`` parts through
    ``run_gemini_v2_call``."""
    t = computer_v2_tool()
    return {"name": t["name"], "description": t["description"], "parameters": t["schema"]}


def run_gemini_v2_call(
    client: AllternitComputers,
    computer_id: str,
    name: str,
    input: Dict[str, Any],
    on_approval: Optional[Callable[[Dict[str, Any]], Any]] = None,
) -> Dict[str, Any]:
    """Run one ``computer_v2`` function call (``{action, ...fields}``).

    Returns ``{"function_response": {name, response}, "error": ...}``. Without
    ``on_approval`` the ApprovalRequiredError propagates, like the pixel adapter.
    """
    args = dict(input or {})
    action = args.pop("action", None)
    if not action:
        raise ValueError("The computer_v2 call needs an action.")
    if on_approval is not None:
        res = run_computer_v2_member(client, computer_id, str(action), args, on_approval)
    else:
        res = client.toolset(computer_id, {"toolset": "computer", "member": str(action), "input": args})
    text = result_text(res)
    response: Dict[str, Any] = {"result": text}
    error = None
    if res.get("is_error"):
        response["error"] = text or "The action failed."
        error = res
    return {"function_response": {"name": name, "response": response}, "error": error}
