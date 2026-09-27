"""Canonical, provider-neutral contracts for Allternit computer use.

These models deliberately use only the Python standard library so native,
browser, sandbox, gateway, MCP, and test providers can share them without
pulling a transport or framework dependency into the trust core.
"""

from __future__ import annotations

from dataclasses import asdict, dataclass, field
from enum import Enum
from typing import Any, Dict, List, Optional
from uuid import uuid4


CONTRACT_VERSION = "1.0.0-alpha.1"
REQUIRED_INVARIANTS = (
    "immutable_observations",
    "state_scoped_refs",
    "stale_write_rejection",
    "honest_outcomes",
)


class OperatingSystem(str, Enum):
    MACOS = "macos"
    WINDOWS = "windows"
    LINUX = "linux"
    ANDROID = "android"


class Isolation(str, Enum):
    HOST = "host"
    CONTAINER = "container"
    VM = "vm"


class ExecutionMode(str, Enum):
    BACKGROUND_STRICT = "background_strict"
    FOREGROUND_ALLOWED = "foreground_allowed"
    SANDBOXED = "sandboxed"


class OutcomeStatus(str, Enum):
    WORKED = "worked"
    DIDNT = "didnt"
    UNKNOWN = "unknown"
    BLOCKED = "blocked"
    CANCELLED = "cancelled"


@dataclass(frozen=True)
class CapabilityManifest:
    provider_id: str
    provider_version: str
    contract_version: str = CONTRACT_VERSION
    invariant_version: str = CONTRACT_VERSION
    invariants: tuple[str, ...] = REQUIRED_INVARIANTS
    operating_systems: tuple[str, ...] = ()
    actions: tuple[str, ...] = ()
    observation_channels: tuple[str, ...] = ()
    execution_modes: tuple[str, ...] = ()
    tools: tuple[str, ...] = ()
    strict_background: bool = False
    semantic_input: bool = False
    raw_input: bool = False
    streaming: bool = False
    clipboard: bool = False
    shell: bool = False
    files: bool = False
    audio: bool = False
    mobile: bool = False
    max_concurrency: int = 1
    limitations: tuple[str, ...] = ()


@dataclass(frozen=True)
class ComputerEnvironment:
    environment_id: str
    provider_id: str
    os: str
    isolation: str
    state: str
    capabilities: CapabilityManifest
    image_digest: Optional[str] = None
    metadata: Dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True)
class Rect:
    x: float
    y: float
    width: float
    height: float


@dataclass(frozen=True)
class ElementNode:
    ref: str
    role: str
    name: str = ""
    value: str = ""
    description: str = ""
    bounds: Optional[Rect] = None
    states: tuple[str, ...] = ()
    actions: tuple[str, ...] = ()
    children: tuple["ElementNode", ...] = ()
    provider_metadata: Dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True)
class Root:
    root_id: str
    resource_id: str
    kind: str
    title: str = ""
    application: str = ""
    process_id: Optional[int] = None
    bounds: Optional[Rect] = None
    focused: bool = False


@dataclass(frozen=True)
class ImageEvidence:
    artifact_id: str
    media_type: str
    width: int
    height: int
    sha256: str
    coordinate_space: str = "screen"


@dataclass(frozen=True)
class Observation:
    state_id: str
    session_id: str
    environment_id: str
    resource_id: str
    epoch: int
    captured_at: str
    provider_id: str
    provider_version: str
    roots: tuple[Root, ...]
    elements: tuple[ElementNode, ...]
    image: Optional[ImageEvidence] = None
    truncated: bool = False
    metadata: Dict[str, Any] = field(default_factory=dict)

    @classmethod
    def create(cls, **values: Any) -> "Observation":
        return cls(state_id=f"state_{uuid4().hex}", **values)


@dataclass(frozen=True)
class ActionTarget:
    ref: Optional[str] = None
    x: Optional[float] = None
    y: Optional[float] = None
    root_id: Optional[str] = None


@dataclass(frozen=True)
class ActionStep:
    action: str
    target: Optional[ActionTarget] = None
    arguments: Dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True)
class Postcondition:
    kind: str
    value: str
    gone: bool = False
    timeout_ms: int = 3000


@dataclass(frozen=True)
class ActionTransaction:
    transaction_id: str
    session_id: str
    environment_id: str
    resource_id: str
    base_state_id: str
    mode: str
    steps: tuple[ActionStep, ...]
    postcondition: Optional[Postcondition] = None
    approval_id: Optional[str] = None

    @classmethod
    def create(cls, **values: Any) -> "ActionTransaction":
        return cls(transaction_id=f"tx_{uuid4().hex}", **values)


@dataclass(frozen=True)
class ActionEvidence:
    grounding: str = "unknown"
    delivery: str = "unknown"
    details: Dict[str, Any] = field(default_factory=dict)
    artifact_ids: tuple[str, ...] = ()


@dataclass(frozen=True)
class StepOutcome:
    index: int
    status: str
    evidence: ActionEvidence
    error_code: Optional[str] = None
    message: Optional[str] = None


@dataclass(frozen=True)
class TransactionOutcome:
    transaction_id: str
    status: str
    step_outcomes: tuple[StepOutcome, ...]
    stopped_at: Optional[int]
    successor_state_id: Optional[str]
    receipt_id: Optional[str] = None
    policy_decision_id: Optional[str] = None
    duration_ms: Optional[int] = None
    metadata: Dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True)
class ComputerEvent:
    event_id: str
    event_type: str
    occurred_at: str
    session_id: str
    payload: Dict[str, Any]
    run_id: Optional[str] = None
    state_id: Optional[str] = None
    transaction_id: Optional[str] = None
    trace_id: Optional[str] = None
    contract_version: str = CONTRACT_VERSION


def to_dict(value: Any) -> Dict[str, Any]:
    """Serialize a canonical dataclass while preserving enum string values."""
    return asdict(value)

