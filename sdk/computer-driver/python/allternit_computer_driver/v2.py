"""Contract v2 (``allternit.computer.v2``): the ten driver-backed structured
members — read_ui, act, run_batch, verify, request_human, use_credential,
run_subtask, run_parallel, run_skill, skills — as typed client methods, plus
the ``computer_v2`` function tool every adapter exposes next to the pixel tool.

Input types come from ``_v2_types.py`` (emitted by
contracts/computer-toolset/generate.mjs from allternit-computer-v2.json), so
they cannot drift from the server contract. Result dicts are the executor's
JSON answers (allternit-api computer_v2.rs / computer_subtask.rs /
computer_parallel.rs and the driver docs tools/allternit-driver.mdx).
"""

from __future__ import annotations

import json
from typing import Any, Callable, Dict, List, Optional

from ._v2_types import (
    COMPUTER_V2_MEMBER_NAMES,
    COMPUTER_V2_STRUCTURED_MEMBER_NAMES,
    COMPUTER_V2_CONTRACT as V2_CONTRACT,
    ComputerV2ActInput,
    ComputerV2ReadUiInput,
    ComputerV2RequestHumanInput,
    ComputerV2RunBatchInput,
    ComputerV2RunParallelInput,
    ComputerV2RunSkillInput,
    ComputerV2RunSubtaskInput,
    ComputerV2SkillsInput,
    ComputerV2UseCredentialInput,
    ComputerV2VerifyInput,
    member_schema,
)
from .client import AllternitComputers, ToolsetResult, result_text

__all__ = [
    "COMPUTER_V2_MEMBER_NAMES", "COMPUTER_V2_STRUCTURED_MEMBER_NAMES", "V2_CONTRACT", "member_schema",
    "ComputerV2ActInput", "ComputerV2ReadUiInput", "ComputerV2RequestHumanInput", "ComputerV2RunBatchInput",
    "ComputerV2RunParallelInput", "ComputerV2RunSkillInput", "ComputerV2RunSubtaskInput", "ComputerV2SkillsInput",
    "ComputerV2UseCredentialInput", "ComputerV2VerifyInput",
    "V2_TOOL_MARKER", "COMPUTER_V2_TOOL_NAME", "ComputerV2Error", "ComputerV2Driver",
    "run_computer_v2_member", "computer_v2_tool_description", "computer_v2_parameters", "computer_v2_tool",
]

V2_TOOL_MARKER = "[allternit.computer.v2]"
"""Marker the Allternit wire tooling uses to recognise the v2 function tool."""
COMPUTER_V2_TOOL_NAME = "computer_v2"
"""The function-tool name the structured members are exposed under."""

ApprovalHandler = Callable[[Dict[str, Any]], Any]
"""``on_approval(approval) -> bool``: true approves the held call and resends it with the grant."""


class ComputerV2Error(RuntimeError):
    """A v2 member answered ``is_error: True``, or returned no JSON where JSON was expected."""

    def __init__(self, member: str, result: ToolsetResult, message: Optional[str] = None) -> None:
        super().__init__(message or result_text(result) or f"The {member} call failed.")
        self.member = member
        self.result = result


def run_computer_v2_member(
    client: AllternitComputers,
    computer_id: str,
    member: str,
    input: Optional[Dict[str, Any]] = None,
    on_approval: Optional[ApprovalHandler] = None,
) -> ToolsetResult:
    """Run one structured v2 member through POST /v1/computers/{id}/toolset, answering an approval hold via ``on_approval``."""
    call: Dict[str, Any] = {"toolset": "computer", "member": member, "input": dict(input or {})}
    if on_approval is not None:
        return client.toolset_with_approval(computer_id, call, on_approval)
    return client.toolset(computer_id, call)


class ComputerV2Driver:
    """Typed methods for the ten structured v2 members of one computer.

    Methods return the executor's parsed JSON dict. ``request_human`` and
    ``use_credential`` return the answer text. A member failure raises
    :class:`ComputerV2Error` carrying the raw result.

    Result field shapes (see the Allternit Driver docs, tools/allternit-driver):

    - ``read_ui`` — ``window`` (pid/window_id/app/title), ``version``,
      ``engine``, ``cached``, ``live``, ``source`` (ax/vision/hybrid),
      ``source_reason``, ``elements`` (each: ``id`` (``e…`` tree / ``v…``
      vision), ``role``, ``name``, ``value``, ``bbox`` [x, y, w, h],
      ``enabled``, ``focused``, ``actions``, ``parent``, ``mark``) or ``diff``
      (``added``/``changed``/``removed``, or ``reset: True`` when the ``since``
      version aged out), ``total``, ``truncated``, ``degraded``,
      ``degraded_reason``, ``marks``, ``vision_version``, ``grounded``.
    - ``act`` — ``status`` (done/changed/stale/stale_version; stale_version
      carries the fresh map under ``map``), ``version``, ``changes``.
    - ``run_batch`` — ``ok``, ``steps`` (each with ``status``/``ms``; failures
      add ``code``/``error``), ``version``, ``failed_at``, ``cross_check``.
    - ``verify`` — ``ok``, ``results`` (each ``check``/``ok``/``detail``).
    - ``run_subtask`` / ``run_skill`` — ``status`` (done/escalated/failed/
      needs_confirmation/paused/denied/use_api), ``goal``, ``steps``,
      ``decisions``, ``actions``, ``elapsed_ms``, ``oracle`` (``tokens`` /
      ``cost_usd``), ``cache`` (``status`` hit/healed/recorded/miss/diverged,
      ``replayed_steps``, ``healed_steps``, ``stored``, ``skill``), ``reason``,
      ``held_step`` (needs_confirmation/paused/denied), ``api`` (use_api),
      ``screen`` and ``next`` for anything but done.
    - ``run_parallel`` — ``status`` (done/partial/failed; escalated for
      best-of), ``done``, ``subtasks``, ``results`` (one run_subtask result
      each, plus ``subtask``/``computer``; budget-misses come back
      ``skipped``), ``elapsed_ms``, ``cost_usd``, ``next``; best-of-N adds
      ``best_of``, ``rollouts`` (``rollout``/``computer``/``status``/
      ``narrative``/``result``), ``chosen`` and ``judge_decisions``.
    - ``skills`` — ``skills`` (each ``name``/``goal``/``app``/``inputs``/
      ``steps``/``runs``/``healed``/``updated_at``/``last_used_at``),
      ``forgot`` when one was deleted first.
    """

    def __init__(self, client: AllternitComputers, computer_id: str, on_approval: Optional[ApprovalHandler] = None) -> None:
        self._client, self._computer_id, self._on_approval = client, computer_id, on_approval

    def _run(self, member: str, input: Dict[str, Any]) -> ToolsetResult:
        return run_computer_v2_member(self._client, self._computer_id, member, input, self._on_approval)

    def _json(self, member: str, input: Dict[str, Any]) -> Dict[str, Any]:
        res = self._run(member, input)
        if res.get("is_error"):
            raise ComputerV2Error(member, res)
        text = result_text(res)
        try:
            parsed = json.loads(text)
        except ValueError:
            raise ComputerV2Error(member, res, f"The {member} call returned no JSON.") from None
        if not isinstance(parsed, dict):
            raise ComputerV2Error(member, res, f"The {member} call returned no JSON object.")
        return parsed

    def read_ui(self, input: Optional["ComputerV2ReadUiInput"] = None) -> Dict[str, Any]:
        """Read the target window or app's UI as a structured element tree (no screenshot)."""
        return self._json("read_ui", dict(input or {}))

    def act(self, input: "ComputerV2ActInput") -> Dict[str, Any]:
        """Act on an element id from read_ui."""
        return self._json("act", dict(input))

    def run_batch(self, input: "ComputerV2RunBatchInput") -> Dict[str, Any]:
        """Run ordered steps in one window in a single call; a failed check stops the batch."""
        return self._json("run_batch", dict(input))

    def verify(self, input: "ComputerV2VerifyInput") -> Dict[str, Any]:
        """Check bounded conditions on the current UI without acting."""
        return self._json("verify", dict(input))

    def request_human(self, input: Optional["ComputerV2RequestHumanInput"] = None) -> str:
        """Pause and hand the computer to a person; returns when they signal done or the timeout elapses."""
        res = self._run("request_human", dict(input or {}))
        if res.get("is_error"):
            raise ComputerV2Error("request_human", res)
        return result_text(res)

    def use_credential(self, input: "ComputerV2UseCredentialInput") -> str:
        """Type a vault credential into the focused field; the value never enters the model context."""
        res = self._run("use_credential", dict(input))
        if res.get("is_error"):
            raise ComputerV2Error("use_credential", res)
        return result_text(res)

    def run_subtask(self, input: "ComputerV2RunSubtaskInput") -> Dict[str, Any]:
        """Hand a bounded UI subtask to the fast decision loop."""
        return self._json("run_subtask", dict(input))

    def run_parallel(self, input: "ComputerV2RunParallelInput") -> Dict[str, Any]:
        """Run independent bounded subtasks on separate computers at the same time."""
        return self._json("run_parallel", dict(input))

    def run_skill(self, input: "ComputerV2RunSkillInput") -> Dict[str, Any]:
        """Run a saved skill (a recording taught with run_subtask save_as) by name."""
        return self._json("run_skill", dict(input))

    def skills(self, input: Optional["ComputerV2SkillsInput"] = None) -> Dict[str, Any]:
        """List the saved skills this person can run (pass forget to delete one first)."""
        return self._json("skills", dict(input or {}))


# Steering every model gets for the structured members. The canonical copy
# gizzi sends is cmd/gizzi-code/src/runtime/tools/computer-toolset/adapter.ts
# (PREFER_STRUCTURED / PREFER_SUBTASK / toolDescriptionV2) — keep the wording
# in sync when either side changes.
_PREFER_STRUCTURED = " ".join([
    "Prefer read_ui + act/run_batch over screenshot + pixel-by-pixel loops: the element tree is faster, cheaper and stable across resizes.",
    'When the tree is empty (canvas/game/remote desktop), read_ui falls back to vision by itself: elements with source "vision" and a mark number act like any other id; pass target (e.g. \'the Export button\') to ground one element. Reach for screenshots only when that still isn\'t enough.',
])

_PREFER_SUBTASK = " ".join([
    "Plan, then delegate: give each bounded UI step sequence (fill a form, search and pick, toggle settings) to run_subtask with the goal, the literal inputs it may type and success checks, instead of choosing every click yourself.",
    "A typical task is one run_subtask call plus your final answer. Keep your own calls for planning, for judgment the subtask hands back (status escalated: continue from the screen it returns), and for steps outside the UI.",
    "API over GUI: when one of your MCP tools or an API does the goal, call it instead of driving the screen; when unsure, pass the candidates as run_subtask api_options (status use_api names the one to call).",
    "Independent subtasks on separate computers go in one run_parallel call. For a high-value subtask, best_of N with N sandbox computers runs N rollouts and a judge picks one by their step narratives (never on the person's own machine).",
])


def computer_v2_tool_description() -> str:
    """Description gizzi-equivalent for the ``computer_v2`` tool, built from the contract's member descriptions."""
    lines = [f"- {m['name']}: {m['description']}" for m in V2_CONTRACT["members"] if m["name"] in COMPUTER_V2_STRUCTURED_MEMBER_NAMES]
    return "\n".join([
        f"{V2_TOOL_MARKER} Read and drive this computer's UI as a structured element tree (no screenshots needed). Set `action` to one of the actions below and pass that action's fields.",
        _PREFER_SUBTASK,
        _PREFER_STRUCTURED,
        "Element ids come from read_ui and are stable until the UI changes; pass the version back to act/run_batch to catch a moved UI.",
        "run_batch runs many steps in one call and stops at the first failed check, then returns one fresh read — batch aggressively.",
        "use_credential types a vault secret or TOTP code into the focused field; the value is never shown to you.",
        "request_human pauses for a person (CAPTCHA, 2FA, judgment calls) and resumes the session when they signal done.",
        "",
        *lines,
    ])


def computer_v2_parameters() -> Dict[str, Any]:
    """The combined JSON Schema for the ``computer_v2`` tool: ``action`` (one of the ten structured members) plus the union of every member's input fields, each optional.

    A call maps 1:1 onto ``{member: action, input: <the rest>}``; the server
    validates against the member's own contract schema.
    """
    properties: Dict[str, Any] = {
        "action": {"type": "string", "enum": list(COMPUTER_V2_STRUCTURED_MEMBER_NAMES), "description": "Which computer_v2 action to run."},
    }
    for name in COMPUTER_V2_STRUCTURED_MEMBER_NAMES:
        schema = member_schema(name) or {}
        for key, value in (schema.get("properties") or {}).items():
            if key not in properties:
                properties[key] = value
    return {"type": "object", "properties": properties, "required": ["action"], "additionalProperties": False}


def computer_v2_tool() -> Dict[str, Any]:
    """The provider-neutral ``computer_v2`` function tool definition.

    Adapt the pieces to your SDK's spelling (Anthropic ``input_schema``, the
    Responses API ``parameters`` on a function tool, Gemini declaration
    ``parameters``).
    """
    return {"name": COMPUTER_V2_TOOL_NAME, "description": computer_v2_tool_description(), "schema": computer_v2_parameters()}
