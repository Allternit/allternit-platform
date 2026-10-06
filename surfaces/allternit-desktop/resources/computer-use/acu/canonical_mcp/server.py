"""MCP tools over the canonical REST authority."""

from __future__ import annotations

import json
import os
import urllib.error
import urllib.parse
import urllib.request
from typing import Any, Dict, Optional

from mcp.server.mcpserver import MCPServer


# Official mcp >= 2.3: serves MCP 2026-07-28 and the legacy initialize handshake.
mcp = MCPServer("allternit-computer-canonical")
BASE_URL = os.environ.get("ALLTERNIT_COMPUTER_URL", "http://127.0.0.1:8760").rstrip("/")


def _call(method: str, path: str, body: Optional[Dict[str, Any]] = None) -> Dict[str, Any]:
    request = urllib.request.Request(
        f"{BASE_URL}/v1/computer-use/canonical{path}", method=method,
        data=json.dumps(body).encode() if body is not None else None,
        headers={"Content-Type": "application/json", "Accept": "application/json"},
    )
    key = os.environ.get("ACU_API_KEY")
    if key:
        request.add_header("Authorization", f"Bearer {key}")
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        raise RuntimeError(error.read().decode("utf-8", "replace")) from error


@mcp.tool()
def computer_providers() -> Dict[str, Any]:
    """List truthful canonical provider capabilities and unavailable diagnostics."""
    return _call("GET", "/providers")


@mcp.tool()
def computer_roots(session_id: str, environment_id: str = "environment_local", provider_id: Optional[str] = None) -> Dict[str, Any]:
    """Discover and bind browser/native roots for a logical session."""
    return _call("POST", "/roots", {
        "session_id": session_id, "environment_id": environment_id, "provider_id": provider_id,
    })


@mcp.tool()
def computer_observe(provider_id: str, session_id: str, environment_id: str, resource_id: str) -> Dict[str, Any]:
    """Capture one immutable, state-scoped canonical observation."""
    return _call("POST", "/observe", {
        "provider_id": provider_id, "session_id": session_id,
        "environment_id": environment_id, "resource_id": resource_id,
    })


@mcp.tool()
def computer_approve_transaction(transaction: Dict[str, Any], approved_by: str, ttl_seconds: int = 120) -> Dict[str, Any]:
    """Issue a single-use approval bound to an exact transaction."""
    return _call("POST", "/approvals", {
        "transaction": transaction, "approved_by": approved_by, "ttl_seconds": ttl_seconds,
    })


@mcp.tool()
def computer_execute_transaction(provider_id: str, transaction: Dict[str, Any]) -> Dict[str, Any]:
    """Execute a state-bound transaction; side effects require a matching approval."""
    return _call("POST", "/transactions", {"provider_id": provider_id, "transaction": transaction})


@mcp.tool()
def computer_trajectory(session_id: str) -> Dict[str, Any]:
    """Export the ordered, redacted, integrity-hashed canonical trajectory."""
    quoted = urllib.parse.quote(session_id, safe="")
    return _call("GET", f"/sessions/{quoted}/trajectory")


@mcp.tool()
def computer_history_status() -> Dict[str, Any]:
    """Check whether CUA Driver Computer History is supported, admitted, and enabled."""
    return _call("POST", "/history/status", {})


@mcp.tool()
def computer_history_query(
    limit: int = 50,
    session_id: Optional[str] = None,
    since_sequence: Optional[int] = None,
    until_sequence: Optional[int] = None,
) -> Dict[str, Any]:
    """Query a bounded slice of CUA Driver Computer History metadata events.

    Returns metadata-only CloudEvents-style records. Results may enter model context.
    Continue without history if the daemon reports absence, denial, or empty results.
    """
    body: Dict[str, Any] = {"limit": limit}
    if session_id is not None:
        body["session_id"] = session_id
    if since_sequence is not None:
        body["since_sequence"] = since_sequence
    if until_sequence is not None:
        body["until_sequence"] = until_sequence
    return _call("POST", "/history/query", body)


def main() -> None:
    mcp.run()


if __name__ == "__main__":
    main()
