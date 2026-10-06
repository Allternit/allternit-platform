import asyncio
import base64
import datetime
import hashlib
import json
import logging
import os
import sys
import uuid

import jsonschema
from pathlib import Path
from typing import Dict, Any, Optional, Iterable, List
from fastapi import FastAPI, Request, HTTPException, Response
from fastapi.responses import JSONResponse, StreamingResponse
from fastapi.middleware.cors import CORSMiddleware
import uvicorn
import httpx
import time
from pydantic import BaseModel

# Configure logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(name)s - %(levelname)s - %(message)s')
logger = logging.getLogger("gateway-runtime")

THIS_FILE = Path(__file__).resolve()
REPO_ROOT = THIS_FILE.parents[4]


def _first_existing_path(candidates: Iterable[Path]) -> Path:
    for candidate in candidates:
        if candidate.exists():
            return candidate
    return next(iter(candidates))


REGISTRY_PATH = _first_existing_path([
    REPO_ROOT / "infrastructure/gateway/gateway_registry.json",
    REPO_ROOT / "infra/gateway/gateway_registry.json",
])
REGISTRY_SCHEMA_PATH = _first_existing_path([
    REPO_ROOT / "spec/Contracts/GatewayRegistry.schema.json",
])
RECEIPT_SCHEMA_PATH = _first_existing_path([
    REPO_ROOT / "spec/Contracts/GatewayRoutingReceipt.schema.json",
])
UI_REGISTRY_PATH = _first_existing_path([
    REPO_ROOT / "ui/ui_registry.json",
])
UI_ACTION_SCHEMA_PATH = _first_existing_path([
    REPO_ROOT / "spec/Contracts/UIAction.schema.json",
])
UI_RECEIPT_SCHEMA_PATH = _first_existing_path([
    REPO_ROOT / "spec/Contracts/UIReceipt.schema.json",
])
UI_REGISTRY_SCHEMA_PATH = _first_existing_path([
    REPO_ROOT / "spec/Contracts/UIRegistry.schema.json",
])
GIZZI_DEV_SESSION_PATH = Path.home() / ".allternit" / "gizzi-dev-session.json"
AGENT_SESSIONS_STORE_PATH = Path.home() / ".allternit" / "gateway-agent-sessions.json"
AGENTS_STORE_PATH = Path.home() / ".allternit" / "gateway-agents.json"
OFFICE_RUNTIME_STORE_PATH = Path.home() / ".allternit" / "gateway-office-runtime.json"
AGENT_WORKSPACES_DIR = Path.home() / ".allternit" / "gateway-agent-workspaces"
COMPAT_HTTP_EXACT_PATHS = {
    "/global/event",
    "/global/health",
    "/session/list",
    "/session/status",
    "/sessions/sync",
    "/v1/events-http",
    "/v1/global/event",
    "/v1/global/health",
    "/v1/session/list",
    "/v1/session/status",
    "/api/v1/agent-sessions/sync",
    "/api/v1/sessions/sync",
}
COMPAT_HTTP_PREFIXES = (
    "/session/",
    "/v1/session/",
)


def is_compat_http_path(path: str) -> bool:
    return path in COMPAT_HTTP_EXACT_PATHS or any(path.startswith(prefix) for prefix in COMPAT_HTTP_PREFIXES)


def load_gizzi_dev_session() -> Dict[str, Any]:
    if not GIZZI_DEV_SESSION_PATH.exists():
        return {}
    try:
        with open(GIZZI_DEV_SESSION_PATH, "r") as f:
            return json.load(f)
    except Exception as e:
        logger.warning(f"Failed to read gizzi dev session credentials: {e}")
        return {}


def get_gizzi_base_url() -> str:
    session = load_gizzi_dev_session()
    return (
        os.environ.get("GIZZI_RUNTIME_URL")
        or os.environ.get("TERMINAL_SERVER_URL")
        or session.get("gizziUrl")
        or "http://127.0.0.1:4096"
    ).rstrip("/")


def get_gizzi_auth_header() -> Optional[str]:
    username = (
        os.environ.get("GIZZI_USERNAME")
        or os.environ.get("NEXT_PUBLIC_GIZZI_USERNAME")
        or "gizzi"
    )
    session = load_gizzi_dev_session()
    password = (
        os.environ.get("GIZZI_PASSWORD")
        or os.environ.get("NEXT_PUBLIC_GIZZI_PASSWORD")
        or session.get("gizziPassword")
    )
    if not password:
        return None
    token = base64.b64encode(f"{username}:{password}".encode("utf-8")).decode("ascii")
    return f"Basic {token}"


def build_gizzi_headers(extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, str]:
    headers: Dict[str, str] = {}
    auth_header = get_gizzi_auth_header()
    if auth_header:
        headers["Authorization"] = auth_header
    if extra_headers:
        headers.update(extra_headers)
    return headers


def build_upstream_url(base_url: str, path: str, query: str = "") -> str:
    target = f"{base_url.rstrip('/')}{path}"
    if query:
        target += f"?{query}"
    return target


def apply_cors_headers(response: Response, request: Optional[Request] = None) -> Response:
    origin = request.headers.get("origin") if request else None
    response.headers["Access-Control-Allow-Origin"] = origin or "*"
    response.headers["Access-Control-Allow-Methods"] = "*"
    response.headers["Access-Control-Allow-Headers"] = "*"
    response.headers["Vary"] = "Origin"
    return response


def rewrite_provider_path(path: str) -> str:
    if path == "/api/v1/providers":
        return "/provider"
    if path.startswith("/api/v1/providers/"):
        suffix = path[len("/api/v1/providers"):]
        if suffix == "/auth/status":
            return "/provider/auth"
        if suffix.endswith("/auth/status"):
            return f"/provider{suffix[:-len('/status')]}"
        return f"/provider{suffix}"
    return path


AGENT_SESSION_LOCAL_PREFIXES = (
    "/api/v1/agent-sessions",
    "/api/v1/agents",
    "/api/v1/openclaw/agents/discovery",
    "/api/v1/office",
)


def is_local_gateway_route(path: str) -> bool:
    return path.startswith(AGENT_SESSION_LOCAL_PREFIXES)


def utc_now_iso() -> str:
    return datetime.datetime.utcnow().isoformat() + "Z"


def _normalize_session_metadata(metadata: Optional[Dict[str, Any]]) -> Dict[str, Any]:
    metadata = dict(metadata or {})
    normalized: Dict[str, Any] = {}

    alias_map = {
        "allternit_origin_surface": "originSurface",
        "allternit_session_mode": "sessionMode",
        "allternit_agent_id": "agentId",
        "allternit_agent_name": "agentName",
        "allternit_project_id": "projectId",
        "allternit_workspace_scope": "workspaceScope",
        "allternit_agent_features": "agentFeatures",
        "allternit_runtime_model": "runtimeModel",
    }

    for key, value in metadata.items():
        normalized[alias_map.get(key, key)] = value

    return normalized


class CreateAgentSessionRequest(BaseModel):
    name: Optional[str] = None
    description: Optional[str] = None
    agent_id: Optional[str] = None
    agent_name: Optional[str] = None
    model: Optional[str] = None
    tags: Optional[List[str]] = None
    metadata: Optional[Dict[str, Any]] = None
    origin_surface: Optional[str] = None
    session_mode: Optional[str] = None
    project_id: Optional[str] = None
    workspace_scope: Optional[str] = None
    agent_features: Optional[Dict[str, Any]] = None


class UpdateAgentSessionRequest(BaseModel):
    name: Optional[str] = None
    description: Optional[str] = None
    active: Optional[bool] = None
    tags: Optional[List[str]] = None
    metadata: Optional[Dict[str, Any]] = None
    origin_surface: Optional[str] = None
    session_mode: Optional[str] = None
    project_id: Optional[str] = None
    workspace_scope: Optional[str] = None
    agent_features: Optional[Dict[str, Any]] = None


class AgentSessionMessageRequest(BaseModel):
    text: str
    role: Optional[str] = None


class AbortAgentSessionRequest(BaseModel):
    reason: Optional[str] = None


class AgentVoiceRequest(BaseModel):
    voice_id: Optional[str] = None
    voice_label: Optional[str] = None
    engine: Optional[str] = None
    enabled: bool = False
    auto_speak: Optional[bool] = None
    speak_on_checkpoint: Optional[bool] = None


class CreateAgentRequest(BaseModel):
    name: str
    description: str = ""
    agent_type: Optional[str] = None
    parent_agent_id: Optional[str] = None
    model: str
    provider: str
    capabilities: Optional[List[str]] = None
    system_prompt: Optional[str] = None
    tools: Optional[List[str]] = None
    max_iterations: Optional[int] = None
    temperature: Optional[float] = None
    voice: Optional[AgentVoiceRequest] = None
    config: Optional[Dict[str, Any]] = None
    workspace_id: Optional[str] = None
    owner_id: Optional[str] = None


class UpdateAgentRequest(BaseModel):
    name: Optional[str] = None
    description: Optional[str] = None
    agent_type: Optional[str] = None
    parent_agent_id: Optional[str] = None
    model: Optional[str] = None
    provider: Optional[str] = None
    capabilities: Optional[List[str]] = None
    system_prompt: Optional[str] = None
    tools: Optional[List[str]] = None
    max_iterations: Optional[int] = None
    temperature: Optional[float] = None
    voice: Optional[AgentVoiceRequest] = None
    config: Optional[Dict[str, Any]] = None
    workspace_id: Optional[str] = None
    owner_id: Optional[str] = None
    status: Optional[str] = None


class InitializeWorkspaceRequest(BaseModel):
    documents: Dict[str, Any]


class OfficeDocumentRequest(BaseModel):
    host: str
    title: Optional[str] = None
    label: Optional[str] = None
    summary: Optional[str] = None
    document_url: Optional[str] = None
    document_id: Optional[str] = None
    fingerprint: Optional[str] = None


class OfficePlatformRequest(BaseModel):
    taskpane_origin: Optional[str] = None
    taskpane_url: Optional[str] = None
    manifest_url: Optional[str] = None
    platform_origin: Optional[str] = None


class OfficeRuntimeStateRequest(BaseModel):
    status: Optional[str] = None
    page_label: Optional[str] = None
    current_task: Optional[str] = None
    history_count: Optional[int] = None
    connected: Optional[bool] = None


class OfficeBootstrapRequest(BaseModel):
    session_id: Optional[str] = None
    project_id: Optional[str] = None
    workspace_id: Optional[str] = None
    document: OfficeDocumentRequest
    platform: Optional[OfficePlatformRequest] = None
    runtime_state: Optional[OfficeRuntimeStateRequest] = None


class OfficeRuntimeSyncRequest(BaseModel):
    binding_id: str
    session_id: str
    project_id: Optional[str] = None
    workspace_id: Optional[str] = None
    document: Optional[OfficeDocumentRequest] = None
    platform: Optional[OfficePlatformRequest] = None
    runtime_state: Optional[OfficeRuntimeStateRequest] = None


class AgentSessionStore:
    def __init__(self, path: Path):
        self.path = path
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self._lock = asyncio.Lock()
        self._sessions: Dict[str, Dict[str, Any]] = {}
        self._subscribers: List[asyncio.Queue[str]] = []
        self._load_from_disk()

    def _load_from_disk(self) -> None:
        if not self.path.exists():
            self._sessions = {}
            return
        try:
            payload = json.loads(self.path.read_text())
            sessions = payload.get("sessions", [])
            self._sessions = {
                session["id"]: session
                for session in sessions
                if isinstance(session, dict) and session.get("id")
            }
        except Exception as e:
            logger.warning(f"Failed to load agent session store from {self.path}: {e}")
            self._sessions = {}

    def _persist(self) -> None:
        payload = {
            "sessions": list(self._sessions.values()),
            "updated_at": utc_now_iso(),
        }
        self.path.write_text(json.dumps(payload, indent=2))

    def _emit_nowait(self, event: Dict[str, Any]) -> None:
        message = json.dumps(event)
        stale: List[asyncio.Queue[str]] = []
        for queue in self._subscribers:
            try:
                queue.put_nowait(message)
            except asyncio.QueueFull:
                stale.append(queue)
        for queue in stale:
            if queue in self._subscribers:
                self._subscribers.remove(queue)

    def subscribe(self) -> asyncio.Queue[str]:
        queue: asyncio.Queue[str] = asyncio.Queue(maxsize=256)
        self._subscribers.append(queue)
        return queue

    def unsubscribe(self, queue: asyncio.Queue[str]) -> None:
        if queue in self._subscribers:
            self._subscribers.remove(queue)

    def _build_session(
        self,
        request: CreateAgentSessionRequest,
    ) -> Dict[str, Any]:
        now = utc_now_iso()
        session_id = str(uuid.uuid4())
        metadata = _normalize_session_metadata(request.metadata)

        if request.origin_surface:
            metadata["originSurface"] = request.origin_surface
        if request.session_mode:
            metadata["sessionMode"] = request.session_mode
        if request.agent_id:
            metadata["agentId"] = request.agent_id
        if request.agent_name:
            metadata["agentName"] = request.agent_name
        if request.project_id:
            metadata["projectId"] = request.project_id
        if request.workspace_scope:
            metadata["workspaceScope"] = request.workspace_scope
        if request.agent_features is not None:
            metadata["agentFeatures"] = request.agent_features
        if request.model:
            metadata["runtimeModel"] = request.model

        return {
            "id": session_id,
            "name": request.name or "New Session",
            "description": request.description,
            "created_at": now,
            "updated_at": now,
            "last_accessed": now,
            "message_count": 0,
            "active": True,
            "tags": list(request.tags or []),
            "metadata": metadata,
            "messages": [],
        }

    async def list_sessions(self) -> List[Dict[str, Any]]:
        async with self._lock:
            sessions = list(self._sessions.values())
        sessions.sort(key=lambda item: item.get("updated_at") or item.get("created_at") or "", reverse=True)
        return [self._serialize_session(session) for session in sessions]

    async def get_session(self, session_id: str) -> Optional[Dict[str, Any]]:
        async with self._lock:
            session = self._sessions.get(session_id)
            if not session:
                return None
            return self._serialize_session(session)

    async def create_session(self, request: CreateAgentSessionRequest) -> Dict[str, Any]:
        async with self._lock:
            session = self._build_session(request)
            self._sessions[session["id"]] = session
            self._persist()
        serialized = self._serialize_session(session)
        self._emit_nowait({"type": "session.created", "payload": {"session": serialized}})
        return serialized

    async def update_session(
        self,
        session_id: str,
        request: UpdateAgentSessionRequest,
    ) -> Optional[Dict[str, Any]]:
        async with self._lock:
            session = self._sessions.get(session_id)
            if not session:
                return None

            if request.name is not None:
                session["name"] = request.name
            if request.description is not None:
                session["description"] = request.description
            if request.active is not None:
                session["active"] = request.active
            if request.tags is not None:
                session["tags"] = list(request.tags)

            metadata = _normalize_session_metadata(session.get("metadata"))
            if request.metadata is not None:
                metadata.update(_normalize_session_metadata(request.metadata))
            if request.origin_surface is not None:
                metadata["originSurface"] = request.origin_surface
            if request.session_mode is not None:
                metadata["sessionMode"] = request.session_mode
            if request.project_id is not None:
                metadata["projectId"] = request.project_id
            if request.workspace_scope is not None:
                metadata["workspaceScope"] = request.workspace_scope
            if request.agent_features is not None:
                metadata["agentFeatures"] = request.agent_features
            session["metadata"] = metadata
            session["updated_at"] = utc_now_iso()
            self._persist()

        serialized = self._serialize_session(session)
        self._emit_nowait({"type": "session.updated", "payload": {"session": serialized}})
        return serialized

    async def delete_session(self, session_id: str) -> bool:
        async with self._lock:
            session = self._sessions.pop(session_id, None)
            if not session:
                return False
            self._persist()

        self._emit_nowait({"type": "session.deleted", "payload": {"sessionId": session_id}})
        return True

    async def list_messages(self, session_id: str) -> Optional[List[Dict[str, Any]]]:
        async with self._lock:
            session = self._sessions.get(session_id)
            if not session:
                return None
            return list(session.get("messages", []))

    async def add_message(
        self,
        session_id: str,
        request: AgentSessionMessageRequest,
    ) -> Optional[Dict[str, Any]]:
        async with self._lock:
            session = self._sessions.get(session_id)
            if not session:
                return None
            now = utc_now_iso()
            message = {
                "id": str(uuid.uuid4()),
                "role": request.role or "user",
                "content": request.text,
                "timestamp": now,
                "metadata": None,
            }
            session.setdefault("messages", []).append(message)
            session["message_count"] = len(session["messages"])
            session["updated_at"] = now
            session["last_accessed"] = now
            self._persist()

        self._emit_nowait(
            {
                "type": "message.added",
                "payload": {
                    "sessionId": session_id,
                    "message": message,
                },
            }
        )
        return message

    async def abort_session(
        self,
        session_id: str,
        request: AbortAgentSessionRequest,
    ) -> Optional[Dict[str, Any]]:
        async with self._lock:
            session = self._sessions.get(session_id)
            if not session:
                return None
            metadata = _normalize_session_metadata(session.get("metadata"))
            if request.reason:
                metadata["lastAbortReason"] = request.reason
            session["metadata"] = metadata
            session["active"] = False
            session["updated_at"] = utc_now_iso()
            self._persist()

        serialized = self._serialize_session(session)
        self._emit_nowait({"type": "session.updated", "payload": {"session": serialized}})
        return serialized

    def _serialize_session(self, session: Dict[str, Any]) -> Dict[str, Any]:
        return {
            "id": session["id"],
            "name": session.get("name"),
            "description": session.get("description"),
            "created_at": session.get("created_at"),
            "updated_at": session.get("updated_at"),
            "last_accessed": session.get("last_accessed"),
            "message_count": session.get("message_count", 0),
            "active": session.get("active", True),
            "tags": list(session.get("tags", [])),
            "metadata": _normalize_session_metadata(session.get("metadata")),
        }


class AgentStore:
    def __init__(self, path: Path, workspace_dir: Path):
        self.path = path
        self.workspace_dir = workspace_dir
        self.workspace_dir.mkdir(parents=True, exist_ok=True)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self._lock = asyncio.Lock()
        self._agents: Dict[str, Dict[str, Any]] = {}
        self._load_from_disk()

    def _load_from_disk(self) -> None:
        if not self.path.exists():
            self._agents = {}
            return
        try:
            payload = json.loads(self.path.read_text())
            agents = payload.get("agents", [])
            self._agents = {
                agent["id"]: agent
                for agent in agents
                if isinstance(agent, dict) and agent.get("id")
            }
        except Exception as e:
            logger.warning(f"Failed to load agent store from {self.path}: {e}")
            self._agents = {}

    def _persist(self) -> None:
        payload = {
            "agents": list(self._agents.values()),
            "updated_at": utc_now_iso(),
        }
        self.path.write_text(json.dumps(payload, indent=2))

    def _serialize_agent(self, agent: Dict[str, Any]) -> Dict[str, Any]:
        return {
            "id": agent["id"],
            "name": agent.get("name", ""),
            "description": agent.get("description", ""),
            "agent_type": agent.get("agent_type", "worker"),
            "parent_agent_id": agent.get("parent_agent_id"),
            "model": agent.get("model", ""),
            "provider": agent.get("provider", "custom"),
            "capabilities": list(agent.get("capabilities", [])),
            "system_prompt": agent.get("system_prompt"),
            "tools": list(agent.get("tools", [])),
            "max_iterations": agent.get("max_iterations", 10),
            "temperature": agent.get("temperature", 0.7),
            "voice": agent.get("voice"),
            "config": dict(agent.get("config", {})),
            "status": agent.get("status", "idle"),
            "created_at": agent.get("created_at"),
            "updated_at": agent.get("updated_at"),
            "last_run_at": agent.get("last_run_at"),
            "workspace_id": agent.get("workspace_id"),
            "owner_id": agent.get("owner_id"),
        }

    def _workspace_path(self, agent_id: str) -> Path:
        return self.workspace_dir / f"{agent_id}.json"

    async def list_agents(self) -> List[Dict[str, Any]]:
        async with self._lock:
            agents = list(self._agents.values())
        agents.sort(key=lambda item: item.get("updated_at") or item.get("created_at") or "", reverse=True)
        return [self._serialize_agent(agent) for agent in agents]

    async def get_agent(self, agent_id: str) -> Optional[Dict[str, Any]]:
        async with self._lock:
            agent = self._agents.get(agent_id)
            if not agent:
                return None
            return self._serialize_agent(agent)

    async def create_agent(self, request: CreateAgentRequest) -> Dict[str, Any]:
        now = utc_now_iso()
        agent_id = str(uuid.uuid4())
        workspace_id = request.workspace_id or f"{request.name.lower().replace(' ', '-')}-{agent_id[:8]}"
        agent = {
            "id": agent_id,
            "name": request.name,
            "description": request.description or "",
            "agent_type": request.agent_type or "worker",
            "parent_agent_id": request.parent_agent_id,
            "model": request.model,
            "provider": request.provider,
            "capabilities": list(request.capabilities or []),
            "system_prompt": request.system_prompt,
            "tools": list(request.tools or []),
            "max_iterations": request.max_iterations if request.max_iterations is not None else 10,
            "temperature": request.temperature if request.temperature is not None else 0.7,
            "voice": request.voice.model_dump(exclude_none=True) if request.voice else None,
            "config": dict(request.config or {}),
            "status": "idle",
            "created_at": now,
            "updated_at": now,
            "last_run_at": None,
            "workspace_id": workspace_id,
            "owner_id": request.owner_id,
        }
        async with self._lock:
            self._agents[agent_id] = agent
            self._persist()
        return self._serialize_agent(agent)

    async def update_agent(self, agent_id: str, request: UpdateAgentRequest) -> Optional[Dict[str, Any]]:
        async with self._lock:
            agent = self._agents.get(agent_id)
            if not agent:
                return None
            payload = request.model_dump(exclude_unset=True, exclude_none=False)
            for key, value in payload.items():
                if key == "voice":
                    agent["voice"] = value
                elif key in {"capabilities", "tools"} and value is not None:
                    agent[key] = list(value)
                elif key == "config" and value is not None:
                    agent["config"] = dict(value)
                else:
                    agent[key] = value
            agent["updated_at"] = utc_now_iso()
            self._persist()
            return self._serialize_agent(agent)

    async def delete_agent(self, agent_id: str) -> bool:
        async with self._lock:
            agent = self._agents.pop(agent_id, None)
            if not agent:
                return False
            self._persist()
        workspace_path = self._workspace_path(agent_id)
        if workspace_path.exists():
            workspace_path.unlink()
        return True

    async def initialize_workspace(self, agent_id: str, documents: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        async with self._lock:
            agent = self._agents.get(agent_id)
            if not agent:
                return None
            agent["updated_at"] = utc_now_iso()
            self._persist()
        payload = {
            "agent_id": agent_id,
            "workspace_id": agent.get("workspace_id"),
            "documents": documents,
            "updated_at": utc_now_iso(),
        }
        self._workspace_path(agent_id).write_text(json.dumps(payload, indent=2))
        return payload


class OfficeRuntimeStore:
    def __init__(self, path: Path):
        self.path = path
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self._lock = asyncio.Lock()
        self._bindings: Dict[str, Dict[str, Any]] = {}
        self._sessions: Dict[str, Dict[str, Any]] = {}
        self._load_from_disk()

    def _load_from_disk(self) -> None:
        if not self.path.exists():
            self._bindings = {}
            self._sessions = {}
            return
        try:
            payload = json.loads(self.path.read_text())
            bindings = payload.get("bindings", [])
            sessions = payload.get("sessions", [])
            self._bindings = {
                binding["id"]: binding
                for binding in bindings
                if isinstance(binding, dict) and binding.get("id")
            }
            self._sessions = {
                session["id"]: session
                for session in sessions
                if isinstance(session, dict) and session.get("id")
            }
        except Exception as e:
            logger.warning(f"Failed to load office runtime store from {self.path}: {e}")
            self._bindings = {}
            self._sessions = {}

    def _persist(self) -> None:
        payload = {
            "bindings": list(self._bindings.values()),
            "sessions": list(self._sessions.values()),
            "updated_at": utc_now_iso(),
        }
        self.path.write_text(json.dumps(payload, indent=2))

    def _document_key(self, document: OfficeDocumentRequest) -> str:
        primary = (
            document.document_id
            or document.document_url
            or document.fingerprint
            or f"{document.host}:{document.title or ''}:{document.label or ''}"
        )
        return hashlib.sha256(primary.encode("utf-8")).hexdigest()

    def _serialize_binding(self, binding: Dict[str, Any]) -> Dict[str, Any]:
        sessions = [
            self._serialize_session(session)
            for session in self._sessions.values()
            if session.get("binding_id") == binding["id"]
        ]
        sessions.sort(key=lambda item: item.get("last_seen_at") or "", reverse=True)
        return {
            "id": binding["id"],
            "document_key": binding.get("document_key"),
            "host": binding.get("host"),
            "title": binding.get("title"),
            "label": binding.get("label"),
            "summary": binding.get("summary"),
            "document_url": binding.get("document_url"),
            "document_id": binding.get("document_id"),
            "fingerprint": binding.get("fingerprint"),
            "project_id": binding.get("project_id"),
            "workspace_id": binding.get("workspace_id"),
            "taskpane_origin": binding.get("taskpane_origin"),
            "taskpane_url": binding.get("taskpane_url"),
            "manifest_url": binding.get("manifest_url"),
            "platform_origin": binding.get("platform_origin"),
            "last_runtime_status": binding.get("last_runtime_status"),
            "last_page_label": binding.get("last_page_label"),
            "last_current_task": binding.get("last_current_task"),
            "last_history_count": binding.get("last_history_count"),
            "connected": binding.get("connected", False),
            "created_at": binding.get("created_at"),
            "updated_at": binding.get("updated_at"),
            "last_seen_at": binding.get("last_seen_at"),
            "active_sessions": sessions,
            "active_session_count": len(sessions),
        }

    def _serialize_session(self, session: Dict[str, Any]) -> Dict[str, Any]:
        return {
            "id": session["id"],
            "binding_id": session.get("binding_id"),
            "status": session.get("status"),
            "page_label": session.get("page_label"),
            "current_task": session.get("current_task"),
            "history_count": session.get("history_count", 0),
            "connected": session.get("connected", False),
            "created_at": session.get("created_at"),
            "updated_at": session.get("updated_at"),
            "last_seen_at": session.get("last_seen_at"),
        }

    async def bootstrap(self, request: OfficeBootstrapRequest) -> Dict[str, Any]:
        async with self._lock:
            document_key = self._document_key(request.document)
            binding = next(
                (item for item in self._bindings.values() if item.get("document_key") == document_key),
                None,
            )
            now = utc_now_iso()
            if binding is None:
                binding = {
                    "id": str(uuid.uuid4()),
                    "document_key": document_key,
                    "created_at": now,
                }
                self._bindings[binding["id"]] = binding

            binding.update(
                {
                    "host": request.document.host,
                    "title": request.document.title,
                    "label": request.document.label,
                    "summary": request.document.summary,
                    "document_url": request.document.document_url,
                    "document_id": request.document.document_id,
                    "fingerprint": request.document.fingerprint,
                    "project_id": request.project_id,
                    "workspace_id": request.workspace_id,
                    "taskpane_origin": request.platform.taskpane_origin if request.platform else binding.get("taskpane_origin"),
                    "taskpane_url": request.platform.taskpane_url if request.platform else binding.get("taskpane_url"),
                    "manifest_url": request.platform.manifest_url if request.platform else binding.get("manifest_url"),
                    "platform_origin": request.platform.platform_origin if request.platform else binding.get("platform_origin"),
                    "last_runtime_status": request.runtime_state.status if request.runtime_state else binding.get("last_runtime_status"),
                    "last_page_label": request.runtime_state.page_label if request.runtime_state else binding.get("last_page_label"),
                    "last_current_task": request.runtime_state.current_task if request.runtime_state else binding.get("last_current_task"),
                    "last_history_count": request.runtime_state.history_count if request.runtime_state else binding.get("last_history_count"),
                    "connected": request.runtime_state.connected if request.runtime_state and request.runtime_state.connected is not None else True,
                    "updated_at": now,
                    "last_seen_at": now,
                }
            )

            session_id = request.session_id or str(uuid.uuid4())
            session = self._sessions.get(session_id)
            if session is None:
                session = {
                    "id": session_id,
                    "binding_id": binding["id"],
                    "created_at": now,
                }
                self._sessions[session_id] = session
            session.update(
                {
                    "binding_id": binding["id"],
                    "status": request.runtime_state.status if request.runtime_state else session.get("status", "idle"),
                    "page_label": request.runtime_state.page_label if request.runtime_state else session.get("page_label"),
                    "current_task": request.runtime_state.current_task if request.runtime_state else session.get("current_task"),
                    "history_count": request.runtime_state.history_count if request.runtime_state else session.get("history_count", 0),
                    "connected": request.runtime_state.connected if request.runtime_state and request.runtime_state.connected is not None else True,
                    "updated_at": now,
                    "last_seen_at": now,
                }
            )

            self._persist()
            return {
                "binding": self._serialize_binding(binding),
                "session": self._serialize_session(session),
            }

    async def update_runtime_state(self, request: OfficeRuntimeSyncRequest) -> Optional[Dict[str, Any]]:
        async with self._lock:
            binding = self._bindings.get(request.binding_id)
            if binding is None:
                return None
            now = utc_now_iso()
            if request.document:
                binding.update(
                    {
                        "host": request.document.host,
                        "title": request.document.title,
                        "label": request.document.label,
                        "summary": request.document.summary,
                        "document_url": request.document.document_url,
                        "document_id": request.document.document_id,
                        "fingerprint": request.document.fingerprint,
                    }
                )
            if request.platform:
                binding.update(
                    {
                        "taskpane_origin": request.platform.taskpane_origin,
                        "taskpane_url": request.platform.taskpane_url,
                        "manifest_url": request.platform.manifest_url,
                        "platform_origin": request.platform.platform_origin,
                    }
                )
            if request.project_id is not None:
                binding["project_id"] = request.project_id
            if request.workspace_id is not None:
                binding["workspace_id"] = request.workspace_id
            if request.runtime_state:
                binding.update(
                    {
                        "last_runtime_status": request.runtime_state.status,
                        "last_page_label": request.runtime_state.page_label,
                        "last_current_task": request.runtime_state.current_task,
                        "last_history_count": request.runtime_state.history_count,
                        "connected": request.runtime_state.connected if request.runtime_state.connected is not None else binding.get("connected", False),
                    }
                )
            binding["updated_at"] = now
            binding["last_seen_at"] = now

            session = self._sessions.get(request.session_id)
            if session is None:
                session = {
                    "id": request.session_id,
                    "binding_id": binding["id"],
                    "created_at": now,
                }
                self._sessions[request.session_id] = session
            if request.runtime_state:
                session.update(
                    {
                        "status": request.runtime_state.status,
                        "page_label": request.runtime_state.page_label,
                        "current_task": request.runtime_state.current_task,
                        "history_count": request.runtime_state.history_count,
                        "connected": request.runtime_state.connected if request.runtime_state.connected is not None else session.get("connected", False),
                    }
                )
            session["binding_id"] = binding["id"]
            session["updated_at"] = now
            session["last_seen_at"] = now

            self._persist()
            return {
                "binding": self._serialize_binding(binding),
                "session": self._serialize_session(session),
            }

    async def list_bindings(self) -> List[Dict[str, Any]]:
        async with self._lock:
            bindings = list(self._bindings.values())
        bindings.sort(key=lambda item: item.get("last_seen_at") or item.get("updated_at") or "", reverse=True)
        return [self._serialize_binding(binding) for binding in bindings]

    async def get_binding(self, binding_id: str) -> Optional[Dict[str, Any]]:
        async with self._lock:
            binding = self._bindings.get(binding_id)
            if binding is None:
                return None
            return self._serialize_binding(binding)

class GatewayRuntime:
    def __init__(self):
        self.registry: Dict[str, Any] = {}
        self.ui_registry: Dict[str, Any] = {}
        self.registry_hash: str = ""
        self.receipt_schema: Dict[str, Any] = {}
        self.ui_receipt_schema: Dict[str, Any] = {}
        self.run_id: str = os.environ.get("Allternit_RUN_ID", "dev-session")
        
        self.load_registry()
        self.load_ui_registry()
        self.load_receipt_schema()
        
        # Keep receipts stable regardless of the process cwd.
        self.receipt_dir = REPO_ROOT / ".allternit" / "receipts" / self.run_id
        self.receipt_dir.mkdir(parents=True, exist_ok=True)

    def load_registry(self):
        # 1. Check file existence
        if not REGISTRY_PATH.exists():
            logger.critical(f"Gateway registry missing at {REGISTRY_PATH}")
            sys.exit(1)
        
        if not REGISTRY_SCHEMA_PATH.exists():
            logger.critical(f"Gateway registry schema missing at {REGISTRY_SCHEMA_PATH}")
            sys.exit(1)

        try:
            with open(REGISTRY_PATH, 'r') as f:
                self.registry = json.load(f)
            with open(REGISTRY_SCHEMA_PATH, 'r') as f:
                schema = json.load(f)
        except Exception as e:
            logger.critical(f"Failed to read/parse registry files: {e}")
            sys.exit(1)

        # 2. Validate against schema
        try:
            jsonschema.validate(instance=self.registry, schema=schema)
            logger.info("Gateway registry validation successful.")
        except jsonschema.ValidationError as e:
            logger.critical(f"Gateway registry schema validation failed: {e}")
            sys.exit(1)

        # 3. Compute Hash
        canonical_json = json.dumps(self.registry, sort_keys=True)
        self.registry_hash = hashlib.sha256(canonical_json.encode('utf-8')).hexdigest()
        logger.info(f"Registry Hash: {self.registry_hash}")

    def load_ui_registry(self):
        if not UI_REGISTRY_PATH.exists():
            logger.warning(f"UI registry missing at {UI_REGISTRY_PATH}")
            return
        try:
            with open(UI_REGISTRY_PATH, 'r') as f:
                self.ui_registry = json.load(f)
            logger.info("UI registry loaded.")
        except Exception as e:
            logger.error(f"Failed to load UI registry: {e}")

    def load_receipt_schema(self):
        if not RECEIPT_SCHEMA_PATH.exists():
            logger.warning(f"Receipt schema missing at {RECEIPT_SCHEMA_PATH}")
        else:
            try:
                with open(RECEIPT_SCHEMA_PATH, 'r') as f:
                    self.receipt_schema = json.load(f)
            except Exception as e:
                logger.error(f"Failed to load receipt schema: {e}")

        if not UI_RECEIPT_SCHEMA_PATH.exists():
            logger.warning(f"UI Receipt schema missing at {UI_RECEIPT_SCHEMA_PATH}")
        else:
            try:
                with open(UI_RECEIPT_SCHEMA_PATH, 'r') as f:
                    self.ui_receipt_schema = json.load(f)
            except Exception as e:
                logger.error(f"Failed to load UI receipt schema: {e}")

    def emit_receipt(self, receipt_data: Dict[str, Any]):
        receipt_id = receipt_data["receipt_id"]
        path = self.receipt_dir / f"gateway-{receipt_id}.json"
        
        try:
            # Validate receipt if schema loaded
            if self.receipt_schema:
                jsonschema.validate(instance=receipt_data, schema=self.receipt_schema)
            
            with open(path, 'w') as f:
                json.dump(receipt_data, f, indent=2)
            logger.info(f"Receipt emitted: {path}")
        except Exception as e:
            logger.error(f"Failed to write receipt: {e}")

    def emit_ui_receipt(self, receipt_data: Dict[str, Any]):
        receipt_id = receipt_data.get("receipt_id") or str(uuid.uuid4())
        path = self.receipt_dir / f"ui-{receipt_id}.json"
        
        try:
            # Validate receipt if schema loaded
            if self.ui_receipt_schema:
                jsonschema.validate(instance=receipt_data, schema=self.ui_receipt_schema)
            
            with open(path, 'w') as f:
                json.dump(receipt_data, f, indent=2)
            logger.info(f"UI Receipt emitted: {path}")
        except Exception as e:
            logger.error(f"Failed to write UI receipt: {e}")

    def resolve_route(self, path: str, method: str) -> tuple[Optional[Dict[str, Any]], Optional[str]]:
        """
        Resolves a path to a service definition.
        Returns (service_def, denial_reason)
        """
        # T0411 - Reject raw URLs/IPs in path
        if "http://" in path or "https://" in path or "localhost" in path or "127.0.0.1" in path:
            return None, "Raw URL/IP not allowed in path"

        # Lookup route
        # Registry structure: "external_ingress": { "routes": [ { "path_prefix": "/", "service": "kernel" } ] }
        # "services": [ { "name": "kernel", "internal_url": "..." } ]
        
        ingress = self.registry.get("external_ingress", {})
        routes = ingress.get("routes", [])
        
        matched_service_name = None
        
        # Simple longest prefix match
        # Sorting routes by length desc to match specific first
        sorted_routes = sorted(routes, key=lambda r: len(r.get("path_prefix", "")), reverse=True)
        
        for route in sorted_routes:
            prefix = route.get("path_prefix", "")
            if path.startswith(prefix):
                # Check method if specified
                allowed_methods = route.get("methods", [])
                if allowed_methods and method not in allowed_methods:
                    continue # Try next route or fail? Usually 405, but here just skip match
                
                matched_service_name = route.get("service")
                break
        
        if not matched_service_name:
            return None, "No matching route found"

        # Lookup service definition
        services = self.registry.get("services", [])
        service_def = next((s for s in services if s["name"] == matched_service_name), None)
        
        if not service_def:
            return None, f"Service '{matched_service_name}' not defined in registry"
            
        return service_def, None


def verify_token(request: Request) -> Optional[str]:
    auth_header = request.headers.get('Authorization')
    desktop_token = request.headers.get('X-Allternit-Desktop-Access-Token')
    forwarded_desktop = request.headers.get('X-Forwarded-Desktop-Auth')
    client_host = request.client.host if request.client else None
    allow_local_dev_bypass = os.environ.get('ALLOW_LOCAL_DEV_BYPASS', '1').lower() in {'1', 'true', 'yes'}
    is_loopback = client_host in {'127.0.0.1', '::1', 'localhost'}

    if request.url.path in {'/health', '/api/health'}:
        return None

    if desktop_token or forwarded_desktop == '1':
        return None

    if not auth_header:
        if allow_local_dev_bypass and is_loopback:
            return None
        return 'Missing Authorization header'

    if not auth_header.startswith('Bearer '):
        if allow_local_dev_bypass and is_loopback:
            return None
        return 'Invalid Authorization scheme'

    token = auth_header.split(' ', 1)[1].strip()
    if token == 'dev-token':
        return None

    if allow_local_dev_bypass and is_loopback:
        return None

    return None


def rewrite_upstream_path(path: str) -> str:
    if path in {'/health', '/api/health'}:
        return '/health'

    # /api/factory/* passes through unchanged (allternit-api serves it).

    if path == '/api/v1':
        return '/'
    if path.startswith('/api/v1/'):
        return '/' + path[len('/api/v1/'):]

    return path

runtime = GatewayRuntime()
agent_session_store = AgentSessionStore(AGENT_SESSIONS_STORE_PATH)
agent_store = AgentStore(AGENTS_STORE_PATH, AGENT_WORKSPACES_DIR)
office_runtime_store = OfficeRuntimeStore(OFFICE_RUNTIME_STORE_PATH)
app = FastAPI()

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

@app.middleware("http")
async def gateway_middleware(request: Request, call_next):
    # This middleware acts as the routing logic
    
    # 1. Handle CORS Preflight
    if request.method == "OPTIONS":
        return apply_cors_headers(Response(status_code=200), request)

    # Skip health check from routing logic
    if (
        request.url.path in {"/health", "/api/health"}
        or is_compat_http_path(request.url.path)
        or is_local_gateway_route(request.url.path)
    ):
        return apply_cors_headers(await call_next(request), request)

    if request.url.path == "/api/v1/providers" or request.url.path.startswith("/api/v1/providers/"):
        return apply_cors_headers(await proxy_provider_request(request), request)

    auth_error = verify_token(request)
    if auth_error:
        return JSONResponse(status_code=401, content={"error": auth_error})

    request_id = str(uuid.uuid4())
    trace_id = request.headers.get("X-Trace-Id", request_id)
    
    # 1. Resolve Route
    service_def, denial_reason = runtime.resolve_route(request.url.path, request.method)
    
    receipt = {
        "receipt_id": str(uuid.uuid4()),
        "created_at": datetime.datetime.utcnow().isoformat() + "Z",
        "gateway_id": "allternit-gateway-runtime",
        "run_id": runtime.run_id,
        "trace_id": trace_id,
        "route_id": service_def["name"] if service_def else "unknown",
        "service": service_def["name"] if service_def else "none",
        "upstream_url": "none", # Default
        "status_code": 0
    }

    if denial_reason:
        receipt["status_code"] = 403
        runtime.emit_receipt(receipt)
        return apply_cors_headers(
            JSONResponse(status_code=403, content={"error": denial_reason}),
            request,
        )

    # 2. Allow - Proxy (Real)
    target_url = service_def["internal_url"]
    
    # Strip path prefix from request path if needed? 
    # For now, assume target expects full path or has its own routing.
    # Actually, if we're at /api/foo, we want to hit target/api/foo.
    
    upstream_path = rewrite_upstream_path(request.url.path)
    if request.url.query:
        upstream_path += "?" + request.url.query
    
    full_target_url = f"{target_url.rstrip('/')}{upstream_path}"
    
    async with httpx.AsyncClient() as client:
        try:
            # We must exclude 'host' header as it points to gateway
            headers = {k: v for k, v in request.headers.items() if k.lower() != 'host'}
            if request.headers.get('X-Allternit-Desktop-Access-Token'):
                headers['X-Forwarded-Desktop-Auth'] = '1'
            
            # Read request body
            body = await request.body()
            
            resp = await client.request(
                method=request.method,
                url=full_target_url,
                headers=headers,
                content=body,
                follow_redirects=True,
                timeout=60.0
            )
            
            receipt["upstream_url"] = full_target_url
            receipt["status_code"] = resp.status_code
            runtime.emit_receipt(receipt)
            
            # Return upstream response with CORS
            response = Response(
                content=resp.content,
                status_code=resp.status_code,
                headers=dict(resp.headers)
            )
            return apply_cors_headers(response, request)
            
        except Exception as e:
            logger.error(f"Proxy error: {e}")
            receipt["status_code"] = 502
            runtime.emit_receipt(receipt)
            return apply_cors_headers(
                JSONResponse(
                    status_code=502,
                    content={"error": f"Upstream proxy error: {e}"},
                ),
                request,
            )

class ActionProposal(BaseModel):
    type: str
    x: Optional[int] = None
    y: Optional[int] = None
    text: Optional[str] = None
    confidence: float
    target: Optional[str] = None
    thought: Optional[str] = None

class UIActionRequest(BaseModel):
    action_version: str
    action_id: str
    gateway_route: str
    run_id: str
    wih_id: str
    payload: Dict[str, Any]
    timestamp: str
    source: str
    session_id: str
    user_id: str

@app.post("/v1/ui/execute")
async def execute_ui_action(request: UIActionRequest):
    start_time = time.time()
    
    # 1. Validate against UI Registry
    actions = runtime.ui_registry.get("ui_actions", [])
    action_def = next((a for a in actions if a["action_id"] == request.action_id), None)
    
    if not action_def:
        raise HTTPException(status_code=403, detail=f"UI Action '{request.action_id}' not allowed by registry")
    
    # 2. Verify gateway_route matches
    if action_def["gateway_route"] != request.gateway_route:
        raise HTTPException(status_code=403, detail="Gateway route mismatch for UI Action")

    # 3. Resolve and Proxy via Gateway logic
    parts = request.gateway_route.split(":", 1)
    if len(parts) != 2:
        raise HTTPException(status_code=400, detail="Invalid gateway_route format")
    
    method, path = parts
    service_def, denial_reason = runtime.resolve_route(path, method)
    
    if denial_reason:
        raise HTTPException(status_code=403, detail=denial_reason)

    # 4. Simulate proxy and emit receipt
    target_url = service_def["internal_url"]
    
    execution_time = int((time.time() - start_time) * 1000)
    
    receipt = {
        "receipt_version": "v0.1",
        "receipt_type": "ui_action_receipt",
        "run_id": request.run_id,
        "task_id": "TBD", # Should extract from context or request if available
        "wih_id": request.wih_id,
        "timestamp": datetime.datetime.utcnow().isoformat() + "Z",
        "action_id": request.action_id,
        "route_invoked": request.gateway_route,
        "invoker_id": request.user_id,
        "session_id": request.session_id,
        "user_id": request.user_id,
        "request_payload_hash": hashlib.sha256(json.dumps(request.payload).encode()).hexdigest(),
        "response_payload_hash": hashlib.sha256(b"{}").hexdigest(), # Simulated empty response
        "gateway_response_code": 200,
        "execution_time_ms": execution_time,
        "success": True
    }
    
    runtime.emit_ui_receipt(receipt)
    
    return {
        "status": "success",
        "action_id": request.action_id,
        "gateway_route": request.gateway_route,
        "target": target_url
    }

@app.get("/health")
async def health():
    return {
        "status": "ok",
        "service": "allternit-gateway",
        "registry": str(REGISTRY_PATH),
    }


@app.get("/api/health")
async def api_health():
    return await health()


async def ensure_authorized(request: Request) -> Optional[Response]:
    auth_error = verify_token(request)
    if auth_error:
        return apply_cors_headers(
            JSONResponse(status_code=401, content={"error": auth_error}),
            request,
        )
    return None


async def proxy_provider_request(request: Request) -> Response:
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response

    upstream_path = rewrite_provider_path(request.url.path)
    target_url = build_upstream_url(get_gizzi_base_url(), upstream_path, request.url.query)

    headers = {
        k: v
        for k, v in request.headers.items()
        if k.lower() not in {"host", "content-length"}
    }
    headers.update(build_gizzi_headers())
    body = await request.body()

    async with httpx.AsyncClient(timeout=30.0) as client:
        try:
            resp = await client.request(
                method=request.method,
                url=target_url,
                headers=headers,
                content=body if body else None,
                follow_redirects=True,
            )
        except Exception as e:
            logger.warning(f"Provider proxy error for {upstream_path}: {e}")
            return apply_cors_headers(
                JSONResponse(
                    status_code=502,
                    content={"error": f"Provider proxy error: {e}"},
                ),
                request,
            )

    content_type = resp.headers.get("content-type", "")
    if "application/json" in content_type:
        try:
            return apply_cors_headers(
                JSONResponse(status_code=resp.status_code, content=resp.json()),
                request,
            )
        except Exception:
            pass

    return apply_cors_headers(
        Response(
            content=resp.content,
            status_code=resp.status_code,
            media_type=content_type or None,
        ),
        request,
    )


async def proxy_gizzi_json(
    request: Request,
    upstream_path: str,
    fallback_payload: Any,
) -> Response:
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response

    target_url = build_upstream_url(get_gizzi_base_url(), upstream_path, request.url.query)
    headers = build_gizzi_headers({"Accept": "application/json"})

    async with httpx.AsyncClient(timeout=10.0) as client:
        try:
            resp = await client.get(target_url, headers=headers)
            media_type = resp.headers.get("content-type", "")
            if "application/json" in media_type:
                return apply_cors_headers(
                    JSONResponse(status_code=resp.status_code, content=resp.json()),
                    request,
                )
            return apply_cors_headers(
                Response(content=resp.content, status_code=resp.status_code, media_type=media_type or None),
                request,
            )
        except Exception as e:
            logger.warning(f"Falling back for Gizzi JSON proxy {upstream_path}: {e}")
            return apply_cors_headers(
                JSONResponse(status_code=200, content=fallback_payload),
                request,
            )


async def proxy_gizzi_sse(
    request: Request,
    upstream_path: str,
    fallback_event: str,
) -> Response:
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response

    target_url = build_upstream_url(get_gizzi_base_url(), upstream_path, request.url.query)
    headers = build_gizzi_headers(
        {
            "Accept": "text/event-stream",
            "Cache-Control": "no-cache",
        }
    )

    try:
        client = httpx.AsyncClient(timeout=None)
        upstream_request = client.build_request("GET", target_url, headers=headers)
        upstream = await client.send(upstream_request, stream=True)
        if upstream.status_code >= 400:
            body = await upstream.aread()
            await upstream.aclose()
            await client.aclose()
            media_type = upstream.headers.get("content-type", "application/json")
            return apply_cors_headers(
                Response(content=body, status_code=upstream.status_code, media_type=media_type),
                request,
            )

        async def stream_upstream():
            try:
                async for chunk in upstream.aiter_bytes():
                    if await request.is_disconnected():
                        break
                    if chunk:
                        yield chunk
            finally:
                await upstream.aclose()
                await client.aclose()

        return apply_cors_headers(
            StreamingResponse(
            stream_upstream(),
            media_type="text/event-stream",
            headers={
                "Cache-Control": "no-cache",
                "Connection": "keep-alive",
                "X-Accel-Buffering": "no",
            },
            ),
            request,
        )
    except Exception as e:
        logger.warning(f"Falling back for Gizzi SSE proxy {upstream_path}: {e}")

        async def fallback_stream():
            yield f"data: {fallback_event}\n\n"
            while not await request.is_disconnected():
                await asyncio.sleep(15)
                yield ": heartbeat\n\n"

        return apply_cors_headers(
            StreamingResponse(
            fallback_stream(),
            media_type="text/event-stream",
            headers={
                "Cache-Control": "no-cache",
                "Connection": "keep-alive",
                "X-Accel-Buffering": "no",
            },
            ),
            request,
        )


@app.get("/api/v1/agent-sessions")
async def list_agent_sessions(request: Request):
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response
    sessions = await agent_session_store.list_sessions()
    return apply_cors_headers(
        JSONResponse(content={"sessions": sessions, "count": len(sessions)}),
        request,
    )


@app.post("/api/v1/agent-sessions")
async def create_agent_session(request: Request, payload: CreateAgentSessionRequest):
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response
    session = await agent_session_store.create_session(payload)
    return apply_cors_headers(JSONResponse(status_code=201, content=session), request)


@app.get("/api/v1/agent-sessions/{session_id}")
async def get_agent_session(session_id: str, request: Request):
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response
    session = await agent_session_store.get_session(session_id)
    if session is None:
        return apply_cors_headers(
            JSONResponse(status_code=404, content={"error": "Session not found"}),
            request,
        )
    return apply_cors_headers(JSONResponse(content=session), request)


@app.patch("/api/v1/agent-sessions/{session_id}")
async def update_agent_session(
    session_id: str,
    request: Request,
    payload: UpdateAgentSessionRequest,
):
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response
    session = await agent_session_store.update_session(session_id, payload)
    if session is None:
        return apply_cors_headers(
            JSONResponse(status_code=404, content={"error": "Session not found"}),
            request,
        )
    return apply_cors_headers(JSONResponse(content=session), request)


@app.delete("/api/v1/agent-sessions/{session_id}")
async def delete_agent_session(session_id: str, request: Request):
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response
    deleted = await agent_session_store.delete_session(session_id)
    if not deleted:
        return apply_cors_headers(
            JSONResponse(status_code=404, content={"error": "Session not found"}),
            request,
        )
    return apply_cors_headers(Response(status_code=204), request)


@app.get("/api/v1/agent-sessions/{session_id}/messages")
async def list_agent_session_messages(session_id: str, request: Request):
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response
    messages = await agent_session_store.list_messages(session_id)
    if messages is None:
        return apply_cors_headers(
            JSONResponse(status_code=404, content={"error": "Session not found"}),
            request,
        )
    return apply_cors_headers(JSONResponse(content=messages), request)


@app.post("/api/v1/agent-sessions/{session_id}/messages")
async def add_agent_session_message(
    session_id: str,
    request: Request,
    payload: AgentSessionMessageRequest,
):
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response
    message = await agent_session_store.add_message(session_id, payload)
    if message is None:
        return apply_cors_headers(
            JSONResponse(status_code=404, content={"error": "Session not found"}),
            request,
        )
    return apply_cors_headers(JSONResponse(content=message), request)


@app.post("/api/v1/agent-sessions/{session_id}/abort")
async def abort_agent_session(
    session_id: str,
    request: Request,
    payload: AbortAgentSessionRequest,
):
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response
    session = await agent_session_store.abort_session(session_id, payload)
    if session is None:
        return apply_cors_headers(
            JSONResponse(status_code=404, content={"error": "Session not found"}),
            request,
        )
    return apply_cors_headers(JSONResponse(content=session), request)


@app.get("/api/v1/agent-sessions/sync")
async def agent_session_sync(request: Request):
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response

    queue = agent_session_store.subscribe()

    async def event_stream():
        try:
            yield f"data: {json.dumps({'type': 'server.connected', 'payload': {}})}\n\n"
            while not await request.is_disconnected():
                try:
                    message = await asyncio.wait_for(queue.get(), timeout=15.0)
                    yield f"data: {message}\n\n"
                except asyncio.TimeoutError:
                    yield ": heartbeat\n\n"
        finally:
            agent_session_store.unsubscribe(queue)

    return apply_cors_headers(
        StreamingResponse(
            event_stream(),
            media_type="text/event-stream",
            headers={
                "Cache-Control": "no-cache",
                "Connection": "keep-alive",
                "X-Accel-Buffering": "no",
            },
        ),
        request,
    )


@app.get("/api/v1/openclaw/agents/discovery")
async def discover_agents(request: Request):
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response
    agents = await agent_store.list_agents()
    payload = {
        "agents": agents,
        "total": len(agents),
        "unregistered": 0,
        "state_dir": str(AGENTS_STORE_PATH.parent),
        "workspace_path": str(AGENT_WORKSPACES_DIR),
        "gateway_port": 8013,
    }
    return apply_cors_headers(JSONResponse(content=payload), request)


@app.post("/api/v1/office/bootstrap")
async def office_bootstrap(request: Request, payload: OfficeBootstrapRequest):
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response
    runtime_state = await office_runtime_store.bootstrap(payload)
    return apply_cors_headers(
        JSONResponse(
            content={
                "ok": True,
                **runtime_state,
                "gateway": {
                    "base_url": "/api/v1",
                    "supports_runtime_sync": True,
                },
            }
        ),
        request,
    )


@app.post("/api/v1/office/runtime/state")
async def office_runtime_state(request: Request, payload: OfficeRuntimeSyncRequest):
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response
    runtime_state = await office_runtime_store.update_runtime_state(payload)
    if runtime_state is None:
        return apply_cors_headers(
            JSONResponse(status_code=404, content={"error": "Office binding not found"}),
            request,
        )
    return apply_cors_headers(JSONResponse(content={"ok": True, **runtime_state}), request)


@app.get("/api/v1/office/bindings")
async def list_office_bindings(request: Request):
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response
    bindings = await office_runtime_store.list_bindings()
    return apply_cors_headers(
        JSONResponse(content={"ok": True, "bindings": bindings, "count": len(bindings)}),
        request,
    )


@app.get("/api/v1/office/bindings/{binding_id}")
async def get_office_binding(binding_id: str, request: Request):
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response
    binding = await office_runtime_store.get_binding(binding_id)
    if binding is None:
        return apply_cors_headers(
            JSONResponse(status_code=404, content={"error": "Office binding not found"}),
            request,
        )
    return apply_cors_headers(JSONResponse(content={"ok": True, "binding": binding}), request)


@app.get("/api/v1/agents")
async def list_agents(request: Request):
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response
    agents = await agent_store.list_agents()
    return apply_cors_headers(
        JSONResponse(content={"ok": True, "agents": agents, "total": len(agents)}),
        request,
    )


@app.post("/api/v1/agents")
async def create_agent(request: Request, payload: CreateAgentRequest):
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response
    agent = await agent_store.create_agent(payload)
    content = {"ok": True, "data": agent, **agent}
    return apply_cors_headers(JSONResponse(status_code=201, content=content), request)


@app.get("/api/v1/agents/{agent_id}")
async def get_agent(agent_id: str, request: Request):
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response
    agent = await agent_store.get_agent(agent_id)
    if agent is None:
        return apply_cors_headers(
            JSONResponse(status_code=404, content={"error": "Agent not found"}),
            request,
        )
    return apply_cors_headers(JSONResponse(content=agent), request)


@app.patch("/api/v1/agents/{agent_id}")
async def update_agent(agent_id: str, request: Request, payload: UpdateAgentRequest):
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response
    agent = await agent_store.update_agent(agent_id, payload)
    if agent is None:
        return apply_cors_headers(
            JSONResponse(status_code=404, content={"error": "Agent not found"}),
            request,
        )
    return apply_cors_headers(JSONResponse(content=agent), request)


@app.delete("/api/v1/agents/{agent_id}")
async def delete_agent(agent_id: str, request: Request):
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response
    deleted = await agent_store.delete_agent(agent_id)
    if not deleted:
        return apply_cors_headers(
            JSONResponse(status_code=404, content={"error": "Agent not found"}),
            request,
        )
    return apply_cors_headers(Response(status_code=204), request)


@app.post("/api/v1/agents/{agent_id}/workspace/initialize")
async def initialize_agent_workspace(
    agent_id: str,
    request: Request,
    payload: InitializeWorkspaceRequest,
):
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response
    workspace = await agent_store.initialize_workspace(agent_id, payload.documents)
    if workspace is None:
        return apply_cors_headers(
            JSONResponse(status_code=404, content={"error": "Agent not found", "ok": False}),
            request,
        )
    return apply_cors_headers(
        JSONResponse(
            content={
                "ok": True,
                "data": {
                    "initialized": True,
                    "workspace_id": workspace["workspace_id"],
                    "document_count": len(payload.documents),
                },
            }
        ),
        request,
    )


@app.post("/api/v1/agents/{agent_id}/runs")
async def create_agent_run(
    agent_id: str,
    request: Request,
    payload: Dict[str, Any],
):
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response
    agent = await agent_store.get_agent(agent_id)
    if agent is None:
        return apply_cors_headers(
            JSONResponse(status_code=404, content={"error": "Agent not found"}),
            request,
        )
    run_id = str(uuid.uuid4())
    body = {
        "id": run_id,
        "agent_id": agent_id,
        "status": "pending",
        "input": payload.get("input", ""),
        "output": None,
        "metadata": payload.get("metadata") or {},
        "started_at": utc_now_iso(),
        "completed_at": None,
    }
    return apply_cors_headers(JSONResponse(status_code=201, content=body), request)


@app.get("/api/v1/agents/{agent_id}/events")
async def agent_events(agent_id: str, request: Request):
    auth_response = await ensure_authorized(request)
    if auth_response is not None:
        return auth_response

    async def event_stream():
        yield f"data: {json.dumps({'event_type': 'connected', 'agent_id': agent_id, 'timestamp': utc_now_iso(), 'data': {}})}\n\n"
        while not await request.is_disconnected():
            await asyncio.sleep(15)
            yield ": heartbeat\n\n"

    return apply_cors_headers(
        StreamingResponse(
            event_stream(),
            media_type="text/event-stream",
            headers={
                "Cache-Control": "no-cache",
                "Connection": "keep-alive",
                "X-Accel-Buffering": "no",
            },
        ),
        request,
    )


@app.options("/{path:path}")
async def options_preflight(path: str, request: Request):
    return apply_cors_headers(Response(status_code=200), request)


@app.get("/global/health")
@app.get("/v1/global/health")
async def gizzi_health(request: Request):
    return await proxy_gizzi_json(
        request,
        "/v1/global/health",
        {"healthy": False, "runtime": "unavailable"},
    )


@app.get("/session/list")
@app.get("/v1/session/list")
async def session_list(request: Request):
    return await proxy_gizzi_json(request, "/session/list", [])


@app.get("/session/status")
@app.get("/v1/session/status")
async def session_status(request: Request):
    return await proxy_gizzi_json(request, "/session/status", {})


@app.get("/global/event")
@app.get("/v1/global/event")
@app.get("/v1/events-http")
@app.get("/sessions/sync")
@app.get("/api/v1/sessions/sync")
async def global_event(request: Request):
    return await proxy_gizzi_sse(
        request,
        "/global/event",
        json.dumps({"payload": {"type": "server.connected", "properties": {}}}),
    )

if __name__ == "__main__":
    # For T0410 testing (just loading)
    print(f"LOADED_HASH={runtime.registry_hash}")
    # Run server if arg provided
    if len(sys.argv) > 1 and sys.argv[1] == "serve":
        port = int(os.environ.get("PORT", "8013"))  # Default to 8013 if not set
        host = os.environ.get("HOST", "127.0.0.1")
        uvicorn.run(app, host=host, port=port)
        
