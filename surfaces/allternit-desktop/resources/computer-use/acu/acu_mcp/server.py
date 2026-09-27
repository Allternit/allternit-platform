"""
Allternit Computer Use — MCP Server

8 tools exposed over Model Context Protocol (Claude's 9 native actions + record).
Two transport modes:
  stdio  — for Claude Desktop integration (default)
  sse    — HTTP SSE endpoint at /mcp (port from ACU_MCP_PORT, default 8765)

Usage:
  python -m mcp.server                    # stdio
  python -m mcp.server stdio              # stdio
  python -m mcp.server sse               # SSE on ACU_MCP_PORT (default 8765)

Env vars:
  ACU_GATEWAY_URL   — gateway base URL (default http://localhost:8760)
  ACU_MCP_PORT      — SSE server port  (default 8765)
"""

from __future__ import annotations

import asyncio
import base64
import json
import sys
import uuid
from pathlib import Path
from typing import Any, Optional

import httpx
from mcp.server.fastmcp import FastMCP

# ---------------------------------------------------------------------------
# FastMCP app
# ---------------------------------------------------------------------------

import os as _os_init
mcp = FastMCP(
    name="allternit-computer-use",
    instructions=(
        "Allternit Computer Use Engine — automates browser and desktop actions. "
        "All tools accept a session_id that identifies an active engine session. "
        "Use screenshot to observe the current state before acting."
    ),
    port=int(_os_init.environ.get("ACU_MCP_PORT", "8765")),
)

# ---------------------------------------------------------------------------
# Gateway base URL — tools proxy to the gateway HTTP API
# ---------------------------------------------------------------------------

import os
GATEWAY_BASE = os.environ.get("ACU_GATEWAY_URL", "http://localhost:8760")
_ACU_API_KEY = os.environ.get("ACU_API_KEY", "")

# Stable session ID for the lifetime of this MCP server process.
# Claude can omit session_id and all tool calls will share this session,
# giving continuity (navigate then screenshot lands on the navigated page).
# Pass an explicit session_id to open an isolated parallel session.
_DEFAULT_SESSION_ID: str = os.environ.get("ACU_DEFAULT_SESSION", f"mcp-{uuid.uuid4().hex[:12]}")


def _gw(path: str) -> str:
    """Build a full gateway URL."""
    return f"{GATEWAY_BASE}{path}"


def _headers() -> dict:
    """Build request headers, including auth when ACU_API_KEY is set."""
    h = {"Content-Type": "application/json"}
    if _ACU_API_KEY:
        h["Authorization"] = f"Bearer {_ACU_API_KEY}"
    return h


async def _post(path: str, body: dict) -> dict:
    """POST to the gateway and return parsed JSON. Raises on HTTP error."""
    async with httpx.AsyncClient(timeout=60.0) as client:
        resp = await client.post(_gw(path), json=body, headers=_headers())
        resp.raise_for_status()
        return resp.json()


async def _get(path: str, params: dict | None = None) -> dict:
    """GET from the gateway and return parsed JSON."""
    async with httpx.AsyncClient(timeout=30.0) as client:
        resp = await client.get(_gw(path), params=params or {}, headers=_headers())
        resp.raise_for_status()
        return resp.json()


# ---------------------------------------------------------------------------
# In-process recording registry (for record_start / record_stop)
# ---------------------------------------------------------------------------

_active_recordings: dict[str, Any] = {}  # recording_id -> ActionRecorder


# ---------------------------------------------------------------------------
# 1. screenshot
# ---------------------------------------------------------------------------


@mcp.tool()
async def screenshot(
    full_page: bool = False,
    session_id: Optional[str] = None,
) -> dict:
    """
    Capture a screenshot of the current browser page.

    session_id: omit to use the shared MCP session (continuity across tool calls).
    Returns base64-encoded PNG, page URL, and viewport dimensions.
    """
    session_id = session_id or _DEFAULT_SESSION_ID
    run_id = str(uuid.uuid4())
    resp = await _post(
        "/v1/computer",
        {
            "action": "screenshot",
            "session_id": session_id,
            "run_id": run_id,
            "parameters": {"full_page": full_page},
        },
    )

    screenshot_b64 = ""
    width = 1280
    height = 720
    url = ""

    ec = resp.get("extracted_content") or {}
    if isinstance(ec, dict):
        data_uri = ec.get("data_url", "")
        if data_uri.startswith("data:"):
            screenshot_b64 = data_uri.split(",", 1)[-1]
        elif ec.get("content"):
            screenshot_b64 = ec["content"]
        url = ec.get("url", "")
        width = ec.get("width", 1280)
        height = ec.get("height", 720)

    # Also check artifacts for adapters that still use that path
    if not screenshot_b64:
        for artifact in resp.get("artifacts", []):
            if artifact.get("type") == "screenshot":
                raw = artifact.get("content") or ""
                if not raw:
                    data_uri = artifact.get("url", "")
                    if data_uri.startswith("data:"):
                        raw = data_uri.split(",", 1)[-1]
                if raw:
                    screenshot_b64 = raw
                    break

    return {
        "screenshot": screenshot_b64,
        "url": url,
        "width": width,
        "height": height,
    }


# ---------------------------------------------------------------------------
# 2. click
# ---------------------------------------------------------------------------


@mcp.tool()
async def click(
    x: Optional[int] = None,
    y: Optional[int] = None,
    selector: Optional[str] = None,
    session_id: Optional[str] = None,
) -> dict:
    """
    Click at coordinates (x, y) or on an element matched by CSS selector.

    Provide either (x, y) for pixel-coordinate clicking, or selector for element
    clicking. If both are given, coordinates take priority.
    session_id: omit to use the shared MCP session.
    """
    session_id = session_id or _DEFAULT_SESSION_ID
    run_id = str(uuid.uuid4())
    body: dict[str, Any] = {
        "action": "left_click",
        "session_id": session_id,
        "run_id": run_id,
    }
    if x is not None and y is not None:
        body["coordinate"] = [x, y]
    if selector:
        body["selector"] = selector

    resp = await _post("/v1/computer", body)

    success = resp.get("status") in ("completed",)
    element: Optional[str] = None
    if resp.get("extracted_content") and isinstance(resp["extracted_content"], dict):
        element = resp["extracted_content"].get("element")

    return {"success": success, "element": element}


# ---------------------------------------------------------------------------
# 3. type
# ---------------------------------------------------------------------------


@mcp.tool()
async def type(
    text: str,
    selector: Optional[str] = None,
    session_id: Optional[str] = None,
) -> dict:
    """
    Type text into the currently focused element, or into selector if provided.
    session_id: omit to use the shared MCP session.
    """
    session_id = session_id or _DEFAULT_SESSION_ID
    run_id = str(uuid.uuid4())
    # With selector: use fill (waits for element). Without: keyboard type into focused element.
    action = "fill" if selector else "type"
    body: dict[str, Any] = {
        "action": action,
        "session_id": session_id,
        "run_id": run_id,
        "text": text,
    }
    if selector:
        body["selector"] = selector

    resp = await _post("/v1/computer", body)

    success = resp.get("status") in ("completed",)
    return {"success": success, "chars_typed": len(text) if success else 0}


# ---------------------------------------------------------------------------
# 4. scroll
# ---------------------------------------------------------------------------


@mcp.tool()
async def scroll(
    direction: str = "down",
    amount: int = 3,
    session_id: Optional[str] = None,
) -> dict:
    """
    Scroll the page in a direction (up/down/left/right) by amount steps.
    session_id: omit to use the shared MCP session.
    """
    session_id = session_id or _DEFAULT_SESSION_ID
    run_id = str(uuid.uuid4())
    # Convert direction+amount into deltaY (positive=down, negative=up)
    sign = 1 if direction in ("down", "right") else -1
    delta_y = sign * amount * 100
    delta_x = sign * amount * 100 if direction in ("left", "right") else 0

    resp = await _post(
        "/v1/computer",
        {
            "action": "scroll",
            "session_id": session_id,
            "run_id": run_id,
            "delta": [delta_x, delta_y],
        },
    )

    success = resp.get("status") in ("completed",)
    position = {"x": 0, "y": 0}
    if resp.get("extracted_content") and isinstance(resp["extracted_content"], dict):
        ec = resp["extracted_content"]
        position = ec.get("position", position)

    return {"success": success, "position": position}


# ---------------------------------------------------------------------------
# 5. key
# ---------------------------------------------------------------------------


@mcp.tool()
async def key(
    keys: str,
    session_id: Optional[str] = None,
) -> dict:
    """
    Press keyboard key(s). Examples: "Enter", "Tab", "Control+a", "Escape".
    session_id: omit to use the shared MCP session.
    """
    session_id = session_id or _DEFAULT_SESSION_ID
    run_id = str(uuid.uuid4())
    resp = await _post(
        "/v1/computer",
        {
            "action": "key",
            "session_id": session_id,
            "run_id": run_id,
            "key": keys,
        },
    )

    success = resp.get("status") in ("completed",)
    return {"success": success, "keys": keys}


# ---------------------------------------------------------------------------
# 6. navigate
# ---------------------------------------------------------------------------


@mcp.tool()
async def navigate(
    url: str,
    wait_until: str = "domcontentloaded",
    session_id: Optional[str] = None,
) -> dict:
    """
    Navigate the browser to a URL.

    wait_until options: "load", "domcontentloaded", "networkidle", "commit"
    session_id: omit to use the shared MCP session (same browser tab as screenshot/click).
    """
    session_id = session_id or _DEFAULT_SESSION_ID
    run_id = str(uuid.uuid4())
    resp = await _post(
        "/v1/computer",
        {
            "action": "navigate",
            "session_id": session_id,
            "run_id": run_id,
            "url": url,
            "parameters": {"wait_until": wait_until},
        },
    )

    success = resp.get("status") in ("completed",)
    result_url = url
    title = ""
    if resp.get("extracted_content") and isinstance(resp["extracted_content"], dict):
        ec = resp["extracted_content"]
        result_url = ec.get("url", url)
        title = ec.get("title", "")

    return {"success": success, "url": result_url, "title": title}


# ---------------------------------------------------------------------------
# 7. record_start
# ---------------------------------------------------------------------------


@mcp.tool()
async def record_start(
    name: Optional[str] = None,
    session_id: Optional[str] = None,
) -> dict:
    """
    Start recording all actions for this session to a JSONL file.

    Returns a recording_id (use with record_stop) and the file path.
    """
    # Import here to avoid circular import at module level
    import sys as _sys
    import os as _os

    # Add core parent to path so we can import ActionRecorder
    _core_dir = str(Path(__file__).parent.parent)
    if _core_dir not in _sys.path:
        _sys.path.insert(0, _core_dir)

    from core.action_recorder import ActionRecorder  # noqa: PLC0415

    session_id = session_id or _DEFAULT_SESSION_ID
    recording_id = f"rec-{uuid.uuid4().hex[:12]}"
    recorder = ActionRecorder(
        recording_id=recording_id,
        task=name or f"session-{session_id}",
        session_id=session_id,
        run_id=str(uuid.uuid4()),
        record_gif=True,
    )
    await recorder.start()
    _active_recordings[recording_id] = recorder

    return {
        "recording_id": recording_id,
        "path": str(recorder.get_path()),
    }


# ---------------------------------------------------------------------------
# 8. record_stop
# ---------------------------------------------------------------------------


@mcp.tool()
async def record_stop(
    recording_id: str,
) -> dict:
    """
    Stop an active recording and finalize the JSONL file.

    recording_id: the value returned by record_start.
    Returns the number of recorded frames and the output path.
    """
    recorder = _active_recordings.pop(recording_id, None)
    if recorder is None:
        return {
            "success": False,
            "frames": 0,
            "path": "",
        }

    await recorder.stop()
    path = str(recorder.get_path())
    frames = recorder._frame_count
    gif_path = str(recorder.get_gif_path()) if recorder.get_gif_path() else None

    return {"success": True, "frames": frames, "path": path, "gif_path": gif_path}


# ---------------------------------------------------------------------------
# 9. scratchpad_read
# ---------------------------------------------------------------------------


@mcp.tool()
async def scratchpad_read(
    task: str,
    section: str = "context",
) -> dict:
    """
    Read the ACU scratchpad for a task.

    section options:
      "context"     — full system-prompt injection string (strategy + skills + lessons)
      "strategy"    — raw strategy.md text
      "skills"      — list of graduated skills
      "reflections" — recent reflection entries
      "environment" — globally discovered shortcuts, URLs, selectors

    task: the task name or description (matches how the planning loop was run).
    """
    import sys as _sys
    from pathlib import Path as _Path
    _core_dir = str(_Path(__file__).parent.parent)
    if _core_dir not in _sys.path:
        _sys.path.insert(0, _core_dir)

    from core.scratchpad import get_scratchpad  # noqa: PLC0415

    sp = get_scratchpad()
    if section == "context":
        return {"context": sp.build_context(task)}
    if section == "strategy":
        return {"strategy": sp.get_strategy(task)}
    if section == "skills":
        return {"skills": sp.get_skills(task)}
    if section == "reflections":
        return {"reflections": sp.get_reflections(task)[-10:]}
    if section == "environment":
        return {"environment": sp.get_environment()}
    return {"error": f"Unknown section {section!r}. Valid: context, strategy, skills, reflections, environment"}


# ---------------------------------------------------------------------------
# 10. scratchpad_write
# ---------------------------------------------------------------------------


@mcp.tool()
async def scratchpad_write(
    task: str,
    heuristic: str,
    overwrite: bool = False,
) -> dict:
    """
    Write a heuristic to the task's strategy.md scratchpad.

    task:      the task name/description.
    heuristic: one actionable instruction to remember for this task.
    overwrite: if True, replace strategy.md entirely; if False (default), append.

    Example heuristic: "Click Accept button before navigating to avoid modal blocking."
    """
    import sys as _sys
    from pathlib import Path as _Path
    _core_dir = str(_Path(__file__).parent.parent)
    if _core_dir not in _sys.path:
        _sys.path.insert(0, _core_dir)

    from core.scratchpad import get_scratchpad  # noqa: PLC0415

    sp = get_scratchpad()
    if overwrite:
        sp.update_strategy(task, heuristic)
    else:
        sp.append_strategy(task, heuristic)

    return {"success": True, "strategy": sp.get_strategy(task)}


# ---------------------------------------------------------------------------
# 11. scratchpad_reflect
# ---------------------------------------------------------------------------


@mcp.tool()
async def scratchpad_reflect(
    task: str,
    outcome: str,
    lesson: Optional[str] = None,
    session_id: Optional[str] = None,
) -> dict:
    """
    Manually trigger a reflection for a task run.

    task:       the task description.
    outcome:    "success", "failure", or "partial".
    lesson:     optional explicit lesson (skips LLM generation if provided).
    session_id: the session this run was in.

    Use this after you observe a task result and want to log what you learned.
    The lesson is appended to strategy.md and stored as a reflection entry.
    """
    import sys as _sys
    from pathlib import Path as _Path
    _core_dir = str(_Path(__file__).parent.parent)
    if _core_dir not in _sys.path:
        _sys.path.insert(0, _core_dir)

    from core.scratchpad import get_scratchpad, ReflectionEntry  # noqa: PLC0415

    sp = get_scratchpad()

    if lesson:
        # Explicit lesson — skip LLM, store directly
        entry = ReflectionEntry(
            task=task,
            session_id=session_id or _DEFAULT_SESSION_ID,
            outcome=outcome,
            lesson=lesson,
        )
        sp.record_reflection(entry)
        if outcome in ("success", "partial"):
            sp.append_strategy(task, lesson)
        return {"success": True, "lesson": lesson, "strategy": sp.get_strategy(task)}

    # No lesson provided — run the full async reflection pipeline (LLM if available)
    entry = await sp.reflect(
        task=task,
        steps=[],
        outcome=outcome,
        session_id=session_id or _DEFAULT_SESSION_ID,
    )
    return {"success": True, "lesson": entry.lesson, "strategy": sp.get_strategy(task)}


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    mode = sys.argv[1] if len(sys.argv) > 1 else "stdio"

    if mode == "stdio":
        mcp.run(transport="stdio")
    elif mode == "sse":
        mcp.run(transport="sse")
    else:
        print(f"Unknown mode: {mode!r}. Use 'stdio' or 'sse'.", file=sys.stderr)
        sys.exit(1)
