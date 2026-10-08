"""Anthropic SDK toolset drivers backed by the Allternit executor.

``ExecutorComputerToolset`` / ``ExecutorBrowserToolset`` subclass the SDK's
``BetaAsyncAbstractComputerToolset20260801`` / ``BetaAsyncAbstractBrowserToolset20260801``
(``anthropic.tools.computer`` / ``anthropic.tools.browser``, anthropic>=1.12).
Every member goes to allternit-api ``POST /api/v1/computers/:id/toolset``
through one ``execute`` override, so the SDK's tool runner, the engine's Claude
path and outside callers all share the executor's validation, scaling,
approval, audit and dispatch.

Approval: the executor decides which calls need a person (409
``approval_required`` with a single-use grant). The SDK ``confirm`` callback is
the person: on a 409 the driver calls it with the SDK's confirm context, then
approves the grant and retries once. The SDK's own pre-call confirm is wired
to an always-yes gate, because the executor (not the SDK's member list) is the
authority on what needs approval for this target.

The engine's Claude planning path uses ``to_dict()`` of these classes as the
``tools[]`` entry, so the model sees the native toolset with no translation.
"""

from __future__ import annotations

from typing import Any, Awaitable, Callable, Dict, Optional, Union

from .toolset_executor import ToolsetExecutorClient, ToolsetReply

try:  # anthropic>=1.12
    from anthropic.tools.computer import (
        BetaAsyncAbstractComputerToolset20260801,
        BetaComputerCursorPositionResult,
        BetaScreenshotResult,
    )
    from anthropic.tools.browser import (
        BetaAsyncAbstractBrowserToolset20260801,
        BetaBrowserNavigateResult,
        BetaBrowserState,
    )
    from anthropic.tools import ToolError
    SDK_TOOLSETS_AVAILABLE = True
except Exception:  # pragma: no cover - older SDK or none installed
    SDK_TOOLSETS_AVAILABLE = False

Confirm = Callable[[Any], Union[bool, Awaitable[bool]]]

# The gateway browser backend doesn't serve tabs, console, network, JS or uploads
# yet (the executor reports them unsupported); keep them off on the wire.
_BROWSER_OFF = ("file_upload", "read_console", "read_network", "javascript_exec",
                "new_tab", "list_tabs", "switch_tab", "close_tab")


def _input_dict(parsed: Any) -> Dict[str, Any]:
    if hasattr(parsed, "model_dump"):
        return parsed.model_dump(exclude_none=True, by_alias=True)
    return dict(parsed or {})


async def _call(client: ToolsetExecutorClient, toolset: str, member: str, context: Any, parsed: Any,
                confirm: Optional[Confirm], frame: Optional[Dict[str, int]]) -> ToolsetReply:
    async def ask() -> bool:
        if confirm is None:
            return False
        decision = confirm(context)
        if hasattr(decision, "__await__"):
            decision = await decision
        return bool(decision)

    run_id = getattr(context, "run_id", None)
    call_index = int(getattr(context, "call_index", 0) or 0)
    reply = await client.run_with_approval(
        toolset, member, _input_dict(parsed), ask,
        run_id=run_id, turn_id=getattr(context, "turn_id", None), call_index=call_index, model_frame=frame,
    )
    if not reply.ok:
        raise ToolError(reply.text() or f"{member} failed")
    return reply


if SDK_TOOLSETS_AVAILABLE:

    async def _always(_context: Any) -> bool:
        return True

    class ExecutorComputerToolset(BetaAsyncAbstractComputerToolset20260801):
        """computer_toolset_20260801, every member on the Allternit executor."""

        def __init__(self, client: ToolsetExecutorClient, *, confirm: Optional[Confirm] = None,
                     model_frame: Optional[Dict[str, int]] = None, **kw: Any) -> None:
            super().__init__(confirm=_always, **kw)
            self._client = client
            self._confirm = confirm
            self._frame = model_frame

        async def execute(self, context, name, input):  # type: ignore[override]
            reply = await _call(self._client, "computer", name, context, input, self._confirm, self._frame)
            image = reply.image_b64()
            if image:
                return BetaScreenshotResult(data=image, media_type="image/png")
            if name == "cursor_position":
                text = reply.text()
                nums = [int(float(t)) for t in text.replace(",", " ").replace("(", " ").replace(")", " ").split()
                        if t.lstrip("-").replace(".", "", 1).isdigit()]
                if len(nums) >= 2:
                    return BetaComputerCursorPositionResult(x=nums[0], y=nums[1])
            return reply.text() or None

    class ExecutorBrowserToolset(BetaAsyncAbstractBrowserToolset20260801):
        """browser_toolset_20260801 on a gateway browser session via the executor."""

        def __init__(self, client: ToolsetExecutorClient, *, confirm: Optional[Confirm] = None,
                     configs: Optional[Dict[str, Any]] = None, **kw: Any) -> None:
            wire = {name: {"enabled": False} for name in _BROWSER_OFF}
            wire.update(configs or {})
            super().__init__(configs=wire, confirm=_always, **kw)
            self._client = client
            self._confirm = confirm
            self._last_state: Dict[str, Any] = {}

        async def _browser_state(self, context):  # type: ignore[override]
            state = self._last_state
            return BetaBrowserState(tabs=[{
                "tab_id": str(state.get("tab_id") or "main"),
                "url": str(state.get("url") or ""),
                "title": str(state.get("title") or ""),
                "active": True,
            }])

        async def execute(self, context, name, input):  # type: ignore[override]
            reply = await _call(self._client, "browser", name, context, input, self._confirm, None)
            if isinstance(reply.body.get("browser_state"), dict):
                self._last_state = reply.body["browser_state"]
            image = reply.image_b64()
            if image:
                return BetaScreenshotResult(data=image, media_type="image/png")
            if name == "navigate":
                state = reply.body.get("browser_state") or {}
                return BetaBrowserNavigateResult(url=str(state.get("url") or _input_dict(input).get("url", "")))
            return reply.text() or None
