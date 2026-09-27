"""
Allternit Computer Use — Planning Loop (Non-Claude Model Path)

Plan → Act → Observe → Reflect loop for non-Claude LLM-driven computer automation.
Used when the active model is GPT-4o, Gemini, Qwen, or any model without native
computer-use tool support. Pairs with vision_providers.py for perception.

Claude's native computer tool bypasses this entirely — Claude calls the ACU executor
directly via the gateway without a planning loop.

Inspired by Agent S (72.6% OSWorld), ScreenAgent (IJCAI 2024).
"""

from __future__ import annotations

import asyncio
import logging
import sys
import time
import uuid
from dataclasses import dataclass, field
from typing import Any, Awaitable, Callable, Dict, List, Optional, Tuple, AsyncIterator
from datetime import datetime, timezone
from enum import Enum

logger = logging.getLogger(__name__)


class StopReason(Enum):
    DONE = "done"
    MAX_STEPS = "max_steps"
    MAX_COST = "max_cost"
    TIMEOUT = "timeout"
    ERROR = "error"
    APPROVAL_DENIED = "approval_denied"
    MONITOR_FLAG = "monitor_flag"
    CANCELLED = "cancelled"


@dataclass
class LoopStep:
    """One complete Plan→Act→Observe→Reflect cycle."""
    step: int
    run_id: str
    session_id: str
    timestamp: str = field(default_factory=lambda: datetime.now(timezone.utc).isoformat())

    # Plan phase
    reasoning: str = ""
    plan_steps: List[str] = field(default_factory=list)
    risk_level: str = "low"
    requires_approval: bool = False

    # Action
    action_type: str = ""
    action_target: str = ""
    action_params: Dict[str, Any] = field(default_factory=dict)

    # Observe phase
    before_screenshot_b64: str = ""
    after_screenshot_b64: str = ""

    # Reflect phase
    reflection: str = ""
    action_succeeded: bool = True

    # Verification
    verification_evidence: Optional[Dict] = None

    # AX tree context
    ax_tree_snapshot: Optional[Dict] = None
    element_refs: Optional[Dict] = None

    # Cost tracking
    tokens_used: int = 0
    cost_usd: float = 0.0

    # Result
    adapter_result: Optional[Dict] = None
    error: Optional[str] = None

    def to_dict(self, include_screenshots: bool = True) -> Dict:
        d = {
            "step": self.step,
            "run_id": self.run_id,
            "session_id": self.session_id,
            "timestamp": self.timestamp,
            "reasoning": self.reasoning,
            "plan_steps": self.plan_steps,
            "risk_level": self.risk_level,
            "requires_approval": self.requires_approval,
            "action_type": self.action_type,
            "action_target": self.action_target,
            "action_params": self.action_params,
            "reflection": self.reflection,
            "action_succeeded": self.action_succeeded,
            "tokens_used": self.tokens_used,
            "cost_usd": self.cost_usd,
            "error": self.error,
        }
        if include_screenshots:
            d["before_screenshot_b64"] = self.before_screenshot_b64
            d["after_screenshot_b64"] = self.after_screenshot_b64
        return d


@dataclass
class PlanningLoopConfig:
    """Configuration for a planning loop run."""
    max_steps: int = 20
    max_cost_usd: float = 1.0
    timeout_ms: int = 120_000
    reflect_after_each_step: bool = True
    approval_policy: str = "on-risk"   # never | on-risk | always
    record: bool = True
    vision_provider: Optional[str] = None  # override env
    # P2 batch dispatch (core/batch_dispatch.py): when a plan carries N≥2
    # groundable whitelist actions, ship them as one grant-bound batch through
    # the Rust /api/aci/batch surface instead of N step-by-step turns.
    batch_enabled: bool = True
    batch_mode: str = "batch"          # "batch" (one grant) | "per_step"
    batch_headless: bool = True
    # Optional operator-pinned page binding folded into the batch descriptor
    # hash; wins over the auto-tracked URL (deferral B). When None, the loop
    # observes the adapter's current URL after each step/batch and pins the
    # NEXT batch's descriptor binding to it; surfaces with no URL keep the
    # origin+session-only binding.
    batch_page_url: Optional[str] = None
    # C2 code mode (core/code_mode.py): the THIRD integration mode, explicitly
    # opt-in per run — never the default. When False (the default), a plan
    # carrying a `code` payload falls through to the whitelist/batch paths
    # unchanged.
    code_mode_enabled: bool = False


@dataclass
class PlanningLoopResult:
    """Final result of a planning loop run."""
    run_id: str
    session_id: str
    task: str
    status: str                          # completed | failed | needs_approval | cancelled | timeout
    stop_reason: StopReason
    steps: List[LoopStep] = field(default_factory=list)
    summary: str = ""
    total_cost_usd: float = 0.0
    total_tokens: int = 0
    total_input_tokens: int = 0         # prompt tokens, when providers report the split
    total_output_tokens: int = 0        # completion tokens, when reported
    duration_ms: int = 0
    final_screenshot_b64: str = ""
    error: Optional[str] = None
    gif_path: Optional[str] = None
    model_turns: int = 0                # planning-loop turns consumed (P3 turns-per-task substrate)

    def to_dict(self) -> Dict:
        return {
            "run_id": self.run_id,
            "session_id": self.session_id,
            "task": self.task,
            "status": self.status,
            "stop_reason": self.stop_reason.value,
            "steps": [s.to_dict() for s in self.steps],
            "summary": self.summary,
            "total_cost_usd": self.total_cost_usd,
            "total_tokens": self.total_tokens,
            "total_input_tokens": self.total_input_tokens,
            "total_output_tokens": self.total_output_tokens,
            "duration_ms": self.duration_ms,
            "error": self.error,
            "gif_path": self.gif_path,
            "model_turns": self.model_turns,
        }


# Event types emitted during the loop — consumed by SSE streaming
LOOP_EVENTS = [
    "plan.created",
    "action.started",
    "screenshot.captured",
    "action.completed",
    "action.verified",
    "reflection.done",
    "approval.required",
    "approval.received",
    "ax_tree.captured",
    "coordinate.contract",
    "cursor.moved",
    "element.targeted",
    "window.discovered",
    "notification.received",
    "page.observed",
    "run.completed",
    "run.failed",
]


class PlanningLoop:
    """
    Core Plan→Act→Observe→Reflect execution engine.

    Usage:
        loop = PlanningLoop(vision_provider, adapter, config)
        result = await loop.run(task, session_id, run_id)

    With streaming:
        async for event in loop.run_streaming(task, session_id, run_id):
            yield f"event: {event['type']}\\ndata: {json.dumps(event)}\\n\\n"
    """

    def __init__(
        self,
        vision_provider,          # VisionProvider instance
        adapter,                  # BaseAdapter or ComputerUseExecutor
        config: Optional[PlanningLoopConfig] = None,
        recorder=None,            # Optional ActionRecorder
        event_callback: Optional[Callable[[Dict], None]] = None,
        approval_callback: Optional[Callable[[LoopStep], bool]] = None,
        history_preflight: Optional[Callable[[str], Awaitable[Optional[Dict]]]] = None,
        monitor: Optional[Any] = None,  # core.monitor.Monitor implementation
        ledger: Optional[Callable[[str, Dict], None]] = None,  # canonical EventLedger writer (session/run bound by caller)
        batch_client: Optional[Any] = None,  # AciBatchClient; default constructed lazily
        code_client: Optional[Any] = None,   # AciCodeClient (core/code_mode.py); default constructed lazily
    ):
        self.vision_provider = vision_provider
        # Accept ComputerUseExecutor directly — it has the same execute() interface
        # as BaseAdapter, so no wrapping is needed.
        self.adapter = adapter
        self.config = config or PlanningLoopConfig()
        self.recorder = recorder
        self.event_callback = event_callback
        self.approval_callback = approval_callback
        self.history_preflight = history_preflight
        self.monitor = monitor
        self.ledger = ledger
        self.batch_client = batch_client
        self.code_client = code_client
        self._cancelled = False
        self._monitor_history: List[Dict[str, Any]] = []
        # Auto page binding (deferral B): current URL observed from the
        # adapter after each step/batch; feeds the NEXT batch's descriptor.
        self._observed_url: Optional[str] = None

    def cancel(self) -> None:
        """Cancel a running loop."""
        self._cancelled = True

    async def run(self, task: str, session_id: str, run_id: Optional[str] = None) -> PlanningLoopResult:
        """Run the planning loop to completion."""
        run_id = run_id or f"cu-{uuid.uuid4().hex[:12]}"
        steps: List[LoopStep] = []
        start_ms = time.time() * 1000
        total_cost = 0.0
        total_tokens = 0
        total_input_tokens = 0
        total_output_tokens = 0
        history: List[Dict] = []
        current_screenshot: bytes = b""
        stop_reason = StopReason.ERROR
        error_msg: Optional[str] = None
        _consecutive_screenshots = 0
        model_turns = 0

        # Inject scratchpad context — strategy + skills + lessons from prior runs
        try:
            from .scratchpad import get_scratchpad
            _scratchpad = get_scratchpad()
            sp_context = _scratchpad.build_context(task)
        except Exception:
            _scratchpad = None
            sp_context = ""

        # Prepend scratchpad context to task so the vision provider sees it
        augmented_task = (task + "\n\n" + sp_context) if sp_context else task

        # Deterministic Computer History consultation for continuation/recent-work requests.
        history_context = await self._consult_history(task)
        if history_context:
            augmented_task = history_context + "\n\n" + augmented_task

        # AX inspector — init once, use throughout loop. `_ax_context` must be
        # bound before the augmentation below reads it (it previously was not,
        # which made every run fail with UnboundLocalError at this point).
        _inspector = None
        _ax_context = ""
        if _ax_context:
            augmented_task = augmented_task + "\n\n[ACCESSIBILITY TREE (skeleton)]:\n" + _ax_context

        try:
            from .accessibility_inspector import AccessibilityInspector
            _inspector = AccessibilityInspector()
            if not await _inspector.is_available():
                _inspector = None
        except Exception:
            _inspector = None

        try:
            # Initial screenshot
            current_screenshot = await self._capture_screenshot(session_id)
            self._emit({"type": "screenshot.captured", "run_id": run_id, "step": 0, "phase": "initial",
                        "screenshot_b64": _bytes_to_b64(current_screenshot)})

            # Emit coordinate contract from initial screenshot dimensions
            if _inspector:
                try:
                    contract = await _inspector.get_coordinate_contract()
                    self._emit({"type": "coordinate.contract", "run_id": run_id, **contract.to_dict()})
                except Exception:
                    pass

            # Initial AX skeleton snapshot
            if _inspector:
                try:
                    ax_root = await _inspector.snapshot(skeleton=True)
                    if ax_root:
                        _ax_context = _inspector._to_tree_text(ax_root)
                        self._emit({"type": "ax_tree.captured", "run_id": run_id, "step": 0,
                                   "surface": "window", "skeleton": True,
                                   "tree": ax_root.to_dict(compact=True)})
                except Exception:
                    pass

            for step_num in range(1, self.config.max_steps + 1):
                if self._cancelled:
                    stop_reason = StopReason.CANCELLED
                    break

                elapsed = time.time() * 1000 - start_ms
                if elapsed > self.config.timeout_ms:
                    stop_reason = StopReason.TIMEOUT
                    break

                if total_cost >= self.config.max_cost_usd:
                    stop_reason = StopReason.MAX_COST
                    break

                step = LoopStep(step=step_num, run_id=run_id, session_id=session_id)
                step.before_screenshot_b64 = _bytes_to_b64(current_screenshot) if current_screenshot else ""

                # PLAN phase
                try:
                    b64_screenshot = _bytes_to_b64(current_screenshot) if current_screenshot else ""
                    # ground_and_reason expects screenshot_b64 (str), not screenshot_bytes
                    plan = await self.vision_provider.ground_and_reason(
                        screenshot_b64=b64_screenshot,
                        task=augmented_task,
                        history=[h.get("action", "") + ": " + h.get("observation", "") for h in history],
                    )
                except Exception as e:
                    # Fallback: use analyze_screenshot (async) to avoid event-loop conflict
                    logger.warning(f"ground_and_reason failed: {e}, falling back to analyze_screenshot")
                    from .vision_providers import ActionPlan, VisionAction
                    try:
                        resp = await self.vision_provider.analyze_screenshot(b64_screenshot, augmented_task)
                        plan = ActionPlan(
                            reasoning=getattr(resp, "raw_response", "") or "",
                            plan_steps=[task],
                            immediate_action=resp.action or VisionAction(type="screenshot", target="screen", reason=""),
                            confidence=resp.confidence,
                            done=False,
                            tokens_used=getattr(resp, "tokens_used", 0),
                            cost_usd=getattr(resp, "cost_usd", 0.0),
                            input_tokens=getattr(resp, "input_tokens", 0),
                            output_tokens=getattr(resp, "output_tokens", 0),
                        )
                    except Exception as fallback_err:
                        logger.error("vision fallback failed: %s", fallback_err)
                        stop_reason = StopReason.ERROR
                        error_msg = str(fallback_err)
                        steps.append(step)
                        break

                step.reasoning = plan.reasoning
                step.plan_steps = plan.plan_steps
                step.risk_level = plan.risk_level
                step.requires_approval = plan.requires_approval
                step.action_type = plan.immediate_action.type
                step.action_target = plan.immediate_action.target
                step.action_params = {
                    "coordinates": plan.immediate_action.coordinates,
                    "text": getattr(plan.immediate_action, "text", None),
                }
                step.tokens_used = getattr(plan, "tokens_used", 0)
                step.cost_usd = getattr(plan, "cost_usd", 0.0)
                total_tokens += step.tokens_used
                total_cost += step.cost_usd
                total_input_tokens += int(getattr(plan, "input_tokens", 0) or 0)
                total_output_tokens += int(getattr(plan, "output_tokens", 0) or 0)
                model_turns += 1

                self._emit({"type": "plan.created", "run_id": run_id, "step": step_num,
                           "reasoning": plan.reasoning, "action_type": plan.immediate_action.type,
                           "risk_level": plan.risk_level})

                # Stall detection: if vision keeps returning "screenshot" with no progress, fail fast
                if plan.immediate_action.type == "screenshot":
                    _consecutive_screenshots += 1
                    if _consecutive_screenshots >= 3:
                        error_msg = "Vision provider stalled: 3 consecutive screenshot-only responses with no real action"
                        logger.error(error_msg)
                        stop_reason = StopReason.ERROR
                        steps.append(step)
                        break
                else:
                    _consecutive_screenshots = 0

                if plan.done:
                    stop_reason = StopReason.DONE
                    if self.recorder:
                        await self.recorder.record_frame_from_step(step)
                    steps.append(step)
                    break

                # Approval gate
                needs_approval = (
                    self.config.approval_policy == "always" or
                    (self.config.approval_policy == "on-risk" and plan.requires_approval)
                )
                if needs_approval:
                    self._emit({"type": "approval.required", "run_id": run_id, "step": step_num,
                               "action_preview": step.to_dict(), "reason": f"risk={plan.risk_level}"})
                    approved = await self._request_approval(step)
                    if not approved:
                        stop_reason = StopReason.APPROVAL_DENIED
                        steps.append(step)
                        break
                    self._emit({"type": "approval.received", "run_id": run_id, "step": step_num, "approved": True})

                # Refresh AX skeleton before each action
                if _inspector:
                    try:
                        ax_root = await _inspector.snapshot(skeleton=True)
                        if ax_root:
                            from .element_refs import get_refmap
                            _refmap = get_refmap()
                            _refmap.apply_to_tree(ax_root)
                            step.ax_tree_snapshot = ax_root.to_dict(compact=True)
                            step.element_refs = _refmap.to_dict()
                            self._emit({"type": "ax_tree.captured", "run_id": run_id, "step": step_num,
                                       "surface": "window", "skeleton": True,
                                       "tree": step.ax_tree_snapshot,
                                       "ref_map": step.element_refs})
                    except Exception:
                        pass

                # ACT phase — C2 code mode: when the run explicitly opted in
                # AND the plan carries a code payload, ship it as ONE grant-
                # bound payload through the Rust /api/aci/code surface. Never
                # the default: code_mode_enabled=False leaves this path cold.
                # A validation refusal surfaces as THIS step's observation and
                # the loop re-plans with whitelist actions — the payload is
                # never silently retried mutated (a mutation is a new
                # descriptor needing a new grant).
                code_outcome: Optional[Dict[str, Any]] = None
                if self.config.code_mode_enabled and getattr(plan, "code", None):
                    code_outcome = await self._dispatch_code(
                        step=step,
                        code_payload=plan.code,
                        session_id=session_id,
                        run_id=run_id,
                        step_num=step_num,
                    )

                # ACT phase — P2 batch dispatch: when the plan grounds N≥2
                # consecutive whitelist actions on the same session, ship them
                # as ONE grant-bound batch through the Rust /api/aci/batch
                # surface. Any batch failure (transport, refusal, declined
                # grant) falls back to the existing step-by-step path — policy
                # never gets decided here.
                batch_outcome: Optional[Dict[str, Any]] = None
                batch_steps: Optional[List[Dict[str, Any]]] = None
                if code_outcome is None and self.config.batch_enabled and getattr(plan, "batch", None):
                    from .batch_dispatch import actions_to_batch_steps
                    candidates = [plan.immediate_action, *plan.batch]
                    batch_steps = actions_to_batch_steps(candidates)
                    if batch_steps is not None:
                        batch_outcome = await self._dispatch_batch(
                            step=step,
                            batch_steps=batch_steps,
                            session_id=session_id,
                            run_id=run_id,
                            step_num=step_num,
                        )
                        if batch_outcome is None:
                            batch_steps = None  # fell back — run step-by-step below

                if code_outcome is not None:
                    step.action_type = "code"
                    step.action_target = f"code mode ({plan.code.get('language', 'playwright-js')})"
                    if code_outcome.get("refused"):
                        # Validation refusal: the observation the model re-
                        # plans from. Recorded, not retried.
                        refusal = {
                            "class": code_outcome.get("class"),
                            "message": code_outcome.get("message"),
                        }
                        step.action_params = {"code_refusal": refusal}
                        step.adapter_result = {"code_refusal": refusal}
                        step.action_succeeded = False
                        step.error = (
                            f"code refused ({refusal['class']}): {refusal['message']}"
                        )
                        self._emit({"type": "action.started", "run_id": run_id,
                                    "step": step_num, "action_type": "code",
                                    "target": step.action_target, "refused": True,
                                    "refusal_class": refusal["class"]})
                    else:
                        envelope = code_outcome.get("envelope") or {}
                        step.action_params = {
                            "descriptor_hash": code_outcome.get("descriptor_hash"),
                            "receipt_id": code_outcome.get("receipt_id"),
                            "exit_status": envelope.get("exit_status"),
                            "timed_out": envelope.get("timed_out", False),
                        }
                        step.adapter_result = {"code_envelope": envelope}
                        step.action_succeeded = code_outcome.get("status") == "completed"
                        if not step.action_succeeded:
                            step.error = (
                                f"code run {code_outcome.get('status')}"
                                + (": timed out" if envelope.get("timed_out") else "")
                            )
                            logger.warning("Code outcome at step %s: %s", step_num, step.error)
                        self._emit({"type": "action.started", "run_id": run_id,
                                    "step": step_num, "action_type": "code",
                                    "target": step.action_target,
                                    "exit_status": envelope.get("exit_status")})
                elif batch_outcome is not None:
                    step.action_type = "batch"
                    step.action_target = f"{len(batch_steps)} whitelisted steps"
                    step.action_params = {
                        "batch_steps": batch_steps,
                        "descriptor_hash": batch_outcome.get("descriptor_hash"),
                        "receipt_id": batch_outcome.get("receipt_id"),
                        "enforcement": batch_outcome.get("enforcement"),
                    }
                    step.adapter_result = {"batch_receipt": batch_outcome.get("receipt")}
                    step.action_succeeded = batch_outcome.get("status") == "completed"
                    if not step.action_succeeded:
                        halted = batch_outcome.get("halted_at")
                        step.error = (
                            f"batch {batch_outcome.get('status')}"
                            + (f" at step {halted}" if halted is not None else "")
                        )
                        logger.warning("Batch outcome at step %s: %s", step_num, step.error)
                    self._emit({"type": "action.started", "run_id": run_id, "step": step_num,
                               "action_type": "batch", "target": step.action_target,
                               "batch_steps": len(batch_steps)})
                else:
                    self._emit({"type": "action.started", "run_id": run_id, "step": step_num,
                               "action_type": step.action_type, "target": step.action_target})

                    # Emit cursor position when coordinates available
                    if plan.immediate_action.coordinates and len(plan.immediate_action.coordinates) >= 2:
                        cx, cy = plan.immediate_action.coordinates[0], plan.immediate_action.coordinates[1]
                        effect = "ripple" if plan.immediate_action.type in ("click", "double_click") else \
                                 "glow" if plan.immediate_action.type == "hover" else "none"
                        self._emit({"type": "cursor.moved", "run_id": run_id, "step": step_num,
                                   "x": cx, "y": cy, "agent_id": "primary", "effect": effect})

                    try:
                        result = await self._execute_action(plan.immediate_action, session_id)
                        step.adapter_result = result
                        step.action_succeeded = True
                    except Exception as act_err:
                        step.error = str(act_err)
                        step.action_succeeded = False
                        logger.warning(f"Action failed at step {step_num}: {act_err}")

                # OBSERVE phase — F1 (cu22 observation disconnect): after a
                # batch, observe the SAME browser the batch executed in (the
                # grant gate's sidecar), not the operator-facing adapter
                # surface. The adapter view may be stale or a different page
                # entirely; re-planning against it amplified grants in the
                # real-model campaign (12 grants on extract-then-act). When
                # the gate captured no observation, keep the adapter path.
                batch_observation = (
                    batch_outcome.get("post_batch_observation")
                    if batch_outcome is not None else None
                )
                if not isinstance(batch_observation, dict):
                    batch_observation = None
                new_screenshot = b""
                if batch_observation and batch_observation.get("screenshot_b64"):
                    try:
                        import base64 as _b64
                        new_screenshot = _b64.b64decode(batch_observation["screenshot_b64"])
                    except Exception as obs_err:
                        logger.warning("post-batch observation undecodable (%s) — "
                                       "falling back to adapter capture", obs_err)
                        new_screenshot = await self._capture_screenshot(session_id)
                else:
                    new_screenshot = await self._capture_screenshot(session_id)
                step.after_screenshot_b64 = _bytes_to_b64(new_screenshot) if new_screenshot else ""
                # Deferral B: track the current page URL for the NEXT batch's
                # descriptor binding (operator pin in config still wins). The
                # batch execution context's URL wins over the adapter's — it
                # is the surface the batch actually navigated.
                batch_obs_url = (batch_observation or {}).get("url")
                if batch_obs_url and str(batch_obs_url).strip():
                    url = str(batch_obs_url).strip()
                    if url != self._observed_url:
                        self._emit({"type": "page.observed", "run_id": step.run_id,
                                    "step": step.step, "url": url})
                    self._observed_url = url
                else:
                    await self._update_observed_url(step)
                self._emit({"type": "screenshot.captured", "run_id": run_id, "step": step_num, "phase": "after_action",
                            "screenshot_b64": step.after_screenshot_b64})
                self._emit({"type": "action.completed", "run_id": run_id, "step": step_num,
                           "success": step.action_succeeded})

                # STATE VERIFICATION
                try:
                    from .state_verification import StateVerifier, StateCapture
                    _verifier = StateVerifier(inspector=_inspector)
                    before_capture = StateCapture(
                        token=_verifier.make_token(current_screenshot, None),
                        screenshot_bytes=current_screenshot,
                        ax_tree=None,
                        captured_at=step.timestamp,
                    )
                    after_capture = StateCapture(
                        token=_verifier.make_token(new_screenshot, None),
                        screenshot_bytes=new_screenshot,
                        ax_tree=None,
                        captured_at=datetime.now(timezone.utc).isoformat(),
                    )
                    evidence = _verifier.verify(before_capture, after_capture, step.action_type)
                    step.verification_evidence = evidence.to_dict()
                    self._emit({"type": "action.verified", "run_id": run_id, "step": step_num,
                               "verified_success": evidence.verified_success,
                               "confidence": evidence.confidence,
                               "changed": evidence.changed,
                               "notes": evidence.notes})
                except Exception:
                    pass

                # REFLECT phase
                if self.config.reflect_after_each_step:
                    step.reflection = await self._reflect(
                        current_screenshot, new_screenshot, plan.immediate_action, step.action_succeeded
                    )
                    self._emit({"type": "reflection.done", "run_id": run_id, "step": step_num,
                               "reflection": step.reflection})

                current_screenshot = new_screenshot

                # Update history for next step (cap to last 8 to bound context growth)
                history.append({
                    "action": f"{step.action_type} on {step.action_target}",
                    "observation": step.reflection or ("succeeded" if step.action_succeeded else f"failed: {step.error}"),
                })
                if len(history) > 8:
                    history = history[-8:]

                # MONITOR: after each observe step, the monitor may pause the run
                self._monitor_history.append({
                    "action_type": step.action_type,
                    "action_target": step.action_target,
                })
                monitor_decision = await self._run_monitor(
                    step=step_num, run_id=run_id, session_id=session_id,
                    screenshot_b64=step.after_screenshot_b64,
                    extracted_text=_extracted_text_of(step.adapter_result),
                )
                if monitor_decision is not None:
                    self._emit({"type": "approval.required", "run_id": run_id, "step": step_num,
                               "action_preview": step.to_dict(),
                               "reason": f"monitor_flag: {monitor_decision.reason}",
                               "monitor_flag": monitor_decision.flag})
                    approved = await self._request_approval(step)
                    if not approved:
                        stop_reason = StopReason.MONITOR_FLAG
                        steps.append(step)
                        break
                    self._emit({"type": "approval.received", "run_id": run_id, "step": step_num,
                               "approved": True, "monitor_flag": monitor_decision.flag})

                # Record step
                if self.recorder:
                    await self.recorder.record_frame_from_step(step)

                steps.append(step)

            else:
                stop_reason = StopReason.MAX_STEPS

        except Exception as e:
            error_msg = str(e)
            logger.error(f"Planning loop error: {e}", exc_info=True)
            stop_reason = StopReason.ERROR

        duration_ms = int(time.time() * 1000 - start_ms)
        status_map = {
            StopReason.DONE: "completed",
            StopReason.MAX_STEPS: "completed",
            StopReason.MAX_COST: "completed",
            StopReason.TIMEOUT: "failed",
            StopReason.ERROR: "failed",
            StopReason.APPROVAL_DENIED: "needs_approval",
            StopReason.MONITOR_FLAG: "needs_approval",
            StopReason.CANCELLED: "cancelled",
        }
        status = status_map.get(stop_reason, "failed")

        result = PlanningLoopResult(
            run_id=run_id,
            session_id=session_id,
            task=task,
            status=status,
            stop_reason=stop_reason,
            steps=steps,
            summary=self._build_summary(steps, stop_reason),
            total_cost_usd=total_cost,
            total_tokens=total_tokens,
            total_input_tokens=total_input_tokens,
            total_output_tokens=total_output_tokens,
            duration_ms=duration_ms,
            final_screenshot_b64=_bytes_to_b64(current_screenshot) if current_screenshot else "",
            error=error_msg,
            model_turns=model_turns,
        )

        event_type = "run.completed" if status in ("completed",) else "run.failed"
        self._emit({"type": event_type, "run_id": run_id, "status": status,
                   "summary": result.summary, "duration_ms": duration_ms})

        # Post-run reflection — compound learnings across runs
        if _scratchpad is not None:
            try:
                sp_outcome = "success" if status == "completed" else ("failure" if status == "failed" else status)
                await _scratchpad.reflect(
                    task=task,
                    steps=steps,
                    outcome=sp_outcome,
                    session_id=session_id,
                    duration_s=duration_ms / 1000,
                    tokens_used=total_tokens,
                    error_summary=error_msg or "",
                )
            except Exception as sp_err:
                logger.warning("[planning-loop] scratchpad reflect failed: %s", sp_err)

        return result

    async def run_streaming(self, task: str, session_id: str, run_id: Optional[str] = None) -> AsyncIterator[Dict]:
        """Run with streaming — yields events as they happen."""
        events: asyncio.Queue[Optional[Dict]] = asyncio.Queue()

        original_callback = self.event_callback

        def collect_event(event: Dict) -> None:
            events.put_nowait(event)
            if original_callback:
                original_callback(event)

        self.event_callback = collect_event

        async def run_loop():
            result = await self.run(task, session_id, run_id)
            events.put_nowait(None)  # sentinel
            return result

        loop_task = asyncio.create_task(run_loop())

        while True:
            event = await events.get()
            if event is None:
                break
            yield event

        await loop_task
        self.event_callback = original_callback

    def _is_executor(self) -> bool:
        """True if self.adapter is a ComputerUseExecutor (has waterfall dispatch)."""
        return hasattr(self.adapter, "registered_adapters")

    async def _capture_screenshot(self, session_id: str) -> bytes:
        """Capture screenshot via adapter or executor, then the host display."""
        png = await self._capture_screenshot_adapter(session_id)
        if png:
            return png
        host = self._capture_screenshot_host()
        if host:
            logger.info("Screenshot capture fell back to host display (%s bytes)", len(host))
        return host

    def _capture_screenshot_host(self) -> bytes:
        """Last-resort capture of the actual computer screen.

        Adapter health often fails (no browser extension, no pyautogui). On
        macOS, `screencapture` still shows the paired Desktop.
        """
        import os
        import subprocess
        import tempfile

        if sys.platform != "darwin":
            return b""
        fd, path = tempfile.mkstemp(suffix=".png")
        os.close(fd)
        try:
            subprocess.run(
                ["screencapture", "-x", path],
                check=True,
                timeout=8,
                capture_output=True,
            )
            with open(path, "rb") as handle:
                data = handle.read()
            return data if len(data) > 32 else b""
        except Exception as exc:
            logger.warning("Host screenshot capture failed: %s", exc)
            return b""
        finally:
            try:
                os.unlink(path)
            except OSError:
                pass

    async def _capture_screenshot_adapter(self, session_id: str) -> bytes:
        """Capture screenshot via adapter or executor."""
        import base64 as _b64
        try:
            # Compatibility adapters may expose a direct screenshot method.
            if hasattr(self.adapter, "screenshot") and not self._is_executor():
                return await self.adapter.screenshot(session_id)

            run_id = str(uuid.uuid4())
            req = type("ActionRequest", (), {
                "action_type": "screenshot", "target": "", "parameters": {},
            })()

            if self._is_executor():
                result = await self.adapter.execute(req, session_id=session_id, run_id=run_id)
            else:
                result = await self.adapter.execute(req)

            if result is None:
                return b""

            ec = getattr(result, "extracted_content", None) or {}
            if isinstance(ec, dict):
                data_url = ec.get("data_url", "")
                if data_url.startswith("data:"):
                    return _b64.b64decode(data_url.split(",", 1)[-1])

            artifacts = getattr(result, "artifacts", []) or []
            for art in artifacts:
                art_type = art.type if hasattr(art, "type") else art.get("type", "")
                if art_type == "screenshot":
                    content = art.content if hasattr(art, "content") else art.get("content")
                    if content:
                        return _b64.b64decode(content)
                    path = art.path if hasattr(art, "path") else art.get("path")
                    if path:
                        with open(path, "rb") as f:
                            return f.read()
            return b""
        except Exception as e:
            logger.warning("Screenshot capture failed: %s", e)
            return b""

    async def _execute_action(self, action, session_id: str) -> Dict:
        """Execute a VisionAction through the adapter or executor."""
        params: Dict[str, Any] = {}
        if action.coordinates:
            if len(action.coordinates) >= 2:
                params["x"] = int(action.coordinates[0])
                params["y"] = int(action.coordinates[1])
        if hasattr(action, "text") and action.text:
            params["text"] = action.text

        run_id = str(uuid.uuid4())
        req = type("ActionRequest", (), {
            "action_type": action.type,
            "target": action.target or "",
            "parameters": params,
        })()

        if self._is_executor():
            result = await self.adapter.execute(req, session_id=session_id, run_id=run_id)
        else:
            result = await self.adapter.execute(req)
        return result.to_dict() if result else {}

    def _batch_page_binding(self) -> Optional[str]:
        """Effective page binding for the next batch descriptor: the operator
        pin wins; otherwise the last URL observed from the adapter. None =
        bind origin+session only (non-browser surface or nothing observed)."""
        return self.config.batch_page_url or self._observed_url

    async def _update_observed_url(self, step: LoopStep) -> None:
        """Track the current page URL from the live adapter (deferral B).

        Best-effort and silent: adapters without a URL surface (desktop
        automation, host-display fallback) leave the binding unchanged. Also
        scans the step's adapter result for a URL when the adapter carries
        one there instead of behind ``get_url()``.
        """
        url: Optional[str] = None
        try:
            from .batch_dispatch import observe_adapter_page_url
            url = await observe_adapter_page_url(self.adapter)
        except Exception:
            url = None
        if not url:
            url = _url_of_result(step.adapter_result)
        if url:
            if url != self._observed_url:
                self._emit({"type": "page.observed", "run_id": step.run_id,
                            "step": step.step, "url": url})
            self._observed_url = url

    async def _dispatch_batch(
        self,
        *,
        step: LoopStep,
        batch_steps: List[Dict[str, Any]],
        session_id: str,
        run_id: str,
        step_num: int,
    ) -> Optional[Dict[str, Any]]:
        """Dispatch a grant-bound batch through the P1 Rust surface.

        Returns the outcome dict when the batch executed (whatever the step
        outcomes — halt-at-first-failure is reported honestly, not retried),
        or ``None`` when the caller should fall back to step-by-step execution
        (transport failure, declined grant, or an unexpected gate response).
        Grant policy (which steps are risky, which mode applies) is decided
        entirely Rust-side; this method only presents the batch and, on a
        ``confirmation_required`` denial, routes the grant request through the
        SAME human-approval flow as per-step risk approval.
        """
        from .batch_context import (
            BatchContextRecord,
            close_batch_context,
            open_batch_context,
        )
        from .batch_dispatch import AciBatchClient, place_grant_for_retry

        client = self.batch_client
        if client is None:
            try:
                client = AciBatchClient()
            except Exception as exc:
                logger.warning("Batch client unavailable, falling back to step-by-step: %s", exc)
                return None
            self.batch_client = client

        record = BatchContextRecord(
            run_id=run_id,
            session_id=session_id,
            step_count=len(batch_steps),
            step_methods=[s["method"] for s in batch_steps],
            batch_mode=self.config.batch_mode,
            page_url=self._batch_page_binding(),
        )
        # Contract §4: opened BEFORE the batch RPC (audit-before-act).
        open_batch_context(self.ledger, record)

        async def _try(approval_id=None, step_approval_ids=None):
            return await client.execute_batch(
                steps=batch_steps,
                mode=self.config.batch_mode,
                session=session_id,
                page_url=self._batch_page_binding(),
                approval_id=approval_id,
                step_approval_ids=step_approval_ids,
                headless=self.config.batch_headless,
            )

        attempt = await _try()

        if attempt.confirmation_required:
            record.batch_id = record.descriptor_hash = attempt.action_hash or "pending"
            # Surface the grant request through the same approval flow as
            # per-step risk approval; the SSE events distinguish it by reason.
            self._emit({"type": "approval.required", "run_id": run_id, "step": step_num,
                       "action_preview": {"kind": "batch", "descriptor_hash": attempt.action_hash,
                                          "step_index": attempt.step_index,
                                          "step_count": len(batch_steps)},
                       "reason": f"batch grant required (hash={attempt.action_hash})"})
            approved = await self._request_approval(step)
            self._emit({"type": "approval.received", "run_id": run_id, "step": step_num,
                       "approved": approved})
            if not approved or not attempt.approval_id:
                close_batch_context(self.ledger, record, status="denied",
                                    model_turns_saved=0)
                logger.info("Batch grant declined at step %s — step-by-step fallback", step_num)
                return None
            retry_kwargs = place_grant_for_retry(
                self.config.batch_mode, attempt, attempt.approval_id, len(batch_steps)
            )
            attempt = await _try(**retry_kwargs)
            if not attempt.executed:
                close_batch_context(
                    self.ledger, record,
                    status="denied" if attempt.confirmation_required else "failed",
                    model_turns_saved=0,
                )
                logger.warning("Batch grant retry did not execute (%s) — step-by-step fallback",
                               attempt.error or "still confirmation_required")
                return None

        if not attempt.executed:
            close_batch_context(self.ledger, record, status="failed", model_turns_saved=0)
            logger.warning("Batch dispatch failed (%s) — step-by-step fallback", attempt.error)
            return None

        receipt = attempt.receipt
        if not isinstance(receipt, dict) or not receipt.get("status"):
            # Audit-before-act runs both ways: the gate always returns a
            # receipt on success, so a 200 without one is not evidence of
            # anything. Fail closed rather than report an unproven batch.
            close_batch_context(self.ledger, record, status="failed", model_turns_saved=0)
            logger.warning("Batch dispatch returned no receipt — failing closed, "
                           "step-by-step fallback")
            return None
        status = receipt.get("status", "completed")
        halted_at = receipt.get("halted_at")
        steps_completed = sum(
            1 for s in receipt.get("steps", []) if s.get("status") == "completed"
        )
        record.batch_id = record.descriptor_hash = attempt.descriptor_hash or "unknown"
        close_batch_context(
            self.ledger, record,
            status=status,
            halted_at=halted_at,
            steps_completed=steps_completed,
            receipt_id=attempt.receipt_id,
            model_turns_saved=len(batch_steps) - 1,
        )
        return {
            "status": status,
            "halted_at": halted_at,
            "descriptor_hash": attempt.descriptor_hash,
            "receipt_id": attempt.receipt_id,
            "enforcement": attempt.enforcement,
            "receipt": receipt,
            # F1: the batch's own execution-context state, when the gate
            # captured it; None keeps the caller's adapter observation path.
            "post_batch_observation": attempt.post_batch_observation,
        }

    async def _dispatch_code(
        self,
        *,
        step: LoopStep,
        code_payload: Dict[str, Any],
        session_id: str,
        run_id: str,
        step_num: int,
    ) -> Optional[Dict[str, Any]]:
        """Dispatch a grant-bound code payload through the C0 Rust surface.

        Returns one of:
          * ``{"executed": True, ...}`` — the payload ran; the fixed envelope
            is the model-visible observation.
          * ``{"refused": True, "class", "message"}`` — descriptor-time
            validation refusal; the caller surfaces it as the step's
            observation so the loop re-plans with whitelist actions. Never
            retried mutated (a mutation is a new descriptor + new grant).
          * ``None`` — declined/failed dispatch: the caller falls back to the
            existing whitelist paths, exactly like a declined batch grant.

        Grant policy (refuse-list, single-use, expiry) is decided entirely
        Rust-side; this method only presents the payload and routes a
        ``confirmation_required`` denial through the SAME human-approval flow
        as per-step risk approval.
        """
        from .code_mode import (
            AciCodeClient,
            CodeContextRecord,
            close_code_context,
            open_code_context,
        )

        client = self.code_client
        if client is None:
            try:
                client = AciCodeClient()
            except Exception as exc:
                logger.warning("Code client unavailable, falling back to whitelist: %s", exc)
                return None
            self.code_client = client

        language = str(code_payload.get("language") or "playwright-js")
        code = str(code_payload.get("code") or "")
        declared_targets = [str(t) for t in (code_payload.get("declaredTargets") or [])]

        record = CodeContextRecord(
            run_id=run_id,
            session_id=session_id,
            language=language,
            code_bytes=len(code.encode()),
            declared_targets=declared_targets,
        )
        # Contract §8: opened BEFORE the code RPC (audit-before-act).
        open_code_context(self.ledger, record)

        async def _try(approval_id=None):
            return await client.execute_code(
                code=code,
                language=language,
                declared_targets=declared_targets,
                session=session_id,
                approval_id=approval_id,
            )

        attempt = await _try()

        if attempt.refused:
            close_code_context(self.ledger, record, status="refused")
            return {
                "refused": True,
                "class": attempt.refusal_class,
                "message": attempt.refusal_message,
            }

        if attempt.confirmation_required:
            record.code_id = record.descriptor_hash = attempt.action_hash or "pending"
            self._emit({"type": "approval.required", "run_id": run_id, "step": step_num,
                        "action_preview": {"kind": "code", "descriptor_hash": attempt.action_hash,
                                           "language": language,
                                           "code_bytes": record.code_bytes},
                        "reason": f"code grant required (hash={attempt.action_hash})"})
            approved = await self._request_approval(step)
            self._emit({"type": "approval.received", "run_id": run_id, "step": step_num,
                        "approved": approved})
            if not approved or not attempt.approval_id:
                close_code_context(self.ledger, record, status="denied")
                logger.info("Code grant declined at step %s — whitelist fallback", step_num)
                return None
            attempt = await _try(approval_id=attempt.approval_id)
            if not attempt.executed:
                close_code_context(
                    self.ledger, record,
                    status="denied" if attempt.confirmation_required else "failed",
                )
                logger.warning("Code grant retry did not execute (%s) — whitelist fallback",
                               attempt.error or "still confirmation_required")
                return None

        if not attempt.executed:
            close_code_context(self.ledger, record, status="failed")
            logger.warning("Code dispatch failed (%s) — whitelist fallback", attempt.error)
            return None

        envelope = attempt.envelope
        if not isinstance(envelope, dict):
            # Audit-before-act runs both ways: the gate always returns the
            # fixed envelope on success — a 200 without one is not evidence.
            close_code_context(self.ledger, record, status="failed")
            logger.warning("Code dispatch returned no envelope — failing closed")
            return None

        clean = (
            envelope.get("exit_status") == 0
            and not envelope.get("timed_out")
            and not envelope.get("refused")
            and not envelope.get("error")
        )
        record.code_id = record.descriptor_hash = attempt.descriptor_hash or "unknown"
        close_code_context(
            self.ledger,
            record,
            status="completed" if clean else "failed",
            receipt_id=attempt.receipt_id,
            exit_status=envelope.get("exit_status"),
            timed_out=bool(envelope.get("timed_out")),
        )
        return {
            "executed": True,
            "status": "completed" if clean else "failed",
            "descriptor_hash": attempt.descriptor_hash,
            "receipt_id": attempt.receipt_id,
            "envelope": envelope,
            "receipt": attempt.receipt,
        }

    async def _reflect(self, before: bytes, after: bytes, action, succeeded: bool) -> str:
        """Ask the vision provider to reflect on what changed."""
        if not before or not after:
            return "succeeded" if succeeded else "failed (no screenshot)"
        try:
            if before == after:
                return "screen unchanged — action may not have had effect"
            prompt = f"The previous action was '{action.type}' on '{action.target}'. Did it succeed? What changed on screen? One sentence."
            response = await self.vision_provider.analyze_screenshot(_bytes_to_b64(after), prompt)
            return response.raw_response or ("succeeded" if succeeded else "failed")
        except Exception:
            return "succeeded" if succeeded else "failed"

    async def _request_approval(self, step: LoopStep) -> bool:
        """Request approval from callback or default allow."""
        if self.approval_callback:
            try:
                if asyncio.iscoroutinefunction(self.approval_callback):
                    coro = self.approval_callback(step)
                else:
                    loop = asyncio.get_event_loop()
                    coro = loop.run_in_executor(None, self.approval_callback, step)
                return await asyncio.wait_for(coro, timeout=300.0)
            except asyncio.TimeoutError:
                return False
        return True

    async def _run_monitor(self, *, step: int, run_id: str, session_id: str,
                           screenshot_b64: str, extracted_text: str) -> Optional[Any]:
        """Evaluate the pluggable monitor after an observe step.

        Never raises: a failing monitor must not break the loop.
        Returns a MonitorDecision with action "pause", or None.
        """
        if self.monitor is None:
            return None
        try:
            try:
                from .monitor import MonitorDecision
            except Exception:
                MonitorDecision = None  # type: ignore[assignment]
            decision = await self.monitor.evaluate(
                screenshot_b64=screenshot_b64 or "",
                extracted_text=extracted_text,
                action_type=self._monitor_history[-1].get("action_type", "") if self._monitor_history else "",
                action_target=self._monitor_history[-1].get("action_target", "") if self._monitor_history else "",
                step=step,
                run_id=run_id,
                session_id=session_id,
                history=list(self._monitor_history),
            )
            if decision is None:
                return None
            if MonitorDecision is not None and isinstance(decision, MonitorDecision):
                if decision.action != "pause":
                    return None
                return decision
            # Duck-typed decision (any object with action/reason/flag).
            if getattr(decision, "action", "continue") != "pause":
                return None
            return decision
        except Exception as exc:
            logger.debug("Monitor evaluation failed at step %s: %s", step, exc)
            return None

    def _should_consult_history(self, task: str) -> bool:
        """Trigger history consultation for continuation / recent-work requests."""
        text = task.lower()
        triggers = (
            "continue", "resume", "pick up", "finish", "complete",
            "where did we leave off", "where did i leave off",
            "what did we do", "what did i do", "recent", "prior run",
            "last time", "previous session", "earlier",
        )
        return any(trigger in text for trigger in triggers)

    async def _consult_history(self, task: str) -> Optional[str]:
        """Call the deterministic history preflight and return a context string."""
        if not self.history_preflight or not self._should_consult_history(task):
            return None
        try:
            result = await self.history_preflight(task)
        except Exception as exc:
            logger.debug("History preflight failed: %s", exc)
            return None
        if not isinstance(result, dict):
            return None

        status = result.get("status") or {}
        if not status.get("supported") or not status.get("admitted") or not status.get("enabled"):
            return None

        events = result.get("events") or []
        if not events:
            return None

        lines: List[str] = [
            "[COMPUTER HISTORY CONTEXT]",
            f"History state: enabled={'true' if status.get('enabled') else 'false'}, "
            f"paused={'true' if status.get('paused') else 'false'}, "
            f"health={status.get('health', 'unknown')}, "
            f"dropped_events={status.get('dropped_events', 0)}",
            f"Retrieved {len(events)} recent metadata event(s):",
        ]
        for event in events:
            if not isinstance(event, dict):
                continue
            data = event.get("data") or {}
            payload = data.get("payload") or {}
            lines.append(
                f"- seq={data.get('sequence')}, type={event.get('type')}, "
                f"capability={data.get('capability')}, kind={payload.get('kind')}, "
                f"effect={payload.get('effect')}, route={payload.get('route')}"
            )
        lines.append(
            "Use this metadata as a lead only. Do not reconstruct typed text, arguments, results, "
            "file paths, window titles, URLs, or user intent from omitted fields."
        )
        return "\n".join(lines)

    def _emit(self, event: Dict) -> None:
        if self.event_callback:
            try:
                self.event_callback(event)
            except Exception as e:
                logger.debug("Event callback error for %s: %s", event.get("type"), e)

    def _build_summary(self, steps: List[LoopStep], stop_reason: StopReason) -> str:
        if not steps:
            return f"No steps completed. Stopped: {stop_reason.value}"
        actions = [f"{s.action_type}({s.action_target[:20]})" for s in steps]
        return f"Completed {len(steps)} step(s): {', '.join(actions[:5])}{'...' if len(actions) > 5 else ''}. Stopped: {stop_reason.value}"


def _bytes_to_b64(data: bytes) -> str:
    import base64
    return base64.b64encode(data).decode("utf-8") if data else ""


def _extracted_text_of(adapter_result: Optional[Dict]) -> str:
    """Best-effort text extraction from an adapter result dict for monitor scans."""
    if not isinstance(adapter_result, dict):
        return ""
    content = adapter_result.get("extracted_content")
    if isinstance(content, str):
        return content
    if isinstance(content, dict):
        parts = []
        for key in ("text", "content", "title", "result", "url"):
            value = content.get(key)
            if isinstance(value, str):
                parts.append(value)
        return "\n".join(parts)
    return ""


def _url_of_result(adapter_result: Optional[Dict]) -> Optional[str]:
    """Pull a page URL out of an adapter result dict, when the surface carries
    one (browser extract/navigate results). None otherwise."""
    if not isinstance(adapter_result, dict):
        return None
    for key in ("url", "page_url", "newPageUrl"):
        value = adapter_result.get(key)
        if isinstance(value, str) and value.startswith(("http://", "https://")):
            return value
    content = adapter_result.get("extracted_content")
    if isinstance(content, dict):
        for key in ("url", "page_url", "newPageUrl"):
            value = content.get(key)
            if isinstance(value, str) and value.startswith(("http://", "https://")):
                return value
    return None
