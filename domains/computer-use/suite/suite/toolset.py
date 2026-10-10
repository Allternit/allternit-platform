"""Direct client for `POST /computers/:id/toolset`: checkers (`verify`,
`read_ui`) and replay mode call the executor exactly as a planner would,
through the same lease, policy, approval and audit pipeline."""
from __future__ import annotations

import time
from typing import Optional

from .stack import Stack, http_json


class Toolset:
    def __init__(self, stack: Stack, run_id: str, computer: str = "this-device"):
        self.stack = stack
        self.run_id = run_id
        self.computer = computer
        self.calls = 0

    def call(self, member: str, input: dict, toolset: str = "computer", turn: Optional[str] = None) -> dict:
        """One member call; approval round trips are taken on the person's
        behalf only because the suite runs on scratch state it owns."""
        req = {
            "toolset": toolset,
            "member": member,
            "input": input,
            "run_id": self.run_id,
            "turn_id": turn or f"{self.run_id}-suite",
            "call_index": self.calls,
            "coordinate_space": "pixels",
        }
        self.calls += 1
        url = f"{self.stack.api}/api/v1/computers/{self.computer}/toolset"
        t0 = time.monotonic()
        status, body = http_json("POST", url, req, self.stack.headers())
        if status == 409 and isinstance(body, dict) and body.get("error") == "approval_required" and body.get("approval_id"):
            http_json("POST", f"{self.stack.api}/api/aci/handoff/{body['approval_id']}/approve", {}, self.stack.headers())
            req["approval_grant"] = body["approval_id"]
            status, body = http_json("POST", url, req, self.stack.headers())
        body = body if isinstance(body, dict) else {"raw": body}
        body["_status"] = status
        body["_ms"] = round((time.monotonic() - t0) * 1000, 1)
        return body

    @staticmethod
    def ok(body: dict) -> bool:
        return body.get("_status", 500) < 400 and not body.get("is_error")

    @staticmethod
    def text(body: dict) -> str:
        return "\n".join(c.get("text", "") for c in body.get("content", []) if c.get("type") == "text")
