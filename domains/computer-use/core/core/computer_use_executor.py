"""
Allternit Computer Use — Unified Action Executor

Single dispatch layer between the gateway and all adapters.
Accepts Claude's 9 native computer-use action types plus browser extensions,
resolves the best available adapter via the waterfall, and returns a
normalized ResultEnvelope.

Adapter waterfall (highest → lowest priority):
  1. browser.extension  — BROWSER.* actions via HTTP relay :3012 → Desktop → TCP 3011 → extension
  2. browser.cdp        — Chrome/Electron CDP connection (headless or headed)
  3. browser.playwright — directly registered physical Playwright adapter
  4. desktop.accessibility — native desktop (Quartz events + accessibility tree)

Claude's native computer tool calls this executor directly.
Non-Claude models use PlanningLoop (planning_loop.py) which also calls this executor.
"""

from __future__ import annotations

import asyncio
import dataclasses
import logging
import time
from datetime import datetime, timezone
from typing import Any, Dict, List, Optional, Tuple, Type

from .base_adapter import (
    ActionRequest,
    AdapterCapabilities,
    BaseAdapter,
    ResultEnvelope,
)

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Claude's 9 native computer-use action types + browser extensions
# ---------------------------------------------------------------------------

NATIVE_CLAUDE_ACTIONS = frozenset({
    "screenshot",
    "left_click",
    "right_click",
    "middle_click",
    "double_click",
    "left_click_drag",
    "type",
    "key",
    "scroll",
    "cursor_position",
})

BROWSER_EXTENSION_ACTIONS = frozenset({
    "navigate",
    "extract",
    "fill",
    "wait",
    "tabs",
})

# Canonical inspect actions from base_adapter.ActionType that adapters
# implement natively (browser.cdp observes the DOM: url/title/text/counts).
# Not browser-extension actions: desktop adapters stay waterfall candidates
# for them. Without this set the executor rejects `observe` as
# UNSUPPORTED_ACTION before the waterfall ever runs.
CANONICAL_INSPECT_ACTIONS = frozenset({
    "observe",
})

ALL_SUPPORTED_ACTIONS = NATIVE_CLAUDE_ACTIONS | BROWSER_EXTENSION_ACTIONS | CANONICAL_INSPECT_ACTIONS

# Plan-loop / grounding-model vocabulary → native executor vocabulary
# (cu22 follow-up F2). Frontier models emit plan types like `click`, `select`,
# `press`, or camelCase `doubleClick`; the adapters speak the native set
# above. Mapping here (rather than at every call site) keeps the per-step
# fallback path aligned with what batch dispatch accepts — the campaign had
# to shim click→left_click by hand because the executor rejected the plan
# types outright. Types absent from both sets still refuse cleanly with
# UNSUPPORTED_ACTION.
PLAN_ACTION_MAP: Dict[str, str] = {
    "click": "left_click",
    "doubleClick": "double_click",
    "press": "key",
    "select": "fill",
    "scrollTo": "scroll",
    # hover passes through as-is: adapters that support it
    # (desktop.accessibility) handle it; browser adapters report unsupported
    # and the waterfall continues, same as right_click et al.
    "hover": "hover",
}

# ---------------------------------------------------------------------------
# Adapter waterfall — ordered from best to fallback
# ---------------------------------------------------------------------------

ADAPTER_WATERFALL: List[str] = [
    "browser.extension",   # BROWSER.* actions via HTTP relay :3012 → Desktop → TCP 3011 → extension
    "browser.cdp",         # Chrome/Electron CDP
    "browser.playwright",  # Playwright headless
    "desktop.accessibility",
]


# ---------------------------------------------------------------------------
# ComputerUseExecutor
# ---------------------------------------------------------------------------

class ComputerUseExecutor:
    """
    Unified executor — resolves adapter via waterfall and dispatches actions.

    Usage:
        executor = ComputerUseExecutor()
        executor.register("browser.extension", extension_adapter_instance)
        result = await executor.execute(action_request, session_id="s-1", run_id="r-1")
    """

    def __init__(self) -> None:
        self._adapters: Dict[str, BaseAdapter] = {}
        self._capabilities_cache: Dict[str, AdapterCapabilities] = {}

    # ── Registration ──────────────────────────────────────────────────────────

    def register(self, adapter_id: str, adapter: BaseAdapter) -> None:
        """Register a live adapter instance."""
        self._adapters[adapter_id] = adapter
        self._capabilities_cache.pop(adapter_id, None)
        logger.info("[executor] registered adapter %s", adapter_id)

    def unregister(self, adapter_id: str) -> None:
        self._adapters.pop(adapter_id, None)
        self._capabilities_cache.pop(adapter_id, None)

    def registered_adapters(self) -> List[str]:
        return list(self._adapters.keys())

    # ── Capabilities ──────────────────────────────────────────────────────────

    async def get_capabilities(self, adapter_id: str) -> Optional[AdapterCapabilities]:
        if adapter_id not in self._adapters:
            return None
        if adapter_id not in self._capabilities_cache:
            try:
                caps = await self._adapters[adapter_id].capabilities()
                self._capabilities_cache[adapter_id] = caps
            except Exception as exc:
                logger.warning("[executor] capabilities() failed for %s: %s", adapter_id, exc)
                return None
        return self._capabilities_cache[adapter_id]

    # ── Dispatch ──────────────────────────────────────────────────────────────

    async def execute(
        self,
        action: ActionRequest,
        session_id: str,
        run_id: str,
        adapter_preference: Optional[str] = None,
    ) -> ResultEnvelope:
        """
        Dispatch action to the best available adapter.

        adapter_preference pins a specific adapter (skips waterfall).
        Falls through the waterfall on health-check failure or unavailability.
        """
        # F2: translate plan vocabulary into the native adapter vocabulary
        # before the whitelist check (e.g. the model's `click` plan becomes
        # the adapters' `left_click`). The envelope reports the action that
        # actually executed.
        #
        # The integration caller (planning_loop._execute_action) builds a
        # plain ActionRequest-like object, not a dataclass, so only use
        # dataclasses.replace on real dataclasses; copy anything else
        # (shallow copy — the caller may still hold the original for its
        # step record).
        native_type = PLAN_ACTION_MAP.get(action.action_type, action.action_type)
        if native_type != action.action_type:
            if dataclasses.is_dataclass(action):
                action = dataclasses.replace(action, action_type=native_type)
            else:
                import copy
                action = copy.copy(action)
                action.action_type = native_type

        # Plan-vocabulary members whose mapping is the identity (e.g. hover)
        # are accepted as-is; adapters that can't handle them report
        # unsupported and the waterfall continues.
        accepted = (
            action.action_type in ALL_SUPPORTED_ACTIONS
            or action.action_type in PLAN_ACTION_MAP
        )
        if not accepted:
            return self._error_envelope(
                action, session_id, run_id,
                f"Unsupported action type: {action.action_type!r}",
                "UNSUPPORTED_ACTION",
            )

        candidates = self._resolve_waterfall(action, adapter_preference)

        if not candidates:
            return self._error_envelope(
                action, session_id, run_id,
                "No adapter available for this action",
                "NO_ADAPTER",
            )

        last_error: Optional[str] = None
        fallbacks_used: List[str] = []

        for adapter_id in candidates:
            adapter = self._adapters.get(adapter_id)
            if adapter is None:
                continue

            try:
                healthy = await adapter.health_check()
                if not healthy:
                    logger.warning("[executor] %s failed health check, trying next", adapter_id)
                    fallbacks_used.append(adapter_id)
                    continue
            except Exception as exc:
                logger.warning("[executor] health check error for %s: %s", adapter_id, exc)
                fallbacks_used.append(adapter_id)
                continue

            started = time.monotonic()
            try:
                result = await adapter.execute(action, session_id, run_id)
                elapsed_ms = int((time.monotonic() - started) * 1000)
                if result.duration_ms is None:
                    result.duration_ms = elapsed_ms

                if result.status == "unsupported":
                    # Adapter explicitly declared it cannot handle this action.
                    # Fall through to the next adapter in the waterfall.
                    logger.info(
                        "[executor] %s unsupported by %s, trying next",
                        action.action_type, adapter_id,
                    )
                    fallbacks_used.append(adapter_id)
                    last_error = result.error.get("message") if result.error else "unsupported"
                    continue

                result.fallbacks_used = fallbacks_used
                logger.info(
                    "[executor] %s → %s in %dms via %s",
                    action.action_type, result.status, elapsed_ms, adapter_id,
                )
                return result
            except Exception as exc:
                last_error = str(exc)
                logger.warning("[executor] %s failed on %s: %s", action.action_type, adapter_id, exc)
                fallbacks_used.append(adapter_id)
                continue

        return self._error_envelope(
            action, session_id, run_id,
            last_error or "All adapters failed",
            "ADAPTER_FAILURE",
            fallbacks_used=fallbacks_used,
        )

    # ── Batch dispatch ────────────────────────────────────────────────────────

    async def execute_batch(
        self,
        actions: List[ActionRequest],
        session_id: str,
        run_id: str,
        adapter_preference: Optional[str] = None,
        sequential: bool = True,
    ) -> List[ResultEnvelope]:
        """Execute a list of actions, sequentially (default) or in parallel."""
        if sequential:
            results = []
            for action in actions:
                result = await self.execute(action, session_id, run_id, adapter_preference)
                results.append(result)
                if result.status == "failed":
                    break
            return results
        else:
            return list(await asyncio.gather(
                *[self.execute(a, session_id, run_id, adapter_preference) for a in actions]
            ))

    # ── Private ───────────────────────────────────────────────────────────────

    def _resolve_waterfall(
        self,
        action: ActionRequest,
        adapter_preference: Optional[str],
    ) -> List[str]:
        """Build ordered candidate list for this action."""
        # Pinned preference first
        if adapter_preference and adapter_preference in self._adapters:
            rest = [a for a in ADAPTER_WATERFALL if a != adapter_preference and a in self._adapters]
            return [adapter_preference] + rest

        # Browser-extension actions only make sense on browser adapters
        browser_only = action.action_type in BROWSER_EXTENSION_ACTIONS
        candidates = []
        for adapter_id in ADAPTER_WATERFALL:
            if adapter_id not in self._adapters:
                continue
            if browser_only and not adapter_id.startswith("browser."):
                continue
            candidates.append(adapter_id)

        return candidates

    def _error_envelope(
        self,
        action: ActionRequest,
        session_id: str,
        run_id: str,
        message: str,
        code: str,
        fallbacks_used: Optional[List[str]] = None,
    ) -> ResultEnvelope:
        return ResultEnvelope(
            run_id=run_id,
            session_id=session_id,
            adapter_id="none",
            family="browser",
            mode="execute",
            action=action.action_type,
            target=action.target,
            status="failed",
            started_at=datetime.now(timezone.utc).isoformat(),
            completed_at=datetime.now(timezone.utc).isoformat(),
            error={"code": code, "message": message},
            fallbacks_used=fallbacks_used or [],
        )


# ---------------------------------------------------------------------------
# Module-level singleton (lazy-initialized)
# ---------------------------------------------------------------------------

_executor: Optional[ComputerUseExecutor] = None


def get_executor() -> ComputerUseExecutor:
    global _executor
    if _executor is None:
        _executor = ComputerUseExecutor()
    return _executor
