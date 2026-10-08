"""
Batch dispatch + planning-loop batch consumption tests.

Covers (spec stagehand-batch-fork, P2):
  * core/batch_dispatch: whitelist mapping discipline (selector-like targets
    only, vocabulary-only methods), per-step/one-grant retry placement.
  * planning_loop: a plan carrying ≥2 groundable actions dispatches ONE batch
    through the injected client; observation returns to LoopStep fields;
    halt-at-first-failure reports honestly and the loop re-plans; a declined
    or failed batch falls back to the existing step-by-step path; grant
    denials route through the human approval flow.
  * vision_providers: the optional ActionPlan.batch field parses.
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
    AciBatchClient,
    BatchDispatchResult,
    action_to_batch_step,
    actions_to_batch_steps,
    place_grant_for_retry,
)
from core.planning_loop import PlanningLoop, PlanningLoopConfig, StopReason  # noqa: E402
from core.vision_providers import ActionPlan, VisionAction, _parse_action_plan  # noqa: E402


def _action(type_, target, text=None):
    return VisionAction(type=type_, target=target, reason="t", text=text)


# ── Mapping discipline ───────────────────────────────────────────────────────

class TestActionMapping:
    def test_whitelisted_actions_map_with_selector_targets(self):
        assert action_to_batch_step(_action("click", "#go")) == {
            "method": "click", "selector": "#go", "arguments": [],
        }
        assert action_to_batch_step(_action("double_click", ".item"))["method"] == "doubleClick"
        assert action_to_batch_step(_action("type", "#name", text="Eoj")) == {
            "method": "fill", "selector": "#name", "arguments": ["Eoj"],
        }
        assert action_to_batch_step(_action("fill", "//input[@id='x']", text="hi"))["method"] == "fill"
        assert action_to_batch_step(_action("scroll", "main"))["method"] == "scrollTo"
        assert action_to_batch_step(_action("key", "#name", text="Enter"))["method"] == "press"

    def test_attribute_selectors_are_selector_like(self):
        # Frontier grounding models prefer attribute selectors even when ids
        # exist (cu22 real-model campaign); they resolve deterministically
        # via querySelector, so they ground like #id or bare-tag selectors.
        step = action_to_batch_step(_action("fill", 'input[placeholder="Full name"]', text="Ada"))
        assert step == {"method": "fill",
                        "selector": 'input[placeholder="Full name"]',
                        "arguments": ["Ada"]}
        assert action_to_batch_step(_action("click", "button[type='submit']"))["method"] == "click"
        assert action_to_batch_step(_action("select", "#plan", text="pro")) == {
            "method": "selectOptionFromDropdown", "selector": "#plan", "arguments": ["pro"],
        }

    def test_non_selector_targets_are_not_batchable(self):
        # Free-text element descriptions cannot be resolved by the in-browser
        # runtime — these keep the coordinate-based step-by-step path.
        assert action_to_batch_step(_action("click", "submit button")) is None
        assert action_to_batch_step(_action("click", "")) is None

    def test_out_of_vocabulary_actions_are_not_batchable(self):
        for type_ in ("navigate", "wait", "screenshot", "left_click_drag", "extract"):
            assert action_to_batch_step(_action(type_, "#a")) is None

    def test_group_requires_min_steps_and_uniform_batchability(self):
        assert actions_to_batch_steps([_action("click", "#a")]) is None
        good = actions_to_batch_steps([_action("click", "#a"), _action("type", "#b", text="x")])
        assert [s["method"] for s in good] == ["click", "fill"]
        mixed = [_action("click", "#a"), _action("navigate", "http://x/")]
        assert actions_to_batch_steps(mixed) is None
        mixed_target = [_action("click", "#a"), _action("click", "save button")]
        assert actions_to_batch_steps(mixed_target) is None


class TestGrantPlacement:
    def test_one_grant_denial_retries_with_approval_id(self):
        attempt = BatchDispatchResult(
            executed=False, confirmation_required=True,
            approval_id="ap-1", action_hash="desc-hash",
        )
        kwargs = place_grant_for_retry("batch", attempt, "ap-1", 3)
        assert kwargs == {"approval_id": "ap-1", "step_approval_ids": None}

    def test_per_step_denial_retries_at_the_named_index(self):
        attempt = BatchDispatchResult(
            executed=False, confirmation_required=True,
            approval_id="ap-2", step_index=1, action_hash="step-hash",
        )
        kwargs = place_grant_for_retry("per_step", attempt, "ap-2", 3)
        assert kwargs["approval_id"] is None
        assert kwargs["step_approval_ids"] == [None, "ap-2", None]


# ── Engine batch consumption ─────────────────────────────────────────────────

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
        # Empty screenshot keeps tests offline and off the host display.
        return b""

    async def execute(self, req):
        self.calls.append(req.action_type)
        class _Result:
            def to_dict(self):
                return {"status": "ok", "action": req.action_type}
        return _Result()


class _FakeBatchClient:
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
    return {
        "status": status,
        "halted_at": halted_at,
        "steps": steps,
    }


def _batch_plan():
    return ActionPlan(
        reasoning="three grounded steps",
        plan_steps=["open form", "fill", "submit"],
        confidence=0.9,
        immediate_action=_action("click", "#open"),
        batch=[_action("type", "#name", text="Eoj"), _action("click", "#submit")],
    )


def _make_loop(provider, adapter, client, ledger_events, **config_kwargs):
    config_kwargs.setdefault("approval_policy", "never")
    loop = PlanningLoop(
        provider,
        adapter,
        PlanningLoopConfig(**config_kwargs),
        ledger=lambda et, payload: ledger_events.append((et, payload)),
        batch_client=client,
    )
    return loop


class TestPlanningLoopBatch:
    @pytest.mark.asyncio
    async def test_batch_plan_dispatches_one_batch_and_closes_ledger(self):
        events = []
        provider = _ScriptedProvider([_batch_plan()])  # then auto-done
        adapter = _FakeAdapter()
        receipt = _receipt("completed", [
            {"index": 0, "status": "completed"},
            {"index": 1, "status": "completed"},
            {"index": 2, "status": "completed"},
        ])
        client = _FakeBatchClient([
            BatchDispatchResult(executed=True, descriptor_hash="dhash",
                                receipt=receipt, receipt_id="rcpt-1",
                                enforcement="one_grant"),
        ])
        loop = _make_loop(provider, adapter, client, events, batch_mode="batch")

        result = await loop.run("fill the form", session_id="s-1", run_id="r-1")

        assert len(client.calls) == 1
        assert adapter.calls == []  # no step-by-step dispatch
        assert [s["method"] for s in client.calls[0]["steps"]] == ["click", "fill", "click"]
        batch_step = result.steps[0]
        assert batch_step.action_type == "batch"
        assert batch_step.action_succeeded is True
        assert batch_step.adapter_result["batch_receipt"]["status"] == "completed"
        assert batch_step.action_params["descriptor_hash"] == "dhash"
        # Observation still lands in the LoopStep fields.
        assert "after_screenshot_b64" in batch_step.to_dict()
        # Ledger: opened before, closed with outcome + receipt pointer.
        kinds = [e[0] for e in events]
        assert kinds == ["batch.context.opened", "batch.context.closed"]
        opened, closed = events[0][1], events[1][1]
        assert opened["step_methods"] == ["click", "fill", "click"]
        assert "arguments" not in opened
        assert closed["batch_id"] == "dhash"
        assert closed["receipt_id"] == "rcpt-1"
        assert closed["status"] == "completed"
        assert closed["model_turns_saved"] == 2
        assert result.model_turns == 2  # batch turn + final done turn
        assert result.stop_reason == StopReason.DONE

    @pytest.mark.asyncio
    async def test_halt_at_first_failure_reports_and_replans(self):
        events = []
        provider = _ScriptedProvider([
            _batch_plan(),
            ActionPlan(reasoning="recover", plan_steps=[], confidence=0.9, done=True,
                       immediate_action=VisionAction(type="screenshot", target="screen", reason="")),
        ])
        adapter = _FakeAdapter()
        receipt = _receipt("completed_halted", [
            {"index": 0, "status": "completed"},
            {"index": 1, "status": "failed"},
            {"index": 2, "status": "skipped"},
        ], halted_at=1)
        client = _FakeBatchClient([
            BatchDispatchResult(executed=True, descriptor_hash="dhash",
                                receipt=receipt, receipt_id="rcpt-h"),
        ])
        loop = _make_loop(provider, adapter, client, events)

        result = await loop.run("fill the form", session_id="s-1", run_id="r-1")

        batch_step = result.steps[0]
        assert batch_step.action_succeeded is False
        assert "halted" in (batch_step.error or "")
        assert "at step 1" in (batch_step.error or "")
        # Skipped steps are NOT retried by the engine; the loop re-plans.
        assert adapter.calls == []
        assert len(client.calls) == 1
        closed = [e for e in events if e[0] == "batch.context.closed"][0][1]
        assert closed["status"] == "completed_halted"
        assert closed["halted_at"] == 1
        assert closed["steps_completed"] == 1
        # The loop re-planned from the post-batch observation and finished.
        assert result.stop_reason == StopReason.DONE
        assert result.model_turns == 2

    @pytest.mark.asyncio
    async def test_one_grant_denial_routes_through_approval_and_retries(self):
        events = []
        provider = _ScriptedProvider([_batch_plan()])
        adapter = _FakeAdapter()
        receipt = _receipt("completed", [
            {"index": i, "status": "completed"} for i in range(3)
        ])
        client = _FakeBatchClient([
            BatchDispatchResult(executed=False, confirmation_required=True,
                                approval_id="ap-9", action_hash="dhash"),
            BatchDispatchResult(executed=True, descriptor_hash="dhash",
                                receipt=receipt, receipt_id="rcpt-g",
                                enforcement="one_grant"),
        ])
        approvals = []
        loop = _make_loop(provider, adapter, client, events, batch_mode="batch")
        loop.approval_callback = lambda step: approvals.append(step) or True

        result = await loop.run("fill the form", session_id="s-1", run_id="r-1")

        assert len(approvals) == 1  # human was asked exactly once
        assert len(client.calls) == 2
        assert client.calls[1]["approval_id"] == "ap-9"
        assert client.calls[1]["step_approval_ids"] is None
        assert result.steps[0].action_succeeded is True
        assert result.stop_reason == StopReason.DONE

    @pytest.mark.asyncio
    async def test_per_step_denial_places_grant_at_step_index(self):
        events = []
        provider = _ScriptedProvider([_batch_plan()])
        adapter = _FakeAdapter()
        receipt = _receipt("completed", [
            {"index": i, "status": "completed"} for i in range(3)
        ])
        client = _FakeBatchClient([
            BatchDispatchResult(executed=False, confirmation_required=True,
                                approval_id="ap-s", step_index=1, action_hash="shash"),
            BatchDispatchResult(executed=True, descriptor_hash="dhash",
                                receipt=receipt, receipt_id="rcpt-p",
                                enforcement="per_step"),
        ])
        loop = _make_loop(provider, adapter, client, events, batch_mode="per_step")
        loop.approval_callback = lambda step: True

        result = await loop.run("fill the form", session_id="s-1", run_id="r-1")

        assert client.calls[1]["approval_id"] is None
        assert client.calls[1]["step_approval_ids"] == [None, "ap-s", None]
        assert result.steps[0].action_succeeded is True

    @pytest.mark.asyncio
    async def test_declined_grant_falls_back_to_step_by_step(self):
        events = []
        provider = _ScriptedProvider([
            _batch_plan(),
            ActionPlan(reasoning="finish", plan_steps=[], confidence=0.9, done=True,
                       immediate_action=VisionAction(type="screenshot", target="screen", reason="")),
        ])
        adapter = _FakeAdapter()
        client = _FakeBatchClient([
            BatchDispatchResult(executed=False, confirmation_required=True,
                                approval_id="ap-x", action_hash="dhash"),
        ])
        loop = _make_loop(provider, adapter, client, events)
        loop.approval_callback = lambda step: False  # human declined

        result = await loop.run("fill the form", session_id="s-1", run_id="r-1")

        # No retry, and the immediate action ran through the normal path.
        assert len(client.calls) == 1
        assert adapter.calls == ["left_click"]
        closed = [e for e in events if e[0] == "batch.context.closed"][0][1]
        assert closed["status"] == "denied"
        assert result.stop_reason == StopReason.DONE

    @pytest.mark.asyncio
    async def test_unbatchable_plan_keeps_step_by_step_path(self):
        events = []
        # Coordinate-only click (no selector target) → not groundable.
        plan = ActionPlan(
            reasoning="click by coordinates",
            plan_steps=["click", "type"],
            confidence=0.9,
            immediate_action=VisionAction(type="click", target="submit button",
                                          reason="", coordinates=[100, 200]),
            batch=[_action("type", "name field", text="x")],
        )
        provider = _ScriptedProvider([
            plan,
            ActionPlan(reasoning="finish", plan_steps=[], confidence=0.9, done=True,
                       immediate_action=VisionAction(type="screenshot", target="screen", reason="")),
        ])
        adapter = _FakeAdapter()
        client = _FakeBatchClient([])  # must never be called
        loop = _make_loop(provider, adapter, client, events)

        result = await loop.run("click around", session_id="s-1", run_id="r-1")

        assert client.calls == []
        assert adapter.calls == ["left_click"]
        assert result.steps[0].action_type == "left_click"
        assert [e[0] for e in events] == []  # no batch context without a batch
        assert result.stop_reason == StopReason.DONE

    @pytest.mark.asyncio
    async def test_batch_disabled_keeps_step_by_step_path(self):
        events = []
        provider = _ScriptedProvider([
            _batch_plan(),
            ActionPlan(reasoning="finish", plan_steps=[], confidence=0.9, done=True,
                       immediate_action=VisionAction(type="screenshot", target="screen", reason="")),
        ])
        adapter = _FakeAdapter()
        client = _FakeBatchClient([])
        loop = _make_loop(provider, adapter, client, events, batch_enabled=False)

        result = await loop.run("fill the form", session_id="s-1", run_id="r-1")

        assert client.calls == []
        assert adapter.calls == ["left_click"]
        assert result.stop_reason == StopReason.DONE


# ── Automatic page binding (deferral B) ─────────────────────────────────────

class _UrlAdapter(_FakeAdapter):
    """Fake adapter whose surface carries a current page URL."""

    def __init__(self, url="https://app.example/step2"):
        super().__init__()
        self._url = url

    async def get_url(self):
        return self._url


class TestAutoPageBinding:
    @pytest.mark.asyncio
    async def test_observed_url_pins_the_next_batch_descriptor(self):
        events = []
        provider = _ScriptedProvider([_batch_plan(), _batch_plan()])
        adapter = _UrlAdapter()
        emitted = []
        client = _FakeBatchClient([
            BatchDispatchResult(executed=True, descriptor_hash="d1",
                                receipt=_receipt("completed", [
                                    {"index": i, "status": "completed"} for i in range(3)
                                ]), receipt_id="r1"),
            BatchDispatchResult(executed=True, descriptor_hash="d2",
                                receipt=_receipt("completed", [
                                    {"index": i, "status": "completed"} for i in range(3)
                                ]), receipt_id="r2"),
        ])
        loop = _make_loop(provider, adapter, client, events)
        loop.event_callback = emitted.append

        result = await loop.run("two batch turns", session_id="s-1", run_id="r-1")

        assert result.stop_reason == StopReason.DONE
        # First dispatch: nothing observed yet → origin+session only.
        assert client.calls[0]["page_url"] is None
        # Post-step observation carried the URL → pinned into the 2nd descriptor.
        assert client.calls[1]["page_url"] == "https://app.example/step2"
        # The ledger record agrees with the descriptor binding.
        opened2 = [e for e in events if e[0] == "batch.context.opened"][1][1]
        assert opened2["page_url"] == "https://app.example/step2"
        # The observation is surfaced as an event.
        assert any(e.get("type") == "page.observed"
                   and e.get("url") == "https://app.example/step2" for e in emitted)

    @pytest.mark.asyncio
    async def test_operator_pin_wins_over_observed_url(self):
        events = []
        provider = _ScriptedProvider([_batch_plan(), _batch_plan()])
        adapter = _UrlAdapter(url="https://observed.example/x")
        client = _FakeBatchClient([
            BatchDispatchResult(executed=True, descriptor_hash="d1",
                                receipt=_receipt("completed", [
                                    {"index": i, "status": "completed"} for i in range(3)
                                ]), receipt_id="r1"),
            BatchDispatchResult(executed=True, descriptor_hash="d2",
                                receipt=_receipt("completed", [
                                    {"index": i, "status": "completed"} for i in range(3)
                                ]), receipt_id="r2"),
        ])
        loop = _make_loop(provider, adapter, client, events,
                          batch_page_url="https://operator.example/pinned")

        await loop.run("two batch turns", session_id="s-1", run_id="r-1")

        assert client.calls[0]["page_url"] == "https://operator.example/pinned"
        assert client.calls[1]["page_url"] == "https://operator.example/pinned"

    @pytest.mark.asyncio
    async def test_no_url_surface_keeps_origin_session_binding(self):
        events = []
        provider = _ScriptedProvider([_batch_plan(), _batch_plan()])
        adapter = _FakeAdapter()  # no get_url()
        client = _FakeBatchClient([
            BatchDispatchResult(executed=True, descriptor_hash="d1",
                                receipt=_receipt("completed", [
                                    {"index": i, "status": "completed"} for i in range(3)
                                ]), receipt_id="r1"),
            BatchDispatchResult(executed=True, descriptor_hash="d2",
                                receipt=_receipt("completed", [
                                    {"index": i, "status": "completed"} for i in range(3)
                                ]), receipt_id="r2"),
        ])
        loop = _make_loop(provider, adapter, client, events)

        await loop.run("two batch turns", session_id="s-1", run_id="r-1")

        assert client.calls[0]["page_url"] is None
        assert client.calls[1]["page_url"] is None


# ── F1: post-batch observation from the batch's own browser ──────────────────

import base64 as _std_b64

_SIDECAR_SCREEN = _std_b64.b64encode(b"sidecar-post-batch-pixels").decode()


class _StaleAdapter(_UrlAdapter):
    """Operator-adapter view that disagrees with the batch's sidecar state —
    the cu22 campaign's stale-screen condition, reproduced in mocks."""

    def __init__(self):
        super().__init__(url="https://operator.example/stale")

    async def screenshot(self, session_id):
        return b"stale-adapter-pixels"


def _four_step_batch_plan():
    return ActionPlan(
        reasoning="four grounded steps",
        plan_steps=["open", "fill", "pick", "submit"],
        confidence=0.9,
        immediate_action=_action("click", "#open"),
        batch=[
            _action("type", "#name", text="Eoj"),
            _action("select", "#plan", text="pro"),
            _action("click", "#submit"),
        ],
    )


class TestPostBatchObservation:
    @pytest.mark.asyncio
    async def test_observation_comes_from_the_batch_browser_not_the_adapter(self):
        events = []
        emitted = []
        provider = _ScriptedProvider([_four_step_batch_plan(), _four_step_batch_plan()])
        adapter = _StaleAdapter()
        client = _FakeBatchClient([
            BatchDispatchResult(
                executed=True, descriptor_hash="d1",
                receipt=_receipt("completed", [
                    {"index": i, "status": "completed"} for i in range(4)
                ]),
                receipt_id="r1",
                post_batch_observation={
                    "screenshot_b64": _SIDECAR_SCREEN,
                    "url": "https://sidecar.example/step-done",
                    "title": "Submitted",
                },
            ),
            BatchDispatchResult(
                executed=True, descriptor_hash="d2",
                receipt=_receipt("completed", [
                    {"index": i, "status": "completed"} for i in range(4)
                ]),
                receipt_id="r2",
                post_batch_observation={
                    "screenshot_b64": _SIDECAR_SCREEN,
                    "url": "https://sidecar.example/step-done",
                },
            ),
        ])
        loop = _make_loop(provider, adapter, client, events)
        loop.event_callback = emitted.append

        result = await loop.run("fill and submit", session_id="s-1", run_id="r-1")

        assert result.stop_reason == StopReason.DONE
        batch_step = result.steps[0]
        # The LoopStep observation reflects the SIDECAR state the batch ran
        # in — not the adapter's stale pixels.
        assert batch_step.after_screenshot_b64 == _SIDECAR_SCREEN
        assert batch_step.after_screenshot_b64 != _std_b64.b64encode(
            b"stale-adapter-pixels").decode()
        # The NEXT batch descriptor binds the sidecar URL, not the adapter's.
        assert client.calls[1]["page_url"] == "https://sidecar.example/step-done"
        opened2 = [e for e in events if e[0] == "batch.context.opened"][1][1]
        assert opened2["page_url"] == "https://sidecar.example/step-done"
        assert any(e.get("type") == "page.observed"
                   and e.get("url") == "https://sidecar.example/step-done"
                   for e in emitted)

    @pytest.mark.asyncio
    async def test_turn_count_saved_end_to_end_through_real_observation_path(self):
        # The P2 4→2 measurement used a scripted provider that declared done
        # from the receipt; the cu22 campaign showed real models re-plan
        # against a stale adapter screen and never save turns. With the
        # observation routed from the batch context, a scripted 4-step batch
        # shows the reduced-turn behavior through the REAL observation path.
        events = []
        provider = _ScriptedProvider([_four_step_batch_plan()])  # then auto-done
        adapter = _StaleAdapter()
        client = _FakeBatchClient([
            BatchDispatchResult(
                executed=True, descriptor_hash="d1",
                receipt=_receipt("completed", [
                    {"index": i, "status": "completed"} for i in range(4)
                ]),
                receipt_id="r1",
                post_batch_observation={
                    "screenshot_b64": _SIDECAR_SCREEN,
                    "url": "https://sidecar.example/step-done",
                },
            ),
        ])
        loop = _make_loop(provider, adapter, client, events)

        result = await loop.run("fill and submit", session_id="s-1", run_id="r-1")

        # 4 steps in ONE batch turn + 1 done turn = 2 turns (would be ≥5 if
        # the model re-planned per step against the stale adapter screen).
        assert result.model_turns == 2
        assert len(client.calls) == 1  # no grant-amplifying re-batch
        assert provider.turns == 2
        closed = [e for e in events if e[0] == "batch.context.closed"][0][1]
        assert closed["model_turns_saved"] == 3

    @pytest.mark.asyncio
    async def test_undecodable_observation_falls_back_to_adapter(self):
        events = []
        provider = _ScriptedProvider([_batch_plan()])
        adapter = _StaleAdapter()
        client = _FakeBatchClient([
            BatchDispatchResult(
                executed=True, descriptor_hash="d1",
                receipt=_receipt("completed", [
                    {"index": i, "status": "completed"} for i in range(3)
                ]),
                receipt_id="r1",
                post_batch_observation={"screenshot_b64": "!!!not-base64!!!",
                                        "url": "https://sidecar.example/x"},
            ),
        ])
        loop = _make_loop(provider, adapter, client, events)

        result = await loop.run("fill the form", session_id="s-1", run_id="r-1")

        assert result.stop_reason == StopReason.DONE
        step = result.steps[0]
        assert step.after_screenshot_b64 == _std_b64.b64encode(
            b"stale-adapter-pixels").decode()
        # The URL still came from the batch context (it decoded fine).
        assert loop._observed_url == "https://sidecar.example/x"

    @pytest.mark.asyncio
    async def test_no_observation_keeps_adapter_path(self):
        events = []
        provider = _ScriptedProvider([_batch_plan()])
        adapter = _StaleAdapter()
        client = _FakeBatchClient([
            BatchDispatchResult(executed=True, descriptor_hash="d1",
                                receipt=_receipt("completed", [
                                    {"index": i, "status": "completed"} for i in range(3)
                                ]), receipt_id="r1"),
        ])
        loop = _make_loop(provider, adapter, client, events)

        result = await loop.run("fill the form", session_id="s-1", run_id="r-1")

        step = result.steps[0]
        assert step.after_screenshot_b64 == _std_b64.b64encode(
            b"stale-adapter-pixels").decode()
        assert loop._observed_url == "https://operator.example/stale"


# ── Provider parsing ─────────────────────────────────────────────────────────

class TestBatchPlanParsing:
    def test_parse_action_plan_reads_optional_batch(self):
        raw = """
        {"reasoning": "r", "plan_steps": ["a"], "confidence": 0.8,
         "immediate_action": {"type": "click", "target": "#a"},
         "batch": [{"type": "fill", "target": "#b", "text": "hi"}],
         "done": false}
        """
        plan = _parse_action_plan(raw)
        assert plan.batch is not None
        assert len(plan.batch) == 1
        assert plan.batch[0].type == "fill"
        assert plan.batch[0].text == "hi"
        assert plan.immediate_action.type == "left_click"

    def test_parse_action_plan_without_batch_is_none(self):
        plan = _parse_action_plan(
            '{"immediate_action": {"type": "click", "target": "#a"}, "done": false}'
        )
        assert plan.batch is None

    def test_batch_field_present_in_action_plan_schema(self):
        from core.vision_providers import ACTION_PLAN_JSON_SCHEMA
        assert "batch" in ACTION_PLAN_JSON_SCHEMA["properties"]


class TestAciBatchClient:
    def test_defaults_from_env(self, monkeypatch):
        monkeypatch.setenv("ALLTERNIT_API_URL", "http://127.0.0.1:9999/")
        monkeypatch.setenv("ALLTERNIT_INTERNAL_SERVICE_TOKEN", "tok")
        client = AciBatchClient()
        assert client.base_url == "http://127.0.0.1:9999"
