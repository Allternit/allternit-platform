"""Batch dispatch for the planning loop — P2 of the stagehand-batch-fork spec.

When a plan carries N≥2 consecutively groundable actions in the 11-action
whitelist vocabulary, the planning loop ships them as ONE batch through the
P1 grant surface (``POST {ALLTERNIT_API_URL}/api/aci/batch``) instead of N
step-by-step turns.

Policy stays on the Rust side: this module never decides which steps are
risky (ConfirmationClass), never computes grant hashes, and never validates
the whitelist beyond mapping — ``plan_batch`` / ``enforce_*`` in
``cmd/allternit-api/src/aci_batch.rs`` own all of that. The engine only:

1. Maps vision actions to whitelist steps (returning ``None`` when any action
   is outside the vocabulary, so the caller falls back to step-by-step).
2. Presents the batch with a mode preference and handles both response
   shapes: the executed receipt, and ``confirmation_required`` (one-grant:
   retry with ``approvalId``; per-step denial: retry with the grant placed at
   the denial's ``step_index``).

Session-preservation contract (docs/session-preservation-contract.md): the
planning loop writes the batch-context ledger record around the dispatch;
per-step outcomes live in the Rust receipt this response points to.
"""

from __future__ import annotations

import asyncio
import logging
import os
import re
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional

logger = logging.getLogger(__name__)

# The 11-action vocabulary, mirrored from BATCH_ACTION_WHITELIST in
# cmd/allternit-api/src/aci_batch.rs. The Rust side is authoritative; this
# copy only lets the engine decide "batchable at all?" without an RPC.
WHITELIST_METHODS = frozenset({
    "click", "fill", "type", "press", "scrollTo", "nextChunk", "prevChunk",
    "selectOptionFromDropdown", "hover", "doubleClick", "dragAndDrop",
})

# Planning-loop action types → whitelist methods. Anything not listed is not
# batchable (coordinate-only actions, navigate, wait, ...).
_ACTION_METHOD_MAP = {
    # Contract members (allternit.computer.v1 / allternit.browser.v1) the
    # planner's immediate_action carries, then the batch's own verbs.
    "left_click": "click",
    "click": "click",
    "double_click": "doubleClick",
    "fill": "fill",
    "type": "fill",
    "select": "selectOptionFromDropdown",
    "scroll": "scrollTo",
    "key": "press",
}

# BatchStep selectors must be CSS selectors or XPath — a grounding model emits
# free-text element descriptions ("submit button") which the in-browser runtime
# cannot resolve. Only selector-like targets are batchable; everything else
# keeps the existing coordinate-based step-by-step path. Attribute selectors
# (input[placeholder='Name']) are deterministic and resolve via querySelector
# exactly like #id or bare-tag selectors, so they ground too (cu22 campaign:
# frontier models prefer them even when ids exist).
_SELECTOR_LIKE = re.compile(
    r"^(//|#|\.|\[|[a-zA-Z][a-zA-Z0-9_-]*(\[[^\]]+\])?$)"
)

MIN_BATCH_STEPS = 2


def action_to_batch_step(action: Any) -> Optional[Dict[str, Any]]:
    """Map one VisionAction-like object to a whitelist batch step.

    Returns ``None`` when the action type is outside the vocabulary or the
    target is not a selector-like string — the caller then falls back to the
    per-step execution path for the whole candidate group.
    """
    method = _ACTION_METHOD_MAP.get(getattr(action, "type", ""))
    if method is None or method not in WHITELIST_METHODS:
        return None
    target = (getattr(action, "target", "") or "").strip()
    if not _SELECTOR_LIKE.match(target):
        return None
    arguments: List[str] = []
    text = getattr(action, "text", None)
    if text and method in ("fill", "press", "selectOptionFromDropdown"):
        arguments.append(str(text))
    return {"method": method, "selector": target, "arguments": arguments}


def actions_to_batch_steps(actions: List[Any]) -> Optional[List[Dict[str, Any]]]:
    """Map a candidate action group to whitelist steps, or ``None`` if any
    member is not batchable (fewer than MIN_BATCH_STEPS, out-of-vocabulary
    action, or non-selector target)."""
    if len(actions) < MIN_BATCH_STEPS:
        return None
    steps: List[Dict[str, Any]] = []
    for action in actions:
        step = action_to_batch_step(action)
        if step is None:
            return None
        steps.append(step)
    return steps


# BrowserWorkflowSpec step kinds (record→teach) → whitelist methods. This is
# deliberately smaller than EXECUTOR_ACTION_MAP in core/workflow_runner.py:
# navigate would rebind the page mid-batch (violating the single page binding),
# and wait/extract/screenshot/download are outside the 11-action vocabulary.
_WORKFLOW_METHOD_MAP = {
    "click": "click",
    "type": "fill",
    "press": "press",
    "scroll": "scrollTo",
    "select": "selectOptionFromDropdown",
    "hover": "hover",
}

# Workflow step input keys that carry the single text argument, per method.
_WORKFLOW_TEXT_KEYS = {
    "fill": ("text", "value"),
    "press": ("key", "text", "value"),
    "selectOptionFromDropdown": ("value", "option", "text"),
}


def workflow_step_to_batch_step(step: Dict[str, Any]) -> Optional[Dict[str, Any]]:
    """Map one BrowserWorkflowSpec step (``{id, kind, input, target}``) to a
    whitelist batch step. Assumes the caller already applied ``{{param}}``
    substitution. Returns ``None`` when the kind is outside the batch
    vocabulary or the target ref is not selector-like — the caller then keeps
    the existing per-step runner for the whole workflow.
    """
    method = _WORKFLOW_METHOD_MAP.get(str(step.get("kind") or ""))
    if method is None or method not in WHITELIST_METHODS:
        return None
    target = step.get("target") or {}
    selector = str(target.get("ref") or target.get("description") or "").strip()
    if not _SELECTOR_LIKE.match(selector):
        return None
    input_data = step.get("input") or {}
    arguments: List[str] = []
    for key in _WORKFLOW_TEXT_KEYS.get(method, ()):
        value = input_data.get(key)
        if value is not None and str(value) != "":
            arguments.append(str(value))
            break
    return {"method": method, "selector": selector, "arguments": arguments}


def workflow_steps_to_batch_steps(
    steps: List[Dict[str, Any]],
    requires_approval_for: Optional[Any] = None,
) -> Optional[List[Dict[str, Any]]]:
    """Compile a full workflow step list to ONE batch descriptor's steps.

    Returns ``None`` (caller falls back to per-step, unchanged) when any step
    is not batchable: fewer than MIN_BATCH_STEPS, an out-of-vocabulary kind, a
    non-selector target, or a kind named in the spec's
    ``safety.requiresApprovalFor`` (those pauses stay on the per-step path).
    """
    requires_approval_for = requires_approval_for or set()
    if len(steps) < MIN_BATCH_STEPS:
        return None
    batch_steps: List[Dict[str, Any]] = []
    for step in steps:
        if str(step.get("kind") or "") in requires_approval_for:
            return None
        batch_step = workflow_step_to_batch_step(step)
        if batch_step is None:
            return None
        batch_steps.append(batch_step)
    return batch_steps


async def observe_adapter_page_url(adapter: Any) -> Optional[str]:
    """Best-effort current page URL from a live adapter.

    Browser adapters (playwright/CDP family) expose ``get_url()``; plain
    adapters may not. Returns ``None`` when the surface carries no URL
    (non-browser adapter, closed page, transport hiccup) — callers then keep
    whatever binding they already had.
    """
    if adapter is None:
        return None
    get_url = getattr(adapter, "get_url", None)
    if get_url is None:
        return None
    try:
        url = get_url()
        if asyncio.iscoroutine(url):
            url = await url
        url = str(url or "").strip()
        return url or None
    except Exception as exc:
        logger.debug("adapter get_url() unavailable: %s", exc)
        return None


@dataclass
class BatchDispatchResult:
    """Normalized outcome of one dispatch attempt (or approved retry)."""

    executed: bool                                  # False → caller falls back to step-by-step
    confirmation_required: bool = False             # Rust gate asked for a grant
    approval_id: Optional[str] = None               # Grant id to redeem after human approval
    step_index: Optional[int] = None                # Per-step denial: place grant here
    action_hash: Optional[str] = None               # Descriptor or step hash the grant binds
    descriptor_hash: Optional[str] = None
    receipt: Optional[Dict[str, Any]] = None        # Full batch receipt when executed
    receipt_id: Optional[str] = None
    enforcement: Optional[str] = None
    status_code: int = 0
    error: Optional[str] = None                     # Hard failure (5xx, transport, ...)
    # F1 (observation disconnect): the batch execution context's own post-batch
    # state (screenshot_b64/url/title from the SAME sidecar browser the batch
    # ran in). None = the gate did not capture one; the caller then observes
    # through its existing adapter path, exactly as before.
    post_batch_observation: Optional[Dict[str, Any]] = None


class AciBatchClient:
    """Thin HTTP client for the P1 batch invocation surface.

    Auth mirrors CustomerCloudBackend: ``x-allternit-internal-token`` when
    ``ALLTERNIT_INTERNAL_SERVICE_TOKEN`` is set; local dev servers also accept
    ``x-allternit-user-id`` (sent unconditionally — harmless when ignored).
    """

    def __init__(
        self,
        base_url: Optional[str] = None,
        token: Optional[str] = None,
        user_id: str = "computer-use-planning-loop",
        timeout_s: float = 150.0,
    ) -> None:
        self._base = (base_url or os.environ.get("ALLTERNIT_API_URL")
                      or "http://localhost:8013").rstrip("/")
        self._token = token or os.environ.get("ALLTERNIT_INTERNAL_SERVICE_TOKEN")
        self._user_id = user_id
        self._timeout_s = timeout_s

    @property
    def base_url(self) -> str:
        return self._base

    async def execute_batch(
        self,
        *,
        steps: List[Dict[str, Any]],
        mode: str,
        origin: str = "aci.batch",
        session: Optional[str] = None,
        page_url: Optional[str] = None,
        approval_id: Optional[str] = None,
        step_approval_ids: Optional[List[Optional[str]]] = None,
        headless: bool = True,
    ) -> BatchDispatchResult:
        """One dispatch attempt. Never raises for gate/refusal responses; a
        transport-level failure returns ``executed=False`` with ``error``."""
        body: Dict[str, Any] = {
            "origin": origin,
            "mode": mode,
            "steps": steps,
            "headless": headless,
        }
        if session:
            body["session"] = session
        if page_url:
            body["pageUrl"] = page_url
        if approval_id:
            body["approvalId"] = approval_id
        if step_approval_ids:
            body["stepApprovalIds"] = step_approval_ids

        headers = {"Content-Type": "application/json", "x-allternit-user-id": self._user_id}
        if self._token:
            headers["x-allternit-internal-token"] = self._token

        try:
            import httpx
        except ImportError:
            return BatchDispatchResult(executed=False, error="httpx not installed")

        try:
            async with httpx.AsyncClient(timeout=self._timeout_s) as client:
                resp = await client.post(f"{self._base}/api/aci/batch", json=body, headers=headers)
        except Exception as exc:
            logger.warning("batch dispatch transport failed: %s", exc)
            return BatchDispatchResult(executed=False, error=f"transport: {exc}")

        data: Dict[str, Any] = {}
        try:
            data = resp.json()
        except Exception:
            data = {}

        if resp.status_code == 403 and data.get("error") == "confirmation_required":
            return BatchDispatchResult(
                executed=False,
                confirmation_required=True,
                approval_id=data.get("approval_id"),
                # Per-step denials name the step index (added to the denial body
                # for exactly this consumer); one-grant denials omit it.
                step_index=data.get("step_index"),
                action_hash=data.get("action_hash"),
                status_code=resp.status_code,
            )

        if resp.status_code >= 400:
            return BatchDispatchResult(
                executed=False,
                status_code=resp.status_code,
                error=data.get("message") or data.get("error") or f"HTTP {resp.status_code}",
            )

        receipt = data.get("receipt") if isinstance(data.get("receipt"), dict) else None
        observation = data.get("post_batch_observation")
        return BatchDispatchResult(
            executed=True,
            descriptor_hash=data.get("descriptor_hash"),
            receipt=receipt,
            receipt_id=data.get("receipt_id"),
            enforcement=data.get("enforcement"),
            status_code=resp.status_code,
            post_batch_observation=observation if isinstance(observation, dict) else None,
        )


def place_grant_for_retry(
    mode: str,
    attempt: BatchDispatchResult,
    approval_id: str,
    step_count: int,
) -> Dict[str, Any]:
    """Build the retry kwargs for an approved grant.

    Policy-free: whichever field the Rust gate consults for its chosen plan is
    filled; the other is ignored. One-grant denial → ``approvalId``; per-step
    denial → ``stepApprovalIds[step_index]``.
    """
    if attempt.step_index is not None:
        step_approval_ids: List[Optional[str]] = [None] * step_count
        step_approval_ids[attempt.step_index] = approval_id
        return {"approval_id": None, "step_approval_ids": step_approval_ids}
    return {"approval_id": approval_id, "step_approval_ids": None}
