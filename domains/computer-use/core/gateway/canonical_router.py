"""HTTP transport for the canonical computer-use service."""

from __future__ import annotations

import asyncio
import hashlib
import os
import secrets
from dataclasses import asdict
from pathlib import Path
from typing import Any, Dict, Optional

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel, Field

from contracts.codec import transaction_from_dict
from core.canonical_runtime import CanonicalRuntimeError, StaleResourceStateError
from core.canonical_approval import ApprovalAuthority
from core.canonical_receipt import ReceiptLedger
from core.canonical_events import EventLedger
from core.canonical_service import CanonicalComputerService, ProviderNotFoundError
from core.native_capabilities import native_capability_payload, native_permission_request_plan
from core.canonical_policy import CanonicalPolicyEngine, PolicyDeniedError
from core.trajectory_export import export_trajectory
from core.session_authority import SessionAuthority
from core.evaluation_authority import EvaluationAuthority, SUITES
from core.canonical_replay import build_time_aligned_mp4
from core.shadow_comparison import compare_observations
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
service = CanonicalComputerService(store=_store)
_approval_key = os.environ.get("ALLTERNIT_COMPUTER_APPROVAL_KEY")
approval_authority = ApprovalAuthority(
    _approval_key.encode("utf-8") if _approval_key else secrets.token_bytes(32)
)
policy_engine = CanonicalPolicyEngine()
_initialized = False
_initialization_lock = asyncio.Lock()
_provider_diagnostics: Dict[str, Dict[str, Any]] = {}


async def ensure_initialized() -> None:
    global _initialized
    if _initialized:
        return
    async with _initialization_lock:
        if _initialized:
            return
        await _initialize_providers()
        _initialized = True


async def _initialize_providers() -> None:
    # The per-backend canonical providers (playwright, accessibility, cdp,
    # extension, cua-driver, agent-desktop, droidrun, phone-harness) were
    # removed in the D0 cleanup; desktop and browser actions run through the
    # canonical toolset path (/v1/execute). The service keeps the receipt,
    # event and trajectory ledgers and an empty provider registry.
    return None


class ObserveRequest(BaseModel):
    provider_id: str = "browser.playwright.canonical"
    session_id: str
    environment_id: str = "environment_local_browser"
    resource_id: str = "browser_page_main"


class FindRootsRequest(BaseModel):
    session_id: str
    environment_id: str = "environment_local"
    provider_id: Optional[str] = None


class ShadowObserveRequest(BaseModel):
    primary_provider_id: str
    shadow_provider_id: str
    session_id: str
    environment_id: str = "environment_local"
    primary_resource_id: str
    shadow_resource_id: str


class ShadowResultRequest(BaseModel):
    receipt_id: str
    session_id: str
    legacy_route_id: str
    legacy_status: str
    legacy_evidence_sha256: Optional[str] = None


class TransactionRequest(BaseModel):
    provider_id: str = "browser.playwright.canonical"
    transaction: Dict[str, Any]


class ApprovalGrantRequest(BaseModel):
    transaction: Dict[str, Any]
    approved_by: str
    ttl_seconds: int = Field(default=120, ge=1, le=600)


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


class StreamNegotiationRequest(BaseModel):
    provider_id: str
    requested: list[str]
    codecs: list[str] = Field(default_factory=list)


class NativePermissionPlanRequest(BaseModel):
    permission: str


def _http_error(error: Exception) -> HTTPException:
    if isinstance(error, ProviderNotFoundError):
        return HTTPException(status_code=404, detail={"code": "provider_not_found", "message": str(error)})
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
    if isinstance(error, PolicyDeniedError):
        return HTTPException(status_code=403, detail={"code": "policy_denied", "message": str(error)})
    return HTTPException(status_code=500, detail={"code": "canonical_internal_error", "message": str(error)})


@router.get("/providers")
async def providers() -> Dict[str, Any]:
    await ensure_initialized()
    return {
        "providers": [asdict(manifest) for manifest in service.capabilities()],
        "diagnostics": _provider_diagnostics,
    }


@router.post("/streams/negotiate")
async def negotiate_stream(body: StreamNegotiationRequest) -> Dict[str, Any]:
    await ensure_initialized()
    try:
        manifest = next(item for item in service.capabilities() if item.provider_id == body.provider_id)
    except StopIteration as error:
        raise _http_error(ProviderNotFoundError(body.provider_id)) from error
    available = set()
    if manifest.streaming:
        available.add("viewport")
    if manifest.audio:
        available.add("audio")
    requested = set(body.requested)
    accepted = sorted(requested & available)
    rejected = sorted(requested - available)
    codec = next((item for item in body.codecs if item in {"h264", "vp9", "png", "jpeg", "opus"}), None)
    return {
        "provider_id": body.provider_id, "accepted": accepted, "rejected": rejected,
        "codec": codec if accepted else None, "transport": None,
        "ready": bool(accepted) and not rejected,
        "reason": None if accepted else "provider_does_not_advertise_streaming",
    }


@router.get("/health")
async def canonical_health() -> Dict[str, Any]:
    await ensure_initialized()
    manifests = service.capabilities()
    return {
        "status": "ready" if manifests else "degraded",
        "contract_version": "1.0.0-alpha.1",
        "registered_provider_count": len(manifests),
        "diagnostics": _provider_diagnostics,
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


@router.post("/observe")
async def observe(body: ObserveRequest) -> Dict[str, Any]:
    await ensure_initialized()
    try:
        observation = await service.observe(
            body.provider_id,
            session_id=body.session_id,
            environment_id=body.environment_id,
            resource_id=body.resource_id,
        )
        _events.append(
            "observation.captured",
            session_id=body.session_id,
            state_id=observation.state_id,
            payload={
                "environment_id": body.environment_id,
                "resource_id": body.resource_id,
                "provider_id": body.provider_id,
                "epoch": observation.epoch,
                "image_artifact_id": observation.image.artifact_id if observation.image else None,
            },
        )
        return asdict(observation)
    except Exception as error:
        raise _http_error(error) from error


@router.post("/shadow/observe")
async def shadow_observe(body: ShadowObserveRequest) -> Dict[str, Any]:
    await ensure_initialized()
    try:
        primary = await service.observe(
            body.primary_provider_id, session_id=body.session_id,
            environment_id=body.environment_id, resource_id=body.primary_resource_id,
        )
        shadow = await service.observe(
            body.shadow_provider_id, session_id=body.session_id,
            environment_id=body.environment_id, resource_id=body.shadow_resource_id,
        )
        comparison = compare_observations(primary, shadow)
        _events.append(
            "shadow.observation.compared", session_id=body.session_id,
            payload=comparison, state_id=primary.state_id,
        )
        return comparison
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


@router.post("/roots")
async def find_roots(body: FindRootsRequest) -> Dict[str, Any]:
    await ensure_initialized()
    try:
        roots = await service.find_roots(
            session_id=body.session_id,
            environment_id=body.environment_id,
            provider_id=body.provider_id,
        )
        for discovered_provider_id, provider_roots in roots.items():
            _sessions.bind_roots(
                session_id=body.session_id, environment_id=body.environment_id,
                provider_id=discovered_provider_id, roots=provider_roots,
            )
        return {
            "session_id": body.session_id,
            "environment_id": body.environment_id,
            "providers": {
                provider_id: [asdict(root) for root in provider_roots]
                for provider_id, provider_roots in roots.items()
            },
            "diagnostics": service.root_discovery_diagnostics,
        }
    except Exception as error:
        raise _http_error(error) from error


@router.post("/transactions")
async def execute_transaction(body: TransactionRequest) -> Dict[str, Any]:
    await ensure_initialized()
    try:
        transaction = transaction_from_dict(body.transaction)
        policy = policy_engine.evaluate(transaction)
        _events.append(
            "transaction.started",
            session_id=transaction.session_id,
            state_id=transaction.base_state_id,
            transaction_id=transaction.transaction_id,
            payload={
                "provider_id": body.provider_id,
                "environment_id": transaction.environment_id,
                "resource_id": transaction.resource_id,
                "mode": transaction.mode,
                "step_count": len(transaction.steps),
                "policy": asdict(policy),
            },
        )
        approval = None
        if policy.requires_approval and not transaction.approval_id:
            raise PolicyDeniedError(
                f"Policy {policy.decision_id} requires an approval bound to this transaction"
            )
        if transaction.approval_id:
            approval = approval_authority.consume(transaction)
        try:
            outcome = await service.execute(body.provider_id, transaction)
        except Exception as execution_error:
            _events.append(
                "transaction.failed",
                session_id=transaction.session_id,
                state_id=transaction.base_state_id,
                transaction_id=transaction.transaction_id,
                payload={
                    "provider_id": body.provider_id,
                    "error_type": type(execution_error).__name__,
                    "message": str(execution_error),
                },
            )
            raise
        receipt = _receipts.append(
            transaction,
            outcome,
            provider_id=body.provider_id,
            approved_by=approval.approved_by if approval is not None else None,
        )
        _events.append(
            "transaction.completed",
            session_id=transaction.session_id,
            state_id=outcome.successor_state_id,
            transaction_id=transaction.transaction_id,
            payload={
                "provider_id": body.provider_id,
                "status": outcome.status,
                "stopped_at": outcome.stopped_at,
                "receipt_id": receipt.receipt_id,
                "approval_id": transaction.approval_id,
            },
        )
        response = asdict(outcome)
        response["receipt_id"] = receipt.receipt_id
        response["receipt"] = asdict(receipt)
        response["policy"] = asdict(policy)
        if approval is not None:
            response["approval"] = asdict(approval)
        return response
    except Exception as error:
        raise _http_error(error) from error


@router.post("/approvals")
async def grant_approval(body: ApprovalGrantRequest) -> Dict[str, Any]:
    await ensure_initialized()
    try:
        transaction = transaction_from_dict(body.transaction)
        policy = policy_engine.evaluate(transaction)
        grant = approval_authority.issue(
            transaction,
            approved_by=body.approved_by,
            ttl_seconds=body.ttl_seconds,
        )
        return {**asdict(grant), "policy": asdict(policy)}
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
    await service.close()
    _store.close()
    _receipts.close()
    _events.close()
    _sessions.close()
    _evaluations.close()
    _routing.close()
    _migrations.close()
