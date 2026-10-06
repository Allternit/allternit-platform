"""Dual-era smoke for the MCP servers on the official `mcp` 2.x SDK.

Each server is spawned over stdio and must answer both a 2026-07-28 client
(`server/discover`, per-request `_meta`) and a legacy `initialize` client with
the same tool list. Skips when `mcp>=2.3` is not installed.
"""
from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

mcp = pytest.importorskip("mcp")
pytest.importorskip("mcp.server.mcpserver")

ROOT = Path(__file__).resolve().parents[1]
META = {
    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
    "io.modelcontextprotocol/clientInfo": {"name": "smoke", "version": "1"},
    "io.modelcontextprotocol/clientCapabilities": {},
}
SERVERS = {
    "canonical_mcp.server": [sys.executable, "-c", "from canonical_mcp.server import main; main()"],
    "acu_mcp.server": [sys.executable, "-m", "acu_mcp.server", "stdio"],
}


def _exchange(cmd: list[str], messages: list[dict]) -> dict[int, dict]:
    """Send messages one by one and wait for each request's response (stdin stays
    open meanwhile: servers abort in-flight requests when stdin closes)."""
    proc = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, cwd=ROOT)
    out: dict[int, dict] = {}
    try:
        for m in messages:
            proc.stdin.write(json.dumps(m) + "\n")
            proc.stdin.flush()
            if "id" not in m:
                continue
            while m["id"] not in out:
                line = proc.stdout.readline()
                if not line:
                    raise AssertionError(f"server exited before answering {m['method']}")
                try:
                    msg = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if isinstance(msg, dict) and "id" in msg:
                    out[msg["id"]] = msg
    finally:
        proc.stdin.close()
        proc.wait(timeout=30)
    return out


@pytest.mark.parametrize("name", sorted(SERVERS))
def test_dual_era(name: str) -> None:
    pytest.importorskip("httpx")
    modern = _exchange(
        SERVERS[name],
        [
            {"jsonrpc": "2.0", "id": 1, "method": "server/discover", "params": {"_meta": META}},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {"_meta": META}},
        ],
    )
    assert "2026-07-28" in modern[1]["result"]["supportedVersions"]
    modern_tools = [t["name"] for t in modern[2]["result"]["tools"]]

    legacy = _exchange(
        SERVERS[name],
        [
            {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "old", "version": "1"}}},
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
        ],
    )
    assert legacy[1]["result"]["protocolVersion"] == "2025-06-18"
    legacy_tools = [t["name"] for t in legacy[2]["result"]["tools"]]
    assert modern_tools == legacy_tools and modern_tools
