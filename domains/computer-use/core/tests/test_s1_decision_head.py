"""Tests for the S1 decision runtime client + S1DecisionHead (mocked HTTP transport)."""
from __future__ import annotations

import json

import pytest

from core import shadow_eval
from core.decision_head import DecisionHead, Question, ShadowHeadError
from core.element_refs import get_refmap
from core.planning_loop import PlanningLoop, PlanningLoopConfig
from core.s1_decision_client import S1DecisionClient, choice_request
from core.s1_decision_head import CU_BANK, S1DecisionHead


class FakeRuntime:
    """Records every POST; answers /v1/decision with a peaked distribution
    (first option, or ``prefer`` when it is an option) and numbered ids."""

    def __init__(self, prefer=None, status=200, raise_exc=None):
        self.calls = []
        self.prefer = prefer or {}
        self.status = status
        self.raise_exc = raise_exc

    def __call__(self, url, body, headers, timeout):
        if self.raise_exc:
            raise self.raise_exc
        payload = json.loads(body)
        self.calls.append((url, payload, headers))
        if url.endswith("/v1/decision/outcome"):
            return 200, b'{"ok":true}'
        if self.status != 200:
            return self.status, b'{"error":{"message":"nope"}}'
        req = payload["request"]
        opts = [c["candidate_id"] for c in req["candidates"]]
        want = self.prefer.get(req["question_id"], opts[0])
        probs = {o: (0.9 if o == want else 0.1 / max(1, len(opts) - 1)) for o in opts}
        if len(opts) == 1:
            probs = {opts[0]: 1.0}
        n = sum(1 for u, *_ in self.calls if u.endswith("/v1/decision"))
        return 200, json.dumps({"probabilities": probs, "extensions": {"x-decision_id": f"dec-{n}"}}).encode()

    def decisions(self):
        return [p for u, p, _ in self.calls if u.endswith("/v1/decision")]

    def outcomes(self):
        return [p for u, p, _ in self.calls if u.endswith("/v1/decision/outcome")]


def _client(rt, **kw):
    return S1DecisionClient(url="http://s1.test", backend="auto", token="tok", transport=rt, **kw)


def test_choice_request_shape():
    req = choice_request(producer="p", bank="bank.x", question_id="operation", options=["click", "type"],
                         instructions="i", primitive_id="computer_use.operation", subject_ref="cu:r:1:operation")
    assert req["operation"] == "CHOICE"
    assert req["decision_bank_id"] == "bank.x"
    assert [c["candidate_id"] for c in req["candidates"]] == ["click", "type"]
    assert req["extensions"]["x-primitive_id"] == "computer_use.operation"
    assert req["extensions"]["x-subject_ref"] == "cu:r:1:operation"


def test_head_is_a_decision_head_and_maps_questions_to_choice_requests():
    rt = FakeRuntime(prefer={"goal_satisfied": "false"})
    head = S1DecisionHead(_client(rt))
    assert isinstance(head, DecisionHead)
    head.begin_run("run-1")
    head.begin_step(3)
    qs = [Question("operation", ["click", "type"]), Question("click_target", ["e1", "e2", "e3"]),
          Question("goal_satisfied", ["true", "false"])]
    d = head.decide("[TASK]\nx", qs)
    assert set(d.choices) == {"operation", "click_target", "goal_satisfied"}
    assert d.choices["goal_satisfied"].chosen == "false"
    assert d.choices["operation"].probabilities["click"] == pytest.approx(0.9)
    assert d.model_id == "s1-decision:auto"
    sent = rt.decisions()
    assert len(sent) == 3
    assert all(p["request"]["decision_bank_id"] == CU_BANK for p in sent)
    assert all(p["backend"] == "auto" and p["state"] == "[TASK]\nx" for p in sent)
    assert sent[1]["request"]["extensions"]["x-primitive_id"] == "computer_use.target"
    assert sent[0]["request"]["extensions"]["x-subject_ref"] == "cu:run-1:3:operation"
    assert rt.calls[0][2]["authorization"] == "Bearer tok"
    assert head.decision_id("operation") == "dec-1"


def test_outcome_reports_only_a_valid_option_of_a_recorded_decision():
    rt = FakeRuntime()
    head = S1DecisionHead(_client(rt))
    head.begin_step(1)
    head.decide("s", [Question("operation", ["click", "type"])])
    assert head.report_outcome("operation", "type", "cu.executed_operation", step=1)
    assert rt.outcomes() == [{"decision_id": "dec-1", "truth": "type", "source": "cu.executed_operation"}]
    assert not head.report_outcome("operation", "scroll", "x", step=1)  # not an option
    assert not head.report_outcome("operation", "type", "x", step=9)  # no decision
    off = S1DecisionHead(_client(FakeRuntime(), outcomes_enabled=False))
    off.decide("s", [Question("operation", ["click"])])
    assert not off.report_outcome("operation", "click", "x")


def test_runtime_errors_surface_as_shadow_head_errors():
    with pytest.raises(ShadowHeadError):
        S1DecisionHead(_client(FakeRuntime(status=500))).decide("s", [Question("operation", ["click"])])
    with pytest.raises(ShadowHeadError):
        S1DecisionHead(_client(FakeRuntime(raise_exc=OSError("refused")))).decide("s", [Question("operation", ["click"])])
    # Outcome reporting never raises.
    assert not _client(FakeRuntime(raise_exc=OSError("refused"))).report_outcome("d", "t", "s")


async def test_planning_loop_reports_executed_operations_and_episode_outcome():
    task = shadow_eval.default_tasks(4)[1]
    rt = FakeRuntime()
    head = S1DecisionHead(_client(rt))
    get_refmap().clear()
    _, restore = shadow_eval._patch_inspector(shadow_eval._step_trees(task))
    try:
        loop = PlanningLoop(
            vision_provider=shadow_eval.ScriptedVisionProvider(task.turns),
            adapter=shadow_eval.ScriptedAdapter(fail_targets=task.fail_targets),
            config=PlanningLoopConfig(
                max_steps=len(task.turns) + 2, approval_policy="never",
                reflect_after_each_step=False, batch_enabled=False,
                shadow_head_enabled=True, shadow_head=head,
            ),
            event_callback=lambda _e: None,
        )
        result = await loop.run(task.task, session_id="s1-head-run")
    finally:
        restore()
    assert rt.decisions(), "the head was asked"
    outs = rt.outcomes()
    executed = [o for o in outs if o["source"] == "cu.executed_operation"]
    assert executed, outs
    op_ids = {p["request"]["extensions"]["x-subject_ref"]: i + 1 for i, p in enumerate(rt.decisions())}
    assert all(o["decision_id"] in {f"dec-{v}" for v in op_ids.values()} for o in executed)
    assert result.status == "completed"
    goal = [o for o in outs if o["source"] == "cu.run_completed_later"]
    goal_ids = {f"dec-{i + 1}" for i, p in enumerate(rt.decisions()) if p["request"]["question_id"] == "goal_satisfied"}
    assert goal and {o["truth"] for o in goal} == {"false"}
    # WP-S1U-3: the head also ran on the planner's `done` step; that one
    # goal_satisfied decision gets the only positive label.
    done = [o for o in outs if o["source"] == "cu.planner_done"]
    assert len(done) == 1 and done[0]["truth"] == "true"
    assert {o["decision_id"] for o in goal} | {done[0]["decision_id"]} == goal_ids
    assert done[0]["decision_id"] not in {o["decision_id"] for o in goal}
    # Q26 x-incumbent: the planner's answers ride along. goal_satisfied is
    # "true" only on the done step; operation matches the executed operation
    # when it is an option; targets and stuck carry none.
    sent = rt.decisions()
    goal_inc = {p["request"]["extensions"].get("x-incumbent") for p in sent if p["request"]["question_id"] == "goal_satisfied"}
    assert goal_inc == {"true", "false"}
    ops = [p for p in sent if p["request"]["question_id"] == "operation"]
    assert any("x-incumbent" in p["request"]["extensions"] for p in ops)
    assert all("x-incumbent" not in p["request"]["extensions"] for p in sent
               if p["request"]["question_id"].endswith("_target") or p["request"]["question_id"] == "stuck")


def test_incumbent_is_sent_only_when_it_is_an_option_and_resets_per_step():
    rt = FakeRuntime()
    head = S1DecisionHead(_client(rt))
    head.begin_step(1)
    head.set_incumbent({"operation": "type", "goal_satisfied": "maybe"})
    head.decide("s", [Question("operation", ["click", "type"]), Question("goal_satisfied", ["true", "false"])])
    head.begin_step(2)
    head.decide("s", [Question("operation", ["click", "type"])])
    ext = [p["request"]["extensions"] for p in rt.decisions()]
    assert ext[0]["x-incumbent"] == "type"
    assert "x-incumbent" not in ext[1] and "x-incumbent" not in ext[2]
