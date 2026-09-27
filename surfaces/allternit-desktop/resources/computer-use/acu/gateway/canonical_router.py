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
from core.environment_authority import EnvironmentConflictError
from core.environment_backends import (
    HostAdapterGate,
    default_environment_authority,
    default_environment_backend_service,
)
from core.native_capabilities import native_capability_payload, native_permission_request_plan
from core.canonical_policy import CanonicalPolicyEngine, PolicyDeniedError
from core.trajectory_export import export_trajectory
from core.image_scanner import scan_image
from core.session_authority import SessionAuthority
from core.evaluation_authority import EvaluationAuthority, SUITES
from core.canonical_replay import build_time_aligned_mp4
from core.shadow_comparison import compare_observations
from core.routing_authority import RoutingAuthority
from core.operation_approval import OperationApprovalAuthority
from core.secret_broker import SecretBroker
from core.legacy_migration import LegacyMigrationService
from core.benchmark_adapters import benchmark_adapter_statuses
from core.persistent_observation_store import SQLiteObservationStore
from providers.playwright_canonical import PlaywrightCanonicalProvider
from providers.accessibility_canonical import AccessibilityCanonicalProvider
from providers.cua_driver_canonical import CuaDriverCanonicalProvider
from providers.cua_driver_transport import CuaDriverTransport
from providers.agent_desktop_canonical import AgentDesktopCanonicalProvider
from providers.agent_desktop_transport import AgentDesktopTransport
from providers.cdp_canonical import CDPCanonicalProvider
from providers.extension_canonical import ExtensionCanonicalProvider
from providers.droidrun_canonical import DroidRunCanonicalProvider
from providers.phone_harness_canonical import PhoneHarnessCanonicalProvider
try:
    from .session_manager import session_manager
except ImportError:  # Legacy direct-script gateway launch.
    from session_manager import session_manager


router = APIRouter(prefix="/v1/computer-use/canonical", tags=["computer-use-canonical"])

_state_dir = Path(os.environ.get("ALLTERNIT_COMPUTER_STATE_DIR", "~/.allternit/computer-use")).expanduser()
_state_dir.mkdir(parents=True, exist_ok=True)
_store = SQLiteObservationStore(_state_dir / "observations.sqlite3")
_receipts = ReceiptLedger(_state_dir / "receipts.sqlite3")
_events = EventLedger(_state_dir / "events.sqlite3")
_environments = default_environment_authority()
_environment_backends = default_environment_backend_service()
_sessions = SessionAuthority(_state_dir / "sessions.sqlite3")
_evaluations = EvaluationAuthority(_state_dir / "evaluations.sqlite3")
_routing = RoutingAuthority(_state_dir / "routing.sqlite3", _evaluations)
_secrets = SecretBroker()
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
operation_approvals = OperationApprovalAuthority(
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
    await service.register(
        PlaywrightCanonicalProvider(
            session_manager,
            _state_dir / "artifacts",
        )
    )
    _provider_diagnostics["browser.playwright.canonical"] = {"available": True}
    try:
        from adapters.desktop.accessibility_adapter import AccessibilityAdapter
        gated_adapter = HostAdapterGate(
            AccessibilityAdapter(),
            authority=_environments,
            backend=_environment_backends.backend("allternit.host"),
        )
        await service.register(
            AccessibilityCanonicalProvider(
                gated_adapter,
                _state_dir / "artifacts",
            )
        )
        _provider_diagnostics["desktop.accessibility.canonical"] = {"available": True}
    except Exception:
        # Native dependencies and interactive permissions are optional. The
        # provider registry remains truthful by omitting an unavailable route.
        _provider_diagnostics["desktop.accessibility.canonical"] = {
            "available": False,
            "reason": "native_dependencies_or_permissions_unavailable",
        }
    try:
        from adapters.browser.cdp_adapter import PlaywrightCDPAdapter
        cdp_adapter = PlaywrightCDPAdapter()
        if await cdp_adapter.health_check():
            await service.register(
                CDPCanonicalProvider(cdp_adapter, _state_dir / "artifacts")
            )
            _provider_diagnostics["browser.cdp.canonical"] = {"available": True}
        else:
            _provider_diagnostics["browser.cdp.canonical"] = {
                "available": False,
                "reason": "cdp_endpoint_unavailable",
            }
    except Exception as error:
        _provider_diagnostics["browser.cdp.canonical"] = {
            "available": False,
            "reason": "cdp_discovery_failed",
            "message": str(error),
        }
    try:
        from adapters.browser.extension_adapter import ExtensionAdapter
        extension_adapter = ExtensionAdapter()
        try:
            await extension_adapter.initialize()
            if await extension_adapter.health_check():
                await service.register(
                    ExtensionCanonicalProvider(extension_adapter, _state_dir / "artifacts")
                )
                _provider_diagnostics["browser.extension.canonical"] = {"available": True}
            else:
                await extension_adapter.close()
                _provider_diagnostics["browser.extension.canonical"] = {
                    "available": False,
                    "reason": "extension_relay_unavailable",
                }
        except Exception:
            await extension_adapter.close()
            raise
    except Exception as error:
        _provider_diagnostics["browser.extension.canonical"] = {
            "available": False,
            "reason": "extension_discovery_failed",
            "message": str(error),
        }
    try:
        phone_harness_provider = PhoneHarnessCanonicalProvider(_state_dir / "artifacts")
        if await phone_harness_provider._driver.health_check():
            await service.register(phone_harness_provider)
            _provider_diagnostics["mobile.phone.canonical"] = {"available": True, "driver": phone_harness_provider._driver.name}
        else:
            # Register the provider anyway so the manifest is visible, but mark it
            # as waiting on a connected iOS device / macOS mirroring session.
            await service.register(phone_harness_provider)
            _provider_diagnostics["mobile.phone.canonical"] = {
                "available": False,
                "reason": "ios_device_or_macos_mirroring_unavailable",
                "driver": phone_harness_provider._driver.name,
            }
    except Exception as error:
        _provider_diagnostics["mobile.phone.canonical"] = {
            "available": False,
            "reason": "phone_harness_registration_failed",
            "message": str(error),
        }
    try:
        transport = await CuaDriverTransport.discover()
        if transport is None:
            _provider_diagnostics["desktop.cua-driver"] = {
                "available": False,
                "reason": "cua_driver_not_installed",
            }
        else:
            installation = await transport.manifest()
            await service.register(
                CuaDriverCanonicalProvider(
                    transport,
                    _state_dir / "artifacts",
                    version=installation.version,
                )
            )
            _provider_diagnostics["desktop.cua-driver"] = {
                "available": True,
                "executable": installation.executable,
                "version": installation.version,
                "telemetry_managed_by_allternit": True,
                "telemetry_enabled": os.environ.get("ALLTERNIT_ALLOW_UPSTREAM_TELEMETRY", "").strip().lower()
                in {"1", "true", "yes", "on"},
            }
    except Exception as error:
        _provider_diagnostics["desktop.cua-driver"] = {
            "available": False,
            "reason": "cua_driver_discovery_failed",
            "message": str(error),
        }
    try:
        transport = await AgentDesktopTransport.discover()
        if transport is None:
            _provider_diagnostics["desktop.agent-desktop.canonical"] = {
                "available": False,
                "reason": "agent_desktop_not_installed",
            }
        else:
            installation = await transport.version()
            version = (
                installation.get("data", {}).get("version", "unknown")
                if isinstance(installation, dict)
                else "unknown"
            )
            await service.register(
                AgentDesktopCanonicalProvider(
                    transport,
                    _state_dir / "artifacts",
                    version=version,
                    authority=_environments,
                    backend=_environment_backends.backend("allternit.host"),
                )
            )
            _provider_diagnostics["desktop.agent-desktop.canonical"] = {
                "available": True,
                "executable": transport.executable,
                "version": version,
            }
    except Exception as error:
        _provider_diagnostics["desktop.agent-desktop.canonical"] = {
            "available": False,
            "reason": "agent_desktop_discovery_failed",
            "message": str(error),
        }
    try:
        droidrun_provider = DroidRunCanonicalProvider()
        await service.register(droidrun_provider)
        _environment_backends.register(droidrun_provider)
        _provider_diagnostics["mobile.droidrun.canonical"] = {
            "available": True,
            "note": "registration succeeded; operational only when mobilerun_core and adb are available",
        }
    except Exception as error:
        _provider_diagnostics["mobile.droidrun.canonical"] = {
            "available": False,
            "reason": "droidrun_registration_failed",
            "message": str(error),
        }


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


class ImageRegistrationRequest(BaseModel):
    source: str
    os: str
    architecture: str
    digest: str
    provenance: Dict[str, Any] = Field(default_factory=dict)
    scan_status: str = "pending"


class EnvironmentCreateRequest(BaseModel):
    owner_id: str
    provider_id: str
    os: str
    isolation: str
    image_digest: Optional[str] = None
    ttl_seconds: Optional[int] = Field(default=None, ge=60, le=604800)
    metadata: Dict[str, Any] = Field(default_factory=dict)


class EnvironmentPoolCreateRequest(BaseModel):
    owner_id: str
    provider_id: str
    os: str
    isolation: str
    image_digest: Optional[str] = None
    maximum_size: int = Field(default=1, ge=1, le=8)
    metadata: Dict[str, Any] = Field(default_factory=dict)


class EnvironmentPoolAllocateRequest(BaseModel):
    ttl_seconds: Optional[int] = Field(default=None, ge=60, le=604800)


class LeaseAcquireRequest(BaseModel):
    holder_id: str
    kind: str
    ttl_seconds: int = Field(default=300, ge=1, le=3600)


class LeaseReleaseRequest(BaseModel):
    holder_id: str


class SnapshotCreateRequest(BaseModel):
    name: Optional[str] = None
    stateful: bool = False
    parent_snapshot_id: Optional[str] = None
    metadata: Dict[str, Any] = Field(default_factory=dict)
    lease_id: str
    holder_id: str
    approval_id: str


class SnapshotCloneRequest(BaseModel):
    owner_id: str
    ttl_seconds: Optional[int] = Field(default=None, ge=60, le=604800)


class EnvironmentCommandRequest(BaseModel):
    command: list[str]
    env: Dict[str, str] = Field(default_factory=dict)
    secret_refs: Dict[str, str] = Field(default_factory=dict)
    lease_id: str
    holder_id: str
    approval_id: str


class EnvironmentControlRequest(BaseModel):
    lease_id: str
    holder_id: str
    approval_id: str


class EnvironmentFileRequest(BaseModel):
    path: str
    lease_id: str
    holder_id: str
    content: Optional[str] = None
    approval_id: Optional[str] = None


class EnvironmentClipboardRequest(BaseModel):
    lease_id: str
    holder_id: str
    text: Optional[str] = None
    approval_id: Optional[str] = None


class MobileActionRequest(BaseModel):
    action: str
    arguments: Dict[str, Any] = Field(default_factory=dict)
    lease_id: str
    holder_id: str
    approval_id: str


class OperationApprovalRequest(BaseModel):
    environment_id: str
    holder_id: str
    operation: str
    payload: Dict[str, Any]
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


class HistoryStatusRequest(BaseModel):
    provider_id: str = "desktop.cua-driver"


class HistoryQueryRequest(BaseModel):
    provider_id: str = "desktop.cua-driver"
    limit: Optional[int] = Field(default=None, ge=1, le=200)
    session_id: Optional[str] = Field(default=None, min_length=1, max_length=128)
    since_sequence: Optional[int] = Field(default=None, ge=1)
    until_sequence: Optional[int] = Field(default=None, ge=1)


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
    if isinstance(error, EnvironmentConflictError):
        return HTTPException(status_code=409, detail={"code": "environment_conflict", "message": str(error)})
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


@router.post("/images")
async def register_image(body: ImageRegistrationRequest) -> Dict[str, Any]:
    try:
        return asdict(_environments.register_image(
            source=body.source, os=body.os, architecture=body.architecture,
            content_digest=body.digest, provenance=body.provenance,
            scan_status=body.scan_status,
        ))
    except Exception as error:
        raise _http_error(error) from error


@router.post("/images/{digest}/scan")
async def scan_registered_image(digest: str) -> Dict[str, Any]:
    try:
        image = _environments.image(digest)
        result = await scan_image(image.source)
        updated = _environments.attest_image_scan(digest, result.status, asdict(result))
        return {"image": asdict(updated), "scan": asdict(result)}
    except Exception as error:
        raise _http_error(error) from error


@router.post("/environments")
async def create_environment(body: EnvironmentCreateRequest) -> Dict[str, Any]:
    try:
        record = _environments.create_environment(
            owner_id=body.owner_id, provider_id=body.provider_id, os=body.os,
            isolation=body.isolation, image_digest=body.image_digest,
            ttl_seconds=body.ttl_seconds, metadata=body.metadata,
        )
        _events.append("environment.requested", session_id=body.owner_id, payload=asdict(record))
        return asdict(record)
    except Exception as error:
        raise _http_error(error) from error


@router.post("/environment-pools")
async def create_environment_pool(body: EnvironmentPoolCreateRequest) -> Dict[str, Any]:
    try:
        return asdict(_environments.create_pool(**body.model_dump()))
    except Exception as error:
        raise _http_error(error) from error


@router.post("/environment-pools/{pool_id}/allocate")
async def allocate_environment_pool(pool_id: str, body: EnvironmentPoolAllocateRequest) -> Dict[str, Any]:
    try:
        return asdict(_environments.allocate_from_pool(pool_id, ttl_seconds=body.ttl_seconds))
    except Exception as error:
        raise _http_error(error) from error


@router.get("/environments")
async def list_environments(owner_id: Optional[str] = None) -> Dict[str, Any]:
    return {"environments": [asdict(item) for item in _environments.list_environments(owner_id)]}


@router.get("/environment-providers")
async def list_environment_providers() -> Dict[str, Any]:
    return {"providers": [asdict(item) for item in _environment_backends.manifests()]}


@router.get("/environments/{environment_id}")
async def get_environment(environment_id: str) -> Dict[str, Any]:
    try:
        return asdict(_environments.get_environment(environment_id))
    except Exception as error:
        raise _http_error(error) from error


@router.post("/environments/{environment_id}/provision")
async def provision_environment(environment_id: str, body: EnvironmentControlRequest) -> Dict[str, Any]:
    try:
        _environments.require_lease(environment_id, body.lease_id, body.holder_id)
        operation_approvals.consume(
            body.approval_id, environment_id=environment_id, holder_id=body.holder_id,
            operation="environment.provision", payload={},
        )
        record = await _environment_backends.provision(environment_id)
        _events.append("environment.running", session_id=record.owner_id, payload=asdict(record))
        return asdict(record)
    except Exception as error:
        raise _http_error(error) from error


@router.post("/environments/{environment_id}/stop")
async def stop_environment(environment_id: str, body: EnvironmentControlRequest) -> Dict[str, Any]:
    try:
        _environments.require_lease(environment_id, body.lease_id, body.holder_id)
        operation_approvals.consume(
            body.approval_id, environment_id=environment_id, holder_id=body.holder_id,
            operation="environment.stop", payload={},
        )
        record = await _environment_backends.stop(environment_id)
        _events.append("environment.stopped", session_id=record.owner_id, payload=asdict(record))
        return asdict(record)
    except Exception as error:
        raise _http_error(error) from error


@router.post("/environments/{environment_id}/exec")
async def execute_environment_command(environment_id: str, body: EnvironmentCommandRequest) -> Dict[str, Any]:
    try:
        lease = _environments.require_lease(environment_id, body.lease_id, body.holder_id)
        if lease.kind != "agent":
            raise EnvironmentConflictError("Shell execution is suspended during human takeover")
        operation_approvals.consume(
            body.approval_id, environment_id=environment_id, holder_id=body.holder_id,
            operation="shell", payload={
                "command": body.command, "env": body.env, "secret_refs": body.secret_refs,
            },
        )
        resolved_env = {**body.env, **_secrets.resolve(body.secret_refs)}
        result = await _environment_backends.execute(environment_id, body.command, resolved_env)
        _events.append(
            "environment.command.completed", session_id=body.holder_id,
            payload={
                "environment_id": environment_id, "lease_id": body.lease_id,
                "argv": body.command, "secret_refs": sorted(body.secret_refs),
                "exit_code": result.get("exit_code"),
            },
        )
        return result
    except Exception as error:
        raise _http_error(error) from error


def _require_agent_lease(environment_id: str, lease_id: str, holder_id: str) -> None:
    lease = _environments.require_lease(environment_id, lease_id, holder_id)
    if lease.kind != "agent":
        raise EnvironmentConflictError("Agent operations are suspended during human takeover")


@router.post("/environments/{environment_id}/files/read")
async def read_environment_file(environment_id: str, body: EnvironmentFileRequest) -> Dict[str, Any]:
    try:
        _require_agent_lease(environment_id, body.lease_id, body.holder_id)
        content = await _environment_backends.provider_operation(environment_id, "read_text", body.path)
        return {"path": body.path, "content": content}
    except Exception as error:
        raise _http_error(error) from error


@router.post("/environments/{environment_id}/files/write")
async def write_environment_file(environment_id: str, body: EnvironmentFileRequest) -> Dict[str, Any]:
    try:
        _require_agent_lease(environment_id, body.lease_id, body.holder_id)
        if body.content is None:
            raise ValueError("File content is required")
        if len(body.content.encode("utf-8")) > 1024 * 1024:
            raise ValueError("Text file writes are limited to 1 MiB per operation")
        if not body.approval_id:
            raise ValueError("A bound operation approval is required")
        operation_approvals.consume(
            body.approval_id, environment_id=environment_id, holder_id=body.holder_id,
            operation="file.write", payload={"path": body.path, "content": body.content},
        )
        await _environment_backends.provider_operation(environment_id, "write_text", body.path, body.content)
        return {"path": body.path, "written": True}
    except Exception as error:
        raise _http_error(error) from error


@router.post("/environments/{environment_id}/files/list")
async def list_environment_files(environment_id: str, body: EnvironmentFileRequest) -> Dict[str, Any]:
    try:
        _require_agent_lease(environment_id, body.lease_id, body.holder_id)
        entries = await _environment_backends.provider_operation(environment_id, "list_files", body.path)
        return {"path": body.path, "entries": entries}
    except Exception as error:
        raise _http_error(error) from error


@router.post("/environments/{environment_id}/clipboard")
async def environment_clipboard(environment_id: str, body: EnvironmentClipboardRequest) -> Dict[str, Any]:
    try:
        _require_agent_lease(environment_id, body.lease_id, body.holder_id)
        if body.text is None:
            return {"text": await _environment_backends.provider_operation(environment_id, "clipboard_get")}
        if len(body.text.encode("utf-8")) > 1024 * 1024:
            raise ValueError("Clipboard writes are limited to 1 MiB per operation")
        if not body.approval_id:
            raise ValueError("A bound operation approval is required")
        operation_approvals.consume(
            body.approval_id, environment_id=environment_id, holder_id=body.holder_id,
            operation="clipboard.write", payload={"text": body.text},
        )
        await _environment_backends.provider_operation(environment_id, "clipboard_set", body.text)
        return {"written": True}
    except Exception as error:
        raise _http_error(error) from error


@router.post("/environments/{environment_id}/artifacts/export")
async def export_environment_artifact(environment_id: str, body: EnvironmentFileRequest) -> Dict[str, Any]:
    try:
        _require_agent_lease(environment_id, body.lease_id, body.holder_id)
        artifact_id = f"artifact_{secrets.token_hex(16)}"
        output_path = _state_dir / "artifacts" / artifact_id
        result = await _environment_backends.provider_operation(
            environment_id, "export_artifact", body.path, output_path, 100 * 1024 * 1024,
        )
        return {"artifact_id": artifact_id, **result}
    except Exception as error:
        raise _http_error(error) from error


@router.post("/environments/{environment_id}/mobile/actions")
async def execute_mobile_action(environment_id: str, body: MobileActionRequest) -> Dict[str, Any]:
    try:
        _require_agent_lease(environment_id, body.lease_id, body.holder_id)
        environment = _environments.get_environment(environment_id)
        operation_approvals.consume(
            body.approval_id, environment_id=environment_id, holder_id=body.holder_id,
            operation=f"mobile.{body.action}", payload=body.arguments,
        )
        if environment.os == "android":
            await _environment_backends.provider_operation(
                environment_id, "mobile_action", body.action, body.arguments,
            )
            return {"action": body.action, "delivered": True, "verified": False}
        if environment.os == "ios":
            await ensure_initialized()
            phone_provider = service.provider("mobile.phone.canonical")
            result = await phone_provider.mobile_action(body.action, body.arguments)
            return {"action": body.action, "delivered": True, "verified": False, **result}
        raise ValueError(f"Mobile actions not supported for environment os={environment.os!r}")
    except Exception as error:
        raise _http_error(error) from error


@router.post("/operation-approvals")
async def grant_operation_approval(body: OperationApprovalRequest) -> Dict[str, Any]:
    try:
        _environments.require_lease(body.environment_id, body.payload.get("lease_id", ""), body.holder_id)
        grant = operation_approvals.issue(
            environment_id=body.environment_id, holder_id=body.holder_id,
            operation=body.operation,
            payload={key: value for key, value in body.payload.items() if key != "lease_id"},
            approved_by=body.approved_by, ttl_seconds=body.ttl_seconds,
        )
        return asdict(grant)
    except Exception as error:
        raise _http_error(error) from error


@router.post("/environments/{environment_id}/leases")
async def acquire_environment_lease(environment_id: str, body: LeaseAcquireRequest) -> Dict[str, Any]:
    try:
        return asdict(_environments.acquire_lease(
            environment_id, holder_id=body.holder_id, kind=body.kind, ttl_seconds=body.ttl_seconds,
        ))
    except Exception as error:
        raise _http_error(error) from error


@router.post("/leases/{lease_id}/release")
async def release_environment_lease(lease_id: str, body: LeaseReleaseRequest) -> Dict[str, Any]:
    try:
        _environments.release_lease(lease_id, body.holder_id)
        return {"lease_id": lease_id, "released": True}
    except Exception as error:
        raise _http_error(error) from error


@router.post("/environments/{environment_id}/snapshots")
async def create_environment_snapshot(environment_id: str, body: SnapshotCreateRequest) -> Dict[str, Any]:
    try:
        _require_agent_lease(environment_id, body.lease_id, body.holder_id)
        operation_approvals.consume(
            body.approval_id, environment_id=environment_id, holder_id=body.holder_id,
            operation="environment.snapshot", payload={"name": body.name, "stateful": body.stateful},
        )
        snapshot_spec = await _environment_backends.provider_operation(
            environment_id, "create_snapshot", body.name, body.stateful,
        )
        return asdict(_environments.create_snapshot(
            environment_id,
            backend_reference=json.dumps(snapshot_spec, sort_keys=True, separators=(",", ":")),
            parent_snapshot_id=body.parent_snapshot_id, metadata=body.metadata,
        ))
    except Exception as error:
        raise _http_error(error) from error


@router.get("/environments/{environment_id}/snapshots")
async def list_environment_snapshots(environment_id: str) -> Dict[str, Any]:
    return {"snapshots": [asdict(item) for item in _environments.list_snapshots(environment_id)]}


@router.post("/snapshots/{snapshot_id}/clone")
async def clone_environment_snapshot(snapshot_id: str, body: SnapshotCloneRequest) -> Dict[str, Any]:
    try:
        return asdict(_environments.clone_from_snapshot(
            snapshot_id, owner_id=body.owner_id, ttl_seconds=body.ttl_seconds,
        ))
    except Exception as error:
        raise _http_error(error) from error


@router.post("/environments/cleanup")
async def cleanup_expired_environments() -> Dict[str, int]:
    return _environments.cleanup_expired()


def _cua_history_provider(provider_id: str) -> CuaDriverCanonicalProvider:
    """Return a CUA driver provider if it advertises history tools."""
    try:
        provider = service.provider(provider_id)
    except ProviderNotFoundError as error:
        raise _http_error(error) from error
    manifest = next((m for m in service.capabilities() if m.provider_id == provider_id), None)
    if manifest is None or "history_status" not in manifest.tools or "history_query" not in manifest.tools:
        raise HTTPException(
            status_code=404,
            detail={
                "code": "history_not_available",
                "message": "CUA Driver Computer History is not available or not admitted for this provider",
                "provider_id": provider_id,
            },
        )
    if not isinstance(provider, CuaDriverCanonicalProvider):
        raise HTTPException(
            status_code=400,
            detail={
                "code": "provider_not_cua_driver",
                "message": "History is only supported by the CUA driver provider",
                "provider_id": provider_id,
            },
        )
    return provider


@router.post("/history/status")
async def history_status(body: HistoryStatusRequest) -> Dict[str, Any]:
    await ensure_initialized()
    try:
        provider = _cua_history_provider(body.provider_id)
        return await provider.history_status()
    except HTTPException:
        raise
    except Exception as error:
        raise _http_error(error) from error


@router.post("/history/query")
async def history_query(body: HistoryQueryRequest) -> Dict[str, Any]:
    await ensure_initialized()
    try:
        provider = _cua_history_provider(body.provider_id)
        return await provider.history_query(
            limit=body.limit,
            session_id=body.session_id,
            since_sequence=body.since_sequence,
            until_sequence=body.until_sequence,
        )
    except HTTPException:
        raise
    except Exception as error:
        raise _http_error(error) from error


async def shutdown_canonical_service() -> None:
    await service.close()
    _store.close()
    _receipts.close()
    _events.close()
    _environments.close()
    _sessions.close()
    _evaluations.close()
    _routing.close()
    _migrations.close()
