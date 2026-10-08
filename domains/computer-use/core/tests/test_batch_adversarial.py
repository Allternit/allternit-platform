"""
Adversarial batch-dispatch engine tests (spec stagehand-batch-fork, D2).

Companion to the Rust adversarial suite (cmd/allternit-api/src/
aci_batch_adversarial.rs). The Rust gate owns hash binding, single-use
redemption, expiry, and receipt chaining; these tests assert the ENGINE side
of the same invariants:

  * a grant retry the server rejects (tampered descriptor, replayed grant,
    expired TTL) is never laundered into a success — the loop fails closed
    and falls back to the honest step-by-step path;
  * the engine never mutates the mapped steps between attempts, so a
    retry carries byte-identical steps to the first attempt;
  * a 200 without a receipt is not evidence — fail closed;
  * workflow steps named in the spec's safety.requiresApprovalFor keep the
    whole workflow on the per-step path (never silently batched).

The adversary here is scripted (queued fake-client responses), not a trained
attacking model — same honest scope as the Rust suite.
"""

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

from core.batch_dispatch import (  # noqa: E402
    BatchDispatchResult,
    actions_to_batch_steps,
    workflow_steps_to_batch_steps,
)
from core.planning_loop import PlanningLoop, PlanningLoopConfig, StopReason  # noqa: E402
from core.vision_providers import ActionPlan, VisionAction  # noqa: E402


def _action(type_, target, text=None):
    return VisionAction(type=type_, target=target, reason="t", text=text)


class _ScriptedProvider:
    """Returns queued ActionPlans, counting turns."""

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
    """Non-executor adapter recording execute() calls."""

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


class _FakeBatchClient:
    """Queued results; a result may be a callable receiving the call dict."""

    def __init__(self, results):
        self._results = list(results)
        self.calls = []

    async def execute_batch(self, *, steps, mode, origin="aci.batch", session=None,
                            page_url=None, approval_id=None, step_approval_ids=None,
                            headless=True):
        self.calls.append({
            "steps": steps, "mode": mode, "origin": origin, "session": session,
            "page_url": page_url, "approval_id": approval_id,
            "step_approval_ids": step_approval_ids, "headless": headless,
        })
        result = self._results.pop(0)
        if callable(result):
            result = result(self.calls[-1])
        return result


def _receipt(status, steps, halted_at=None):
    return {"status": status, "halted_at": halted_at, "steps": steps}


def _batch_plan():
    return ActionPlan(
        reasoning="three grounded steps",
        plan_steps=["open form", "fill", "submit"],
        confidence=0.9,
        immediate_action=_action("click", "#open"),
        batch=[_action("type", "#name", text="Eoj"), _action("click", "#submit")],
    )


def _done_plan():
    return ActionPlan(reasoning="finish", plan_steps=[], confidence=0.9, done=True,
                      immediate_action=VisionAction(type="screenshot", target="screen", reason=""))


def _make_loop(provider, adapter, client, ledger_events, **config_kwargs):
    config_kwargs.setdefault("approval_policy", "never")
    return PlanningLoop(
        provider,
        adapter,
        PlanningLoopConfig(**config_kwargs),
        ledger=lambda et, payload: ledger_events.append((et, payload)),
        batch_client=client,
    )


class _Ledger:
    """Rust-suite-style ledger: fails the test if any attack gets through."""

    def __init__(self):
        self.attempted = 0
        self.blocked = 0
        self.got_through = []

    def attack(self, name, blocked):
        self.attempted += 1
        if blocked:
            self.blocked += 1
        else:
            self.got_through.append(name)

    def finish(self):
        print(f"engine adversarial: {self.blocked}/{self.attempted} blocked")
        assert not self.got_through, (
            f"CRITICAL: {len(self.got_through)} engine attack(s) got through: {self.got_through}"
        )


def _closed_statuses(events):
    return [p["status"] for et, p in events if et == "batch.context.closed"]


class TestAdversarialBatchDispatch:
    @pytest.mark.asyncio
    async def test_rejected_grant_retry_fails_closed_and_steps_are_stable(self):
        """Tampered-descriptor retry: the server refuses (the Rust suite proves
        the gate does); the engine must not launder that into a success, must
        not mutate the steps it mapped, and must not keep retrying."""
        events = []
        provider = _ScriptedProvider([_batch_plan(), _done_plan()])
        adapter = _FakeAdapter()
        client = _FakeBatchClient([
            BatchDispatchResult(executed=False, confirmation_required=True,
                                approval_id="ap-t", action_hash="dhash"),
            # Server rejected the retry: descriptor hash no longer matches
            # the granted one (tampered / widened / replayed grant).
            BatchDispatchResult(executed=False, status_code=403,
                                error="approval_denied"),
        ])
        loop = _make_loop(provider, adapter, client, events, batch_mode="batch")
        loop.approval_callback = lambda step: True

        result = await loop.run("fill the form", session_id="s-1", run_id="r-1")

        ledger = _Ledger()
        ledger.attack("denied batch never became a batch step",
                      result.steps[0].action_type != "batch")
        ledger.attack("exactly one retry issued", len(client.calls) == 2)
        ledger.attack(
            "steps byte-identical between attempts",
            client.calls[0]["steps"] == client.calls[1]["steps"]
            and client.calls[1]["steps"] == [
                {"method": "click", "selector": "#open", "arguments": []},
                {"method": "fill", "selector": "#name", "arguments": ["Eoj"]},
                {"method": "click", "selector": "#submit", "arguments": []},
            ],
        )
        ledger.attack("grant placed only on the retry",
                      client.calls[0]["approval_id"] is None
                      and client.calls[1]["approval_id"] == "ap-t")
        ledger.attack("refusal closed the batch context honestly",
                      _closed_statuses(events) == ["failed"])
        ledger.attack("fell back to step-by-step, no silent success",
                      adapter.calls[0] == "left_click" and result.stop_reason == StopReason.DONE)
        ledger.finish()

    @pytest.mark.asyncio
    async def test_replayed_grant_retry_fails_closed(self):
        """The grant was already consumed by an earlier dispatch; the server's
        AlreadyConsumed denial must stop the batch path, not loop."""
        events = []
        provider = _ScriptedProvider([_batch_plan(), _done_plan()])
        adapter = _FakeAdapter()
        client = _FakeBatchClient([
            BatchDispatchResult(executed=False, confirmation_required=True,
                                approval_id="ap-r", action_hash="dhash"),
            BatchDispatchResult(executed=False, status_code=403,
                                error="approval_denied: grant is single-use and has already been redeemed"),
        ])
        loop = _make_loop(provider, adapter, client, events, batch_mode="batch")
        loop.approval_callback = lambda step: True

        result = await loop.run("fill the form", session_id="s-1", run_id="r-1")

        ledger = _Ledger()
        ledger.attack("replay denial stops dispatch",
                      result.steps[0].action_type != "batch")
        ledger.attack("no third attempt", len(client.calls) == 2)
        ledger.attack("replay denial on the audit trail",
                      _closed_statuses(events) == ["failed"])
        ledger.finish()

    @pytest.mark.asyncio
    async def test_executed_without_receipt_fails_closed(self):
        """A 200 with no receipt proves nothing. The engine must not report a
        batch success it has no evidence for."""
        events = []
        provider = _ScriptedProvider([_batch_plan(), _done_plan()])
        adapter = _FakeAdapter()
        client = _FakeBatchClient([
            BatchDispatchResult(executed=True, descriptor_hash="dhash",
                                receipt=None, receipt_id="rcpt-x",
                                enforcement="one_grant"),
        ])
        loop = _make_loop(provider, adapter, client, events, batch_mode="batch")

        result = await loop.run("fill the form", session_id="s-1", run_id="r-1")

        ledger = _Ledger()
        ledger.attack("missing receipt never became a batch step",
                      result.steps[0].action_type != "batch")
        ledger.attack("no receipt closed honestly", _closed_statuses(events) == ["failed"])
        ledger.attack("fallback ran the plan's immediate action",
                      adapter.calls[0] == "left_click")
        ledger.finish()

    def test_requires_approval_workflow_steps_keep_per_step_path(self):
        """A workflow naming an approval-requiring kind must never compile to
        a batch, even when every step is otherwise batchable."""
        steps = [
            {"id": "1", "kind": "click", "target": {"ref": "#a"}, "input": {}},
            {"id": "2", "kind": "hover", "target": {"ref": "#b"}, "input": {}},
        ]
        ledger = _Ledger()
        ledger.attack(
            "approval-requiring kind blocks batching",
            workflow_steps_to_batch_steps(steps, requires_approval_for={"hover"}) is None,
        )
        ledger.attack(
            "same steps batchable without the approval kind",
            workflow_steps_to_batch_steps(steps) is not None,
        )
        ledger.finish()

    def test_mixed_vocabulary_group_is_never_partially_batched(self):
        """One out-of-vocabulary action poisons the whole candidate group —
        the engine never ships a partial batch of the groundable prefix."""
        ledger = _Ledger()
        good = [_action("click", "#a"), _action("click", "#b")]
        ledger.attack("uniform group batches",
                      actions_to_batch_steps(good) is not None)
        ledger.attack(
            "mixed group (coordinate click) does not batch",
            actions_to_batch_steps([_action("click", "#a"), _action("click", "100,200")]) is None,
        )
        ledger.attack(
            "mixed group (wait action) does not batch",
            actions_to_batch_steps([_action("click", "#a"), _action("wait", "screen")]) is None,
        )
        ledger.finish()
