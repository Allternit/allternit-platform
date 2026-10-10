"""HTTP transport for the canonical computer-use service."""

from __future__ import annotations

import hashlib
import os
from dataclasses import asdict
from pathlib import Path
from typing import Any, Dict, Optional

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel, Field

from core.canonical_runtime import CanonicalRuntimeError, StaleResourceStateError
from core.canonical_receipt import ReceiptLedger
from core.canonical_events import EventLedger
from core.native_capabilities import native_capability_payload, native_permission_request_plan
from core.trajectory_export import export_trajectory
from core.session_authority import SessionAuthority
from core.evaluation_authority import EvaluationAuthority, SUITES
from core.canonical_replay import build_time_aligned_mp4
from core.routing_authority import RoutingAuthority
from core.legacy_migration import LegacyMigrationService
from core.benchmark_adapters import benchmark_adapter_statuses
from core.persistent_observation_store import SQLiteObservationStore


router = APIRouter(prefix="/v1/computer-use/canonical", tags=["computer-use-canonical"])

_state_dir = Path(os.environ.get("ALLTERNIT_COMPUTER_STATE_DIR", "~/.allternit/computer-use")).expanduser()
_state_dir.mkdir(parents=True, exist_ok=True)
_store = SQLiteObservationStore(_state_dir / "observations.sqlite3")
_receipts = ReceiptLedger(_state_dir / "receipts.sqlite3")
_events = EventLedger(_state_dir / "events.sqlite3")
_sessions = SessionAuthority(_state_dir / "sessions.sqlite3")
_evaluations = EvaluationAuthority(_state_dir / "evaluations.sqlite3")
_routing = RoutingAuthority(_state_dir / "routing.sqlite3", _evaluations)
_recording_roots = tuple(
    value for value in os.environ.get(
        "ALLTERNIT_LEGACY_RECORDING_ROOTS", "~/.allternit/recordings:/tmp/allternit-recordings"
    ).split(os.pathsep) if value
)
_migrations = LegacyMigrationService(
    _state_dir / "migrations.sqlite3", _events, allowed_roots=_recording_roots,
)


class ShadowResultRequest(BaseModel):
    receipt_id: str
    session_id: str
    legacy_route_id: str
    legacy_status: str
    legacy_evidence_sha256: Optional[str] = None


class EvaluationRecordRequest(BaseModel):
    suite_id: str
    provider_id: str
    capability_cell: str
    environment_id: str
    passed: bool
    score: float = Field(ge=0, le=1)
    evidence_sha256: str
    source: str = "measured"
    metadata: Dict[str, Any] = Field(default_factory=dict)


class RoutingCellRequest(BaseModel):
    capability_cell: str
    canonical_provider_id: str
    legacy_route_id: str


class RoutingTransitionRequest(BaseModel):
    stage: str


class LegacyRecordingMigrationRequest(BaseModel):
    path: str


class LegacyReceiptMigrationRequest(BaseModel):
    path: str
    session_id: str


class NativePermissionPlanRequest(BaseModel):
    permission: str


def _http_error(error: Exception) -> HTTPException:
    if isinstance(error, StaleResourceStateError):
        return HTTPException(
            status_code=409,
            detail={
                "code": "stale_resource_state",
                "message": str(error),
                "resource_id": error.resource_id,
                "expected_epoch": error.expected,
                "actual_epoch": error.actual,
            },
        )
    if isinstance(error, (CanonicalRuntimeError, ValueError, KeyError)):
        return HTTPException(status_code=422, detail={"code": "canonical_validation_failed", "message": str(error)})
    return HTTPException(status_code=500, detail={"code": "canonical_internal_error", "message": str(error)})


# The per-backend canonical providers (playwright, accessibility, cdp,
# extension, cua-driver, agent-desktop, droidrun, phone-harness) and the
# observe / roots / transactions / approvals routes that drove them were removed
# in the D0 cleanup. Desktop and browser actions run through the computer
# toolset (/v1/execute, /v1/computers/:id/toolset). This router keeps the
# receipt, event, trajectory, evaluation and routing ledgers. /providers and
# /health stay so existing clients get an empty catalog instead of a 404.


@router.get("/providers")
async def providers() -> Dict[str, Any]:
    return {"providers": [], "diagnostics": {}}


@router.get("/health")
async def canonical_health() -> Dict[str, Any]:
    return {
        "status": "degraded",
        "contract_version": "1.0.0-alpha.1",
        "registered_provider_count": 0,
        "diagnostics": {},
    }


@router.get("/native-capabilities")
async def native_capabilities() -> Dict[str, Any]:
    return native_capability_payload()


@router.post("/native-permissions/request-plan")
async def native_permission_plan(body: NativePermissionPlanRequest) -> Dict[str, Any]:
    try:
        return native_permission_request_plan(body.permission)
    except Exception as error:
        raise _http_error(error) from error


@router.get("/evaluation/suites")
async def evaluation_suites() -> Dict[str, Any]:
    return {
        "suites": [asdict(suite) for suite in SUITES],
        "adapters": benchmark_adapter_statuses(),
    }


@router.post("/evaluation/results")
async def record_evaluation(body: EvaluationRecordRequest) -> Dict[str, Any]:
    try:
        return asdict(_evaluations.record(**body.model_dump()))
    except Exception as error:
        raise _http_error(error) from error


@router.get("/evaluation/gates/{provider_id}/{capability_cell}")
async def evaluation_gate(provider_id: str, capability_cell: str) -> Dict[str, Any]:
    return _evaluations.release_gate(provider_id, capability_cell)


@router.get("/routing/cells")
async def routing_cells() -> Dict[str, Any]:
    return {"cells": [asdict(item) for item in _routing.list()]}


@router.post("/routing/cells")
async def configure_routing_cell(body: RoutingCellRequest) -> Dict[str, Any]:
    try:
        return asdict(_routing.configure(**body.model_dump()))
    except Exception as error:
        raise _http_error(error) from error


@router.post("/routing/cells/{capability_cell}/transition")
async def transition_routing_cell(capability_cell: str, body: RoutingTransitionRequest) -> Dict[str, Any]:
    try:
        return asdict(_routing.transition(capability_cell, body.stage))
    except Exception as error:
        raise _http_error(error) from error


@router.post("/migrations/legacy-recording")
async def migrate_legacy_recording(body: LegacyRecordingMigrationRequest) -> Dict[str, Any]:
    try:
        return _migrations.import_recording(body.path)
    except Exception as error:
        raise _http_error(error) from error


@router.post("/migrations/legacy-receipts")
async def migrate_legacy_receipts(body: LegacyReceiptMigrationRequest) -> Dict[str, Any]:
    try:
        return _migrations.import_receipts(body.path, session_id=body.session_id)
    except Exception as error:
        raise _http_error(error) from error


@router.post("/shadow/results")
async def shadow_result(body: ShadowResultRequest) -> Dict[str, Any]:
    try:
        receipt = _receipts.get(body.receipt_id)
        if receipt.session_id != body.session_id:
            raise ValueError("Receipt does not belong to the supplied session")
        comparison = {
            "receipt_id": body.receipt_id,
            "canonical_status": receipt.outcome_status,
            "legacy_status": body.legacy_status,
            "status_agrees": receipt.outcome_status == body.legacy_status,
            "legacy_route_id": body.legacy_route_id,
            "legacy_evidence_sha256": body.legacy_evidence_sha256,
            "side_effect_replayed": False,
        }
        _events.append(
            "shadow.result.compared", session_id=body.session_id,
            transaction_id=receipt.transaction_id, payload=comparison,
        )
        return comparison
    except Exception as error:
        raise _http_error(error) from error


@router.get("/receipts/{receipt_id}")
async def get_receipt(receipt_id: str) -> Dict[str, Any]:
    try:
        receipt = _receipts.get(receipt_id)
        return {**asdict(receipt), "verified": _receipts.verify(receipt)}
    except Exception as error:
        raise _http_error(error) from error


@router.get("/sessions/{session_id}/events")
async def list_events(session_id: str, after_sequence: int = 0, limit: int = 1000) -> Dict[str, Any]:
    events = _events.list_session(session_id, after_sequence=after_sequence, limit=limit)
    return {"session_id": session_id, "events": events}


@router.get("/sessions/{session_id}/trajectory")
async def get_session_trajectory(session_id: str) -> Dict[str, Any]:
    return export_trajectory(_events, session_id)


@router.post("/sessions/{session_id}/replay/mp4")
async def create_session_mp4(session_id: str) -> Dict[str, Any]:
    try:
        safe_session = hashlib.sha256(session_id.encode("utf-8")).hexdigest()[:32]
        output = _state_dir / "replays" / f"session_{safe_session}.mp4"
        return await build_time_aligned_mp4(
            _store.list_session(session_id), artifact_dir=_state_dir / "artifacts", output_path=output,
        )
    except Exception as error:
        raise _http_error(error) from error


@router.get("/sessions/{session_id}/resources")
async def get_session_resources(session_id: str, environment_id: Optional[str] = None) -> Dict[str, Any]:
    return {
        "session_id": session_id,
        "resources": [asdict(item) for item in _sessions.list_bindings(session_id, environment_id=environment_id)],
    }


async def shutdown_canonical_service() -> None:
    _store.close()
    _receipts.close()
    _events.close()
    _sessions.close()
    _evaluations.close()
    _routing.close()
    _migrations.close()
