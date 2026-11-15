# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Allternit LLC
"""Engine API v1 over real HTTP, with a fake worker process in place of torch."""
from __future__ import annotations

import json
import sys
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path

import pytest

from allternit_tune.jobs import JobManager
from allternit_tune.server import engine_card, serve

TOKEN = "t0ken"
FAKE = str(Path(__file__).with_name("fake_worker.py"))
HARDWARE = {"device": "mps", "torch": "2.x", "unavailable": {"qlora": "needs CUDA"}}


@pytest.fixture
def engine(tmp_path, monkeypatch):
    def start(mode: str = "ok"):
        monkeypatch.setenv("FAKE_WORKER_MODE", mode)
        manager = JobManager(
            tmp_path / "data",
            unavailable_methods=HARDWARE["unavailable"],
            worker_command=lambda plan: [sys.executable, FAKE, str(plan)],
            cancel_grace_s=2,
        )
        server = serve(manager, TOKEN, 0, lambda: engine_card(HARDWARE))
        threading.Thread(target=server.serve_forever, daemon=True).start()
        started.append(server)
        return manager, f"http://127.0.0.1:{server.server_address[1]}"

    started: list = []
    yield start
    for server in started:
        server.shutdown()
        server.server_close()


def call(base, method, path, body=None, token=TOKEN):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(base + path, data=data, method=method)
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            return resp.status, json.loads(resp.read())
    except urllib.error.HTTPError as err:
        return err.code, json.loads(err.read())


def wait_state(base, job_id, states, timeout=20):
    deadline = time.time() + timeout
    while time.time() < deadline:
        _, job = call(base, "GET", f"/v1/jobs/{job_id}")
        if job["state"] in states:
            return job
        time.sleep(0.05)
    raise AssertionError(f"job never reached {states}; last {job['state']}")


def test_token_required(engine):
    _, base = engine()
    assert call(base, "GET", "/v1/engine", token=None)[0] == 401
    status, body = call(base, "GET", "/v1/engine", token="wrong")
    assert status == 401 and body["error"] == "unauthorized"


def test_engine_card_states_what_this_computer_cannot_run(engine):
    _, base = engine()
    status, card = call(base, "GET", "/v1/engine")
    assert status == 200
    assert card["id"] == "allternit-tune" and card["license"] == "Apache-2.0"
    assert card["install_state"] == "installed"
    assert card["limits"]["unavailable_here"] == {"qlora": "needs CUDA"}
    assert {"id": "mlx", "status": "pending_decision"} in card["backends"]


def test_repo_id_base_is_refused_with_plain_error(engine, spec):
    _, base = engine()
    spec["base"]["repo"] = "HuggingFaceTB/SmolLM2-135M"
    status, body = call(base, "POST", "/v1/jobs", spec)
    assert status == 422 and body["error"] == "invalid_spec"
    assert "never downloads weights" in body["message"]


def test_bad_json_and_unknown_job(engine):
    _, base = engine()
    req = urllib.request.Request(base + "/v1/jobs", data=b"{nope", method="POST")
    req.add_header("Authorization", f"Bearer {TOKEN}")
    with pytest.raises(urllib.error.HTTPError) as info:
        urllib.request.urlopen(req, timeout=5)
    assert info.value.code == 422
    assert call(base, "GET", "/v1/jobs/job_missing")[0] == 404
    assert call(base, "POST", "/v1/jobs/job_missing/cancel")[0] == 404


def test_event_sequence_of_a_successful_job(engine, spec):
    _, base = engine("ok")
    status, job = call(base, "POST", "/v1/jobs", spec)
    assert status == 201 and job["state"] == "queued"
    done = wait_state(base, job["id"], {"succeeded", "failed"})
    assert done["state"] == "succeeded", done
    assert done["outputs"][0]["kind"] == "adapter"
    assert done["outputs"][0]["bytes"] == 10

    page = call(base, "GET", f"/v1/jobs/{job['id']}/events?after=0")[1]
    events = page["events"]
    assert [e["seq"] for e in events] == list(range(1, len(events) + 1))
    states = [e["message"] for e in events if e["kind"] == "state"]
    assert states == ["queued", "starting the worker", "training lora", "exporting", "finished"]
    kinds = [e["kind"] for e in events]
    assert kinds.index("progress") < kinds.index("output") < len(kinds) - 1
    assert [e["progress"] for e in events if e["kind"] == "progress"] == [0.25, 0.5, 0.75, 1.0]
    assert any(e["kind"] == "log" and "plain library output" in e["message"] for e in events)
    assert done["last_seq"] == len(events)

    # `after` returns only newer events.
    tail = call(base, "GET", f"/v1/jobs/{job['id']}/events?after={len(events) - 2}")[1]
    assert [e["seq"] for e in tail["events"]] == [len(events) - 1, len(events)]


def test_state_order_is_forward_only(engine, spec):
    manager, base = engine("ok")
    job = call(base, "POST", "/v1/jobs", spec)[1]
    wait_state(base, job["id"], {"succeeded"})
    job_obj = manager.get(job["id"])
    # A late or backwards state change from the worker is ignored, not applied.
    job_obj.set_state("running")
    assert job_obj.state == "succeeded"
    assert len([e for e in job_obj.events if e["kind"] == "state"]) == 5


def test_worker_failure_marks_job_failed_with_its_error(engine, spec):
    _, base = engine("fail")
    job = call(base, "POST", "/v1/jobs", spec)[1]
    done = wait_state(base, job["id"], {"succeeded", "failed"})
    assert done["state"] == "failed"
    assert done["error"] == "RuntimeError: out of memory"


def test_one_job_at_a_time_and_cancel(engine, spec):
    manager, base = engine("slow")
    job = call(base, "POST", "/v1/jobs", spec)[1]
    wait_state(base, job["id"], {"running"})
    status, body = call(base, "POST", "/v1/jobs", spec)
    assert status == 409 and body["error"] == "job_running"

    status, cancelled = call(base, "POST", f"/v1/jobs/{job['id']}/cancel")
    assert status == 200 and cancelled["state"] == "cancelled"
    proc = manager.get(job["id"]).process
    assert proc.poll() is not None, "worker process still alive after cancel"

    status, body = call(base, "POST", f"/v1/jobs/{job['id']}/cancel")
    assert status == 409 and body["error"] == "already_finished"
    # No events recorded after the cancel.
    time.sleep(0.6)
    events = call(base, "GET", f"/v1/jobs/{job['id']}/events")[1]
    assert events["state"] == "cancelled"
    assert events["events"][-1]["message"] == "cancelled by request"

    # The engine takes a new job once the old one is finished.
    assert call(base, "POST", "/v1/jobs", spec)[0] == 201
