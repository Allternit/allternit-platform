"""`observe` routing through the gateway's executor waterfall.

Regression: `observe` (a canonical base_adapter.ActionType) was missing from
ALL_SUPPORTED_ACTIONS, so ComputerUseExecutor.execute rejected it with
UNSUPPORTED_ACTION *before* the adapter waterfall ran — every adapter
answered "no adapter available for direct execution" (adapter_id "none"),
even though browser.cdp implements observe natively.

These tests pin the contract: observe is accepted, falls through adapters
that declare it unsupported, and stays claimable by desktop adapters (it is
not a browser-only action).
"""

import sys
from pathlib import Path

import pytest

DOMAIN_CORE_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(DOMAIN_CORE_ROOT / "gateway"))
sys.path.insert(0, str(DOMAIN_CORE_ROOT))

_existing_core = sys.modules.get("core")
if _existing_core is not None:
    _existing_file = getattr(_existing_core, "__file__", "") or ""
    if Path(_existing_file).parent != DOMAIN_CORE_ROOT / "core":
        for _name in [
            m for m in list(sys.modules)
            if (m == "core" or m.startswith("core."))
            and not m.startswith(("core.tests", "core.gateway"))
        ]:
            del sys.modules[_name]

from core.base_adapter import ActionRequest, ResultEnvelope  # noqa: E402
from core.computer_use_executor import (  # noqa: E402
    ALL_SUPPORTED_ACTIONS,
    ComputerUseExecutor,
)


class _StubAdapter:
    """Minimal adapter: claims the configured actions, unsupported otherwise."""

    def __init__(self, adapter_id: str, supported: set):
        self.adapter_id = adapter_id
        self._supported = supported
        self.calls: list[str] = []

    async def health_check(self) -> bool:
        return True

    async def capabilities(self):
        return None

    async def execute(self, action: ActionRequest, session_id: str, run_id: str) -> ResultEnvelope:
        self.calls.append(action.action_type)
        env = ResultEnvelope(
            run_id=run_id, session_id=session_id, adapter_id=self.adapter_id,
            family="browser" if self.adapter_id.startswith("browser.") else "desktop",
            mode="execute", action=action.action_type, target=action.target,
        )
        if action.action_type not in self._supported:
            env.status = "unsupported"
            env.error = {"code": "UNSUPPORTED", "message": f"{self.adapter_id} cannot {action.action_type}"}
            return env
        env.status = "completed"
        env.extracted_content = {"observed_by": self.adapter_id}
        return env


def _observe_request() -> ActionRequest:
    return ActionRequest(action_type="observe", target="")


@pytest.mark.asyncio
async def test_observe_is_accepted_and_routers_to_a_real_adapter():
    assert "observe" in ALL_SUPPORTED_ACTIONS, "observe must pass the executor's acceptance gate"
    ext = _StubAdapter("browser.extension", supported=set())      # no observe -> waterfall continues
    cdp = _StubAdapter("browser.cdp", supported={"observe"})
    exe = ComputerUseExecutor()
    exe.register("browser.extension", ext)
    exe.register("browser.cdp", cdp)

    result = await exe.execute(_observe_request(), session_id="s-1", run_id="r-1")

    assert result.status == "completed", f"expected a real adapter to claim observe: {result.error}"
    assert result.adapter_id == "browser.cdp", "the claiming adapter must be named, not 'none'"
    assert result.error is None
    assert ext.calls == ["observe"], "extension saw the action first and declared it unsupported"
    assert cdp.calls == ["observe"]


@pytest.mark.asyncio
async def test_observe_is_not_browser_only_desktop_adapters_may_claim_it():
    desktop = _StubAdapter("desktop.accessibility", supported={"observe"})
    exe = ComputerUseExecutor()
    exe.register("desktop.accessibility", desktop)

    result = await exe.execute(_observe_request(), session_id="s-1", run_id="r-1")

    assert result.status == "completed"
    assert result.adapter_id == "desktop.accessibility"
    assert desktop.calls == ["observe"]


@pytest.mark.asyncio
async def test_screenshot_still_routes_and_unknown_actions_still_fail_closed():
    cdp = _StubAdapter("browser.cdp", supported={"screenshot", "observe"})
    exe = ComputerUseExecutor()
    exe.register("browser.cdp", cdp)

    shot = await exe.execute(ActionRequest(action_type="screenshot"), session_id="s-1", run_id="r-1")
    assert (shot.status, shot.adapter_id) == ("completed", "browser.cdp")

    bad = await exe.execute(ActionRequest(action_type="launch_missiles"), session_id="s-1", run_id="r-1")
    assert bad.status == "failed"
    assert bad.error and bad.error["code"] == "UNSUPPORTED_ACTION"
    assert bad.adapter_id == "none"
    assert cdp.calls == ["screenshot"], "rejected actions never reach the adapters"
