"""
Code-mode planning-loop integration tests (spec code-mode-execution, C2).

Covers:
  * core/code_mode: AciCodeClient response normalization (executed /
    confirmation_required / code_refused / hard failure) and the code-context
    ledger record (contract §8).
  * vision_providers: a plan's `code` payload parses from model JSON.
  * planning_loop: code mode is STRICTLY opt-in (default config never
    dispatches code); when enabled, a plan carrying a code payload dispatches
    ONE grant-bound run; the fixed envelope returns to the LoopStep; a grant
    denial routes through the human approval flow; a validation refusal
    surfaces as the step's observation and the loop re-plans with whitelist
    actions (never a silent retry); a declined grant falls back to whitelist.
"""

import json
import sys
from pathlib import Path

import pytest

DOMAIN_CORE_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(DOMAIN_CORE_ROOT / "gateway"))
sys.path.insert(0, str(DOMAIN_CORE_ROOT))

_existing_core = sys.modules.get("core")
if _existing_core is not None:
    _existing_file = getattr(_existing_core, "__file__", "") or ""
    if Path(_existing_file).parent != DOMAIN_CORE_ROOT / "core":
        for _name in [
            m for m in list(sys.modules)
            if (m == "core" or m.startswith("core.")) and not m.startswith("core.tests")
        ]:
            del sys.modules[_name]

from core.code_mode import (  # noqa: E402
    AciCodeClient,
    CodeContextRecord,
    CodeDispatchResult,
    close_code_context,
    open_code_context,
)
from core.planning_loop import PlanningLoop, PlanningLoopConfig  # noqa: E402
from core.vision_providers import ActionPlan, VisionAction, _parse_action_plan  # noqa: E402

pytestmark = pytest.mark.asyncio

CODE = "console.log('ran');"


def _code_plan(code=CODE, targets=None):
    return ActionPlan(
        reasoning="one code step",
        plan_steps=["run code"],
        confidence=0.9,
        immediate_action=VisionAction(type="click", target="#fallback", reason="fallback"),
        code={"language": "playwright-js", "code": code,
              "declaredTargets": targets or ["http://127.0.0.1:8080"]},
    )


class _ScriptedProvider:
    def __init__(self, plans):
        self._plans = list(plans)
        self.turns = 0

    async def ground_and_reason(self, screenshot_b64, task, history=None, **kwargs):
        self.turns += 1
        if not self._plans:
            return ActionPlan(
                reasoning="done", plan_steps=[], confidence=1.0, done=True,
                immediate_action=VisionAction(type="screenshot", target="screen", reason="done"),
            )
        return self._plans.pop(0)

    async def analyze_screenshot(self, screenshot_b64, task, **kwargs):
        class _R:
            raw_response = "ok"
            action = None
            confidence = 1.0
        return _R()


class _FakeAdapter:
    def __init__(self):
        self.calls = []

    async def screenshot(self, session_id):
        return b""

    async def execute(self, req):
        self.calls.append(req.action_type)

        class _Result:
            def to_dict(self):
                return {"status": "ok", "action": req.action_type}
        return _Result()


class _FakeCodeClient:
    def __init__(self, results):
        self._results = list(results)
        self.calls = []

    async def execute_code(self, *, code, language="playwright-js",
                           declared_targets=None, origin="aci.code",
                           session=None, approval_id=None):
        self.calls.append({
            "code": code, "language": language,
            "declared_targets": declared_targets, "session": session,
            "approval_id": approval_id,
        })
        result = self._results.pop(0)
        if callable(result):
            result = result(self.calls[-1])
        return result


def _envelope(stdout="ran", exit_status=0, **overrides):
    env = {
        "stdout": stdout,
        "exit_status": exit_status,
        "timed_out": False,
        "refused": None,
        "error": None,
        "screenshot_sha256": None,
        "screenshot_ref": None,
    }
    env.update(overrides)
    return env


def _executed(envelope=None, receipt=None, descriptor_hash="dh-1", receipt_id="rcpt-1"):
    return CodeDispatchResult(
        executed=True,
        descriptor_hash=descriptor_hash,
        receipt=receipt or {"status": "completed", "descriptor_hash": descriptor_hash},
        receipt_id=receipt_id,
        envelope=envelope or _envelope(),
        status_code=200,
    )


def _make_loop(provider, adapter, client, ledger_events, **config_kwargs):
    config_kwargs.setdefault("approval_policy", "never")
    return PlanningLoop(
        provider,
        adapter,
        PlanningLoopConfig(**config_kwargs),
        ledger=lambda et, payload: ledger_events.append((et, payload)),
        code_client=client,
    )


# ── Plan parsing ─────────────────────────────────────────────────────────────

def test_code_payload_parses_from_model_json():
    text = json.dumps({
        "reasoning": "r", "plan_steps": [], "confidence": 0.8, "done": False,
        "immediate_action": {"type": "click", "target": "#a"},
        "code": {"language": "playwright-js", "code": CODE,
                 "declaredTargets": ["http://x.example"]},
    })
    plan = _parse_action_plan(text)
    assert plan.code["language"] == "playwright-js"
    assert plan.code["code"] == CODE
    assert plan.code["declaredTargets"] == ["http://x.example"]


def test_code_payload_absent_by_default():
    plan = _parse_action_plan(json.dumps({
        "reasoning": "r", "plan_steps": [], "done": False,
        "immediate_action": {"type": "click", "target": "#a"},
    }))
    assert plan.code is None


# ── Code-context record (contract §8) ───────────────────────────────────────

def test_code_context_record_open_close_payloads():
    events = []
    ledger = lambda et, payload: events.append((et, payload))
    record = CodeContextRecord(
        run_id="r-1", session_id="s-1", language="playwright-js",
        code_bytes=42, declared_targets=["http://x.example"],
    )
    open_code_context(ledger, record)
    record.code_id = record.descriptor_hash = "dh-1"
    close_code_context(ledger, record, status="completed", receipt_id="rcpt-1",
                       exit_status=0, timed_out=False)
    assert [et for et, _ in events] == ["code.context.opened", "code.context.closed"]
    opened = events[0][1]
    assert opened["contract_version"] == "1"
    assert opened["code_bytes"] == 42
    assert opened["declared_targets"] == ["http://x.example"]
    assert "code" not in opened  # payload content never enters the ledger
    closed = events[1][1]
    assert closed["status"] == "completed"
    assert closed["receipt_id"] == "rcpt-1"
    assert closed["exit_status"] == 0


# ── Client normalization ─────────────────────────────────────────────────────

async def test_client_shapes(monkeypatch):
    # Transport-level behavior of AciCodeClient against canned responses.
    import httpx

    class _Resp:
        def __init__(self, status, data):
            self.status_code = status
            self._data = data

        def json(self):
            return self._data

    async def _fake_post(self, url, json=None, headers=None):
        return _Resp(_fake_post.status, _fake_post.data)

    monkeypatch.setattr(httpx.AsyncClient, "post", _fake_post)

    client = AciCodeClient(base_url="http://testserver")

    _fake_post.status, _fake_post.data = 403, {
        "error": "confirmation_required", "approval_id": "ap-9", "action_hash": "dh-9",
    }
    r = await client.execute_code(code=CODE)
    assert r.confirmation_required and r.approval_id == "ap-9" and r.action_hash == "dh-9"

    _fake_post.status, _fake_post.data = 400, {
        "error": "code_refused", "class": "credential_pattern", "message": "m",
    }
    r = await client.execute_code(code=CODE)
    assert r.refused and r.refusal_class == "credential_pattern"

    _fake_post.status, _fake_post.data = 200, {
        "descriptor_hash": "dh-1", "receipt_id": "rcpt-1",
        "receipt": {"status": "completed"},
        "result": _envelope(),
    }
    r = await client.execute_code(code=CODE, approval_id="ap-9")
    assert r.executed and r.envelope["stdout"] == "ran"
    assert r.receipt["status"] == "completed"


# ── Loop integration ─────────────────────────────────────────────────────────

async def test_code_mode_is_never_the_default():
    # plan carries code, but code_mode_enabled is False (the default): the
    # loop must execute the WHITELIST action and never touch the code client.
    client = _FakeCodeClient([_executed()])
    adapter = _FakeAdapter()
    ledger = []
    loop = _make_loop(_ScriptedProvider([_code_plan()]), adapter, client, ledger)
    result = await loop.run(task="t", session_id="s-1", run_id="r-1")
    assert client.calls == []
    assert adapter.calls == ["left_click"]
    assert all(not et.startswith("code.context") for et, _ in ledger)
    assert result.status == "completed"


async def test_opted_in_code_plan_dispatches_one_grant_bound_run():
    client = _FakeCodeClient([_executed()])
    adapter = _FakeAdapter()
    ledger = []
    loop = _make_loop(_ScriptedProvider([_code_plan()]), adapter, client, ledger,
                      code_mode_enabled=True)
    result = await loop.run(task="t", session_id="s-1", run_id="r-1")
    assert result.status == "completed"
    # Exactly ONE code dispatch; the whitelist fallback was never executed.
    assert len(client.calls) == 1
    assert client.calls[0]["approval_id"] is None  # first attempt presents bare
    assert adapter.calls == []
    step = result.steps[0]
    assert step.action_type == "code"
    assert step.action_succeeded is True
    assert step.adapter_result["code_envelope"]["stdout"] == "ran"
    assert step.action_params["descriptor_hash"] == "dh-1"
    # Contract §8: opened before closed, both present.
    kinds = [et for et, _ in ledger]
    assert kinds.index("code.context.opened") < kinds.index("code.context.closed")
    opened_payload = [p for et, p in ledger if et == "code.context.opened"][0]
    closed_payload = [p for et, p in ledger if et == "code.context.closed"][0]
    # Opened is audit-before-act: hash unknown until the gate responds.
    assert opened_payload["descriptor_hash"] == "pending"
    assert closed_payload["descriptor_hash"] == "dh-1"
    assert closed_payload["status"] == "completed"
    assert closed_payload["receipt_id"] == "rcpt-1"


async def test_grant_denial_routes_through_human_approval_flow():
    def approve(call):
        # The retry after approval carries the grant id.
        assert call["approval_id"] == "ap-1"
        return _executed()

    client = _FakeCodeClient([
        CodeDispatchResult(executed=False, confirmation_required=True,
                           approval_id="ap-1", action_hash="dh-1"),
        approve,
    ])
    ledger = []
    approvals = []
    loop = _make_loop(_ScriptedProvider([_code_plan()]), _FakeAdapter(), client, ledger,
                      code_mode_enabled=True)
    loop.approval_callback = lambda step: approvals.append(step) or True
    result = await loop.run(task="t", session_id="s-1", run_id="r-1")
    assert result.status == "completed"
    assert len(approvals) == 1  # the human gate fired exactly once
    assert len(client.calls) == 2
    closed = [p for et, p in ledger if et == "code.context.closed"][0]
    assert closed["status"] == "completed"


async def test_declined_grant_falls_back_to_whitelist():
    client = _FakeCodeClient([
        CodeDispatchResult(executed=False, confirmation_required=True,
                           approval_id="ap-1", action_hash="dh-1"),
    ])
    adapter = _FakeAdapter()
    ledger = []
    loop = _make_loop(_ScriptedProvider([_code_plan()]), adapter, client, ledger,
                      code_mode_enabled=True)
    loop.approval_callback = lambda step: False
    result = await loop.run(task="t", session_id="s-1", run_id="r-1")
    # Declined → step-by-step fallback executes the whitelist action.
    assert adapter.calls == ["left_click"]
    closed = [p for et, p in ledger if et == "code.context.closed"][0]
    assert closed["status"] == "denied"


async def test_validation_refusal_surfaces_as_observation_and_replans():
    client = _FakeCodeClient([
        CodeDispatchResult(executed=False, refused=True,
                           refusal_class="credential_pattern",
                           refusal_message="payload contains aws key id"),
    ])
    adapter = _FakeAdapter()
    ledger = []
    provider = _ScriptedProvider([
        _code_plan(code="const k='AKIAIOSFODNN7EXAMPLE';"),
        # After the refusal observation the model re-plans with a whitelist
        # action — the code payload is NOT retried.
        ActionPlan(reasoning="rewrite", plan_steps=[], confidence=0.9,
                   immediate_action=VisionAction(type="click", target="#go", reason="r")),
    ])
    loop = _make_loop(provider, adapter, client, ledger, code_mode_enabled=True)
    result = await loop.run(task="t", session_id="s-1", run_id="r-1")
    assert result.status == "completed"
    # One code attempt only — never a silent retry of the mutated payload.
    assert len(client.calls) == 1
    refusal_step = result.steps[0]
    assert refusal_step.action_type == "code"
    assert refusal_step.action_succeeded is False
    assert "credential_pattern" in refusal_step.error
    assert refusal_step.adapter_result["code_refusal"]["class"] == "credential_pattern"
    # The loop re-planned with the whitelist action.
    assert adapter.calls == ["left_click"]
    closed = [p for et, p in ledger if et == "code.context.closed"][0]
    assert closed["status"] == "refused"


async def test_failed_run_reports_the_envelope_honestly():
    client = _FakeCodeClient([_executed(envelope=_envelope(stdout="boom", exit_status=1,
                                                             error="TypeError: x"))])
    ledger = []
    loop = _make_loop(_ScriptedProvider([_code_plan()]), _FakeAdapter(), client, ledger,
                      code_mode_enabled=True)
    result = await loop.run(task="t", session_id="s-1", run_id="r-1")
    step = result.steps[0]
    assert step.action_type == "code"
    assert step.action_succeeded is False
    assert step.adapter_result["code_envelope"]["exit_status"] == 1
    closed = [p for et, p in ledger if et == "code.context.closed"][0]
    assert closed["status"] == "failed"
    assert closed["exit_status"] == 1
