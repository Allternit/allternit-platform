"""Canonical native provider backed by an installed Cua Driver."""

from __future__ import annotations

import base64
import hashlib
import json
import platform
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, Iterable, Optional, Tuple


class CuaDriverHistoryError(RuntimeError):
    """History-specific error from the CUA Driver provider."""
from uuid import uuid4

from contracts.canonical import (
    ActionEvidence,
    ActionStep,
    ActionTransaction,
    CapabilityManifest,
    ElementNode,
    ExecutionMode,
    ImageEvidence,
    Observation,
    OutcomeStatus,
    Rect,
    Root,
    StepOutcome,
)
from providers.cua_driver_transport import CuaDriverCallError, CuaDriverTransport


def cua_resource_id(pid: int, window_id: int) -> str:
    return f"cua:{pid}:{window_id}"


def parse_cua_resource_id(resource_id: str) -> Tuple[int, int]:
    parts = resource_id.split(":")
    if len(parts) != 3 or parts[0] != "cua":
        raise ValueError("Cua resource IDs must use cua:<pid>:<window_id>")
    return int(parts[1]), int(parts[2])


def _structured(response: Dict[str, Any]) -> Dict[str, Any]:
    for key in ("structuredContent", "structured_content", "result"):
        value = response.get(key)
        if isinstance(value, dict):
            nested = value.get("structuredContent") or value.get("structured_content")
            return nested if isinstance(nested, dict) else value
    for item in _content(response):
        if item.get("type") != "text" or not isinstance(item.get("text"), str):
            continue
        try:
            value = json.loads(item["text"])
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict):
            return value
    return response


def _content(response: Dict[str, Any]) -> Iterable[Dict[str, Any]]:
    value = response.get("content")
    if isinstance(value, list):
        return (item for item in value if isinstance(item, dict))
    result = response.get("result")
    if isinstance(result, dict) and isinstance(result.get("content"), list):
        return (item for item in result["content"] if isinstance(item, dict))
    return ()


def _rect(value: Any) -> Optional[Rect]:
    if not isinstance(value, dict):
        return None
    width = value.get("width", value.get("w"))
    height = value.get("height", value.get("h"))
    if width is None or height is None:
        return None
    return Rect(
        float(value.get("x", 0)),
        float(value.get("y", 0)),
        float(width),
        float(height),
    )


class CuaDriverCanonicalProvider:
    provider_id = "desktop.cua-driver"

    def __init__(
        self,
        transport: CuaDriverTransport,
        artifact_dir: str | Path,
        *,
        version: str = "unknown",
    ) -> None:
        self._transport = transport
        self._artifact_dir = Path(artifact_dir)
        self._version = version
        self._state_tokens: Dict[str, Dict[str, Dict[str, Any]]] = {}
        self._state_order: list[str] = []
        self._history_tools: Optional[Tuple[str, ...]] = None

    async def _detect_history_tools(self) -> Tuple[str, ...]:
        """Probe CUA Driver for Computer History support; cache the result.

        History tools are advertised only when the driver reports the preview is
        supported and admitted. Errors or unsupported platforms degrade to an
        empty tool set so the provider remains usable without history.
        """
        try:
            status = await self._transport.history_status()
        except CuaDriverCallError:
            return ()
        except Exception:
            return ()
        if not isinstance(status, dict):
            return ()
        if status.get("supported") is True and status.get("admitted") is True:
            return ("history_status", "history_query")
        return ()

    async def capabilities(self) -> CapabilityManifest:
        system = platform.system().lower()
        os_name = "macos" if system == "darwin" else system
        # The upstream driver mixes window-scoped semantic routes with raw,
        # potentially global input routes. Until capability negotiation is
        # action-granular, advertise only the guarantee common to every action.
        strict = False
        modes = [ExecutionMode.FOREGROUND_ALLOWED.value]
        if self._history_tools is None:
            self._history_tools = await self._detect_history_tools()
        return CapabilityManifest(
            provider_id=self.provider_id,
            provider_version=self._version,
            operating_systems=(os_name,) if os_name in ("macos", "windows", "linux") else (),
            actions=(
                "click", "doubleClick", "rightClick", "typeText", "setText",
                "keypress", "hotkey", "scroll", "drag", "moveMouse",
                "launchApp", "closeApp",
            ),
            observation_channels=("accessibility", "screenshot"),
            execution_modes=tuple(modes),
            strict_background=strict,
            semantic_input=True,
            raw_input=True,
            streaming=False,
            clipboard=False,
            max_concurrency=8,
            limitations=(
                "installed_external_provider",
                "background_guarantee_requires_action_granular_upstream_manifest",
                "linux_background_raw_input_depends_on_display_route",
                "tool_results_require_successor_verification",
            ),
            tools=self._history_tools,
        )

    async def list_roots(self, pid: Optional[int] = None) -> Tuple[Root, ...]:
        response = await self._transport.call("list_windows", {"pid": pid} if pid is not None else {})
        structured = _structured(response)
        windows = structured.get("windows", structured.get("items", []))
        if not isinstance(windows, list):
            windows = []
        roots = []
        for window in windows:
            if not isinstance(window, dict):
                continue
            window_id = window.get("window_id", window.get("id", window.get("hwnd")))
            window_pid = window.get("pid", pid)
            if window_id is None or window_pid is None:
                continue
            resource_id = cua_resource_id(int(window_pid), int(window_id))
            roots.append(
                Root(
                    root_id=f"root_{resource_id}",
                    resource_id=resource_id,
                    kind=str(window.get("kind", "window")),
                    title=str(window.get("title", window.get("name", ""))),
                    application=str(window.get("app_name", window.get("application", ""))),
                    process_id=int(window_pid),
                    bounds=_rect(window.get("bounds", window.get("frame"))),
                    focused=bool(window.get("focused", window.get("is_focused", False))),
                )
            )
        return tuple(roots)

    async def discover_roots(self, *, session_id: str, environment_id: str) -> Tuple[Root, ...]:
        return await self.list_roots()

    def _remember(self, state_id: str, values: Dict[str, Dict[str, Any]]) -> None:
        self._state_tokens[state_id] = values
        self._state_order.append(state_id)
        while len(self._state_order) > 128:
            self._state_tokens.pop(self._state_order.pop(0), None)

    async def observe(
        self,
        *,
        session_id: str,
        environment_id: str,
        resource_id: str,
        epoch: int,
    ) -> Observation:
        pid, window_id = parse_cua_resource_id(resource_id)
        self._artifact_dir.mkdir(parents=True, exist_ok=True)
        artifact_id = f"artifact_{uuid4().hex}"
        artifact_path = self._artifact_dir / f"{artifact_id}.png"
        response = await self._transport.call(
            "get_window_state",
            {"pid": pid, "window_id": window_id, "session": session_id},
            screenshot_out_file=str(artifact_path),
        )
        structured = _structured(response)
        raw_elements = structured.get("elements", [])
        if not isinstance(raw_elements, list):
            raw_elements = []
        elements = []
        tokens: Dict[str, Dict[str, Any]] = {}
        for offset, item in enumerate(raw_elements, start=1):
            if not isinstance(item, dict):
                continue
            index = int(item.get("element_index", offset))
            ref = f"@e{offset}"
            token = item.get("element_token")
            tokens[ref] = {
                "element_index": index,
                "element_token": str(token) if token is not None else None,
                "pid": pid,
                "window_id": window_id,
            }
            elements.append(
                ElementNode(
                    ref=ref,
                    role=str(item.get("role", item.get("type", ""))),
                    name=str(item.get("name", item.get("label", item.get("title", "")))),
                    value=str(item.get("value", "")),
                    description=str(item.get("description", "")),
                    bounds=_rect(item.get("bounds", item.get("frame"))),
                    states=tuple(str(value) for value in item.get("states", ())),
                    actions=tuple(str(value) for value in item.get("actions", ())),
                    provider_metadata={
                        "element_index": index,
                        "element_token": token,
                    },
                )
            )

        image = None
        if artifact_path.is_file():
            payload = artifact_path.read_bytes()
            if payload:
                width = int(structured.get("screenshot_width", structured.get("width", 1)))
                height = int(structured.get("screenshot_height", structured.get("height", 1)))
                image = ImageEvidence(
                    artifact_id=artifact_id,
                    media_type="image/png",
                    width=max(1, width),
                    height=max(1, height),
                    sha256=hashlib.sha256(payload).hexdigest(),
                    coordinate_space="window_pixels",
                )
        if image is None:
            for item in _content(response):
                if item.get("type") != "image" or not isinstance(item.get("data"), str):
                    continue
                payload = base64.b64decode(item["data"])
                artifact_path.write_bytes(payload)
                image = ImageEvidence(
                    artifact_id=artifact_id,
                    media_type=str(item.get("mimeType", "image/png")),
                    width=max(1, int(structured.get("screenshot_width", 1))),
                    height=max(1, int(structured.get("screenshot_height", 1))),
                    sha256=hashlib.sha256(payload).hexdigest(),
                    coordinate_space="window_pixels",
                )
                break

        roots = await self.list_roots(pid)
        root = next((candidate for candidate in roots if candidate.resource_id == resource_id), None)
        if root is None:
            root = Root(
                root_id=f"root_{resource_id}",
                resource_id=resource_id,
                kind="window",
                process_id=pid,
            )
        state_id = f"state_{uuid4().hex}"
        self._remember(state_id, tokens)
        return Observation(
            state_id=state_id,
            session_id=session_id,
            environment_id=environment_id,
            resource_id=resource_id,
            epoch=epoch,
            captured_at=datetime.now(timezone.utc).isoformat(),
            provider_id=self.provider_id,
            provider_version=self._version,
            roots=(root,),
            elements=tuple(elements),
            image=image,
            truncated=bool(structured.get("truncated", False)),
            metadata={
                "pid": pid,
                "window_id": window_id,
                "tree_markdown": structured.get("tree_markdown"),
                "degraded": bool(structured.get("degraded", False)),
                "artifact_path": str(artifact_path) if image else None,
            },
        )

    def _arguments(self, transaction: ActionTransaction, step: ActionStep) -> Dict[str, Any]:
        pid, window_id = parse_cua_resource_id(transaction.resource_id)
        arguments: Dict[str, Any] = {
            "pid": pid,
            "window_id": window_id,
            "session": transaction.session_id,
            **step.arguments,
        }
        if step.target and step.target.ref:
            mapping = self._state_tokens.get(transaction.base_state_id)
            if mapping is None or step.target.ref not in mapping:
                raise ValueError(f"Cua element mapping for {step.target.ref!r} is unavailable")
            token = mapping[step.target.ref]
            arguments["element_index"] = token["element_index"]
            if token.get("element_token"):
                arguments["element_token"] = token["element_token"]
        elif step.target and step.target.x is not None and step.target.y is not None:
            arguments["x"] = step.target.x
            arguments["y"] = step.target.y
        return arguments

    async def execute_step(
        self,
        *,
        transaction: ActionTransaction,
        index: int,
        step: ActionStep,
    ) -> StepOutcome:
        tool_map = {
            "click": "click", "doubleClick": "double_click", "rightClick": "right_click",
            "typeText": "type_text", "setText": "set_value", "keypress": "press_key",
            "hotkey": "hotkey", "scroll": "scroll", "drag": "drag",
            "moveMouse": "move_cursor", "launchApp": "launch_app", "closeApp": "kill_app",
        }
        tool = tool_map[step.action]
        try:
            response = await self._transport.call(tool, self._arguments(transaction, step))
            structured = _structured(response)
            is_error = bool(response.get("isError", response.get("is_error", False)))
            effect = str(structured.get("effect", structured.get("outcome", "unverifiable")))
            if is_error or effect in ("failed", "didnt", "error"):
                status = OutcomeStatus.DIDNT.value
            elif effect in ("worked", "success", "verified"):
                status = OutcomeStatus.WORKED.value
            else:
                status = OutcomeStatus.UNKNOWN.value
            return StepOutcome(
                index=index,
                status=status,
                evidence=ActionEvidence(
                    grounding="cua_element_token" if step.target and step.target.ref else "cua_window_pixel",
                    delivery=str(structured.get("delivery", structured.get("route", "cua-driver"))),
                    details={
                        "tool": tool,
                        "effect": effect,
                        "structured": structured,
                    },
                ),
                error_code="cua_driver_error" if is_error else None,
                message=str(structured.get("message", "")) or None,
            )
        except CuaDriverCallError as error:
            return StepOutcome(
                index=index,
                status=OutcomeStatus.DIDNT.value,
                evidence=ActionEvidence(
                    grounding="cua_driver",
                    delivery="cua-driver-cli",
                    details={"tool": tool, "exit_code": error.exit_code},
                ),
                error_code="cua_driver_call_failed",
                message=str(error),
            )

    async def history_status(self) -> Dict[str, Any]:
        """Return CUA Driver Computer History operational status."""
        return await self._transport.history_status()

    async def history_query(
        self,
        *,
        limit: Optional[int] = None,
        session_id: Optional[str] = None,
        since_sequence: Optional[int] = None,
        until_sequence: Optional[int] = None,
    ) -> Dict[str, Any]:
        """Query a bounded, metadata-only slice of CUA Driver Computer History."""
        return await self._transport.history_query(
            limit=limit,
            session_id=session_id,
            since_sequence=since_sequence,
            until_sequence=until_sequence,
        )

    async def close(self) -> None:
        return None
