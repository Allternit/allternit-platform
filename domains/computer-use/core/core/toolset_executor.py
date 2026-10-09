"""The engine's side of the Allternit computer toolset (allternit.computer.v1 /
allternit.browser.v1).

Every provider path in the engine (vision_providers.py) expresses its next
action as a contract call: ``(toolset, member, input)`` with the member names
and input fields of ``contracts/toolset_v1.py`` (identical to Anthropic's
``computer_toolset_20260801`` / ``browser_toolset_20260801``). This module:

* generates the planning JSON schema and the prompt's member list from the
  contract (no hand-kept copy of the action vocabulary),
* normalizes the few legacy verbs older model outputs still use
  (``click`` -> ``left_click``, ``goto`` -> ``navigate``, ...) onto members,
* sends calls to THE executor, allternit-api ``POST /api/v1/computers/:id/toolset``
  (spec 1.2). The executor validates, scales coordinates between the model
  frame and the screen, classifies risk, asks for approval (409
  ``approval_required``), writes the audit row and dispatches. The engine does
  no coordinate scaling or screenshot resizing of its own.

No loop: the executor reaches a gateway browser session through the gateway's
``/v1/execute`` handlers, which run Playwright directly and never call back
into the executor. Only the planning loop calls the executor.
"""

from __future__ import annotations

import logging
import os
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional, Tuple

from contracts.toolset_v1 import CONTRACTS, member_spec

logger = logging.getLogger(__name__)

TOOLSETS = ("computer", "browser")

# Older outputs (pre-contract prompts and model text) use these verbs. They are names, not a second schema:
# each maps onto one contract member.
LEGACY_VERBS: Dict[str, str] = {
    "click": "left_click",
    "tap": "left_click",
    "doubleclick": "double_click",
    "rightclick": "right_click",
    "press": "key",
    "keypress": "key",
    "hotkey": "key",
    "fill": "type",
    "input": "type",
    "goto": "navigate",
    "open_url": "navigate",
    "move": "mouse_move",
    "drag": "left_click_drag",
    "observe": "screenshot",
    "sleep": "wait",
}


def default_enabled_members(toolset: str) -> List[Dict[str, Any]]:
    return [m for m in CONTRACTS[toolset]["members"] if m.get("default_enabled", True)]


def plan_json_schema() -> Dict[str, Any]:
    """The ActionPlan JSON schema, generated from the contract.

    ``immediate_action`` is one contract call: ``toolset`` + ``member`` +
    ``input`` (the member's own input schema). ``batch`` and ``code`` are the
    separate grant-bound selector batch (core/batch_dispatch.py) and code-mode
    surfaces; they are unchanged.
    """
    variants: List[Dict[str, Any]] = []
    for toolset in TOOLSETS:
        for m in default_enabled_members(toolset):
            variants.append({
                "type": "object",
                "properties": {
                    "toolset": {"type": "string", "const": toolset},
                    "member": {"type": "string", "const": m["name"]},
                    "input": m["input_schema"],
                    "target": {"type": "string", "description": "What the action is aimed at, in words."},
                    "reason": {"type": "string"},
                },
                "required": ["toolset", "member", "input"],
            })
    return {
        "type": "object",
        "properties": {
            "reasoning": {"type": "string"},
            "plan_steps": {"type": "array", "items": {"type": "string"}},
            "immediate_action": {"anyOf": variants},
            "confidence": {"type": "number"},
            "requires_approval": {"type": "boolean"},
            "risk_level": {"type": "string"},
            "done": {"type": "boolean"},
            "batch": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "type": {"type": "string"},
                        "target": {"type": "string"},
                        "reason": {"type": "string"},
                        "text": {"type": "string"},
                    },
                    "required": ["type", "target"],
                },
            },
            "code": {
                "type": "object",
                "properties": {
                    "language": {"type": "string"},
                    "code": {"type": "string"},
                    "declaredTargets": {"type": "array", "items": {"type": "string"}},
                },
                "required": ["code"],
            },
        },
        "required": ["immediate_action", "done"],
    }


def members_prompt() -> str:
    """One line per toolset listing its members and input fields, for prompts."""
    lines = []
    for toolset in TOOLSETS:
        parts = []
        for m in default_enabled_members(toolset):
            props = m["input_schema"].get("properties", {})
            req = set(m["input_schema"].get("required", []))
            fields = ",".join(f"{k}{'' if k in req else '?'}" for k in props if k != "tab_id")
            parts.append(f"{m['name']}({fields})")
        lines.append(f"{toolset}: " + " ".join(parts))
    return "\n".join(lines)


def _point(coordinates: Any) -> Optional[List[float]]:
    if isinstance(coordinates, (list, tuple)) and len(coordinates) >= 2:
        try:
            return [float(coordinates[0]), float(coordinates[1])]
        except (TypeError, ValueError):
            return None
    return None


def normalize_call(
    member: str,
    input: Optional[Dict[str, Any]] = None,
    *,
    toolset: Optional[str] = None,
    coordinates: Any = None,
    text: Optional[str] = None,
    target: Optional[str] = None,
) -> Tuple[str, str, Dict[str, Any]]:
    """Turn a model's action into a valid-looking contract call.

    ``input`` wins when given; otherwise the legacy ``coordinates`` / ``text``
    / ``target`` fields fill the member's inputs. Unknown verbs become a
    ``screenshot`` (observe) rather than a guess.
    """
    name = (member or "").strip().lower().replace("-", "_")
    name = LEGACY_VERBS.get(name.replace("_", ""), LEGACY_VERBS.get(name, name))
    if toolset not in TOOLSETS:
        toolset = "browser" if name in ("navigate", "hover", "scroll_to", "read_page", "find",
                                        "get_page_text", "form_input") else "computer"
    if member_spec(toolset, name) is None:
        other = "browser" if toolset == "computer" else "computer"
        if member_spec(other, name) is not None:
            toolset = other
        else:
            return "computer", "screenshot", {}
    data: Dict[str, Any] = dict(input) if isinstance(input, dict) else {}
    if not data:
        point = _point(coordinates)
        if toolset == "computer":
            if point and name in ("left_click", "right_click", "middle_click", "double_click",
                                  "triple_click", "mouse_move", "scroll", "left_click_drag"):
                data["coordinate"] = point
            if name in ("type", "key") and text:
                data["text"] = text
            if name == "scroll":
                data.setdefault("scroll_direction", "down")
                data.setdefault("scroll_amount", 3)
            if name in ("wait", "hold_key"):
                data.setdefault("duration", 1)
                if name == "hold_key" and text:
                    data["text"] = text
        else:
            if name == "navigate":
                data["url"] = text or target or ""
            elif point and name in ("left_click", "right_click", "middle_click", "double_click",
                                    "triple_click", "hover", "mouse_move", "scroll"):
                data["target"] = {"type": "coordinate", "x": point[0], "y": point[1]}
            if name == "type" and text:
                data["text"] = text
            if name == "key" and text:
                data["text"] = text
            if name == "scroll":
                data.setdefault("scroll_direction", "down")
                data.setdefault("scroll_amount", 3)
            if name == "wait":
                data.setdefault("duration", 1)
    return toolset, name, data


def point_of(toolset: str, input: Dict[str, Any]) -> Optional[List[float]]:
    """The action's primary point in the model frame, when it has one."""
    if toolset == "computer":
        return _point(input.get("coordinate"))
    tgt = input.get("target")
    if isinstance(tgt, dict) and tgt.get("type") == "coordinate":
        return _point([tgt.get("x"), tgt.get("y")])
    return None


def legacy_request(toolset: str, member: str, input: Dict[str, Any]) -> Tuple[str, str, Dict[str, Any]]:
    """``(action_type, target, parameters)`` for the in-process adapter
    waterfall, used only when the engine runs standalone (no allternit-api
    configured: tests, the demo launcher)."""
    params: Dict[str, Any] = {}
    pt = point_of(toolset, input)
    if pt:
        params["x"], params["y"] = int(pt[0]), int(pt[1])
    if input.get("text") is not None:
        params["text"] = input["text"]
    # ComputerUseExecutor takes the contract names natively (NATIVE_CLAUDE_ACTIONS,
    # BROWSER_EXTENSION_ACTIONS); only a browser hover keeps its adapter verb.
    action = "hover" if member == "mouse_move" and toolset == "browser" else member
    if member == "key":
        params["key"] = input.get("text", "")
    if member == "scroll":
        amount = int(input.get("scroll_amount") or 3) * 100
        direction = input.get("scroll_direction", "down")
        params["deltaY"] = amount if direction == "down" else -amount if direction == "up" else 0
        params["deltaX"] = amount if direction == "right" else -amount if direction == "left" else 0
    if member == "left_click_drag":
        start = _point(input.get("start_coordinate"))
        if start:
            params["startX"], params["startY"] = int(start[0]), int(start[1])
    if member == "wait":
        params["duration"] = input.get("duration", 1)
    target = str(input.get("url") or "") if member == "navigate" else ""
    return action, target, params


# ---------------------------------------------------------------------------
# Executor client
# ---------------------------------------------------------------------------

@dataclass
class ToolsetReply:
    status: int
    body: Dict[str, Any] = field(default_factory=dict)

    @property
    def ok(self) -> bool:
        return self.status == 200 and not self.body.get("is_error")

    @property
    def approval_id(self) -> Optional[str]:
        if self.status == 409 or self.body.get("error") == "approval_required":
            return self.body.get("approval_id")
        return None

    def text(self) -> str:
        return "\n".join(
            str(b.get("text", "")) for b in self.body.get("content") or []
            if isinstance(b, dict) and b.get("type") == "text"
        ).strip()

    def image_b64(self) -> Optional[str]:
        for b in self.body.get("content") or []:
            if isinstance(b, dict) and b.get("type") == "image" and b.get("data"):
                return str(b["data"])
        return None


def executor_configured() -> bool:
    """True where the engine runs next to allternit-api (Desktop and cloud
    runtimes set ALLTERNIT_API_URL for the sidecar)."""
    return bool(os.environ.get("ALLTERNIT_API_URL"))


class ToolsetExecutorClient:
    """HTTP client for ``POST /api/v1/computers/:id/toolset``.

    Auth: the Desktop passes its access token + the signed-in user id
    (``x-allternit-desktop-access-token`` + ``x-allternit-user-id``), the cloud
    runtime its internal service token. ``user_id`` is the run's owner, which
    allternit-api forwards in the ACI run request (``options.userId``).
    """

    def __init__(
        self,
        computer_id: str = "this-device",
        *,
        user_id: Optional[str] = None,
        base_url: Optional[str] = None,
        browser_session_id: Optional[str] = None,
        observe_toolset: str = "computer",
        timeout_s: float = 120.0,
    ) -> None:
        self.computer_id = computer_id or "this-device"
        # Which toolset the loop's screenshots come from: "browser" for a
        # gateway browser session, "computer" for a desktop.
        self.observe_toolset = observe_toolset if observe_toolset in TOOLSETS else "computer"
        self.user_id = user_id
        self.browser_session_id = browser_session_id
        self._base = (base_url or os.environ.get("ALLTERNIT_API_URL") or "http://127.0.0.1:8013").rstrip("/")
        self._timeout_s = timeout_s

    def _headers(self) -> Dict[str, str]:
        headers = {"Content-Type": "application/json"}
        desktop = os.environ.get("ALLTERNIT_DESKTOP_ACCESS_TOKEN")
        internal = os.environ.get("ALLTERNIT_INTERNAL_SERVICE_TOKEN")
        if self.user_id:
            headers["x-allternit-user-id"] = self.user_id
        if desktop and self.user_id:
            headers["x-allternit-desktop-access-token"] = desktop
        elif internal:
            headers["x-allternit-internal-token"] = internal
        return headers

    async def run(
        self,
        toolset: str,
        member: str,
        input: Dict[str, Any],
        *,
        run_id: Optional[str] = None,
        turn_id: Optional[str] = None,
        call_index: int = 0,
        model_frame: Optional[Dict[str, int]] = None,
        approval_grant: Optional[str] = None,
    ) -> ToolsetReply:
        body: Dict[str, Any] = {"toolset": toolset, "member": member, "input": input or {}, "call_index": call_index}
        if run_id:
            body["run_id"] = run_id
        if turn_id:
            body["turn_id"] = turn_id
        if model_frame:
            body["model_frame"] = model_frame
        if approval_grant:
            body["approval_grant"] = approval_grant
        if toolset == "browser" and self.browser_session_id:
            body["browser_session_id"] = self.browser_session_id
        import httpx
        url = f"{self._base}/api/v1/computers/{self.computer_id}/toolset"
        try:
            async with httpx.AsyncClient(timeout=self._timeout_s) as client:
                resp = await client.post(url, json=body, headers=self._headers())
        except Exception as exc:
            logger.warning("toolset executor unreachable: %s", exc)
            return ToolsetReply(503, {"is_error": True, "error": "executor_unreachable",
                                      "content": [{"type": "text", "text": f"The computer executor isn't reachable: {exc}"}]})
        try:
            data = resp.json()
        except Exception:
            data = {}
        if not isinstance(data, dict):
            data = {}
        if resp.status_code != 200 and not data.get("content"):
            msg = data.get("message") or data.get("error") or f"executor HTTP {resp.status_code}"
            data.setdefault("is_error", True)
            data["content"] = [{"type": "text", "text": str(msg)}]
        return ToolsetReply(resp.status_code, data)

    async def approve(self, approval_id: str) -> bool:
        """Approve a pending grant after a person said yes in the engine's own
        approval flow (the same route gizzi's adapter uses)."""
        import httpx
        try:
            async with httpx.AsyncClient(timeout=10.0) as client:
                resp = await client.post(f"{self._base}/api/aci/handoff/{approval_id}/approve",
                                         json={}, headers=self._headers())
            return resp.status_code < 300
        except Exception as exc:
            logger.warning("approval grant failed: %s", exc)
            return False

    async def run_with_approval(self, toolset: str, member: str, input: Dict[str, Any], confirm, **kw) -> ToolsetReply:
        """Run a call; on 409 ask ``confirm()`` (a person), approve the grant
        and retry once with it. ``confirm`` may be sync or async."""
        reply = await self.run(toolset, member, input, **kw)
        grant = reply.approval_id
        if not grant:
            return reply
        decision = confirm() if confirm else False
        if hasattr(decision, "__await__"):
            decision = await decision
        if not decision:
            return ToolsetReply(403, {"is_error": True, "error": "approval_denied",
                                      "content": [{"type": "text", "text": f"{member} was not approved."}]})
        if not await self.approve(grant):
            return ToolsetReply(403, {"is_error": True, "error": "approval_failed",
                                      "content": [{"type": "text", "text": "The approval could not be recorded."}]})
        kw["approval_grant"] = grant
        return await self.run(toolset, member, input, **kw)
