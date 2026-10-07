"""Generated from cmd/allternit-cloud-api/openapi/platform-v1.yaml by scripts/platform-sdk/generate.py. Do not edit."""
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
Account = TypedDict('Account', {'id': str, 'object': Literal['account'], 'name': str, 'external_ref': Optional[str], 'metadata': Dict[str, Any], 'created_at': str}, total=False)
Agent = TypedDict('Agent', {'id': str, 'object': Literal['agent'], 'account_id': str, 'name': str, 'instructions': str, 'greeting': str, 'model': str, 'voice': "StockVoice", 'tools': List["AgentTool"], 'autonomy': "Autonomy", 'transfer_targets': List[str], 'business_hours': Optional[Dict[str, Any]], 'metadata': Dict[str, Any], 'status': Literal['pending', 'ready'], 'created_at': str, 'updated_at': str}, total=False)
KnowledgeFile = TypedDict('KnowledgeFile', {'id': str, 'object': Literal['knowledge_file'], 'agent_id': str, 'account_id': str, 'name': str, 'content_type': str, 'bytes': int, 'chunk_count': int, 'created_at': str}, total=False)
AgentTool = Literal['send_text', 'call', 'email', 'channel_post', 'calendar', 'web_fetch', 'web_search', 'knowledge_search', 'ask_human', 'transfer']
Autonomy = Literal['draft', 'ask', 'tell', 'limits']
StockVoice = Literal['af_alloy', 'af_aoede', 'af_bella', 'af_heart', 'af_jessica', 'af_kore', 'af_nicole', 'af_nova', 'af_river', 'af_sarah', 'af_sky', 'am_adam', 'am_echo', 'am_eric', 'am_fenrir', 'am_liam', 'am_michael', 'am_onyx', 'am_puck', 'am_santa', 'bf_alice', 'bf_emma', 'bf_isabella', 'bf_lily', 'bm_daniel', 'bm_fable', 'bm_george', 'bm_lewis']
Conversation = TypedDict('Conversation', {'id': str, 'object': Literal['conversation'], 'agent_id': str, 'account_id': str, 'metadata': Dict[str, Any], 'created_at': str, 'messages': List["ConversationMessage"]}, total=False)
ConversationMessage = TypedDict('ConversationMessage', {'id': str, 'object': Literal['conversation.message'], 'conversation_id': str, 'role': Literal['user', 'assistant'], 'content': str, 'status': Literal['completed', 'failed'], 'error': Optional[str], 'created_at': str}, total=False)
ModelKey = TypedDict('ModelKey', {'provider': Literal['anthropic', 'openai', 'xai'], 'object': Literal['model_key'], 'masked': str, 'created_at': str, 'updated_at': str}, total=False)
UsageRow = TypedDict('UsageRow', {'group': Optional[str], 'meter': Literal['voice_min_allternit', 'voice_min_byok', 'agent_month', 'tokens_in', 'tokens_out', 'number_local_month', 'number_tollfree_month', 'sms_segment', 'mms', 'registration_passthrough_cents', 'recording_min_month'], 'unit': Optional[str], 'quantity': float, 'events': int}, total=False)
PhoneNumber = TypedDict('PhoneNumber', {'id': str, 'object': Literal['phone_number'], 'account_id': Optional[str], 'e164': str, 'type': Literal['local', 'toll_free'], 'sms_state': Literal['pending_registration', 'active', 'rejected', 'blocked'], 'simulated': bool, 'created_at': str}, total=False)
PhoneNumberList = TypedDict('PhoneNumberList', {'data': List["PhoneNumber"], 'has_more': bool, 'next_cursor': Optional[str]}, total=False)
Message = TypedDict('Message', {'id': str, 'object': Literal['message'], 'account_id': Optional[str], 'number_id': str, 'direction': Literal['inbound', 'outbound'], 'from': str, 'to': str, 'body': str, 'segments': int, 'status': Literal['sent', 'simulated', 'received', 'failed'], 'error_code': Optional[str], 'media': List[Dict[str, Any]], 'created_at': str}, total=False)
MessageList = TypedDict('MessageList', {'data': List["Message"], 'has_more': bool, 'next_cursor': Optional[str]}, total=False)
WebhookEndpoint = TypedDict('WebhookEndpoint', {'id': str, 'object': Literal['webhook_endpoint'], 'url': str, 'events': List[str], 'description': Optional[str], 'created_at': str}, total=False)
Event = TypedDict('Event', {'id': str, 'object': Literal['event'], 'type': Literal['message.received', 'message.status', 'registration.updated', 'webhook.test'], 'created': int, 'project_id': str, 'account_id': Optional[str], 'data': Dict[str, Any]}, total=False)


class AccountsResource:
    def __init__(self, client: Transport) -> None:
        self._client = client

    def list(self, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, external_ref: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """List accounts

        Accounts in the project, oldest first. A key bound to an account sees only that account. Requires a resource scope.

        ``GET /v1/accounts``
        """
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        if external_ref is not NOT_GIVEN:
            _query['external_ref'] = external_ref
        return self._client.request("GET", f"/v1/accounts", query=_query, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_all(self, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, external_ref: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Iterator["Account"]:
        """Every item of ``list``, fetching pages as you iterate."""
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        if external_ref is not NOT_GIVEN:
            _query['external_ref'] = external_ref
        def _fetch(cursor: Optional[str]) -> Page:
            q = dict(_query)
            if cursor is not None:
                q['after'] = cursor
            return self._client.request("GET", f"/v1/accounts", query=q, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]
        return self._client.paginate(_fetch, after if after is not NOT_GIVEN else None)

    def create(self, *, name: str, external_ref: str = NOT_GIVEN, metadata: Dict[str, Any] = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Account":
        """Create an account

        One account per end-customer business. Not available to account-bound keys.

        ``POST /v1/accounts``
        """
        _body: Dict[str, Any] = {}
        if name is not NOT_GIVEN:
            _body['name'] = name
        if external_ref is not NOT_GIVEN:
            _body['external_ref'] = external_ref
        if metadata is not NOT_GIVEN:
            _body['metadata'] = metadata
        return self._client.request("POST", f"/v1/accounts", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Account":
        """Retrieve an account

        ``GET /v1/accounts/{id}``
        """
        return self._client.request("GET", f"/v1/accounts/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def update(self, id: str, *, name: str = NOT_GIVEN, external_ref: Optional[str] = NOT_GIVEN, metadata: Dict[str, Any] = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Account":
        """Update an account

        Absent fields are unchanged; `external_ref: null` clears it; `metadata` replaces the object.

        ``PATCH /v1/accounts/{id}``
        """
        _body: Dict[str, Any] = {}
        if name is not NOT_GIVEN:
            _body['name'] = name
        if external_ref is not NOT_GIVEN:
            _body['external_ref'] = external_ref
        if metadata is not NOT_GIVEN:
            _body['metadata'] = metadata
        return self._client.request("PATCH", f"/v1/accounts/{_q(id)}", body=_body, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def delete(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Delete an account

        Soft delete. Also revokes the API keys bound to the account. Not available to account-bound keys.

        ``DELETE /v1/accounts/{id}``
        """
        return self._client.request("DELETE", f"/v1/accounts/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]


class AgentsResource:
    def __init__(self, client: Transport) -> None:
        self._client = client

    def list(self, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, account_id: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """List agents

        Agents in the project, oldest first. A key bound to an account sees only that account's agents. Requires the `agents` scope.

        ``GET /v1/agents``
        """
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        if account_id is not NOT_GIVEN:
            _query['account_id'] = account_id
        return self._client.request("GET", f"/v1/agents", query=_query, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_all(self, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, account_id: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Iterator["Agent"]:
        """Every item of ``list``, fetching pages as you iterate."""
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        if account_id is not NOT_GIVEN:
            _query['account_id'] = account_id
        def _fetch(cursor: Optional[str]) -> Page:
            q = dict(_query)
            if cursor is not None:
                q['after'] = cursor
            return self._client.request("GET", f"/v1/agents", query=q, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]
        return self._client.paginate(_fetch, after if after is not NOT_GIVEN else None)

    def create(self, *, name: str, account_id: str = NOT_GIVEN, instructions: str = NOT_GIVEN, greeting: str = NOT_GIVEN, model: str = NOT_GIVEN, voice: "StockVoice" = NOT_GIVEN, tools: List["AgentTool"] = NOT_GIVEN, autonomy: "Autonomy" = NOT_GIVEN, transfer_targets: List[str] = NOT_GIVEN, business_hours: Dict[str, Any] = NOT_GIVEN, metadata: Dict[str, Any] = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Agent":
        """Create an agent

        A hosted agent for one of your accounts. The greeting must say the caller is talking to an AI
        (`greeting_missing_ai_disclosure` otherwise); voices are stock voices only; tools come from the
        launch list. Sandbox projects can have 3 agents (`agent_limit_reached`).

        ``POST /v1/agents``
        """
        _body: Dict[str, Any] = {}
        if account_id is not NOT_GIVEN:
            _body['account_id'] = account_id
        if name is not NOT_GIVEN:
            _body['name'] = name
        if instructions is not NOT_GIVEN:
            _body['instructions'] = instructions
        if greeting is not NOT_GIVEN:
            _body['greeting'] = greeting
        if model is not NOT_GIVEN:
            _body['model'] = model
        if voice is not NOT_GIVEN:
            _body['voice'] = voice
        if tools is not NOT_GIVEN:
            _body['tools'] = tools
        if autonomy is not NOT_GIVEN:
            _body['autonomy'] = autonomy
        if transfer_targets is not NOT_GIVEN:
            _body['transfer_targets'] = transfer_targets
        if business_hours is not NOT_GIVEN:
            _body['business_hours'] = business_hours
        if metadata is not NOT_GIVEN:
            _body['metadata'] = metadata
        return self._client.request("POST", f"/v1/agents", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Agent":
        """Retrieve an agent

        ``GET /v1/agents/{id}``
        """
        return self._client.request("GET", f"/v1/agents/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def update(self, id: str, *, name: str = NOT_GIVEN, instructions: str = NOT_GIVEN, greeting: str = NOT_GIVEN, model: str = NOT_GIVEN, voice: "StockVoice" = NOT_GIVEN, tools: List["AgentTool"] = NOT_GIVEN, autonomy: "Autonomy" = NOT_GIVEN, transfer_targets: List[str] = NOT_GIVEN, business_hours: Optional[Dict[str, Any]] = NOT_GIVEN, metadata: Dict[str, Any] = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Agent":
        """Update an agent

        Absent fields are unchanged; `business_hours: null` clears it; arrays and `metadata` replace the old value. The agent's account can't change.

        ``PATCH /v1/agents/{id}``
        """
        _body: Dict[str, Any] = {}
        if name is not NOT_GIVEN:
            _body['name'] = name
        if instructions is not NOT_GIVEN:
            _body['instructions'] = instructions
        if greeting is not NOT_GIVEN:
            _body['greeting'] = greeting
        if model is not NOT_GIVEN:
            _body['model'] = model
        if voice is not NOT_GIVEN:
            _body['voice'] = voice
        if tools is not NOT_GIVEN:
            _body['tools'] = tools
        if autonomy is not NOT_GIVEN:
            _body['autonomy'] = autonomy
        if transfer_targets is not NOT_GIVEN:
            _body['transfer_targets'] = transfer_targets
        if business_hours is not NOT_GIVEN:
            _body['business_hours'] = business_hours
        if metadata is not NOT_GIVEN:
            _body['metadata'] = metadata
        return self._client.request("PATCH", f"/v1/agents/{_q(id)}", body=_body, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def delete(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Delete an agent

        ``DELETE /v1/agents/{id}``
        """
        return self._client.request("DELETE", f"/v1/agents/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_knowledge_files(self, id: str, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """List an agent's knowledge files

        Oldest first. Requires the `agents` scope; a key bound to an account sees only its own account's agents.

        ``GET /v1/agents/{id}/knowledge``
        """
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        return self._client.request("GET", f"/v1/agents/{_q(id)}/knowledge", query=_query, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_knowledge_files_all(self, id: str, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Iterator["KnowledgeFile"]:
        """Every item of ``list_knowledge_files``, fetching pages as you iterate."""
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        def _fetch(cursor: Optional[str]) -> Page:
            q = dict(_query)
            if cursor is not None:
                q['after'] = cursor
            return self._client.request("GET", f"/v1/agents/{_q(id)}/knowledge", query=q, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]
        return self._client.paginate(_fetch, after if after is not NOT_GIVEN else None)

    def upload_knowledge_file(self, id: str, *, name: str, content_type: Literal['text/plain', 'text/markdown', 'text/csv', 'text/html', 'application/json'] = NOT_GIVEN, content: str = NOT_GIVEN, content_base64: str = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "KnowledgeFile":
        """Upload a knowledge file

        Text files the agent searches with the knowledge_search tool. Types: text/plain, text/markdown, text/csv, text/html, application/json (UTF-8). At most 1 MB per file, 20 files and 5 MB in total per agent. Send the text as `content`, or the bytes as `content_base64`. `content_type` defaults from the name's extension.

        ``POST /v1/agents/{id}/knowledge``
        """
        _body: Dict[str, Any] = {}
        if name is not NOT_GIVEN:
            _body['name'] = name
        if content_type is not NOT_GIVEN:
            _body['content_type'] = content_type
        if content is not NOT_GIVEN:
            _body['content'] = content
        if content_base64 is not NOT_GIVEN:
            _body['content_base64'] = content_base64
        return self._client.request("POST", f"/v1/agents/{_q(id)}/knowledge", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def delete_knowledge_file(self, id: str, file_id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Delete a knowledge file

        The agent stops finding it right away; the stored original is removed.

        ``DELETE /v1/agents/{id}/knowledge/{file_id}``
        """
        return self._client.request("DELETE", f"/v1/agents/{_q(id)}/knowledge/{_q(file_id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_model_keys(self, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """List the project's model keys

        Masked; the key itself is never returned.

        ``GET /v1/model_keys``
        """
        return self._client.request("GET", f"/v1/model_keys", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def put_model_key(self, provider: str, *, api_key: str, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "ModelKey":
        """Set a model key

        The project's own key for a provider. Agents whose `model` is `provider/…` run on it. Not available to account-bound keys.

        ``PUT /v1/model_keys/{provider}``
        """
        _body: Dict[str, Any] = {}
        if api_key is not NOT_GIVEN:
            _body['api_key'] = api_key
        return self._client.request("PUT", f"/v1/model_keys/{_q(provider)}", body=_body, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def delete_model_key(self, provider: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Delete a model key

        ``DELETE /v1/model_keys/{provider}``
        """
        return self._client.request("DELETE", f"/v1/model_keys/{_q(provider)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]


class ConversationsResource:
    def __init__(self, client: Transport) -> None:
        self._client = client

    def list(self, id: str, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """List an agent's conversations

        The agent's conversations, oldest first, without their messages (use getConversation for those). A key bound to an account sees only that account's agent. Requires the `agents` scope.

        ``GET /v1/agents/{id}/conversations``
        """
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        return self._client.request("GET", f"/v1/agents/{_q(id)}/conversations", query=_query, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_all(self, id: str, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Iterator["Conversation"]:
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
            return self._client.request("GET", f"/v1/agents/{_q(id)}/conversations", query=q, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]
        return self._client.paginate(_fetch, after if after is not NOT_GIVEN else None)

    def create(self, id: str, *, metadata: Dict[str, Any] = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Conversation":
        """Start a conversation

        Opens a conversation with the agent, in the agent's account. No message is sent yet.

        ``POST /v1/agents/{id}/conversations``
        """
        _body: Dict[str, Any] = {}
        if metadata is not NOT_GIVEN:
            _body['metadata'] = metadata
        return self._client.request("POST", f"/v1/agents/{_q(id)}/conversations", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Conversation":
        """Retrieve a conversation

        The conversation with its latest 200 messages, oldest first.

        ``GET /v1/conversations/{id}``
        """
        return self._client.request("GET", f"/v1/conversations/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def send_message(self, id: str, *, content: str, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "ConversationMessage":
        """Send a message

        Sends a message and answers with the agent's reply. With `stream: true` the answer is
        server-sent events: `message.delta` (`{"delta": "..."}`) while the agent writes, then
        `message.completed` (the stored reply) or `error`. The first message starts the
        project's hosted runtime: `503 runtime_starting` means retry in a few seconds.
        One message at a time per conversation (`409 conversation_busy`).

        ``POST /v1/conversations/{id}/messages``
        """
        _body: Dict[str, Any] = {}
        if content is not NOT_GIVEN:
            _body['content'] = content
        return self._client.request("POST", f"/v1/conversations/{_q(id)}/messages", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def send_message_stream(self, id: str, *, content: str, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Iterator[SSEEvent]:
        """``send_message`` with ``stream: true``: yields the raw server-sent events."""
        _body: Dict[str, Any] = {}
        if content is not NOT_GIVEN:
            _body['content'] = content
        _body['stream'] = True
        return self._client.stream_request("POST", f"/v1/conversations/{_q(id)}/messages", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)


class UsageResource:
    def __init__(self, client: Transport) -> None:
        self._client = client

    def get(self, *, group_by: Literal['meter', 'key', 'account'] = NOT_GIVEN, from_: str = NOT_GIVEN, to: str = NOT_GIVEN, account_id: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Usage by meter, key or account

        Sums of recorded usage events in `[from, to)`. Requires the `usage` scope.

        ``GET /v1/usage``
        """
        _query: Dict[str, Any] = {}
        if group_by is not NOT_GIVEN:
            _query['group_by'] = group_by
        if from_ is not NOT_GIVEN:
            _query['from'] = from_
        if to is not NOT_GIVEN:
            _query['to'] = to
        if account_id is not NOT_GIVEN:
            _query['account_id'] = account_id
        return self._client.request("GET", f"/v1/usage", query=_query, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]


class NumbersResource:
    def __init__(self, client: Transport) -> None:
        self._client = client

    def list_available_numbers(self, *, country: str = NOT_GIVEN, area_code: str = NOT_GIVEN, locality: str = NOT_GIVEN, type: Literal['local', 'toll_free'] = NOT_GIVEN, limit: int = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Search numbers to buy (live projects)

        ``GET /v1/numbers/available``
        """
        _query: Dict[str, Any] = {}
        if country is not NOT_GIVEN:
            _query['country'] = country
        if area_code is not NOT_GIVEN:
            _query['area_code'] = area_code
        if locality is not NOT_GIVEN:
            _query['locality'] = locality
        if type is not NOT_GIVEN:
            _query['type'] = type
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        return self._client.request("GET", f"/v1/numbers/available", query=_query, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list(self, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, account_id: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "PhoneNumberList":
        """List numbers

        ``GET /v1/numbers``
        """
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        if account_id is not NOT_GIVEN:
            _query['account_id'] = account_id
        return self._client.request("GET", f"/v1/numbers", query=_query, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_all(self, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, account_id: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Iterator["PhoneNumber"]:
        """Every item of ``list``, fetching pages as you iterate."""
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        if account_id is not NOT_GIVEN:
            _query['account_id'] = account_id
        def _fetch(cursor: Optional[str]) -> Page:
            q = dict(_query)
            if cursor is not None:
                q['after'] = cursor
            return self._client.request("GET", f"/v1/numbers", query=q, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]
        return self._client.paginate(_fetch, after if after is not NOT_GIVEN else None)

    def create(self, *, account_id: str, e164: str = NOT_GIVEN, type: Literal['local', 'toll_free'] = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "PhoneNumber":
        """Get a number for an account

        Sandbox projects get a simulated number (no `e164`). Live projects buy `e164` from `/v1/numbers/available`; needs a paid plan.

        ``POST /v1/numbers``
        """
        _body: Dict[str, Any] = {}
        if account_id is not NOT_GIVEN:
            _body['account_id'] = account_id
        if e164 is not NOT_GIVEN:
            _body['e164'] = e164
        if type is not NOT_GIVEN:
            _body['type'] = type
        return self._client.request("POST", f"/v1/numbers", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "PhoneNumber":
        """Retrieve a number

        ``GET /v1/numbers/{id}``
        """
        return self._client.request("GET", f"/v1/numbers/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def release(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Release a number

        ``DELETE /v1/numbers/{id}``
        """
        return self._client.request("DELETE", f"/v1/numbers/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get_registration(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Carrier registration status

        ``GET /v1/numbers/{id}/registration``
        """
        return self._client.request("GET", f"/v1/numbers/{_q(id)}/registration", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def submit_registration(self, id: str, *, body: Optional[Dict[str, Any]] = None, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Register the number for US texting (10DLC or toll-free)

        The end-customer business's own details. The campaign files itself once the brand is verified.

        ``POST /v1/numbers/{id}/registration``
        """
        _body: Dict[str, Any] = dict(body or {})
        return self._client.request("POST", f"/v1/numbers/{_q(id)}/registration", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def verify_registration_code(self, id: str, *, pin: str = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Sole proprietors — verify the texted code, or send a new one

        `{ "pin": "123456" }` checks the code the person received; an empty body sends a new code. Once verified, the campaign files itself.

        ``POST /v1/numbers/{id}/registration/otp``
        """
        _body: Dict[str, Any] = {}
        if pin is not NOT_GIVEN:
            _body['pin'] = pin
        return self._client.request("POST", f"/v1/numbers/{_q(id)}/registration/otp", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def record_consent(self, id: str, *, e164: str, source: str, evidence: str = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Record that a person agreed to be contacted

        ``POST /v1/numbers/{id}/consent``
        """
        _body: Dict[str, Any] = {}
        if e164 is not NOT_GIVEN:
            _body['e164'] = e164
        if source is not NOT_GIVEN:
            _body['source'] = source
        if evidence is not NOT_GIVEN:
            _body['evidence'] = evidence
        return self._client.request("POST", f"/v1/numbers/{_q(id)}/consent", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def simulate_inbound(self, id: str, *, from_: str, body: str, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Sandbox only — play a text arriving

        ``POST /v1/numbers/{id}/simulate_inbound``
        """
        _body: Dict[str, Any] = {}
        if from_ is not NOT_GIVEN:
            _body['from'] = from_
        if body is not NOT_GIVEN:
            _body['body'] = body
        return self._client.request("POST", f"/v1/numbers/{_q(id)}/simulate_inbound", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]


class MessagingResource:
    def __init__(self, client: Transport) -> None:
        self._client = client

    def list_messages(self, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, number_id: str = NOT_GIVEN, account_id: str = NOT_GIVEN, direction: Literal['inbound', 'outbound'] = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "MessageList":
        """List texts

        ``GET /v1/messages``
        """
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        if number_id is not NOT_GIVEN:
            _query['number_id'] = number_id
        if account_id is not NOT_GIVEN:
            _query['account_id'] = account_id
        if direction is not NOT_GIVEN:
            _query['direction'] = direction
        return self._client.request("GET", f"/v1/messages", query=_query, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_messages_all(self, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, number_id: str = NOT_GIVEN, account_id: str = NOT_GIVEN, direction: Literal['inbound', 'outbound'] = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Iterator["Message"]:
        """Every item of ``list_messages``, fetching pages as you iterate."""
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        if number_id is not NOT_GIVEN:
            _query['number_id'] = number_id
        if account_id is not NOT_GIVEN:
            _query['account_id'] = account_id
        if direction is not NOT_GIVEN:
            _query['direction'] = direction
        def _fetch(cursor: Optional[str]) -> Page:
            q = dict(_query)
            if cursor is not None:
                q['after'] = cursor
            return self._client.request("GET", f"/v1/messages", query=q, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]
        return self._client.paginate(_fetch, after if after is not NOT_GIVEN else None)

    def send_message(self, *, number_id: str, to: str, body: str, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Message":
        """Send a text

        Refused when texting isn't active on the number, the person sent STOP, or they never texted first and have no consent recorded.

        ``POST /v1/messages``
        """
        _body: Dict[str, Any] = {}
        if number_id is not NOT_GIVEN:
            _body['number_id'] = number_id
        if to is not NOT_GIVEN:
            _body['to'] = to
        if body is not NOT_GIVEN:
            _body['body'] = body
        return self._client.request("POST", f"/v1/messages", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get_message(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Message":
        """Retrieve a text

        ``GET /v1/messages/{id}``
        """
        return self._client.request("GET", f"/v1/messages/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]


class WebhooksResource:
    def __init__(self, client: Transport) -> None:
        self._client = client

    def list(self, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """List webhook endpoints

        ``GET /v1/webhooks``
        """
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        return self._client.request("GET", f"/v1/webhooks", query=_query, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_all(self, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Iterator["WebhookEndpoint"]:
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
            return self._client.request("GET", f"/v1/webhooks", query=q, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]
        return self._client.paginate(_fetch, after if after is not NOT_GIVEN else None)

    def create(self, *, url: str, events: List[Literal['*', 'message.received', 'message.status', 'registration.updated']], description: str = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Create a webhook endpoint

        `url` must be public https. The response carries `secret` once.

        ``POST /v1/webhooks``
        """
        _body: Dict[str, Any] = {}
        if url is not NOT_GIVEN:
            _body['url'] = url
        if events is not NOT_GIVEN:
            _body['events'] = events
        if description is not NOT_GIVEN:
            _body['description'] = description
        return self._client.request("POST", f"/v1/webhooks", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "WebhookEndpoint":
        """Retrieve a webhook endpoint

        ``GET /v1/webhooks/{id}``
        """
        return self._client.request("GET", f"/v1/webhooks/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def delete(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Delete a webhook endpoint

        ``DELETE /v1/webhooks/{id}``
        """
        return self._client.request("DELETE", f"/v1/webhooks/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def test(self, id: str, *, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Send a webhook.test event to this endpoint

        ``POST /v1/webhooks/{id}/test``
        """
        return self._client.request("POST", f"/v1/webhooks/{_q(id)}/test", idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_deliveries(self, id: str, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Delivery log for an endpoint

        ``GET /v1/webhooks/{id}/deliveries``
        """
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        return self._client.request("GET", f"/v1/webhooks/{_q(id)}/deliveries", query=_query, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_deliveries_all(self, id: str, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Iterator[Dict[str, Any]]:
        """Every item of ``list_deliveries``, fetching pages as you iterate."""
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        def _fetch(cursor: Optional[str]) -> Page:
            q = dict(_query)
            if cursor is not None:
                q['after'] = cursor
            return self._client.request("GET", f"/v1/webhooks/{_q(id)}/deliveries", query=q, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]
        return self._client.paginate(_fetch, after if after is not NOT_GIVEN else None)

    def redeliver(self, id: str, delivery_id: str, *, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Try a delivery again now

        ``POST /v1/webhooks/{id}/deliveries/{delivery_id}/redeliver``
        """
        return self._client.request("POST", f"/v1/webhooks/{_q(id)}/deliveries/{_q(delivery_id)}/redeliver", idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]


def _q(v: str) -> str:
    from urllib.parse import quote
    return quote(str(v), safe='')


class GeneratedResources:
    """Every API area as a client attribute. ``AllternitPlatform`` extends this."""

    def __init__(self, client: Transport) -> None:
        self.accounts = AccountsResource(client)
        self.agents = AgentsResource(client)
        self.conversations = ConversationsResource(client)
        self.usage = UsageResource(client)
        self.numbers = NumbersResource(client)
        self.messaging = MessagingResource(client)
        self.webhooks = WebhooksResource(client)
