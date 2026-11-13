"""Generated from cmd/allternit-api/openapi/selfhost-v1.yaml by scripts/platform-sdk/generate.py. Do not edit."""
# flake8: noqa
# fmt: off

from typing import Any, Dict, Iterator, List, Optional, Union

try:
    from typing import Literal, TypedDict
except ImportError:  # pragma: no cover
    from typing_extensions import Literal, TypedDict  # type: ignore

from ._core import Page, SSEEvent, Transport


class _Unset:
    def __repr__(self) -> str:
        return "NOT_GIVEN"


NOT_GIVEN: Any = _Unset()


Error = TypedDict('Error', {'error': Dict[str, Any]}, total=False)
ListEnvelope = TypedDict('ListEnvelope', {'data': List[Any], 'has_more': bool, 'next_cursor': Optional[str]}, total=False)
Deleted = TypedDict('Deleted', {'id': str, 'deleted': Literal[True]}, total=False)
ServerInfo = TypedDict('ServerInfo', {'version': str, 'provider': Literal['anthropic', 'bedrock', 'vertex', 'foundry', 'gateway'], 'features': Dict[str, Any], 'auth_modes': List[Literal['api_key', 'oidc']], 'public_url': Optional[str], 'max_tool_result_bytes': int}, total=False)
McpServer = TypedDict('McpServer', {'name': str, 'url': str, 'headers': Dict[str, Any]}, total=False)
AgentTool = TypedDict('AgentTool', {'type': Literal['builtin', 'custom'], 'name': str, 'description': str, 'input_schema': Dict[str, Any]}, total=False)
Agent = TypedDict('Agent', {'id': str, 'object': Literal['agent'], 'name': str, 'instructions': str, 'model': str, 'tools': List["AgentTool"], 'mcp_servers': List["McpServer"], 'skills': List[str], 'metadata': Dict[str, Any], 'status': Literal['ready'], 'created_at': str, 'updated_at': str}, total=False)
CreateAgentRequest = TypedDict('CreateAgentRequest', {'name': str, 'instructions': str, 'model': str, 'tools': List["AgentTool"], 'mcp_servers': List["McpServer"], 'skills': List[str], 'metadata': Dict[str, Any]}, total=False)
UpdateAgentRequest = TypedDict('UpdateAgentRequest', {'name': str, 'instructions': str, 'model': str, 'tools': List["AgentTool"], 'mcp_servers': List["McpServer"], 'skills': List[str], 'metadata': Dict[str, Any]}, total=False)
NetworkMode = Literal['none', 'limited', 'full']
GitPolicy = TypedDict('GitPolicy', {'allowed_hostnames': List[str], 'allowed_push_refs': List[str]}, total=False)
McpPolicy = TypedDict('McpPolicy', {'allowed_hostnames': List[str]}, total=False)
EnvironmentImage = TypedDict('EnvironmentImage', {'image': str, 'packages': Dict[str, Any]}, total=False)
Environment = TypedDict('Environment', {'id': str, 'object': Literal['environment'], 'name': str, 'network': "NetworkMode", 'allowed_hosts': List[str], 'git': "GitPolicy", 'mcp': "McpPolicy", 'image': Optional[str], 'packages': Dict[str, Any], 'worker_count': int, 'queue_depth': int, 'metadata': Dict[str, Any], 'created_at': str, 'updated_at': str}, total=False)
CreateEnvironmentRequest = TypedDict('CreateEnvironmentRequest', {'name': str, 'network': "NetworkMode", 'allowed_hosts': List[str], 'git': "GitPolicy", 'mcp': "McpPolicy", 'image': str, 'packages': Dict[str, Any], 'metadata': Dict[str, Any]}, total=False)
UpdateEnvironmentRequest = TypedDict('UpdateEnvironmentRequest', {'name': str, 'network': "NetworkMode", 'allowed_hosts': List[str], 'git': "GitPolicy", 'mcp': "McpPolicy", 'image': Optional[str], 'packages': Dict[str, Any], 'metadata': Dict[str, Any]}, total=False)
EnvironmentPolicy = TypedDict('EnvironmentPolicy', {'network': "NetworkMode", 'allowed_hosts': List[str], 'git': "GitPolicy", 'mcp': "McpPolicy", 'image': Optional[str], 'packages': Dict[str, Any]}, total=False)
SessionStatus = Literal['idle', 'running', 'waiting_for_worker', 'interrupted', 'error', 'archived']
Session = TypedDict('Session', {'id': str, 'object': Literal['session'], 'agent_id': str, 'environment_id': str, 'title': Optional[str], 'status': "SessionStatus", 'status_reason': Optional[str], 'last_seq': int, 'metadata': Dict[str, Any], 'created_at': str, 'updated_at': str, 'archived_at': Optional[str]}, total=False)
EventType = Literal['message', 'tool_call', 'tool_result', 'status', 'error']
Event = TypedDict('Event', {'id': str, 'object': Literal['event'], 'session_id': str, 'seq': int, 'type': "EventType", 'data': Union["MessageData", "ToolCallData", "ToolResultData", "StatusData", "ErrorData"], 'created_at': str}, total=False)
MessageData = TypedDict('MessageData', {'role': Literal['user', 'assistant'], 'content': str}, total=False)
ToolCallData = TypedDict('ToolCallData', {'tool_call_id': str, 'name': str, 'input': Dict[str, Any]}, total=False)
ToolResultData = TypedDict('ToolResultData', {'tool_call_id': str, 'output': str, 'is_error': bool}, total=False)
StatusData = TypedDict('StatusData', {'status': "SessionStatus", 'reason': Optional[str]}, total=False)
ErrorData = TypedDict('ErrorData', {'code': str, 'message': str}, total=False)
WorkClaim = TypedDict('WorkClaim', {'work_id': str, 'session_id': str, 'session_secret': str, 'lease_generation': int, 'lease_expires_at': str, 'environment': "EnvironmentPolicy"}, total=False)
Lease = TypedDict('Lease', {'work_id': str, 'lease_generation': int, 'lease_expires_at': Optional[str], 'status': Literal['claimed', 'ready', 'completed']}, total=False)
ToolCall = TypedDict('ToolCall', {'tool_call_id': str, 'name': str, 'input': Dict[str, Any]}, total=False)
Role = Literal['admin', 'developer', 'viewer']
ApiKey = TypedDict('ApiKey', {'id': str, 'object': Literal['api_key'], 'name': str, 'role': "Role", 'prefix': str, 'created_at': str, 'expires_at': Optional[str], 'last_used_at': Optional[str], 'revoked_at': Optional[str]}, total=False)
ApiKeyWithSecret = Dict[str, Any]
EnvironmentKey = TypedDict('EnvironmentKey', {'id': str, 'object': Literal['environment_key'], 'environment_id': str, 'name': Optional[str], 'prefix': str, 'created_at': str, 'expires_at': Optional[str], 'last_used_at': Optional[str], 'revoked_at': Optional[str]}, total=False)
EnvironmentKeyWithSecret = Dict[str, Any]


class ServerResource:
    def __init__(self, client: Transport) -> None:
        self._client = client

    def get_health_live(self, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Liveness

        200 while the process serves requests. No auth. For container and load-balancer probes.

        ``GET /health/live``
        """
        return self._client.request("GET", f"/health/live", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get_health_ready(self, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Readiness

        200 when the database answers and the agent loop (the gizzi child) is
        ready. 503 `store_unavailable` when the database does not answer, 503
        `agent_loop_unavailable` while the agent loop is starting or restarting,
        or when it cannot run (for example the gizzi-code binary is missing). No auth.

        ``GET /health/ready``
        """
        return self._client.request("GET", f"/health/ready", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get_info(self, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "ServerInfo":
        """Server info

        Version, inference provider, enabled features and accepted auth modes. Safe to call before you sign in, so a client can show the right sign-in screen. Reveals no secrets.

        ``GET /v1/server/info``
        """
        return self._client.request("GET", f"/v1/server/info", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]


class AgentsResource:
    def __init__(self, client: Transport) -> None:
        self._client = client

    def list(self, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """List agents

        Agents, oldest first.

        ``GET /v1/agents``
        """
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        return self._client.request("GET", f"/v1/agents", query=_query, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_all(self, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Iterator["Agent"]:
        """Every item of ``list``, fetching pages as you iterate."""
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        def _fetch(cursor: Optional[str]) -> Page:
            q = dict(_query)
            if cursor is not None:
                q['after'] = cursor
            return self._client.request("GET", f"/v1/agents", query=q, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]
        return self._client.paginate(_fetch, after if after is not NOT_GIVEN else None)

    def create(self, *, name: str, model: str, instructions: str = NOT_GIVEN, tools: List["AgentTool"] = NOT_GIVEN, mcp_servers: List["McpServer"] = NOT_GIVEN, skills: List[str] = NOT_GIVEN, metadata: Dict[str, Any] = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Agent":
        """Create an agent

        An agent is a model, a system prompt and a set of tools. If the server
        uses a gateway provider, `model` must be in the settings `model_map`
        (else `422 model_not_mapped`). Needs role `developer` or `admin`.

        ``POST /v1/agents``
        """
        _body: Dict[str, Any] = {}
        if name is not NOT_GIVEN:
            _body['name'] = name
        if instructions is not NOT_GIVEN:
            _body['instructions'] = instructions
        if model is not NOT_GIVEN:
            _body['model'] = model
        if tools is not NOT_GIVEN:
            _body['tools'] = tools
        if mcp_servers is not NOT_GIVEN:
            _body['mcp_servers'] = mcp_servers
        if skills is not NOT_GIVEN:
            _body['skills'] = skills
        if metadata is not NOT_GIVEN:
            _body['metadata'] = metadata
        return self._client.request("POST", f"/v1/agents", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Agent":
        """Retrieve an agent

        ``GET /v1/agents/{id}``
        """
        return self._client.request("GET", f"/v1/agents/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def update(self, id: str, *, name: str = NOT_GIVEN, instructions: str = NOT_GIVEN, model: str = NOT_GIVEN, tools: List["AgentTool"] = NOT_GIVEN, mcp_servers: List["McpServer"] = NOT_GIVEN, skills: List[str] = NOT_GIVEN, metadata: Dict[str, Any] = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Agent":
        """Update an agent

        Absent fields are unchanged. Arrays and `metadata` replace the old value. Running sessions keep the agent version they started with.

        ``PATCH /v1/agents/{id}``
        """
        _body: Dict[str, Any] = {}
        if name is not NOT_GIVEN:
            _body['name'] = name
        if instructions is not NOT_GIVEN:
            _body['instructions'] = instructions
        if model is not NOT_GIVEN:
            _body['model'] = model
        if tools is not NOT_GIVEN:
            _body['tools'] = tools
        if mcp_servers is not NOT_GIVEN:
            _body['mcp_servers'] = mcp_servers
        if skills is not NOT_GIVEN:
            _body['skills'] = skills
        if metadata is not NOT_GIVEN:
            _body['metadata'] = metadata
        return self._client.request("PATCH", f"/v1/agents/{_q(id)}", body=_body, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def delete(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Deleted":
        """Delete an agent

        Fails with 409 `agent_in_use` while the agent has a non-archived session.

        ``DELETE /v1/agents/{id}``
        """
        return self._client.request("DELETE", f"/v1/agents/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]


class EnvironmentsResource:
    def __init__(self, client: Transport) -> None:
        self._client = client

    def list(self, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """List environments

        An environment is the place where an agent's tools run, with its own network rules. Oldest first.

        ``GET /v1/environments``
        """
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        return self._client.request("GET", f"/v1/environments", query=_query, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_all(self, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Iterator["Environment"]:
        """Every item of ``list``, fetching pages as you iterate."""
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        def _fetch(cursor: Optional[str]) -> Page:
            q = dict(_query)
            if cursor is not None:
                q['after'] = cursor
            return self._client.request("GET", f"/v1/environments", query=q, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]
        return self._client.paginate(_fetch, after if after is not NOT_GIVEN else None)

    def create(self, *, name: str, network: "NetworkMode" = NOT_GIVEN, allowed_hosts: List[str] = NOT_GIVEN, git: "GitPolicy" = NOT_GIVEN, mcp: "McpPolicy" = NOT_GIVEN, image: str = NOT_GIVEN, packages: Dict[str, Any] = NOT_GIVEN, metadata: Dict[str, Any] = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Environment":
        """Create an environment

        Network defaults to `none`. Workers enforce the policy on every
        outbound connection. An environment's `git` and `mcp` lists can only
        narrow the server-wide lists in the settings file, never widen them.

        ``POST /v1/environments``
        """
        _body: Dict[str, Any] = {}
        if name is not NOT_GIVEN:
            _body['name'] = name
        if network is not NOT_GIVEN:
            _body['network'] = network
        if allowed_hosts is not NOT_GIVEN:
            _body['allowed_hosts'] = allowed_hosts
        if git is not NOT_GIVEN:
            _body['git'] = git
        if mcp is not NOT_GIVEN:
            _body['mcp'] = mcp
        if image is not NOT_GIVEN:
            _body['image'] = image
        if packages is not NOT_GIVEN:
            _body['packages'] = packages
        if metadata is not NOT_GIVEN:
            _body['metadata'] = metadata
        return self._client.request("POST", f"/v1/environments", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Environment":
        """Retrieve an environment

        ``GET /v1/environments/{id}``
        """
        return self._client.request("GET", f"/v1/environments/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def update(self, id: str, *, name: str = NOT_GIVEN, network: "NetworkMode" = NOT_GIVEN, allowed_hosts: List[str] = NOT_GIVEN, git: "GitPolicy" = NOT_GIVEN, mcp: "McpPolicy" = NOT_GIVEN, image: Optional[str] = NOT_GIVEN, packages: Dict[str, Any] = NOT_GIVEN, metadata: Dict[str, Any] = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Environment":
        """Update an environment

        Policy changes reach a worker on its next claim. Work already claimed keeps the policy it was claimed with.

        ``PATCH /v1/environments/{id}``
        """
        _body: Dict[str, Any] = {}
        if name is not NOT_GIVEN:
            _body['name'] = name
        if network is not NOT_GIVEN:
            _body['network'] = network
        if allowed_hosts is not NOT_GIVEN:
            _body['allowed_hosts'] = allowed_hosts
        if git is not NOT_GIVEN:
            _body['git'] = git
        if mcp is not NOT_GIVEN:
            _body['mcp'] = mcp
        if image is not NOT_GIVEN:
            _body['image'] = image
        if packages is not NOT_GIVEN:
            _body['packages'] = packages
        if metadata is not NOT_GIVEN:
            _body['metadata'] = metadata
        return self._client.request("PATCH", f"/v1/environments/{_q(id)}", body=_body, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def delete(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Deleted":
        """Delete an environment

        Fails with 409 `environment_in_use` while a non-archived session uses it. Revokes the environment's keys.

        ``DELETE /v1/environments/{id}``
        """
        return self._client.request("DELETE", f"/v1/environments/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_workers(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """List workers seen for an environment

        Workers that claimed, heartbeat or renewed in the last 10 minutes, newest first. `online` is true when the worker was seen in the last 60 seconds.

        ``GET /v1/environments/{id}/workers``
        """
        return self._client.request("GET", f"/v1/environments/{_q(id)}/workers", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]


class EnvironmentKeysResource:
    def __init__(self, client: Transport) -> None:
        self._client = client

    def list(self, id: str, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """List environment keys

        Keys for this environment. The secret is never shown again. Needs role `admin`.

        ``GET /v1/environments/{id}/keys``
        """
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        return self._client.request("GET", f"/v1/environments/{_q(id)}/keys", query=_query, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_all(self, id: str, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Iterator["EnvironmentKey"]:
        """Every item of ``list``, fetching pages as you iterate."""
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        def _fetch(cursor: Optional[str]) -> Page:
            q = dict(_query)
            if cursor is not None:
                q['after'] = cursor
            return self._client.request("GET", f"/v1/environments/{_q(id)}/keys", query=q, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]
        return self._client.paginate(_fetch, after if after is not NOT_GIVEN else None)

    def create(self, id: str, *, name: str = NOT_GIVEN, expires_at: Optional[str] = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "EnvironmentKeyWithSecret":
        """Create an environment key

        Returns the `aek_...` secret once. Give it to the workers of this environment. Needs role `admin`.

        ``POST /v1/environments/{id}/keys``
        """
        _body: Dict[str, Any] = {}
        if name is not NOT_GIVEN:
            _body['name'] = name
        if expires_at is not NOT_GIVEN:
            _body['expires_at'] = expires_at
        return self._client.request("POST", f"/v1/environments/{_q(id)}/keys", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def revoke(self, id: str, key_id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Deleted":
        """Revoke an environment key

        Takes effect at once. Workers using the key get 401 on their next call. Needs role `admin`.

        ``DELETE /v1/environments/{id}/keys/{key_id}``
        """
        return self._client.request("DELETE", f"/v1/environments/{_q(id)}/keys/{_q(key_id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]


class SessionsResource:
    def __init__(self, client: Transport) -> None:
        self._client = client

    def list(self, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, agent_id: str = NOT_GIVEN, environment_id: str = NOT_GIVEN, status: "SessionStatus" = NOT_GIVEN, include_archived: bool = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """List sessions

        Newest first. Archived sessions are hidden unless `include_archived` is true.

        ``GET /v1/sessions``
        """
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        if agent_id is not NOT_GIVEN:
            _query['agent_id'] = agent_id
        if environment_id is not NOT_GIVEN:
            _query['environment_id'] = environment_id
        if status is not NOT_GIVEN:
            _query['status'] = status
        if include_archived is not NOT_GIVEN:
            _query['include_archived'] = include_archived
        return self._client.request("GET", f"/v1/sessions", query=_query, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_all(self, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, agent_id: str = NOT_GIVEN, environment_id: str = NOT_GIVEN, status: "SessionStatus" = NOT_GIVEN, include_archived: bool = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Iterator["Session"]:
        """Every item of ``list``, fetching pages as you iterate."""
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        if agent_id is not NOT_GIVEN:
            _query['agent_id'] = agent_id
        if environment_id is not NOT_GIVEN:
            _query['environment_id'] = environment_id
        if status is not NOT_GIVEN:
            _query['status'] = status
        if include_archived is not NOT_GIVEN:
            _query['include_archived'] = include_archived
        def _fetch(cursor: Optional[str]) -> Page:
            q = dict(_query)
            if cursor is not None:
                q['after'] = cursor
            return self._client.request("GET", f"/v1/sessions", query=q, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]
        return self._client.paginate(_fetch, after if after is not NOT_GIVEN else None)

    def create(self, *, agent_id: str, environment_id: str, title: str = NOT_GIVEN, metadata: Dict[str, Any] = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Session":
        """Create a session

        A session pairs one agent with one environment. Creating it does not start a turn. Send a message to do that. Returns 422 `model_not_mapped` when the agent's model is not in the model map, with no outbound request made.

        ``POST /v1/sessions``
        """
        _body: Dict[str, Any] = {}
        if agent_id is not NOT_GIVEN:
            _body['agent_id'] = agent_id
        if environment_id is not NOT_GIVEN:
            _body['environment_id'] = environment_id
        if title is not NOT_GIVEN:
            _body['title'] = title
        if metadata is not NOT_GIVEN:
            _body['metadata'] = metadata
        return self._client.request("POST", f"/v1/sessions", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Session":
        """Retrieve a session

        ``GET /v1/sessions/{id}``
        """
        return self._client.request("GET", f"/v1/sessions/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def archive(self, id: str, *, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Session":
        """Archive a session

        Stops the session and hides it from the default list. Events stay readable. Archiving cannot be undone.

        ``POST /v1/sessions/{id}/archive``
        """
        return self._client.request("POST", f"/v1/sessions/{_q(id)}/archive", idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def send_message(self, id: str, *, content: str, metadata: Dict[str, Any] = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Event":
        """Send a user message

        Adds a user turn and starts the agent loop. Returns at once with the
        event that was stored. Follow progress on the events stream. A model-only
        turn needs no worker. A turn that calls a tool waits until a worker
        for the environment is attached; the session shows `waiting_for_worker`.
        Returns 409 `session_busy` if a turn is already running, and 409
        `session_archived` for an archived session. The agent's model is checked
        against the server's model map again here (it may have changed since the
        session was made): an unmapped model is 422 `model_not_mapped`, with no
        outbound request. While the agent loop is starting or cannot run, the
        answer is 503 `agent_loop_unavailable`; a server built without an agent
        loop answers 501 `not_implemented`. In those cases nothing is stored.
        A turn that fails later ends with an `error` event and status `error`.

        ``POST /v1/sessions/{id}/messages``
        """
        _body: Dict[str, Any] = {}
        if content is not NOT_GIVEN:
            _body['content'] = content
        if metadata is not NOT_GIVEN:
            _body['metadata'] = metadata
        return self._client.request("POST", f"/v1/sessions/{_q(id)}/messages", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def interrupt(self, id: str, *, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Session":
        """Interrupt the running turn

        Stops the current turn. Pending tool calls are cancelled. The session's work item stays with its worker, which keeps serving the session's next turns. Does nothing if the session is idle.

        ``POST /v1/sessions/{id}/interrupt``
        """
        return self._client.request("POST", f"/v1/sessions/{_q(id)}/interrupt", idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]


class EventsResource:
    def __init__(self, client: Transport) -> None:
        self._client = client

    def list_session_events(self, id: str, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, after_seq: int = NOT_GIVEN, types: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """List session events

        The session log, oldest first, in pages. Events are never edited or removed. `seq` rises by one for each event in a session.

        ``GET /v1/sessions/{id}/events``
        """
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        if after_seq is not NOT_GIVEN:
            _query['after_seq'] = after_seq
        if types is not NOT_GIVEN:
            _query['types'] = types
        return self._client.request("GET", f"/v1/sessions/{_q(id)}/events", query=_query, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_session_events_all(self, id: str, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, after_seq: int = NOT_GIVEN, types: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Iterator["Event"]:
        """Every item of ``list_session_events``, fetching pages as you iterate."""
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        if after_seq is not NOT_GIVEN:
            _query['after_seq'] = after_seq
        if types is not NOT_GIVEN:
            _query['types'] = types
        def _fetch(cursor: Optional[str]) -> Page:
            q = dict(_query)
            if cursor is not None:
                q['after'] = cursor
            return self._client.request("GET", f"/v1/sessions/{_q(id)}/events", query=q, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]
        return self._client.paginate(_fetch, after if after is not NOT_GIVEN else None)

    def stream_session_events(self, id: str, *, after_seq: int = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Iterator[SSEEvent]:
        """Stream session events (SSE)

        Server-sent events. Each frame has `id: <seq>`, `event: <type>` and
        `data: <Event JSON>`. Event types: `message`, `tool_call`,
        `tool_result`, `status`, `error`. A `: keepalive` comment goes out every
        15 seconds. To resume after a dropped connection, send `Last-Event-ID`
        (or `after_seq`); the server replays everything after it, then goes live.

        ``GET /v1/sessions/{id}/events/stream``
        """
        _query: Dict[str, Any] = {}
        if after_seq is not NOT_GIVEN:
            _query['after_seq'] = after_seq
        return self._client.stream_request("GET", f"/v1/sessions/{_q(id)}/events/stream", query=_query, timeout=timeout, extra_headers=extra_headers)


class WorkResource:
    def __init__(self, client: Transport) -> None:
        self._client = client

    def claim(self, id: str, *, worker_id: str, block_ms: int = NOT_GIVEN, reclaim_older_than_ms: int = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Optional["WorkClaim"]:
        """Claim a work item

        Worker side. Claims the oldest claimable work item for this environment and
        starts a 60 second lease. Waits up to `block_ms` for work to appear and
        answers as soon as an item is queued. Returns 204 when there is nothing.

        Claimable: an item that is ready, or one whose lease ran out (a worker that
        died). With `reclaim_older_than_ms`, also an item whose lease is still live
        but was last renewed at least that long ago; pick a value above the 20 s
        renew interval. Each claim raises the item's `lease_generation`, so the old
        worker gets 409 `lease_stale` on its next call. Tool calls the old worker
        took but did not answer are handed out again.

        The server also returns expired leases to the queue every 5 seconds.
        Each session has at most one open work item, queued the first time a turn
        needs a sandbox tool. Auth: environment key of this environment.

        ``POST /v1/environments/{id}/work/claim``
        """
        _body: Dict[str, Any] = {}
        if worker_id is not NOT_GIVEN:
            _body['worker_id'] = worker_id
        if block_ms is not NOT_GIVEN:
            _body['block_ms'] = block_ms
        if reclaim_older_than_ms is not NOT_GIVEN:
            _body['reclaim_older_than_ms'] = reclaim_older_than_ms
        return self._client.request("POST", f"/v1/environments/{_q(id)}/work/claim", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def heartbeat(self, work_id: str, *, lease_generation: int, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Lease":
        """Heartbeat a work item

        Tells the server the worker is alive, without moving the lease. Send it between renewals. 409 `lease_stale` if the generation is old.

        ``POST /v1/work/{work_id}/heartbeat``
        """
        _body: Dict[str, Any] = {}
        if lease_generation is not NOT_GIVEN:
            _body['lease_generation'] = lease_generation
        return self._client.request("POST", f"/v1/work/{_q(work_id)}/heartbeat", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def renew(self, work_id: str, *, lease_generation: int, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Lease":
        """Renew a lease

        Moves `lease_expires_at` to 60 seconds from now. Renew every 20 seconds. 409 `lease_stale` if the generation is old, which means another worker owns the item now. Stop at once.

        ``POST /v1/work/{work_id}/renew``
        """
        _body: Dict[str, Any] = {}
        if lease_generation is not NOT_GIVEN:
            _body['lease_generation'] = lease_generation
        return self._client.request("POST", f"/v1/work/{_q(work_id)}/renew", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def complete(self, work_id: str, *, lease_generation: int, outcome: Literal['success', 'failed', 'cancelled'] = NOT_GIVEN, error_message: str = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Lease":
        """Complete a work item

        Marks the item done and ends the lease. 409 `lease_stale` if the generation is old. Completing twice with the current generation returns 200 both times.

        ``POST /v1/work/{work_id}/complete``
        """
        _body: Dict[str, Any] = {}
        if lease_generation is not NOT_GIVEN:
            _body['lease_generation'] = lease_generation
        if outcome is not NOT_GIVEN:
            _body['outcome'] = outcome
        if error_message is not NOT_GIVEN:
            _body['error_message'] = error_message
        return self._client.request("POST", f"/v1/work/{_q(work_id)}/complete", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def release(self, work_id: str, *, lease_generation: int, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Lease":
        """Hand a work item back

        The worker is shutting down (or was started with `--drain` and is done) and gives the item back. The lease ends, the item goes back to ready, and another worker can claim it without waiting for the lease to run out. Tool calls that were running are retried (at-least-once). 409 `lease_stale` if the generation is old.

        ``POST /v1/work/{work_id}/release``
        """
        _body: Dict[str, Any] = {}
        if lease_generation is not NOT_GIVEN:
            _body['lease_generation'] = lease_generation
        return self._client.request("POST", f"/v1/work/{_q(work_id)}/release", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]


class ToolCallsResource:
    def __init__(self, client: Transport) -> None:
        self._client = client

    def next(self, work_id: str, *, block_ms: int = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Optional["ToolCall"]:
        """Get the next tool call

        Worker side. Long-polls up to `block_ms` for the next tool call of the
        session. Returns 204 when none arrived. A tool call that was handed out but has no result yet
        is handed out again after the lease changes hands (at-least-once), with the
        same `tool_call_id`. Run it once per id. Auth: the session secret.

        **Session finished.** When the session will send no more tool calls, the
        server answers 204 with the header `X-Session-Closed: true`. That happens
        when the session is archived, or when it has had no turn running and no
        pending tool call for `sessions.idle_timeout` (default 1 hour). The worker
        then calls `/complete` for the work item. The session's next turn that
        needs a tool queues a new item. A 204 without the header only means
        "nothing yet, poll again".

        ``POST /v1/work/{work_id}/tool_calls/next``
        """
        _body: Dict[str, Any] = {}
        if block_ms is not NOT_GIVEN:
            _body['block_ms'] = block_ms
        return self._client.request("POST", f"/v1/work/{_q(work_id)}/tool_calls/next", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def submit_tool_result(self, id: str, *, output: str, is_error: bool = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Submit a tool result

        Posts the output of a tool call. Sending the same id again stores nothing new and returns 200 (idempotent on `tool_call_id`). Output over 100 KiB is refused with 413 `result_too_large`; the worker should truncate and retry with `is_error` as needed. Auth is the session secret of the session that owns the call.

        ``POST /v1/tool_calls/{id}/result``
        """
        _body: Dict[str, Any] = {}
        if output is not NOT_GIVEN:
            _body['output'] = output
        if is_error is not NOT_GIVEN:
            _body['is_error'] = is_error
        return self._client.request("POST", f"/v1/tool_calls/{_q(id)}/result", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]


class APIKeysResource:
    def __init__(self, client: Transport) -> None:
        self._client = client

    def list(self, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """List server API keys

        Prefix and metadata only. The secret is never shown again. Needs role `admin`.

        ``GET /v1/api_keys``
        """
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        return self._client.request("GET", f"/v1/api_keys", query=_query, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_all(self, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Iterator["ApiKey"]:
        """Every item of ``list``, fetching pages as you iterate."""
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        def _fetch(cursor: Optional[str]) -> Page:
            q = dict(_query)
            if cursor is not None:
                q['after'] = cursor
            return self._client.request("GET", f"/v1/api_keys", query=q, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]
        return self._client.paginate(_fetch, after if after is not NOT_GIVEN else None)

    def create(self, *, name: str, role: "Role", expires_at: Optional[str] = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "ApiKeyWithSecret":
        """Create a server API key

        Returns the `ask_...` secret once. The server stores only a hash. Needs role `admin`.

        First key: run `allternit-agents-server admin setup`, or call this route
        with the setup token (the `ALLTERNIT_SELF_HOSTED_SETUP_TOKEN` environment
        variable of the server) while no API key exists. The first key must have
        role `admin` (else 400 `first_key_must_be_admin`). Once any key exists, the
        setup token is refused.

        ``POST /v1/api_keys``
        """
        _body: Dict[str, Any] = {}
        if name is not NOT_GIVEN:
            _body['name'] = name
        if role is not NOT_GIVEN:
            _body['role'] = role
        if expires_at is not NOT_GIVEN:
            _body['expires_at'] = expires_at
        return self._client.request("POST", f"/v1/api_keys", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def revoke(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Deleted":
        """Revoke a server API key

        Takes effect at once. The last admin key cannot be revoked (409 `last_admin_key`). Needs role `admin`.

        ``DELETE /v1/api_keys/{id}``
        """
        return self._client.request("DELETE", f"/v1/api_keys/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]


def _q(v: str) -> str:
    from urllib.parse import quote
    return quote(str(v), safe='')


class GeneratedResources:
    """Every API area as a client attribute. ``SelfHostClient`` extends this."""

    def __init__(self, client: Transport) -> None:
        self.server = ServerResource(client)
        self.agents = AgentsResource(client)
        self.environments = EnvironmentsResource(client)
        self.environment_keys = EnvironmentKeysResource(client)
        self.sessions = SessionsResource(client)
        self.events = EventsResource(client)
        self.work = WorkResource(client)
        self.tool_calls = ToolCallsResource(client)
        self.api_keys = APIKeysResource(client)
