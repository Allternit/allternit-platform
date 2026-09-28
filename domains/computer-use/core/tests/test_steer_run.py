"""Steering a running planning loop (ACI plan P2): the user's note reaches the
next step's task text, and the route only accepts it while the run is live."""
import asyncio
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

import computer_use_router as router_module  # noqa: E402
from core.planning_loop import PlanningLoop  # noqa: E402
from fastapi import HTTPException  # noqa: E402


def _loop():
    events = []
    loop = PlanningLoop(vision_provider=object(), adapter=object(), event_callback=events.append)
    return loop, events


def test_steering_is_folded_into_the_next_step_and_kept():
    loop, events = _loop()
    assert loop.steer("  use the second result ")
    task = loop._apply_steering("Find venues", "cu-1", 3)
    assert "Find venues" in task
    assert "- use the second result" in task
    assert any(e.get("type") == "run.steered" and e.get("text") == "use the second result" for e in events)
    # Consumed once; the task text keeps it for later steps.
    assert loop._apply_steering(task, "cu-1", 4) == task


def test_empty_or_cancelled_steering_is_refused():
    loop, _ = _loop()
    assert not loop.steer("   ")
    loop.cancel()
    assert not loop.steer("go back")


def test_steer_route_reaches_the_live_loop_only():
    async def scenario():
        store = router_module._run_store
        state = store.create("cu-steer-1", "sess-1", "intent", "browser")
        with pytest.raises(HTTPException) as not_running:
            await router_module.steer_run("cu-steer-1", router_module.SteerBody(text="go back"))
        assert not_running.value.status_code == 409

        loop, _ = _loop()
        state.planning_loop = loop
        state.status = "running"
        result = await router_module.steer_run("cu-steer-1", router_module.SteerBody(text="go back"))
        assert result == {"run_id": "cu-steer-1", "accepted": True}
        assert loop._steer_notes == ["go back"]

        with pytest.raises(HTTPException) as missing:
            await router_module.steer_run("cu-nope", router_module.SteerBody(text="x"))
        assert missing.value.status_code == 404

    asyncio.run(scenario())
