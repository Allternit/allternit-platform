"""Anthropic toolset drivers for computer_toolset_20260801 / browser_toolset_20260801.

Each member call runs on an Allternit hosted computer via ``POST /v1/computers/{id}/toolset``.
When the installed ``anthropic`` package ships the abstract toolsets (``anthropic.tools.computer``
/ ``anthropic.tools.browser``), these classes subclass them and override ``execute``. Older
releases (anthropic <= 0.125.0 has none) get a small stand-in base with the same ``execute``
hook plus ``to_dict()`` and ``tool_result(tool_use)`` for a hand-written loop.
"""

from __future__ import annotations

import json
import re
from typing import Any, Callable, Dict, Optional

from .client import AllternitComputers, ApprovalRequiredError, ToolsetResult, result_image, result_text


def _sdk_base(module: str, names: tuple) -> Optional[type]:
    try:
        mod = __import__(module, fromlist=["_"])
    except ImportError:
        return None
    for n in names:
        if hasattr(mod, n):
            return getattr(mod, n)
    return None


class _ToolError(Exception):
    def __init__(self, content: Any) -> None:
        super().__init__(content if isinstance(content, str) else "toolset error")
        self.content = content


try:  # The SDK's ToolError, so its runner reports is_error the same way.
    from anthropic.lib.tools import ToolError as _SDKToolError  # type: ignore

    ToolError: Any = _SDKToolError
except ImportError:
    ToolError = _ToolError


class _StandIn:
    type = ""
    toolset_name = ""

    def __init__(self, **_: Any) -> None:
        pass

    def to_dict(self) -> Dict[str, Any]:
        return {"type": self.type}

    def tool_result(self, tool_use: Dict[str, Any]) -> Dict[str, Any]:
        base = {"type": "tool_result", "tool_use_id": tool_use["id"], "toolset_name": self.toolset_name}
        try:
            out = self.execute(None, tool_use["name"], tool_use.get("input") or {})  # type: ignore[attr-defined]
        except Exception as e:  # noqa: BLE001
            content = getattr(e, "content", str(e))
            return {**base, "content": content, "is_error": True}
        if isinstance(out, dict) and "data" in out:
            out = [{"type": "image", "source": {"type": "base64", "media_type": out.get("media_type", "image/png"), "data": out["data"]}}]
        elif isinstance(out, (dict, list)):
            out = json.dumps(out)
        return {**base, "content": out or "Done."}


_ComputerBase: Any = _sdk_base("anthropic.tools.computer", ("BetaAbstractComputerToolset20260801", "AbstractComputerToolset20260801")) or type("_ComputerBase", (_StandIn,), {"type": "computer_toolset_20260801", "toolset_name": "computer"})
_BrowserBase: Any = _sdk_base("anthropic.tools.browser", ("BetaAbstractBrowserToolset20260801", "AbstractBrowserToolset20260801")) or type("_BrowserBase", (_StandIn,), {"type": "browser_toolset_20260801", "toolset_name": "browser"})


def _server_decides(*_: Any) -> bool:
    """Default ``confirm``: the Allternit server applies member policy and holds risky calls (409)."""
    return True


class _Driver:
    def _setup(self, client: AllternitComputers, computer_id: str, on_approval: Optional[Callable[[Dict[str, Any]], bool]], call_ids: Optional[Callable[[Any, str], Dict[str, Any]]], browser_session_id: Optional[str]) -> None:
        self._client, self._computer_id = client, computer_id
        self._on_approval, self._call_ids, self._session = on_approval, call_ids, browser_session_id

    def _call(self, toolset: str, ctx: Any, member: str, input: Any) -> ToolsetResult:
        call: Dict[str, Any] = {"toolset": toolset, "member": member, "input": dict(input or {})}
        if self._call_ids:
            call.update(self._call_ids(ctx, member) or {})
        if self._session:
            call["browser_session_id"] = self._session
        try:
            res = self._client.toolset(self._computer_id, call)
        except ApprovalRequiredError as e:
            if self._on_approval and self._on_approval(e.approval):
                self._client.approve(self._computer_id, e.approval["id"])
                res = self._client.toolset(self._computer_id, {**call, "approval_grant": e.approval["id"]})
            else:
                res = e.result
        if res.get("is_error"):
            raise ToolError(_blocks(res))
        return res


def _blocks(r: ToolsetResult) -> Any:
    out = [
        {"type": "text", "text": b["text"]} if b.get("type") == "text" else {"type": "image", "source": {"type": "base64", "media_type": b["media_type"], "data": b["data"]}}
        for b in r.get("content", [])
    ]
    return out or "The action failed."


def _shot(r: ToolsetResult) -> Dict[str, str]:
    img = result_image(r)
    if not img:
        raise ToolError("The computer returned no screenshot.")
    return {"data": img["data"], "media_type": img["media_type"]}


def _json(r: ToolsetResult) -> Any:
    try:
        return json.loads(result_text(r))
    except ValueError:
        return None


class AllternitComputerToolset(_Driver, _ComputerBase):
    def __init__(self, client: AllternitComputers, computer_id: str, on_approval: Optional[Callable[[Dict[str, Any]], bool]] = None, call_ids: Optional[Callable[[Any, str], Dict[str, Any]]] = None, **sdk_options: Any) -> None:
        sdk_options.setdefault("confirm", _server_decides)
        _ComputerBase.__init__(self, **sdk_options)
        self._setup(client, computer_id, on_approval, call_ids, None)

    def execute(self, ctx: Any, name: str, input: Any) -> Any:
        r = self._call("computer", ctx, name, input)
        if name in ("screenshot", "zoom"):
            return _shot(r)
        if name == "cursor_position":
            j = _json(r)
            if isinstance(j, dict) and "x" in j:
                return {"x": j["x"], "y": j["y"]}
            m = re.search(r"(-?\d+)\D+(-?\d+)", result_text(r))
            if not m:
                raise ToolError("The computer returned no cursor position.")
            return {"x": int(m.group(1)), "y": int(m.group(2))}
        return result_text(r) or None


class AllternitBrowserToolset(_Driver, _BrowserBase):
    def __init__(self, client: AllternitComputers, computer_id: str, on_approval: Optional[Callable[[Dict[str, Any]], bool]] = None, call_ids: Optional[Callable[[Any, str], Dict[str, Any]]] = None, browser_session_id: Optional[str] = None, **sdk_options: Any) -> None:
        self._state: Optional[Dict[str, Any]] = None
        sdk_options.setdefault("confirm", _server_decides)
        sdk_options.setdefault("browser_state", self._current_state)
        _BrowserBase.__init__(self, **sdk_options)
        self._setup(client, computer_id, on_approval, call_ids, browser_session_id)

    def _current_state(self, ctx: Any = None) -> Dict[str, Any]:
        if self._state is None:
            r = self._call("browser", ctx, "list_tabs", {})
            self._state = r.get("browser_state") or {"tabs": _json(r) or []}
        return self._state

    def execute(self, ctx: Any, name: str, input: Any) -> Any:
        r = self._call("browser", ctx, name, input)
        if r.get("browser_state"):
            self._state = r["browser_state"]
        if name in ("screenshot", "zoom"):
            return _shot(r)
        j, tabs = _json(r), (self._state or {}).get("tabs", [])
        if name == "navigate":
            return j if isinstance(j, dict) and j.get("url") else {"url": tabs[0]["url"] if tabs else str((input or {}).get("url", ""))}
        if name == "list_tabs":
            return j if isinstance(j, list) else tabs
        if name in ("new_tab", "switch_tab"):
            if isinstance(j, dict) and j.get("tab_id"):
                return j
            if tabs:
                return tabs[0]
            raise ToolError("The browser returned no tab.")
        if name == "close_tab":
            return None
        return result_text(r) or None
