"""
Allternit Computer Use — REST API Router

/v1/computer-use/ surface: execute, runs, sessions, adapters, record, recordings, replay, health.

Execution model:
  mode='direct' → /execute dispatches the explicit actions list through the
                  adapter/executor layer directly (no planning loop, no vision model)
  Claude (native computer tool) → gateway /v1/execute → ACU executor directly
  Non-Claude models (GPT-4o, Gemini, Qwen, etc.) → /execute → PlanningLoop → executor

PlanningLoop is the non-Claude fallback path. When _planning_available=False the
/execute endpoint returns a failed status immediately so Claude is never blocked.
"""

from __future__ import annotations

import asyncio
import json
import logging
import re
import sys
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, List, Literal, Optional

from fastapi import APIRouter, HTTPException, Query
from fastapi.responses import Response, StreamingResponse
from pydantic import BaseModel, Field, model_validator

# ---------------------------------------------------------------------------
# Adapter imports — all wrapped so the router loads even if deps are missing
# ---------------------------------------------------------------------------

try:
    import sys as _sys
    import os as _os
    _sys.path.insert(0, _os.path.join(_os.path.dirname(__file__), ".."))
    from core.planning_loop import (
        PlanningLoop,
        PlanningLoopConfig,
        PlanningLoopResult,
        StopReason,
    )
    from core.vision_providers import AllternitGatewayProvider, VisionProviderFactory
    from core.computer_use_executor import get_executor as _get_executor
    from core.cost_accounting import cost_dict_from_planning_result, zero_run_cost
    from core.sandbox_env import (
        sandbox_env_context,
        scrub_secrets,
        secret_values,
        validate_sandbox_env,
    )
    _planning_available = True
except ImportError:
    PlanningLoop = None  # type: ignore[assignment,misc]
    PlanningLoopConfig = None  # type: ignore[assignment,misc]
    PlanningLoopResult = None  # type: ignore[assignment,misc]
    StopReason = None  # type: ignore[assignment,misc]
    VisionProviderFactory = None  # type: ignore[assignment,misc]
    _get_executor = None  # type: ignore[assignment]
    _planning_available = False

    def zero_run_cost() -> Dict[str, Any]:  # type: ignore[misc]
        """Fallback when core.cost_accounting is unavailable (honest zero)."""
        return {
            "input_tokens": 0,
            "output_tokens": 0,
            "total_tokens": 0,
            "est_cost_usd": 0.0,
            "pricing": "unavailable",
            "by_stage": {},
            "model": None,
            "provider": None,
        }

    def cost_dict_from_planning_result(result: Any, provider: Any = None) -> Dict[str, Any]:  # type: ignore[misc]
        return zero_run_cost()

    # Identity fallbacks: only reached when the core package itself is
    # unimportable (the planning path is dead in that state too). The
    # sandbox_env field is ignored, exactly as before this feature existed.
    def validate_sandbox_env(env: Dict[str, str]) -> Dict[str, str]:  # type: ignore[misc]
        return dict(env)

    def secret_values(env: Dict[str, str]) -> List[str]:  # type: ignore[misc]
        return []

    def scrub_secrets(value: Any, secrets: Any) -> Any:  # type: ignore[misc]
        return value

    from contextlib import nullcontext as sandbox_env_context  # type: ignore[misc]

# Playwright-based adapter (inline stub that delegates to session_manager)
try:
    from session_manager import session_manager
    _session_manager_available = True
except ImportError:
    session_manager = None  # type: ignore[assignment]
    _session_manager_available = False

try:
    from core.action_recorder import ActionRecorder, list_recordings
    _recorder_available = True
except ImportError:
    ActionRecorder = None  # type: ignore[assignment,misc]
    list_recordings = None  # type: ignore[assignment]
    _recorder_available = False

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Router
# ---------------------------------------------------------------------------

router = APIRouter(prefix="/v1/computer-use", tags=["computer-use"])


def _get_adapter_for_planning(target_scope: str, adapter_preference: Optional[str]) -> Any:
    """
    Return the adapter to pass to PlanningLoop.

    Preferred: ComputerUseExecutor singleton — gives the full waterfall
    (extension → CDP → playwright → desktop) for non-Claude paths.
    No HTTP self-loop fallback is permitted when the executor is unavailable.
    """
    if _get_executor is not None:
        try:
            executor = _get_executor()
            if executor.registered_adapters():
                return executor
        except Exception:
            pass

    return None


# ---------------------------------------------------------------------------
# Run store
# ---------------------------------------------------------------------------

def _toolset_client_for(body: Any) -> Any:
    """The shared toolset executor for this run (spec 1.2), when the engine
    runs next to allternit-api (Desktop / cloud runtime set ALLTERNIT_API_URL).
    Standalone (tests, demo) keeps the in-process adapter waterfall."""
    try:
        from core.toolset_executor import ToolsetExecutorClient, executor_configured
    except ImportError:
        return None
    opts = dict(getattr(body, "options", {}) or {})
    ctx = dict(getattr(body, "context", {}) or {})
    computer_id = opts.get("computerId") or ctx.get("computerId")
    if not executor_configured() and not computer_id:
        return None
    return ToolsetExecutorClient(
        computer_id or "this-device",
        user_id=opts.get("userId") or ctx.get("userId"),
        browser_session_id=getattr(body, "session_id", None),
        observe_toolset="browser" if getattr(body, "target_scope", "browser") == "browser" else "computer",
    )


class RunState:
    def __init__(
        self,
        run_id: str,
        session_id: str,
        mode: str,
        target_scope: str,
        sandbox_secrets: Optional[List[str]] = None,
    ) -> None:
        self.run_id = run_id
        self.session_id = session_id
        self.status: str = "pending"
        self.mode = mode
        self.target_scope = target_scope
        # Plaintext credential values held ONLY for scrubbing run output
        # (events, results, errors) before it reaches queues, receipts,
        # persistence, or clients. Never serialized, never logged.
        self.sandbox_secrets: List[str] = list(sandbox_secrets or [])
        self.created_at: str = _utcnow()
        self.updated_at: str = _utcnow()
        self.result: Optional[Dict[str, Any]] = None
        self.error: Optional[str] = None
        # Per-run token/cost accounting (observability only). Honest zero
        # until an execution path records real usage.
        self.cost: Dict[str, Any] = zero_run_cost()
        self.approval_future: Optional[asyncio.Future] = None
        self.approval_timed_out: bool = False
        self.cancel_event: asyncio.Event = asyncio.Event()
        # The live PlanningLoop, so steering can reach it (set while it runs).
        self.planning_loop: Optional[Any] = None

    def to_dict(self) -> Dict[str, Any]:
        return {
            "run_id": self.run_id,
            "session_id": self.session_id,
            "status": self.status,
            "mode": self.mode,
            "target_scope": self.target_scope,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
            "result": self.result,
            "error": self.error,
            "cost": self.cost,
            "approval_timed_out": self.approval_timed_out,
        }


_RUN_TTL_SECONDS = 3600  # purge completed/failed runs after 1 hour


class RunStore:
    def __init__(self) -> None:
        self.runs: Dict[str, RunState] = {}
        self.event_queues: Dict[str, asyncio.Queue] = {}
        self._persistence: Optional[Any] = None

    def attach_persistence(self, persistence: Any) -> None:
        """Attach a durable backend (run_persistence.RunPersistence).

        Non-terminal runs left behind by a previous process are marked
        interrupted so historical listings stay honest after a restart.
        """
        self._persistence = persistence
        try:
            interrupted = persistence.mark_interrupted()
            if interrupted:
                logger.info("Marked %d non-terminal run(s) as interrupted after restart", interrupted)
        except Exception as exc:
            logger.warning("Run history reconciliation failed: %s", exc)

    def create(
        self,
        run_id: str,
        session_id: str,
        mode: str,
        target_scope: str,
        sandbox_secrets: Optional[List[str]] = None,
    ) -> RunState:
        state = RunState(run_id, session_id, mode, target_scope, sandbox_secrets=sandbox_secrets)
        self.runs[run_id] = state
        self.event_queues[run_id] = asyncio.Queue()
        self._persist(state)
        return state

    def get(self, run_id: str) -> Optional[RunState]:
        state = self.runs.get(run_id)
        if state is not None or self._persistence is None:
            return state
        record = self._persistence.get_run(run_id)
        if record is None:
            return None
        # Historical run from a previous process — no live event queue.
        return _run_state_from_record(record)

    def list_runs(self, limit: int = 200) -> List[Dict[str, Any]]:
        """Live runs plus durable history (live wins on run_id collision)."""
        merged: Dict[str, Dict[str, Any]] = {}
        if self._persistence is not None:
            try:
                for record in self._persistence.list_runs(limit):
                    merged[record["run_id"]] = record
            except Exception as exc:
                logger.warning("Run history listing failed: %s", exc)
        for run_id, state in self.runs.items():
            merged[run_id] = state.to_dict()
        runs = sorted(merged.values(), key=lambda r: r.get("updated_at", ""), reverse=True)
        return runs[:limit]

    async def push_event(self, run_id: str, event: Dict[str, Any]) -> None:
        q = self.event_queues.get(run_id)
        if q:
            state = self.runs.get(run_id)
            if state is not None and state.sandbox_secrets:
                # A child process that echoes its own environment must not
                # leak credential values into stream frames.
                event = scrub_secrets(event, state.sandbox_secrets)
            await q.put(event)

    async def push_sentinel(self, run_id: str) -> None:
        q = self.event_queues.get(run_id)
        if q:
            await q.put(None)

    def update_status(self, run_id: str, status: str) -> None:
        state = self.runs.get(run_id)
        if state:
            state.status = status
            state.updated_at = _utcnow()
            self._persist(state)

    def finalize(self, run_id: str) -> None:
        """Persist a run's terminal state (result/error) via the optional backend."""
        state = self.runs.get(run_id)
        if state is not None:
            self._persist(state)

    def _persist(self, state: RunState) -> None:
        backend = getattr(self, "_persistence", None)
        if backend is not None:
            try:
                backend.upsert_run(state)
            except Exception as exc:
                logger.warning("Run persistence failed for %s: %s", state.run_id, exc)

    def purge_expired(self) -> int:
        """Remove completed/failed/cancelled runs older than _RUN_TTL_SECONDS."""
        terminal = {"completed", "failed", "cancelled"}
        now = datetime.now(timezone.utc)
        expired = [
            run_id for run_id, state in self.runs.items()
            if state.status in terminal
            and (now - datetime.fromisoformat(state.updated_at)).total_seconds() > _RUN_TTL_SECONDS
        ]
        for run_id in expired:
            self.runs.pop(run_id, None)
            self.event_queues.pop(run_id, None)
        return len(expired)


_run_store = RunStore()


def _run_state_from_record(record: Dict[str, Any]) -> RunState:
    """Reconstruct a RunState from a durable record (no live event queue)."""
    state = RunState(
        run_id=record["run_id"],
        session_id=record.get("session_id", ""),
        mode=record.get("mode", "unknown"),
        target_scope=record.get("target_scope", "auto"),
    )
    state.status = record.get("status", "unknown")
    state.created_at = record.get("created_at", state.created_at)
    state.updated_at = record.get("updated_at", state.updated_at)
    state.result = record.get("result")
    state.error = record.get("error")
    # Rebuild the cost record from the persisted columns.
    state.cost = zero_run_cost()
    state.cost["input_tokens"] = int(record.get("input_tokens") or 0)
    state.cost["output_tokens"] = int(record.get("output_tokens") or 0)
    state.cost["total_tokens"] = int(record.get("total_tokens") or 0)
    state.cost["est_cost_usd"] = float(record.get("est_cost_usd") or 0.0)
    if state.cost["total_tokens"] > 0:
        state.cost["pricing"] = "estimated"
    return state


def _emit_canonical(event_type: str, *, session_id: str, run_id: Optional[str] = None,
                    payload: Optional[Dict[str, Any]] = None) -> None:
    """Append a lifecycle event to the canonical event ledger (no-op on failure).

    The ledger lives in canonical_router (same gateway process); importing it
    lazily keeps unit tests light and never breaks request handling.
    """
    try:
        try:
            from canonical_router import _events as events
        except ImportError:
            from gateway.canonical_router import _events as events  # type: ignore
        events.append(
            event_type,
            session_id=session_id,
            run_id=run_id,
            payload=payload or {},
        )
    except Exception as exc:
        logger.debug("canonical event emission failed (%s): %s", event_type, exc)


def _index_recording(**fields: Any) -> None:
    """Upsert a recording into the durable recordings index (no-op on failure)."""
    persistence = getattr(_run_store, "_persistence", None)
    if persistence is None:
        return
    try:
        persistence.upsert_recording(fields)
    except Exception as exc:
        logger.debug("recordings index upsert failed: %s", exc)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _utcnow() -> str:
    return datetime.now(timezone.utc).isoformat()


def _sse_line(event_type: str, run_id: str, message: str, data: Dict[str, Any]) -> str:
    payload = json.dumps({
        "event_type": event_type,
        "run_id": run_id,
        "message": message,
        "data": data,
    })
    return f"data: {payload}\n\n"


# ---------------------------------------------------------------------------
# Pydantic models
# ---------------------------------------------------------------------------

class DirectAction(BaseModel):
    """One executable action for mode='direct' (mirrors SDK EngineAction)."""
    kind: str
    action_id: Optional[str] = None
    target: Optional[Dict[str, Any]] = None
    input: Optional[Dict[str, Any]] = None
    expect: Optional[Dict[str, Any]] = None
    metadata: Optional[Dict[str, Any]] = None


class ExecuteBody(BaseModel):
    mode: Literal["intent", "direct", "assist"] = "intent"
    task: Optional[str] = None
    actions: Optional[List[DirectAction]] = None
    session_id: str = Field(default_factory=lambda: f"sess-{uuid.uuid4().hex[:8]}")
    run_id: str = Field(default_factory=lambda: f"cu-{uuid.uuid4().hex[:12]}")
    target_scope: Literal["browser", "desktop", "hybrid", "auto"] = "browser"
    options: Dict[str, Any] = Field(default_factory=dict)
    context: Dict[str, Any] = Field(default_factory=dict)
    # Credential-injection channel (ACI server-side vault, aci_credentials.rs):
    # plaintext values for the run's sandbox environment only. Top-level —
    # deliberately NOT part of task/options — so the model context never
    # carries them. Consumed exclusively via sandbox_env_context/scrub_secrets.
    sandbox_env: Dict[str, str] = Field(default_factory=dict)

    def __repr__(self) -> str:
        # Defensive: a wholesale `repr(body)` / f-string log of the request
        # must never print credential values — variable names only.
        fields = self.model_dump()
        fields["sandbox_env"] = {key: "***" for key in self.sandbox_env}
        return f"ExecuteBody({fields!r})"

    __str__ = __repr__

    @model_validator(mode="after")
    def _require_task_or_actions(self) -> "ExecuteBody":
        if self.mode == "direct":
            if not self.actions:
                raise ValueError("mode='direct' requires a non-empty 'actions' list")
        elif not self.task:
            raise ValueError("'task' is required unless mode='direct' with actions")
        return self


class ApproveBody(BaseModel):
    decision: Literal["approve", "deny", "cancel"] = "approve"
    comment: str = ""


class SteerBody(BaseModel):
    text: str


class RecordBody(BaseModel):
    session_id: str
    action: Literal["start", "append", "stop"] = "start"
    recording_id: Optional[str] = None
    name: Optional[str] = None
    record_gif: bool = True
    # For action="append": recorded frames to append to the in-flight recording.
    frames: Optional[List[Dict[str, Any]]] = None
    # Optional video artifact metadata (chrome-stream provider recordVideo).
    # Usable on start (pre-known path) or stop (path known after finalizing).
    video_path: Optional[str] = None
    video_start_epoch: Optional[int] = None


class ReplayBody(BaseModel):
    recording_id: str
    export_gif: bool = False
    session_id: Optional[str] = None
    deviation_threshold: Optional[float] = 0.05
    wait: bool = False


class ExecutionResult(BaseModel):
    run_id: str
    session_id: str
    status: str
    mode: str
    target_scope: str
    summary: str = ""
    result: Optional[Dict[str, Any]] = None
    artifacts: List[Dict[str, Any]] = Field(default_factory=list)
    error: Optional[str] = None


# ---------------------------------------------------------------------------
# Core: run a planning loop and pipe events into the run store
# ---------------------------------------------------------------------------

async def _execute_non_claude_path(
    body: ExecuteBody,
    run_state: RunState,
) -> None:
    """Non-Claude model path: runs the PlanningLoop and pushes events to RunStore.

    Claude's native computer tool bypasses this entirely — it calls /v1/execute
    in the gateway directly, which routes to the ACU executor.
    """
    if not _planning_available:
        run_state.status = "failed"
        run_state.error = "planning_loop not available (import error)"
        run_state.updated_at = _utcnow()
        await _run_store.push_event(run_state.run_id, {
            "event_type": "run.failed",
            "run_id": run_state.run_id,
            "message": run_state.error,
            "data": {},
        })
        await _run_store.push_sentinel(run_state.run_id)
        return

    _run_store.update_status(run_state.run_id, "running")

    loop_config = PlanningLoopConfig(
        max_steps=body.options.get("max_steps", 20),
        max_cost_usd=body.options.get("max_cost_usd", 1.0),
        timeout_ms=body.options.get("timeout_ms", 120_000),
        approval_policy=body.options.get("approval_policy", "on-risk"),
        record=body.options.get("record", True),
        vision_provider=body.options.get("vision_provider"),
        batch_enabled=body.options.get("batch_enabled", True),
        batch_mode=body.options.get("batch_mode", "batch"),
        batch_page_url=body.options.get("batch_page_url"),
    )

    vp_override = body.options.get("vision_provider") or loop_config.vision_provider
    model = body.options.get("model")
    if vp_override == "mock":
        vision_provider = VisionProviderFactory.create("mock")
    else:
        # Same Gizzi provider/model picker as Home and Code. Do not fall
        # through to ak- keys, cloud VL, or a CLI subprocess.
        vision_provider = AllternitGatewayProvider(model=model)
        if model:
            logger.info("ACI using platform brain %s", model)
    # Use only in-process executor routes. The retired gateway proxy must not
    # recurse back into this process over HTTP.
    adapter = _get_adapter_for_planning(body.target_scope, body.options.get("adapter_preference"))

    # Approval callback wired through RunState.approval_future
    async def approval_callback(step) -> bool:
        loop = asyncio.get_event_loop()
        future: asyncio.Future = loop.create_future()
        run_state.approval_future = future
        _run_store.update_status(run_state.run_id, "awaiting_approval")
        await _push_approval_event(run_state, "approval.required", {
            "kind": "planning_loop",
            "step": getattr(step, "step", None),
            "risk_level": getattr(step, "risk_level", None),
            "timeout_seconds": _DIRECT_APPROVAL_TIMEOUT_SECONDS,
        })
        _emit_canonical("approval.required", session_id=run_state.session_id,
                        run_id=run_state.run_id,
                        payload={"kind": "planning_loop", "step": getattr(step, "step", None)})
        try:
            decision_payload = await asyncio.wait_for(future, timeout=_DIRECT_APPROVAL_TIMEOUT_SECONDS)
        except asyncio.TimeoutError:
            run_state.approval_future = None
            run_state.approval_timed_out = True
            _run_store.update_status(run_state.run_id, "running")
            logger.warning("Approval timed out for run %s step %s", run_state.run_id, getattr(step, "step", "?"))
            await _push_approval_event(run_state, "approval.resolved", {
                "kind": "planning_loop",
                "step": getattr(step, "step", None),
                "approved": False,
                "timed_out": True,
            })
            return False
        approved = decision_payload.get("decision") == "approve"
        run_state.approval_future = None
        _run_store.update_status(run_state.run_id, "running")
        await _push_approval_event(run_state, "approval.resolved", {
            "kind": "planning_loop",
            "step": getattr(step, "step", None),
            "approved": approved,
            "timed_out": False,
        })
        _emit_canonical("approval.resolved", session_id=run_state.session_id,
                        run_id=run_state.run_id,
                        payload={"kind": "planning_loop", "step": getattr(step, "step", None),
                                 "approved": approved})
        return approved

    # Cancel check — wire cancel_event into the planning loop
    def event_callback(event: Dict[str, Any]) -> None:
        # Approval events are owned by the router's approval_callback, which
        # emits machine-readable approval.required/approval.resolved events
        # carrying the run status — don't forward the loop's bare copies.
        if event.get("type") in ("approval.required", "approval.received"):
            return
        asyncio.create_task(_run_store.push_event(run_state.run_id, {
            "event_type": event.get("type", "unknown"),
            "run_id": run_state.run_id,
            "message": event.get("type", ""),
            "data": event,
        }))

    # Build recorder (always on by default; GIF enabled when record_gif=true)
    recorder = None
    if _recorder_available and ActionRecorder is not None and loop_config.record:
        record_gif = body.options.get("record_gif", True)
        recorder = ActionRecorder(
            task=body.task,
            session_id=body.session_id,
            run_id=body.run_id,
            vision_provider=body.options.get("vision_provider", ""),
            record_gif=record_gif,
            gif_fps=body.options.get("gif_fps", 2),
            gif_scale=body.options.get("gif_scale", 0.5),
        )
        await recorder.start()

    # Canonical ledger writer for run-scoped records (batch.context.* per the
    # session-preservation contract). Session/run bound here so the loop
    # stays testable with a plain collector.
    def ledger(event_type: str, payload: Dict[str, Any]) -> None:
        _emit_canonical(event_type, session_id=body.session_id, run_id=body.run_id,
                        payload=payload)

    planning_loop = PlanningLoop(
        vision_provider=vision_provider,
        adapter=adapter,
        config=loop_config,
        recorder=recorder,
        event_callback=event_callback,
        approval_callback=approval_callback if loop_config.approval_policy != "never" else None,
        ledger=ledger,
        toolset_client=_toolset_client_for(body),
    )

    run_state.planning_loop = planning_loop

    # Hook cancel_event into the loop
    async def _cancel_watcher() -> None:
        await run_state.cancel_event.wait()
        planning_loop.cancel()

    cancel_task = asyncio.create_task(_cancel_watcher())

    try:
        result = await planning_loop.run(
            task=body.task,
            session_id=body.session_id,
            run_id=body.run_id,
        )
        # Cost accounting: tokens/cost from the planning loop's vision calls.
        run_state.cost = cost_dict_from_planning_result(result, vision_provider)
        # Finalize recorder and attach gif_path to result
        if recorder is not None:
            await recorder.stop()
            gif_path = recorder.get_gif_path()
            if gif_path:
                result.gif_path = str(gif_path)
            _index_recording(
                recording_id=recorder.recording_id,
                task=recorder.manifest.task,
                session_id=body.session_id,
                run_id=body.run_id,
                status=recorder.manifest.status,
                started_at=recorder.manifest.started_at,
                completed_at=recorder.manifest.completed_at,
                total_steps=recorder.manifest.total_steps,
                path=str(recorder.get_path()),
                gif_path=str(gif_path) if gif_path else None,
            )
        run_state.status = result.status
        run_state.result = scrub_secrets(result.to_dict(), run_state.sandbox_secrets)
        _emit_canonical(
            "run.completed" if result.status == "completed" else "run.failed",
            session_id=body.session_id, run_id=body.run_id,
            payload={"mode": "intent", "status": result.status,
                     "steps": len(result.steps), "stop_reason": result.stop_reason.value},
        )
    except Exception as exc:
        logger.exception(
            "Planning loop raised exception: %s",
            scrub_secrets(str(exc), run_state.sandbox_secrets),
        )
        run_state.status = "failed"
        run_state.error = scrub_secrets(str(exc), run_state.sandbox_secrets)
        _emit_canonical("run.failed", session_id=body.session_id, run_id=body.run_id,
                        payload={"mode": "intent", "error": run_state.error})
        if recorder is not None:
            try:
                await recorder.stop()
            except Exception:
                pass
    finally:
        cancel_task.cancel()
        run_state.updated_at = _utcnow()
        _run_store.finalize(run_state.run_id)
        await _run_store.push_sentinel(run_state.run_id)


# ---------------------------------------------------------------------------
# Direct actions path (mode='direct'): no planning loop, no vision model.
# Each action is dispatched straight through the adapter/executor layer.
# ---------------------------------------------------------------------------

_DIRECT_APPROVAL_TIMEOUT_SECONDS = 120.0


def _serialize_direct_target(target: Optional[Dict[str, Any]]) -> str:
    """Flatten a DirectAction.target dict into the adapter target string."""
    if not target:
        return ""
    if isinstance(target, str):
        return target
    for key in ("selector", "ref", "url", "text", "id", "role"):
        value = target.get(key)
        if isinstance(value, str) and value:
            return value
    coords = target.get("coordinates") or target.get("point")
    if isinstance(coords, (list, tuple)) and len(coords) >= 2:
        return f"{int(coords[0])},{int(coords[1])}"
    return json.dumps(target, sort_keys=True)


def _serialize_adapter_result(result: Any) -> Any:
    if result is None:
        return None
    if hasattr(result, "to_dict"):
        try:
            return result.to_dict()
        except Exception:
            pass
    if isinstance(result, dict):
        return result
    if hasattr(result, "__dataclass_fields__"):
        return {k: getattr(result, k) for k in result.__dataclass_fields__}
    return str(result)


async def _run_direct_action(
    adapter: Any,
    body: ExecuteBody,
    action: DirectAction,
) -> Any:
    """Execute one DirectAction through the adapter layer."""
    if adapter is None:
        raise RuntimeError("no adapter available for direct execution")
    params: Dict[str, Any] = dict(action.input or {})
    if action.expect:
        params["_expect"] = dict(action.expect)
    try:
        from core.base_adapter import ActionRequest

        req = ActionRequest(
            action_type=action.kind,
            target=_serialize_direct_target(action.target),
            parameters=params,
        )
    except Exception:
        req = type("ActionRequest", (), {
            "action_type": action.kind,
            "target": _serialize_direct_target(action.target),
            "parameters": params,
        })()

    if hasattr(adapter, "registered_adapters"):
        result = await adapter.execute(req, session_id=body.session_id, run_id=str(uuid.uuid4()))
    else:
        result = await adapter.execute(req)
    return _serialize_adapter_result(result)


async def _execute_direct_path(
    body: ExecuteBody,
    run_state: RunState,
) -> None:
    """Execute an explicit action list against the adapter layer.

    No planning loop and no vision model: each action is dispatched in order,
    per-action outcomes are recorded, and a final screenshot artifact is
    captured when any adapter can produce one.
    """
    _run_store.update_status(run_state.run_id, "running")
    adapter = _get_adapter_for_planning(body.target_scope, body.options.get("adapter_preference"))

    # Cost accounting: the direct path makes no LLM calls — honest zero.
    run_state.cost = zero_run_cost()

    actions_out: List[Dict[str, Any]] = []
    ok_count = 0
    cancelled = False
    try:
        for index, action in enumerate(body.actions or []):
            if run_state.cancel_event.is_set():
                cancelled = True
                break
            entry: Dict[str, Any] = {
                "index": index,
                "action_id": action.action_id or f"act-{index}",
                "kind": action.kind,
                "status": "ok",
                "result": None,
                "error": None,
            }
            await _run_store.push_event(run_state.run_id, {
                "event_type": "action.started",
                "run_id": run_state.run_id,
                "message": f"{action.kind} (#{index})",
                "data": {"index": index, "action_id": entry["action_id"], "kind": action.kind},
            })
            try:
                entry["result"] = await _run_direct_action(adapter, body, action)
                ok_count += 1
            except Exception as exc:
                logger.warning(
                    "Direct action %s (%s) failed: %s",
                    index, action.kind,
                    scrub_secrets(str(exc), run_state.sandbox_secrets),
                )
                entry["status"] = "error"
                entry["error"] = scrub_secrets(str(exc), run_state.sandbox_secrets)
            actions_out.append(entry)
            await _run_store.push_event(run_state.run_id, {
                "event_type": "action.completed",
                "run_id": run_state.run_id,
                "message": f"{action.kind} (#{index}): {entry['status']}",
                "data": {
                    "index": index,
                    "action_id": entry["action_id"],
                    "kind": action.kind,
                    "status": entry["status"],
                    "error": entry["error"],
                },
            })

        screenshot_b64 = ""
        if adapter is not None:
            try:
                from core.replay_engine import capture_screenshot

                png = await capture_screenshot(adapter, body.session_id, run_state.sandbox_secrets)
                if png:
                    import base64 as _b64

                    screenshot_b64 = _b64.b64encode(png).decode("ascii")
            except Exception as exc:
                logger.warning("Direct path screenshot capture failed: %s", exc)

        total = len(actions_out)
        run_state.status = "completed" if (not cancelled and ok_count == total and total > 0) else "failed"
        run_state.result = scrub_secrets({
            "task": body.task,
            "status": run_state.status,
            "stop_reason": "done" if run_state.status == "completed" else "error",
            "actions": actions_out,
            "total_steps": total,
            "succeeded": ok_count,
            "screenshot_b64": screenshot_b64,
            "summary": (
                f"Executed {ok_count}/{total} actions"
                + (" (cancelled)" if cancelled else "")
            ),
        }, run_state.sandbox_secrets)
        if screenshot_b64:
            run_state.result["artifacts"] = [{
                "type": "screenshot",
                "mime": "image/png",
                "content": screenshot_b64,
            }]
    except Exception as exc:
        logger.exception(
            "Direct execution raised exception: %s",
            scrub_secrets(str(exc), run_state.sandbox_secrets),
        )
        run_state.status = "failed"
        run_state.error = scrub_secrets(str(exc), run_state.sandbox_secrets)
        _emit_canonical("run.failed", session_id=body.session_id, run_id=run_state.run_id,
                        payload={"mode": "direct", "error": run_state.error})
    finally:
        run_state.updated_at = _utcnow()
        _run_store.finalize(run_state.run_id)
        if run_state.status == "completed":
            _emit_canonical("run.completed", session_id=body.session_id, run_id=run_state.run_id,
                            payload={"mode": "direct", "actions": len(actions_out)})
        await _run_store.push_sentinel(run_state.run_id)


# ---------------------------------------------------------------------------
# Approval-gate events (machine-readable, shared by planning + replay paths)
# ---------------------------------------------------------------------------

async def _push_approval_event(
    run_state: RunState,
    event_type: str,
    data: Dict[str, Any],
) -> None:
    """Push an approval.* event whose data always carries the run status."""
    payload = {"status": run_state.status}
    payload.update(data)
    await _run_store.push_event(run_state.run_id, {
        "event_type": event_type,
        "run_id": run_state.run_id,
        "message": event_type,
        "data": payload,
    })


# ---------------------------------------------------------------------------
# Endpoints
# ---------------------------------------------------------------------------

@router.post("/execute", response_model=ExecutionResult)
async def execute(
    body: ExecuteBody,
    stream: bool = Query(default=False),
) -> Any:
    """Start a run. stream=true returns SSE.

    mode='direct' executes the explicit actions list through the adapter
    layer with no planning loop; any other mode runs the planning loop on
    `task`.

    sandbox_env (top-level) is the credential-injection channel from the
    ACI server-side vault: values are validated, held only in memory for
    output scrubbing, and exposed to the run's child processes through the
    process environment for the duration of the run. They are never part
    of the model context, logs, stream frames, receipts, or responses.
    """
    # Validate before creating the run so a bad variable name fails the
    # request without leaving a run record behind. Error text names only
    # the offending variable — never the value.
    try:
        sandbox_env = validate_sandbox_env(body.sandbox_env)
    except ValueError as exc:
        raise HTTPException(status_code=400, detail=str(exc))

    _run_store.purge_expired()
    run_state = _run_store.create(
        run_id=body.run_id,
        session_id=body.session_id,
        mode=body.mode,
        target_scope=body.target_scope,
        sandbox_secrets=secret_values(sandbox_env),
    )

    run_impl = _execute_direct_path if body.mode == "direct" else _execute_non_claude_path

    async def run_with_sandbox_env() -> None:
        # Child processes launched by the adapters (Playwright browser
        # processes, desktop subprocesses) inherit these variables for the
        # duration of the run; prior values are restored afterwards.
        with sandbox_env_context(sandbox_env):
            await run_impl(body, run_state)

    if stream:
        # SSE path: launch background task, stream events
        asyncio.create_task(run_with_sandbox_env())

        async def event_generator():
            q = _run_store.event_queues[body.run_id]
            while True:
                event = await q.get()
                if event is None:
                    # Send final status event then close
                    yield _sse_line(
                        "run.ended",
                        body.run_id,
                        run_state.status,
                        run_state.to_dict(),
                    )
                    break
                yield _sse_line(
                    event.get("event_type", "unknown"),
                    body.run_id,
                    event.get("message", ""),
                    event.get("data", {}),
                )

        return StreamingResponse(event_generator(), media_type="text/event-stream")

    # Non-streaming path: wait for completion
    await run_with_sandbox_env()

    return ExecutionResult(
        run_id=run_state.run_id,
        session_id=run_state.session_id,
        status=run_state.status,
        mode=run_state.mode,
        target_scope=run_state.target_scope,
        summary=run_state.result.get("summary", "") if run_state.result else "",
        result=run_state.result,
        artifacts=(run_state.result or {}).get("artifacts", []),
        error=run_state.error,
    )


@router.get("/runs")
async def list_runs_endpoint(limit: int = Query(default=200, ge=1, le=1000)) -> Dict[str, Any]:
    """List runs: live in-memory runs plus durable history from prior processes."""
    return {"runs": _run_store.list_runs(limit), "count": len(_run_store.list_runs(limit))}


@router.get("/recordings/index")
async def list_recordings_index_endpoint() -> Dict[str, Any]:
    """List the durable recordings index (maintained on record start/stop)."""
    persistence = getattr(_run_store, "_persistence", None)
    if persistence is None:
        return {"recordings": [], "note": "recordings index not enabled"}
    try:
        recordings = persistence.list_recordings_index()
    except Exception as exc:
        raise HTTPException(status_code=500, detail=f"recordings index unavailable: {exc}")
    return {"recordings": recordings, "count": len(recordings)}


@router.get("/runs/{run_id}")
async def get_run(run_id: str) -> Dict[str, Any]:
    """Return current state of a run."""
    state = _run_store.get(run_id)
    if state is None:
        raise HTTPException(status_code=404, detail=f"Run {run_id} not found")
    return state.to_dict()


@router.get("/runs/{run_id}/cost")
async def get_run_cost(run_id: str) -> Dict[str, Any]:
    """Per-run token/cost accounting (observability; zeros when unavailable)."""
    state = _run_store.get(run_id)
    if state is None:
        raise HTTPException(status_code=404, detail=f"Run {run_id} not found")
    return {"run_id": run_id, "cost": state.cost}


@router.get("/cost/summary")
async def cost_summary_endpoint() -> Dict[str, Any]:
    """Aggregate cost observability across runs (total, completed, success
    rate, avg cost per completed task). Falls back to live in-memory runs
    when no durable backend is attached."""
    persistence = getattr(_run_store, "_persistence", None)
    if persistence is not None:
        try:
            return {"summary": persistence.cost_summary()}
        except Exception as exc:
            raise HTTPException(status_code=500, detail=f"cost summary unavailable: {exc}")

    runs = _run_store.list_runs(2000)
    total = len(runs)
    completed = sum(1 for r in runs if r.get("status") == "completed")
    terminal = sum(
        1 for r in runs
        if r.get("status") in ("completed", "failed", "cancelled", "abandoned",
                               "deviated", "interrupted")
    )
    costs = [
        float((r.get("cost") or {}).get("est_cost_usd") or 0.0)
        for r in runs if r.get("status") == "completed"
    ]
    return {
        "summary": {
            "total_runs": total,
            "completed": completed,
            "terminal_runs": terminal,
            "success_rate": round(completed / terminal, 4) if terminal else 0.0,
            "total_est_cost_usd": round(
                sum(float((r.get("cost") or {}).get("est_cost_usd") or 0.0) for r in runs), 8
            ),
            "avg_cost_per_task_usd": round(sum(costs) / completed, 8) if completed else 0.0,
            "avg_tokens_per_run": 0.0,
        }
    }


@router.get("/runs/{run_id}/events")
async def stream_run_events(run_id: str) -> StreamingResponse:
    """SSE stream of events for an existing run."""
    state = _run_store.get(run_id)
    if state is None:
        raise HTTPException(status_code=404, detail=f"Run {run_id} not found")
    if run_id not in _run_store.event_queues:
        raise HTTPException(
            status_code=404,
            detail=f"Run {run_id} finished in a previous process; events no longer available",
        )

    async def event_generator():
        q = _run_store.event_queues[run_id]
        while True:
            event = await q.get()
            if event is None:
                yield _sse_line("run.ended", run_id, state.status, state.to_dict())
                break
            yield _sse_line(
                event.get("event_type", "unknown"),
                run_id,
                event.get("message", ""),
                event.get("data", {}),
            )

    return StreamingResponse(event_generator(), media_type="text/event-stream")


@router.post("/runs/{run_id}/approve")
async def approve_run(run_id: str, body: ApproveBody) -> Dict[str, Any]:
    """Resolve a pending approval gate for a run."""
    state = _run_store.get(run_id)
    if state is None:
        raise HTTPException(status_code=404, detail=f"Run {run_id} not found")

    if body.decision == "cancel":
        state.cancel_event.set()
        return {"run_id": run_id, "action": "cancelled"}

    future = state.approval_future
    if future is None:
        raise HTTPException(status_code=409, detail="No pending approval for this run")

    if future.done():
        raise HTTPException(status_code=409, detail="Approval already resolved")

    future.set_result({"decision": body.decision, "comment": body.comment})
    return {"run_id": run_id, "decision": body.decision, "acknowledged": True}


@router.post("/runs/{run_id}/steer")
async def steer_run(run_id: str, body: SteerBody) -> Dict[str, Any]:
    """Guidance from the user for a running planning loop, read before its next step."""
    state = _run_store.get(run_id)
    if state is None:
        raise HTTPException(status_code=404, detail=f"Run {run_id} not found")
    loop = getattr(state, "planning_loop", None)
    if loop is None or state.status not in ("running", "awaiting_approval"):
        raise HTTPException(status_code=409, detail="Run is not taking guidance")
    if not loop.steer(body.text):
        raise HTTPException(status_code=409, detail="Nothing to send, or the run has ended")
    await _run_store.push_event(run_id, {
        "event_type": "run.steer.received",
        "run_id": run_id,
        "message": "Guidance received",
        "data": {"text": body.text.strip()[:2000]},
    })
    return {"run_id": run_id, "accepted": True}


@router.post("/runs/{run_id}/cancel")
async def cancel_run(run_id: str) -> Dict[str, Any]:
    """Cancel a running loop."""
    state = _run_store.get(run_id)
    if state is None:
        raise HTTPException(status_code=404, detail=f"Run {run_id} not found")

    state.cancel_event.set()
    _run_store.update_status(run_id, "cancelled")
    return {"run_id": run_id, "status": "cancelled"}


@router.get("/sessions")
async def list_sessions() -> Dict[str, Any]:
    """List all active browser sessions."""
    if not _session_manager_available or session_manager is None:
        return {"sessions": [], "error": "session_manager not available"}
    try:
        stats = await session_manager.get_session_stats()
        sessions_raw = getattr(session_manager, "_sessions", {})
        session_list = [
            {
                "session_id": sid,
                "action_count": s.action_count,
                "idle_seconds": round(s.idle_time, 1),
                "age_seconds": round(s.age, 1),
            }
            for sid, s in sessions_raw.items()
        ]
        return {"sessions": session_list, "stats": stats}
    except Exception as exc:
        raise HTTPException(status_code=500, detail=str(exc))


@router.post("/sessions")
async def create_session() -> Dict[str, Any]:
    """Create a new browser session and return its session_id."""
    if not _session_manager_available or session_manager is None:
        raise HTTPException(status_code=503, detail="session_manager not available")
    session_id = f"sess-{uuid.uuid4().hex[:8]}"
    try:
        await session_manager.get_or_create_session(session_id)
        return {"session_id": session_id, "status": "created"}
    except Exception as exc:
        raise HTTPException(status_code=500, detail=str(exc))


@router.delete("/sessions/{session_id}")
async def delete_session(session_id: str) -> Dict[str, Any]:
    """Close and remove a browser session."""
    if not _session_manager_available or session_manager is None:
        raise HTTPException(status_code=503, detail="session_manager not available")
    try:
        closed = await session_manager.close_session(session_id)
        if not closed:
            raise HTTPException(status_code=404, detail=f"Session {session_id} not found")
        return {"session_id": session_id, "status": "closed"}
    except HTTPException:
        raise
    except Exception as exc:
        raise HTTPException(status_code=500, detail=str(exc))


@router.get("/adapters")
async def list_adapters() -> Dict[str, Any]:
    """Return registered adapters and their capability summary from the executor's live registry."""
    adapters = []

    if _get_executor is not None:
        try:
            executor = _get_executor()
            for adapter_id in executor.registered_adapters():
                instance = executor._adapters[adapter_id]
                entry: Dict[str, Any] = {"adapter_id": adapter_id, "available": True}
                try:
                    caps_raw = await instance.capabilities()
                    caps = vars(caps_raw) if hasattr(caps_raw, "__dataclass_fields__") else (caps_raw if isinstance(caps_raw, dict) else {})
                    entry["capabilities"] = {
                        "dom_tree": caps.get("dom_tree"),
                        "vision_required": caps.get("vision_required"),
                        "multi_tab": caps.get("multi_tab"),
                        "auth_flows": caps.get("auth_flows"),
                        "platform": caps.get("platform"),
                        "family": caps.get("family"),
                    }
                    entry["healthy"] = await instance.health_check()
                except Exception as exc:
                    entry["capabilities"] = None
                    entry["error"] = str(exc)
                adapters.append(entry)
        except Exception:
            pass

    return {"adapters": adapters, "count": len(adapters)}


# In-router recording registry (recording_id -> ActionRecorder)
_router_recordings: Dict[str, Any] = {}


@router.post("/record")
async def record(body: RecordBody) -> Dict[str, Any]:
    """
    Start, append frames to, or stop an action recording for a session.

    action="start":  creates a new ActionRecorder and returns a recording_id.
    action="append": appends recorded frames to an in-flight recording (lets
                     callers build a recording step-by-step over HTTP).
    action="stop":   finalizes the recording and returns frame count + paths.
    """
    if not _recorder_available or ActionRecorder is None:
        raise HTTPException(status_code=503, detail="ActionRecorder not available")

    if body.action == "start":
        recording_id = f"rec-{uuid.uuid4().hex[:12]}"
        recorder = ActionRecorder(
            recording_id=recording_id,
            task=body.name or f"session-{body.session_id}",
            session_id=body.session_id,
            run_id=str(uuid.uuid4()),
            record_gif=body.record_gif,
        )
        if body.video_path:
            recorder.manifest.video_path = body.video_path
        if body.video_start_epoch is not None:
            recorder.manifest.video_start_epoch = body.video_start_epoch
        await recorder.start()
        _router_recordings[recording_id] = recorder
        _index_recording(
            recording_id=recording_id,
            task=body.name or f"session-{body.session_id}",
            session_id=body.session_id,
            run_id=str(uuid.uuid4()),
            status="recording",
            started_at=recorder.manifest.started_at,
            path=str(recorder.get_path()),
        )
        return {
            "recording_id": recording_id,
            "path": str(recorder.get_path()),
            "status": "recording",
        }

    if not body.recording_id:
        raise HTTPException(status_code=400, detail="recording_id required")

    if body.action == "append":
        if not body.frames:
            raise HTTPException(status_code=400, detail="frames required to append")
        recorder = _router_recordings.get(body.recording_id)
        if recorder is None:
            raise HTTPException(
                status_code=404,
                detail=f"Recording {body.recording_id} not found (already stopped or unknown)",
            )
        from core.action_recorder import RecordedFrame

        appended = 0
        for frame_data in body.frames:
            frame = RecordedFrame(
                recording_id=body.recording_id,
                step=frame_data.get("step", appended),
                action_type=frame_data.get("action_type", ""),
                action_target=frame_data.get("action_target", ""),
                action_params=frame_data.get("action_params", {}) or {},
                before_screenshot_b64=frame_data.get("before_screenshot_b64", ""),
                after_screenshot_b64=frame_data.get("after_screenshot_b64", ""),
                reasoning=frame_data.get("reasoning", ""),
                reflection=frame_data.get("reflection", ""),
                action_succeeded=frame_data.get("action_succeeded", True),
                risk_level=frame_data.get("risk_level", "low"),
                tokens_used=frame_data.get("tokens_used", 0),
            )
            await recorder.record_frame(frame)
            recorder.feed_gif_frame(frame)
            appended += 1
        return {
            "recording_id": body.recording_id,
            "appended": appended,
            "frames": getattr(recorder, "_frame_count", 0),
            "status": "recording",
        }

    # action == "stop"
    recorder = _router_recordings.pop(body.recording_id, None)
    if recorder is None:
        raise HTTPException(status_code=404, detail=f"Recording {body.recording_id} not found")
    if body.video_path:
        recorder.manifest.video_path = body.video_path
    if body.video_start_epoch is not None:
        recorder.manifest.video_start_epoch = body.video_start_epoch
    await recorder.stop()
    gif_path = str(recorder.get_gif_path()) if recorder.get_gif_path() else None
    _index_recording(
        recording_id=body.recording_id,
        task=recorder.manifest.task,
        session_id=recorder.manifest.session_id,
        run_id=recorder.manifest.run_id,
        status=recorder.manifest.status,
        started_at=recorder.manifest.started_at,
        completed_at=recorder.manifest.completed_at,
        total_steps=recorder.manifest.total_steps,
        path=str(recorder.get_path()),
        gif_path=gif_path,
    )
    return {
        "recording_id": body.recording_id,
        "frames": getattr(recorder, "_frame_count", 0),
        "path": str(recorder.get_path()),
        "gif_path": gif_path,
        "status": "stopped",
    }


@router.get("/recordings")
async def list_recordings_endpoint() -> Dict[str, Any]:
    """List recordings on disk (newest first), including completed ones."""
    if not _recorder_available or list_recordings is None:
        raise HTTPException(status_code=503, detail="ActionRecorder not available")
    return {"recordings": list_recordings()}


# ---------------------------------------------------------------------------
# Recording detail / file / GIF routes
#
# Contract: surfaces/ai.allternit.com/src/remote-control/api/recordings.ts
# (RecordingManifest / RecordedStep / RecordingDetail).
# ---------------------------------------------------------------------------

# recording_id becomes a filesystem lookup — allowlist plus resolved-path
# containment below are both required before touching disk.
_RECORDING_ID_RE = re.compile(r"^[A-Za-z0-9._-]+$")


def _recordings_root() -> Path:
    from core.action_recorder import DEFAULT_RECORDINGS_DIR

    return DEFAULT_RECORDINGS_DIR


def _resolve_recording_path(recording_id: str) -> Path:
    """Resolve a recording_id to its on-disk JSONL path, or raise 404.

    Defense in depth: regex allowlist on the id, then resolve the candidate
    path and verify it stays inside the recordings root. Never follows ids
    containing '/', '..', or absolute paths.
    """
    if not _RECORDING_ID_RE.match(recording_id):
        raise HTTPException(
            status_code=404,
            detail=f"Recording {recording_id} not found",
        )
    from core.action_recorder import find_recording_path

    root = _recordings_root().resolve()
    try:
        path = find_recording_path(recording_id).resolve()
    except FileNotFoundError:
        raise HTTPException(
            status_code=404,
            detail=f"Recording {recording_id} not found",
        )
    if not path.is_relative_to(root):
        raise HTTPException(
            status_code=404,
            detail=f"Recording {recording_id} not found",
        )
    return path


def _resolve_recording_gif_path(recording_id: str, manifest: Any) -> Optional[Path]:
    """Locate a recording's GIF on disk, constrained to the recordings root.

    Candidates, in order: the manifest's gif_path, then a GIF named after the
    recording id, then the ActionRecorder convention (session-<run_id>.gif)
    — all alongside the JSONL under the recordings root.
    """
    try:
        root = _recordings_root().resolve()
    except Exception:
        return None
    candidates: List[Path] = []
    gif_path = getattr(manifest, "gif_path", None)
    if gif_path:
        candidates.append(Path(gif_path))
    run_id = getattr(manifest, "run_id", "") or ""
    candidates.append(root / f"{recording_id}.gif")
    if run_id:
        candidates.append(root / f"session-{run_id}.gif")
    for candidate in candidates:
        try:
            resolved = candidate.resolve()
        except Exception:
            continue
        if resolved.is_file() and resolved.is_relative_to(root):
            return resolved
    return None


def _resolve_recording_video_path(recording_id: str, manifest: Any) -> Optional[Path]:
    """Locate a recording's video on disk, constrained to the recordings root.

    Candidates, in order: the manifest's video_path, then a video named after
    the recording id, then the chrome-stream artifact convention
    (<recording_id>.webm / session-<run_id>.webm) — all under the recordings
    root, mirroring the GIF lookup.
    """
    try:
        root = _recordings_root().resolve()
    except Exception:
        return None
    candidates: List[Path] = []
    video_path = getattr(manifest, "video_path", None)
    if video_path:
        candidates.append(Path(video_path))
    candidates.append(root / f"{recording_id}.webm")
    run_id = getattr(manifest, "run_id", "") or ""
    if run_id:
        candidates.append(root / f"session-{run_id}.webm")
    for candidate in candidates:
        try:
            resolved = candidate.resolve()
        except Exception:
            continue
        if resolved.is_file() and resolved.is_relative_to(root):
            return resolved
    return None


def _load_recording_or_raise(recording_id: str) -> tuple[Any, List[Any], Path]:
    """Load manifest + frames + path, mapping filesystem errors to HTTP errors."""
    path = _resolve_recording_path(recording_id)
    try:
        manifest, frames = ActionRecorder.load(path)
    except FileNotFoundError:
        raise HTTPException(
            status_code=404,
            detail=f"Recording {recording_id} not found",
        )
    except (ValueError, json.JSONDecodeError) as exc:
        raise HTTPException(
            status_code=400,
            detail=f"Invalid recording {recording_id}: {exc}",
        )
    return manifest, frames, path


def _manifest_to_dict(manifest: Any) -> Dict[str, Any]:
    """Map the on-disk manifest to the RecordingManifest TS contract."""
    return {
        "recording_id": manifest.recording_id,
        "task": manifest.task,
        "session_id": manifest.session_id,
        "run_id": manifest.run_id,
        "started_at": manifest.started_at,
        "completed_at": manifest.completed_at,
        "total_steps": manifest.total_steps,
        "status": manifest.status,
        "gif_path": manifest.gif_path,
        "video_path": getattr(manifest, "video_path", None),
        "video_start_epoch": getattr(manifest, "video_start_epoch", None),
    }


def _frame_to_step(frame: Any, index: int) -> Dict[str, Any]:
    """Map a recorder frame to the RecordedStep TS contract (no screenshots).

    Handles both classic action frames and ToolCallFrame (``_type ==
    "tool_call"``) — the latter surface as a step with ``kind == "tool_call"``.
    """
    if getattr(frame, "frame_type", None) == "tool_call" or hasattr(frame, "tool_name"):
        return {
            "step": getattr(frame, "step", None) or index + 1,
            "timestamp": frame.timestamp,
            "kind": "tool_call",
            "tool_name": frame.tool_name,
            "args": getattr(frame, "args", None) or {},
            "result_summary": getattr(frame, "result_summary", None),
            "latency_ms": getattr(frame, "latency_ms", None),
            "error": getattr(frame, "error", None),
            "action_succeeded": not getattr(frame, "error", None),
            "risk_level": "low",
        }
    return {
        "step": getattr(frame, "step", None) or index + 1,
        "timestamp": frame.timestamp,
        "kind": "action",
        "action_type": frame.action_type,
        "action_target": frame.action_target,
        "action_params": frame.action_params or {},
        "reasoning": frame.reasoning,
        "action_succeeded": frame.action_succeeded is not False,
        "risk_level": frame.risk_level or "low",
    }


@router.get("/recordings/{recording_id}")
async def get_recording_detail(recording_id: str) -> Dict[str, Any]:
    """Recording detail: manifest + parsed steps + gif_url (contract: recordings.ts)."""
    if not _recorder_available or ActionRecorder is None:
        raise HTTPException(status_code=503, detail="ActionRecorder not available")
    manifest, frames, _path = _load_recording_or_raise(recording_id)
    gif = _resolve_recording_gif_path(recording_id, manifest)
    video = _resolve_recording_video_path(recording_id, manifest)
    return {
        "manifest": _manifest_to_dict(manifest),
        "steps": [_frame_to_step(frame, i) for i, frame in enumerate(frames)],
        "gif_url": (
            f"/v1/computer-use/recordings/{recording_id}/gif" if gif is not None else None
        ),
        "video_url": (
            f"/v1/computer-use/recordings/{recording_id}/video" if video is not None else None
        ),
    }


@router.get("/recordings/{recording_id}/file")
async def get_recording_file(recording_id: str) -> Response:
    """Raw recording JSONL bytes (manifest line first, then frame lines)."""
    if not _recorder_available or ActionRecorder is None:
        raise HTTPException(status_code=503, detail="ActionRecorder not available")
    path = _resolve_recording_path(recording_id)
    try:
        content = path.read_bytes()
    except FileNotFoundError:
        raise HTTPException(
            status_code=404,
            detail=f"Recording {recording_id} not found",
        )
    return Response(content=content, media_type="application/x-ndjson")


@router.get("/recordings/{recording_id}/gif")
async def get_recording_gif(recording_id: str) -> Response:
    """GIF replay bytes for a recording, when one exists on disk."""
    if not _recorder_available or ActionRecorder is None:
        raise HTTPException(status_code=503, detail="ActionRecorder not available")
    manifest, _frames, _path = _load_recording_or_raise(recording_id)
    gif = _resolve_recording_gif_path(recording_id, manifest)
    if gif is None:
        raise HTTPException(
            status_code=404,
            detail=f"No GIF for recording {recording_id}",
        )
    return Response(content=gif.read_bytes(), media_type="image/gif")


@router.get("/recordings/{recording_id}/video")
async def get_recording_video(recording_id: str) -> Response:
    """WebM video bytes for a recording, when one exists on disk."""
    if not _recorder_available or ActionRecorder is None:
        raise HTTPException(status_code=503, detail="ActionRecorder not available")
    manifest, _frames, _path = _load_recording_or_raise(recording_id)
    video = _resolve_recording_video_path(recording_id, manifest)
    if video is None:
        raise HTTPException(
            status_code=404,
            detail=f"No video for recording {recording_id}",
        )
    return Response(content=video.read_bytes(), media_type="video/webm")


@router.post("/replay")
async def replay(body: ReplayBody) -> Dict[str, Any]:
    """
    Replay a completed recording from disk, or export its GIF.

    The recording is resolved from the in-flight registry first, then from
    disk under ~/.allternit/recordings/ — so completed recordings replay
    after their recorder has been stopped and popped.

    The recording is re-executed step-by-step through the adapter layer as a
    replay run in the run store. After each action the live screen is compared
    with the recorded after-screenshot; a deviation score above
    deviation_threshold pauses the run, which can then be resumed or abandoned
    via POST /runs/{run_id}/approve (decision="approve" resumes,
    decision="deny" abandons).

    With export_gif=true (and no execution wanted), returns the recording's
    GIF path when one exists on disk.

    With wait=true, blocks until the replay finishes and returns the full
    result; otherwise returns immediately with the run_id (poll
    GET /runs/{run_id} or stream GET /runs/{run_id}/events).
    """
    if not _recorder_available or ActionRecorder is None:
        raise HTTPException(status_code=503, detail="ActionRecorder not available")

    # GIF export for a recording that already has one on disk.
    if body.export_gif:
        gif_path = _lookup_gif_on_disk(body.recording_id)
        if gif_path is None:
            recorder = _router_recordings.get(body.recording_id)
            if recorder is not None and recorder.get_gif_path():
                gif_path = str(recorder.get_gif_path())
        if gif_path is None:
            raise HTTPException(status_code=404, detail=f"No GIF for recording {body.recording_id}")
        return {"recording_id": body.recording_id, "gif_path": gif_path, "status": "exported"}

    # Resolve the recording from the in-flight registry, then from disk.
    from core.action_recorder import load_recording

    try:
        manifest, frames, recording_path = load_recording(body.recording_id)
    except FileNotFoundError:
        raise HTTPException(
            status_code=404,
            detail=f"Recording {body.recording_id} not found (unknown or not on disk)",
        )
    except ValueError as exc:
        raise HTTPException(status_code=400, detail=f"Invalid recording: {exc}")

    if not frames:
        raise HTTPException(status_code=400, detail=f"Recording {body.recording_id} has no frames to replay")

    session_id = body.session_id or manifest.session_id or f"sess-{uuid.uuid4().hex[:8]}"
    run_id = f"replay-{uuid.uuid4().hex[:12]}"
    run_state = _run_store.create(
        run_id=run_id,
        session_id=session_id,
        mode="replay",
        target_scope="auto",
    )
    _run_store.update_status(run_id, "running")
    _emit_canonical("replay.started", session_id=session_id, run_id=run_id,
                    payload={"recording_id": body.recording_id, "total_steps": len(frames)})

    adapter = _get_adapter_for_planning("auto", None)

    # Deviation pause → approval_future, same pattern as the planning loop.
    async def replay_approval_callback(deviation: Any) -> bool:
        loop = asyncio.get_event_loop()
        future: asyncio.Future = loop.create_future()
        run_state.approval_future = future
        _run_store.update_status(run_id, "awaiting_approval")
        deviation_dict = deviation.to_dict() if hasattr(deviation, "to_dict") else dict(deviation or {})
        await _push_approval_event(run_state, "approval.required", {
            "kind": "replay.deviation",
            "deviation": deviation_dict,
            "timeout_seconds": _DIRECT_APPROVAL_TIMEOUT_SECONDS,
        })
        _emit_canonical("approval.required", session_id=session_id, run_id=run_id,
                        payload={"kind": "replay.deviation", "step": deviation_dict.get("step"),
                                 "score": deviation_dict.get("score")})
        approved = False
        timed_out = False
        try:
            decision_payload = await asyncio.wait_for(future, timeout=_DIRECT_APPROVAL_TIMEOUT_SECONDS)
            approved = decision_payload.get("decision") == "approve"
        except asyncio.TimeoutError:
            run_state.approval_timed_out = True
            timed_out = True
            logger.warning("Replay deviation approval timed out for run %s", run_id)
        finally:
            run_state.approval_future = None
            _run_store.update_status(run_id, "running")
            await _push_approval_event(run_state, "approval.resolved", {
                "kind": "replay.deviation",
                "deviation": deviation_dict,
                "approved": approved,
                "timed_out": timed_out,
            })
            _emit_canonical("approval.resolved", session_id=session_id, run_id=run_id,
                            payload={"kind": "replay.deviation", "approved": approved,
                                     "timed_out": timed_out})
        return approved

    async def on_event(event: Dict[str, Any]) -> None:
        await _run_store.push_event(run_id, {
            "event_type": event.get("type", "replay.event"),
            "run_id": run_id,
            "message": event.get("type", ""),
            "data": event,
        })

    async def run_replay() -> None:
        from core.replay_engine import ReplayEngine

        engine = ReplayEngine(
            adapter=adapter,
            session_id=session_id,
            deviation_threshold=body.deviation_threshold,
            approval_callback=replay_approval_callback,
            on_event=on_event,
            cancel_event=run_state.cancel_event,
        )
        try:
            result = await engine.replay(recording_path)
            run_state.status = result.status
            run_state.result = result.to_dict()
            # Cost accounting: replay makes no LLM calls — honest zero.
            run_state.cost = zero_run_cost()
            _emit_canonical("replay.finished", session_id=session_id, run_id=run_id,
                            payload={"recording_id": manifest.recording_id,
                                     "status": result.status,
                                     "replayed_steps": len(result.steps),
                                     "deviations": len(result.deviations)})
        except Exception as exc:
            logger.exception("Replay raised exception: %s", exc)
            run_state.status = "failed"
            run_state.error = str(exc)
            _emit_canonical("run.failed", session_id=session_id, run_id=run_id,
                            payload={"mode": "replay", "error": str(exc)})
        finally:
            run_state.updated_at = _utcnow()
            _run_store.finalize(run_id)
            await _run_store.push_event(run_id, {
                "event_type": "run.ended",
                "run_id": run_id,
                "message": run_state.status,
                "data": run_state.to_dict(),
            })
            await _run_store.push_sentinel(run_id)

    task = asyncio.create_task(run_replay())

    if body.wait:
        await task
        return {
            "run_id": run_id,
            "recording_id": body.recording_id,
            "status": run_state.status,
            "path": str(recording_path),
            "result": run_state.result,
            "error": run_state.error,
        }

    return {
        "run_id": run_id,
        "recording_id": body.recording_id,
        "session_id": session_id,
        "status": "running",
        "path": str(recording_path),
        "total_steps": len(frames),
    }


def _lookup_gif_on_disk(recording_id: str) -> Optional[str]:
    """Return the GIF path from a recording's on-disk manifest, if any."""
    try:
        from core.action_recorder import load_recording

        manifest, _frames, _path = load_recording(recording_id)
        return manifest.gif_path
    except Exception:
        return None


@router.get("/health")
async def health() -> Dict[str, Any]:
    """Health check for the computer-use subsystem."""
    if _session_manager_available and session_manager is not None:
        try:
            stats = await session_manager.get_session_stats()
            session_count = stats.get("active_sessions", 0)
        except Exception:
            session_count = -1
    else:
        session_count = 0

    adapter_count = 0
    if _get_executor is not None:
        try:
            adapter_count = len(_get_executor().registered_adapters())
        except Exception:
            pass

    return {
        "status": "ok",
        "adapters": adapter_count,
        "sessions": session_count,
        "version": "0.1.0",
        "planning_available": _planning_available,
    }
