"""
Allternit Computer Use — Cost accounting tests

Covers:
  * core/cost_accounting: honest zero, blended estimation, provider-reported
    vs estimated vs unavailable pricing, planning-result conversion.
  * run_persistence: cost columns roundtrip, migration of pre-cost databases,
    aggregate cost_summary.
  * HTTP: GET /v1/computer-use/runs/{id}/cost and /v1/computer-use/cost/summary;
    wiring through the direct, planning, replay, and workflow paths.
"""

import sqlite3
import sys
import uuid
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

from core.cost_accounting import (  # noqa: E402
    RunCostTracker,
    cost_dict_from_planning_result,
    estimate_cost_usd,
    zero_run_cost,
)
from core.planning_loop import PlanningLoopResult, StopReason  # noqa: E402
import computer_use_router as router_module  # noqa: E402
from computer_use_router import RunState, RunStore  # noqa: E402
from run_persistence import RunPersistence  # noqa: E402


class StubResult:
    def __init__(self, status="completed", error=None):
        self.status = status
        self.error = error

    def to_dict(self):
        return {"status": self.status, "error": self.error}


class StubAdapter:
    async def execute(self, req):
        return StubResult()

    async def screenshot(self, session_id: str = "") -> bytes:
        return b""


def _state(run_id: str, status: str = "completed") -> RunState:
    state = RunState(run_id, "sess-x", "direct", "browser")
    state.status = status
    return state


# ---------------------------------------------------------------------------
# Unit: cost_accounting
# ---------------------------------------------------------------------------

class TestCostAccounting:
    def test_zero_run_cost_is_honest_zero(self):
        cost = zero_run_cost()
        assert cost["total_tokens"] == 0
        assert cost["input_tokens"] == 0
        assert cost["output_tokens"] == 0
        assert cost["est_cost_usd"] == 0.0
        assert cost["pricing"] == "unavailable"
        assert cost["by_stage"] == {}

    def test_estimate_cost_usd_known_model(self):
        # gpt-4o blended: $10/1M
        assert estimate_cost_usd(1_000_000, "gpt-4o") == 10.0
        assert estimate_cost_usd(0, "gpt-4o") == 0.0

    def test_estimate_cost_usd_unknown_model_uses_default(self):
        assert estimate_cost_usd(1_000_000, "some-local-model") == 10.0
        assert estimate_cost_usd(1_000_000, None) == 10.0

    def test_tracker_provider_reported_wins(self):
        tracker = RunCostTracker(model="gpt-4o")
        tracker.record("vision_planning", input_tokens=100, output_tokens=50,
                       total_tokens=150, cost_usd=0.004)
        cost = tracker.to_dict()
        assert cost["pricing"] == "provider-reported"
        assert cost["est_cost_usd"] == 0.004
        assert cost["by_stage"]["vision_planning"]["output_tokens"] == 50

    def test_tracker_estimated_when_only_tokens(self):
        tracker = RunCostTracker(model="gpt-4o")
        tracker.record("vision_planning", total_tokens=200_000)
        cost = tracker.to_dict()
        assert cost["pricing"] == "estimated"
        assert cost["est_cost_usd"] == pytest.approx(2.0)

    def test_tracker_accumulates_stages(self):
        tracker = RunCostTracker()
        tracker.record("vision_planning", total_tokens=100)
        tracker.record("vision_planning", total_tokens=150)
        cost = tracker.to_dict()
        assert cost["total_tokens"] == 250
        assert cost["by_stage"]["vision_planning"]["total_tokens"] == 250

    def test_planning_result_conversion(self):
        result = PlanningLoopResult(
            run_id="r1", session_id="s1", task="t", status="completed",
            stop_reason=StopReason.DONE,
            total_tokens=300, total_input_tokens=200, total_output_tokens=100,
            total_cost_usd=0.0,
        )
        cost = cost_dict_from_planning_result(result, provider=None)
        assert cost["pricing"] == "estimated"
        assert cost["total_tokens"] == 300
        assert cost["input_tokens"] == 200
        assert cost["output_tokens"] == 100
        assert cost["est_cost_usd"] > 0

    def test_planning_result_conversion_no_tokens_is_zero(self):
        result = PlanningLoopResult(
            run_id="r1", session_id="s1", task="t", status="completed",
            stop_reason=StopReason.DONE,
        )
        cost = cost_dict_from_planning_result(result)
        assert cost == zero_run_cost()


# ---------------------------------------------------------------------------
# RunPersistence: cost columns, migration, summary
# ---------------------------------------------------------------------------

class TestRunPersistenceCost:
    def test_cost_columns_roundtrip(self, tmp_path):
        store = RunPersistence(tmp_path / "runs.sqlite3")
        state = _state("run-cost")
        state.cost["input_tokens"] = 200
        state.cost["output_tokens"] = 100
        state.cost["total_tokens"] = 300
        state.cost["est_cost_usd"] = 0.004
        store.upsert_run(state)
        record = store.get_run("run-cost")
        assert record["input_tokens"] == 200
        assert record["output_tokens"] == 100
        assert record["total_tokens"] == 300
        assert record["est_cost_usd"] == 0.004
        store.close()

    def test_existing_database_migrates(self, tmp_path):
        """A runs.sqlite3 created before cost accounting gets the new columns."""
        db = tmp_path / "runs.sqlite3"
        conn = sqlite3.connect(str(db))
        conn.execute(
            """
            CREATE TABLE runs (
                run_id TEXT PRIMARY KEY, session_id TEXT NOT NULL, mode TEXT NOT NULL,
                target_scope TEXT NOT NULL, status TEXT NOT NULL,
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
                result_json TEXT, error TEXT
            )
            """
        )
        conn.execute(
            "INSERT INTO runs VALUES ('old-1','s','direct','browser','completed','t','t',NULL,NULL)"
        )
        conn.commit()
        conn.close()

        store = RunPersistence(db)  # must not raise; ALTER TABLE migrates
        record = store.get_run("old-1")
        assert record["status"] == "completed"
        assert record["total_tokens"] == 0
        assert record["est_cost_usd"] == 0.0
        store.upsert_run(_state("new-1"))
        assert store.get_run("new-1")["total_tokens"] == 0
        store.close()

    def test_cost_summary_aggregates(self, tmp_path):
        store = RunPersistence(tmp_path / "runs.sqlite3")
        store.upsert_run(_state("s1", "completed"))
        rich = _state("s2", "completed")
        rich.cost["total_tokens"] = 300
        rich.cost["est_cost_usd"] = 0.004
        store.upsert_run(rich)
        store.upsert_run(_state("s3", "failed"))
        store.upsert_run(_state("s4", "running"))  # non-terminal

        summary = store.cost_summary()
        assert summary["total_runs"] == 4
        assert summary["completed"] == 2
        assert summary["terminal_runs"] == 3
        assert summary["success_rate"] == pytest.approx(2 / 3, abs=1e-4)
        assert summary["total_est_cost_usd"] == pytest.approx(0.004)
        assert summary["avg_cost_per_task_usd"] == pytest.approx(0.002)
        store.close()

    def test_cost_summary_empty(self, tmp_path):
        store = RunPersistence(tmp_path / "runs.sqlite3")
        summary = store.cost_summary()
        assert summary["total_runs"] == 0
        assert summary["success_rate"] == 0.0
        assert summary["avg_cost_per_task_usd"] == 0.0
        store.close()


# ---------------------------------------------------------------------------
# HTTP: cost endpoints + four-path wiring
# ---------------------------------------------------------------------------

def _make_client(monkeypatch, tmp_path, adapter, extra_routers=()):
    from fastapi import FastAPI
    from fastapi.testclient import TestClient

    monkeypatch.setattr(router_module, "_get_adapter_for_planning", lambda *a, **k: adapter)
    persistence = RunPersistence(tmp_path / "runs.sqlite3")
    fresh_store = RunStore()
    fresh_store.attach_persistence(persistence)
    monkeypatch.setattr(router_module, "_run_store", fresh_store)
    app = FastAPI()
    app.include_router(router_module.router)
    for r in extra_routers:
        app.include_router(r)
    client = TestClient(app)
    client.__enter__()
    return client, fresh_store, persistence


class TestCostEndpoints:
    def test_run_cost_direct_path_honest_zero(self, monkeypatch, tmp_path):
        client, _store, _p = _make_client(monkeypatch, tmp_path, StubAdapter())
        try:
            run_id = f"cu-{uuid.uuid4().hex[:12]}"
            resp = client.post("/v1/computer-use/execute", json={
                "mode": "direct",
                "actions": [{"kind": "screenshot"}],
                "run_id": run_id,
                "session_id": "sess-cost-direct",
            })
            assert resp.json()["status"] == "completed"

            cost_resp = client.get(f"/v1/computer-use/runs/{run_id}/cost")
            assert cost_resp.status_code == 200
            cost = cost_resp.json()["cost"]
            assert cost["pricing"] == "unavailable"
            assert cost["total_tokens"] == 0
            assert cost["est_cost_usd"] == 0.0

            # cost is also embedded in the run record
            run = client.get(f"/v1/computer-use/runs/{run_id}").json()
            assert run["cost"]["pricing"] == "unavailable"
        finally:
            client.__exit__(None, None, None)

    def test_run_cost_planning_path_records_tokens(self, monkeypatch, tmp_path):
        class StubLoop:
            def __init__(self, **kwargs):
                pass

            def cancel(self):
                pass

            async def run(self, task, session_id, run_id):
                return PlanningLoopResult(
                    run_id=run_id, session_id=session_id, task=task,
                    status="completed", stop_reason=StopReason.DONE,
                    total_tokens=300, total_input_tokens=200,
                    total_output_tokens=100, total_cost_usd=0.0,
                )

        monkeypatch.setattr(router_module, "PlanningLoop", StubLoop)
        client, _store, _p = _make_client(monkeypatch, tmp_path, StubAdapter())
        try:
            run_id = f"cu-{uuid.uuid4().hex[:12]}"
            resp = client.post("/v1/computer-use/execute", json={
                "mode": "intent",
                "task": "open example.com",
                "run_id": run_id,
                "session_id": "sess-cost-plan",
                "options": {
                    "vision_provider": "mock",
                    "record": False,
                    "record_gif": False,
                    "approval_policy": "never",
                },
            })
            assert resp.json()["status"] == "completed"

            cost = client.get(f"/v1/computer-use/runs/{run_id}/cost").json()["cost"]
            assert cost["total_tokens"] == 300
            assert cost["input_tokens"] == 200
            assert cost["output_tokens"] == 100
            assert cost["pricing"] == "estimated"
            assert cost["est_cost_usd"] > 0
            assert cost["by_stage"]["vision_planning"]["total_tokens"] == 300
        finally:
            client.__exit__(None, None, None)

    def test_run_cost_survives_restart(self, monkeypatch, tmp_path):
        class StubLoop:
            def __init__(self, **kwargs):
                pass

            def cancel(self):
                pass

            async def run(self, task, session_id, run_id):
                return PlanningLoopResult(
                    run_id=run_id, session_id=session_id, task=task,
                    status="completed", stop_reason=StopReason.DONE,
                    total_tokens=100, total_cost_usd=0.0,
                )

        monkeypatch.setattr(router_module, "PlanningLoop", StubLoop)
        client, _store, persistence = _make_client(monkeypatch, tmp_path, StubAdapter())
        try:
            run_id = f"cu-{uuid.uuid4().hex[:12]}"
            client.post("/v1/computer-use/execute", json={
                "mode": "intent", "task": "t", "run_id": run_id,
                "session_id": "sess-cost-restart",
                "options": {"vision_provider": "mock", "record": False,
                            "record_gif": False, "approval_policy": "never"},
            })

            restarted = RunStore()
            restarted.attach_persistence(persistence)
            monkeypatch.setattr(router_module, "_run_store", restarted)
            cost = client.get(f"/v1/computer-use/runs/{run_id}/cost").json()["cost"]
            assert cost["total_tokens"] == 100
            assert cost["est_cost_usd"] > 0
        finally:
            client.__exit__(None, None, None)

    def test_run_cost_replay_path_honest_zero(self, monkeypatch, tmp_path):
        import core.action_recorder as action_recorder_module

        monkeypatch.setattr(action_recorder_module, "DEFAULT_RECORDINGS_DIR", tmp_path)
        client, _store, _p = _make_client(monkeypatch, tmp_path, StubAdapter())
        try:
            start = client.post("/v1/computer-use/record", json={
                "session_id": "sess-cost-replay", "action": "start", "record_gif": False,
            })
            recording_id = start.json()["recording_id"]
            client.post("/v1/computer-use/record", json={
                "session_id": "sess-cost-replay", "action": "append",
                "recording_id": recording_id,
                "frames": [{"step": 1, "action_type": "screenshot"}],
            })
            client.post("/v1/computer-use/record", json={
                "session_id": "sess-cost-replay", "action": "stop",
                "recording_id": recording_id,
            })
            resp = client.post("/v1/computer-use/replay", json={
                "recording_id": recording_id, "wait": True,
            })
            run_id = resp.json()["run_id"]
            cost = client.get(f"/v1/computer-use/runs/{run_id}/cost").json()["cost"]
            assert cost["pricing"] == "unavailable"
            assert cost["total_tokens"] == 0
        finally:
            client.__exit__(None, None, None)

    def test_run_cost_workflow_path_honest_zero(self, monkeypatch, tmp_path):
        import browser_skills_router as skills_module

        client, store, _p = _make_client(
            monkeypatch, tmp_path, StubAdapter(), extra_routers=(skills_module.router,)
        )
        # browser_skills_router binds _run_store at import time — point its
        # reference at the same fresh store.
        monkeypatch.setattr(skills_module, "_run_store", store)
        try:
            resp = client.post("/v1/browser-skills/run", json={
                "workflow": {
                    "workflowId": "wf-cost",
                    "title": "cost test",
                    "steps": [{"id": "s1", "kind": "screenshot"}],
                },
                "wait": True,
            })
            assert resp.status_code == 200
            run_id = resp.json()["run_id"]
            cost = client.get(f"/v1/computer-use/runs/{run_id}/cost").json()["cost"]
            assert cost["pricing"] == "unavailable"
            assert cost["total_tokens"] == 0
        finally:
            client.__exit__(None, None, None)

    def test_cost_summary_endpoint(self, monkeypatch, tmp_path):
        client, store, _p = _make_client(monkeypatch, tmp_path, StubAdapter())
        try:
            for index in range(2):
                run_id = f"cu-{uuid.uuid4().hex[:12]}"
                resp = client.post("/v1/computer-use/execute", json={
                    "mode": "direct",
                    "actions": [{"kind": "screenshot"}],
                    "run_id": run_id,
                    "session_id": f"sess-summary-{index}",
                })
                assert resp.json()["status"] == "completed"

            summary = client.get("/v1/computer-use/cost/summary").json()["summary"]
            assert summary["total_runs"] == 2
            assert summary["completed"] == 2
            assert summary["success_rate"] == 1.0
            assert summary["avg_cost_per_task_usd"] == 0.0
        finally:
            client.__exit__(None, None, None)

    def test_cost_summary_live_fallback_without_persistence(self, monkeypatch, tmp_path):
        # Detach persistence — endpoint must still answer from live runs.
        client, store, _p = _make_client(monkeypatch, tmp_path, StubAdapter())
        try:
            monkeypatch.setattr(store, "_persistence", None)
            client.post("/v1/computer-use/execute", json={
                "mode": "direct", "actions": [{"kind": "screenshot"}],
                "session_id": "sess-summary-live",
            })
            summary = client.get("/v1/computer-use/cost/summary").json()["summary"]
            assert summary["total_runs"] == 1
            assert summary["completed"] == 1
        finally:
            client.__exit__(None, None, None)

    def test_run_cost_404_unknown_run(self, monkeypatch, tmp_path):
        client, _store, _p = _make_client(monkeypatch, tmp_path, StubAdapter())
        try:
            resp = client.get("/v1/computer-use/runs/does-not-exist/cost")
            assert resp.status_code == 404
        finally:
            client.__exit__(None, None, None)
