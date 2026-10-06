"""
Allternit Computer Use — S1 decision runtime client

The one Python client for the S1 decision runtime (``POST /v1/decision``,
``POST /v1/decision/outcome``), mirroring the TS client
(``tools/system-one-local/src/decision/client.ts``) and the Rust
``OutcomeReporter`` (``factory/engine/src/workflows/kernel/s1_outcome.rs``).

- URL: ``ALLTERNIT_S1_URL`` / ``SYSTEM_ONE_URL`` (default ``http://127.0.0.1:7717``).
- Backend: ``ALLTERNIT_S1_BACKEND`` (default ``auto``).
- Token: ``SYSTEM_ONE_TOKEN`` (bearer), optional.
- ``ALLTERNIT_S1_OUTCOMES=0`` disables outcome reporting.

The HTTP transport is injectable (tests pass a fake); the default uses
``urllib`` so the module has no third-party dependency. ``decide`` raises on
failure (heads surface errors as ``ShadowHeadError``); ``report_outcome``
never raises.
"""

from __future__ import annotations

import json
import os
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Any, Callable, Dict, List, Optional, Sequence, Tuple

DEFAULT_RUNTIME_URL = "http://127.0.0.1:7717"

# (url, body_bytes, headers, timeout_s) -> (status, response_bytes)
Transport = Callable[[str, bytes, Dict[str, str], float], Tuple[int, bytes]]


class S1DecisionError(Exception):
    """The decision runtime was unreachable or rejected the request."""


def urllib_transport(url: str, body: bytes, headers: Dict[str, str], timeout: float) -> Tuple[int, bytes]:
    req = urllib.request.Request(url, data=body, headers=headers, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:  # noqa: S310 (local runtime URL)
            return resp.status, resp.read()
    except urllib.error.HTTPError as err:
        return err.code, err.read() or b""


def envelope(producer: str, run_id: Optional[str] = None) -> Dict[str, Any]:
    rid = run_id or producer
    return {
        "abi_version": "1.0.0",
        "schema_id": "allternit.kernel.DecisionRequestV1",
        "schema_version": "1.0.0",
        "run_id": rid,
        "session_id": rid,
        "task_id": rid,
        "state_version": 0,
        "created_at": datetime.now(timezone.utc).isoformat(),
        "producer": producer,
        "trace_id": rid,
        "provenance": [],
    }


def choice_request(
    *,
    producer: str,
    bank: str,
    question_id: str,
    options: Sequence[str],
    instructions: str,
    motif: str = "ROUTE",
    primitive_id: Optional[str] = None,
    subject_ref: Optional[str] = None,
    run_id: Optional[str] = None,
    incumbent: Optional[str] = None,
) -> Dict[str, Any]:
    """A CHOICE ``DecisionRequestV1`` over a closed option set.

    ``incumbent`` is the live decider's answer (Q26 ``x-incumbent``); it is
    sent only when it is one of ``options``."""
    ext: Dict[str, Any] = {"x-motif": motif}
    if primitive_id:
        ext["x-primitive_id"] = primitive_id
    if subject_ref:
        ext["x-subject_ref"] = subject_ref
    if incumbent is not None and incumbent in options:
        ext["x-incumbent"] = incumbent
    return {
        "envelope": envelope(producer, run_id),
        "operation": "CHOICE",
        "state_projection_ref": f"state.{producer}",
        "decision_bank_id": bank,
        "question_id": question_id,
        "instructions": instructions,
        "latency_class": "INTERACTIVE",
        "candidates": [{"candidate_id": o, "label": o} for o in options],
        "extensions": ext,
    }


@dataclass
class S1DecisionClient:
    url: str = field(default_factory=lambda: os.environ.get("ALLTERNIT_S1_URL") or os.environ.get("SYSTEM_ONE_URL") or DEFAULT_RUNTIME_URL)
    backend: str = field(default_factory=lambda: os.environ.get("ALLTERNIT_S1_BACKEND") or "auto")
    token: Optional[str] = field(default_factory=lambda: os.environ.get("SYSTEM_ONE_TOKEN") or None)
    timeout_s: float = 10.0
    outcomes_enabled: bool = field(default_factory=lambda: os.environ.get("ALLTERNIT_S1_OUTCOMES", "1") != "0")
    transport: Transport = urllib_transport

    def _post(self, path: str, payload: Dict[str, Any]) -> Tuple[int, Any]:
        headers = {"content-type": "application/json"}
        if self.token:
            headers["authorization"] = f"Bearer {self.token}"
        status, raw = self.transport(
            f"{self.url.rstrip('/')}{path}", json.dumps(payload).encode(), headers, self.timeout_s
        )
        try:
            body = json.loads(raw.decode() or "{}")
        except ValueError:
            body = {}
        return status, body

    def decide(self, request: Dict[str, Any], state: str) -> Dict[str, Any]:
        """POST /v1/decision; returns the ``DecisionResultV1``. Raises ``S1DecisionError``."""
        try:
            status, body = self._post("/v1/decision", {"request": request, "state": state, "backend": self.backend})
        except (OSError, urllib.error.URLError) as err:
            raise S1DecisionError(f"S1 decision runtime unreachable at {self.url}: {err}") from err
        if not 200 <= status < 300:
            msg = (body.get("error") or {}).get("message", "") if isinstance(body, dict) else ""
            raise S1DecisionError(f"S1 decision runtime returned {status}: {msg}")
        if not isinstance(body, dict):
            raise S1DecisionError("S1 decision runtime returned a non-object body")
        return body

    def report_outcome(self, decision_id: str, truth: str, source: str) -> bool:
        """POST /v1/decision/outcome. Whether the runtime accepted it; never raises."""
        if not self.outcomes_enabled or not decision_id:
            return False
        try:
            status, _ = self._post("/v1/decision/outcome", {"decision_id": decision_id, "truth": truth, "source": source})
        except Exception:  # noqa: BLE001 — fire-and-forget by contract
            return False
        return 200 <= status < 300


def decision_id_of(result: Dict[str, Any]) -> Optional[str]:
    ext = result.get("extensions") or {}
    v = ext.get("x-decision_id")
    return str(v) if v else None


def probabilities_of(result: Dict[str, Any], options: List[str]) -> Dict[str, float]:
    """The result's distribution restricted to ``options`` and renormalised."""
    raw = result.get("probabilities") or {}
    probs = {o: max(0.0, float(raw.get(o, 0.0) or 0.0)) for o in options}
    total = sum(probs.values())
    if total <= 0.0:
        raise S1DecisionError("S1 decision result has no probability mass on the options")
    return {o: p / total for o, p in probs.items()}
