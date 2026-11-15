# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Allternit LLC
"""Events from the worker process to the engine server.

The worker writes one line per event to stdout, prefixed with MARKER so that
anything a library prints to stdout is kept as a plain log line instead of
being mistaken for an event.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import IO, Any, Dict, Optional

MARKER = "@@allternit-tune "


def encode(event: Dict[str, Any]) -> str:
    return MARKER + json.dumps(event, separators=(",", ":"))


def decode(line: str) -> Optional[Dict[str, Any]]:
    """The event in a worker stdout line, or None if it is plain output."""
    if not line.startswith(MARKER):
        return None
    try:
        event = json.loads(line[len(MARKER):])
    except ValueError:
        return None
    return event if isinstance(event, dict) and "kind" in event else None


def dir_bytes(path: Path) -> int:
    if path.is_file():
        return path.stat().st_size
    return sum(p.stat().st_size for p in path.rglob("*") if p.is_file())


class Emitter:
    """Worker side: writes events for the server to read."""

    def __init__(self, stream: Optional[IO[str]] = None) -> None:
        self._stream = stream or sys.stdout

    def _send(self, event: Dict[str, Any]) -> None:
        self._stream.write(encode(event) + "\n")
        self._stream.flush()

    def state(self, state: str, message: str = "") -> None:
        self._send({"kind": "state", "state": state, "message": message or state})

    def log(self, message: str) -> None:
        self._send({"kind": "log", "message": message})

    def progress(self, fraction: float, message: str) -> None:
        self._send({"kind": "progress", "progress": max(0.0, min(1.0, fraction)), "message": message})

    def output(self, kind: str, path: Path, message: str = "") -> None:
        # The contract: JobOutput.path is an absolute path on this computer.
        path = Path(path).expanduser().resolve()
        self._send({
            "kind": "output",
            "output": {"kind": kind, "path": str(path), "bytes": dir_bytes(path)},
            "message": message or f"{kind} written to {path}",
        })

    def error(self, message: str) -> None:
        self._send({"kind": "error", "message": message})
